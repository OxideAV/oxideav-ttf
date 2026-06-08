//! Integration coverage for the `meta` accessors on [`oxideav_ttf::Font`].
//!
//! Two paths:
//!
//! * **Absent path** — DejaVu Sans Mono / DejaVu Sans / Inter
//!   Variable / Noto Sans Arabic all ship without a `meta` table
//!   (the table is optional per ISO/IEC 14496-22:2019 §5.7.6); every
//!   `meta_*` accessor must return `None`.
//!
//! * **Present path** — a synthesised TrueType sfnt that carries a
//!   `meta` table with the two registered tags `'dlng'` + `'slng'`
//!   plus a vendor-private tag, exercised via [`Font::meta_table`],
//!   [`Font::meta_record`], [`Font::meta_design_languages`], and
//!   [`Font::meta_supported_languages`].

use oxideav_ttf::Font;
use oxideav_ttf::{
    script_lang_tags, META_DATA_MAP_LEN, META_HEADER_LEN, META_TAG_DLNG, META_TAG_SLNG,
    META_VERSION_1,
};

const DEJAVU_MONO: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");
const DEJAVU_SANS: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");
const INTER_VARIABLE: &[u8] = include_bytes!("fixtures/InterVariable.ttf");
const NOTO_ARABIC: &[u8] = include_bytes!("fixtures/NotoSansArabic-Regular.ttf");

#[test]
fn shipped_fixtures_omit_meta_table() {
    // §5.7.6 declares the table optional; none of the OFL / Bitstream
    // Vera fixtures bundled with this crate ship one. The accessor
    // surface must return `None` end-to-end on each.
    for bytes in [DEJAVU_MONO, DEJAVU_SANS, INTER_VARIABLE, NOTO_ARABIC] {
        let font = Font::from_bytes(bytes).expect("fixture parses");
        assert!(!font.has_meta(), "expected no meta on this fixture");
        assert!(font.meta_table().is_none());
        assert!(font.meta_record(b"dlng").is_none());
        assert!(font.meta_design_languages().is_none());
        assert!(font.meta_supported_languages().is_none());
    }
}

/// Build the smallest sfnt the directory parser accepts, with the
/// table list passed in as `(tag, payload)` pairs. The checksum field
/// is left zero (the parser ignores it). Offsets are packed
/// sequentially after the directory, 4-byte aligned between tables.
fn build_minimal_sfnt(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let n = tables.len() as u16;
    let mut out = Vec::new();
    out.extend_from_slice(&0x00010000u32.to_be_bytes()); // version (TrueType)
    out.extend_from_slice(&n.to_be_bytes()); // numTables
    out.extend_from_slice(&0u16.to_be_bytes()); // searchRange
    out.extend_from_slice(&0u16.to_be_bytes()); // entrySelector
    out.extend_from_slice(&0u16.to_be_bytes()); // rangeShift

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
        out.extend_from_slice(&0u32.to_be_bytes()); // checksum
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
    h[0..4].copy_from_slice(&0x00005000u32.to_be_bytes()); // version 0.5
    h[4..6].copy_from_slice(&num_glyphs.to_be_bytes());
    h
}

fn make_cmap_empty() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_be_bytes()); // version
    out.extend_from_slice(&1u16.to_be_bytes()); // numTables
    out.extend_from_slice(&3u16.to_be_bytes()); // platformID
    out.extend_from_slice(&1u16.to_be_bytes()); // encodingID
    out.extend_from_slice(&12u32.to_be_bytes()); // offset to subtable

    let mut sub = Vec::new();
    sub.extend_from_slice(&4u16.to_be_bytes()); // format
    sub.extend_from_slice(&32u16.to_be_bytes()); // length (placeholder)
    sub.extend_from_slice(&0u16.to_be_bytes());
    sub.extend_from_slice(&2u16.to_be_bytes()); // segCountX2
    sub.extend_from_slice(&2u16.to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes());
    sub.extend_from_slice(&0xFFFFu16.to_be_bytes()); // endCode[0]
    sub.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
    sub.extend_from_slice(&0xFFFFu16.to_be_bytes()); // startCode[0]
    sub.extend_from_slice(&1u16.to_be_bytes()); // idDelta[0]
    sub.extend_from_slice(&0u16.to_be_bytes()); // idRangeOffset[0]
    let total = sub.len() as u16;
    sub[2..4].copy_from_slice(&total.to_be_bytes());
    out.extend_from_slice(&sub);
    out
}

fn make_name_empty() -> Vec<u8> {
    let mut out = vec![0u8; 6];
    out[4..6].copy_from_slice(&6u16.to_be_bytes()); // storageOffset
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

/// Build a `meta` table per §5.7.6.1 with the supplied
/// `(tag, payload)` records packed sequentially after the DataMap
/// array. Header `version` is 1, `flags` and `reserved` are 0.
fn make_meta(records: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&META_VERSION_1.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes()); // flags
    out.extend_from_slice(&0u32.to_be_bytes()); // reserved
    out.extend_from_slice(&(records.len() as u32).to_be_bytes()); // dataMapsCount
    let payload_base = META_HEADER_LEN + records.len() * META_DATA_MAP_LEN;
    let mut cur = payload_base;
    for (tag, payload) in records {
        out.extend_from_slice(tag.as_slice());
        out.extend_from_slice(&(cur as u32).to_be_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        cur += payload.len();
    }
    for (_tag, payload) in records {
        out.extend_from_slice(payload);
    }
    out
}

fn build_font_with_meta(meta_payload: Vec<u8>) -> Vec<u8> {
    let num_glyphs = 4u16;
    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"head", make_head(1000, 0)),
        (b"hhea", make_hhea(1)),
        (b"maxp", make_maxp(num_glyphs)),
        (b"cmap", make_cmap_empty()),
        (b"name", make_name_empty()),
        (b"hmtx", make_hmtx(1, num_glyphs)),
        (b"loca", make_loca_short(num_glyphs)),
        (b"glyf", make_glyf_empty()),
        (b"meta", meta_payload),
    ];
    build_minimal_sfnt(&tables)
}

#[test]
fn synth_font_round_trips_dlng_and_slng() {
    // §5.7.6.2 worked example pattern: dlng = single design
    // language, slng = comma-separated supported list.
    let payload = make_meta(&[
        (b"dlng", b"Latn"),
        (b"slng", b"Latn, Cyrl, Grek"),
        (b"XYZ1", b"vendor-blob"),
    ]);
    let font_bytes = build_font_with_meta(payload);
    let font = Font::from_bytes(&font_bytes).expect("synth font parses");

    assert!(font.has_meta(), "synthesised font must surface meta");
    let table = font.meta_table().expect("meta_table");
    assert_eq!(table.version(), META_VERSION_1);
    assert_eq!(table.flags(), 0);
    assert_eq!(table.records().len(), 3);

    // Convenience accessors.
    assert_eq!(font.meta_design_languages(), Some("Latn"));
    assert_eq!(font.meta_supported_languages(), Some("Latn, Cyrl, Grek"));

    // Direct record lookup hits both registered tags + the vendor
    // tag.
    let dlng = font.meta_record(&META_TAG_DLNG).expect("dlng record");
    assert_eq!(&dlng.tag, &META_TAG_DLNG);
    assert_eq!(dlng.payload, b"Latn");
    let slng = font.meta_record(&META_TAG_SLNG).expect("slng record");
    assert_eq!(slng.payload_as_str(), Some("Latn, Cyrl, Grek"));
    let vendor = font.meta_record(b"XYZ1").expect("vendor record");
    assert_eq!(vendor.payload, b"vendor-blob");
}

#[test]
fn script_lang_tag_splitter_on_synth_font_slng_payload() {
    // §5.7.6.3 ScriptLangTag splitter chains naturally off the
    // `'slng'` accessor: each tag is a hyphen-separated ASCII
    // token.
    let payload = make_meta(&[(b"slng", b"Latn, sr-Cyrl, en-Dsrt, Hant-HK")]);
    let font_bytes = build_font_with_meta(payload);
    let font = Font::from_bytes(&font_bytes).expect("synth font parses");

    let slng = font
        .meta_supported_languages()
        .expect("supported languages decoded");
    let tags: Vec<_> = script_lang_tags(slng).map(|t| t.raw).collect();
    assert_eq!(
        tags,
        vec!["Latn", "sr-Cyrl", "en-Dsrt", "Hant-HK"],
        "splitter must preserve order and strip whitespace"
    );
}

#[test]
fn meta_table_with_no_records_round_trips_as_empty() {
    // §5.7.6.1 permits a `dataMapsCount == 0` header (the minimal
    // table). The accessors must report the table as present but
    // every tag-keyed query as absent.
    let payload = make_meta(&[]);
    let font_bytes = build_font_with_meta(payload);
    let font = Font::from_bytes(&font_bytes).expect("synth font parses");

    assert!(font.has_meta());
    let table = font.meta_table().unwrap();
    assert_eq!(table.records().len(), 0);
    assert!(font.meta_design_languages().is_none());
    assert!(font.meta_supported_languages().is_none());
    assert!(font.meta_record(&META_TAG_DLNG).is_none());
}

#[test]
fn malformed_meta_table_rejects_at_parse_time() {
    // §5.7.6.1: version must be 1. We synth a font with a
    // version-2 `meta` table and confirm the parse fails — i.e.
    // the malformed table is caught at `Font::from_bytes`, not
    // silently surfaced through the accessors.
    let mut payload = make_meta(&[(b"dlng", b"Latn")]);
    payload[0..4].copy_from_slice(&2u32.to_be_bytes());
    let font_bytes = build_font_with_meta(payload);
    assert!(
        Font::from_bytes(&font_bytes).is_err(),
        "version != 1 must surface as parse error"
    );
}
