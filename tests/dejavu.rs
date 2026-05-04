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
fn outline_only_font_has_no_color_layers() {
    let f = Font::from_bytes(FIXTURE_SANS).unwrap();
    assert!(!f.has_color_layers());
    assert!(!f.has_color_bitmaps());
    let a = f.glyph_index('A').unwrap();
    assert!(f.color_layers(a).is_empty());
    assert!(f.cpal_color(0, 0).is_none());
    assert!(f.cpal_palette(0).is_none());
    assert_eq!(f.cpal_num_palettes(), 0);
    assert_eq!(f.cpal_palette_type(0), 0);
}
