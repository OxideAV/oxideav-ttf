//! `USE_MY_METRICS` composite advance/LSB inheritance (ISO/IEC
//! 14496-22:2019 §5.3.4).
//!
//! When a composite glyph references a component carrying the
//! `USE_MY_METRICS` flag (bit 9), the composite's advance width and side
//! bearings are forced to equal that component's `hmtx` values — the spec's
//! mechanism for making e.g. `i`-circumflex inherit dotless-`i`'s metrics.
//! This test hand-stitches a font with:
//!
//!   * glyph 1 — a simple triangle (advance 600, lsb 50),
//!   * glyph 2 — a simple triangle (advance 999, lsb 17), and
//!   * glyph 3 — a composite referencing glyph 1 (plain) then glyph 2 with
//!     `USE_MY_METRICS`; the composite's *own* hmtx entry is advance 100,
//!     lsb 5.
//!
//! `Font::glyph_advance(3)` must return 999 and `Font::glyph_lsb(3)` 17 (the
//! flagged component glyph 2's metrics), not the composite's own 100 / 5.

use oxideav_ttf::Font;

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

fn make_head() -> Vec<u8> {
    let mut h = vec![0u8; 54];
    h[0..4].copy_from_slice(&0x00010000u32.to_be_bytes());
    h[12..16].copy_from_slice(&0x5F0F3CF5u32.to_be_bytes());
    h[18..20].copy_from_slice(&1000u16.to_be_bytes());
    h[50..52].copy_from_slice(&1i16.to_be_bytes()); // long loca
    h
}

fn make_hhea(num_h_metrics: u16) -> Vec<u8> {
    let mut h = vec![0u8; 36];
    h[0..4].copy_from_slice(&0x00010000u32.to_be_bytes());
    h[34..36].copy_from_slice(&num_h_metrics.to_be_bytes());
    h
}

fn make_maxp(num_glyphs: u16) -> Vec<u8> {
    let mut h = vec![0u8; 32]; // version 1.0 maxp (TrueType)
    h[0..4].copy_from_slice(&0x00010000u32.to_be_bytes());
    h[4..6].copy_from_slice(&num_glyphs.to_be_bytes());
    // maxComponentDepth at the end — leave a modest value.
    h[30..32].copy_from_slice(&2u16.to_be_bytes());
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
    sub.extend_from_slice(&0u16.to_be_bytes());
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
    out[4..6].copy_from_slice(&6u16.to_be_bytes());
    out
}

/// hmtx for 4 glyphs (0=.notdef, 1, 2, 3=composite).
fn make_hmtx() -> Vec<u8> {
    let mut out = Vec::new();
    let entries: [(u16, i16); 4] = [
        (1000, 0), // .notdef
        (600, 50), // glyph 1
        (999, 17), // glyph 2 (the USE_MY_METRICS source)
        (100, 5),  // glyph 3 (composite's OWN metrics)
    ];
    for (aw, lsb) in entries {
        out.extend_from_slice(&aw.to_be_bytes());
        out.extend_from_slice(&lsb.to_be_bytes());
    }
    out
}

/// A simple triangle glyph: 1 contour, 3 on-curve points.
fn make_triangle() -> Vec<u8> {
    let mut g = Vec::new();
    g.extend_from_slice(&1i16.to_be_bytes()); // numberOfContours
    g.extend_from_slice(&0i16.to_be_bytes()); // xMin
    g.extend_from_slice(&0i16.to_be_bytes()); // yMin
    g.extend_from_slice(&100i16.to_be_bytes()); // xMax
    g.extend_from_slice(&100i16.to_be_bytes()); // yMax
    g.extend_from_slice(&2u16.to_be_bytes()); // endPtsOfContours[0] = 2
    g.extend_from_slice(&0u16.to_be_bytes()); // instructionLength
                                              // 3 points, all on-curve (flag 0x01), X/Y as shorts.
    g.push(0x01);
    g.push(0x01);
    g.push(0x01);
    // X coords (delta, i16): 0, 100, -50.
    g.extend_from_slice(&0i16.to_be_bytes());
    g.extend_from_slice(&100i16.to_be_bytes());
    g.extend_from_slice(&(-50i16).to_be_bytes());
    // Y coords: 0, 0, 100.
    g.extend_from_slice(&0i16.to_be_bytes());
    g.extend_from_slice(&0i16.to_be_bytes());
    g.extend_from_slice(&100i16.to_be_bytes());
    g
}

/// Composite glyph 3: component glyph 1 (plain), then glyph 2 with
/// USE_MY_METRICS.
fn make_composite() -> Vec<u8> {
    const ARGS_ARE_XY_VALUES: u16 = 0x0002;
    const MORE_COMPONENTS: u16 = 0x0020;
    const USE_MY_METRICS: u16 = 0x0200;
    let mut g = Vec::new();
    g.extend_from_slice(&(-1i16).to_be_bytes()); // composite
    g.extend_from_slice(&0i16.to_be_bytes());
    g.extend_from_slice(&0i16.to_be_bytes());
    g.extend_from_slice(&100i16.to_be_bytes());
    g.extend_from_slice(&100i16.to_be_bytes());
    // Component 0: glyph 1, XY (0,0), more follow.
    g.extend_from_slice(&(ARGS_ARE_XY_VALUES | MORE_COMPONENTS).to_be_bytes());
    g.extend_from_slice(&1u16.to_be_bytes());
    g.push(0);
    g.push(0);
    // Component 1: glyph 2, XY (0,0), USE_MY_METRICS, last.
    g.extend_from_slice(&(ARGS_ARE_XY_VALUES | USE_MY_METRICS).to_be_bytes());
    g.extend_from_slice(&2u16.to_be_bytes());
    g.push(0);
    g.push(0);
    g
}

fn build_font() -> Vec<u8> {
    let tri1 = make_triangle();
    let tri2 = make_triangle();
    let comp = make_composite();
    // glyf: glyph 0 empty, then tri1, tri2, composite.
    let mut glyf = Vec::new();
    let off0 = glyf.len() as u32; // 0 (empty .notdef)
    let off1 = glyf.len() as u32;
    glyf.extend_from_slice(&tri1);
    let off2 = glyf.len() as u32;
    glyf.extend_from_slice(&tri2);
    let off3 = glyf.len() as u32;
    glyf.extend_from_slice(&comp);
    let off4 = glyf.len() as u32;
    // long loca: offsets for glyphs 0..=4.
    let mut loca = Vec::new();
    for v in [off0, off1, off2, off3, off4] {
        loca.extend_from_slice(&v.to_be_bytes());
    }

    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"head", make_head()),
        (b"hhea", make_hhea(4)),
        (b"maxp", make_maxp(4)),
        (b"cmap", make_cmap_empty()),
        (b"name", make_name_empty()),
        (b"hmtx", make_hmtx()),
        (b"loca", loca),
        (b"glyf", glyf),
    ];
    build_minimal_sfnt(&tables)
}

#[test]
fn composite_inherits_flagged_component_metrics() {
    let bytes = build_font();
    let font = Font::from_bytes(&bytes).expect("font parses");

    // Sanity: the simple glyphs report their own metrics.
    assert_eq!(font.glyph_advance(1), 600);
    assert_eq!(font.glyph_lsb(1), 50);
    assert_eq!(font.glyph_advance(2), 999);
    assert_eq!(font.glyph_lsb(2), 17);

    // The composite glyph 3 has its OWN hmtx entry (advance 100, lsb 5),
    // but component glyph 2 carries USE_MY_METRICS, so glyph 3 inherits
    // glyph 2's advance (999) and lsb (17).
    assert_eq!(font.glyph_advance(3), 999);
    assert_eq!(font.glyph_lsb(3), 17);
}
