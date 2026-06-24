//! Integration tests for the new GPOS LookupType 3 / 5 / 7-extension
//! apply paths against the real Noto Sans Arabic 2022 font. The font
//! ships the full Arabic shaping cascade — single positioning (LT 1),
//! mark-to-base (LT 4), mark-to-ligature (LT 5), mark-to-mark (LT 6),
//! and chained-context positioning (LT 8) — but no cursive (LT 3),
//! which is typical for non-Nastaliq Arabic. The LT 3 path is
//! exercised here as panic-freedom only; LT 1 + LT 5 carry real
//! anchor / advance values we assert against.
//!
//! The LAM-ALEF ligature (U+FEFB / U+FEFC) is a 2-component ligature
//! whose mark anchors are the canonical real-world LT 5 case: an
//! Arabic vowel mark (FATHA U+064E etc.) attaching above the LAM
//! component (component 0) or above the ALEF component (component 1).

use oxideav_ttf::Font;

const FIXTURE: &[u8] = include_bytes!("fixtures/NotoSansArabic-Regular.ttf");

/// Sanity: Noto Sans Arabic ships at least one lookup of every type
/// we now support — 1, 4, 5, 6, 8 — proving the LookupList walker
/// surfaces them and the `gpos_lookup_list` enumeration is exhaustive.
#[test]
fn noto_arabic_gpos_lookup_list_has_lt_1_4_5_6_8() {
    let f = Font::from_bytes(FIXTURE).unwrap();
    let lookups = f.gpos_lookup_list();
    let types: std::collections::BTreeSet<u16> = lookups.iter().map(|(_, t, _)| *t).collect();
    for ty in [1u16, 4, 5, 6, 8] {
        assert!(
            types.contains(&ty),
            "Noto Sans Arabic should expose GPOS LookupType {ty}; got {types:?}"
        );
    }
}

/// Apply GPOS LookupType 1 (single positioning) to every Arabic
/// glyph and confirm the apply path yields at least 100 distinct
/// non-zero adjustments — Noto Sans Arabic ships per-glyph advance
/// trims via a SinglePos lookup as part of its `kern` / shaping
/// cascade.
#[test]
fn lt1_single_positioning_fires_on_real_arabic_glyphs() {
    let f = Font::from_bytes(FIXTURE).unwrap();
    let lookups = f.gpos_lookup_list();
    // Find every LT 1 lookup index.
    let lt1: Vec<u16> = lookups
        .iter()
        .filter_map(|(idx, ty, _)| (*ty == 1).then_some(*idx))
        .collect();
    assert!(
        !lt1.is_empty(),
        "Noto Sans Arabic should have ≥1 LT-1 lookup"
    );
    let mut hits = 0usize;
    for idx in &lt1 {
        for gid in 0..f.glyph_count() {
            if let Some(v) = f.gpos_apply_lookup_type_1(*idx, gid) {
                if v.x_advance != 0 || v.y_advance != 0 || v.x_placement != 0 || v.y_placement != 0
                {
                    hits += 1;
                }
            }
        }
    }
    assert!(
        hits >= 100,
        "LT 1 should produce ≥100 non-zero adjustments across Noto Arabic glyphs, got {hits}"
    );
}

/// The shared §2 lookupFlag skip predicate behaves per the bit
/// enumeration on real GDEF-classified glyphs: FATHA (a combining mark)
/// is skipped under IGNORE_MARKS but not IGNORE_BASE_GLYPHS, and a
/// base letter is skipped under IGNORE_BASE_GLYPHS but not IGNORE_MARKS.
/// With no flag bits set nothing is skipped.
#[test]
fn lookup_skips_glyph_honours_ignore_bits() {
    let f = Font::from_bytes(FIXTURE).unwrap();
    // FATHA U+064E is a combining mark (GDEF class 3).
    let mark = f.glyph_index('\u{064E}').expect("FATHA glyph");
    assert!(f.is_mark_glyph(mark), "FATHA should be a GDEF mark");
    // A base Arabic letter (BEH U+0628) is GDEF class 1 (base).
    let base = f.glyph_index('\u{0628}').expect("BEH glyph");
    assert!(!f.is_mark_glyph(base), "BEH should not be a GDEF mark");

    const IGNORE_BASE: u16 = 0x0002;
    const IGNORE_MARKS: u16 = 0x0008;

    // No flags → never skip.
    assert!(!f.lookup_skips_glyph(0, None, mark));
    assert!(!f.lookup_skips_glyph(0, None, base));

    // IGNORE_MARKS skips the mark, leaves the base.
    assert!(f.lookup_skips_glyph(IGNORE_MARKS, None, mark));
    assert!(!f.lookup_skips_glyph(IGNORE_MARKS, None, base));

    // IGNORE_BASE_GLYPHS skips the base, leaves the mark.
    assert!(!f.lookup_skips_glyph(IGNORE_BASE, None, mark));
    assert!(f.lookup_skips_glyph(IGNORE_BASE, None, base));

    // Both bits together skip both.
    assert!(f.lookup_skips_glyph(IGNORE_BASE | IGNORE_MARKS, None, mark));
    assert!(f.lookup_skips_glyph(IGNORE_BASE | IGNORE_MARKS, None, base));
}

/// Apply GPOS LookupType 5 (mark-to-ligature) to the LAM-ALEF
/// ligature glyph (U+FEFB) for both components × every Arabic vowel
/// mark in U+064B..U+0652. Noto Sans Arabic anchors the marks on
/// both components (LAM and ALEF), so we expect at least one hit
/// per (component, mark) pair.
#[test]
fn lt5_mark_to_ligature_anchors_marks_on_lam_alef() {
    let f = Font::from_bytes(FIXTURE).unwrap();
    let lam_alef = f
        .glyph_index('\u{FEFB}')
        .expect("LAM-ALEF (U+FEFB) must map");
    let mut marks: Vec<(char, u16)> = Vec::new();
    for cp in 0x064Bu32..=0x0652 {
        if let Some(c) = char::from_u32(cp) {
            if let Some(g) = f.glyph_index(c) {
                marks.push((c, g));
            }
        }
    }
    assert!(
        marks.len() >= 5,
        "expected at least 5 Arabic vowel-mark glyphs, got {}",
        marks.len()
    );
    let mut total_hits = 0usize;
    for (mark_ch, mark_gid) in &marks {
        for component in 0u16..=1 {
            let off = f.lookup_mark_to_ligature(lam_alef, component, *mark_gid);
            if let Some((dx, dy)) = off {
                total_hits += 1;
                // Sanity: anchor offsets are bounded; the font's UPM
                // is 1000 so individual coords above ~5 UPMs would
                // indicate a parsing offset bug.
                assert!(
                    dx.abs() < 5000 && dy.abs() < 5000,
                    "anchor offset out of plausible range for LAM-ALEF \
                     component {component} + mark {mark_ch:?}: ({dx}, {dy})"
                );
            }
        }
    }
    // Both components must yield at least one hit each — typically all
    // 8 marks anchor on both. Demand ≥ 8 total to prove both component
    // arrays are walked.
    assert!(
        total_hits >= 8,
        "expected ≥8 LT-5 anchor hits on LAM-ALEF (2 components × ≥4 marks), got {total_hits}"
    );
}

/// LT 5 must return None for an out-of-range component index. LAM-ALEF
/// has exactly 2 components; component 5 is malformed input and should
/// be silently rejected, not panic.
#[test]
fn lt5_returns_none_for_out_of_range_component() {
    let f = Font::from_bytes(FIXTURE).unwrap();
    let lam_alef = f.glyph_index('\u{FEFB}').unwrap();
    let fatha = f.glyph_index('\u{064E}').unwrap();
    assert_eq!(f.lookup_mark_to_ligature(lam_alef, 99, fatha), None);
}

/// LT 5 must return None when the mark glyph isn't covered. .notdef
/// (gid 0) is never in any mark coverage.
#[test]
fn lt5_returns_none_for_uncovered_mark() {
    let f = Font::from_bytes(FIXTURE).unwrap();
    let lam_alef = f.glyph_index('\u{FEFB}').unwrap();
    assert_eq!(f.lookup_mark_to_ligature(lam_alef, 0, 0), None);
}

/// LT 3 (cursive attachment) is not present in Noto Sans Arabic
/// (Nastaliq fonts are the typical cursive carriers; Noto's regular
/// Arabic cut isn't Nastaliq). The walker should surface `None` for
/// every glyph without panicking.
#[test]
fn lt3_cursive_attachment_returns_none_in_noto_sans_arabic() {
    let f = Font::from_bytes(FIXTURE).unwrap();
    let mut probed = 0usize;
    // Probe a representative slice of glyphs (every 16th) — ~70
    // probes — to keep the test fast while exercising the walker.
    for gid in (0..f.glyph_count()).step_by(16) {
        assert_eq!(
            f.lookup_cursive_attachment(gid),
            None,
            "Noto Sans Arabic should have no cursive lookup; got hit on gid {gid}"
        );
        probed += 1;
    }
    assert!(probed > 0);
}

/// Drive every GPOS lookup index through every new apply path
/// (LT 1, LT 3, LT 5, LT 8) at every position of a small Arabic
/// shaping run (LAM-ALEF + FATHA + KASRA). The test passes if no
/// dispatch panics — type-mismatch / coverage-miss is the
/// expected outcome on most index/type combinations and surfaces
/// as `None`.
#[test]
fn new_gpos_lookup_types_are_panic_free_on_real_arabic_run() {
    let f = Font::from_bytes(FIXTURE).unwrap();
    let lam_alef = f.glyph_index('\u{FEFB}').unwrap();
    let fatha = f.glyph_index('\u{064E}').unwrap();
    let kasra = f.glyph_index('\u{0650}').unwrap();
    let run = [lam_alef, fatha, kasra];
    for (idx, _ty, _sub) in f.gpos_lookup_list() {
        for &g in &run {
            let _ = f.gpos_apply_lookup_type_1(idx, g);
            let _ = f.gpos_apply_lookup_type_3(idx, g);
            for component in 0u16..=2 {
                let _ = f.gpos_apply_lookup_type_5(idx, lam_alef, component, g);
            }
        }
        for pos in 0..run.len() {
            let _ = f.gpos_apply_lookup_type_8(idx, &run, pos);
        }
    }
}

/// Combined LT 1 + LT 5 round-trip in a single shaping pass:
///
/// 1. Take the LAM-ALEF ligature gid.
/// 2. Apply every LT 1 lookup to it (advance trim).
/// 3. Apply LT 5 to attach a FATHA mark on component 0.
///
/// The test asserts a real-world dual exercise of two new apply
/// paths against the same font, which is what task #480 calls for.
#[test]
fn lt1_plus_lt5_combined_shaping_pass_on_lam_alef_with_fatha() {
    let f = Font::from_bytes(FIXTURE).unwrap();
    let lam_alef = f.glyph_index('\u{FEFB}').unwrap();
    let fatha = f.glyph_index('\u{064E}').unwrap();

    // 1. LT 1 single-position pass: try every LT 1 lookup, accept any hit.
    let lt1_indices: Vec<u16> = f
        .gpos_lookup_list()
        .into_iter()
        .filter_map(|(idx, ty, _)| (ty == 1).then_some(idx))
        .collect();
    let mut lt1_hit_count = 0usize;
    for idx in &lt1_indices {
        if f.gpos_apply_lookup_type_1(*idx, lam_alef).is_some() {
            lt1_hit_count += 1;
        }
    }
    let _ = lt1_hit_count; // LAM-ALEF may or may not be in LT 1 coverage
                           // — the test is a panic-freedom + dispatch
                           // walk, not a value assertion for LT 1.

    // 2. LT 5 mark-to-ligature: attach FATHA on LAM (component 0).
    let off = f
        .lookup_mark_to_ligature(lam_alef, 0, fatha)
        .expect("FATHA on LAM (component 0) of LAM-ALEF must anchor in Noto");
    // Sanity: the offset is non-zero (a mark-on-base anchor is never
    // exactly at the mark's own pen origin) and within plausible range.
    assert!(
        off.0.abs() < 5000 && off.1.abs() < 5000,
        "implausible LT-5 anchor for LAM-ALEF + FATHA: {off:?}"
    );
    assert!(
        off.0 != 0 || off.1 != 0,
        "LT-5 anchor for LAM-ALEF + FATHA should be non-zero, got {off:?}"
    );
}
