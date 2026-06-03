//! `STAT` integration tests against InterVariable.ttf.
//!
//! Inter ships a v1.1 STAT table with three design axes (`opsz`,
//! `wght`, `ital`), twelve axis-value tables, and an
//! `elidedFallbackNameID` of 2 ("Regular"). The wght=400 and ital=0
//! entries are format-3 style-linked pairs pointing at their Bold /
//! Italic counterparts, exactly the structural shape ISO/IEC
//! 14496-22:2019 §7.3.7.3 specifies.
//!
//! Reference values were derived from the bit-exact decoded STAT
//! payload, NOT by black-box comparison against any existing font
//! shaper — every assertion below traces to a field laid out in
//! `tests/fixtures/InterVariable.ttf`.

use oxideav_ttf::{Font, StatAxisValue, STAT_FLAG_ELIDABLE_AXIS_VALUE_NAME};

const FONT: &[u8] = include_bytes!("fixtures/InterVariable.ttf");

#[test]
fn parses_stat_header_and_axes() {
    let font = Font::from_bytes(FONT).expect("parse");
    let t = font.stat_table().expect("Inter ships STAT");
    assert_eq!(t.major_version(), 1);
    assert_eq!(t.minor_version(), 1);
    assert_eq!(t.elided_fallback_name_id(), 2);

    let axes = t.axes();
    assert_eq!(axes.len(), 3);
    // Document order: opsz, wght, ital. We check by tag rather than
    // index so a future re-ordering would not regress the test.
    let tags: Vec<[u8; 4]> = axes.iter().map(|a| a.axis_tag).collect();
    assert!(tags.contains(b"opsz"));
    assert!(tags.contains(b"wght"));
    assert!(tags.contains(b"ital"));

    // Inter's axis records carry these specific (nameID, ordering)
    // pairs (laid out in the file at offset 20).
    let opsz = axes.iter().find(|a| &a.axis_tag == b"opsz").unwrap();
    assert_eq!(opsz.axis_name_id, 297);
    assert_eq!(opsz.axis_ordering, 0);

    let wght = axes.iter().find(|a| &a.axis_tag == b"wght").unwrap();
    assert_eq!(wght.axis_name_id, 278);
    assert_eq!(wght.axis_ordering, 1);

    let ital = axes.iter().find(|a| &a.axis_tag == b"ital").unwrap();
    assert_eq!(ital.axis_name_id, 300);
    assert_eq!(ital.axis_ordering, 2);
}

#[test]
fn axis_value_count_matches_inter() {
    let font = Font::from_bytes(FONT).unwrap();
    let t = font.stat_table().unwrap();
    // Inter: 12 axis value tables (2 opsz, 9 wght, 1 ital).
    assert_eq!(t.axis_values().len(), 12);
}

#[test]
fn wght_400_is_style_linked_to_700() {
    let font = Font::from_bytes(FONT).unwrap();
    // Walk every wght axis-value record and find the format-3 one;
    // it's the "Regular → Bold" style link.
    let mut found_link = false;
    for v in font.stat_axis_values_for_tag(*b"wght") {
        if let StatAxisValue::Format3 {
            value,
            linked_value,
            flags,
            ..
        } = v
        {
            assert!((*value - 400.0).abs() < 1e-5);
            assert!((*linked_value - 700.0).abs() < 1e-5);
            // §7.3.7.3: "Regular" gets the ELIDABLE flag.
            assert!(flags & STAT_FLAG_ELIDABLE_AXIS_VALUE_NAME != 0);
            found_link = true;
        }
    }
    assert!(found_link, "expected a format-3 wght entry");
}

#[test]
fn wght_axis_carries_nine_distinct_values() {
    let font = Font::from_bytes(FONT).unwrap();
    let mut wghts: Vec<f32> = Vec::new();
    for v in font.stat_axis_values_for_tag(*b"wght") {
        match v {
            StatAxisValue::Format1 { value, .. } => wghts.push(*value),
            StatAxisValue::Format3 { value, .. } => wghts.push(*value),
            // Inter's wght entries are all format 1 or 3.
            _ => panic!("unexpected wght axis-value format: {v:?}"),
        }
    }
    wghts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(wghts.len(), 9);
    let expected = [
        100.0, 200.0, 300.0, 400.0, 500.0, 600.0, 700.0, 800.0, 900.0,
    ];
    for (got, want) in wghts.iter().zip(expected.iter()) {
        assert!((got - want).abs() < 1e-5, "got {got} want {want}");
    }
}

#[test]
fn opsz_axis_carries_two_values_one_per_endpoint() {
    let font = Font::from_bytes(FONT).unwrap();
    let mut opsz_values: Vec<f32> = Vec::new();
    for v in font.stat_axis_values_for_tag(*b"opsz") {
        if let StatAxisValue::Format1 { value, .. } = v {
            opsz_values.push(*value);
        }
    }
    opsz_values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(opsz_values.len(), 2);
    assert!((opsz_values[0] - 14.0).abs() < 1e-5);
    assert!((opsz_values[1] - 32.0).abs() < 1e-5);
}

#[test]
fn ital_axis_is_style_linked_to_italic_counterpart() {
    let font = Font::from_bytes(FONT).unwrap();
    let entries: Vec<&StatAxisValue> = font.stat_axis_values_for_tag(*b"ital").collect();
    assert_eq!(entries.len(), 1);
    if let StatAxisValue::Format3 {
        value,
        linked_value,
        flags,
        ..
    } = entries[0]
    {
        assert!((*value - 0.0).abs() < 1e-5);
        assert!((*linked_value - 1.0).abs() < 1e-5);
        // The "Roman" (ital=0) entry is elidable.
        assert!(flags & STAT_FLAG_ELIDABLE_AXIS_VALUE_NAME != 0);
    } else {
        panic!("expected a format-3 ital entry");
    }
}

#[test]
fn font_level_accessors_round_trip() {
    let font = Font::from_bytes(FONT).unwrap();
    assert_eq!(font.stat_axes().len(), 3);
    assert_eq!(font.stat_axis_values().len(), 12);
    assert_eq!(font.stat_elided_fallback_name_id(), Some(2));
}

#[test]
fn dejavu_has_no_stat() {
    // Static font baseline: DejaVu Sans Mono is non-variable and
    // does not ship a STAT table. The accessors should report None /
    // empty consistently.
    const DEJAVU: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");
    let font = Font::from_bytes(DEJAVU).unwrap();
    assert!(font.stat_table().is_none());
    assert_eq!(font.stat_axes().len(), 0);
    assert_eq!(font.stat_axis_values().len(), 0);
    assert_eq!(font.stat_elided_fallback_name_id(), None);
    assert_eq!(font.stat_axis_values_for_tag(*b"wght").count(), 0);
}
