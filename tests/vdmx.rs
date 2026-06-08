//! Integration coverage for the `VDMX` accessors on
//! [`oxideav_ttf::Font`].
//!
//! Two paths:
//!
//! * **Absent path** — DejaVu Sans Mono / DejaVu Sans / Inter, all of
//!   which do not ship `VDMX`. Every accessor must return
//!   `None` / `false`.
//!
//! * **Synthetic path** — a TrueType-flavoured sfnt that ships a
//!   `VDMX` table with one RatioRange `(charset, 1:1)`, one group,
//!   and three vTable records at distinct ppem sizes. We verify the
//!   table parses, [`Font::vdmx_y_extent_square`] resolves through
//!   the 1:1 ratio, [`Font::vdmx_y_extent_for_device`] honours the
//!   §5.7.8 "exact ppem only" rule (no nearest-neighbour fallback),
//!   and a non-matching device ratio returns `None` because the
//!   table ships no `(0,0,0)` sentinel.
//!
//! A third synthetic font ships an explicit `(0,0,0)` sentinel and
//! shows the catch-all matches every non-1:1 device.

use oxideav_ttf::Font;

const DEJAVU_MONO: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");
const DEJAVU_SANS: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");
const INTER: &[u8] = include_bytes!("fixtures/InterVariable.ttf");

#[test]
fn dejavu_mono_has_no_vdmx() {
    let f = Font::from_bytes(DEJAVU_MONO).unwrap();
    assert!(!f.has_vdmx());
    assert!(f.vdmx_table().is_none());
    assert_eq!(f.vdmx_y_extent_square(12), None);
    assert_eq!(f.vdmx_y_extent_for_device(12, 1, 1), None);
}

#[test]
fn dejavu_sans_has_no_vdmx() {
    let f = Font::from_bytes(DEJAVU_SANS).unwrap();
    assert!(!f.has_vdmx());
}

#[test]
fn inter_has_no_vdmx() {
    let f = Font::from_bytes(INTER).unwrap();
    assert!(!f.has_vdmx());
}

// ---- synthetic-font scaffolding ------------------------------------------

fn build_minimal_sfnt(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let n = tables.len() as u16;
    let mut out = Vec::new();
    out.extend_from_slice(&0x00010000u32.to_be_bytes());
    out.extend_from_slice(&n.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());

    let dir_size = 16usize * n as usize;
    let mut payload_offset = 12usize + dir_size;
    let mut records = Vec::with_capacity(n as usize);
    for (tag, payload) in tables {
        records.push((*tag, payload_offset as u32, payload.len() as u32));
        payload_offset += payload.len();
        while payload_offset % 4 != 0 {
            payload_offset += 1;
        }
    }
    for (tag, offset, length) in &records {
        out.extend_from_slice(tag.as_slice());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&offset.to_be_bytes());
        out.extend_from_slice(&length.to_be_bytes());
    }
    for (_tag, payload) in tables {
        out.extend_from_slice(payload);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    out
}

fn make_head(units_per_em: u16, index_to_loc_format: i16) -> Vec<u8> {
    let mut h = vec![0u8; 54];
    h[0..4].copy_from_slice(&0x00010000u32.to_be_bytes());
    h[4..8].copy_from_slice(&0x00010000u32.to_be_bytes());
    h[12..16].copy_from_slice(&0x5F0F3CF5u32.to_be_bytes());
    h[18..20].copy_from_slice(&units_per_em.to_be_bytes());
    h[50..52].copy_from_slice(&index_to_loc_format.to_be_bytes());
    h
}

fn make_hhea(num_h_metrics: u16) -> Vec<u8> {
    let mut h = vec![0u8; 36];
    h[0..4].copy_from_slice(&0x00010000u32.to_be_bytes());
    h[34..36].copy_from_slice(&num_h_metrics.to_be_bytes());
    h
}

fn make_maxp(num_glyphs: u16) -> Vec<u8> {
    let mut h = vec![0u8; 6];
    h[0..4].copy_from_slice(&0x00005000u32.to_be_bytes());
    h[4..6].copy_from_slice(&num_glyphs.to_be_bytes());
    h
}

fn make_cmap_empty() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&3u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&12u32.to_be_bytes());
    let mut sub = Vec::new();
    sub.extend_from_slice(&4u16.to_be_bytes());
    sub.extend_from_slice(&32u16.to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes());
    sub.extend_from_slice(&2u16.to_be_bytes());
    sub.extend_from_slice(&2u16.to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes());
    sub.extend_from_slice(&0xFFFFu16.to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes());
    sub.extend_from_slice(&0xFFFFu16.to_be_bytes());
    sub.extend_from_slice(&1u16.to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes());
    let total = sub.len() as u16;
    sub[2..4].copy_from_slice(&total.to_be_bytes());
    out.extend_from_slice(&sub);
    out
}

fn make_name_empty() -> Vec<u8> {
    let mut out = vec![0u8; 6];
    out[0..2].copy_from_slice(&0u16.to_be_bytes());
    out[2..4].copy_from_slice(&0u16.to_be_bytes());
    out[4..6].copy_from_slice(&6u16.to_be_bytes());
    out
}

fn make_hmtx(num_h_metrics: u16, num_glyphs: u16) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..num_h_metrics {
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&0i16.to_be_bytes());
    }
    for _ in num_h_metrics..num_glyphs {
        out.extend_from_slice(&0i16.to_be_bytes());
    }
    out
}

fn make_loca_short(num_glyphs: u16) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..=num_glyphs {
        out.extend_from_slice(&0u16.to_be_bytes());
    }
    out
}

fn make_glyf_empty() -> Vec<u8> {
    vec![0u8; 2]
}

/// Build a `VDMX` payload that ships one RatioRange and one group
/// containing `entries.len()` vTable records, in document order.
/// `extra_ratio_with_sentinel` adds a trailing `(0,0,0)` sentinel
/// pointing at a second group (with its own one-record contents) so
/// the test can exercise the sentinel-match path.
fn make_vdmx(
    version: u16,
    entries: &[(u16, i16, i16)],
    extra_sentinel_group: Option<&[(u16, i16, i16)]>,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&version.to_be_bytes());
    let num_recs = 1u16 + extra_sentinel_group.is_some() as u16;
    let num_ratios = 1u16 + extra_sentinel_group.is_some() as u16;
    out.extend_from_slice(&num_recs.to_be_bytes());
    out.extend_from_slice(&num_ratios.to_be_bytes());
    // RatioRange[0]: (charset=1, 1:1).
    out.extend_from_slice(&[1, 1, 1, 1]);
    if extra_sentinel_group.is_some() {
        // RatioRange[1]: sentinel (0,0,0,0).
        out.extend_from_slice(&[1, 0, 0, 0]);
    }
    // Offset16 array entries.
    let ratio_bytes = 4 * num_ratios as usize;
    let offset_bytes = 2 * num_ratios as usize;
    let g0_off = 6 + ratio_bytes + offset_bytes;
    let g0_body_size = 4 + 6 * entries.len();
    let g1_off = g0_off + g0_body_size;
    out.extend_from_slice(&(g0_off as u16).to_be_bytes());
    if extra_sentinel_group.is_some() {
        out.extend_from_slice(&(g1_off as u16).to_be_bytes());
    }
    // Group 0.
    out.extend_from_slice(&(entries.len() as u16).to_be_bytes());
    out.push(entries.first().map(|e| e.0 as u8).unwrap_or(0));
    out.push(entries.last().map(|e| e.0 as u8).unwrap_or(0));
    for &(ppem, ymax, ymin) in entries {
        out.extend_from_slice(&ppem.to_be_bytes());
        out.extend_from_slice(&ymax.to_be_bytes());
        out.extend_from_slice(&ymin.to_be_bytes());
    }
    // Group 1 (sentinel target), if requested.
    if let Some(sentinel_entries) = extra_sentinel_group {
        out.extend_from_slice(&(sentinel_entries.len() as u16).to_be_bytes());
        out.push(sentinel_entries.first().map(|e| e.0 as u8).unwrap_or(0));
        out.push(sentinel_entries.last().map(|e| e.0 as u8).unwrap_or(0));
        for &(ppem, ymax, ymin) in sentinel_entries {
            out.extend_from_slice(&ppem.to_be_bytes());
            out.extend_from_slice(&ymax.to_be_bytes());
            out.extend_from_slice(&ymin.to_be_bytes());
        }
    }
    out
}

fn build_synth_with_vdmx(
    num_glyphs: u16,
    entries: &[(u16, i16, i16)],
    extra_sentinel_group: Option<&[(u16, i16, i16)]>,
) -> Vec<u8> {
    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"head", make_head(1000, 0)),
        (b"hhea", make_hhea(1)),
        (b"maxp", make_maxp(num_glyphs)),
        (b"cmap", make_cmap_empty()),
        (b"name", make_name_empty()),
        (b"hmtx", make_hmtx(1, num_glyphs)),
        (b"loca", make_loca_short(num_glyphs)),
        (b"glyf", make_glyf_empty()),
        (b"VDMX", make_vdmx(1, entries, extra_sentinel_group)),
    ];
    build_minimal_sfnt(&tables)
}

#[test]
fn synth_vdmx_round_trips_through_font_accessors() {
    // 2-glyph synthetic font. Three vTable records at 10/14/20 ppem
    // with increasing yMax / decreasing yMin envelopes.
    let entries: &[(u16, i16, i16)] = &[(10, 8, -2), (14, 12, -3), (20, 17, -4)];
    let font_bytes = build_synth_with_vdmx(2, entries, None);
    let font = Font::from_bytes(&font_bytes).expect("parse synth");

    assert!(font.has_vdmx());
    let t = font.vdmx_table().expect("vdmx table");
    assert_eq!(t.version_raw(), 1);
    assert_eq!(t.num_ratios(), 1);
    assert_eq!(t.groups().len(), 1);
    assert_eq!(t.groups()[0].entries().len(), 3);
    assert_eq!(t.groups()[0].start_sz(), 10);
    assert_eq!(t.groups()[0].end_sz(), 20);

    // Square-pixel convenience and the explicit (1,1) lookup agree.
    assert_eq!(font.vdmx_y_extent_square(14), Some((12, -3)));
    assert_eq!(font.vdmx_y_extent_for_device(14, 1, 1), Some((12, -3)));

    // §5.7.8 has no "round down" rule — unrecorded ppem returns
    // `None`, not the nearest neighbour.
    assert_eq!(font.vdmx_y_extent_square(11), None);
    assert_eq!(font.vdmx_y_extent_square(15), None);

    // Non-matching device ratio (this table ships no sentinel).
    assert_eq!(font.vdmx_y_extent_for_device(14, 2, 1), None);
}

#[test]
fn synth_vdmx_sentinel_catches_non_square_ratio() {
    // First ratio is 1:1 with one record; the sentinel ratio binds a
    // second group with a different record. Devices reading at
    // (2,3) hit the sentinel and the second group.
    let entries = &[(12, 9, -2)];
    let sentinel_entries = &[(18, 14, -3)];
    let font_bytes = build_synth_with_vdmx(2, entries, Some(sentinel_entries));
    let font = Font::from_bytes(&font_bytes).expect("parse synth");

    assert!(font.has_vdmx());
    let t = font.vdmx_table().expect("vdmx table");
    assert_eq!(t.num_ratios(), 2);
    assert_eq!(t.groups().len(), 2);

    // 1:1 device picks the first group.
    assert_eq!(font.vdmx_y_extent_for_device(12, 1, 1), Some((9, -2)));
    // Non-1:1 device picks the sentinel group (recorded at 18 ppem only).
    assert_eq!(font.vdmx_y_extent_for_device(18, 2, 3), Some((14, -3)));
    // The 1:1 ratio's group does not record 18 ppem, even though the
    // sentinel's group does — first-match-wins forbids cross-group
    // fallback.
    assert_eq!(font.vdmx_y_extent_for_device(12, 2, 3), None);
    // The sentinel's group does not record 12 ppem, so a non-1:1
    // device at that ppem misses.
    assert_eq!(font.vdmx_y_extent_for_device(18, 1, 1), None);
}

#[test]
fn synth_vdmx_zero_offset_rejected_at_font_parse() {
    // Build a VDMX payload by hand whose Offset16[0] is zero (would
    // alias to the table header). The Font path must surface the
    // BadStructure rejection from VdmxTable::parse, not silently
    // walk the header bytes as a group.
    let mut vdmx = Vec::new();
    vdmx.extend_from_slice(&1u16.to_be_bytes()); // version
    vdmx.extend_from_slice(&1u16.to_be_bytes()); // numRecs
    vdmx.extend_from_slice(&1u16.to_be_bytes()); // numRatios
    vdmx.extend_from_slice(&[1, 1, 1, 1]); // RatioRange
    vdmx.extend_from_slice(&0u16.to_be_bytes()); // Offset16 = 0 (invalid)
    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"head", make_head(1000, 0)),
        (b"hhea", make_hhea(1)),
        (b"maxp", make_maxp(2)),
        (b"cmap", make_cmap_empty()),
        (b"name", make_name_empty()),
        (b"hmtx", make_hmtx(1, 2)),
        (b"loca", make_loca_short(2)),
        (b"glyf", make_glyf_empty()),
        (b"VDMX", vdmx),
    ];
    let font_bytes = build_minimal_sfnt(&tables);
    let err = Font::from_bytes(&font_bytes).expect_err("zero offset");
    assert!(matches!(err, oxideav_ttf::Error::BadStructure(_)));
}
