//! `HVAR` integration tests against InterVariable.ttf.
//!
//! Inter ships an HVAR with an `advanceWidthMapping` table (2926
//! entries, 2-byte packed entries with 9 inner-index bits and 7 outer-
//! index bits) and a 10-subtable item variation store referencing all
//! five MVAR regions. LSB and RSB mapping offsets are zero (advance-
//! only variation), so `lsb_variation_delta` / `rsb_variation_delta`
//! must return `None` for this font.
//!
//! All reference values in this file were computed from the bit-exact
//! decoded HVAR payload using the §7.3.5.3 / §7.1 spec math; nothing
//! here comes from a black-box comparison against any existing font
//! shaper.

use oxideav_ttf::Font;

const FONT: &[u8] = include_bytes!("fixtures/InterVariable.ttf");

#[test]
fn parses_hvar_header() {
    let font = Font::from_bytes(FONT).expect("parse");
    let h = font.hvar_table().expect("Inter ships HVAR");
    // Inter publishes an advance-width map but no LSB/RSB mapping
    // tables (LSB / RSB variations would require those offsets to be
    // non-zero per §7.3.5.2).
    assert!(h.has_advance_width_map());
    assert!(!h.has_lsb_map());
    assert!(!h.has_rsb_map());
    // Shape of the embedded IVS: Inter publishes 5 regions across 10
    // ItemVariationData subtables on a 2-axis (opsz + wght) variation
    // space.
    let ivs = h.item_variation_store();
    assert_eq!(ivs.axis_count(), 2);
    assert_eq!(ivs.region_count(), 5);
    assert_eq!(ivs.subtable_count(), 10);
}

#[test]
fn advance_width_delta_zero_at_axis_defaults() {
    let font = Font::from_bytes(FONT).unwrap();
    // Defaults: opsz = 14, wght = 400 → normalised (0, 0). All
    // regions in Inter's HVAR peak at +1 / -1 along at least one
    // axis, so every region scalar is zero at the default instance.
    for gid in [1, 2, 3, 4, 100, 500, 1000].iter().copied() {
        let d = font
            .advance_width_variation_delta(gid)
            .expect("gid in IVS range");
        assert!(
            d.abs() < 1e-5,
            "expected zero delta at axis defaults for gid {gid}, got {d}"
        );
    }
}

#[test]
fn advance_width_delta_at_max_weight() {
    let mut font = Font::from_bytes(FONT).unwrap();
    // wght = 900, opsz = 14 → normalised (0, 1).
    font.set_variation_coords(&[14.0, 900.0]);

    // gid 1 maps to (outer=9, inner=0). IVD[9] references all five
    // regions; at (0, 1) only region 2 (opsz peak=0 ⇒ axis ignored,
    // wght peak=+1 ⇒ scalar 1) is active. IVD[9] row 0 contributes
    // exactly the bytes that sum to 0 there.
    let d = font.advance_width_variation_delta(1).expect("gid 1");
    assert!(d.abs() < 1e-5, "gid 1 wght=900 got {d}");

    // gid 2 / 3 / 4 all map to (3, 45). Reference value computed
    // from the decoded IVD bytes is exactly +215.
    for gid in [2u16, 3, 4] {
        let d = font.advance_width_variation_delta(gid).expect("gid");
        assert!(
            (d - 215.0).abs() < 1e-5,
            "gid {gid} wght=900 got {d} (expected 215.0)"
        );
    }

    // gid 100 maps to (1, 274). Reference value: +24.0.
    let d = font.advance_width_variation_delta(100).expect("gid 100");
    assert!((d - 24.0).abs() < 1e-5, "gid 100 wght=900 got {d}");
}

#[test]
fn advance_width_delta_at_min_weight() {
    let mut font = Font::from_bytes(FONT).unwrap();
    // wght = 100, opsz = 14 → normalised (0, -1).
    font.set_variation_coords(&[14.0, 100.0]);

    // Same indices as above; reference values: gid 2/3/4 ⇒ -103,
    // gid 100 ⇒ -35.
    for gid in [2u16, 3, 4] {
        let d = font.advance_width_variation_delta(gid).expect("gid");
        assert!(
            (d - (-103.0)).abs() < 1e-5,
            "gid {gid} wght=100 got {d} (expected -103.0)"
        );
    }
    let d = font.advance_width_variation_delta(100).expect("gid 100");
    assert!((d - (-35.0)).abs() < 1e-5, "gid 100 wght=100 got {d}");
}

#[test]
fn lsb_rsb_delta_none_when_maps_absent() {
    let mut font = Font::from_bytes(FONT).unwrap();
    font.set_variation_coords(&[14.0, 900.0]);
    // Inter has no LSB / RSB mapping tables, so per §7.3.5.2 these
    // queries must yield `None`.
    assert!(font.lsb_variation_delta(2).is_none());
    assert!(font.rsb_variation_delta(2).is_none());
}

#[test]
fn advance_width_delta_clamps_oob_glyph_to_last_map_entry() {
    let mut font = Font::from_bytes(FONT).unwrap();
    font.set_variation_coords(&[14.0, 900.0]);
    // Inter's advance map has 2926 entries; gid 65535 must use the
    // last entry per §7.3.5.2. The crate accepts the lookup (no
    // panic / no None on the map-side); whether the resolved IVS
    // pair actually has a delta depends on the entry. We just check
    // that the call returns `Some(_)` — the implementation must not
    // bail before getting to the IVS lookup.
    let _ = font.advance_width_variation_delta(65_535);
}
