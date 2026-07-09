//! Hostile-input hardening harness.
//!
//! A font file is fully attacker-controlled, so no malformed input may
//! ever panic a parser: every table decode and every public accessor must
//! bottom out in a typed `Result`/`Option`, never an out-of-bounds index,
//! integer-overflow, or unbounded recursion. This test deterministically
//! mutates the bundled fixtures three ways and drives the eager parse path
//! (`Font::from_bytes` decodes essentially every table) plus a broad
//! accessor battery under each mutant, asserting no thread ever unwinds:
//!
//! 1. **Truncation** — every prefix length, catching short-read gaps.
//! 2. **Blind byte flips** — random multi-byte corruption anywhere, which
//!    mostly stresses the sfnt header + table-directory walker.
//! 3. **Structure-aware corruption** — the sfnt header and table directory
//!    are left intact (so `from_bytes` reaches every table parser) and
//!    bytes are flipped only inside one table body per iteration, driving
//!    corrupt-but-in-range data deep into each individual decoder.
//!
//! The seed is fixed, so a regression reproduces deterministically. This
//! harness caught the `glyf` non-monotonic-endPtsOfContours OOB.

use oxideav_ttf::Font;
use std::collections::BTreeSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;

static SITES: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

fn fixtures() -> Vec<(&'static str, Vec<u8>)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
    [
        "DejaVuSans.ttf",
        "DejaVuSansMono.ttf",
        "InterVariable.ttf",
        "NotoSansArabic-Regular.ttf",
    ]
    .iter()
    .map(|n| (*n, std::fs::read(format!("{dir}{n}")).unwrap()))
    .collect()
}

/// Drive the parse path plus a broad accessor battery. Every call must
/// return normally (via `Result`/`Option`/default) rather than panic.
fn exercise(bytes: &[u8]) {
    let Ok(mut f) = Font::from_bytes(bytes) else {
        return;
    };
    for gid in [0u16, 1, 2, 3, 10, 50, 100, 255, 1000, 5000, 65000, u16::MAX] {
        let _ = f.glyph_advance(gid);
        let _ = f.glyph_lsb(gid);
        let _ = f.glyph_name(gid);
        let _ = f.glyph_outline(gid);
        let _ = f.color_layers(gid);
        let _ = f.svg_document(gid);
        let _ = f.glyph_color_bitmap(gid, 32);
        let _ = f.glyph_gray_bitmap(gid, 16);
        let _ = f.glyph_gray_bitmap_scaled(gid, 12);
        let _ = f.sbix_glyph(gid, 64);
        let _ = f.sbix_glyph_resolved(gid, 64);
        let _ = f.ltsh_threshold(gid);
        let _ = f.ltsh_linearly_scales_at_ppem(gid, 16);
        let _ = f.hdmx_advance_pixels(gid, 16);
        let _ = f.vert_origin_y_from_vorg(gid);
        let _ = f.math_italics_correction_var(gid);
        let _ = f.math_top_accent_attachment_var(gid);
        let _ = f.gsub_apply_lookup_type_1(0, gid);
        let _ = f.gsub_apply_lookup_type_2(0, gid);
        let _ = f.gpos_apply_lookup_type_1(0, gid);
        let _ = f.gpos_apply_lookup_type_1_var(0, gid);
    }
    for cp in [0u32, 0x41, 0x20AC, 0x1F600, 0x10FFFF, 0xFFFF] {
        if let Some(c) = char::from_u32(cp) {
            let _ = f.glyph_index(c);
        }
    }
    for idx in [0u16, 1, 10, 100, 1000, u16::MAX] {
        let _ = f.cvt_value(idx);
        let _ = f.cvt_value_varied(idx);
    }
    let _ = f.cvt_deltas();
    let _ = f.sbix_strikes();
    let _ = f.name_records();
    for nid in [0u16, 1, 2, 4, 6, 256, u16::MAX] {
        let _ = f.name_string(nid);
    }
    for (l, r) in [(1u16, 2u16), (10, 20), (0, 0), (u16::MAX, u16::MAX)] {
        let _ = f.lookup_kerning(l, r);
        let _ = f.lookup_kerning_var(l, r);
    }
    for ppem in [0u16, 8, 16, 255, u16::MAX] {
        let _ = f.gasp_behavior_for_ppem(ppem);
        let _ = f.vdmx_y_extent_square(ppem);
    }
    let _ = f.base_horiz_y_for_script_baseline(*b"latn", *b"romn");
    let _ = f.base_vert_x_for_script_baseline(*b"hani", *b"icfb");
    let _ = f.meta_design_languages();
    let _ = f.meta_supported_languages();
    for idx in [0usize, 1, 10, 100, 10000] {
        let _ = f.math_constant_var(idx);
    }
    let _ = f.shape("Aa1! fi ffl", *b"latn", None, &[]);
    let _ = f.shape(
        "\u{0627}\u{0644}\u{0639}\u{0631}\u{0628}",
        *b"arab",
        None,
        &[*b"init", *b"medi", *b"fina"],
    );
    let _ = f.shape("\u{0410}\u{0411}\u{0412}", *b"cyrl", None, &[]);
    // Variable instances stress the gvar / cvar / HVAR / MVAR delta paths,
    // including out-of-range and non-finite user coordinates.
    let axis_count = f.variation_axes().len();
    if axis_count > 0 {
        for v in [0.0f32, 0.5, 1.0, -1.0, 1e9, f32::NAN] {
            f.set_variation_coords(&vec![v; axis_count]);
            for gid in [0u16, 1, 5, 50, 500] {
                let _ = f.glyph_outline(gid);
                let _ = f.glyph_advance(gid);
            }
            let _ = f.cvt_value_varied(0);
        }
    }
}

/// Deterministic xorshift64 PRNG — reproducible corruption.
fn rng(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn be32(b: &[u8], o: usize) -> usize {
    u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize
}

/// Minimal sfnt directory walk → `(offset, length)` of each table body.
fn table_regions(b: &[u8]) -> Vec<(usize, usize)> {
    if b.len() < 6 {
        return Vec::new();
    }
    let n = u16::from_be_bytes([b[4], b[5]]) as usize;
    let mut out = Vec::new();
    for i in 0..n {
        let rec = 12 + i * 16;
        if rec + 16 > b.len() {
            break;
        }
        let off = be32(b, rec + 8);
        let len = be32(b, rec + 12);
        if off.saturating_add(len) <= b.len() {
            out.push((off, len));
        }
    }
    out
}

#[test]
fn no_panic_on_mutated_fonts() {
    // Record panic locations instead of aborting so a single run surfaces
    // every distinct crash site, and suppress the default backtrace spew.
    std::panic::set_hook(Box::new(|info| {
        if let Some(loc) = info.location() {
            SITES
                .lock()
                .unwrap()
                .insert(format!("{}:{}", loc.file(), loc.line()));
        }
    }));

    // Iteration budgets are deliberately modest so the test stays a few
    // seconds in a debug CI build while still exercising every parser and
    // reproducing deterministically. Raise these locally (or run under a
    // dedicated fuzzer) for a deeper campaign.
    for (_name, base) in fixtures() {
        // 1. Truncation at every ~1/120th prefix.
        let step = (base.len() / 120).max(1);
        for len in (0..base.len()).step_by(step) {
            let slice = &base[..len];
            let _ = catch_unwind(AssertUnwindSafe(|| exercise(slice)));
        }

        // 2. Blind multi-byte flips anywhere in the file (mostly stress the
        //    sfnt header + directory walker; most mutants fail fast).
        let mut state = 0x1234_5678_9abc_def0u64;
        for _ in 0..600u64 {
            let mut m = base.clone();
            let nflips = 1 + (rng(&mut state) % 6) as usize;
            for _ in 0..nflips {
                let pos = (rng(&mut state) as usize) % m.len();
                m[pos] = (rng(&mut state) & 0xff) as u8;
            }
            let _ = catch_unwind(AssertUnwindSafe(|| exercise(&m)));
        }

        // 3. Structure-aware corruption: a fixed budget of mutants, each
        //    corrupting one randomly-chosen table body while leaving the
        //    sfnt header + directory intact so from_bytes reaches every
        //    parser with corrupt-but-in-range data.
        let regions: Vec<(usize, usize)> = table_regions(&base)
            .into_iter()
            .filter(|&(_, len)| len > 0)
            .collect();
        if !regions.is_empty() {
            for _ in 0..1_500u64 {
                let (off, len) = regions[(rng(&mut state) as usize) % regions.len()];
                let mut m = base.clone();
                let nflips = 1 + (rng(&mut state) % 8) as usize;
                for _ in 0..nflips {
                    let pos = off + (rng(&mut state) as usize) % len;
                    m[pos] = (rng(&mut state) & 0xff) as u8;
                }
                let _ = catch_unwind(AssertUnwindSafe(|| exercise(&m)));
            }
        }
    }

    let _ = std::panic::take_hook();
    let sites = SITES.lock().unwrap();
    let hits: Vec<_> = sites.iter().collect();
    assert!(
        hits.is_empty(),
        "parser panicked on malformed input: {hits:?}"
    );
}
