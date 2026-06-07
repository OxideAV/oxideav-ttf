//! Integration coverage for the `LTSH` accessors on
//! [`oxideav_ttf::Font`].
//!
//! Two paths:
//!
//! * **Absent path** — DejaVu Sans Mono / DejaVu Sans / Inter, all of
//!   which do not ship `LTSH`. Every accessor must return `None` /
//!   `false`.
//!
//! * **Synthetic path** — a TrueType-flavoured sfnt that ships an
//!   `LTSH` table with a mixed yPels array exercising the §5.7.4
//!   "always linear" sentinel (`1`), the §5.7.4 criterion (a) `ppem ≥
//!   50` threshold, and an intermediate convergence ppem. We verify
//!   the table parses, the per-glyph accessors round-trip the
//!   threshold, `linearly_scales_at_ppem` honours the spec inequality,
//!   and `parse_with_glyph_count` rejects an LTSH whose `numGlyphs`
//!   disagrees with `maxp`.

use oxideav_ttf::Font;

const DEJAVU_MONO: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");
const DEJAVU_SANS: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");
const INTER: &[u8] = include_bytes!("fixtures/InterVariable.ttf");

#[test]
fn dejavu_mono_has_no_ltsh() {
    let f = Font::from_bytes(DEJAVU_MONO).unwrap();
    assert!(!f.has_ltsh());
    assert!(f.ltsh_table().is_none());
    // Out-of-range / absent => None / false at every ppem.
    assert_eq!(f.ltsh_threshold(0), None);
    assert!(!f.ltsh_linearly_scales_at_ppem(0, 12));
    assert!(!f.ltsh_linearly_scales_at_ppem(0, 200));
}

#[test]
fn dejavu_sans_has_no_ltsh() {
    let f = Font::from_bytes(DEJAVU_SANS).unwrap();
    assert!(!f.has_ltsh());
}

#[test]
fn inter_has_no_ltsh() {
    let f = Font::from_bytes(INTER).unwrap();
    assert!(!f.has_ltsh());
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

fn make_ltsh(version: u16, y_pels: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(4 + y_pels.len());
    b.extend_from_slice(&version.to_be_bytes());
    b.extend_from_slice(&(y_pels.len() as u16).to_be_bytes());
    b.extend_from_slice(y_pels);
    b
}

/// Build a TrueType-flavoured sfnt with a published `LTSH` array of
/// length `num_glyphs`. The required tables are the §5.2.1 set
/// (`head`, `hhea`, `maxp`, `cmap`, `name`, `hmtx`, plus the TrueType-
/// outline `loca` / `glyf` pair).
fn build_synth_with_ltsh(num_glyphs: u16, y_pels: &[u8]) -> Vec<u8> {
    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"head", make_head(1000, 0)),
        (b"hhea", make_hhea(1)),
        (b"maxp", make_maxp(num_glyphs)),
        (b"cmap", make_cmap_empty()),
        (b"name", make_name_empty()),
        (b"hmtx", make_hmtx(1, num_glyphs)),
        (b"loca", make_loca_short(num_glyphs)),
        (b"glyf", make_glyf_empty()),
        (b"LTSH", make_ltsh(0, y_pels)),
    ];
    build_minimal_sfnt(&tables)
}

#[test]
fn synth_ltsh_round_trips_through_font_accessors() {
    // 6-glyph synthetic font with a mixed yPels array:
    //   gid 0 (.notdef): yPels = 1 (always linear sentinel)
    //   gid 1:           yPels = 1
    //   gid 2:           yPels = 8  (converges at 8 ppem)
    //   gid 3:           yPels = 24 (converges at 24 ppem)
    //   gid 4:           yPels = 50 (§5.7.4 criterion (a) baseline)
    //   gid 5:           yPels = 1
    let num_glyphs = 6u16;
    let y_pels = [1u8, 1, 8, 24, 50, 1];
    let font_bytes = build_synth_with_ltsh(num_glyphs, &y_pels);
    let font = Font::from_bytes(&font_bytes).expect("parse synth");

    assert!(font.has_ltsh());
    let t = font.ltsh_table().expect("LTSH table");
    assert_eq!(t.num_glyphs(), num_glyphs);
    assert_eq!(t.y_pels(), &y_pels[..]);
    assert!(!t.all_always_linear()); // mixed array

    // Per-glyph threshold accessor: in-range yields the recorded ppem,
    // out-of-range yields `None`.
    assert_eq!(font.ltsh_threshold(0), Some(1));
    assert_eq!(font.ltsh_threshold(2), Some(8));
    assert_eq!(font.ltsh_threshold(3), Some(24));
    assert_eq!(font.ltsh_threshold(4), Some(50));
    assert_eq!(font.ltsh_threshold(num_glyphs), None);

    // linearly_scales_at_ppem honours the §5.7.4 `ppem >= yPels[gid]`
    // inequality: at-or-above => true, below => false.
    assert!(font.ltsh_linearly_scales_at_ppem(0, 1));
    assert!(font.ltsh_linearly_scales_at_ppem(0, 200));
    assert!(!font.ltsh_linearly_scales_at_ppem(2, 7));
    assert!(font.ltsh_linearly_scales_at_ppem(2, 8));
    assert!(!font.ltsh_linearly_scales_at_ppem(3, 23));
    assert!(font.ltsh_linearly_scales_at_ppem(3, 24));
    assert!(!font.ltsh_linearly_scales_at_ppem(4, 49));
    assert!(font.ltsh_linearly_scales_at_ppem(4, 50));
    // Out-of-range glyph stays false at every ppem.
    assert!(!font.ltsh_linearly_scales_at_ppem(num_glyphs, 100));
}

#[test]
fn synth_all_ones_table_short_circuits_to_always_linear() {
    // Every glyph carries the §5.7.4 sentinel `1` — the table publishes
    // "always scales linearly" for every glyph at every ppem. The
    // `all_always_linear` accessor lets a consumer short-circuit
    // per-glyph lookup.
    let num_glyphs = 4u16;
    let font_bytes = build_synth_with_ltsh(num_glyphs, &[1, 1, 1, 1]);
    let font = Font::from_bytes(&font_bytes).expect("parse synth");
    assert!(font.has_ltsh());
    let t = font.ltsh_table().unwrap();
    assert!(t.all_always_linear());
    for gid in 0..num_glyphs {
        assert!(t.is_always_linear(gid));
        // ppem ≥ 1 satisfies every glyph; ppem 0 fails the inequality.
        assert!(font.ltsh_linearly_scales_at_ppem(gid, 1));
        assert!(!font.ltsh_linearly_scales_at_ppem(gid, 0));
    }
}

#[test]
fn synth_ltsh_glyph_count_mismatch_rejected_at_font_parse() {
    // numGlyphs in maxp = 3 but LTSH carries a 4-entry yPels array. The
    // Font::from_bytes path runs parse_with_glyph_count and must reject
    // the mismatch rather than silently truncating per-glyph lookups.
    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"head", make_head(1000, 0)),
        (b"hhea", make_hhea(1)),
        (b"maxp", make_maxp(3)),
        (b"cmap", make_cmap_empty()),
        (b"name", make_name_empty()),
        (b"hmtx", make_hmtx(1, 3)),
        (b"loca", make_loca_short(3)),
        (b"glyf", make_glyf_empty()),
        (b"LTSH", make_ltsh(0, &[1, 1, 1, 1])),
    ];
    let font_bytes = build_minimal_sfnt(&tables);
    let err = Font::from_bytes(&font_bytes).expect_err("LTSH/maxp mismatch");
    assert!(matches!(err, oxideav_ttf::Error::BadStructure(_)));
}
