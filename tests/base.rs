//! Integration coverage for the `BASE` accessors on
//! [`oxideav_ttf::Font`].
//!
//! Three paths:
//!
//! * **Absent path** — DejaVu Sans Mono / DejaVu Sans / Inter, all of
//!   which do not ship `BASE`. Every accessor must return `None`.
//!
//! * **HorizAxis-only synthetic** — a TrueType-flavoured sfnt that
//!   carries a `BASE` table with one HorizAxis whose BaseTagList is
//!   the alphabetical `ideo` / `romn` pair and whose BaseScriptList
//!   has two entries (`hani` / `latn`) — exactly the shape §6.3.1.3
//!   prescribes for a Latin-on-CJK dominant-run setup. We verify the
//!   table parses, the per-script accessors surface the correct
//!   coordinate for the right baseline, and unknown script / baseline
//!   tags decline cleanly.
//!
//! * **HorizAxis + VertAxis synthetic** — adds a VertAxis with the
//!   `ideo` / `romn` baselines flipped to vertical X coordinates, and
//!   confirms the `base_vert_x_for_script_baseline` accessor walks
//!   the VertAxis tree independently of the horizontal one. §6.3.1.3
//!   notes that the HorizAxis carries Y values and the VertAxis carries
//!   X values; the same tag can mean different baselines in each
//!   direction.

use oxideav_ttf::Font;

const DEJAVU_MONO: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");
const DEJAVU_SANS: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");
const INTER: &[u8] = include_bytes!("fixtures/InterVariable.ttf");

#[test]
fn dejavu_mono_has_no_base() {
    let f = Font::from_bytes(DEJAVU_MONO).unwrap();
    assert!(!f.has_base());
    assert!(f.base_table().is_none());
    assert!(f
        .base_horiz_y_for_script_baseline(*b"latn", *b"romn")
        .is_none());
    assert!(f
        .base_vert_x_for_script_baseline(*b"latn", *b"romn")
        .is_none());
}

#[test]
fn dejavu_sans_has_no_base() {
    let f = Font::from_bytes(DEJAVU_SANS).unwrap();
    assert!(!f.has_base());
}

#[test]
fn inter_has_no_base() {
    let f = Font::from_bytes(INTER).unwrap();
    assert!(!f.has_base());
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

// ---- BASE-table builders -------------------------------------------------

/// Build a §6.3.1.3 HorizAxis-only BASE table: BaseTagList lists
/// `ideo` / `romn` (alphabetical); BaseScriptList lists `hani` / `latn`
/// (alphabetical). The Latin BaseValues gives roman = 0,
/// ideographic = -120 with the roman baseline as default; the Han
/// BaseValues gives ideographic = 0, roman = +120 with the
/// ideographic baseline as default. Neither script ships extents or
/// language-specific overrides.
fn make_base_horiz_two_scripts() -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    // BASE header (v1.0).
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    let horiz_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes()); // vertAxis = NULL

    // HorizAxis.
    let horiz_start = buf.len();
    buf[horiz_off_pos..horiz_off_pos + 2].copy_from_slice(&(horiz_start as u16).to_be_bytes());
    let tag_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());
    let script_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());

    // BaseTagList: ideo, romn (alphabetical per §6.3.1.3).
    let tag_start = buf.len();
    buf[tag_off_pos..tag_off_pos + 2]
        .copy_from_slice(&((tag_start - horiz_start) as u16).to_be_bytes());
    buf.extend_from_slice(&2u16.to_be_bytes());
    buf.extend_from_slice(b"ideo");
    buf.extend_from_slice(b"romn");

    // BaseScriptList: hani, latn (alphabetical).
    let script_start = buf.len();
    buf[script_off_pos..script_off_pos + 2]
        .copy_from_slice(&((script_start - horiz_start) as u16).to_be_bytes());
    buf.extend_from_slice(&2u16.to_be_bytes()); // baseScriptCount
    buf.extend_from_slice(b"hani");
    let hani_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(b"latn");
    let latn_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());

    // BaseScript hani.
    let hani_start = buf.len();
    buf[hani_off_pos..hani_off_pos + 2]
        .copy_from_slice(&((hani_start - script_start) as u16).to_be_bytes());
    let hani_bv_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes()); // defaultMinMax NULL
    buf.extend_from_slice(&0u16.to_be_bytes()); // baseLangSysCount = 0

    // hani BaseValues: defaultBaselineIndex = 0 (ideo), 2 coords.
    let hani_bv_start = buf.len();
    buf[hani_bv_off_pos..hani_bv_off_pos + 2]
        .copy_from_slice(&((hani_bv_start - hani_start) as u16).to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes()); // defaultBaselineIndex = ideo
    buf.extend_from_slice(&2u16.to_be_bytes()); // baseCoordCount
    let hani_bc0_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes()); // ideo offset
    let hani_bc1_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes()); // romn offset

    // hani ideo (Format 1, 0).
    let hani_bc0_start = buf.len();
    buf[hani_bc0_off_pos..hani_bc0_off_pos + 2]
        .copy_from_slice(&((hani_bc0_start - hani_bv_start) as u16).to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    // hani romn (Format 1, 120).
    let hani_bc1_start = buf.len();
    buf[hani_bc1_off_pos..hani_bc1_off_pos + 2]
        .copy_from_slice(&((hani_bc1_start - hani_bv_start) as u16).to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(&120i16.to_be_bytes());

    // BaseScript latn.
    let latn_start = buf.len();
    buf[latn_off_pos..latn_off_pos + 2]
        .copy_from_slice(&((latn_start - script_start) as u16).to_be_bytes());
    let latn_bv_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes()); // defaultMinMax NULL
    buf.extend_from_slice(&0u16.to_be_bytes()); // baseLangSysCount = 0

    // latn BaseValues: defaultBaselineIndex = 1 (romn), 2 coords.
    let latn_bv_start = buf.len();
    buf[latn_bv_off_pos..latn_bv_off_pos + 2]
        .copy_from_slice(&((latn_bv_start - latn_start) as u16).to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes()); // defaultBaselineIndex = romn
    buf.extend_from_slice(&2u16.to_be_bytes());
    let latn_bc0_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());
    let latn_bc1_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());

    // latn ideo (Format 1, -120).
    let latn_bc0_start = buf.len();
    buf[latn_bc0_off_pos..latn_bc0_off_pos + 2]
        .copy_from_slice(&((latn_bc0_start - latn_bv_start) as u16).to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(&(-120i16).to_be_bytes());
    // latn romn (Format 1, 0).
    let latn_bc1_start = buf.len();
    buf[latn_bc1_off_pos..latn_bc1_off_pos + 2]
        .copy_from_slice(&((latn_bc1_start - latn_bv_start) as u16).to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());

    buf
}

/// Build a HorizAxis + VertAxis BASE table. The HorizAxis is the
/// `latn`-only Latin tree (BaseTagList = `[romn]`; latn baseline = 0).
/// The VertAxis is the `hani`-only Han tree (BaseTagList = `[ideo]`;
/// hani baseline = 50). The two axes carry independent BaseScriptList
/// arrays per §6.3.1.3 ("If a font has values for only one text
/// direction, the Axis table offset value for the other direction will
/// be set to NULL" — here we exercise the inverse: both axes
/// present with disjoint script coverage).
fn make_base_horiz_and_vert() -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    let horiz_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());
    let vert_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());

    // HorizAxis with `latn` -> romn = 0.
    let horiz_start = buf.len();
    buf[horiz_off_pos..horiz_off_pos + 2].copy_from_slice(&(horiz_start as u16).to_be_bytes());
    let tag_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());
    let script_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());

    let tag_start = buf.len();
    buf[tag_off_pos..tag_off_pos + 2]
        .copy_from_slice(&((tag_start - horiz_start) as u16).to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(b"romn");

    let script_start = buf.len();
    buf[script_off_pos..script_off_pos + 2]
        .copy_from_slice(&((script_start - horiz_start) as u16).to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(b"latn");
    let latn_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());

    let latn_start = buf.len();
    buf[latn_off_pos..latn_off_pos + 2]
        .copy_from_slice(&((latn_start - script_start) as u16).to_be_bytes());
    let latn_bv_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());

    let latn_bv_start = buf.len();
    buf[latn_bv_off_pos..latn_bv_off_pos + 2]
        .copy_from_slice(&((latn_bv_start - latn_start) as u16).to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    let latn_bc0_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());

    let latn_bc0_start = buf.len();
    buf[latn_bc0_off_pos..latn_bc0_off_pos + 2]
        .copy_from_slice(&((latn_bc0_start - latn_bv_start) as u16).to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());

    // VertAxis with `hani` -> ideo = 50.
    let vert_start = buf.len();
    buf[vert_off_pos..vert_off_pos + 2].copy_from_slice(&(vert_start as u16).to_be_bytes());
    let vtag_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());
    let vscript_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());

    let vtag_start = buf.len();
    buf[vtag_off_pos..vtag_off_pos + 2]
        .copy_from_slice(&((vtag_start - vert_start) as u16).to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(b"ideo");

    let vscript_start = buf.len();
    buf[vscript_off_pos..vscript_off_pos + 2]
        .copy_from_slice(&((vscript_start - vert_start) as u16).to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(b"hani");
    let hani_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());

    let hani_start = buf.len();
    buf[hani_off_pos..hani_off_pos + 2]
        .copy_from_slice(&((hani_start - vscript_start) as u16).to_be_bytes());
    let hani_bv_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());

    let hani_bv_start = buf.len();
    buf[hani_bv_off_pos..hani_bv_off_pos + 2]
        .copy_from_slice(&((hani_bv_start - hani_start) as u16).to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    let hani_bc0_off_pos = buf.len();
    buf.extend_from_slice(&0u16.to_be_bytes());

    let hani_bc0_start = buf.len();
    buf[hani_bc0_off_pos..hani_bc0_off_pos + 2]
        .copy_from_slice(&((hani_bc0_start - hani_bv_start) as u16).to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(&50i16.to_be_bytes());

    buf
}

#[test]
fn synth_horiz_only_table_walks_both_scripts() {
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
        (b"BASE", make_base_horiz_two_scripts()),
    ];
    let font_bytes = build_minimal_sfnt(&tables);
    let font = Font::from_bytes(&font_bytes).expect("parse synth");
    assert!(font.has_base());
    let base = font.base_table().unwrap();
    assert_eq!(base.major_version, 1);
    assert_eq!(base.minor_version, 0);
    let h = base.horiz_axis.as_ref().unwrap();
    let tags = h.baseline_tags.as_ref().unwrap();
    assert_eq!(tags, &vec![*b"ideo", *b"romn"]);
    assert_eq!(h.base_scripts.len(), 2);

    // Latin: roman = 0 (the default), ideographic = -120.
    assert_eq!(
        font.base_horiz_y_for_script_baseline(*b"latn", *b"romn"),
        Some(0)
    );
    assert_eq!(
        font.base_horiz_y_for_script_baseline(*b"latn", *b"ideo"),
        Some(-120)
    );
    // Han: ideographic = 0 (the default), roman = 120.
    assert_eq!(
        font.base_horiz_y_for_script_baseline(*b"hani", *b"ideo"),
        Some(0)
    );
    assert_eq!(
        font.base_horiz_y_for_script_baseline(*b"hani", *b"romn"),
        Some(120)
    );

    // Default-baseline-index per BaseValues.
    let latn = h.base_script_for_tag(*b"latn").unwrap();
    assert_eq!(latn.base_values.as_ref().unwrap().default_baseline_index, 1);
    let hani = h.base_script_for_tag(*b"hani").unwrap();
    assert_eq!(hani.base_values.as_ref().unwrap().default_baseline_index, 0);

    // Unknown scripts / baselines decline cleanly.
    assert!(font
        .base_horiz_y_for_script_baseline(*b"arab", *b"romn")
        .is_none());
    assert!(font
        .base_horiz_y_for_script_baseline(*b"latn", *b"hang")
        .is_none());
    // VertAxis-side lookups on a HorizAxis-only font return None.
    assert!(font
        .base_vert_x_for_script_baseline(*b"latn", *b"romn")
        .is_none());
}

#[test]
fn synth_table_with_horiz_and_vert_axes() {
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
        (b"BASE", make_base_horiz_and_vert()),
    ];
    let font_bytes = build_minimal_sfnt(&tables);
    let font = Font::from_bytes(&font_bytes).expect("parse synth");
    assert!(font.has_base());
    let base = font.base_table().unwrap();
    assert!(base.horiz_axis.is_some());
    assert!(base.vert_axis.is_some());

    // HorizAxis: Latin -> romn = 0; no Han.
    assert_eq!(
        font.base_horiz_y_for_script_baseline(*b"latn", *b"romn"),
        Some(0)
    );
    assert!(font
        .base_horiz_y_for_script_baseline(*b"hani", *b"ideo")
        .is_none());
    // VertAxis: Han -> ideo = 50; no Latin.
    assert_eq!(
        font.base_vert_x_for_script_baseline(*b"hani", *b"ideo"),
        Some(50)
    );
    assert!(font
        .base_vert_x_for_script_baseline(*b"latn", *b"romn")
        .is_none());
}
