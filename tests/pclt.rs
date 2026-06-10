//! Integration coverage for the `PCLT` accessors on
//! [`oxideav_ttf::Font`].
//!
//! Two paths:
//!
//! * **Absent path** — DejaVu Sans Mono / DejaVu Sans / Inter, none
//!   of which ship `PCLT` (the table is "strongly discouraged for
//!   OFF fonts with TrueType outlines" per ISO/IEC 14496-22:2019
//!   §5.7.7). `has_pclt()` must be `false` and `pclt_table()` must
//!   return `None`.
//!
//! * **Synthetic path** — a TrueType-flavoured sfnt that ships a
//!   §5.7.7-shaped `PCLT` exercising the packed-word decoders
//!   (FontNumber segments, Style bits, TypeFamily vendor/family,
//!   SymbolSet number/ID), the fixed-size string fields (Typeface /
//!   CharacterComplement / FileName), and the classification bytes
//!   (StrokeWeight / WidthType / SerifStyle). We also verify that a
//!   `majorVersion != 1` table fails the whole-font parse rather
//!   than degrading silently.

use oxideav_ttf::{Font, PCLT_TABLE_LEN};

const DEJAVU_MONO: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");
const DEJAVU_SANS: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");
const INTER: &[u8] = include_bytes!("fixtures/InterVariable.ttf");

#[test]
fn dejavu_mono_has_no_pclt() {
    let f = Font::from_bytes(DEJAVU_MONO).unwrap();
    assert!(!f.has_pclt());
    assert!(f.pclt_table().is_none());
}

#[test]
fn dejavu_sans_has_no_pclt() {
    let f = Font::from_bytes(DEJAVU_SANS).unwrap();
    assert!(!f.has_pclt());
}

#[test]
fn inter_has_no_pclt() {
    let f = Font::from_bytes(INTER).unwrap();
    assert!(!f.has_pclt());
}

// ---- synthetic-font scaffolding ------------------------------------------
//
// Minimal sfnt layout: 12-byte header + 16-byte-per-record directory +
// payloads, each padded to 4-byte boundaries per §4.5.2. Offsets in the
// directory records are file-relative, which is what the parent
// `parser::TableDirectory` expects (and exactly what real sfnts use for
// a non-TTC font).

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

/// Build a §5.7.7-shaped `PCLT` payload. The shape mirrors the spec's
/// own worked examples: Times New (text weight, upright), PCL symbol
/// set 19U (decimal 629), Windows 3.1 "ANSI" character complement.
fn make_pclt(major_version: u16) -> Vec<u8> {
    let mut b = Vec::with_capacity(PCLT_TABLE_LEN);
    b.extend_from_slice(&major_version.to_be_bytes()); // majorVersion
    b.extend_from_slice(&0u16.to_be_bytes()); // minorVersion
                                              // FontNumber: native (bit 31 clear), vendor 'M', assigned 7.
    let font_number: u32 = (u32::from(b'M') << 24) | 7;
    b.extend_from_slice(&font_number.to_be_bytes());
    b.extend_from_slice(&569u16.to_be_bytes()); // Pitch
    b.extend_from_slice(&1062u16.to_be_bytes()); // xHeight
                                                 // Style: structure 0 (solid), width 0 (normal), posture 1 (italic).
    b.extend_from_slice(&1u16.to_be_bytes());
    // TypeFamily: vendor 4 (Monotype) + family code 517.
    b.extend_from_slice(&((4u16 << 12) | 517).to_be_bytes());
    b.extend_from_slice(&1466u16.to_be_bytes()); // CapHeight
    b.extend_from_slice(&629u16.to_be_bytes()); // SymbolSet (PCL 19U)
    let mut typeface = [b' '; 16];
    typeface[..9].copy_from_slice(b"Times New");
    b.extend_from_slice(&typeface); // Typeface[16]
                                    // CharacterComplement: Windows 3.1 "ANSI" per the §5.7.7 example.
    b.extend_from_slice(&0xFFFF_FFFF_37FF_FFFEu64.to_be_bytes());
    b.extend_from_slice(b"TNRI00"); // FileName[6]
    b.push(0u8); // StrokeWeight (0 = Book / text / regular)
    b.push(0u8); // WidthType (0 = Normal)
    b.push((2 << 6) | 6); // SerifStyle: Serif/Contrasting + Serif Bracket
    b.push(0u8); // Reserved
    assert_eq!(b.len(), PCLT_TABLE_LEN);
    b
}

/// Build a TrueType-flavoured sfnt carrying a `PCLT` table alongside
/// the §5.2.1 required set (`head`, `hhea`, `maxp`, `cmap`, `name`,
/// `hmtx`, plus the TrueType-outline `loca` / `glyf` pair).
fn build_synth_with_pclt(major_version: u16) -> Vec<u8> {
    let num_glyphs = 4u16;
    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"head", make_head(2048, 0)),
        (b"hhea", make_hhea(1)),
        (b"maxp", make_maxp(num_glyphs)),
        (b"cmap", make_cmap_empty()),
        (b"name", make_name_empty()),
        (b"hmtx", make_hmtx(1, num_glyphs)),
        (b"loca", make_loca_short(num_glyphs)),
        (b"glyf", make_glyf_empty()),
        (b"PCLT", make_pclt(major_version)),
    ];
    build_minimal_sfnt(&tables)
}

#[test]
fn synth_pclt_round_trips_through_font_accessors() {
    let font_bytes = build_synth_with_pclt(1);
    let font = Font::from_bytes(&font_bytes).expect("parse synth");

    assert!(font.has_pclt());
    let t = font.pclt_table().expect("PCLT table");

    // Version 1.0 per §5.7.7 "The current PCLT table version is 1.0."
    assert_eq!(t.major_version(), 1);
    assert_eq!(t.minor_version(), 0);

    // FontNumber segments: native flag + vendor letter + assigned id.
    assert!(t.font_number_is_native());
    assert_eq!(t.font_number_vendor_code(), b'M');
    assert_eq!(t.font_number_vendor_assigned(), 7);

    // Design-unit metrics.
    assert_eq!(t.pitch(), 569);
    assert_eq!(t.x_height(), 1062);
    assert_eq!(t.cap_height(), 1466);

    // Style word: solid / normal / italic.
    assert_eq!(t.style_structure(), 0);
    assert_eq!(t.style_width(), 0);
    assert_eq!(t.style_posture(), 1);

    // TypeFamily word: Monotype vendor code + family 517.
    assert_eq!(t.type_family_vendor_code(), 4);
    assert_eq!(t.type_family_code(), 517);

    // SymbolSet 629 = PCL 19U per the §5.7.7 example table.
    assert_eq!(t.symbol_set_number(), 19);
    assert_eq!(t.symbol_set_id(), b'U');

    // Fixed-size strings.
    assert_eq!(t.typeface(), Some("Times New"));
    assert_eq!(t.file_name(), Some("TNRI00"));
    assert_eq!(t.file_name_treatment(), b'I'); // italic treatment

    // Windows 3.1 "ANSI" complement: ASCII + Latin 1 ext + DTP ext
    // provided (cleared bits 31 / 30 / 27), Unicode index order
    // (cleared bit 0).
    assert!(t.provides_collection(31));
    assert!(t.provides_collection(30));
    assert!(t.provides_collection(27));
    assert!(!t.provides_collection(29));
    assert!(t.is_unicode_indexed());

    // Classification bytes.
    assert_eq!(t.stroke_weight(), 0);
    assert!(t.stroke_weight_is_valid());
    assert_eq!(t.width_type(), 0);
    assert!(t.width_type_is_valid());
    assert_eq!(t.serif_style_class(), 2);
    assert_eq!(t.serif_style_value(), 6);

    assert_eq!(t.reserved(), 0);
}

#[test]
fn synth_pclt_bad_major_version_fails_font_parse() {
    // §5.7.7 defines only version 1.0; majorVersion 2 is rejected as
    // BadStructure, and the whole-font parse surfaces it rather than
    // silently dropping the table.
    let font_bytes = build_synth_with_pclt(2);
    assert!(Font::from_bytes(&font_bytes).is_err());
}
