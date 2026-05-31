//! `MVAR` integration tests against InterVariable.ttf.
//!
//! Inter ships a five-record MVAR with the `xhgt`, `stro`, `strs`,
//! `undo`, `unds` tags and a single `ItemVariationData` subtable. The
//! variation regions reference the `opsz` + `wght` axes; deltas at
//! the axis defaults must therefore evaluate to zero, and the
//! per-tag interpolated values at the four canonical corner points
//! of the design space must match the spec's region-scalar
//! computation (ISO/IEC 14496-22:2019 §7.1, §7.2.3, §7.3.6.2).
//!
//! Reference values were computed by hand against the bit-exact
//! decoded MVAR payload, NOT by black-box comparison against any
//! existing font shaper — every byte that drives the assertion below
//! lives in `tests/fixtures/InterVariable.ttf` and the math is the
//! spec's pseudocode.

use oxideav_ttf::Font;

const FONT: &[u8] = include_bytes!("fixtures/InterVariable.ttf");

#[test]
fn parses_mvar_value_records() {
    let font = Font::from_bytes(FONT).expect("parse");
    let m = font.mvar_table().expect("Inter ships MVAR");
    // Inter ships exactly five value records.
    assert_eq!(m.value_record_count(), 5);
    // The spec mandates binary order of the tag bytes; check the
    // exact set is present (order tolerance is intentional so that
    // upstream re-orderings don't make this test brittle).
    let mut tags: Vec<[u8; 4]> = m.value_records().map(|(t, _, _)| t).collect();
    tags.sort();
    let mut expected: Vec<[u8; 4]> = vec![*b"stro", *b"strs", *b"undo", *b"unds", *b"xhgt"];
    expected.sort();
    assert_eq!(tags, expected);
}

#[test]
fn mvar_ivs_shape_matches_inter() {
    let font = Font::from_bytes(FONT).unwrap();
    let m = font.mvar_table().unwrap();
    let ivs = m.item_variation_store().unwrap();
    // Inter: 2 axes (opsz, wght), 5 regions, 1 IVD subtable.
    assert_eq!(ivs.axis_count(), 2);
    assert_eq!(ivs.region_count(), 5);
    assert_eq!(ivs.subtable_count(), 1);
}

#[test]
fn mvar_delta_zero_at_axis_defaults() {
    let font = Font::from_bytes(FONT).unwrap();
    // Defaults: opsz = 14, wght = 400 → normalised = (0, 0).
    for tag in [b"xhgt", b"stro", b"strs", b"unds", b"undo"] {
        let d = font
            .metric_variation_delta(tag)
            .expect("known tag in Inter MVAR");
        assert!(
            d.abs() < 1e-5,
            "expected zero delta at axis defaults for {:?}, got {d}",
            std::str::from_utf8(tag).unwrap()
        );
    }
}

#[test]
fn mvar_delta_at_max_weight() {
    let mut font = Font::from_bytes(FONT).unwrap();
    // opsz axis is first in Inter's fvar; leave opsz at default (14)
    // and push wght to 900. Normalised: (0, 1).
    font.set_variation_coords(&[14.0, 900.0]);
    // Per the spec's region-scalar calculation, regions whose opsz
    // peak is nonzero have a scalar of 0 at opsz=14; the only active
    // region at (0, 1) is region 2 (opsz axis ignored via peak=0;
    // wght axis (0, 1, 1) ⇒ scalar 1). Its row contribution gives:
    //   xhgt: 0; stro: 0; strs: +92; unds: +92; undo: +46.
    assert!((font.metric_variation_delta(b"xhgt").unwrap() - 0.0).abs() < 1e-5);
    assert!((font.metric_variation_delta(b"stro").unwrap() - 0.0).abs() < 1e-5);
    assert!((font.metric_variation_delta(b"strs").unwrap() - 92.0).abs() < 1e-5);
    assert!((font.metric_variation_delta(b"unds").unwrap() - 92.0).abs() < 1e-5);
    assert!((font.metric_variation_delta(b"undo").unwrap() - 46.0).abs() < 1e-5);
}

#[test]
fn mvar_delta_at_max_opsz_default_weight() {
    let mut font = Font::from_bytes(FONT).unwrap();
    // opsz = 32, wght = 400 → normalised = (1, 0). Active regions
    // have their wght-axis component as (0, 0, 0) (peak=0 ⇒ axis
    // ignored): regions 0 and 4 (opsz peak=+1) and region 2 (opsz
    // peak=0). Combined scalars: region 0 ⇒ 1, region 4 ⇒ 0
    // (wght peak=+1 with c=0 ⇒ 0), region 2 ⇒ 0 (wght peak=+1 with
    // c=0 ⇒ 0). Row contributions reduce to region-0 entries only.
    //   xhgt: -62; stro: -37; strs: 0; unds: 0; undo: +140.
    font.set_variation_coords(&[32.0, 400.0]);
    assert!((font.metric_variation_delta(b"xhgt").unwrap() - (-62.0)).abs() < 1e-5);
    assert!((font.metric_variation_delta(b"stro").unwrap() - (-37.0)).abs() < 1e-5);
    assert!((font.metric_variation_delta(b"strs").unwrap() - 0.0).abs() < 1e-5);
    assert!((font.metric_variation_delta(b"unds").unwrap() - 0.0).abs() < 1e-5);
    assert!((font.metric_variation_delta(b"undo").unwrap() - 140.0).abs() < 1e-5);
}

#[test]
fn mvar_delta_at_min_weight() {
    let mut font = Font::from_bytes(FONT).unwrap();
    // wght = 100, opsz = 14 → normalised = (0, -1).
    // Active region: region 1 (opsz peak=0 ⇒ ignored; wght (-1, -1, 0)
    // ⇒ scalar 1 at c=-1). Row contribution: column 1 of each row.
    //   xhgt: 0; stro: 0; strs: -94; unds: -94; undo: -47.
    font.set_variation_coords(&[14.0, 100.0]);
    assert!((font.metric_variation_delta(b"xhgt").unwrap() - 0.0).abs() < 1e-5);
    assert!((font.metric_variation_delta(b"strs").unwrap() - (-94.0)).abs() < 1e-5);
    assert!((font.metric_variation_delta(b"unds").unwrap() - (-94.0)).abs() < 1e-5);
    assert!((font.metric_variation_delta(b"undo").unwrap() - (-47.0)).abs() < 1e-5);
}

#[test]
fn mvar_interior_weight_interpolates_through_avar() {
    let mut font = Font::from_bytes(FONT).unwrap();
    // wght = 700, opsz = 32 → user-space (32, 700).
    // Inter ships an `avar` table that remaps the wght axis with a
    // 7-segment piecewise-linear curve; at user wght=700 the linear
    // normalisation gives +0.6 but `avar` bends that to +0.54 (the
    // (0.6, 0.54) segment in `avar`'s wght axis-value map).
    //
    // Active regions at the avar-remapped (1, 0.54) instance:
    //   region 0 (opsz: 1 ⇒ 1; wght peak=0 ⇒ 1) ⇒ scalar 1
    //   region 2 (opsz peak=0 ⇒ 1; wght peak=+1, c=0.54) ⇒ 0.54
    //   region 4 (opsz: 1 ⇒ 1; wght peak=+1, c=0.54) ⇒ 0.54
    // For 'undo' (row [140, -47, 46, -94, 74]):
    //   140*1 + 46*0.54 + 74*0.54 = 140 + 24.84 + 39.96 = 204.8.
    font.set_variation_coords(&[32.0, 700.0]);
    let d = font.metric_variation_delta(b"undo").unwrap();
    assert!((d - 204.8).abs() < 1e-3, "got {d}");
    // 'strs' row [0, -94, 92, 0, 28]:
    //   0 + 92*0.54 + 28*0.54 = 64.8.
    let d = font.metric_variation_delta(b"strs").unwrap();
    assert!((d - 64.8).abs() < 1e-3, "got {d}");
}

#[test]
fn mvar_unknown_tag_returns_none() {
    let font = Font::from_bytes(FONT).unwrap();
    // 'cpht' (cap height) is not in Inter's MVAR.
    assert!(font.metric_variation_delta(b"cpht").is_none());
    // Truly unknown 4-byte tag also misses.
    assert!(font.metric_variation_delta(b"abcd").is_none());
}
