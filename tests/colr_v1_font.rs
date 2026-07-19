//! Font-level COLR v1 accessors, driven through a real variable font:
//! a synthetic paint graph (built byte-for-byte from the staged COLR
//! reference) is grafted onto the InterVariable fixture as its `COLR`
//! table, so `Font::color_paint` resolves variation deltas through the
//! fixture's genuine `fvar` + `avar` normalisation pipeline.

use oxideav_ttf::{ClipBox, Font, Paint};

/// Rebuild an sfnt with one extra table appended: the directory grows
/// by one 16-byte record (every existing table offset shifts by 16)
/// and the new table body lands at the end of the file.
fn graft_table(font: &[u8], tag: [u8; 4], body: &[u8]) -> Vec<u8> {
    let num_tables = u16::from_be_bytes([font[4], font[5]]);
    let dir_end = 12 + num_tables as usize * 16;
    let mut out = Vec::with_capacity(font.len() + 16 + body.len());
    // Header with numTables + 1 (the binary-search helper fields are
    // not consulted by the parser; keep them as-is).
    out.extend_from_slice(&font[0..4]);
    out.extend_from_slice(&(num_tables + 1).to_be_bytes());
    out.extend_from_slice(&font[6..12]);
    // Existing directory records, offsets shifted by the 16 bytes the
    // new record inserts.
    for i in 0..num_tables as usize {
        let rec = 12 + i * 16;
        out.extend_from_slice(&font[rec..rec + 8]);
        let off =
            u32::from_be_bytes([font[rec + 8], font[rec + 9], font[rec + 10], font[rec + 11]]);
        out.extend_from_slice(&(off + 16).to_be_bytes());
        out.extend_from_slice(&font[rec + 12..rec + 16]);
    }
    // New record: body appended at (shifted) EOF.
    let body_off = (font.len() + 16) as u32;
    out.extend_from_slice(&tag);
    out.extend_from_slice(&0u32.to_be_bytes()); // checksum (unchecked)
    out.extend_from_slice(&body_off.to_be_bytes());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    // Original table bodies, unchanged.
    out.extend_from_slice(&font[dir_end..]);
    out.extend_from_slice(body);
    out
}

/// Build a COLR v1 table binding `gid` to a PaintVarSolid (alpha 0.5,
/// varIndexBase 0) plus a variable clip box, with a single-region IVS
/// peaking at +1 on fvar axis `axis_index` of `axis_count`.
fn build_colr(gid: u16, axis_index: usize, axis_count: usize) -> Vec<u8> {
    let mut b: Vec<u8> = Vec::new();
    let u16b = |b: &mut Vec<u8>, x: u16| b.extend_from_slice(&x.to_be_bytes());
    let i16b = |b: &mut Vec<u8>, x: i16| b.extend_from_slice(&x.to_be_bytes());
    let u32b = |b: &mut Vec<u8>, x: u32| b.extend_from_slice(&x.to_be_bytes());
    let u24b = |b: &mut Vec<u8>, x: u32| b.extend_from_slice(&x.to_be_bytes()[1..4]);

    // Header: version 1, empty v0 arrays, offsets patched below.
    u16b(&mut b, 1);
    u16b(&mut b, 0);
    u32b(&mut b, 0);
    u32b(&mut b, 0);
    u16b(&mut b, 0);
    let bgl_slot = b.len();
    u32b(&mut b, 0);
    u32b(&mut b, 0); // layerListOffset (unused)
    let cl_slot = b.len();
    u32b(&mut b, 0);
    u32b(&mut b, 0); // varIndexMapOffset (identity)
    let ivs_slot = b.len();
    u32b(&mut b, 0);

    // IVS: one region spanning all fvar axes, peak +1 only on
    // `axis_index`; one IVD with three int16 rows:
    //   inner 0: +8192 (F2DOT14 +0.5)  inner 1: +40  inner 2: −40
    let ivs = b.len() as u32;
    u16b(&mut b, 1); // format
    u32b(&mut b, 12); // regionListOffset
    u16b(&mut b, 1); // ivdCount
    let region_bytes = 4 + axis_count * 6;
    u32b(&mut b, (12 + region_bytes) as u32); // ivdOffsets[0]
    u16b(&mut b, axis_count as u16);
    u16b(&mut b, 1); // regionCount
    for a in 0..axis_count {
        if a == axis_index {
            i16b(&mut b, 0);
            i16b(&mut b, 16384);
            i16b(&mut b, 16384);
        } else {
            i16b(&mut b, 0);
            i16b(&mut b, 0);
            i16b(&mut b, 0);
        }
    }
    u16b(&mut b, 3); // itemCount
    u16b(&mut b, 1); // shortDeltaCount
    u16b(&mut b, 1); // regionIndexCount
    u16b(&mut b, 0);
    i16b(&mut b, 8192);
    i16b(&mut b, 40);
    i16b(&mut b, -40);
    let at = ivs.to_be_bytes();
    b[ivs_slot..ivs_slot + 4].copy_from_slice(&at);

    // ClipList: one variable box over `gid`, varIndexBase 1
    // (xMin +40, yMin −40, xMax / yMax past the rows → static).
    let cl = b.len() as u32;
    b.push(1); // format
    u32b(&mut b, 1);
    u16b(&mut b, gid);
    u16b(&mut b, gid);
    let box_slot = b.len();
    u24b(&mut b, 0);
    let box_off = b.len() as u32 - cl;
    b.push(2); // ClipBoxFormat 2
    i16b(&mut b, 0);
    i16b(&mut b, -100);
    i16b(&mut b, 500);
    i16b(&mut b, 700);
    u32b(&mut b, 1); // varIndexBase
    b[box_slot..box_slot + 3].copy_from_slice(&box_off.to_be_bytes()[1..4]);
    b[cl_slot..cl_slot + 4].copy_from_slice(&cl.to_be_bytes());

    // BaseGlyphList: gid -> PaintVarSolid.
    let bgl = b.len() as u32;
    u32b(&mut b, 1);
    u16b(&mut b, gid);
    let paint_slot = b.len();
    u32b(&mut b, 0);
    b[bgl_slot..bgl_slot + 4].copy_from_slice(&bgl.to_be_bytes());
    let paint = b.len() as u32;
    b.push(3); // PaintVarSolid
    u16b(&mut b, 7); // paletteIndex
    i16b(&mut b, 8192); // alpha 0.5
    u32b(&mut b, 0); // varIndexBase -> inner 0
    let rel = paint - bgl;
    b[paint_slot..paint_slot + 4].copy_from_slice(&rel.to_be_bytes());
    b
}

fn load_grafted() -> (Vec<u8>, u16, usize, usize) {
    let inter = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/InterVariable.ttf"
    ))
    .expect("fixture");
    let probe = Font::from_bytes(&inter).expect("parse fixture");
    let gid = probe.glyph_index('A').expect("gid for A");
    let axes = probe.variation_axes();
    let wght = axes
        .iter()
        .position(|a| &a.tag == b"wght")
        .expect("wght axis");
    let colr = build_colr(gid, wght, axes.len());
    (graft_table(&inter, *b"COLR", &colr), gid, wght, axes.len())
}

#[test]
fn font_resolves_paint_graph_at_variation_instances() {
    let (bytes, gid, _, _) = load_grafted();
    let mut font = Font::from_bytes(&bytes).expect("parse grafted font");
    assert!(font.has_colr_v1());
    assert!(!font.colr_var_index_map_unsupported());

    let root = font.color_paint_root(gid).expect("paint root");
    assert_eq!(font.color_paint_format(root), Some(3));
    assert!(font.color_paint_root(gid.wrapping_add(1)).is_none());

    // Default instance: raw alpha.
    let Some(Paint::Solid {
        palette_index,
        alpha,
    }) = font.color_paint(root)
    else {
        panic!("expected Solid");
    };
    assert_eq!(palette_index, 7);
    assert!((alpha - 0.5).abs() < 1e-4);

    // Max weight: wght normalises to +1 (the avar v1 map pins the +1
    // anchor per spec), so the +0.5 alpha delta applies in full.
    font.set_axis_value(b"wght", 900.0);
    let Some(Paint::Solid { alpha, .. }) = font.color_paint(root) else {
        panic!("expected Solid");
    };
    assert!((alpha - 1.0).abs() < 1e-4, "alpha {alpha}");

    // Below default: the region (0, +1, +1) contributes nothing.
    font.set_axis_value(b"wght", 100.0);
    let Some(Paint::Solid { alpha, .. }) = font.color_paint(root) else {
        panic!("expected Solid");
    };
    assert!((alpha - 0.5).abs() < 1e-4, "alpha {alpha}");
}

#[test]
fn font_resolves_variable_clip_box() {
    let (bytes, gid, _, _) = load_grafted();
    let mut font = Font::from_bytes(&bytes).expect("parse grafted font");
    assert_eq!(
        font.color_clip_box(gid),
        Some(ClipBox {
            x_min: 0,
            y_min: -100,
            x_max: 500,
            y_max: 700,
        })
    );
    assert_eq!(font.color_clip_box(gid.wrapping_add(1)), None);

    font.set_axis_value(b"wght", 900.0);
    assert_eq!(
        font.color_clip_box(gid),
        Some(ClipBox {
            x_min: 40,
            y_min: -140,
            x_max: 500,
            y_max: 700,
        })
    );
}

#[test]
fn fonts_without_colr_v1_report_absence() {
    let dejavu = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/DejaVuSansMono.ttf"
    ))
    .expect("fixture");
    let font = Font::from_bytes(&dejavu).expect("parse");
    assert!(!font.has_colr_v1());
    assert!(!font.colr_var_index_map_unsupported());
    assert!(font.color_paint_root(1).is_none());
    assert!(font.color_clip_box(1).is_none());

    // The grafted Inter still parses everything else it always did.
    let (bytes, _, _, _) = load_grafted();
    let font = Font::from_bytes(&bytes).expect("parse grafted font");
    assert!(font.is_variable());
    assert!(font.glyph_outline(font.glyph_index('A').unwrap()).is_ok());
}
