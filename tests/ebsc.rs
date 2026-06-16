//! `EBSC` — Embedded Bitmap Scaling table (ISO/IEC 14496-22:2019 §5.6.4).
//!
//! `EBSC` redirects a requested ppem to a real `EBLC`/`EBDT` strike,
//! scaling that strike's glyph metrics by the target / substitute ppem
//! ratio. No public font fixture in this crate ships `EBSC` (it is rare —
//! the spec motivates it with small-Kanji bitmap faces), so this test
//! hand-stitches a TrueType-flavoured sfnt carrying:
//!
//!   * one real 16-ppem strike in `EBLC` + `EBDT` (glyph 5, a 4×3 1-bpp
//!     pattern with advance 4), and
//!   * an `EBSC` `BitmapScale` synthesising a 32-ppem strike from that
//!     16-ppem substitute (a 2× scale in both axes).
//!
//! `Font::glyph_gray_bitmap_scaled(5, 32)` must return the substitute
//! strike's pixels with width/height/advance doubled and the bearings
//! scaled per §5.6.4 ("Glyph metrics are scaled by the same factor as the
//! pixels per Em … and are rounded to the nearest integer pixel").

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
    h[18..20].copy_from_slice(&1000u16.to_be_bytes()); // unitsPerEm
    h[50..52].copy_from_slice(&0i16.to_be_bytes()); // short loca
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

/// Build a minimal `EBLC` carrying one strike at `ppem`, covering only
/// glyph 5, via an IndexSubTable format 1 (u32 offsets) that points at
/// EBDT byte offset 4 (right after the EBDT 4-byte header) with the entry
/// length `entry_len`.
fn make_eblc(ppem: u8, entry_len: u32) -> Vec<u8> {
    // Layout:
    //   [0..8)   EblcHeader (major=2, minor=0, numSizes=1)
    //   [8..56)  BitmapSize record (48 bytes)
    //   [56..64) IndexSubTableArray: one IndexSubTableRecord (8 bytes)
    //   [64..)   IndexSubTable format 1
    let mut bytes = vec![0u8; 56];
    bytes[0..2].copy_from_slice(&2u16.to_be_bytes()); // major (EBLC)
    bytes[2..4].copy_from_slice(&0u16.to_be_bytes()); // minor
    bytes[4..8].copy_from_slice(&1u32.to_be_bytes()); // numSizes

    let list_off = 56u32;
    let base = 8;
    bytes[base..base + 4].copy_from_slice(&list_off.to_be_bytes()); // indexSubTableArrayOffset
                                                                    // indexTablesSize = IndexSubTableArray record (8) + IndexSubTable
                                                                    // format-1 header (8) + sbitOffsets[2] (8) = 24.
    bytes[base + 4..base + 8].copy_from_slice(&24u32.to_be_bytes());
    bytes[base + 8..base + 12].copy_from_slice(&1u32.to_be_bytes()); // numberOfIndexSubTables
    bytes[base + 12..base + 16].copy_from_slice(&0u32.to_be_bytes()); // colorRef
                                                                      // hori (12) + vert (12) line metrics zeroed.
    let after = base + 16 + 24;
    bytes[after..after + 2].copy_from_slice(&5u16.to_be_bytes()); // startGlyphIndex
    bytes[after + 2..after + 4].copy_from_slice(&5u16.to_be_bytes()); // endGlyphIndex
    bytes[after + 4] = ppem; // ppemX
    bytes[after + 5] = ppem; // ppemY
    bytes[after + 6] = 1; // bitDepth = 1 (monochrome)
    bytes[after + 7] = 0x01; // flags = horizontal

    // IndexSubTableArray: one record at offset 56.
    bytes.extend_from_slice(&5u16.to_be_bytes()); // firstGlyphIndex
    bytes.extend_from_slice(&5u16.to_be_bytes()); // lastGlyphIndex
    bytes.extend_from_slice(&8u32.to_be_bytes()); // indexSubTableOffset (rel to list)

    // IndexSubTable format 1 at offset 64.
    bytes.extend_from_slice(&1u16.to_be_bytes()); // indexFormat = 1 (u32 offsets)
    bytes.extend_from_slice(&1u16.to_be_bytes()); // imageFormat = 1 (small, byte-aligned)
    bytes.extend_from_slice(&4u32.to_be_bytes()); // imageDataOffset (into EBDT, past header)
                                                  // sbitOffsets[2]: glyph 5 spans [0, entry_len).
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&entry_len.to_be_bytes());
    bytes
}

/// Build an `EBDT` whose single glyph-5 entry is a format-1 (small
/// metrics, byte-aligned) 4×3 1-bpp pattern with advance 4. Returns the
/// table bytes and the entry length so the EBLC can point at it.
fn make_ebdt() -> (Vec<u8>, u32) {
    let mut bytes = vec![0u8; 4]; // EbdtHeader: major=2, minor=0
    bytes[0..2].copy_from_slice(&2u16.to_be_bytes());
    // SmallGlyphMetrics: height=3, width=4, bearingX=1, bearingY=3, advance=4.
    let entry_off = bytes.len();
    bytes.extend_from_slice(&[3, 4, 1, 3, 4]);
    // 4-wide rows, 1 bpp, byte-aligned: one byte per row.
    // row0 = 1010 -> 0xA0, row1 = 0101 -> 0x50, row2 = 1111 -> 0xF0.
    bytes.push(0xA0);
    bytes.push(0x50);
    bytes.push(0xF0);
    let entry_len = (bytes.len() - entry_off) as u32;
    (bytes, entry_len)
}

/// Build an `EBSC` with one `BitmapScale`: target ppem 32, substitute
/// ppem 16 (a 2× scale in both axes).
fn make_ebsc(target_ppem: u8, substitute_ppem: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&2u16.to_be_bytes()); // majorVersion = 2
    bytes.extend_from_slice(&0u16.to_be_bytes()); // minorVersion = 0
    bytes.extend_from_slice(&1u32.to_be_bytes()); // numSizes = 1
                                                  // BitmapScale: hori (12) + vert (12) line metrics, then 4 ppem bytes.
    bytes.extend_from_slice(&[0u8; 24]);
    bytes.push(target_ppem); // ppemX
    bytes.push(target_ppem); // ppemY
    bytes.push(substitute_ppem); // substitutePpemX
    bytes.push(substitute_ppem); // substitutePpemY
    bytes
}

fn build_font(eblc: Vec<u8>, ebdt: Vec<u8>, ebsc: Option<Vec<u8>>) -> Vec<u8> {
    let num_glyphs = 8u16;
    let mut tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
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
    if let Some(e) = ebsc {
        tables.push((b"EBSC", e));
    }
    build_minimal_sfnt(&tables)
}

// ---- tests ---------------------------------------------------------------

#[test]
fn scaled_strike_doubles_metrics_and_passes_pixels_through() {
    let (ebdt, entry_len) = make_ebdt();
    let eblc = make_eblc(16, entry_len);
    let ebsc = make_ebsc(32, 16);
    let font_bytes = build_font(eblc, ebdt, Some(ebsc));
    let font = Font::from_bytes(&font_bytes).expect("font parses");

    assert!(font.has_ebsc());
    assert_eq!(font.ebsc_target_sizes(), vec![(32, 32)]);

    // Sanity: the real 16-ppem strike resolves directly.
    let real = font.glyph_gray_bitmap(5, 16).expect("real strike");
    assert_eq!((real.width, real.height), (4, 3));
    assert_eq!(real.advance, 4);
    // 4×3 pattern: row0 1010, row1 0101, row2 1111.
    assert_eq!(
        real.pixels,
        vec![
            255, 0, 255, 0, // row 0
            0, 255, 0, 255, // row 1
            255, 255, 255, 255, // row 2
        ]
    );

    // The synthesised 32-ppem strike: a 2× scale of the 16-ppem source.
    let scaled = font
        .glyph_gray_bitmap_scaled(5, 32)
        .expect("scaled strike at 32 ppem");
    assert_eq!((scaled.width, scaled.height), (8, 6)); // 4*2, 3*2
    assert_eq!(scaled.advance, 8); // 4*2
    assert_eq!(scaled.bearing_x, 2); // 1 * 32/16
    assert_eq!(scaled.bearing_y, 6); // 3 * 32/16
    assert_eq!(scaled.ppem, 32); // reported as the synthesised target
    assert_eq!(scaled.bit_depth, 1);
    // §5.6.4 leaves resampling to the rasteriser; the source pixels pass
    // through unchanged so the consumer can resample at its own filter.
    assert_eq!(scaled.pixels, real.pixels);
}

#[test]
fn scaled_lookup_misses_unlisted_target_ppem() {
    let (ebdt, entry_len) = make_ebdt();
    let eblc = make_eblc(16, entry_len);
    let ebsc = make_ebsc(32, 16);
    let font_bytes = build_font(eblc, ebdt, Some(ebsc));
    let font = Font::from_bytes(&font_bytes).expect("font parses");

    // No BitmapScale record targets 40 ppem.
    assert!(font.glyph_gray_bitmap_scaled(5, 40).is_none());
    // Glyph not present in the substitute strike.
    assert!(font.glyph_gray_bitmap_scaled(2, 32).is_none());
}

#[test]
fn ebsc_table_introspection() {
    let (ebdt, entry_len) = make_ebdt();
    let eblc = make_eblc(16, entry_len);
    let ebsc = make_ebsc(48, 24);
    let font_bytes = build_font(eblc, ebdt, Some(ebsc));
    let font = Font::from_bytes(&font_bytes).expect("font parses");

    let table = font.ebsc_table().expect("EBSC present");
    assert_eq!(table.num_scales(), 1);
    assert_eq!(table.minor_version(), 0);
    let scale = &table.scales()[0];
    assert_eq!((scale.ppem_x, scale.ppem_y), (48, 48));
    assert_eq!((scale.substitute_ppem_x, scale.substitute_ppem_y), (24, 24));
}

#[test]
fn font_without_ebsc_reports_absent() {
    let (ebdt, entry_len) = make_ebdt();
    let eblc = make_eblc(16, entry_len);
    let font_bytes = build_font(eblc, ebdt, None);
    let font = Font::from_bytes(&font_bytes).expect("font parses");

    assert!(!font.has_ebsc());
    assert!(font.ebsc_table().is_none());
    assert!(font.ebsc_target_sizes().is_empty());
    assert!(font.glyph_gray_bitmap_scaled(5, 32).is_none());
    // The real strike still resolves through the direct path.
    assert!(font.glyph_gray_bitmap(5, 16).is_some());
}
