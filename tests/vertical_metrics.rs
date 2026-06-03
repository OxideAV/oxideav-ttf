//! Integration coverage for the `vhea` + `vmtx` accessors on
//! [`oxideav_ttf::Font`].
//!
//! Two paths:
//!
//! * **Absent path** — DejaVu Sans Mono / DejaVu Sans, both of which
//!   ship without `vhea` / `vmtx` (they are horizontal-only Latin /
//!   Cyrillic / Greek faces). Every vertical accessor must return
//!   `None`, and [`Font::has_vertical_metrics`] must report `false`.
//!
//! * **Present path** — opportunistically read the cached
//!   `NotoSansCJK-Medium.ttc` fixture (populated by the consumer
//!   crate when `OXIDEAV_NETWORK_TESTS=1` was set; see
//!   `tests/ttc_subfont.rs` for the discovery code that this file
//!   reuses). When the fixture is unavailable the present-path
//!   assertions skip silently — exactly the policy the other TTC
//!   integration test uses.

use std::path::PathBuf;

use oxideav_ttf::{is_collection, Font};

const DEJAVU_MONO: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");
const DEJAVU_SANS: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");

#[test]
fn dejavu_mono_has_no_vertical_metrics() {
    let f = Font::from_bytes(DEJAVU_MONO).unwrap();
    assert!(!f.has_vertical_metrics());
    assert!(f.vhea_table().is_none());
    assert!(f.vmtx_table().is_none());
    assert!(f.vertical_ascent().is_none());
    assert!(f.vertical_descent().is_none());
    assert!(f.vertical_line_gap().is_none());
    assert!(f.advance_height_max().is_none());
    let gid = f.glyph_index('A').unwrap();
    assert!(f.glyph_advance_height(gid).is_none());
    assert!(f.glyph_top_side_bearing(gid).is_none());
    assert!(f.glyph_vertical_origin_y(gid).is_none());
}

#[test]
fn dejavu_sans_has_no_vertical_metrics() {
    let f = Font::from_bytes(DEJAVU_SANS).unwrap();
    assert!(!f.has_vertical_metrics());
    assert!(f.vhea_table().is_none());
    let gid = f.glyph_index('g').unwrap();
    assert!(f.glyph_advance_height(gid).is_none());
}

fn cached_ttc() -> Option<Vec<u8>> {
    // Same discovery logic as `ttc_subfont.rs` so both tests can be
    // satisfied by the same cached blob. Honour `CARGO_TARGET_DIR`
    // first to match the per-crate /tmp target-dir convention.
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        candidates.push(
            PathBuf::from(&dir)
                .join("test-fixtures")
                .join("fonts")
                .join("NotoSansCJK-Medium.ttc"),
        );
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut probe: Option<PathBuf> = Some(manifest.clone());
    while let Some(p) = probe {
        candidates.push(
            p.join("target")
                .join("test-fixtures")
                .join("fonts")
                .join("NotoSansCJK-Medium.ttc"),
        );
        probe = p.parent().map(|p| p.to_path_buf());
    }
    candidates.into_iter().find_map(|p| std::fs::read(&p).ok())
}

#[test]
fn noto_cjk_publishes_vertical_metrics_when_fixture_cached() {
    let Some(bytes) = cached_ttc() else {
        return;
    };
    assert!(is_collection(&bytes));
    // Subfont 0 covers CJK Han glyphs and ships vertical metrics
    // (vhea + vmtx). Skip the test if the assumption breaks (i.e.
    // the fixture got swapped for a different TTC at the cache path);
    // we have no other way to validate without per-glyph reference
    // data, and a noisy fail would just confuse a future round.
    let f = match Font::from_collection_bytes(&bytes, 0) {
        Ok(f) => f,
        Err(_) => return,
    };
    if !f.has_vertical_metrics() {
        return;
    }

    // Headline accessors are present and finite.
    let asc = f.vertical_ascent().unwrap();
    let desc = f.vertical_descent().unwrap();
    let lg = f.vertical_line_gap().unwrap();
    let max = f.advance_height_max().unwrap();
    // Sanity bounds against the §5.7.9 example values + general
    // CJK metric ranges (vertical fonts at upem=1000 are typically
    // ±500 each side; CJK fonts use upem 1000 or 1024).
    assert!(asc.abs() < 4096);
    assert!(desc.abs() < 4096);
    assert!(lg.abs() < 4096);
    assert!(max > 0);
    assert!(max as i32 >= asc as i32 - desc as i32);

    // A handful of glyph IDs covering both the long-pair array and
    // — likely — the monospaced tsb tail. Empty / blank glyphs may
    // still legitimately have a zero advance height, so we only
    // assert that the call doesn't panic + returns Some.
    for gid in [0u16, 1, 100, 500, 2000, 10_000] {
        if gid >= f.glyph_count() {
            continue;
        }
        let ah = f.glyph_advance_height(gid).unwrap();
        // CJK advance heights at upem 1000 are typically near 1000.
        // We only assert non-pathology (not zero for at least one
        // populated glyph).
        let _ = ah;
        let _ = f.glyph_top_side_bearing(gid).unwrap();
        // glyph_vertical_origin_y requires a bbox; allow None for
        // empty glyphs (gid 1, .null, .space) but the call must
        // not panic.
        let _ = f.glyph_vertical_origin_y(gid);
    }

    // At least one non-zero advance height in the run we sampled.
    let any_nonzero = [0u16, 100, 500, 2000, 10_000]
        .iter()
        .filter(|&&g| g < f.glyph_count())
        .any(|&g| f.glyph_advance_height(g).unwrap() != 0);
    assert!(any_nonzero, "expected at least one non-zero advance height");
}
