//! TrueType outline data structures and helpers.
//!
//! A TrueType outline is a list of *contours*, each a closed loop of
//! quadratic-Bezier control points. Each `Point` is flagged on-curve or
//! off-curve. Off-curve points are control handles for quadratic Beziers;
//! consecutive off-curve points imply an implicit on-curve midpoint per
//! the standard reconstruction rule (see the rasterizer crate, not here).
//!
//! Composite glyphs reference other glyphs and apply a 2x2 transform +
//! offset. `apply_affine` does the affine math; the recursive composite
//! resolver lives in `tables::glyf`.

/// A single point in a TrueType outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: i16,
    pub y: i16,
    pub on_curve: bool,
}

/// A closed contour — a sequence of (mostly alternating) on-curve and
/// off-curve points. The contour closes implicitly between the last and
/// first point.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contour {
    pub points: Vec<Point>,
}

/// Glyph bounding box in font units (matches `glyf` header semantics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BBox {
    pub x_min: i16,
    pub y_min: i16,
    pub x_max: i16,
    pub y_max: i16,
}

impl BBox {
    pub fn width(&self) -> i32 {
        self.x_max as i32 - self.x_min as i32
    }
    pub fn height(&self) -> i32 {
        self.y_max as i32 - self.y_min as i32
    }
}

/// A decoded TrueType glyph outline.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TtOutline {
    pub contours: Vec<Contour>,
    pub bounds: Option<BBox>,
}

impl TtOutline {
    pub fn is_empty(&self) -> bool {
        self.contours.is_empty()
    }

    /// Append all contours from `other`, applying a 2x2 affine transform
    /// `(xx, xy, yx, yy)` plus integer offset `(dx, dy)`. This is the
    /// composite-glyph component pipeline: spec field names
    /// `xScale, scale01, scale10, yScale` plus argument1/argument2.
    ///
    /// All math uses i32 internally then saturates back to i16; this is
    /// the same behaviour the reference TrueType implementations document
    /// (and matches what we expect downstream rasterizers to consume).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn append_transformed(
        &mut self,
        other: &TtOutline,
        xx: f32,
        xy: f32,
        yx: f32,
        yy: f32,
        dx: i32,
        dy: i32,
    ) {
        for c in &other.contours {
            let mut nc = Contour {
                points: Vec::with_capacity(c.points.len()),
            };
            for p in &c.points {
                let px = p.x as f32;
                let py = p.y as f32;
                let nx = (px * xx + py * yx).round() as i32 + dx;
                let ny = (px * xy + py * yy).round() as i32 + dy;
                nc.points.push(Point {
                    x: clamp_i16(nx),
                    y: clamp_i16(ny),
                    on_curve: p.on_curve,
                });
            }
            self.contours.push(nc);
        }
        // Re-derive bounds from the merged point set.
        self.bounds = derive_bbox(&self.contours);
    }
}

fn clamp_i16(v: i32) -> i16 {
    if v < i16::MIN as i32 {
        i16::MIN
    } else if v > i16::MAX as i32 {
        i16::MAX
    } else {
        v as i16
    }
}

pub(crate) fn derive_bbox(contours: &[Contour]) -> Option<BBox> {
    let mut x_min = i16::MAX;
    let mut y_min = i16::MAX;
    let mut x_max = i16::MIN;
    let mut y_max = i16::MIN;
    let mut any = false;
    for c in contours {
        for p in &c.points {
            any = true;
            if p.x < x_min {
                x_min = p.x;
            }
            if p.x > x_max {
                x_max = p.x;
            }
            if p.y < y_min {
                y_min = p.y;
            }
            if p.y > y_max {
                y_max = p.y;
            }
        }
    }
    if any {
        Some(BBox {
            x_min,
            y_min,
            x_max,
            y_max,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_outline_has_no_bounds() {
        let o = TtOutline::default();
        assert!(o.is_empty());
        assert!(o.bounds.is_none());
    }

    #[test]
    fn affine_translates_points() {
        let src = TtOutline {
            contours: vec![Contour {
                points: vec![
                    Point {
                        x: 10,
                        y: 20,
                        on_curve: true,
                    },
                    Point {
                        x: 30,
                        y: 40,
                        on_curve: false,
                    },
                ],
            }],
            bounds: Some(BBox {
                x_min: 10,
                y_min: 20,
                x_max: 30,
                y_max: 40,
            }),
        };
        let mut dst = TtOutline::default();
        dst.append_transformed(&src, 1.0, 0.0, 0.0, 1.0, 100, 200);
        let p0 = dst.contours[0].points[0];
        assert_eq!((p0.x, p0.y), (110, 220));
        let p1 = dst.contours[0].points[1];
        assert_eq!((p1.x, p1.y), (130, 240));
    }
}
