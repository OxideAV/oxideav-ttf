//! Integration coverage for the `MATH` table accessors on
//! [`oxideav_ttf::Font`], including the variation-aware `*_var` helpers
//! (ISO/IEC 14496-22:2019 §6.3.6).
//!
//! DejaVu Sans ships a static (non-variable) `MATH` table, so the
//! variation-aware accessors must equal the plain design-unit values:
//! with no variable axes there is no VariationIndex delta to fold in.
//! This exercises the full `Font` → `MathTable` → resolved-accessor wire
//! path on a real font. The fractional VariationIndex deltas themselves
//! are covered by the synthetic unit tests in `tables::math`.

use oxideav_ttf::tables::math::{constant, GrowDirection, MathKernCorner};
use oxideav_ttf::Font;

const DEJAVU_SANS: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");
const DEJAVU_MONO: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");
const INTER: &[u8] = include_bytes!("fixtures/InterVariable.ttf");

#[test]
fn dejavu_sans_has_math_constants() {
    let f = Font::from_bytes(DEJAVU_SANS).unwrap();
    assert!(f.has_math());
    let m = f.math_table().expect("MATH table");
    let c = m.constants().expect("MathConstants");

    // Spot-check a few well-known DejaVu Sans MATH constants.
    assert_eq!(c.script_percent_scale_down(), 80);
    assert_eq!(c.value_i16(constant::AXIS_HEIGHT), 642);
    assert_eq!(c.value_i16(constant::FRACTION_RULE_THICKNESS), 90);
}

#[test]
fn static_font_var_accessors_equal_plain_values() {
    // A non-variable font has no axes, so every resolved value equals the
    // plain design-unit value (no VariationIndex delta can apply).
    let f = Font::from_bytes(DEJAVU_SANS).unwrap();
    let c = f.math_table().unwrap().constants().unwrap();

    for idx in [
        constant::AXIS_HEIGHT,
        constant::FRACTION_RULE_THICKNESS,
        constant::MATH_LEADING,
        constant::RADICAL_RULE_THICKNESS,
        constant::STACK_GAP_MIN,
    ] {
        let plain = c.value_i16(idx) as f32;
        let resolved = f.math_constant_var(idx).expect("constant resolves");
        assert_eq!(resolved, plain, "constant index {idx}");
    }
}

#[test]
fn glyph_info_present_and_var_accessors_wire_through() {
    let f = Font::from_bytes(DEJAVU_SANS).unwrap();
    let m = f.math_table().unwrap();
    let gi = m.glyph_info().expect("MathGlyphInfo");

    // DejaVu Sans MATH declares no italics-correction or math-kern
    // coverage, so both the plain and resolved accessors decline for any
    // glyph — the resolved path must agree with the plain path rather
    // than fabricate a value.
    for g in [0u16, 11, 42, 100, 500] {
        assert_eq!(gi.italics_correction(g), None);
        assert_eq!(f.math_italics_correction_var(g), None);
        assert_eq!(gi.math_kern(g, MathKernCorner::TopRight, 0), None);
        assert_eq!(f.math_kern_var(g, MathKernCorner::TopRight, 0), None);
    }
}

#[test]
fn assembly_italics_correction_var_matches_plain() {
    // DejaVu Sans defines vertical glyph assemblies (e.g. growing
    // brackets). gid 11 is one such assembly; its italics correction is
    // zero and, on a static font, the resolved value must match.
    let f = Font::from_bytes(DEJAVU_SANS).unwrap();
    let v = f.math_table().unwrap().variants().expect("MathVariants");

    let (plain_italics, parts) = v
        .assembly(11, GrowDirection::Vertical)
        .expect("vertical assembly for gid 11");
    assert!(parts.len() >= 2);

    let resolved = f
        .math_assembly_italics_correction_var(11, GrowDirection::Vertical)
        .expect("assembly italics resolves");
    assert_eq!(resolved, plain_italics as f32);

    // A glyph with no assembly declines on both paths.
    assert!(v.assembly(0, GrowDirection::Vertical).is_none());
    assert!(f
        .math_assembly_italics_correction_var(0, GrowDirection::Vertical)
        .is_none());
}

#[test]
fn fonts_without_math_decline_cleanly() {
    // Neither DejaVu Sans Mono nor Inter ship a MATH table; every
    // accessor must return None / false rather than panic.
    for bytes in [DEJAVU_MONO, INTER] {
        let f = Font::from_bytes(bytes).unwrap();
        assert!(!f.has_math());
        assert!(f.math_table().is_none());
        assert_eq!(f.math_constant_var(constant::AXIS_HEIGHT), None);
        assert_eq!(f.math_italics_correction_var(5), None);
        assert_eq!(f.math_top_accent_attachment_var(5), None);
        assert_eq!(f.math_kern_var(5, MathKernCorner::TopLeft, 0), None);
        assert_eq!(
            f.math_assembly_italics_correction_var(5, GrowDirection::Vertical),
            None
        );
    }
}
