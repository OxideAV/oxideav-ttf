//! Integration coverage for the `hdmx` accessors on
//! [`oxideav_ttf::Font`].
//!
//! Two paths:
//!
//! * **Absent path** — DejaVu Sans Mono / DejaVu Sans / Inter, all of
//!   which do not ship `hdmx`. Every accessor must return
//!   `None` / `false` / an empty vec.
//!
//! * **Synthetic path** — a TrueType-flavoured sfnt that ships an
//!   `hdmx` table with two device records at distinct ppem sizes plus
//!   a third with long-alignment padding past the per-record body.
//!   We verify the table parses, the per-glyph accessors round-trip
//!   the recorded advances, [`Font::hdmx_advance_pixels`] honours
//!   §5.7.2's "exact ppem only" rule (no nearest-neighbour fallback),
//!   and a `widths[]` length disagreement against `maxp.numGlyphs`
//!   is rejected at parse time as `BadStructure` / `UnexpectedEof`.

use oxideav_ttf::Font;

const DEJAVU_MONO: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");
const DEJAVU_SANS: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");
const INTER: &[u8] = include_bytes!("fixtures/InterVariable.ttf");

#[test]
fn dejavu_mono_has_no_hdmx() {
    let f = Font::from_bytes(DEJAVU_MONO).unwrap();
    assert!(!f.has_hdmx());
    assert!(f.hdmx_table().is_none());
    assert_eq!(f.hdmx_advance_pixels(0, 12), None);
    assert!(f.hdmx_recorded_ppem_sizes().is_empty());
}

#[test]
fn dejavu_sans_has_no_hdmx() {
    let f = Font::from_bytes(DEJAVU_SANS).unwrap();
    assert!(!f.has_hdmx());
}

#[test]
fn inter_has_no_hdmx() {
    let f = Font::from_bytes(INTER).unwrap();
    assert!(!f.has_hdmx());
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

/// Build an `hdmx` payload from a list of `(pixel_size, max_width,
/// widths[])` tuples, rounding `sizeDeviceRecord` up to the §5.7.2
/// long-alignment boundary so the result mirrors what a real font
/// writer would emit.
fn make_hdmx(num_glyphs: usize, recs: &[(u8, u8, &[u8])]) -> Vec<u8> {
    let min_stride = 2 + num_glyphs;
    let stride = (min_stride + 3) & !3;
    let mut out = Vec::with_capacity(8 + stride * recs.len());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(recs.len() as i16).to_be_bytes());
    out.extend_from_slice(&(stride as i32).to_be_bytes());
    for &(ppem, max_w, widths) in recs {
        assert_eq!(widths.len(), num_glyphs);
        out.push(ppem);
        out.push(max_w);
        out.extend_from_slice(widths);
        let pad = stride - 2 - num_glyphs;
        out.resize(out.len() + pad, 0);
    }
    out
}

fn build_synth_with_hdmx(num_glyphs: u16, recs: &[(u8, u8, &[u8])]) -> Vec<u8> {
    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"head", make_head(1000, 0)),
        (b"hhea", make_hhea(1)),
        (b"maxp", make_maxp(num_glyphs)),
        (b"cmap", make_cmap_empty()),
        (b"name", make_name_empty()),
        (b"hmtx", make_hmtx(1, num_glyphs)),
        (b"loca", make_loca_short(num_glyphs)),
        (b"glyf", make_glyf_empty()),
        (b"hdmx", make_hdmx(num_glyphs as usize, recs)),
    ];
    build_minimal_sfnt(&tables)
}

#[test]
fn synth_hdmx_round_trips_through_font_accessors() {
    // 4-glyph synthetic font. Records at 12 and 16 ppem; the advances
    // for gid 0 (.notdef) are 0 in both; gid 1..3 widen with ppem.
    let num_glyphs = 4u16;
    let recs: &[(u8, u8, &[u8])] = &[
        (12, 9, &[0, 6, 7, 9]),
        (16, 12, &[0, 8, 10, 12]),
        (20, 15, &[0, 10, 12, 15]),
    ];
    let font_bytes = build_synth_with_hdmx(num_glyphs, recs);
    let font = Font::from_bytes(&font_bytes).expect("parse synth");

    assert!(font.has_hdmx());
    let t = font.hdmx_table().expect("hdmx table");
    assert_eq!(t.version_raw(), 0);
    assert_eq!(t.num_records(), 3);
    assert_eq!(font.hdmx_recorded_ppem_sizes(), vec![12, 16, 20]);

    // Per-(glyph, ppem) advance lookup.
    assert_eq!(font.hdmx_advance_pixels(0, 12), Some(0));
    assert_eq!(font.hdmx_advance_pixels(1, 12), Some(6));
    assert_eq!(font.hdmx_advance_pixels(2, 16), Some(10));
    assert_eq!(font.hdmx_advance_pixels(3, 20), Some(15));

    // §5.7.2 has no "round down" rule — an unrecorded ppem returns
    // `None`, not the nearest neighbour.
    assert_eq!(font.hdmx_advance_pixels(1, 14), None);
    assert_eq!(font.hdmx_advance_pixels(1, 11), None);
    assert_eq!(font.hdmx_advance_pixels(1, 255), None);

    // Out-of-range glyph IDs return `None` at every ppem.
    assert_eq!(font.hdmx_advance_pixels(num_glyphs, 12), None);

    // Per-record accessor surfaces the on-wire `maxWidth` byte
    // unchanged.
    let r12 = t.record_for_ppem(12).expect("ppem 12 record");
    assert_eq!(r12.pixel_size(), 12);
    assert_eq!(r12.max_width(), 9);
    assert_eq!(r12.widths(), &[0, 6, 7, 9]);
}

#[test]
fn synth_hdmx_stride_below_widths_length_rejected_at_font_parse() {
    // maxp.numGlyphs = 16 but the on-wire `sizeDeviceRecord` claims a
    // stride of 4 bytes — less than the 2-byte per-record header +
    // 16 widths the §5.7.2 record layout requires. The Font path runs
    // HdmxTable::parse with expected_num_glyphs = 16 and must reject
    // the undersized stride as BadStructure rather than silently
    // walking off the end of the per-glyph widths array.
    let num_glyphs = 16u16;
    let mut bad_hdmx = Vec::new();
    bad_hdmx.extend_from_slice(&0u16.to_be_bytes()); // version
    bad_hdmx.extend_from_slice(&1i16.to_be_bytes()); // numRecords
    bad_hdmx.extend_from_slice(&4i32.to_be_bytes()); // sizeDeviceRecord = 4
    bad_hdmx.extend_from_slice(&[12u8, 0, 0, 0]); // header + filler
    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"head", make_head(1000, 0)),
        (b"hhea", make_hhea(1)),
        (b"maxp", make_maxp(num_glyphs)),
        (b"cmap", make_cmap_empty()),
        (b"name", make_name_empty()),
        (b"hmtx", make_hmtx(1, num_glyphs)),
        (b"loca", make_loca_short(num_glyphs)),
        (b"glyf", make_glyf_empty()),
        (b"hdmx", bad_hdmx),
    ];
    let font_bytes = build_minimal_sfnt(&tables);
    let err = Font::from_bytes(&font_bytes).expect_err("undersized stride");
    assert!(matches!(err, oxideav_ttf::Error::BadStructure(_)));
}

#[test]
fn synth_hdmx_single_record_table() {
    // §5.7.2 doesn't require multiple records; a one-record table is
    // legal and the parser must surface it.
    let num_glyphs = 2u16;
    let font_bytes = build_synth_with_hdmx(num_glyphs, &[(13, 4, &[0, 4])]);
    let font = Font::from_bytes(&font_bytes).expect("parse synth");
    assert!(font.has_hdmx());
    assert_eq!(font.hdmx_recorded_ppem_sizes(), vec![13]);
    assert_eq!(font.hdmx_advance_pixels(1, 13), Some(4));
    assert_eq!(font.hdmx_advance_pixels(1, 12), None);
}
