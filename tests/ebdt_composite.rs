//! `EBDT` composite glyph bitmaps — formats 8 and 9 (ISO/IEC
//! 14496-22:2019 §5.6.2.2.8 / §5.6.2.2.9).
//!
//! A composite (component-data) `EBDT` glyph carries no imagery of its
//! own: it lists other glyphs (by glyph ID) plus per-component
//! `(xOffset, yOffset)` placements, and the finished glyph is assembled by
//! copying each component's bitmap onto the composite's canvas. No public
//! font fixture in this crate ships composite embedded bitmaps, so this
//! test hand-stitches a TrueType-flavoured sfnt with a single 16-ppem
//! strike whose `EBLC` carries two index subtables:
//!
//!   * glyphs 5 and 6 — format-1 (small metrics, byte-aligned) 2×2 1-bpp
//!     pixel patterns, and
//!   * glyph 7 — a format-8 composite placing glyph 5 at (0, 0) and glyph 6
//!     at (2, 0), yielding a 4×2 assembled bitmap.
//!
//! `Font::glyph_gray_bitmap(7, 16)` must recursively resolve glyphs 5 and 6
//! out of the same strike and blit them at their offsets, returning the
//! composite's bounding metrics and the assembled pixel grid.

use oxideav_ttf::Font;

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

fn make_head() -> Vec<u8> {
    let mut h = vec![0u8; 54];
    h[0..4].copy_from_slice(&0x00010000u32.to_be_bytes());
    h[4..8].copy_from_slice(&0x00010000u32.to_be_bytes());
    h[12..16].copy_from_slice(&0x5F0F3CF5u32.to_be_bytes());
    h[18..20].copy_from_slice(&1000u16.to_be_bytes());
    h[50..52].copy_from_slice(&0i16.to_be_bytes());
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

/// Build the `EBDT` payload. Returns the table bytes plus the EBDT-relative
/// byte offsets (from the start of the table) where glyphs 5, 6 and 7
/// begin, so the EBLC index subtables can point at them.
///
/// * glyph 5 — format-1 2×2 1-bpp: rows `10` / `01`.
/// * glyph 6 — format-1 2×2 1-bpp: rows `01` / `10`.
/// * glyph 7 — format-8 composite: glyph 5 at (0,0), glyph 6 at (2,0); the
///   composite bbox is 4×2.
fn make_ebdt() -> (Vec<u8>, [u32; 4]) {
    let mut bytes = vec![0u8; 4];
    bytes[0..2].copy_from_slice(&2u16.to_be_bytes()); // major=2 (EBDT)

    // glyph 5: SmallGlyphMetrics h=2,w=2,bx=0,by=2,adv=2 + 2 byte-aligned rows.
    let g5 = bytes.len() as u32;
    bytes.extend_from_slice(&[2, 2, 0, 2, 2]);
    bytes.push(0x80); // row0 = 10xxxxxx
    bytes.push(0x40); // row1 = 01xxxxxx

    // glyph 6: same metrics, rows 01 / 10.
    let g6 = bytes.len() as u32;
    bytes.extend_from_slice(&[2, 2, 0, 2, 2]);
    bytes.push(0x40); // row0 = 01
    bytes.push(0x80); // row1 = 10

    // glyph 7: format-8 composite. SmallGlyphMetrics(5) + pad(1) +
    // numComponents(2) + EbdtComponent[2].
    let g7 = bytes.len() as u32;
    bytes.extend_from_slice(&[2, 4, 0, 2, 4]); // h=2,w=4,bx=0,by=2,adv=4
    bytes.push(0x00); // pad
    bytes.extend_from_slice(&2u16.to_be_bytes()); // numComponents
    bytes.extend_from_slice(&5u16.to_be_bytes()); // component 0 -> glyph 5
    bytes.push(0); // xOffset
    bytes.push(0); // yOffset
    bytes.extend_from_slice(&6u16.to_be_bytes()); // component 1 -> glyph 6
    bytes.push(2); // xOffset
    bytes.push(0); // yOffset
    let end = bytes.len() as u32;

    (bytes, [g5, g6, g7, end])
}

/// Build an `EBLC` carrying one 16-ppem strike with two index subtables:
///   * subtable A (format 1, imageFormat 1) covering glyphs 5..=6, and
///   * subtable B (format 1, imageFormat 8) covering glyph 7.
fn make_eblc(offsets: [u32; 4]) -> Vec<u8> {
    let [g5, g6, g7, end] = offsets;
    let mut bytes = vec![0u8; 8];
    bytes[0..2].copy_from_slice(&2u16.to_be_bytes()); // major=2 (EBLC)
    bytes[2..4].copy_from_slice(&0u16.to_be_bytes()); // minor
    bytes[4..8].copy_from_slice(&1u32.to_be_bytes()); // numSizes=1

    // BitmapSize record (48 bytes) at [8..56).
    let mut bm = vec![0u8; 48];
    let list_off = 56u32;
    bm[0..4].copy_from_slice(&list_off.to_be_bytes()); // indexSubTableArrayOffset
                                                       // indexTablesSize: 2 IndexSubTableArray records (16) + subtable A (8 hdr
                                                       // + 3 u32 offsets = 20) + subtable B (8 hdr + 2 u32 offsets = 16) = 52.
    bm[4..8].copy_from_slice(&52u32.to_be_bytes());
    bm[8..12].copy_from_slice(&2u32.to_be_bytes()); // numberOfIndexSubTables
    bm[12..16].copy_from_slice(&0u32.to_be_bytes()); // colorRef
                                                     // hori (12) + vert (12) line metrics left zero ([16..40)).
    bm[40..42].copy_from_slice(&5u16.to_be_bytes()); // startGlyphIndex
    bm[42..44].copy_from_slice(&7u16.to_be_bytes()); // endGlyphIndex
    bm[44] = 16; // ppemX
    bm[45] = 16; // ppemY
    bm[46] = 1; // bitDepth
    bm[47] = 0x01; // flags = horizontal
    bytes.extend_from_slice(&bm);

    // IndexSubTableArray at offset 56: two 8-byte records.
    // Record 0: glyphs 5..=6 -> subtable A (rel offset 16 from list start).
    bytes.extend_from_slice(&5u16.to_be_bytes());
    bytes.extend_from_slice(&6u16.to_be_bytes());
    bytes.extend_from_slice(&16u32.to_be_bytes());
    // Record 1: glyph 7..=7 -> subtable B (rel offset 36).
    bytes.extend_from_slice(&7u16.to_be_bytes());
    bytes.extend_from_slice(&7u16.to_be_bytes());
    bytes.extend_from_slice(&36u32.to_be_bytes());

    // Subtable A (format 1, imageFormat 1) at list+16 = file 72.
    // imageDataOffset is the EBDT base; sbitOffsets are deltas from it, so
    // we set imageDataOffset = 0 and store absolute EBDT offsets.
    bytes.extend_from_slice(&1u16.to_be_bytes()); // indexFormat
    bytes.extend_from_slice(&1u16.to_be_bytes()); // imageFormat = 1 (pixel)
    bytes.extend_from_slice(&0u32.to_be_bytes()); // imageDataOffset base
    bytes.extend_from_slice(&g5.to_be_bytes()); // glyph 5 start
    bytes.extend_from_slice(&g6.to_be_bytes()); // glyph 5 end / glyph 6 start
    bytes.extend_from_slice(&g7.to_be_bytes()); // glyph 6 end

    // Subtable B (format 1, imageFormat 8) at list+36.
    bytes.extend_from_slice(&1u16.to_be_bytes()); // indexFormat
    bytes.extend_from_slice(&8u16.to_be_bytes()); // imageFormat = 8 (composite)
    bytes.extend_from_slice(&0u32.to_be_bytes()); // imageDataOffset base
    bytes.extend_from_slice(&g7.to_be_bytes()); // glyph 7 start
    bytes.extend_from_slice(&end.to_be_bytes()); // glyph 7 end

    bytes
}

fn build_font(eblc: Vec<u8>, ebdt: Vec<u8>) -> Vec<u8> {
    let num_glyphs = 8u16;
    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"head", make_head()),
        (b"hhea", make_hhea(num_glyphs)),
        (b"maxp", make_maxp(num_glyphs)),
        (b"cmap", make_cmap_empty()),
        (b"name", make_name_empty()),
        (b"hmtx", make_hmtx(num_glyphs, num_glyphs)),
        (b"loca", make_loca_short(num_glyphs)),
        (b"glyf", make_glyf_empty()),
        (b"EBLC", eblc),
        (b"EBDT", ebdt),
    ];
    build_minimal_sfnt(&tables)
}

// ---- tests ---------------------------------------------------------------

#[test]
fn composite_assembles_two_components() {
    let (ebdt, offsets) = make_ebdt();
    let eblc = make_eblc(offsets);
    let font_bytes = build_font(eblc, ebdt);
    let font = Font::from_bytes(&font_bytes).expect("font parses");

    assert!(font.has_gray_bitmaps());

    // Sanity: the two pixel components resolve directly.
    let g5 = font.glyph_gray_bitmap(5, 16).expect("glyph 5");
    assert_eq!((g5.width, g5.height), (2, 2));
    assert_eq!(g5.pixels, vec![255, 0, 0, 255]); // 10 / 01
    let g6 = font.glyph_gray_bitmap(6, 16).expect("glyph 6");
    assert_eq!(g6.pixels, vec![0, 255, 255, 0]); // 01 / 10

    // The composite glyph 7 assembles 5 at (0,0) and 6 at (2,0) into a 4×2.
    let g7 = font.glyph_gray_bitmap(7, 16).expect("composite glyph 7");
    assert_eq!((g7.width, g7.height), (4, 2));
    assert_eq!(g7.advance, 4);
    assert_eq!(g7.bearing_y, 2);
    assert_eq!(g7.ppem, 16);
    // Row 0: [g5 row0 = 10][g6 row0 = 01] = 1,0,0,1 -> 255,0,0,255.
    // Row 1: [g5 row1 = 01][g6 row1 = 10] = 0,1,1,0 -> 0,255,255,0.
    assert_eq!(
        g7.pixels,
        vec![
            255, 0, 0, 255, // row 0
            0, 255, 255, 0, // row 1
        ]
    );
}

#[test]
fn composite_clips_out_of_bounds_components() {
    // Build the same font but shift component 6 to x=3 so its right column
    // lands at x=4, one past the 4-wide composite canvas (which clips).
    let (mut ebdt, offsets) = make_ebdt();
    let [_g5, _g6, g7, _end] = offsets;
    // Patch component 1's xOffset (at g7 + 5 metrics + 1 pad + 2 count + 4
    // first-component + 2 glyphID = +14). Reset to x=3.
    let patch = g7 as usize + 5 + 1 + 2 + 4 + 2;
    ebdt[patch] = 3;
    let eblc = make_eblc(offsets);
    let font_bytes = build_font(eblc, ebdt);
    let font = Font::from_bytes(&font_bytes).expect("font parses");

    let g7 = font.glyph_gray_bitmap(7, 16).expect("composite glyph 7");
    // Canvas is still 4×2. g5 at (0,0) fills cols 0-1; g6 at (3,0) fills
    // col 3 (its col-0 pixel) and clips col 4. g6 row0 = 0,1 -> col 3 = 0;
    // g6 row1 = 1,0 -> col 3 = 1.
    assert_eq!((g7.width, g7.height), (4, 2));
    // Row 0: g5 = 1,0 at cols 0,1; col 2 untouched (0); col 3 = g6[0][0]=0.
    // Row 1: g5 = 0,1 at cols 0,1; col 2 = 0; col 3 = g6[1][0]=1.
    assert_eq!(
        g7.pixels,
        vec![
            255, 0, 0, 0, // row 0
            0, 255, 0, 255, // row 1
        ]
    );
}
