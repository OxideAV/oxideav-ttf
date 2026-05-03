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
// 0x0200 USE_MY_METRICS, 0x0400 OVERLAP_COMPOUND, 0x0800 SCALED_COMPONENT_OFFSET
// 0x1000 UNSCALED_COMPONENT_OFFSET — none affect outline geometry.

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

            // Recurse — only the XY-offset variant is supported. The
            // alternate "match-points" variant (when ARGS_ARE_XY_VALUES
            // is *cleared*) requires resolving an existing point in the
            // accumulated outline; we treat it as zero-offset (best-effort
            // for round 1).
            let child_range = loca.glyph_range(glyph_index)?;
            let child = self.glyph_outline(child_range, loca, depth + 1)?;
            let (dx, dy) = if flags & C_ARGS_ARE_XY_VALUES != 0 {
                (arg1, arg2)
            } else {
                (0, 0)
            };
            out.append_transformed(&child, xx, xy, yx, yy, dx, dy);

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
}
