//! Lazy optional-table parsing: construction only validates the
//! directory + required tables; optional tables parse on first use and
//! are cached; `Font::validate` restores the strict all-tables check.

use oxideav_ttf::Font;

const FIXTURES: &[(&str, &[u8])] = &[
    ("DejaVuSans", include_bytes!("fixtures/DejaVuSans.ttf")),
    (
        "DejaVuSansMono",
        include_bytes!("fixtures/DejaVuSansMono.ttf"),
    ),
    (
        "InterVariable",
        include_bytes!("fixtures/InterVariable.ttf"),
    ),
    (
        "NotoSansArabic",
        include_bytes!("fixtures/NotoSansArabic-Regular.ttf"),
    ),
    (
        "colr_1_variable",
        include_bytes!("fixtures/test_glyphs-glyf_colr_1_variable.ttf"),
    ),
];

#[test]
fn font_is_send_sync_debug() {
    fn assert_traits<T: Send + Sync + core::fmt::Debug>() {}
    assert_traits::<Font<'static>>();
}

#[test]
fn every_fixture_validates() {
    for (name, bytes) in FIXTURES {
        let font = Font::from_bytes(bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        font.validate().unwrap_or_else(|e| panic!("{name}: {e}"));
        // Idempotent: the second call reads the cached outcomes.
        font.validate().unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
fn accessors_identical_before_and_after_validate() {
    for (name, bytes) in FIXTURES {
        let lazy = Font::from_bytes(bytes).unwrap();
        let forced = Font::from_bytes(bytes).unwrap();
        forced.validate().unwrap();
        assert_eq!(lazy.has_post(), forced.has_post(), "{name}");
        assert_eq!(lazy.glyph_name(3), forced.glyph_name(3), "{name}");
        assert_eq!(lazy.is_variable(), forced.is_variable(), "{name}");
        assert_eq!(lazy.variation_coords(), forced.variation_coords(), "{name}");
        assert_eq!(
            lazy.gsub_features_for_script(*b"latn", None).len(),
            forced.gsub_features_for_script(*b"latn", None).len(),
            "{name}"
        );
        if let Some(a) = lazy.glyph_index('a') {
            assert_eq!(lazy.glyph_outline(a), forced.glyph_outline(a), "{name}");
            assert_eq!(
                lazy.glyph_advance_varied(a),
                forced.glyph_advance_varied(a),
                "{name}"
            );
        }
    }
}

#[test]
fn variation_coords_default_then_set_then_reset() {
    let bytes: &[u8] = include_bytes!("fixtures/InterVariable.ttf");
    let mut font = Font::from_bytes(bytes).unwrap();
    // Defaults are visible before any table accessor has run.
    let defaults: Vec<f32> = font.variation_coords().to_vec();
    assert_eq!(defaults.len(), font.variation_axes().len());
    assert!(!defaults.is_empty());
    for (c, a) in defaults.iter().zip(font.variation_axes()) {
        assert_eq!(*c, a.default);
    }
    let gid = font.glyph_index('a').unwrap();
    let default_outline = font.glyph_outline(gid).unwrap();

    assert!(font.set_axis_value(b"wght", 900.0));
    assert_eq!(font.axis_value(b"wght"), Some(900.0));
    let bold = font.glyph_outline(gid).unwrap();
    assert_ne!(
        bold, default_outline,
        "coordinate change must not be cached"
    );

    font.set_variation_coords(&defaults);
    assert_eq!(font.variation_coords(), &defaults[..]);
    assert_eq!(font.glyph_outline(gid).unwrap(), default_outline);
}

#[test]
fn static_font_coords_stay_empty() {
    let bytes: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");
    let mut font = Font::from_bytes(bytes).unwrap();
    assert!(font.variation_coords().is_empty());
    font.set_variation_coords(&[400.0]);
    assert!(!font.set_axis_value(b"wght", 400.0));
    assert!(font.variation_coords().is_empty());
}
