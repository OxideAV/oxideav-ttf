//! `glyf` — glyph data (TrueType outlines + composite references).
//!
//! Each glyph starts with a 10-byte header
//! `(numberOfContours: i16, xMin, yMin, xMax, yMax: i16)`. A negative
//! `numberOfContours` indicates a composite glyph; otherwise the body
//! holds simple TT outline data.
//!
//! Spec: Microsoft OpenType `glyf` (TrueType outlines and composites).

use crate::outline::{derive_bbox, BBox, Contour, Point, TtOutline};
use crate::parser::{read_i16, read_u16, read_u8};
use crate::tables::loca::LocaTable;
use crate::Error;

const MAX_COMPOSITE_DEPTH: u8 = 16;

// --- simple-glyph flag bits (per spec table) -------------------------------

const FLAG_ON_CURVE: u8 = 0x01;
const FLAG_X_SHORT: u8 = 0x02;
const FLAG_Y_SHORT: u8 = 0x04;
const FLAG_REPEAT: u8 = 0x08;
/// When X_SHORT: bit set ⇒ x is positive. When NOT X_SHORT: bit set
/// ⇒ x repeats previous x (delta == 0).
const FLAG_X_SAME_OR_POS: u8 = 0x10;
const FLAG_Y_SAME_OR_POS: u8 = 0x20;
// Bit 6 reserved, bit 7 OVERLAP (no effect on geometry).

// --- composite-glyph flag bits ---------------------------------------------

const C_ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
const C_ARGS_ARE_XY_VALUES: u16 = 0x0002;
// 0x0004 ROUND_XY_TO_GRID - hinting only
const C_WE_HAVE_A_SCALE: u16 = 0x0008;
const C_MORE_COMPONENTS: u16 = 0x0020;
const C_WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
const C_WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;
const C_WE_HAVE_INSTRUCTIONS: u16 = 0x0100;
// 0x0200 USE_MY_METRICS and 0x0400 OVERLAP_COMPOUND do not affect outline
// geometry. The two offset-interpretation flags below DO: per the `glyf`
// "Composite glyph description" §, when the offset vector form is used
// (ARGS_ARE_XY_VALUES set), SCALED_COMPONENT_OFFSET means the (x, y)
// offset is in the component's pre-transform coordinate system and the 2×2
// scale/transform is applied to it before it is added to the child points;
// UNSCALED_COMPONENT_OFFSET (and the recommended default when neither is
// set) means the offset is in the parent coordinate system and the
// transform is NOT applied. A font that sets both is invalid and falls
// back to the default (unscaled) behaviour.
const C_SCALED_COMPONENT_OFFSET: u16 = 0x0800;
const C_UNSCALED_COMPONENT_OFFSET: u16 = 0x1000;

#[derive(Debug, Clone)]
pub struct GlyfTable<'a> {
    bytes: &'a [u8],
}

impl<'a> GlyfTable<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    pub fn raw(&self) -> &'a [u8] {
        self.bytes
    }

    /// Bounding box from a glyph's 10-byte header. Returns `None` if the
    /// range is too short to hold a header.
    pub fn bbox(&self, range: core::ops::Range<usize>) -> Option<BBox> {
        let body = self.bytes.get(range.clone())?;
        if body.len() < 10 {
            return None;
        }
        Some(BBox {
            x_min: read_i16(body, 2).ok()?,
            y_min: read_i16(body, 4).ok()?,
            x_max: read_i16(body, 6).ok()?,
            y_max: read_i16(body, 8).ok()?,
        })
    }

    /// Decode a glyph outline (simple or composite). `loca` is needed to
    /// resolve composite references; `depth` guards against runaway
    /// recursion.
    pub fn glyph_outline(
        &self,
        range: core::ops::Range<usize>,
        loca: &LocaTable<'a>,
        depth: u8,
    ) -> Result<TtOutline, Error> {
        if depth >= MAX_COMPOSITE_DEPTH {
            return Err(Error::CompositeTooDeep);
        }
        let body = self.bytes.get(range).ok_or(Error::BadOffset)?;
        if body.len() < 10 {
            return Ok(TtOutline::default());
        }
        let n_contours = read_i16(body, 0)?;
        let bbox = BBox {
            x_min: read_i16(body, 2)?,
            y_min: read_i16(body, 4)?,
            x_max: read_i16(body, 6)?,
            y_max: read_i16(body, 8)?,
        };
        let payload = &body[10..];
        if n_contours >= 0 {
            decode_simple(payload, n_contours as u16, bbox)
        } else {
            self.decode_composite(payload, loca, depth)
        }
    }

    fn decode_composite(
        &self,
        bytes: &[u8],
        loca: &LocaTable<'a>,
        depth: u8,
    ) -> Result<TtOutline, Error> {
        let mut out = TtOutline::default();
        let mut off = 0usize;
        loop {
            if off + 4 > bytes.len() {
                return Err(Error::BadStructure("composite truncated"));
            }
            let flags = read_u16(bytes, off)?;
            let glyph_index = read_u16(bytes, off + 2)?;
            off += 4;

            // Decode arg1 / arg2.
            let (arg1, arg2);
            if flags & C_ARG_1_AND_2_ARE_WORDS != 0 {
                if off + 4 > bytes.len() {
                    return Err(Error::BadStructure("composite arg words truncated"));
                }
                arg1 = read_i16(bytes, off)? as i32;
                arg2 = read_i16(bytes, off + 2)? as i32;
                off += 4;
            } else {
                if off + 2 > bytes.len() {
                    return Err(Error::BadStructure("composite arg bytes truncated"));
                }
                arg1 = bytes[off] as i8 as i32;
                arg2 = bytes[off + 1] as i8 as i32;
                off += 2;
            }

            // Decode 2x2 transform.
            let (xx, xy, yx, yy);
            if flags & C_WE_HAVE_A_SCALE != 0 {
                if off + 2 > bytes.len() {
                    return Err(Error::BadStructure("composite scale truncated"));
                }
                let s = f2dot14(read_i16(bytes, off)?);
                xx = s;
                yy = s;
                xy = 0.0;
                yx = 0.0;
                off += 2;
            } else if flags & C_WE_HAVE_AN_X_AND_Y_SCALE != 0 {
                if off + 4 > bytes.len() {
                    return Err(Error::BadStructure("composite x/y scale truncated"));
                }
                xx = f2dot14(read_i16(bytes, off)?);
                yy = f2dot14(read_i16(bytes, off + 2)?);
                xy = 0.0;
                yx = 0.0;
                off += 4;
            } else if flags & C_WE_HAVE_A_TWO_BY_TWO != 0 {
                if off + 8 > bytes.len() {
                    return Err(Error::BadStructure("composite 2x2 truncated"));
                }
                xx = f2dot14(read_i16(bytes, off)?);
                xy = f2dot14(read_i16(bytes, off + 2)?);
                yx = f2dot14(read_i16(bytes, off + 4)?);
                yy = f2dot14(read_i16(bytes, off + 6)?);
                off += 8;
            } else {
                xx = 1.0;
                xy = 0.0;
                yx = 0.0;
                yy = 1.0;
            }

            let child_range = loca.glyph_range(glyph_index)?;
            let child = self.glyph_outline(child_range, loca, depth + 1)?;

            if flags & C_ARGS_ARE_XY_VALUES != 0 {
                // Offset-vector placement. argument1/argument2 are an
                // (x, y) translation in design units.
                let scale_offset = flags & C_SCALED_COMPONENT_OFFSET != 0
                    && flags & C_UNSCALED_COMPONENT_OFFSET == 0;
                let (dx, dy) = if scale_offset {
                    // SCALED_COMPONENT_OFFSET: the offset is in the
                    // component's coordinate system, so the 2×2 transform
                    // applies to it before it is added to the (already
                    // transformed) child points. Transforming the offset
                    // and then translating by it is equivalent to letting
                    // `append_transformed` add the transformed offset, so
                    // we pre-transform (arg1, arg2) here and feed the
                    // result as the post-transform translation.
                    let fx = arg1 as f32;
                    let fy = arg2 as f32;
                    let tx = (fx * xx + fy * yx).round() as i32;
                    let ty = (fx * xy + fy * yy).round() as i32;
                    (tx, ty)
                } else {
                    // UNSCALED_COMPONENT_OFFSET / default: offset is in the
                    // parent coordinate system, untransformed.
                    (arg1, arg2)
                };
                out.append_transformed(&child, xx, xy, yx, yy, dx, dy);
            } else {
                // Point-matching placement. argument1 is a point number in
                // the parent (the contours accumulated from previous
                // components, re-numbered); argument2 is a point number in
                // the child (its own pre-renumber numbering). The child is
                // transformed first, then translated so child point arg2
                // coincides with parent point arg1.
                let child_t = child.transformed(xx, xy, yx, yy);
                let parent_pt = out.flat_point(arg1 as usize);
                let child_pt = child_t.flat_point(arg2 as usize);
                match (parent_pt, child_pt) {
                    (Some(pp), Some(cp)) => {
                        let dx = pp.x as i32 - cp.x as i32;
                        let dy = pp.y as i32 - cp.y as i32;
                        out.append_translated(&child_t, dx, dy);
                    }
                    _ => {
                        // A referenced point index that lands outside the
                        // real (non-phantom) point set — typically a
                        // phantom-point reference, which needs hmtx/vmtx
                        // metrics we don't thread through the outline
                        // resolver. Fall back to zero-offset placement so
                        // the contours still appear rather than dropping
                        // the component entirely.
                        out.append_translated(&child_t, 0, 0);
                    }
                }
            }

            if flags & C_MORE_COMPONENTS == 0 {
                if flags & C_WE_HAVE_INSTRUCTIONS != 0 {
                    // Skip the instruction stream entirely. Format:
                    //   u16 numInstr, then numInstr bytes of bytecode.
                    if off + 2 <= bytes.len() {
                        // numInstr + bytecode left unread; we don't run it.
                    }
                }
                break;
            }
        }
        Ok(out)
    }
}

fn f2dot14(raw: i16) -> f32 {
    raw as f32 / 16384.0
}

fn decode_simple(bytes: &[u8], n_contours: u16, bbox: BBox) -> Result<TtOutline, Error> {
    if n_contours == 0 {
        return Ok(TtOutline {
            contours: Vec::new(),
            bounds: Some(bbox),
        });
    }
    let mut off = 0usize;
    if bytes.len() < (n_contours as usize) * 2 + 2 {
        return Err(Error::BadStructure("simple glyph truncated"));
    }
    // endPtsOfContours[n] u16
    let mut end_pts = Vec::with_capacity(n_contours as usize);
    for _ in 0..n_contours {
        end_pts.push(read_u16(bytes, off)?);
        off += 2;
    }
    let n_points = (*end_pts.last().unwrap() as usize) + 1;
    // instructionLength (u16) + that many bytes of bytecode.
    let inst_len = read_u16(bytes, off)? as usize;
    off += 2;
    if off + inst_len > bytes.len() {
        return Err(Error::BadStructure("simple glyph instructions truncated"));
    }
    off += inst_len;

    // Flags array — variable length due to FLAG_REPEAT.
    let mut flags = Vec::with_capacity(n_points);
    while flags.len() < n_points {
        if off >= bytes.len() {
            return Err(Error::BadStructure("simple glyph flags truncated"));
        }
        let f = bytes[off];
        off += 1;
        flags.push(f);
        if f & FLAG_REPEAT != 0 {
            if off >= bytes.len() {
                return Err(Error::BadStructure("simple glyph flag repeat truncated"));
            }
            let rep = bytes[off];
            off += 1;
            for _ in 0..rep {
                if flags.len() >= n_points {
                    break;
                }
                flags.push(f);
            }
        }
    }
    if flags.len() != n_points {
        return Err(Error::BadStructure("simple glyph flag count mismatch"));
    }

    // x coordinates.
    let mut xs = Vec::with_capacity(n_points);
    let mut acc: i32 = 0;
    for &f in &flags {
        let dx = read_coord(bytes, &mut off, f & FLAG_X_SHORT, f & FLAG_X_SAME_OR_POS)?;
        acc += dx;
        xs.push(clamp_i16(acc));
    }
    // y coordinates.
    let mut ys = Vec::with_capacity(n_points);
    acc = 0;
    for &f in &flags {
        let dy = read_coord(bytes, &mut off, f & FLAG_Y_SHORT, f & FLAG_Y_SAME_OR_POS)?;
        acc += dy;
        ys.push(clamp_i16(acc));
    }

    // Carve into contours.
    let mut contours = Vec::with_capacity(n_contours as usize);
    let mut start = 0usize;
    for &end in &end_pts {
        let end = end as usize;
        let mut c = Contour {
            points: Vec::with_capacity(end - start + 1),
        };
        for i in start..=end {
            c.points.push(Point {
                x: xs[i],
                y: ys[i],
                on_curve: flags[i] & FLAG_ON_CURVE != 0,
            });
        }
        contours.push(c);
        start = end + 1;
    }

    let bounds = derive_bbox(&contours).or(Some(bbox));
    Ok(TtOutline { contours, bounds })
}

fn read_coord(bytes: &[u8], off: &mut usize, short: u8, same_or_pos: u8) -> Result<i32, Error> {
    if short != 0 {
        let v = read_u8(bytes, *off)?;
        *off += 1;
        Ok(if same_or_pos != 0 {
            v as i32
        } else {
            -(v as i32)
        })
    } else if same_or_pos != 0 {
        // Repeat previous value: delta 0.
        Ok(0)
    } else {
        let v = read_i16(bytes, *off)? as i32;
        *off += 2;
        Ok(v)
    }
}

fn clamp_i16(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a single-contour triangle: (0,0)→(100,0)→(50,100), all
    /// on-curve. Returns the full glyph bytes (10-byte header + body).
    fn build_triangle() -> Vec<u8> {
        let mut g = Vec::new();
        // header: 1 contour, bbox 0..100, 0..100
        g.extend_from_slice(&1i16.to_be_bytes());
        g.extend_from_slice(&0i16.to_be_bytes());
        g.extend_from_slice(&0i16.to_be_bytes());
        g.extend_from_slice(&100i16.to_be_bytes());
        g.extend_from_slice(&100i16.to_be_bytes());
        // endPtsOfContours
        g.extend_from_slice(&2u16.to_be_bytes());
        // instructionLength = 0
        g.extend_from_slice(&0u16.to_be_bytes());
        // 3 flag bytes (all on-curve).
        g.extend_from_slice(&[FLAG_ON_CURVE, FLAG_ON_CURVE, FLAG_ON_CURVE]);
        // x coords (i16 each): 0, 100, 50 -> deltas 0, 100, -50
        g.extend_from_slice(&0i16.to_be_bytes());
        g.extend_from_slice(&100i16.to_be_bytes());
        g.extend_from_slice(&(-50i16).to_be_bytes());
        // y coords: 0, 0, 100 -> deltas 0, 0, 100
        g.extend_from_slice(&0i16.to_be_bytes());
        g.extend_from_slice(&0i16.to_be_bytes());
        g.extend_from_slice(&100i16.to_be_bytes());
        g
    }

    #[test]
    fn decodes_simple_triangle() {
        let g = build_triangle();
        // Build a one-glyph loca for self-reference (won't be read).
        let mut loca_bytes = Vec::new();
        loca_bytes.extend_from_slice(&0u32.to_be_bytes());
        loca_bytes.extend_from_slice(&(g.len() as u32).to_be_bytes());
        let loca = LocaTable::parse(&loca_bytes, 1, 1).unwrap();
        let glyf = GlyfTable::new(&g);
        let out = glyf.glyph_outline(0..g.len(), &loca, 0).unwrap();
        assert_eq!(out.contours.len(), 1);
        assert_eq!(out.contours[0].points.len(), 3);
        assert_eq!(out.contours[0].points[0].x, 0);
        assert_eq!(out.contours[0].points[1].x, 100);
        assert_eq!(out.contours[0].points[2].x, 50);
        assert_eq!(out.contours[0].points[2].y, 100);
        assert!(out.contours[0].points.iter().all(|p| p.on_curve));
    }

    #[test]
    fn decodes_composite_translates_child() {
        // Two glyphs in a synthetic glyf: glyph 0 = simple triangle,
        // glyph 1 = composite translating glyph 0 by (+1000, +2000).
        let triangle = build_triangle();
        let mut composite = Vec::new();
        // header: -1 contour (composite), zero bbox
        composite.extend_from_slice(&(-1i16).to_be_bytes());
        composite.extend_from_slice(&0i16.to_be_bytes());
        composite.extend_from_slice(&0i16.to_be_bytes());
        composite.extend_from_slice(&0i16.to_be_bytes());
        composite.extend_from_slice(&0i16.to_be_bytes());
        // flags = ARGS_ARE_XY_VALUES | ARG_1_AND_2_ARE_WORDS
        let flags = C_ARGS_ARE_XY_VALUES | C_ARG_1_AND_2_ARE_WORDS;
        composite.extend_from_slice(&flags.to_be_bytes());
        // glyphIndex = 0
        composite.extend_from_slice(&0u16.to_be_bytes());
        // arg1=1000 arg2=2000 (i16 each)
        composite.extend_from_slice(&1000i16.to_be_bytes());
        composite.extend_from_slice(&2000i16.to_be_bytes());

        // Stitch glyf = triangle | composite, build loca.
        let glyf_bytes: Vec<u8> = [triangle.as_slice(), composite.as_slice()].concat();
        let tri_len = triangle.len() as u32;
        let total = glyf_bytes.len() as u32;
        let mut loca_bytes = Vec::new();
        for v in [0u32, tri_len, total] {
            loca_bytes.extend_from_slice(&v.to_be_bytes());
        }
        let loca = LocaTable::parse(&loca_bytes, 2, 1).unwrap();
        let glyf = GlyfTable::new(&glyf_bytes);

        // Decode composite (glyph 1).
        let comp_range = (tri_len as usize)..(total as usize);
        let out = glyf.glyph_outline(comp_range, &loca, 0).unwrap();
        assert_eq!(out.contours.len(), 1);
        let p0 = out.contours[0].points[0];
        assert_eq!((p0.x, p0.y), (1000, 2000));
        let p1 = out.contours[0].points[1];
        assert_eq!((p1.x, p1.y), (1100, 2000));
        let p2 = out.contours[0].points[2];
        assert_eq!((p2.x, p2.y), (1050, 2100));
    }

    /// A minimal composite-glyph body referencing exactly one child
    /// glyph at the given index (no XY offset, no transform, no MORE
    /// components). Used to build long composite chains for the
    /// depth-limit tests.
    fn build_composite_referencing(child_index: u16) -> Vec<u8> {
        let mut g = Vec::new();
        // header: -1 contour (composite), zero bbox.
        g.extend_from_slice(&(-1i16).to_be_bytes());
        g.extend_from_slice(&0i16.to_be_bytes());
        g.extend_from_slice(&0i16.to_be_bytes());
        g.extend_from_slice(&0i16.to_be_bytes());
        g.extend_from_slice(&0i16.to_be_bytes());
        // flags: ARGS_ARE_XY_VALUES; arg1/arg2 are bytes (no
        // ARG_1_AND_2_ARE_WORDS bit), so 2 bytes follow for offsets.
        let flags = C_ARGS_ARE_XY_VALUES;
        g.extend_from_slice(&flags.to_be_bytes());
        g.extend_from_slice(&child_index.to_be_bytes());
        // arg1=0 arg2=0 (i8 each)
        g.push(0);
        g.push(0);
        g
    }

    /// A composite-glyph chain `0 -> 1 -> 2 -> ... -> N-1 -> triangle`
    /// of total depth `N` must succeed when N <= MAX_COMPOSITE_DEPTH
    /// and must fail with `CompositeTooDeep` when N exceeds it.
    /// MAX_COMPOSITE_DEPTH is currently 16; we walk a 16-link chain
    /// (passes) and then a 17-link chain (fails) so the boundary is
    /// pinned on both sides.
    ///
    /// The chain layout in `glyf`:
    ///   glyph 0  = triangle (leaf)
    ///   glyph 1  = composite referencing glyph 0
    ///   glyph 2  = composite referencing glyph 1
    ///   …
    ///   glyph K  = composite referencing glyph K-1
    fn build_chained_glyf(depth: usize) -> (Vec<u8>, Vec<u32>) {
        // Glyph 0 is the triangle leaf.
        let triangle = build_triangle();
        let mut glyf = triangle.clone();
        let mut offsets: Vec<u32> = vec![0, triangle.len() as u32];
        // Glyphs 1..=depth each reference the previous glyph.
        for k in 1..=depth {
            let comp = build_composite_referencing((k - 1) as u16);
            glyf.extend_from_slice(&comp);
            offsets.push(glyf.len() as u32);
        }
        (glyf, offsets)
    }

    #[test]
    fn composite_chain_at_max_depth_succeeds() {
        // The depth guard fires when `depth >= MAX_COMPOSITE_DEPTH`,
        // and the root call enters at depth=0. So a chain whose
        // outermost composite is glyph `MAX_COMPOSITE_DEPTH - 1` walks
        // depths 0..=MAX_COMPOSITE_DEPTH-1 — the last depth tested is
        // `MAX_COMPOSITE_DEPTH - 1`, which still passes the `<` check.
        // The next deeper chain (one more link) would push the leaf
        // call to depth=MAX_COMPOSITE_DEPTH and trip the guard.
        let depth = (MAX_COMPOSITE_DEPTH as usize) - 1;
        let (glyf_bytes, offsets) = build_chained_glyf(depth);

        let mut loca_bytes = Vec::new();
        for v in &offsets {
            loca_bytes.extend_from_slice(&v.to_be_bytes());
        }
        let num_glyphs = (offsets.len() - 1) as u16;
        let loca = LocaTable::parse(&loca_bytes, num_glyphs, 1).unwrap();
        let glyf = GlyfTable::new(&glyf_bytes);

        // Decode the outermost composite (glyph `depth`).
        let top_start = offsets[depth] as usize;
        let top_end = offsets[depth + 1] as usize;
        let out = glyf
            .glyph_outline(top_start..top_end, &loca, 0)
            .expect("16-deep chain must decode");
        // The leaf triangle has three on-curve points.
        assert_eq!(out.contours.len(), 1);
        assert_eq!(out.contours[0].points.len(), 3);
    }

    #[test]
    fn composite_chain_over_max_depth_returns_composite_too_deep() {
        // Outermost composite is glyph `MAX_COMPOSITE_DEPTH`; decoding
        // it pushes the leaf call to depth = MAX_COMPOSITE_DEPTH, which
        // trips the `>=` guard immediately and rejects.
        let depth = MAX_COMPOSITE_DEPTH as usize;
        let (glyf_bytes, offsets) = build_chained_glyf(depth);

        let mut loca_bytes = Vec::new();
        for v in &offsets {
            loca_bytes.extend_from_slice(&v.to_be_bytes());
        }
        let num_glyphs = (offsets.len() - 1) as u16;
        let loca = LocaTable::parse(&loca_bytes, num_glyphs, 1).unwrap();
        let glyf = GlyfTable::new(&glyf_bytes);

        let top_start = offsets[depth] as usize;
        let top_end = offsets[depth + 1] as usize;
        let err = glyf
            .glyph_outline(top_start..top_end, &loca, 0)
            .expect_err("17-deep chain must reject");
        assert_eq!(err, Error::CompositeTooDeep);
    }

    /// A malicious / corrupted font in which a composite glyph
    /// references itself (or a cycle including itself) must terminate
    /// with `CompositeTooDeep` rather than recursing until stack
    /// overflow. The depth guard at MAX_COMPOSITE_DEPTH = 16 caps the
    /// cycle at 16 frames.
    /// SCALED_COMPONENT_OFFSET: the offset vector is expressed in the
    /// component's own coordinate system, so the 2×2 scale applies to it.
    /// Here the child triangle is scaled 2× and offset by (10, 20). With
    /// SCALED the effective translation is (20, 40); with UNSCALED it would
    /// be (10, 20). Both forms are exercised to pin the difference.
    fn build_scaled_offset_composite(child_index: u16, scaled: bool) -> Vec<u8> {
        let mut g = Vec::new();
        g.extend_from_slice(&(-1i16).to_be_bytes());
        g.extend_from_slice(&0i16.to_be_bytes());
        g.extend_from_slice(&0i16.to_be_bytes());
        g.extend_from_slice(&0i16.to_be_bytes());
        g.extend_from_slice(&0i16.to_be_bytes());
        // flags: ARGS_ARE_XY_VALUES | WE_HAVE_A_SCALE | offset-mode bit.
        let mut flags = C_ARGS_ARE_XY_VALUES | C_WE_HAVE_A_SCALE;
        flags |= if scaled {
            C_SCALED_COMPONENT_OFFSET
        } else {
            C_UNSCALED_COMPONENT_OFFSET
        };
        g.extend_from_slice(&flags.to_be_bytes());
        g.extend_from_slice(&child_index.to_be_bytes());
        // arg1=10 arg2=20 (i8 each, no ARG_1_AND_2_ARE_WORDS).
        g.push(10i8 as u8);
        g.push(20i8 as u8);
        // F2DOT14 scale = 2.0 -> 2 * 16384 = 32768 which overflows i16,
        // so use 1.5 (24576) to keep a representable signed value and a
        // clean arithmetic check.
        g.extend_from_slice(&24576i16.to_be_bytes());
        g
    }

    #[test]
    fn scaled_component_offset_transforms_the_offset_vector() {
        let triangle = build_triangle(); // points (0,0) (100,0) (50,100)
                                         // SCALED form.
        let scaled = build_scaled_offset_composite(0, true);
        let glyf_bytes: Vec<u8> = [triangle.as_slice(), scaled.as_slice()].concat();
        let tri_len = triangle.len() as u32;
        let total = glyf_bytes.len() as u32;
        let mut loca_bytes = Vec::new();
        for v in [0u32, tri_len, total] {
            loca_bytes.extend_from_slice(&v.to_be_bytes());
        }
        let loca = LocaTable::parse(&loca_bytes, 2, 1).unwrap();
        let glyf = GlyfTable::new(&glyf_bytes);
        let out = glyf
            .glyph_outline(tri_len as usize..total as usize, &loca, 0)
            .unwrap();
        // Child scaled 1.5×: (0,0)->(0,0), (100,0)->(150,0), (50,100)->(75,150).
        // SCALED offset: (10,20) transformed by 1.5 = (15, 30).
        let p = &out.contours[0].points;
        assert_eq!((p[0].x, p[0].y), (15, 30));
        assert_eq!((p[1].x, p[1].y), (165, 30));
        assert_eq!((p[2].x, p[2].y), (90, 180));
    }

    #[test]
    fn unscaled_component_offset_leaves_the_offset_vector_raw() {
        let triangle = build_triangle();
        let unscaled = build_scaled_offset_composite(0, false);
        let glyf_bytes: Vec<u8> = [triangle.as_slice(), unscaled.as_slice()].concat();
        let tri_len = triangle.len() as u32;
        let total = glyf_bytes.len() as u32;
        let mut loca_bytes = Vec::new();
        for v in [0u32, tri_len, total] {
            loca_bytes.extend_from_slice(&v.to_be_bytes());
        }
        let loca = LocaTable::parse(&loca_bytes, 2, 1).unwrap();
        let glyf = GlyfTable::new(&glyf_bytes);
        let out = glyf
            .glyph_outline(tri_len as usize..total as usize, &loca, 0)
            .unwrap();
        // Child scaled 1.5×, offset applied raw (10, 20).
        let p = &out.contours[0].points;
        assert_eq!((p[0].x, p[0].y), (10, 20));
        assert_eq!((p[1].x, p[1].y), (160, 20));
        assert_eq!((p[2].x, p[2].y), (85, 170));
    }

    /// Point-matching placement (ARGS_ARE_XY_VALUES cleared). The parent
    /// already incorporates one triangle component (points 0,1,2 at
    /// (0,0),(100,0),(50,100)). A second component (another triangle)
    /// aligns its own point 0 (child (0,0)) onto parent point 1
    /// ((100,0)), so the offset is (100, 0).
    #[test]
    fn point_matching_aligns_child_point_onto_parent_point() {
        let triangle = build_triangle();
        // Composite with two components, both triangles (glyph 0).
        let mut composite = Vec::new();
        composite.extend_from_slice(&(-1i16).to_be_bytes());
        for _ in 0..4 {
            composite.extend_from_slice(&0i16.to_be_bytes());
        }
        // Component 1: XY offset (0,0), MORE_COMPONENTS set so a second
        // component follows. First component must use ARGS_ARE_XY_VALUES.
        let c1_flags = C_ARGS_ARE_XY_VALUES | C_MORE_COMPONENTS;
        composite.extend_from_slice(&c1_flags.to_be_bytes());
        composite.extend_from_slice(&0u16.to_be_bytes()); // child = glyph 0
        composite.push(0); // arg1 = 0
        composite.push(0); // arg2 = 0
                           // Component 2: point-matching. arg1 = parent point 1, arg2 = child
                           // point 0. No ARGS_ARE_XY_VALUES bit -> point match.
        let c2_flags = 0u16; // no XY-values, no more components
        composite.extend_from_slice(&c2_flags.to_be_bytes());
        composite.extend_from_slice(&0u16.to_be_bytes()); // child = glyph 0
        composite.push(1); // arg1 = parent point index 1
        composite.push(0); // arg2 = child point index 0

        let glyf_bytes: Vec<u8> = [triangle.as_slice(), composite.as_slice()].concat();
        let tri_len = triangle.len() as u32;
        let total = glyf_bytes.len() as u32;
        let mut loca_bytes = Vec::new();
        for v in [0u32, tri_len, total] {
            loca_bytes.extend_from_slice(&v.to_be_bytes());
        }
        let loca = LocaTable::parse(&loca_bytes, 2, 1).unwrap();
        let glyf = GlyfTable::new(&glyf_bytes);
        let out = glyf
            .glyph_outline(tri_len as usize..total as usize, &loca, 0)
            .unwrap();
        assert_eq!(out.contours.len(), 2);
        // First component placed at origin.
        let a = &out.contours[0].points;
        assert_eq!((a[0].x, a[0].y), (0, 0));
        assert_eq!((a[1].x, a[1].y), (100, 0));
        // Second component aligned so child point 0 sits on parent point 1
        // (100,0): offset = (100,0).
        let b = &out.contours[1].points;
        assert_eq!((b[0].x, b[0].y), (100, 0));
        assert_eq!((b[1].x, b[1].y), (200, 0));
        assert_eq!((b[2].x, b[2].y), (150, 100));
    }

    /// A point-matching component that references a point index past the
    /// real point set (a phantom-point reference we can't resolve without
    /// metrics) falls back to zero-offset placement rather than dropping
    /// the component.
    #[test]
    fn point_matching_out_of_range_falls_back_to_zero_offset() {
        let triangle = build_triangle();
        let mut composite = Vec::new();
        composite.extend_from_slice(&(-1i16).to_be_bytes());
        for _ in 0..4 {
            composite.extend_from_slice(&0i16.to_be_bytes());
        }
        // Single point-matching component referencing parent point 0 — but
        // the parent is empty (no prior component), so point 0 is
        // out-of-range and we fall back to (0,0).
        let flags = 0u16;
        composite.extend_from_slice(&flags.to_be_bytes());
        composite.extend_from_slice(&0u16.to_be_bytes());
        composite.push(0); // arg1 = parent point 0 (none exist yet)
        composite.push(0); // arg2 = child point 0

        let glyf_bytes: Vec<u8> = [triangle.as_slice(), composite.as_slice()].concat();
        let tri_len = triangle.len() as u32;
        let total = glyf_bytes.len() as u32;
        let mut loca_bytes = Vec::new();
        for v in [0u32, tri_len, total] {
            loca_bytes.extend_from_slice(&v.to_be_bytes());
        }
        let loca = LocaTable::parse(&loca_bytes, 2, 1).unwrap();
        let glyf = GlyfTable::new(&glyf_bytes);
        let out = glyf
            .glyph_outline(tri_len as usize..total as usize, &loca, 0)
            .unwrap();
        assert_eq!(out.contours.len(), 1);
        let p = &out.contours[0].points;
        assert_eq!((p[0].x, p[0].y), (0, 0));
        assert_eq!((p[1].x, p[1].y), (100, 0));
    }

    #[test]
    fn composite_self_cycle_terminates_with_composite_too_deep() {
        // Two glyphs:
        //   glyph 0 = triangle (innocent bystander, used only so loca
        //             has a valid first entry; the self-cycling glyph
        //             below is glyph 1).
        //   glyph 1 = composite referencing glyph 1 (itself).
        let triangle = build_triangle();
        let self_cycle = build_composite_referencing(1);

        let mut glyf = triangle.clone();
        let tri_len = glyf.len() as u32;
        glyf.extend_from_slice(&self_cycle);
        let total = glyf.len() as u32;

        let mut loca_bytes = Vec::new();
        for v in [0u32, tri_len, total] {
            loca_bytes.extend_from_slice(&v.to_be_bytes());
        }
        let loca = LocaTable::parse(&loca_bytes, 2, 1).unwrap();
        let glyf_t = GlyfTable::new(&glyf);

        let err = glyf_t
            .glyph_outline(tri_len as usize..total as usize, &loca, 0)
            .expect_err("self-cycle must reject, not stack-overflow");
        assert_eq!(err, Error::CompositeTooDeep);
    }
}
