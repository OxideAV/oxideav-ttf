//! Integration coverage for the `VORG` accessors on [`oxideav_ttf::Font`].
//!
//! Two paths:
//!
//! * **Absent path** — DejaVu Sans Mono / DejaVu Sans, both
//!   horizontal-only Latin / Cyrillic / Greek faces that do not ship
//!   the optional vertical-origin table (§5.4.4 is specific to
//!   CFF-flavoured CJK sfnts). Every `VORG` accessor must return
//!   `None`.
//!
//! * **TrueType ignore-on-decode path** — synthesised TrueType sfnt
//!   that carries a `VORG` table alongside `glyf`. §5.4.4 requires
//!   font clients to ignore `VORG` when the outlines are TrueType
//!   ("If present in TrueType OFF fonts it must be ignored by font
//!   clients, just as any other unrecognized table would be"). We
//!   confirm: `vorg_table()` returns the parsed bytes (so tooling can
//!   introspect them) but `vert_origin_y_from_vorg()` declines to
//!   honour them and returns `None`.
//!
//! The TrueType-flavoured sfnt is hand-stitched here because every
//! production TrueType fixture in `tests/fixtures/` correctly omits
//! `VORG`; we need a malformed-but-parseable file to confirm the
//! policy at the [`Font`] boundary.

use oxideav_ttf::Font;

const DEJAVU_MONO: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");
const DEJAVU_SANS: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");

#[test]
fn dejavu_mono_has_no_vorg() {
    let f = Font::from_bytes(DEJAVU_MONO).unwrap();
    assert!(!f.has_vorg());
    assert!(f.vorg_table().is_none());
    assert!(f.vorg_default_vert_origin_y().is_none());
    let gid = f.glyph_index('A').unwrap();
    assert!(f.vert_origin_y_from_vorg(gid).is_none());
}

#[test]
fn dejavu_sans_has_no_vorg() {
    let f = Font::from_bytes(DEJAVU_SANS).unwrap();
    assert!(!f.has_vorg());
    assert!(f.vorg_table().is_none());
}

/// Build the smallest sfnt the directory parser accepts, with the
/// table list passed in as `(tag, payload)` pairs. The checksum field
/// is left zero (the parser ignores it). Offsets are packed
/// sequentially after the directory.
fn build_minimal_sfnt(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let n = tables.len() as u16;
    let mut out = Vec::new();
    // sfnt header.
    out.extend_from_slice(&0x00010000u32.to_be_bytes()); // version (TrueType)
    out.extend_from_slice(&n.to_be_bytes()); // numTables
    out.extend_from_slice(&0u16.to_be_bytes()); // searchRange
    out.extend_from_slice(&0u16.to_be_bytes()); // entrySelector
    out.extend_from_slice(&0u16.to_be_bytes()); // rangeShift

    let dir_size = 16usize * n as usize;
    let mut payload_offset = 12usize + dir_size;
    // Two passes: first record offsets, then payloads.
    let mut records = Vec::with_capacity(n as usize);
    for (tag, payload) in tables {
        records.push((*tag, payload_offset as u32, payload.len() as u32));
        payload_offset += payload.len();
        // 4-byte alignment between tables (the spec recommends it; the
        // parser does not require it but real-world fonts always do).
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

/// Build a minimal `head` table (54 bytes; §5.2.4). Only fields the
/// `head` parser inspects need real values; the rest are zeroed.
fn make_head(units_per_em: u16, index_to_loc_format: i16) -> Vec<u8> {
    let mut h = vec![0u8; 54];
    h[0..4].copy_from_slice(&0x00010000u32.to_be_bytes()); // version
    h[4..8].copy_from_slice(&0x00010000u32.to_be_bytes()); // fontRevision
                                                           // checksumAdjustment (8..12) = 0
    h[12..16].copy_from_slice(&0x5F0F3CF5u32.to_be_bytes()); // magicNumber
                                                             // flags (16..18), unitsPerEm (18..20)
    h[18..20].copy_from_slice(&units_per_em.to_be_bytes());
    // created/modified left zero
    // xMin/yMin/xMax/yMax (36..44) left zero
    // macStyle/lowestRecPPEM/fontDirectionHint left zero
    h[50..52].copy_from_slice(&index_to_loc_format.to_be_bytes());
    // glyphDataFormat left zero
    h
}

/// Build a minimal `hhea` table (36 bytes; §5.7.3) with
/// `numberOfHMetrics = 1`.
fn make_hhea(num_h_metrics: u16) -> Vec<u8> {
    let mut h = vec![0u8; 36];
    h[0..4].copy_from_slice(&0x00010000u32.to_be_bytes()); // version
                                                           // metricDataFormat at 32..34 = 0; numberOfHMetrics at 34..36.
    h[34..36].copy_from_slice(&num_h_metrics.to_be_bytes());
    h
}

/// Build a minimal `maxp` 0.5 table (6 bytes; §5.2.7).
fn make_maxp(num_glyphs: u16) -> Vec<u8> {
    let mut h = vec![0u8; 6];
    h[0..4].copy_from_slice(&0x00005000u32.to_be_bytes()); // version 0.5
    h[4..6].copy_from_slice(&num_glyphs.to_be_bytes());
    h
}

/// Build a minimal cmap with one format-4 subtable mapping no
/// codepoints (so the parser accepts the table without us declaring
/// real glyph mappings). Format 4 layout: §5.2.1.4.4.
fn make_cmap_empty() -> Vec<u8> {
    let mut out = Vec::new();
    // cmap header: version 0, numTables 1, one EncodingRecord (8 bytes)
    out.extend_from_slice(&0u16.to_be_bytes()); // version
    out.extend_from_slice(&1u16.to_be_bytes()); // numTables
    out.extend_from_slice(&3u16.to_be_bytes()); // platformID (Windows)
    out.extend_from_slice(&1u16.to_be_bytes()); // encodingID (Unicode BMP)
    out.extend_from_slice(&12u32.to_be_bytes()); // offset to subtable

    // Format 4 with one terminator segment.
    let mut sub = Vec::new();
    sub.extend_from_slice(&4u16.to_be_bytes()); // format
    sub.extend_from_slice(&32u16.to_be_bytes()); // length (placeholder)
    sub.extend_from_slice(&0u16.to_be_bytes()); // language
    sub.extend_from_slice(&2u16.to_be_bytes()); // segCountX2 (= 1 segment)
    sub.extend_from_slice(&2u16.to_be_bytes()); // searchRange
    sub.extend_from_slice(&0u16.to_be_bytes()); // entrySelector
    sub.extend_from_slice(&0u16.to_be_bytes()); // rangeShift
                                                // endCode[1] = 0xFFFF (terminator)
    sub.extend_from_slice(&0xFFFFu16.to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
                                                // startCode[1]
    sub.extend_from_slice(&0xFFFFu16.to_be_bytes());
    // idDelta[1] = 1 (so 0xFFFF maps to glyph 0)
    sub.extend_from_slice(&1u16.to_be_bytes());
    // idRangeOffset[1] = 0
    sub.extend_from_slice(&0u16.to_be_bytes());
    // Patch the length field in place.
    let total = sub.len() as u16;
    sub[2..4].copy_from_slice(&total.to_be_bytes());
    out.extend_from_slice(&sub);
    out
}

/// Build a minimal `name` table with zero name records.
fn make_name_empty() -> Vec<u8> {
    let mut out = vec![0u8; 6];
    out[0..2].copy_from_slice(&0u16.to_be_bytes()); // version 0
    out[2..4].copy_from_slice(&0u16.to_be_bytes()); // count
    out[4..6].copy_from_slice(&6u16.to_be_bytes()); // storageOffset
    out
}

/// Build a minimal `hmtx` table for `num_h_metrics` long pairs and
/// `num_glyphs - num_h_metrics` tail entries.
fn make_hmtx(num_h_metrics: u16, num_glyphs: u16) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..num_h_metrics {
        out.extend_from_slice(&0u16.to_be_bytes()); // advanceWidth
        out.extend_from_slice(&0i16.to_be_bytes()); // lsb
    }
    for _ in num_h_metrics..num_glyphs {
        out.extend_from_slice(&0i16.to_be_bytes());
    }
    out
}

/// Build a minimal `loca` short-format table for `num_glyphs`, with
/// every entry pointing at offset 0 (= empty glyph).
fn make_loca_short(num_glyphs: u16) -> Vec<u8> {
    let mut out = Vec::new();
    // loca has numGlyphs+1 entries.
    for _ in 0..=num_glyphs {
        out.extend_from_slice(&0u16.to_be_bytes());
    }
    out
}

/// Build a minimal empty `glyf` table (zero bytes is valid: every
/// loca entry points at offset 0 with length 0 ⇒ empty glyphs).
fn make_glyf_empty() -> Vec<u8> {
    // The parser requires loca/glyf to both be present; an empty body
    // is fine since every `loca` range computes to length-zero.
    // Provide a single zero byte to avoid edge cases around zero-length
    // table records in the directory parser.
    vec![0u8; 2]
}

/// Build a minimal `VORG` table for the spec's worked example
/// (§5.4.4 — defaultVertOriginY = 880, overrides at gids 10/12/13).
fn make_vorg_spec_example() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&1u16.to_be_bytes()); // majorVersion
    out.extend_from_slice(&0u16.to_be_bytes()); // minorVersion
    out.extend_from_slice(&880i16.to_be_bytes()); // defaultVertOriginY
    out.extend_from_slice(&3u16.to_be_bytes()); // numVertOriginYMetrics
    for (gid, y) in [(10u16, 889i16), (12, 861), (13, 849)] {
        out.extend_from_slice(&gid.to_be_bytes());
        out.extend_from_slice(&y.to_be_bytes());
    }
    out
}

#[test]
fn truetype_sfnt_with_vorg_parses_but_ignores_lookup() {
    // §5.4.4: "If present in TrueType OFF fonts it must be ignored by
    // font clients, just as any other unrecognized table would be."
    // We confirm both halves: the table is parsed (so tooling can
    // introspect it) and the high-level lookup honours the ignore
    // policy.
    let num_glyphs = 16u16;
    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"head", make_head(1000, 0)),
        (b"hhea", make_hhea(1)),
        (b"maxp", make_maxp(num_glyphs)),
        (b"cmap", make_cmap_empty()),
        (b"name", make_name_empty()),
        (b"hmtx", make_hmtx(1, num_glyphs)),
        (b"loca", make_loca_short(num_glyphs)),
        (b"glyf", make_glyf_empty()),
        (b"VORG", make_vorg_spec_example()),
    ];
    let font_bytes = build_minimal_sfnt(&tables);
    let font = Font::from_bytes(&font_bytes).expect("synth font parses");

    // Bytes-level introspection works.
    assert!(font.has_vorg());
    let vorg = font.vorg_table().expect("vorg present");
    assert_eq!(vorg.default_vert_origin_y, 880);
    assert_eq!(vorg.metrics_len(), 3);
    assert_eq!(font.vorg_default_vert_origin_y(), Some(880));

    // High-level vertical-origin accessor declines to honour the
    // table because the font is TrueType-flavoured (a `glyf` table is
    // present).
    for gid in [0u16, 5, 10, 12, 13, 14, 15] {
        assert!(
            font.vert_origin_y_from_vorg(gid).is_none(),
            "VORG must be ignored on TrueType per §5.4.4 (gid={gid})"
        );
    }
}

#[test]
fn cff_flavoured_sfnt_with_vorg_honours_lookup() {
    // The contrapositive of the previous test: when `glyf` is absent
    // (i.e. the font is CFF-flavoured, per §5.4.4's "CFF OFF fonts"
    // restriction), `vert_origin_y_from_vorg` returns the per-glyph
    // origin from the parsed table — the override for known gids and
    // `defaultVertOriginY` for the rest.
    //
    // Note: this crate does not decode CFF outlines (see README
    // "Out of scope"), but the sfnt container — and therefore the
    // `VORG` table — is the same shape regardless of outline kind,
    // so the lookup-path test is meaningful even without CFF support.
    let num_glyphs = 16u16;
    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"head", make_head(1000, 0)),
        (b"hhea", make_hhea(1)),
        (b"maxp", make_maxp(num_glyphs)),
        (b"cmap", make_cmap_empty()),
        (b"name", make_name_empty()),
        (b"hmtx", make_hmtx(1, num_glyphs)),
        (b"VORG", make_vorg_spec_example()),
    ];
    let font_bytes = build_minimal_sfnt(&tables);
    let font = Font::from_bytes(&font_bytes).expect("synth font parses");

    // The CFF-shaped sfnt has no glyf/loca; the lookup should now
    // surface the parsed VORG values.
    assert!(font.has_vorg());
    assert_eq!(font.vert_origin_y_from_vorg(0), Some(880));
    assert_eq!(font.vert_origin_y_from_vorg(5), Some(880));
    assert_eq!(font.vert_origin_y_from_vorg(10), Some(889));
    assert_eq!(font.vert_origin_y_from_vorg(11), Some(880));
    assert_eq!(font.vert_origin_y_from_vorg(12), Some(861));
    assert_eq!(font.vert_origin_y_from_vorg(13), Some(849));
    assert_eq!(font.vert_origin_y_from_vorg(15), Some(880));
}
