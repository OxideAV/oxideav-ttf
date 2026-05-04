//! End-to-end integration test loading DejaVu Sans Mono 2.37 +
//! DejaVu Sans 2.37. The Mono variant exercises CJK-style cmap +
//! basic glyph extraction; the Sans variant exercises GPOS pair
//! kerning + GSUB ligatures (Mono has neither — monospace fonts
//! historically don't ship pair-kerning lookups).

use oxideav_ttf::Font;

const FIXTURE: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");
const FIXTURE_SANS: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");

#[test]
fn parses_dejavu_sans_mono() {
    let f = Font::from_bytes(FIXTURE).expect("DejaVu parse");
    let family = f.family_name().expect("family");
    assert!(
        family.contains("DejaVu") && family.contains("Mono"),
        "unexpected family name: {family:?}"
    );
    assert_eq!(f.units_per_em(), 2048);
    assert!(
        f.glyph_count() > 3000,
        "expected > 3000 glyphs, got {}",
        f.glyph_count()
    );
    assert!(f.ascent() > 0);
    assert!(f.descent() < 0);
}

#[test]
fn glyph_lookup_and_outline_for_a() {
    let f = Font::from_bytes(FIXTURE).unwrap();
    let gid = f.glyph_index('A').expect("'A' must map");
    assert!(gid > 0);
    assert!(f.glyph_advance(gid) > 0);
    let outline = f.glyph_outline(gid).expect("outline");
    assert!(!outline.is_empty(), "'A' should have at least one contour");
    assert!(outline.bounds.is_some());
}

#[test]
fn glyph_index_for_basic_set() {
    let f = Font::from_bytes(FIXTURE).unwrap();
    for ch in "ABCMagWZ012!".chars() {
        let gid = f
            .glyph_index(ch)
            .unwrap_or_else(|| panic!("missing glyph for {ch:?}"));
        assert!(gid > 0, "got gid 0 for {ch:?}");
        // Every Latin glyph should have a positive advance.
        assert!(f.glyph_advance(gid) > 0, "non-positive advance for {ch:?}");
    }
}

/// Sanity-decode glyph outlines for a non-trivial set of glyphs to catch
/// any composite-glyph or flag-decoding regressions.
#[test]
fn many_outlines_decode() {
    let f = Font::from_bytes(FIXTURE).unwrap();
    let mut decoded = 0usize;
    for ch in 'A'..='z' {
        if let Some(gid) = f.glyph_index(ch) {
            let _ = f.glyph_outline(gid).expect("outline decode");
            decoded += 1;
        }
    }
    assert!(decoded >= 30, "decoded too few glyphs: {decoded}");
}

#[test]
fn fi_ligature_lookup_sans() {
    let f = Font::from_bytes(FIXTURE_SANS).unwrap();
    let f_gid = f.glyph_index('f').expect("'f' must map");
    let i_gid = f.glyph_index('i').expect("'i' must map");
    let fi_gid = f.glyph_index('\u{FB01}').expect("'fi' codepoint must map");
    // DejaVu Sans `fi` ligature should match `[f, i]` -> (fi_gid, 2).
    let hit = f
        .lookup_ligature(&[f_gid, i_gid])
        .expect("fi ligature must exist");
    assert_eq!(hit, (fi_gid, 2), "expected ({fi_gid}, 2), got {hit:?}");
}

/// DejaVu Sans publishes a `liga` feature for `latn` whose lookup is a
/// LookupType 4 (Ligature Substitution) covering the standard f-pair
/// ligatures. This exercises the lookup-index-specific apply path
/// (Font::gsub_apply_lookup_type_4) that a feature-driven shaper uses
/// after resolving the `liga` feature for the active script via
/// gsub_features_for_script.
#[test]
fn gsub_lookup_type_4_ligature_substitution_applies_for_fi_in_dejavu() {
    let f = Font::from_bytes(FIXTURE_SANS).unwrap();
    let f_gid = f.glyph_index('f').expect("'f' must map");
    let i_gid = f.glyph_index('i').expect("'i' must map");
    let fi_gid = f.glyph_index('\u{FB01}').expect("'fi' codepoint must map");

    // Resolve the `liga` feature for `latn` and try every lookup it
    // exposes; one of them should hit (fi_gid, 2).
    let feats = f.gsub_features_for_script(*b"latn", None);
    let mut got: Option<(u16, usize)> = None;
    for feat in &feats {
        if &feat.tag == b"liga" {
            for &li in &feat.lookup_indices {
                if let Some(hit) = f.gsub_apply_lookup_type_4(li, &[f_gid, i_gid]) {
                    got = Some(hit);
                    break;
                }
            }
        }
    }
    let (sub, consumed) = got.expect("liga feature should match [f, i]");
    assert_eq!(sub, fi_gid, "fi ligature should produce fi codepoint glyph");
    assert_eq!(consumed, 2, "fi ligature should consume both input glyphs");
}

/// 2-glyph ligatures (the f-i / f-l pairs in DejaVu Sans) all return
/// consumed=2; verify the `consumed` count in the public API contract.
#[test]
fn gsub_lookup_type_4_returns_consumed_count_2_for_2_glyph_ligature() {
    let f = Font::from_bytes(FIXTURE_SANS).unwrap();
    let f_gid = f.glyph_index('f').expect("'f' must map");
    let l_gid = f.glyph_index('l').expect("'l' must map");
    let fl_gid = f.glyph_index('\u{FB02}').expect("'fl' codepoint must map");

    // The lookup_ligature walker resolves the same lookup; assert
    // both API paths agree on the 2-glyph consumption count.
    let walked = f
        .lookup_ligature(&[f_gid, l_gid])
        .expect("fl ligature must exist via the global walker");
    assert_eq!(walked, (fl_gid, 2));

    let feats = f.gsub_features_for_script(*b"latn", None);
    for feat in &feats {
        if &feat.tag == b"liga" {
            for &li in &feat.lookup_indices {
                if let Some((sub, consumed)) = f.gsub_apply_lookup_type_4(li, &[f_gid, l_gid]) {
                    assert_eq!(sub, fl_gid);
                    assert_eq!(consumed, 2);
                    return;
                }
            }
        }
    }
    panic!("no liga lookup matched [f, l] via gsub_apply_lookup_type_4");
}

#[test]
fn av_kerning_is_negative_sans() {
    let f = Font::from_bytes(FIXTURE_SANS).unwrap();
    let a_gid = f.glyph_index('A').expect("'A' must map");
    let v_gid = f.glyph_index('V').expect("'V' must map");
    let kern = f.lookup_kerning(a_gid, v_gid);
    assert!(
        kern < 0,
        "AV kerning should be negative, got {kern} (pair: A={a_gid} V={v_gid})"
    );
}

/// Sanity check: monospace-mono variant has no pair kerning, so a
/// lookup should return 0 (and not crash).
#[test]
fn mono_has_no_pair_kerning() {
    let f = Font::from_bytes(FIXTURE).unwrap();
    let a = f.glyph_index('A').unwrap();
    let v = f.glyph_index('V').unwrap();
    assert_eq!(f.lookup_kerning(a, v), 0);
}

/// DejaVu Sans is an outline-only font — no COLR/CPAL/sbix/CBDT —
/// so every colour-glyph API should report empty / `false` rather
/// than crash.
#[test]
fn outline_only_font_has_no_color_tables() {
    let f = Font::from_bytes(FIXTURE_SANS).unwrap();
    assert!(!f.has_color_layers());
    assert!(!f.has_color_bitmaps());
    assert!(!f.has_sbix());
    let a = f.glyph_index('A').unwrap();
    assert!(f.color_layers(a).is_empty());
    assert!(f.cpal_color(0, 0).is_none());
    assert!(f.cpal_palette(0).is_none());
    assert_eq!(f.cpal_num_palettes(), 0);
    assert_eq!(f.cpal_palette_type(0), 0);
    assert!(f.sbix_strikes().is_empty());
    assert!(f.sbix_glyph(a, 32).is_none());
}
