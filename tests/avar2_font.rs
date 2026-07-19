//! Font-level avar version-2 resolution, driven through a real
//! variable font: the InterVariable fixture's `avar` table is swapped
//! for a synthetic v2 table (built byte-for-byte from the staged avar
//! v2 reference), so the three-stage normalisation runs end-to-end
//! through `Font::normalised_coords` against genuine `fvar` axes.

use oxideav_ttf::Font;

/// Rebuild an sfnt with table `tag`'s body replaced: the new body is
/// appended at EOF and the directory record repointed (the old body
/// bytes stay in place, unreferenced — offsets don't shift).
fn replace_table(font: &[u8], tag: [u8; 4], body: &[u8]) -> Vec<u8> {
    let num_tables = u16::from_be_bytes([font[4], font[5]]) as usize;
    let mut out = font.to_vec();
    let mut found = false;
    for i in 0..num_tables {
        let rec = 12 + i * 16;
        if &font[rec..rec + 4] == tag.as_slice() {
            out[rec + 8..rec + 12].copy_from_slice(&(font.len() as u32).to_be_bytes());
            out[rec + 12..rec + 16].copy_from_slice(&(body.len() as u32).to_be_bytes());
            found = true;
            break;
        }
    }
    assert!(found, "table {:?} not present", std::str::from_utf8(&tag));
    out.extend_from_slice(body);
    out
}

/// Build an avar v2 table for `axis_count` axes with no segment maps
/// and one cross-axis rule: a region peaking at +1 on `driver_axis`
/// applies per-axis F2DOT14 deltas from `rows` (identity axisIndexMap:
/// inner = axis index).
fn build_avar2(axis_count: usize, driver_axis: usize, rows: &[i16]) -> Vec<u8> {
    let mut b: Vec<u8> = Vec::new();
    b.extend_from_slice(&2u16.to_be_bytes()); // majorVersion
    b.extend_from_slice(&0u16.to_be_bytes()); // minorVersion
    b.extend_from_slice(&0u16.to_be_bytes()); // reserved
    b.extend_from_slice(&0u16.to_be_bytes()); // axisSegmentMapCount = 0
    b.extend_from_slice(&0u32.to_be_bytes()); // axisIndexMapOffset = 0
    let store_slot = b.len();
    b.extend_from_slice(&0u32.to_be_bytes()); // varStoreOffset (patched)

    let ivs = b.len() as u32;
    b.extend_from_slice(&1u16.to_be_bytes()); // format
    b.extend_from_slice(&12u32.to_be_bytes()); // regionListOffset
    b.extend_from_slice(&1u16.to_be_bytes()); // ivdCount
    let region_bytes = 4 + axis_count * 6;
    b.extend_from_slice(&((12 + region_bytes) as u32).to_be_bytes()); // ivdOffsets[0]
    b.extend_from_slice(&(axis_count as u16).to_be_bytes());
    b.extend_from_slice(&1u16.to_be_bytes()); // regionCount
    for a in 0..axis_count {
        let peak: i16 = if a == driver_axis { 16384 } else { 0 };
        b.extend_from_slice(&0i16.to_be_bytes());
        b.extend_from_slice(&peak.to_be_bytes());
        b.extend_from_slice(&peak.to_be_bytes());
    }
    b.extend_from_slice(&(rows.len() as u16).to_be_bytes()); // itemCount
    b.extend_from_slice(&1u16.to_be_bytes()); // shortDeltaCount
    b.extend_from_slice(&1u16.to_be_bytes()); // regionIndexCount
    b.extend_from_slice(&0u16.to_be_bytes()); // regionIndexes[0]
    for &d in rows {
        b.extend_from_slice(&d.to_be_bytes());
    }
    b[store_slot..store_slot + 4].copy_from_slice(&ivs.to_be_bytes());
    b
}

fn fixture() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/InterVariable.ttf"
    ))
    .expect("fixture")
}

#[test]
fn avar2_cross_axis_delta_flows_through_normalised_coords() {
    let inter = fixture();
    let probe = Font::from_bytes(&inter).expect("parse fixture");
    let axes = probe.variation_axes();
    let wght = axes
        .iter()
        .position(|a| &a.tag == b"wght")
        .expect("wght axis");
    let opsz = axes
        .iter()
        .position(|a| &a.tag == b"opsz")
        .expect("opsz axis");
    let wght_max = axes[wght].max;

    // wght at +1 pushes opsz by +0.5 (row `opsz` = 8192); every other
    // axis row is 0.
    let mut rows = vec![0i16; axes.len()];
    rows[opsz] = 8192;
    let avar2 = build_avar2(axes.len(), wght, &rows);
    let bytes = replace_table(&inter, *b"avar", &avar2);
    let mut font = Font::from_bytes(&bytes).expect("parse swapped font");
    assert!(!font.avar_axis_index_map_unsupported());

    // Default instance: all zero.
    let coords = font.normalised_coords();
    assert!(coords.iter().all(|c| c.abs() < 1e-6), "{coords:?}");

    // wght at max: wght normalises to +1 (no segment maps in the v2
    // table), and stage 3 moves opsz from 0 to +0.5.
    font.set_axis_value(b"wght", wght_max);
    let coords = font.normalised_coords();
    assert!((coords[wght] - 1.0).abs() < 1e-6, "{coords:?}");
    assert!((coords[opsz] - 0.5).abs() < 1e-4, "{coords:?}");

    // A varied outline still renders at the warped instance (the gvar
    // pipeline consumes the stage-3 output downstream).
    let gid = font.glyph_index('A').expect("gid");
    let outline = font.glyph_outline(gid).expect("outline");
    assert!(!outline.contours.is_empty());
}

#[test]
fn avar1_fixture_behaviour_is_unchanged() {
    // Regression guard: the fixture's own avar (v1) still bends
    // per-axis only, and the default instance is all-zero.
    let inter = fixture();
    let mut font = Font::from_bytes(&inter).expect("parse fixture");
    assert!(!font.avar_axis_index_map_unsupported());
    assert!(font
        .normalised_coords()
        .iter()
        .all(|c| c.abs() < f32::EPSILON));
    let axes = font.variation_axes();
    let wght = axes.iter().position(|a| &a.tag == b"wght").unwrap();
    let wght_max = axes[wght].max;
    font.set_axis_value(b"wght", wght_max);
    let coords = font.normalised_coords();
    // The +1 anchor is mandatory in any non-empty v1 segment map, so
    // max weight still normalises to exactly +1; other axes stay 0.
    assert!((coords[wght] - 1.0).abs() < 1e-6, "{coords:?}");
    for (i, c) in coords.iter().enumerate() {
        if i != wght {
            assert!(c.abs() < 1e-6, "{coords:?}");
        }
    }
}
