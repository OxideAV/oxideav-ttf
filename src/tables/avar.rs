//! `avar` — Axis Variations Table.
//!
//! Spec: Microsoft OpenType §"avar — Axis Variations Table" / OpenType
//! 1.9. Apple TrueType Reference §"avar".
//!
//! After the caller picks a coordinate vector against the `fvar` axes
//! (in user-space units), the per-axis values are first **normalised**
//! into the closed interval `[-1.0, +1.0]`:
//!
//! ```text
//!   if v == default:   normalised = 0.0
//!   if v <  default:   normalised = (v - default) / (default - min)   (in [-1, 0])
//!   if v >  default:   normalised = (v - default) / (max - default)   (in (0, 1])
//! ```
//!
//! `avar` then optionally bends the normalised value via a
//! per-axis **piecewise-linear segment map**. Each segment list is a
//! sorted (ascending by `from`) sequence of `(from, to)` pairs in
//! F2DOT14 (`i16 / 16384.0`). The map is the identity at -1, 0, +1
//! (the spec requires those three anchors to be present in any
//! non-empty list — we do not enforce). For a normalised input `n`,
//! find the segment `[fromₖ, fromₖ₊₁]` containing `n` and linearly
//! interpolate between `toₖ` and `toₖ₊₁`. An axis with **zero**
//! segments leaves the normalised value unchanged.
//!
//! Header layout:
//!
//! ```text
//!   0  / 2  / majorVersion             (1)
//!   2  / 2  / minorVersion             (0)
//!   4  / 2  / (reserved, must be 0)
//!   6  / 2  / axisCount
//!   8  / .. / per-axis SegmentMaps[axisCount]
//! ```
//!
//! Each `SegmentMaps` block:
//!
//! ```text
//!   0 / 2 / positionMapCount
//!   2 / 4*positionMapCount / AxisValueMap{ fromCoord: F2DOT14, toCoord: F2DOT14 }
//! ```
//!
//! avar 2.0 (variable-axis remap with a per-glyph delta-set index map)
//! is out of scope for this round; we accept the version-1 header and
//! fall back to identity if the major version is anything other than 1.

use crate::parser::{read_i16, read_u16};
use crate::Error;

/// Sanity cap on the per-axis segment-map length. Real fonts top out
/// in single digits; the cap exists purely to bound parse cost.
const MAX_SEGMENTS: u16 = 256;

/// Parsed avar table — one piecewise-linear segment list per axis.
#[derive(Debug, Clone, Default)]
pub struct AvarTable {
    /// `segments[axis_index]` is the sorted-ascending `(from, to)`
    /// list. Empty list = identity remap for that axis.
    segments: Vec<Vec<(f32, f32)>>,
}

impl AvarTable {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 8 {
            return Err(Error::UnexpectedEof);
        }
        let major = read_u16(bytes, 0)?;
        if major != 1 {
            // avar 2.0 carries a delta-set index map after the segment
            // arrays. We don't decode the v2 extras (they only matter
            // for variable-axis remap, which we don't apply); fall
            // back to identity for the whole table.
            return Ok(Self::default());
        }
        // bytes[2..4] minor — ignored.
        // bytes[4..6] reserved — ignored.
        let axis_count = read_u16(bytes, 6)?;
        let mut off = 8usize;
        let mut segments = Vec::with_capacity(axis_count as usize);
        for _ in 0..axis_count {
            if off + 2 > bytes.len() {
                return Err(Error::UnexpectedEof);
            }
            let n = read_u16(bytes, off)?;
            off += 2;
            if n > MAX_SEGMENTS {
                return Err(Error::BadStructure("avar segment count exceeds cap"));
            }
            let need = (n as usize).checked_mul(4).ok_or(Error::BadOffset)?;
            if off + need > bytes.len() {
                return Err(Error::UnexpectedEof);
            }
            let mut list = Vec::with_capacity(n as usize);
            let mut prev_from = f32::NEG_INFINITY;
            for _ in 0..n {
                let from = f2dot14(read_i16(bytes, off)?);
                let to = f2dot14(read_i16(bytes, off + 2)?);
                off += 4;
                // Spec requires fromCoord strictly increasing; tolerate
                // equal entries by simply ignoring the disorder rather
                // than rejecting the whole font.
                if from < prev_from {
                    return Err(Error::BadStructure("avar fromCoord not ascending"));
                }
                prev_from = from;
                list.push((from, to));
            }
            segments.push(list);
        }
        Ok(Self { segments })
    }

    /// Apply this avar table's remap for `axis_index` to a normalised
    /// value `n` (in `[-1.0, +1.0]`). Out-of-range axes or empty
    /// segment lists return `n` unchanged.
    pub fn remap_normalised(&self, axis_index: usize, n: f32) -> f32 {
        let n = n.clamp(-1.0, 1.0);
        let segs = match self.segments.get(axis_index) {
            Some(s) if !s.is_empty() => s,
            _ => return n,
        };
        // If `n` is at or below the first anchor, snap to it; if at or
        // above the last, snap to it. Otherwise locate the segment
        // containing `n` and interpolate.
        if n <= segs[0].0 {
            return segs[0].1;
        }
        if n >= segs[segs.len() - 1].0 {
            return segs[segs.len() - 1].1;
        }
        for w in segs.windows(2) {
            let (f0, t0) = w[0];
            let (f1, t1) = w[1];
            if n >= f0 && n <= f1 {
                if (f1 - f0).abs() < f32::EPSILON {
                    return t0;
                }
                let alpha = (n - f0) / (f1 - f0);
                return t0 + alpha * (t1 - t0);
            }
        }
        // Should be unreachable given the bracket above; safe default.
        n
    }

    pub fn axis_count(&self) -> usize {
        self.segments.len()
    }
}

#[inline]
fn f2dot14(raw: i16) -> f32 {
    raw as f32 / 16384.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a v1 avar header with `axis_count` axes, each empty.
    fn build_empty(axis_count: u16) -> Vec<u8> {
        let mut b = vec![0u8; 8 + (axis_count as usize) * 2];
        b[0..2].copy_from_slice(&1u16.to_be_bytes()); // major
        b[6..8].copy_from_slice(&axis_count.to_be_bytes());
        // each axis: positionMapCount = 0
        // (already zero from vec! init)
        b
    }

    #[test]
    fn avar_remap_identity_when_no_segments() {
        let raw = build_empty(2);
        let a = AvarTable::parse(&raw).expect("parse");
        for &v in &[-1.0f32, -0.5, 0.0, 0.25, 1.0] {
            assert_eq!(a.remap_normalised(0, v), v);
            assert_eq!(a.remap_normalised(1, v), v);
        }
        // Out-of-axis-range request: identity (clamped).
        assert_eq!(a.remap_normalised(99, 0.5), 0.5);
    }

    #[test]
    fn avar_remap_identity_segments() {
        // axis 0: 3 segments at -1/0/+1 with identity mapping
        let mut b = vec![0u8; 8 + 2 + 12];
        b[0..2].copy_from_slice(&1u16.to_be_bytes());
        b[6..8].copy_from_slice(&1u16.to_be_bytes());
        b[8..10].copy_from_slice(&3u16.to_be_bytes());
        // (-1, -1)  (0, 0)  (+1, +1) — F2DOT14
        for (i, &v) in [-16384i16, -16384, 0, 0, 16384, 16384].iter().enumerate() {
            let off = 10 + i * 2;
            b[off..off + 2].copy_from_slice(&v.to_be_bytes());
        }
        let a = AvarTable::parse(&b).unwrap();
        for &v in &[-1.0f32, -0.5, 0.0, 0.25, 1.0] {
            assert!((a.remap_normalised(0, v) - v).abs() < 1e-6);
        }
    }

    #[test]
    fn avar_remap_piecewise_linear() {
        // axis 0: -1→-1, 0→0, +0.5→+0.25, +1→+1
        let mut b = vec![0u8; 8 + 2 + 16];
        b[0..2].copy_from_slice(&1u16.to_be_bytes());
        b[6..8].copy_from_slice(&1u16.to_be_bytes());
        b[8..10].copy_from_slice(&4u16.to_be_bytes());
        let pairs: [(i16, i16); 4] = [
            (-16384, -16384),
            (0, 0),
            (16384 / 2, 16384 / 4),
            (16384, 16384),
        ];
        for (i, (f, t)) in pairs.iter().enumerate() {
            let off = 10 + i * 4;
            b[off..off + 2].copy_from_slice(&f.to_be_bytes());
            b[off + 2..off + 4].copy_from_slice(&t.to_be_bytes());
        }
        let a = AvarTable::parse(&b).unwrap();
        // At 0.0 → 0.0
        assert!(a.remap_normalised(0, 0.0).abs() < 1e-6);
        // At 0.25 (mid of 0..0.5) → mid of 0..0.25 = 0.125
        assert!((a.remap_normalised(0, 0.25) - 0.125).abs() < 1e-4);
        // At 0.75 (mid of 0.5..1.0) → mid of 0.25..1.0 = 0.625
        assert!((a.remap_normalised(0, 0.75) - 0.625).abs() < 1e-4);
        // At 1.0 → 1.0
        assert!((a.remap_normalised(0, 1.0) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn avar_v2_falls_back_to_identity() {
        let mut b = vec![0u8; 8];
        b[0..2].copy_from_slice(&2u16.to_be_bytes()); // major = 2
        let a = AvarTable::parse(&b).expect("parse");
        // Identity for any axis index since we don't know how many.
        assert_eq!(a.remap_normalised(0, 0.5), 0.5);
    }
}
