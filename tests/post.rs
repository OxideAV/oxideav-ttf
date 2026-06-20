//! Integration coverage for the `post` table accessors.
//!
//! Validates the parsed `post` table against the real-font fixtures
//! checked into `tests/fixtures/`. DejaVu Sans and DejaVu Sans Mono
//! both publish a `post` v2.0 table; the test confirms the parser
//! decodes it end-to-end and that the per-glyph name accessors agree
//! with the file's `glyphNameIndex` map.
//!
//! Spec sources:
//! - `docs/text/opentype/spec/ISO_IEC_14496-22-OFF-2019.pdf` §5.2.10
//!   (`post` table header + v1.0 / v2.0 / v2.5 / v3.0 layouts).
//! - `docs/text/opentype/otspec-post.html` (the §5.2.10.2 v2.0
//!   worked example used in `tables::post` unit tests).

use oxideav_ttf::{Font, GlyphNameRef, PostFormat, POST_VERSION_20};

const DEJAVU_SANS: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");
const DEJAVU_MONO: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");

#[test]
fn dejavu_sans_publishes_post_v2_with_custom_names() {
    let font = Font::from_bytes(DEJAVU_SANS).unwrap();
    assert!(font.has_post(), "DejaVu Sans is expected to ship `post`");
    let post = font.post_table().expect("post table present");
    assert_eq!(post.version_raw, POST_VERSION_20);
    match &post.format {
        PostFormat::Version20(v) => {
            // numGlyphs in `post` should match `maxp.numGlyphs`.
            assert_eq!(v.num_glyphs, font.glyph_count());
            assert_eq!(v.glyph_name_indices.len(), font.glyph_count() as usize);
            assert!(
                !v.pascal_strings.is_empty(),
                "v2.0 font is expected to ship at least one Pascal string"
            );
            // No oversize or non-conformant names in a clean release.
            assert!(!v.has_oversize_glyph_name);
            assert!(!v.has_non_conformant_glyph_name);
        }
        other => panic!("expected post v2.0 in DejaVu Sans, got {other:?}"),
    }
}

#[test]
fn dejavu_sans_resolves_at_least_one_custom_glyph_name() {
    let font = Font::from_bytes(DEJAVU_SANS).unwrap();
    // The Font-level convenience accessor must return Some(name) for
    // at least one glyph (v2.0 fonts almost always carry a non-trivial
    // string pool).
    let mut custom_count = 0usize;
    let mut std_count = 0usize;
    for gid in 0..font.glyph_count() {
        match font.glyph_name_ref(gid) {
            Some(GlyphNameRef::Custom(_)) => custom_count += 1,
            Some(GlyphNameRef::StandardMac { .. }) => std_count += 1,
            None => {}
        }
    }
    assert!(
        custom_count > 0,
        "DejaVu Sans is expected to carry at least one Pascal glyph name"
    );
    // With the 258-name standard Macintosh table staged, the
    // Font::glyph_name() convenience now resolves *both* the Custom and
    // StandardMac branches, so its Some-count equals the total number
    // of glyphs that carry any name reference.
    let convenience_count: usize = (0..font.glyph_count())
        .filter(|gid| font.glyph_name(*gid).is_some())
        .count();
    assert_eq!(convenience_count, custom_count + std_count);
    // Every StandardMac reference must now resolve to a real name
    // through the convenience accessor (no more #1277-pending None).
    for gid in 0..font.glyph_count() {
        if let Some(GlyphNameRef::StandardMac { index }) = font.glyph_name_ref(gid) {
            let name = font
                .glyph_name(gid)
                .expect("StandardMac glyph name must resolve");
            assert_eq!(name, oxideav_ttf::standard_mac_glyph_name(index).unwrap());
        }
    }
}

#[test]
fn dejavu_sans_post_metrics_are_sane() {
    let font = Font::from_bytes(DEJAVU_SANS).unwrap();
    let post = font.post_table().unwrap();
    // DejaVu Sans is upright + proportionally spaced.
    assert!((post.italic_angle).abs() < 0.5);
    assert!(!post.is_fixed_pitch);
    // Underline thickness is a positive small integer (in font units).
    assert!(post.underline_thickness > 0);
}

#[test]
fn dejavu_mono_post_publishes_glyph_names() {
    let font = Font::from_bytes(DEJAVU_MONO).unwrap();
    let post = font.post_table().unwrap();
    assert!(post.has_glyph_names());
    // DejaVu Sans Mono is monospaced.
    assert!(post.is_fixed_pitch);
}

#[test]
fn glyph_name_for_out_of_range_gid_is_none() {
    let font = Font::from_bytes(DEJAVU_SANS).unwrap();
    let max = u16::MAX;
    // Definitely out of any real font's glyph range.
    assert!(font.glyph_name_ref(max).is_none());
    assert!(font.glyph_name(max).is_none());
}

#[test]
fn dejavu_sans_reverse_lookup_round_trips_every_named_glyph() {
    let font = Font::from_bytes(DEJAVU_SANS).unwrap();
    // For every glyph the `post` table names, the forward name and the
    // reverse lookup must round-trip: gid_for_glyph_name(glyph_name(g))
    // returns g, unless an earlier glyph shares the same name (legal,
    // and then the lowest gid is canonical — so the reverse result is
    // <= g and itself names the same string).
    let mut named = 0usize;
    for (gid, name) in font.iter_glyph_names() {
        named += 1;
        let back = font
            .gid_for_glyph_name(name)
            .expect("a named glyph must reverse-resolve");
        assert!(
            back <= gid,
            "reverse lookup must not exceed the forward gid"
        );
        assert_eq!(
            font.glyph_name(back),
            Some(name),
            "the reverse-resolved gid must carry the same name"
        );
    }
    assert!(named > 0, "DejaVu Sans publishes glyph names");
    // The iterator count equals the number of gids that carry any name.
    let direct: usize = (0..font.glyph_count())
        .filter(|g| font.glyph_name(*g).is_some())
        .count();
    assert_eq!(named, direct);
}

#[test]
fn dejavu_sans_reverse_lookup_resolves_known_names() {
    let font = Font::from_bytes(DEJAVU_SANS).unwrap();
    // 'A' is reachable both via cmap and via its PostScript name; the
    // two routes must agree for this glyph.
    let via_cmap = font.glyph_index('A').expect("cmap maps 'A'");
    let via_name = font
        .gid_for_glyph_name("A")
        .expect("post names a glyph 'A'");
    assert_eq!(via_cmap, via_name);
    // A name no glyph carries yields None.
    assert!(font
        .gid_for_glyph_name("this_is_not_a_real_glyph_name")
        .is_none());
}

#[test]
fn reverse_lookup_on_fontless_post_is_none() {
    // A font without a usable post-name path returns None rather than
    // panicking. DejaVu has post, so confirm the empty-name guard here.
    let font = Font::from_bytes(DEJAVU_SANS).unwrap();
    assert!(font.gid_for_glyph_name("").is_none());
}
