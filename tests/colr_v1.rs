//! COLR version-1 paint-graph tests against hand-built synthetic
//! tables (staged paint-graph reference: header, BaseGlyphList /
//! LayerList, Paint formats 1–32, ColorLine / VarColorLine, ClipList /
//! ClipBox, and the varIndexBase / delta-set variation scheme).
//!
//! No installed system font ships a COLR v1 table (the local corpus
//! was scanned), so the graphs here are synthesised byte-for-byte from
//! the staged format tables — the same methodology the MVAR / HVAR /
//! DeltaSetIndexMap suites use.

use oxideav_ttf::tables::colr::{
    Affine2x3, ClipBox, ColorStop, ColrTable, CompositeMode, Extend, Paint, PaintRef,
};

/// Little byte-builder with big-endian pushes and offset patching.
#[derive(Default)]
struct B {
    v: Vec<u8>,
}

impl B {
    fn len(&self) -> usize {
        self.v.len()
    }
    fn u8(&mut self, x: u8) {
        self.v.push(x);
    }
    fn u16(&mut self, x: u16) {
        self.v.extend_from_slice(&x.to_be_bytes());
    }
    fn i16(&mut self, x: i16) {
        self.v.extend_from_slice(&x.to_be_bytes());
    }
    fn u24(&mut self, x: u32) {
        self.v.extend_from_slice(&x.to_be_bytes()[1..4]);
    }
    fn u32(&mut self, x: u32) {
        self.v.extend_from_slice(&x.to_be_bytes());
    }
    fn i32(&mut self, x: i32) {
        self.v.extend_from_slice(&x.to_be_bytes());
    }
    /// Reserve a 32-bit offset slot, returning its position.
    fn slot32(&mut self) -> usize {
        let p = self.v.len();
        self.u32(0);
        p
    }
    fn patch32(&mut self, slot: usize, x: u32) {
        self.v[slot..slot + 4].copy_from_slice(&x.to_be_bytes());
    }
    /// Reserve a 24-bit offset slot, returning its position.
    fn slot24(&mut self) -> usize {
        let p = self.v.len();
        self.u24(0);
        p
    }
    fn patch24(&mut self, slot: usize, x: u32) {
        self.v[slot..slot + 3].copy_from_slice(&x.to_be_bytes()[1..4]);
    }
}

/// Append a single-axis ItemVariationStore whose outer-0 subtable has
/// one int16 delta column against the region (start 0, peak +1,
/// end +1), one row per entry of `rows`. Returns the IVS offset.
fn push_ivs(b: &mut B, rows: &[i16]) -> u32 {
    let ivs = b.len() as u32;
    b.u16(1); // format
    b.u32(12); // variationRegionListOffset (IVS-relative)
    b.u16(1); // itemVariationDataCount
    b.u32(22); // ivdOffsets[0]
               // Region list: 1 axis, 1 region, (0, +1, +1) in F2DOT14.
    b.u16(1);
    b.u16(1);
    b.i16(0);
    b.i16(16384);
    b.i16(16384);
    // IVD: itemCount rows, 1 short (int16) column, region index 0.
    b.u16(rows.len() as u16);
    b.u16(1); // shortDeltaCount
    b.u16(1); // regionIndexCount
    b.u16(0); // regionIndexes[0]
    for &d in rows {
        b.i16(d);
    }
    ivs
}

/// Delta rows shared by the synthetic graphs, addressed as
/// `(outer 0, inner i)` through the implicit identity varIndexMap.
const ROWS: [i16; 8] = [8192, -8192, 100, -50, 16384, 200, 25, -25];

/// Build the main synthetic COLR v1 table (identity variation
/// mapping — no DeltaSetIndexMap). Returns the table bytes plus the
/// PaintRefs recorded while assembling (absolute offsets).
struct MainTable {
    bytes: Vec<u8>,
    p_solid: u32,
    p_var_solid: u32,
    p_glyph: u32,
    p_lin_grad: u32,
    p_sweep: u32,
    p_transform: u32,
    p_translate: u32,
    p_scale16: u32,
    p_scale23: u32,
    p_rotate26: u32,
    p_skew28: u32,
    p_composite: u32,
}

#[allow(clippy::too_many_lines)]
fn build_main_table() -> MainTable {
    let mut b = B::default();
    // ---- v1 header (34 bytes), v0 arrays empty ----
    b.u16(1); // version
    b.u16(0); // numBaseGlyphRecords
    b.u32(0); // baseGlyphRecordsOffset
    b.u32(0); // layerRecordsOffset
    b.u16(0); // numLayerRecords
    let bgl_slot = b.slot32(); // baseGlyphListOffset
    let ll_slot = b.slot32(); // layerListOffset
    let cl_slot = b.slot32(); // clipListOffset
    b.u32(0); // varIndexMapOffset (identity mapping)
    let ivs_slot = b.slot32(); // itemVariationStoreOffset

    // ---- ItemVariationStore ----
    let ivs = push_ivs(&mut b, &ROWS);
    b.patch32(ivs_slot, ivs);

    // ---- ClipList (format 1, two clips) ----
    let cl = b.len() as u32;
    b.u8(1); // format
    b.u32(2); // numClips
              // Clip 0: gids 5..=6 -> static ClipBoxFormat1.
    b.u16(5);
    b.u16(6);
    let box0_slot = b.slot24();
    // Clip 1: gids 7..=7 -> variable ClipBoxFormat2, varIndexBase 6.
    b.u16(7);
    b.u16(7);
    let box1_slot = b.slot24();
    let box0 = b.len() as u32 - cl;
    b.u8(1);
    b.i16(10);
    b.i16(-20);
    b.i16(300);
    b.i16(400);
    let box1 = b.len() as u32 - cl;
    b.u8(2);
    b.i16(10);
    b.i16(-20);
    b.i16(300);
    b.i16(400);
    b.u32(6); // varIndexBase -> rows 6 (+25) and 7 (−25); 8/9 out of range
    b.patch24(box0_slot, box0);
    b.patch24(box1_slot, box1);
    b.patch32(cl_slot, cl);

    // ---- BaseGlyphList (5 records; paints are appended later, so
    //      their record offsets are patched at the end) ----
    let bgl = b.len() as u32;
    b.u32(5); // numBaseGlyphPaintRecords
    let mut bgl_paint_slots = Vec::new();
    for gid in [5u16, 7, 9, 11, 13] {
        b.u16(gid);
        bgl_paint_slots.push(b.slot32());
    }
    b.patch32(bgl_slot, bgl);

    // ---- LayerList (2 layers, patched later) ----
    let ll = b.len() as u32;
    b.u32(2);
    let ll_slot0 = b.slot32();
    let ll_slot1 = b.slot32();
    b.patch32(ll_slot, ll);

    // ---- Paint tables (parents first: Offset24 children must sit at
    //      higher offsets) ----

    // PaintColrLayers { first 0, num 2 } — root of gid 5.
    let p_layers = b.len() as u32;
    b.u8(1);
    b.u8(2); // numLayers
    b.u32(0); // firstLayerIndex

    // PaintComposite { source: pVarSolid, Plus, backdrop: pSweep } — root of gid 9.
    let p_composite = b.len() as u32;
    b.u8(32);
    let comp_src_slot = b.slot24();
    b.u8(12); // COMPOSITE_PLUS
    let comp_back_slot = b.slot24();

    // PaintColrGlyph { gid 5 } — root of gid 11.
    let p_colr_glyph = b.len() as u32;
    b.u8(11);
    b.u16(5);

    // Unrecognised paint format — root of gid 13.
    let p_bad = b.len() as u32;
    b.u8(99);
    b.u32(0xDEAD_BEEF);

    // PaintVarTranslate { child pGlyph, dx 10 dy 20, varIndexBase 2 }.
    let p_translate = b.len() as u32;
    b.u8(15);
    let translate_child_slot = b.slot24();
    b.i16(10);
    b.i16(20);
    b.u32(2);

    // PaintVarTransform { child pSolid, VarAffine2x3 vb 4 }.
    let p_transform = b.len() as u32;
    b.u8(13);
    let transform_child_slot = b.slot24();
    let affine_slot = b.slot24();

    // PaintScale { child pSolid, sx 0.5, sy 1.0 }.
    let p_scale16 = b.len() as u32;
    b.u8(16);
    let scale16_child_slot = b.slot24();
    b.i16(8192);
    b.i16(16384);

    // PaintVarScaleUniformAroundCenter { child pSolid, scale 0.5,
    // centre (5, 7), varIndexBase 0 }.
    let p_scale23 = b.len() as u32;
    b.u8(23);
    let scale23_child_slot = b.slot24();
    b.i16(8192);
    b.i16(5);
    b.i16(7);
    b.u32(0);

    // PaintRotateAroundCenter { child pSolid, angle 0.5 (=90°), centre (11, 12) }.
    let p_rotate26 = b.len() as u32;
    b.u8(26);
    let rotate_child_slot = b.slot24();
    b.i16(8192);
    b.i16(11);
    b.i16(12);

    // PaintSkew { child pSolid, x −0.25 (=−45°), y 0 }.
    let p_skew28 = b.len() as u32;
    b.u8(28);
    let skew_child_slot = b.slot24();
    b.i16(-4096);
    b.i16(0);

    // PaintGlyph { child pSolid, glyph 40 } — LayerList entry 0.
    let p_glyph = b.len() as u32;
    b.u8(10);
    let glyph_child_slot = b.slot24();
    b.u16(40);

    // PaintVarLinearGradient — LayerList entry 1. Points static via
    // the 0xFFFFFFFF sentinel; the VarColorLine still varies per stop.
    let p_lin_grad = b.len() as u32;
    b.u8(5);
    let lin_cl_slot = b.slot24();
    for v in [1i16, 2, 3, 4, 5, 6] {
        b.i16(v);
    }
    b.u32(0xFFFF_FFFF);

    // PaintSweepGradient { centre (30, 40), start −2.0 → −180°,
    // end 0.0 → +180° } — backdrop of the composite.
    let p_sweep = b.len() as u32;
    b.u8(8);
    let sweep_cl_slot = b.slot24();
    b.i16(30);
    b.i16(40);
    b.i16(-32768);
    b.i16(0);

    // PaintVarSolid { palette 1, alpha 0.5, varIndexBase 0 } — root of
    // gid 7 and composite source.
    let p_var_solid = b.len() as u32;
    b.u8(3);
    b.u16(1);
    b.i16(8192);
    b.u32(0);

    // PaintSolid { palette 3, alpha 1.0 } — the shared leaf.
    let p_solid = b.len() as u32;
    b.u8(2);
    b.u16(3);
    b.i16(16384);

    // VarColorLine for the linear gradient: extend Repeat, two stops.
    // Stop A: offset 1.0 (varIndexBase 1: −0.5 at wght=1), alpha 1.0
    //         (delta +100 → clamps at 1.0).
    // Stop B: offset 0.75, alpha 0.25, varIndexBase sentinel.
    let lin_cl = b.len() as u32;
    b.u8(1); // EXTEND_REPEAT
    b.u16(2);
    b.i16(16384);
    b.u16(2);
    b.i16(16384);
    b.u32(1);
    b.i16(12288);
    b.u16(4);
    b.i16(4096);
    b.u32(0xFFFF_FFFF);

    // ColorLine for the sweep gradient: extend 200 (unrecognised →
    // Pad), one stop at 0.0 / palette 0xFFFF / alpha −0.5 (clamps to 0).
    let sweep_cl = b.len() as u32;
    b.u8(200);
    b.u16(1);
    b.i16(0);
    b.u16(0xFFFF);
    b.i16(-8192);

    // VarAffine2x3 { xx 1.0, yx 0, xy 0, yy 1.0, dx 8.0, dy 0,
    // varIndexBase 4 }: xx += rows[4] (+0.25), yx += rows[5]
    // (200/65536), the last four fields index past the store → 0.
    let affine = b.len() as u32;
    b.i32(65536);
    b.i32(0);
    b.i32(0);
    b.i32(65536);
    b.i32(8 * 65536);
    b.i32(0);
    b.u32(4);

    // ---- Patch all the deferred offsets ----
    // BaseGlyphList paint offsets are relative to the BGL start.
    for (slot, target) in
        bgl_paint_slots
            .iter()
            .zip([p_layers, p_var_solid, p_composite, p_colr_glyph, p_bad])
    {
        b.patch32(*slot, target - bgl);
    }
    // LayerList offsets are relative to the LayerList start.
    b.patch32(ll_slot0, p_glyph - ll);
    b.patch32(ll_slot1, p_lin_grad - ll);
    // Paint-relative Offset24 children.
    b.patch24(comp_src_slot, p_var_solid - p_composite);
    b.patch24(comp_back_slot, p_sweep - p_composite);
    b.patch24(translate_child_slot, p_glyph - p_translate);
    b.patch24(transform_child_slot, p_solid - p_transform);
    b.patch24(affine_slot, affine - p_transform);
    b.patch24(scale16_child_slot, p_solid - p_scale16);
    b.patch24(scale23_child_slot, p_solid - p_scale23);
    b.patch24(rotate_child_slot, p_solid - p_rotate26);
    b.patch24(skew_child_slot, p_solid - p_skew28);
    b.patch24(glyph_child_slot, p_solid - p_glyph);
    b.patch24(lin_cl_slot, lin_cl - p_lin_grad);
    b.patch24(sweep_cl_slot, sweep_cl - p_sweep);

    MainTable {
        bytes: b.v,
        p_solid,
        p_var_solid,
        p_glyph,
        p_lin_grad,
        p_sweep,
        p_transform,
        p_translate,
        p_scale16,
        p_scale23,
        p_rotate26,
        p_skew28,
        p_composite,
    }
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn v1_header_and_base_glyph_list() {
    let t = build_main_table();
    let colr = ColrTable::parse(&t.bytes).expect("parse");
    assert!(colr.has_paint_graph());
    assert!(colr.has_variations());
    assert!(!colr.var_index_map_unsupported());
    assert_eq!(colr.num_base_glyph_paint_records(), 5);
    assert_eq!(colr.layer_list_len(), 2);
    // Binary-search hits and misses.
    for gid in [5u16, 7, 9, 11, 13] {
        assert!(colr.base_glyph_paint(gid).is_some(), "gid {gid}");
    }
    for gid in [0u16, 4, 6, 8, 10, 12, 14, 0xFFFF] {
        assert!(colr.base_glyph_paint(gid).is_none(), "gid {gid}");
    }
    // Record enumeration preserves wire order.
    let gids: Vec<u16> = colr.base_glyph_paint_records().map(|(g, _)| g).collect();
    assert_eq!(gids, vec![5, 7, 9, 11, 13]);
}

#[test]
fn paint_colr_layers_slices_layer_list() {
    let t = build_main_table();
    let colr = ColrTable::parse(&t.bytes).expect("parse");
    let root = colr.base_glyph_paint(5).expect("root");
    assert_eq!(colr.paint_format(root), Some(1));
    let Some(Paint::ColrLayers { layers }) = colr.paint(root, &[]) else {
        panic!("expected ColrLayers");
    };
    assert_eq!(
        layers,
        vec![PaintRef(t.p_glyph), PaintRef(t.p_lin_grad)],
        "bottom-up z-order out of the LayerList"
    );
}

#[test]
fn paint_glyph_clips_child_fill() {
    let t = build_main_table();
    let colr = ColrTable::parse(&t.bytes).expect("parse");
    let Some(Paint::Glyph { paint, glyph_id }) = colr.paint(PaintRef(t.p_glyph), &[]) else {
        panic!("expected Glyph");
    };
    assert_eq!(glyph_id, 40);
    assert_eq!(paint, PaintRef(t.p_solid));
    // The leaf solid.
    let Some(Paint::Solid {
        palette_index,
        alpha,
    }) = colr.paint(paint, &[])
    else {
        panic!("expected Solid");
    };
    assert_eq!(palette_index, 3);
    assert!(approx(alpha, 1.0));
}

#[test]
fn var_solid_folds_alpha_delta_and_clamps() {
    let t = build_main_table();
    let colr = ColrTable::parse(&t.bytes).expect("parse");
    let root = colr.base_glyph_paint(7).expect("root");
    assert_eq!(colr.paint_format(root), Some(3));
    // Default instance: raw 0.5 alpha, no delta.
    let Some(Paint::Solid { alpha, .. }) = colr.paint(root, &[]) else {
        panic!("expected Solid");
    };
    assert!(approx(alpha, 0.5));
    // wght = +1: rows[0] = +8192 → alpha 1.0.
    let Some(Paint::Solid { alpha, .. }) = colr.paint(root, &[1.0]) else {
        panic!("expected Solid");
    };
    assert!(approx(alpha, 1.0));
    // Half-way: scalar 0.5 → 0.75.
    let Some(Paint::Solid { alpha, .. }) = colr.paint(root, &[0.5]) else {
        panic!("expected Solid");
    };
    assert!(approx(alpha, 0.75));
}

#[test]
fn var_linear_gradient_stops_vary_sort_and_clamp() {
    let t = build_main_table();
    let colr = ColrTable::parse(&t.bytes).expect("parse");
    let p = PaintRef(t.p_lin_grad);
    assert_eq!(colr.paint_format(p), Some(5));

    // Default instance: stops sort to [0.75, 1.0]; points untouched.
    let Some(Paint::LinearGradient {
        color_line,
        x0,
        y0,
        x1,
        y1,
        x2,
        y2,
    }) = colr.paint(p, &[])
    else {
        panic!("expected LinearGradient");
    };
    assert_eq!(color_line.extend, Extend::Repeat);
    assert_eq!((x0, y0, x1, y1, x2, y2), (1.0, 2.0, 3.0, 4.0, 5.0, 6.0));
    let offs: Vec<f32> = color_line.stops.iter().map(|s| s.stop_offset).collect();
    assert!(approx(offs[0], 0.75) && approx(offs[1], 1.0));
    assert_eq!(color_line.stops[1].palette_index, 2);

    // wght = +1: stop A moves 1.0 → 0.5 (rows[1] = −8192) and its
    // alpha clamps at 1.0 (rows[2] = +100 on a raw 1.0); the sort is
    // re-established *after* the deltas, so A now precedes B.
    let Some(Paint::LinearGradient { color_line, .. }) = colr.paint(p, &[1.0]) else {
        panic!("expected LinearGradient");
    };
    let offs: Vec<f32> = color_line.stops.iter().map(|s| s.stop_offset).collect();
    assert!(approx(offs[0], 0.5) && approx(offs[1], 0.75), "{offs:?}");
    assert_eq!(color_line.stops[0].palette_index, 2, "stop A first now");
    assert!(approx(color_line.stops[0].alpha, 1.0), "alpha clamped");
    // Stop B carries the 0xFFFFFFFF sentinel: untouched.
    assert!(approx(color_line.stops[1].alpha, 0.25));
    // Points carried the sentinel too.
    let Some(Paint::LinearGradient { x0, .. }) = colr.paint(p, &[1.0]) else {
        panic!("expected LinearGradient");
    };
    assert!(approx(x0, 1.0));
}

#[test]
fn sweep_gradient_applies_angle_bias() {
    let t = build_main_table();
    let colr = ColrTable::parse(&t.bytes).expect("parse");
    let Some(Paint::SweepGradient {
        color_line,
        center_x,
        center_y,
        start_angle_degrees,
        end_angle_degrees,
    }) = colr.paint(PaintRef(t.p_sweep), &[])
    else {
        panic!("expected SweepGradient");
    };
    // F2DOT14 −2.0 → −180°, 0.0 → +180° (the +1.0 bias).
    assert!(approx(start_angle_degrees, -180.0));
    assert!(approx(end_angle_degrees, 180.0));
    assert_eq!((center_x, center_y), (30.0, 40.0));
    // Unrecognised extend value 200 → Pad; alpha −0.5 clamps to 0;
    // palette 0xFFFF (foreground) passes through.
    assert_eq!(color_line.extend, Extend::Pad);
    assert_eq!(
        color_line.stops,
        vec![ColorStop {
            stop_offset: 0.0,
            palette_index: 0xFFFF,
            alpha: 0.0,
        }]
    );
}

#[test]
fn var_transform_resolves_fixed_deltas() {
    let t = build_main_table();
    let colr = ColrTable::parse(&t.bytes).expect("parse");
    let p = PaintRef(t.p_transform);
    assert_eq!(colr.paint_format(p), Some(13));
    // Default instance: the raw matrix.
    let Some(Paint::Transform { paint, transform }) = colr.paint(p, &[]) else {
        panic!("expected Transform");
    };
    assert_eq!(paint, PaintRef(t.p_solid));
    assert_eq!(
        transform,
        Affine2x3 {
            xx: 1.0,
            yx: 0.0,
            xy: 0.0,
            yy: 1.0,
            dx: 8.0,
            dy: 0.0
        }
    );
    // wght = +1: the six fields consume rows[4..10] — xx +0.25,
    // yx +200/65536, xy +25/65536, yy −25/65536; dx / dy index past
    // the store rows → no delta.
    let Some(Paint::Transform { transform, .. }) = colr.paint(p, &[1.0]) else {
        panic!("expected Transform");
    };
    assert!(approx(transform.xx, 1.25));
    assert!(approx(transform.yx, 200.0 / 65536.0));
    assert!(approx(transform.xy, 25.0 / 65536.0));
    assert!(approx(transform.yy, 1.0 - 25.0 / 65536.0));
    assert!(approx(transform.dx, 8.0));
    assert!(approx(transform.dy, 0.0));
}

#[test]
fn var_translate_folds_font_unit_deltas() {
    let t = build_main_table();
    let colr = ColrTable::parse(&t.bytes).expect("parse");
    let p = PaintRef(t.p_translate);
    let Some(Paint::Translate { paint, dx, dy }) = colr.paint(p, &[]) else {
        panic!("expected Translate");
    };
    assert_eq!(paint, PaintRef(t.p_glyph));
    assert_eq!((dx, dy), (10.0, 20.0));
    // rows[2] = +100, rows[3] = −50.
    let Some(Paint::Translate { dx, dy, .. }) = colr.paint(p, &[1.0]) else {
        panic!("expected Translate");
    };
    assert!(approx(dx, 110.0) && approx(dy, -30.0));
}

#[test]
fn scale_forms_fold_into_one_variant() {
    let t = build_main_table();
    let colr = ColrTable::parse(&t.bytes).expect("parse");
    // Plain PaintScale: independent x/y factors, origin centre.
    let Some(Paint::Scale {
        scale_x,
        scale_y,
        center_x,
        center_y,
        ..
    }) = colr.paint(PaintRef(t.p_scale16), &[])
    else {
        panic!("expected Scale");
    };
    assert!(approx(scale_x, 0.5) && approx(scale_y, 1.0));
    assert_eq!((center_x, center_y), (0.0, 0.0));
    assert_eq!(colr.paint_format(PaintRef(t.p_scale16)), Some(16));

    // PaintVarScaleUniformAroundCenter at the default instance.
    let p23 = PaintRef(t.p_scale23);
    assert_eq!(colr.paint_format(p23), Some(23));
    let Some(Paint::Scale {
        scale_x,
        scale_y,
        center_x,
        center_y,
        ..
    }) = colr.paint(p23, &[])
    else {
        panic!("expected Scale");
    };
    assert!(approx(scale_x, 0.5) && approx(scale_y, 0.5), "uniform");
    assert_eq!((center_x, center_y), (5.0, 7.0));
    // At wght = +1: scale += rows[0]/16384 = +0.5 → 1.0 on both axes;
    // centre consumes rows[1] / rows[2].
    let Some(Paint::Scale {
        scale_x,
        scale_y,
        center_x,
        center_y,
        ..
    }) = colr.paint(p23, &[1.0])
    else {
        panic!("expected Scale");
    };
    assert!(approx(scale_x, 1.0) && approx(scale_y, 1.0));
    assert!(approx(center_x, 5.0 - 8192.0) && approx(center_y, 7.0 + 100.0));
}

#[test]
fn rotate_and_skew_angles_have_no_bias() {
    let t = build_main_table();
    let colr = ColrTable::parse(&t.bytes).expect("parse");
    let Some(Paint::Rotate {
        angle_degrees,
        center_x,
        center_y,
        ..
    }) = colr.paint(PaintRef(t.p_rotate26), &[])
    else {
        panic!("expected Rotate");
    };
    // F2DOT14 0.5 × 180 = 90° — no +1.0 bias for rotations.
    assert!(approx(angle_degrees, 90.0));
    assert_eq!((center_x, center_y), (11.0, 12.0));

    let Some(Paint::Skew {
        x_skew_degrees,
        y_skew_degrees,
        center_x,
        center_y,
        ..
    }) = colr.paint(PaintRef(t.p_skew28), &[])
    else {
        panic!("expected Skew");
    };
    assert!(approx(x_skew_degrees, -45.0));
    assert!(approx(y_skew_degrees, 0.0));
    assert_eq!((center_x, center_y), (0.0, 0.0));
}

#[test]
fn composite_decodes_mode_and_children() {
    let t = build_main_table();
    let colr = ColrTable::parse(&t.bytes).expect("parse");
    let root = colr.base_glyph_paint(9).expect("root");
    assert_eq!(root, PaintRef(t.p_composite));
    let Some(Paint::Composite {
        source,
        mode,
        backdrop,
    }) = colr.paint(root, &[])
    else {
        panic!("expected Composite");
    };
    assert_eq!(mode, CompositeMode::Plus);
    assert_eq!(source, PaintRef(t.p_var_solid));
    assert_eq!(backdrop, PaintRef(t.p_sweep));
}

#[test]
fn composite_mode_boundedness_table() {
    use CompositeMode::*;
    // Always bounded.
    assert!(Clear.is_bounded(false, false));
    // Bounded iff source bounded.
    assert!(Src.is_bounded(true, false) && !Src.is_bounded(false, true));
    assert!(SrcOut.is_bounded(true, false) && !SrcOut.is_bounded(false, true));
    // Bounded iff backdrop bounded.
    assert!(Dest.is_bounded(false, true) && !Dest.is_bounded(true, false));
    assert!(DestOut.is_bounded(false, true) && !DestOut.is_bounded(true, false));
    // Bounded iff either bounded.
    assert!(SrcIn.is_bounded(true, false) && SrcIn.is_bounded(false, true));
    assert!(!DestIn.is_bounded(false, false));
    // Everything else: both.
    for m in [SrcOver, Plus, Screen, Multiply, HslLuminosity] {
        assert!(m.is_bounded(true, true));
        assert!(!m.is_bounded(true, false));
        assert!(!m.is_bounded(false, true));
    }
}

#[test]
fn colr_glyph_reuses_base_glyph_graph() {
    let t = build_main_table();
    let colr = ColrTable::parse(&t.bytes).expect("parse");
    let root = colr.base_glyph_paint(11).expect("root");
    let Some(Paint::ColrGlyph { glyph_id }) = colr.paint(root, &[]) else {
        panic!("expected ColrGlyph");
    };
    assert_eq!(glyph_id, 5);
    // Resolving the referenced base glyph lands on gid 5's root.
    let reused = colr.base_glyph_paint(glyph_id).expect("reused root");
    assert!(matches!(
        colr.paint(reused, &[]),
        Some(Paint::ColrLayers { .. })
    ));
}

#[test]
fn unrecognised_paint_format_is_ignored() {
    let t = build_main_table();
    let colr = ColrTable::parse(&t.bytes).expect("parse");
    let root = colr.base_glyph_paint(13).expect("root");
    assert_eq!(colr.paint_format(root), Some(99));
    assert_eq!(colr.paint(root, &[]), None);
}

#[test]
fn clip_list_static_and_variable_boxes() {
    let t = build_main_table();
    let colr = ColrTable::parse(&t.bytes).expect("parse");
    // Static box covers gids 5..=6.
    let expect = ClipBox {
        x_min: 10,
        y_min: -20,
        x_max: 300,
        y_max: 400,
    };
    assert_eq!(colr.clip_box(5, &[]), Some(expect));
    assert_eq!(colr.clip_box(6, &[]), Some(expect));
    // Out-of-range gids miss.
    assert_eq!(colr.clip_box(4, &[]), None);
    assert_eq!(colr.clip_box(8, &[]), None);
    assert_eq!(colr.clip_box(0xFFFF, &[]), None);
    // The variable box equals its raw values at the default instance…
    assert_eq!(colr.clip_box(7, &[]), Some(expect));
    // …applies whole deltas at wght = +1 (rows 6/7 = ±25; the xMax /
    // yMax indices 8/9 walk past the store rows → no delta)…
    assert_eq!(
        colr.clip_box(7, &[1.0]),
        Some(ClipBox {
            x_min: 35,
            y_min: -45,
            x_max: 300,
            y_max: 400,
        })
    );
    // …and rounds *outward* on fractional deltas (scalar 0.5 →
    // ±12.5): xMin 22.5 floors to 22, yMin −32.5 floors to −33.
    assert_eq!(
        colr.clip_box(7, &[0.5]),
        Some(ClipBox {
            x_min: 22,
            y_min: -33,
            x_max: 300,
            y_max: 400,
        })
    );
}

/// A depth-bounded traversal over the whole gid-5 graph touches every
/// reachable node without recursion blowups; the visited set doubles
/// as the cycle guard the module docs prescribe.
#[test]
fn bounded_graph_walk_visits_all_nodes() {
    let t = build_main_table();
    let colr = ColrTable::parse(&t.bytes).expect("parse");
    let mut stack: Vec<(PaintRef, u32)> = colr
        .base_glyph_paint_records()
        .map(|(_, p)| (p, 0u32))
        .collect();
    let mut visited = std::collections::BTreeSet::new();
    let mut decoded = 0usize;
    while let Some((p, depth)) = stack.pop() {
        if depth > 16 || !visited.insert(p) {
            continue;
        }
        let Some(paint) = colr.paint(p, &[1.0]) else {
            continue;
        };
        decoded += 1;
        match paint {
            Paint::ColrLayers { layers } => {
                stack.extend(layers.into_iter().map(|l| (l, depth + 1)))
            }
            Paint::Glyph { paint, .. }
            | Paint::Transform { paint, .. }
            | Paint::Translate { paint, .. }
            | Paint::Scale { paint, .. }
            | Paint::Rotate { paint, .. }
            | Paint::Skew { paint, .. } => stack.push((paint, depth + 1)),
            Paint::Composite {
                source, backdrop, ..
            } => {
                stack.push((source, depth + 1));
                stack.push((backdrop, depth + 1));
            }
            Paint::ColrGlyph { glyph_id } => {
                if let Some(root) = colr.base_glyph_paint(glyph_id) {
                    stack.push((root, depth + 1));
                }
            }
            Paint::Solid { .. }
            | Paint::LinearGradient { .. }
            | Paint::RadialGradient { .. }
            | Paint::SweepGradient { .. } => {}
        }
    }
    // Every node in the synthetic graph decodes except the format-99
    // probe.
    assert!(decoded >= 8, "decoded {decoded}");
}

/// Radial gradients: UFWORD radii resolve with font-unit deltas and
/// may go negative under variation (spec note on r(ω)); a varIndexBase
/// near the u32 ceiling resolves out-of-range / overflow indices to
/// "no delta" without wrapping.
#[test]
fn radial_gradient_radius_deltas_and_index_overflow() {
    let mut b = B::default();
    b.u16(1);
    b.u16(0);
    b.u32(0);
    b.u32(0);
    b.u16(0);
    let bgl_slot = b.slot32();
    b.u32(0);
    b.u32(0);
    b.u32(0);
    let ivs_slot = b.slot32();
    let ivs = push_ivs(&mut b, &[10, 20, -40, 0, 0, 7]);
    b.patch32(ivs_slot, ivs);
    let bgl = b.len() as u32;
    b.u32(2);
    b.u16(77);
    let paint_slot = b.slot32();
    b.u16(78);
    let paint_slot2 = b.slot32();
    b.patch32(bgl_slot, bgl);
    // gid 77: varIndexBase 0 → x0 +10, y0 +20, radius0 −40, radius1 +7.
    let p_radial = b.len() as u32;
    b.u8(7);
    let cl_slot = b.slot24();
    b.i16(0); // x0
    b.i16(0); // y0
    b.u16(25); // radius0 (UFWORD)
    b.i16(10); // x1
    b.i16(0); // y1
    b.u16(50); // radius1
    b.u32(0);
    // gid 78: varIndexBase 0xFFFFFFFE → field 0 maps to identity
    // outer 0xFFFF (out of range → 0 delta), field 1 is the +1 =
    // 0xFFFFFFFF index (still just an out-of-range identity pair —
    // the sentinel applies to varIndexBase itself, not derived
    // indices), fields 2+ overflow u32 → 0 delta. All static.
    let p_radial2 = b.len() as u32;
    b.u8(7);
    let cl_slot2 = b.slot24();
    b.i16(0);
    b.i16(0);
    b.u16(25);
    b.i16(10);
    b.i16(0);
    b.u16(50);
    b.u32(0xFFFF_FFFE);
    let cl = b.len() as u32;
    b.u8(0);
    b.u16(1);
    b.i16(0);
    b.u16(0);
    b.i16(16384);
    b.u32(0xFFFF_FFFF);
    b.patch24(cl_slot, cl - p_radial);
    b.patch24(cl_slot2, cl - p_radial2);
    b.patch32(paint_slot, p_radial - bgl);
    b.patch32(paint_slot2, p_radial2 - bgl);

    let colr = ColrTable::parse(&b.v).expect("parse");
    let root = colr.base_glyph_paint(77).expect("root");
    let Some(Paint::RadialGradient {
        x0,
        y0,
        radius0,
        radius1,
        ..
    }) = colr.paint(root, &[1.0])
    else {
        panic!("expected RadialGradient");
    };
    assert!(approx(x0, 10.0) && approx(y0, 20.0));
    assert!(approx(radius0, -15.0), "radius driven negative: {radius0}");
    assert!(approx(radius1, 57.0));
    // Static at the default instance.
    let Some(Paint::RadialGradient { radius0, .. }) = colr.paint(root, &[]) else {
        panic!("expected RadialGradient");
    };
    assert!(approx(radius0, 25.0));
    // The near-ceiling varIndexBase is inert.
    let root2 = colr.base_glyph_paint(78).expect("root");
    let Some(Paint::RadialGradient {
        x0,
        radius0,
        radius1,
        ..
    }) = colr.paint(root2, &[1.0])
    else {
        panic!("expected RadialGradient");
    };
    assert!(approx(x0, 0.0) && approx(radius0, 25.0) && approx(radius1, 50.0));
}

/// A format-0 DeltaSetIndexMap routes varIndexBase values through its
/// entries: out-of-range indices clamp to the last entry, and the
/// 0xFFFF/0xFFFF entry means "no variation data".
#[test]
fn var_index_map_routing_clamp_and_no_data_sentinel() {
    // Three solids with varIndexBase 0 / 1 / 9 against a 3-entry map:
    //   entry 0 → (0, 1)      (delta rows[1] = −8192 → −0.5)
    //   entry 1 → 0xFFFF/0xFFFF (no variation data)
    //   entry 2 → (0, 0)      (delta rows[0] = +8192 → +0.5)
    // varIndexBase 9 clamps to entry 2.
    let mut b = B::default();
    b.u16(1);
    b.u16(0);
    b.u32(0);
    b.u32(0);
    b.u16(0);
    let bgl_slot = b.slot32();
    b.u32(0);
    b.u32(0);
    let map_slot = b.slot32();
    let ivs_slot = b.slot32();
    let ivs = push_ivs(&mut b, &[8192, -8192]);
    b.patch32(ivs_slot, ivs);
    // DeltaSetIndexMap, staged §7.3.5.2 layout == 1.9 format 0:
    // entryFormat 0x003F → 4-byte entries, 16 inner bits.
    let map = b.len() as u32;
    b.u16(0x003F);
    b.u16(3);
    b.u32(1); // (outer 0, inner 1)
    b.u32(0xFFFF_FFFF); // (0xFFFF, 0xFFFF) — the no-data sentinel
    b.u32(0); // (outer 0, inner 0)
    b.patch32(map_slot, map);
    let bgl = b.len() as u32;
    b.u32(3);
    let mut slots = Vec::new();
    for gid in [1u16, 2, 3] {
        b.u16(gid);
        slots.push(b.slot32());
    }
    b.patch32(bgl_slot, bgl);
    for (slot, vb) in slots.iter().zip([0u32, 1, 9]) {
        let p = b.len() as u32;
        b.u8(3);
        b.u16(0);
        b.i16(8192); // alpha 0.5
        b.u32(vb);
        b.patch32(*slot, p - bgl);
    }

    let colr = ColrTable::parse(&b.v).expect("parse");
    assert!(!colr.var_index_map_unsupported());
    let alpha_of = |gid: u16, coords: &[f32]| -> f32 {
        let Some(Paint::Solid { alpha, .. }) =
            colr.paint(colr.base_glyph_paint(gid).unwrap(), coords)
        else {
            panic!("expected Solid");
        };
        alpha
    };
    // gid 1: vb 0 → entry 0 → inner 1 → −0.5 → alpha 0.0.
    assert!(approx(alpha_of(1, &[1.0]), 0.0));
    // gid 2: vb 1 → entry 1 → no-data sentinel → alpha 0.5.
    assert!(approx(alpha_of(2, &[1.0]), 0.5));
    // gid 3: vb 9 → clamps to entry 2 → inner 0 → +0.5 → alpha 1.0.
    assert!(approx(alpha_of(3, &[1.0]), 1.0));
    // All static at the default instance.
    for gid in [1, 2, 3] {
        assert!(approx(alpha_of(gid, &[]), 0.5));
    }
}

/// An OpenType 1.9 format-1 DeltaSetIndexMap (leading 0x01 format
/// byte) is outside the staged layouts: the table still parses, the
/// degradation is flagged, and variation deltas resolve to 0.
#[test]
fn format1_var_index_map_degrades_to_static() {
    let mut b = B::default();
    b.u16(1);
    b.u16(0);
    b.u32(0);
    b.u32(0);
    b.u16(0);
    let bgl_slot = b.slot32();
    b.u32(0);
    b.u32(0);
    let map_slot = b.slot32();
    let ivs_slot = b.slot32();
    let ivs = push_ivs(&mut b, &[8192]);
    b.patch32(ivs_slot, ivs);
    // Format-1 map: format byte 1, entryFormat 0x00, uint32 mapCount.
    let map = b.len() as u32;
    b.u8(1);
    b.u8(0x00);
    b.u32(1);
    b.u32(0);
    b.patch32(map_slot, map);
    let bgl = b.len() as u32;
    b.u32(1);
    b.u16(1);
    let slot = b.slot32();
    b.patch32(bgl_slot, bgl);
    let p = b.len() as u32;
    b.u8(3);
    b.u16(0);
    b.i16(8192);
    b.u32(0);
    b.patch32(slot, p - bgl);

    let colr = ColrTable::parse(&b.v).expect("parse");
    assert!(colr.var_index_map_unsupported());
    let Some(Paint::Solid { alpha, .. }) = colr.paint(colr.base_glyph_paint(1).unwrap(), &[1.0])
    else {
        panic!("expected Solid");
    };
    assert!(approx(alpha, 0.5), "delta suppressed, static value");
}

/// Boundedness analysis (staged reference §9 + the §6 composite-mode
/// table) over the main synthetic graph.
#[test]
fn boundedness_of_main_graph() {
    let t = build_main_table();
    let colr = ColrTable::parse(&t.bytes).expect("parse");
    // A PaintGlyph clips its fill: inherently bounded — and so is any
    // transform stack above one.
    assert_eq!(colr.paint_is_bounded(PaintRef(t.p_glyph)), Some(true));
    assert_eq!(colr.paint_is_bounded(PaintRef(t.p_translate)), Some(true));
    // Bare fills are unbounded, transforms of them too.
    assert_eq!(colr.paint_is_bounded(PaintRef(t.p_solid)), Some(false));
    assert_eq!(colr.paint_is_bounded(PaintRef(t.p_var_solid)), Some(false));
    assert_eq!(colr.paint_is_bounded(PaintRef(t.p_lin_grad)), Some(false));
    assert_eq!(colr.paint_is_bounded(PaintRef(t.p_sweep)), Some(false));
    assert_eq!(colr.paint_is_bounded(PaintRef(t.p_transform)), Some(false));
    assert_eq!(colr.paint_is_bounded(PaintRef(t.p_scale16)), Some(false));
    assert_eq!(colr.paint_is_bounded(PaintRef(t.p_scale23)), Some(false));
    assert_eq!(colr.paint_is_bounded(PaintRef(t.p_rotate26)), Some(false));
    assert_eq!(colr.paint_is_bounded(PaintRef(t.p_skew28)), Some(false));
    // gid 5's layer slice mixes a bounded PaintGlyph with an unbounded
    // gradient: not bounded overall. gid 11 reuses gid 5's graph.
    assert_eq!(colr.color_glyph_is_bounded(5), Some(false));
    assert_eq!(colr.color_glyph_is_bounded(11), Some(false));
    // gid 9: Plus needs both sides bounded; both are fills.
    assert_eq!(colr.color_glyph_is_bounded(9), Some(false));
    assert_eq!(colr.paint_is_bounded(PaintRef(t.p_composite)), Some(false));
    // gid 13 is the unrecognised-format probe: not well-formed.
    assert_eq!(colr.color_glyph_is_bounded(13), None);
    // No paint record at all.
    assert_eq!(colr.color_glyph_is_bounded(4), None);
}

/// Composite-mode boundedness through real graphs, plus cycle and
/// dangling-reference rejection.
#[test]
fn boundedness_composites_cycles_and_dangling_refs() {
    let mut b = B::default();
    b.u16(1);
    b.u16(0);
    b.u32(0);
    b.u32(0);
    b.u16(0);
    let bgl_slot = b.slot32();
    b.u32(0);
    b.u32(0);
    b.u32(0);
    b.u32(0);
    let bgl = b.len() as u32;
    b.u32(4);
    let mut slots = Vec::new();
    for gid in [1u16, 2, 3, 4] {
        b.u16(gid);
        slots.push(b.slot32());
    }
    b.patch32(bgl_slot, bgl);

    // gid 1: PaintColrGlyph(1) — a self-cycle.
    let p_cycle = b.len() as u32;
    b.u8(11);
    b.u16(1);
    // gid 2: PaintColrGlyph(99) — dangling base-glyph reference.
    let p_dangling = b.len() as u32;
    b.u8(11);
    b.u16(99);
    // gid 3: Composite(SrcIn, source = PaintGlyph -> Solid, backdrop =
    // Solid): SrcIn is bounded iff either side is.
    let p_src_in = b.len() as u32;
    b.u8(32);
    let src_slot = b.slot24();
    b.u8(5); // COMPOSITE_SRC_IN
    let back_slot = b.slot24();
    // gid 4: Composite(Clear, Solid, Solid): always bounded.
    let p_clear = b.len() as u32;
    b.u8(32);
    let clear_src_slot = b.slot24();
    b.u8(0); // COMPOSITE_CLEAR
    let clear_back_slot = b.slot24();
    // Shared leaves.
    let p_glyph = b.len() as u32;
    b.u8(10);
    let glyph_child_slot = b.slot24();
    b.u16(33);
    let p_solid = b.len() as u32;
    b.u8(2);
    b.u16(0);
    b.i16(16384);

    b.patch24(src_slot, p_glyph - p_src_in);
    b.patch24(back_slot, p_solid - p_src_in);
    b.patch24(clear_src_slot, p_solid - p_clear);
    b.patch24(clear_back_slot, p_solid - p_clear);
    b.patch24(glyph_child_slot, p_solid - p_glyph);
    for (slot, target) in slots.iter().zip([p_cycle, p_dangling, p_src_in, p_clear]) {
        b.patch32(*slot, target - bgl);
    }

    let colr = ColrTable::parse(&b.v).expect("parse");
    assert_eq!(colr.color_glyph_is_bounded(1), None, "self-cycle");
    assert_eq!(colr.color_glyph_is_bounded(2), None, "dangling ref");
    assert_eq!(colr.color_glyph_is_bounded(3), Some(true), "SrcIn either");
    assert_eq!(colr.color_glyph_is_bounded(4), Some(true), "Clear always");
    // The decode surface itself stays total on the cyclic node.
    assert!(matches!(
        colr.paint(PaintRef(p_cycle), &[]),
        Some(Paint::ColrGlyph { glyph_id: 1 })
    ));
}

/// Deterministic hostile-input slice for the v1 structures: prefix
/// truncations and blind byte flips of the synthetic table must never
/// panic — parse either fails cleanly or yields a table whose paint /
/// clip accessors stay total.
#[test]
fn hostile_mutations_never_panic() {
    let t = build_main_table();
    // Every prefix truncation.
    for len in 0..t.bytes.len() {
        let slice = &t.bytes[..len];
        if let Ok(colr) = ColrTable::parse(slice) {
            let _ = colr.base_glyph_paint(5);
            let _ = colr.clip_box(7, &[0.5]);
        }
    }
    // Blind deterministic byte flips (xor a marching pattern), a few
    // positions at a time, over the whole table.
    let mut rng: u32 = 0x1234_5678;
    for round in 0..2048 {
        let mut bytes = t.bytes.clone();
        for _ in 0..1 + (round % 4) {
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let pos = (rng >> 8) as usize % bytes.len();
            bytes[pos] ^= (rng & 0xFF) as u8;
        }
        let Ok(colr) = ColrTable::parse(&bytes) else {
            continue;
        };
        // Walk every root with a bounded visitor at two instances.
        for coords in [&[][..], &[1.0][..]] {
            let mut stack: Vec<(PaintRef, u32)> = colr
                .base_glyph_paint_records()
                .map(|(_, p)| (p, 0u32))
                .collect();
            let mut visited = std::collections::BTreeSet::new();
            while let Some((p, depth)) = stack.pop() {
                if depth > 8 || !visited.insert(p) {
                    continue;
                }
                match colr.paint(p, coords) {
                    Some(Paint::ColrLayers { layers }) => {
                        stack.extend(layers.into_iter().map(|l| (l, depth + 1)))
                    }
                    Some(
                        Paint::Glyph { paint, .. }
                        | Paint::Transform { paint, .. }
                        | Paint::Translate { paint, .. }
                        | Paint::Scale { paint, .. }
                        | Paint::Rotate { paint, .. }
                        | Paint::Skew { paint, .. },
                    ) => stack.push((paint, depth + 1)),
                    Some(Paint::Composite {
                        source, backdrop, ..
                    }) => {
                        stack.push((source, depth + 1));
                        stack.push((backdrop, depth + 1));
                    }
                    Some(Paint::ColrGlyph { glyph_id }) => {
                        if let Some(root) = colr.base_glyph_paint(glyph_id) {
                            stack.push((root, depth + 1));
                        }
                    }
                    _ => {}
                }
            }
            for gid in 0..16u16 {
                let _ = colr.clip_box(gid, &[0.25]);
            }
        }
    }
}
