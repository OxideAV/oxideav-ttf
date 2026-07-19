//! `avar` — Axis Variations Table (versions 1 and 2).
//!
//! Spec: Microsoft OpenType §"avar — Axis Variations Table" (v1) and
//! the staged avar version-2 reference (`otspec-avar-v2.md` — the
//! working specification behind the OFF amendment; v2 is a strict
//! superset of v1).
//!
//! After the caller picks a coordinate vector against the `fvar` axes
//! (in user-space units), the per-axis values are resolved in three
//! stages (staged v2 reference §4):
//!
//! 1. **Initial normalisation** — each axis's user coordinate maps
//!    into `[-1.0, +1.0]`:
//!
//!    ```text
//!      if v == default:   normalised = 0.0
//!      if v <  default:   normalised = (v - default) / (default - min)   (in [-1, 0])
//!      if v >  default:   normalised = (v - default) / (max - default)   (in (0, 1])
//!    ```
//!
//! 2. **v1 segment-map remap** — each axis is bent independently
//!    through a piecewise-linear segment map: a sorted (ascending by
//!    `from`) sequence of `(from, to)` pairs in F2DOT14
//!    (`i16 / 16384.0`). The map is the identity at -1, 0, +1 (the
//!    spec requires those three anchors in any non-empty list — we do
//!    not enforce). For a normalised input `n`, find the segment
//!    `[fromₖ, fromₖ₊₁]` containing `n` and linearly interpolate
//!    between `toₖ` and `toₖ₊₁`. An axis with **zero** segments leaves
//!    the value unchanged.
//!
//! 3. **v2 cross-axis delta application** — version 2 appends an
//!    `axisIndexMap` (`DeltaSetIndexMap`) + `varStore`
//!    (`ItemVariationStore`) pair after the segment maps. Using the
//!    stage-2 *intermediate* vector, a per-axis interpolated delta is
//!    computed from the store (deltas are integers in F2DOT14 units —
//!    1.0 is stored as 16384), rounded, added to the axis's F2DOT14
//!    coordinate, and the result clamped to `[-1.0, +1.0]`. The axis's
//!    delta-set index is `axisIndexMap[i]` (clamping to the last entry
//!    when out of range) or the identity split `outer = i >> 16`,
//!    `inner = i & 0xFFFF` when the map is absent. The set of axes
//!    adjusting an axis may include the axis itself.
//!
//! Header layout (v1; v2 is identical with `majorVersion = 2` and two
//! trailing Offset32 fields after the segment-map array):
//!
//! ```text
//!   0  / 2  / majorVersion             (1 or 2)
//!   2  / 2  / minorVersion             (0)
//!   4  / 2  / (reserved, must be 0)
//!   6  / 2  / axisCount                (v2: axisSegmentMapCount, may be 0)
//!   8  / .. / per-axis SegmentMaps[axisCount]
//!   .. / 4  / axisIndexMapOffset       (v2 only; may be 0)
//!   .. / 4  / varStoreOffset           (v2 only; may be 0)
//! ```
//!
//! Each `SegmentMaps` block:
//!
//! ```text
//!   0 / 2 / positionMapCount
//!   2 / 4*positionMapCount / AxisValueMap{ fromCoord: F2DOT14, toCoord: F2DOT14 }
//! ```
//!
//! The `DeltaSetIndexMap` decoder implements the staged ISO/IEC
//! 14496-22:2019 §7.3.5.2 layout (byte-identical to the OpenType 1.9
//! "format 0" map); a format-1 map — defined only in the unstaged
//! `otvarcommonformats` chapter — is flagged via
//! [`AvarTable::axis_index_map_unsupported`] and stage 3 is skipped
//! for the whole table (v1 segment maps still apply).

use crate::parser::{read_i16, read_u16, read_u32};
use crate::tables::hvar::DeltaSetIndexMap;
use crate::tables::mvar::ItemVariationStore;
use crate::Error;

/// Sanity cap on the per-axis segment-map length. Real fonts top out
/// in single digits; the cap exists purely to bound parse cost.
const MAX_SEGMENTS: u16 = 256;

/// Parsed avar table — one piecewise-linear segment list per axis,
/// plus the optional version-2 cross-axis delta mapping.
#[derive(Debug, Clone, Default)]
pub struct AvarTable {
    /// `segments[axis_index]` is the sorted-ascending `(from, to)`
    /// list. Empty list = identity remap for that axis.
    segments: Vec<Vec<(f32, f32)>>,
    /// v2 `axisIndexMap`: fvar axis index → variation-store delta-set
    /// index. `None` = identity mapping.
    axis_index_map: Option<DeltaSetIndexMap>,
    /// An `axisIndexMap` was present but not decodable against the
    /// staged layout (an OpenType 1.9 format-1 map); stage 3 is
    /// skipped entirely.
    axis_index_map_unsupported: bool,
    /// v2 `varStore`: the per-axis delta sets, in F2DOT14 units.
    var_store: Option<ItemVariationStore>,
}

impl AvarTable {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 8 {
            return Err(Error::UnexpectedEof);
        }
        let major = read_u16(bytes, 0)?;
        if major != 1 && major != 2 {
            // Unknown future major version: fall back to identity for
            // the whole table (the v1-only fallback behaviour the
            // staged v2 reference §5 describes, one version up).
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

        let mut table = Self {
            segments,
            ..Self::default()
        };

        if major == 2 {
            // The two v2 offsets trail the segment-map array; both are
            // measured from the start of the table and either may be 0.
            if off + 8 > bytes.len() {
                return Err(Error::UnexpectedEof);
            }
            let axis_index_map_off = read_u32(bytes, off)? as usize;
            let var_store_off = read_u32(bytes, off + 4)? as usize;
            if axis_index_map_off != 0 {
                if axis_index_map_off >= bytes.len() {
                    return Err(Error::BadOffset);
                }
                // Format-0 maps decode through the shared staged-layout
                // parser; a 1.9 format-1 map trips its reserved-bit
                // check → degrade to v1-only behaviour and flag it.
                match DeltaSetIndexMap::parse(&bytes[axis_index_map_off..]) {
                    Ok(map) => table.axis_index_map = Some(map),
                    Err(_) => table.axis_index_map_unsupported = true,
                }
            }
            if var_store_off != 0 {
                if var_store_off >= bytes.len() {
                    return Err(Error::BadOffset);
                }
                table.var_store = Some(ItemVariationStore::parse(&bytes[var_store_off..])?);
            }
        }
        Ok(table)
    }

    /// Apply this avar table's **v1 segment-map** remap for
    /// `axis_index` to a normalised value `n` (in `[-1.0, +1.0]`).
    /// Out-of-range axes or empty segment lists return `n` unchanged.
    /// This is stage 2 only — use [`Self::remap_vector`] for the full
    /// v2 pipeline (stage 3 needs the whole vector at once).
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

    /// Run stages 2 and 3 on an initially-normalised coordinate
    /// vector: per-axis v1 segment-map bending, then — for a version-2
    /// table — the cross-axis delta application per the staged v2
    /// reference §4: region scalars are computed against the
    /// **intermediate** (stage-2) vector, each axis's interpolated
    /// F2DOT14-unit delta is rounded and added, and the result is
    /// clamped to `[-1.0, +1.0]`. For a v1 table (or a v2 table with
    /// no `varStore`) this equals the per-axis
    /// [`Self::remap_normalised`] results.
    pub fn remap_vector(&self, initial: &[f32]) -> Vec<f32> {
        // Stage 2.
        let intermediate: Vec<f32> = initial
            .iter()
            .enumerate()
            .map(|(i, &n)| self.remap_normalised(i, n))
            .collect();
        // Stage 3.
        let Some(store) = self.var_store.as_ref() else {
            return intermediate;
        };
        if self.axis_index_map_unsupported {
            return intermediate;
        }
        intermediate
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let (outer, inner) = match self.axis_index_map.as_ref() {
                    Some(map) if !map.is_empty() => {
                        let entries = map.entries();
                        entries[i.min(entries.len() - 1)]
                    }
                    // Identity: outer = i >> 16 (0 for any real fvar
                    // axis count), inner = i.
                    _ => ((i >> 16) as u16, (i & 0xFFFF) as u16),
                };
                // The delta is in F2DOT14 units; work the sum in that
                // integer-scaled space per the reference algorithm
                // (`v += roundf(delta)`), then clamp to ±1.0.
                let delta = store.delta(outer, inner, &intermediate).unwrap_or(0.0);
                ((v * 16384.0 + delta.round()).clamp(-16384.0, 16384.0)) / 16384.0
            })
            .collect()
    }

    /// `true` when this is a version-2 table with a variation store —
    /// i.e. stage 3 can move coordinates across axes.
    pub fn has_cross_axis_mapping(&self) -> bool {
        self.var_store.is_some() && !self.axis_index_map_unsupported
    }

    /// An `axisIndexMap` was present but uses the OpenType 1.9
    /// "format 1" layout, which is outside the staged spec chapters:
    /// stage 3 is skipped for the whole table (v1 segment maps still
    /// apply).
    pub fn axis_index_map_unsupported(&self) -> bool {
        self.axis_index_map_unsupported
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

    /// Append a two-axis IVS with one region and one IVD whose rows
    /// are the given F2DOT14-unit deltas. The region peaks at +1 on
    /// `peak_axis` (the other axis does not factor).
    fn push_two_axis_ivs(b: &mut Vec<u8>, peak_axis: usize, rows: &[i16]) -> u32 {
        let ivs = b.len() as u32;
        b.extend_from_slice(&1u16.to_be_bytes()); // format
        b.extend_from_slice(&12u32.to_be_bytes()); // regionListOffset
        b.extend_from_slice(&1u16.to_be_bytes()); // ivdCount
        b.extend_from_slice(&28u32.to_be_bytes()); // ivdOffsets[0] = 12 + 4 + 2*6
        b.extend_from_slice(&2u16.to_be_bytes()); // axisCount
        b.extend_from_slice(&1u16.to_be_bytes()); // regionCount
        for a in 0..2usize {
            let peak: i16 = if a == peak_axis { 16384 } else { 0 };
            b.extend_from_slice(&0i16.to_be_bytes());
            b.extend_from_slice(&peak.to_be_bytes());
            b.extend_from_slice(&peak.to_be_bytes());
        }
        b.extend_from_slice(&(rows.len() as u16).to_be_bytes()); // itemCount
        b.extend_from_slice(&1u16.to_be_bytes()); // shortDeltaCount
        b.extend_from_slice(&1u16.to_be_bytes()); // regionIndexCount
        b.extend_from_slice(&0u16.to_be_bytes()); // regionIndexes[0]
        for &d in rows {
            b.extend_from_slice(&d.to_be_bytes());
        }
        ivs
    }

    /// Build a v2 header for two axes with empty segment maps and the
    /// given IVS rows; `map` optionally supplies raw axisIndexMap
    /// bytes.
    fn build_v2(peak_axis: usize, rows: &[i16], map: Option<&[u8]>) -> Vec<u8> {
        let mut b = vec![0u8; 8];
        b[0..2].copy_from_slice(&2u16.to_be_bytes()); // major = 2
        b[6..8].copy_from_slice(&2u16.to_be_bytes()); // axisSegmentMapCount
        b.extend_from_slice(&0u16.to_be_bytes()); // axis 0: 0 segments
        b.extend_from_slice(&0u16.to_be_bytes()); // axis 1: 0 segments
        let map_slot = b.len();
        b.extend_from_slice(&0u32.to_be_bytes()); // axisIndexMapOffset
        let store_slot = b.len();
        b.extend_from_slice(&0u32.to_be_bytes()); // varStoreOffset
        if let Some(map_bytes) = map {
            let off = b.len() as u32;
            b.extend_from_slice(map_bytes);
            b[map_slot..map_slot + 4].copy_from_slice(&off.to_be_bytes());
        }
        let ivs = push_two_axis_ivs(&mut b, peak_axis, rows);
        b[store_slot..store_slot + 4].copy_from_slice(&ivs.to_be_bytes());
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
        // The vector form matches per-axis for a v1 table.
        assert_eq!(a.remap_vector(&[0.25, -0.5]), vec![0.25, -0.5]);
        assert!(!a.has_cross_axis_mapping());
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
    fn avar_unknown_major_falls_back_to_identity() {
        let mut b = vec![0u8; 8];
        b[0..2].copy_from_slice(&3u16.to_be_bytes()); // major = 3
        let a = AvarTable::parse(&b).expect("parse");
        // Identity for any axis index since we don't know how many.
        assert_eq!(a.remap_normalised(0, 0.5), 0.5);
        assert_eq!(a.remap_vector(&[0.5, -0.25]), vec![0.5, -0.25]);
    }

    #[test]
    fn avar_v2_cross_axis_delta() {
        // Axis 1's coordinate drives a delta on axis 0: the region
        // peaks on axis 1; delta rows (identity map: inner = axis
        // index) are axis 0 → −8192 (−0.5), axis 1 → 0.
        let b = build_v2(1, &[-8192, 0], None);
        let a = AvarTable::parse(&b).expect("parse");
        assert!(a.has_cross_axis_mapping());
        assert!(!a.axis_index_map_unsupported());
        // Axis 1 at 0 → no scalar → both unchanged.
        let out = a.remap_vector(&[0.5, 0.0]);
        assert!((out[0] - 0.5).abs() < 1e-6 && out[1].abs() < 1e-6);
        // Axis 1 at +1 → axis 0 shifts by −0.5; axis 1 unchanged.
        let out = a.remap_vector(&[0.5, 1.0]);
        assert!((out[0] - 0.0).abs() < 1e-6, "{out:?}");
        assert!((out[1] - 1.0).abs() < 1e-6);
        // Axis 1 at +0.5 → half the delta (−0.25), rounded in F2DOT14
        // integer space.
        let out = a.remap_vector(&[0.5, 0.5]);
        assert!((out[0] - 0.25).abs() < 1e-4, "{out:?}");
    }

    #[test]
    fn avar_v2_self_reference_and_clamp() {
        // The region peaks on axis 0 and its delta row pushes axis 0
        // itself by +1.0 — the sum clamps at +1.0 (staged reference:
        // clamp to ±16384 F2DOT14 after summing).
        let b = build_v2(0, &[16384, 0], None);
        let a = AvarTable::parse(&b).expect("parse");
        let out = a.remap_vector(&[0.5, 0.0]);
        // Scalar at 0.5 on the rising edge = 0.5 → delta = +0.5;
        // 0.5 + 0.5 = 1.0 (no clamp needed).
        assert!((out[0] - 1.0).abs() < 1e-6, "{out:?}");
        // At 1.0 the delta is +1.0 → 2.0 clamps to 1.0.
        let out = a.remap_vector(&[1.0, 0.0]);
        assert!((out[0] - 1.0).abs() < 1e-6, "{out:?}");
    }

    #[test]
    fn avar_v2_stage3_uses_stage2_intermediate_coords() {
        // Axis 1 carries a v1 segment map bending +1 → +0.5, and the
        // stage-3 region peaks on axis 1. The scalar must be computed
        // from the *bent* 0.5 (→ 0.5 of the −0.5 delta on axis 0),
        // not the initial 1.0 (which would apply the full delta).
        let mut b = vec![0u8; 8];
        b[0..2].copy_from_slice(&2u16.to_be_bytes());
        b[6..8].copy_from_slice(&2u16.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes()); // axis 0: no segments
        b.extend_from_slice(&3u16.to_be_bytes()); // axis 1: 3 segments
        for (f, t) in [(-16384i16, -16384i16), (0, 0), (16384, 8192)] {
            b.extend_from_slice(&f.to_be_bytes());
            b.extend_from_slice(&t.to_be_bytes());
        }
        let map_slot = b.len();
        b.extend_from_slice(&0u32.to_be_bytes());
        let store_slot = b.len();
        b.extend_from_slice(&0u32.to_be_bytes());
        let _ = map_slot; // identity mapping
        let ivs = push_two_axis_ivs(&mut b, 1, &[-8192, 0]);
        b[store_slot..store_slot + 4].copy_from_slice(&ivs.to_be_bytes());

        let a = AvarTable::parse(&b).expect("parse");
        let out = a.remap_vector(&[0.5, 1.0]);
        // Axis 1: 1.0 bends to 0.5 in stage 2. Axis 0: scalar 0.5 →
        // delta −0.25 → 0.25.
        assert!((out[1] - 0.5).abs() < 1e-6, "{out:?}");
        assert!((out[0] - 0.25).abs() < 1e-4, "{out:?}");
    }

    #[test]
    fn avar_v2_axis_index_map_routes_and_clamps() {
        // Map with a single entry (outer 0, inner 1): both axes route
        // to delta row 1 (axis 1's index clamps to the last = only
        // entry). Rows: inner 0 = +8192 (unreachable), inner 1 =
        // −4096 (−0.25).
        let map: Vec<u8> = {
            let mut m = Vec::new();
            m.extend_from_slice(&0x003Fu16.to_be_bytes()); // 4-byte entries, 16 inner bits
            m.extend_from_slice(&1u16.to_be_bytes()); // mapCount
            m.extend_from_slice(&1u32.to_be_bytes()); // (0, 1)
            m
        };
        let b = build_v2(1, &[8192, -4096], Some(&map));
        let a = AvarTable::parse(&b).expect("parse");
        let out = a.remap_vector(&[0.0, 1.0]);
        // Both axes get row 1's −0.25 at full scalar.
        assert!((out[0] - (-0.25)).abs() < 1e-4, "{out:?}");
        assert!((out[1] - 0.75).abs() < 1e-4, "{out:?}");
    }

    #[test]
    fn avar_v2_format1_axis_index_map_degrades_to_v1() {
        // A 1.9 format-1 DeltaSetIndexMap (leading 0x01 format byte)
        // is outside the staged layouts: stage 3 is skipped, the
        // degradation is flagged, and stage 2 still applies.
        let map: Vec<u8> = vec![0x01, 0x00, 0, 0, 0, 1, 0, 0, 0, 0];
        let b = build_v2(1, &[-8192, 0], Some(&map));
        let a = AvarTable::parse(&b).expect("parse");
        assert!(a.axis_index_map_unsupported());
        assert!(!a.has_cross_axis_mapping());
        let out = a.remap_vector(&[0.5, 1.0]);
        assert_eq!(out, vec![0.5, 1.0], "no stage-3 movement");
    }

    #[test]
    fn avar_v2_without_store_is_stage2_only() {
        // v2 header, no varStore, no axisIndexMap.
        let mut b = vec![0u8; 8];
        b[0..2].copy_from_slice(&2u16.to_be_bytes());
        b[6..8].copy_from_slice(&0u16.to_be_bytes()); // no segment maps
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes());
        let a = AvarTable::parse(&b).expect("parse");
        assert!(!a.has_cross_axis_mapping());
        assert_eq!(a.remap_vector(&[0.5, -1.0]), vec![0.5, -1.0]);
    }

    #[test]
    fn avar_v2_truncated_offsets_rejected() {
        // major 2 with segment maps but the trailing offsets missing.
        let mut b = vec![0u8; 8 + 2];
        b[0..2].copy_from_slice(&2u16.to_be_bytes());
        b[6..8].copy_from_slice(&1u16.to_be_bytes());
        // axis 0: positionMapCount 0 (bytes 8..10 zero) — then EOF.
        assert!(matches!(AvarTable::parse(&b), Err(Error::UnexpectedEof)));
    }
}
