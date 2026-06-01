//! `MVAR` — Metrics Variations Table.
//!
//! Spec: ISO/IEC 14496-22:2019 §7.3.6 ("MVAR – Metrics variations
//! table"). The shared `ItemVariationStore` substructure used by MVAR
//! (and by HVAR / VVAR / GDEF) is specified in §7.2.3.
//!
//! MVAR provides per-instance adjustments for font-wide metric values
//! (e.g. `OS/2.sxHeight`, `OS/2.sCapHeight`, `hhea.caretSlopeRise`)
//! in a variable font. Each adjustment is keyed by a four-byte tag
//! ('xhgt', 'cpht', 'unds', …) and resolved through an embedded
//! `ItemVariationStore` whose region-scaled deltas are summed against
//! the current normalised coordinate vector.
//!
//! ## Header layout
//!
//! ```text
//!   0  / 2 / majorVersion          (== 1)
//!   2  / 2 / minorVersion          (== 0)
//!   4  / 2 / (reserved, set to 0)
//!   6  / 2 / valueRecordSize       (== 8 in v1; future minor versions
//!                                   may grow it — we use this for the
//!                                   record stride)
//!   8  / 2 / valueRecordCount
//!  10  / 2 / itemVariationStoreOffset  (relative to MVAR start; zero
//!                                       allowed only when count == 0)
//!  12  / .. / valueRecords[valueRecordCount]
//! ```
//!
//! Each `ValueRecord` (`valueRecordSize` bytes, minimum 8):
//!
//! ```text
//!   0 / 4 / valueTag                       (e.g. b"xhgt")
//!   4 / 2 / deltaSetOuterIndex             (into IVS subtable array)
//!   6 / 2 / deltaSetInnerIndex             (into the delta-set row)
//! ```
//!
//! Records MUST be sorted in binary order of `valueTag`; we trust the
//! ordering for binary-search lookup but do not enforce it.
//!
//! ## ItemVariationStore (§7.2.3)
//!
//! ```text
//!   0 / 2 / format                          (== 1)
//!   2 / 4 / variationRegionListOffset       (relative to IVS start)
//!   6 / 2 / itemVariationDataCount
//!   8 / 4*N / itemVariationDataOffsets[N]   (each relative to IVS start)
//! ```
//!
//! The VariationRegionList:
//!
//! ```text
//!   0 / 2 / axisCount                       (== fvar.axisCount)
//!   2 / 2 / regionCount
//!   4 / 6*axisCount*regionCount / regions   (F2DOT14 start/peak/end
//!                                            per axis, per region)
//! ```
//!
//! Each `ItemVariationData` subtable:
//!
//! ```text
//!   0 / 2 / itemCount                       (delta-set rows)
//!   2 / 2 / shortDeltaCount                 (cols using int16; rest int8)
//!   4 / 2 / regionIndexCount                (cols)
//!   6 / 2*regionIndexCount / regionIndexes[]
//!   .. / itemCount * (shortDeltaCount*2 + (regionIndexCount-shortDeltaCount))
//!        / deltaSets[itemCount][regionIndexCount]
//! ```
//!
//! ## Processing (§7.3.6.2 / §7.1)
//!
//! Given a normalised coordinate vector `n[axisCount]`:
//!
//! 1. Locate the value record for the requested tag.
//! 2. Pick `IVD = subtables[outer]`, then `row = IVD.deltaSets[inner]`.
//! 3. For each column `k` of the row, the region scalar is the
//!    product of per-axis scalars for `IVD.regionIndexes[k]`. A
//!    per-axis scalar is `1` if `peak == 0` (axis ignored), `0` if
//!    `n` lies outside `[start, end]` or has opposite sign to `peak`,
//!    `(n - start) / (peak - start)` on the rising side, and
//!    `(end - n) / (end - peak)` on the falling side.
//! 4. The interpolated adjustment is `Σₖ scalar(k) * delta(k)` (the
//!    delta is `int16` for the first `shortDeltaCount` columns and
//!    `int8` for the rest).
//!
//! Future-version compatibility: `valueRecordSize` is honoured as the
//! stride so a minor-version bump that grows the record (with the
//! first 8 bytes preserved per the spec note in §7.3.6.1) parses
//! correctly; the trailing bytes are ignored.

use crate::parser::{read_i16, read_i8, read_u16, read_u32};
use crate::Error;

/// One region axis as a `(start, peak, end)` triple in F2DOT14 (decoded
/// to f32). One per variation axis declared in `fvar`.
type RegionAxis = (f32, f32, f32);
/// One `VariationRegion` — a slice of `(start, peak, end)` per axis.
type Region = Vec<RegionAxis>;

/// Bound on the value-record count we will parse. Real-world MVAR
/// tables stay in low double digits (the §7.3.6.3 registry defines
/// 39 standard tags); the cap exists purely to bound work on a
/// malformed header.
const MAX_VALUE_RECORDS: u16 = 2048;
/// Bound on the IVS subtable count. The spec allows up to 65 536;
/// fonts in practice ship 1.
const MAX_IVD_SUBTABLES: u16 = 4096;
/// Bound on per-IVS region count.
const MAX_REGIONS: u16 = 4096;

/// Parsed MVAR table.
#[derive(Debug, Clone)]
pub struct MvarTable {
    /// Value records, in document order. Each is
    /// `(tag, outer_index, inner_index)`. Sorted by tag in the source
    /// file (binary-search-friendly) but we do not re-sort.
    records: Vec<([u8; 4], u16, u16)>,
    /// The IVS substructure parsed out of the table body. `None` only
    /// when `valueRecordCount == 0` (no metric variations published).
    ivs: Option<ItemVariationStore>,
}

/// Parsed `ItemVariationStore` (§7.2.3).
#[derive(Debug, Clone)]
pub struct ItemVariationStore {
    /// `fvar.axisCount` repeated for self-validation; callers should
    /// pass an equal-length normalised vector when computing scalars.
    axis_count: u16,
    /// All regions referenced by any subtable. Each region is one
    /// `(start, peak, end)` triple per axis, in axis order.
    regions: Vec<Region>,
    /// Item-variation data subtables.
    subtables: Vec<ItemVariationData>,
}

#[derive(Debug, Clone)]
struct ItemVariationData {
    /// Indices into the parent `ItemVariationStore::regions` array,
    /// one per column of `delta_sets`.
    region_indexes: Vec<u16>,
    /// `delta_sets[row][col]` — the dense delta matrix. The split
    /// between the leading `int16` columns and the trailing `int8`
    /// columns from `shortDeltaCount` is consumed at parse time;
    /// widened values are stored as `i32`.
    delta_sets: Vec<Vec<i32>>,
}

impl MvarTable {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 12 {
            return Err(Error::UnexpectedEof);
        }
        let major = read_u16(bytes, 0)?;
        let _minor = read_u16(bytes, 2)?;
        // bytes[4..6]: reserved
        let value_record_size = read_u16(bytes, 6)?;
        let value_record_count = read_u16(bytes, 8)?;
        let ivs_offset = read_u16(bytes, 10)? as usize;

        if major != 1 {
            return Err(Error::BadStructure("MVAR majorVersion != 1"));
        }
        if value_record_count == 0 {
            // No metric variations published — return an empty
            // table; lookups will all return `None`.
            return Ok(Self {
                records: Vec::new(),
                ivs: None,
            });
        }
        if value_record_size < 8 {
            return Err(Error::BadStructure("MVAR valueRecordSize < 8"));
        }
        if value_record_count > MAX_VALUE_RECORDS {
            return Err(Error::BadStructure("MVAR valueRecordCount exceeds cap"));
        }
        // valueRecordCount > 0 ⇒ ivs_offset must be > 0 per §7.3.6.1.
        if ivs_offset == 0 || ivs_offset > bytes.len() {
            return Err(Error::BadOffset);
        }

        let stride = value_record_size as usize;
        let total_records_bytes = (value_record_count as usize)
            .checked_mul(stride)
            .ok_or(Error::BadOffset)?;
        if 12usize
            .checked_add(total_records_bytes)
            .map(|end| end > bytes.len())
            .unwrap_or(true)
        {
            return Err(Error::UnexpectedEof);
        }

        let mut records = Vec::with_capacity(value_record_count as usize);
        for i in 0..value_record_count as usize {
            let off = 12 + i * stride;
            let tag = [bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]];
            let outer = read_u16(bytes, off + 4)?;
            let inner = read_u16(bytes, off + 6)?;
            records.push((tag, outer, inner));
        }

        let ivs = ItemVariationStore::parse(&bytes[ivs_offset..])?;
        Ok(Self {
            records,
            ivs: Some(ivs),
        })
    }

    /// Number of value records in the table.
    pub fn value_record_count(&self) -> usize {
        self.records.len()
    }

    /// Iterate `(tag, outer, inner)` triples in document order.
    pub fn value_records(&self) -> impl Iterator<Item = ([u8; 4], u16, u16)> + '_ {
        self.records.iter().copied()
    }

    /// Compute the metric delta for `tag` against `normalised_coords`.
    ///
    /// Returns `None` when:
    /// * `tag` is not present in the value-record array, or
    /// * the value record's `(outer, inner)` pair is out of range for
    ///   the embedded IVS.
    ///
    /// Returns `Some(0.0)` when the variation evaluates to zero (e.g.
    /// at the default coordinate) — this is distinct from "tag
    /// absent" because callers may want to log presence even when the
    /// adjustment is currently nil.
    pub fn delta_for_tag(&self, tag: &[u8; 4], normalised_coords: &[f32]) -> Option<f32> {
        let ivs = self.ivs.as_ref()?;
        let (_, outer, inner) = self.records.iter().copied().find(|(t, _, _)| t == tag)?;
        ivs.delta(outer, inner, normalised_coords)
    }

    /// Direct access to the embedded `ItemVariationStore`. Intended
    /// for tests / debugging.
    pub fn item_variation_store(&self) -> Option<&ItemVariationStore> {
        self.ivs.as_ref()
    }
}

impl ItemVariationStore {
    /// Parse an `ItemVariationStore` (§7.2.3) starting at the given
    /// byte slice. Exposed at crate scope so sibling tables that
    /// embed an IVS (notably `HVAR` / `VVAR`) can share the decoder.
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 8 {
            return Err(Error::UnexpectedEof);
        }
        let format = read_u16(bytes, 0)?;
        if format != 1 {
            return Err(Error::BadStructure("IVS format != 1"));
        }
        let vrl_off = read_u32(bytes, 2)? as usize;
        let ivd_count = read_u16(bytes, 6)?;
        if ivd_count > MAX_IVD_SUBTABLES {
            return Err(Error::BadStructure("IVS subtable count exceeds cap"));
        }
        if vrl_off == 0 || vrl_off > bytes.len() {
            return Err(Error::BadOffset);
        }

        let regions = parse_region_list(&bytes[vrl_off..])?;

        let mut subtables = Vec::with_capacity(ivd_count as usize);
        let off_base = 8usize;
        let need = (ivd_count as usize)
            .checked_mul(4)
            .and_then(|n| n.checked_add(off_base))
            .ok_or(Error::BadOffset)?;
        if need > bytes.len() {
            return Err(Error::UnexpectedEof);
        }
        for i in 0..ivd_count as usize {
            let off = off_base + i * 4;
            let sub_off = read_u32(bytes, off)? as usize;
            if sub_off == 0 || sub_off > bytes.len() {
                return Err(Error::BadOffset);
            }
            subtables.push(ItemVariationData::parse(
                &bytes[sub_off..],
                regions.len() as u16,
            )?);
        }

        // axis_count is taken from the region list (§7.2.3.1: equals
        // `fvar.axisCount`).
        let axis_count = if let Some(first) = regions.first() {
            first.len() as u16
        } else {
            0
        };

        Ok(Self {
            axis_count,
            regions,
            subtables,
        })
    }

    /// Number of variation axes the store references.
    pub fn axis_count(&self) -> u16 {
        self.axis_count
    }

    /// Number of regions the store defines.
    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    /// Number of `ItemVariationData` subtables.
    pub fn subtable_count(&self) -> usize {
        self.subtables.len()
    }

    /// Interpolated delta for `(outer, inner)` against
    /// `normalised_coords`. `None` when either index is out of range.
    pub fn delta(&self, outer: u16, inner: u16, normalised_coords: &[f32]) -> Option<f32> {
        let sub = self.subtables.get(outer as usize)?;
        let row = sub.delta_sets.get(inner as usize)?;
        let mut acc = 0.0f32;
        for (col, &delta) in row.iter().enumerate() {
            let region_index = *sub.region_indexes.get(col)? as usize;
            let region = self.regions.get(region_index)?;
            let scalar = region_scalar(region, normalised_coords);
            if scalar == 0.0 {
                continue;
            }
            acc += scalar * delta as f32;
        }
        Some(acc)
    }
}

fn parse_region_list(bytes: &[u8]) -> Result<Vec<Region>, Error> {
    if bytes.len() < 4 {
        return Err(Error::UnexpectedEof);
    }
    let axis_count = read_u16(bytes, 0)?;
    let region_count = read_u16(bytes, 2)?;
    if region_count > MAX_REGIONS {
        return Err(Error::BadStructure("IVS regionCount exceeds cap"));
    }
    let stride = (axis_count as usize)
        .checked_mul(6)
        .ok_or(Error::BadOffset)?;
    let total = (region_count as usize)
        .checked_mul(stride)
        .and_then(|n| n.checked_add(4))
        .ok_or(Error::BadOffset)?;
    if total > bytes.len() {
        return Err(Error::UnexpectedEof);
    }
    let mut regions = Vec::with_capacity(region_count as usize);
    for r in 0..region_count as usize {
        let base = 4 + r * stride;
        let mut axes = Vec::with_capacity(axis_count as usize);
        for a in 0..axis_count as usize {
            let off = base + a * 6;
            let start = f2dot14(read_i16(bytes, off)?);
            let peak = f2dot14(read_i16(bytes, off + 2)?);
            let end = f2dot14(read_i16(bytes, off + 4)?);
            axes.push((start, peak, end));
        }
        regions.push(axes);
    }
    Ok(regions)
}

impl ItemVariationData {
    fn parse(bytes: &[u8], region_count_in_store: u16) -> Result<Self, Error> {
        if bytes.len() < 6 {
            return Err(Error::UnexpectedEof);
        }
        let item_count = read_u16(bytes, 0)?;
        let short_delta_count = read_u16(bytes, 2)?;
        let region_index_count = read_u16(bytes, 4)?;
        // No explicit row/column cap is needed beyond the u16 width:
        // the trailing bounds check on `total` against `bytes.len()`
        // already rejects oversized matrices.
        if short_delta_count > region_index_count {
            return Err(Error::BadStructure("shortDeltaCount > regionIndexCount"));
        }
        let region_index_bytes = (region_index_count as usize)
            .checked_mul(2)
            .ok_or(Error::BadOffset)?;
        let header_end = 6usize
            .checked_add(region_index_bytes)
            .ok_or(Error::BadOffset)?;
        if header_end > bytes.len() {
            return Err(Error::UnexpectedEof);
        }
        let mut region_indexes = Vec::with_capacity(region_index_count as usize);
        for i in 0..region_index_count as usize {
            let idx = read_u16(bytes, 6 + i * 2)?;
            if idx >= region_count_in_store {
                return Err(Error::BadStructure("IVD region index out of range"));
            }
            region_indexes.push(idx);
        }

        let row_size = (short_delta_count as usize)
            .checked_mul(2)
            .and_then(|n| n.checked_add((region_index_count - short_delta_count) as usize))
            .ok_or(Error::BadOffset)?;
        let total = (item_count as usize)
            .checked_mul(row_size)
            .and_then(|n| n.checked_add(header_end))
            .ok_or(Error::BadOffset)?;
        if total > bytes.len() {
            return Err(Error::UnexpectedEof);
        }

        let mut delta_sets = Vec::with_capacity(item_count as usize);
        for r in 0..item_count as usize {
            let row_start = header_end + r * row_size;
            let mut row = Vec::with_capacity(region_index_count as usize);
            for c in 0..short_delta_count as usize {
                row.push(read_i16(bytes, row_start + c * 2)? as i32);
            }
            let i8_base = row_start + (short_delta_count as usize) * 2;
            for c in 0..(region_index_count - short_delta_count) as usize {
                row.push(read_i8(bytes, i8_base + c)? as i32);
            }
            delta_sets.push(row);
        }

        Ok(Self {
            region_indexes,
            delta_sets,
        })
    }
}

/// Combined per-region scalar (§7.1 / §7.2.3.1). Returns 0 when the
/// coordinate is outside the region span on any axis, 1 when every
/// per-axis peak is 0 (the "region matches all instances" degenerate
/// case the spec mentions in passing for placeholder regions).
fn region_scalar(region: &[RegionAxis], coords: &[f32]) -> f32 {
    let mut s = 1.0f32;
    for (ai, &(start, peak, end)) in region.iter().enumerate() {
        let c = coords.get(ai).copied().unwrap_or(0.0);
        if peak == 0.0 {
            // §7.2.3.1: peakCoord == 0 ⇒ axis does not factor.
            continue;
        }
        if c == peak {
            continue;
        }
        if c <= start || c >= end {
            return 0.0;
        }
        if c < peak {
            // Rising edge.
            if (peak - start).abs() < f32::EPSILON {
                return 0.0;
            }
            s *= (c - start) / (peak - start);
        } else {
            // Falling edge.
            if (end - peak).abs() < f32::EPSILON {
                return 0.0;
            }
            s *= (end - c) / (end - peak);
        }
    }
    s
}

#[inline]
fn f2dot14(raw: i16) -> f32 {
    raw as f32 / 16384.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-built single-axis MVAR with one tag and one region/row.
    /// Region: opsz-style (start=0, peak=+1, end=+1).
    /// IVD: one row, one int16 delta of -100 against region 0.
    fn build_single_axis_mvar() -> Vec<u8> {
        // Layout:
        //   [0..12)  MVAR header
        //   [12..20) one ValueRecord (8 bytes)
        //   [20..)   IVS
        // IVS layout (starts at offset 20):
        //   IVS+0..8     header (format=1, vrlOff=12, ivdCount=1)
        //   IVS+8..12    ivdOffsets[0]
        //   IVS+12..22   region list (axisCount=1, regionCount=1, +1 region × 6 B)
        //   IVS+22..32   IVD subtable
        // IVD layout (10 bytes total):
        //   [0..6)   header (itemCount=1, shortDeltaCount=1, regionIxCount=1)
        //   [6..8)   regionIndexes[0] = 0
        //   [8..10)  deltaSet[0]: int16 delta
        let ivd_rel = 22u32;
        let mut b = vec![0u8; 12 + 8 + 22 + 10];

        // MVAR header
        b[0..2].copy_from_slice(&1u16.to_be_bytes()); // major
                                                      // minor / reserved = 0
        b[6..8].copy_from_slice(&8u16.to_be_bytes()); // valueRecordSize
        b[8..10].copy_from_slice(&1u16.to_be_bytes()); // valueRecordCount
        b[10..12].copy_from_slice(&20u16.to_be_bytes()); // ivsOffset

        // Value record at [12..20)
        b[12..16].copy_from_slice(b"xhgt");
        b[16..18].copy_from_slice(&0u16.to_be_bytes()); // outer
        b[18..20].copy_from_slice(&0u16.to_be_bytes()); // inner

        let ivs = 20usize;
        // IVS header
        b[ivs..ivs + 2].copy_from_slice(&1u16.to_be_bytes()); // format
        b[ivs + 2..ivs + 6].copy_from_slice(&12u32.to_be_bytes()); // vrlOff
        b[ivs + 6..ivs + 8].copy_from_slice(&1u16.to_be_bytes()); // ivdCount
        b[ivs + 8..ivs + 12].copy_from_slice(&ivd_rel.to_be_bytes());

        // Region list at IVS+12..22 (axisCount=1, regionCount=1, +6 B region).
        let rl = ivs + 12;
        b[rl..rl + 2].copy_from_slice(&1u16.to_be_bytes()); // axisCount
        b[rl + 2..rl + 4].copy_from_slice(&1u16.to_be_bytes()); // regionCount
                                                                // axis 0: start=0, peak=16384 (=+1), end=16384
        b[rl + 4..rl + 6].copy_from_slice(&0i16.to_be_bytes());
        b[rl + 6..rl + 8].copy_from_slice(&16384i16.to_be_bytes());
        b[rl + 8..rl + 10].copy_from_slice(&16384i16.to_be_bytes());

        // IVD at IVS+22..32.
        let ivd = ivs + ivd_rel as usize;
        b[ivd..ivd + 2].copy_from_slice(&1u16.to_be_bytes()); // itemCount
        b[ivd + 2..ivd + 4].copy_from_slice(&1u16.to_be_bytes()); // shortDeltaCount
        b[ivd + 4..ivd + 6].copy_from_slice(&1u16.to_be_bytes()); // regionIndexCount
        b[ivd + 6..ivd + 8].copy_from_slice(&0u16.to_be_bytes()); // regionIndexes[0] = 0
        b[ivd + 8..ivd + 10].copy_from_slice(&(-100i16).to_be_bytes()); // row[0][0]
        b
    }

    #[test]
    fn parses_minimal_table() {
        let raw = build_single_axis_mvar();
        let m = MvarTable::parse(&raw).expect("parse");
        assert_eq!(m.value_record_count(), 1);
        assert_eq!(
            m.value_records().collect::<Vec<_>>(),
            vec![(*b"xhgt", 0u16, 0u16)]
        );
        let ivs = m.item_variation_store().expect("ivs");
        assert_eq!(ivs.axis_count(), 1);
        assert_eq!(ivs.region_count(), 1);
        assert_eq!(ivs.subtable_count(), 1);
    }

    #[test]
    fn delta_zero_at_default_coords() {
        let raw = build_single_axis_mvar();
        let m = MvarTable::parse(&raw).unwrap();
        // Normalised coord at axis default (= 0): peak=+1 ⇒ scalar 0 ⇒
        // delta == 0.
        let d = m.delta_for_tag(b"xhgt", &[0.0]).expect("known tag");
        assert_eq!(d, 0.0);
    }

    #[test]
    fn delta_interpolates_along_axis() {
        let raw = build_single_axis_mvar();
        let m = MvarTable::parse(&raw).unwrap();
        // At halfway up the rising edge: scalar = (0.5 - 0) / (1 - 0) = 0.5
        // delta = 0.5 * -100 = -50.0
        let d = m.delta_for_tag(b"xhgt", &[0.5]).expect("known tag");
        assert!((d - (-50.0)).abs() < 1e-5, "got {d}");
        // At the peak: scalar = 1, delta = -100.
        let d = m.delta_for_tag(b"xhgt", &[1.0]).expect("known tag");
        assert!((d - (-100.0)).abs() < 1e-5, "got {d}");
    }

    #[test]
    fn unknown_tag_returns_none() {
        let raw = build_single_axis_mvar();
        let m = MvarTable::parse(&raw).unwrap();
        assert!(m.delta_for_tag(b"none", &[0.5]).is_none());
    }

    #[test]
    fn empty_value_records_parses_with_no_ivs() {
        // valueRecordCount = 0, ivsOffset = 0 — minimum legal header.
        let mut b = vec![0u8; 12];
        b[0..2].copy_from_slice(&1u16.to_be_bytes());
        // Everything else 0.
        let m = MvarTable::parse(&b).expect("parse");
        assert_eq!(m.value_record_count(), 0);
        assert!(m.item_variation_store().is_none());
        assert!(m.delta_for_tag(b"xhgt", &[0.0]).is_none());
    }

    #[test]
    fn rejects_short_value_record_size() {
        let mut b = vec![0u8; 14];
        b[0..2].copy_from_slice(&1u16.to_be_bytes());
        b[6..8].copy_from_slice(&4u16.to_be_bytes()); // valueRecordSize = 4 (< 8)
        b[8..10].copy_from_slice(&1u16.to_be_bytes()); // 1 record
        b[10..12].copy_from_slice(&12u16.to_be_bytes());
        assert!(matches!(MvarTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn honours_larger_value_record_stride() {
        // Simulate a hypothetical minor-version with a 10-byte value
        // record (first 8 bytes match v1, two trailing bytes ignored).
        let stride: u16 = 10;
        let count: u16 = 1;
        let ivd_rel: u32 = 22;
        // header 12 + record 10 + IVS{8 + 4 + 10 + 10} = 12 + 10 + 32
        let mut b = vec![0u8; 12 + stride as usize + 32];
        b[0..2].copy_from_slice(&1u16.to_be_bytes());
        b[6..8].copy_from_slice(&stride.to_be_bytes());
        b[8..10].copy_from_slice(&count.to_be_bytes());
        let ivs_off: u16 = 12 + stride;
        b[10..12].copy_from_slice(&ivs_off.to_be_bytes());
        // ValueRecord (8-byte prefix + 2 trailing bytes)
        b[12..16].copy_from_slice(b"hasc");
        b[16..18].copy_from_slice(&0u16.to_be_bytes()); // outer
        b[18..20].copy_from_slice(&0u16.to_be_bytes()); // inner
                                                        // bytes 20..22 are the unknown trailing bytes; we leave them zero.
                                                        // IVS at ivs_off
        let ivs = ivs_off as usize;
        b[ivs..ivs + 2].copy_from_slice(&1u16.to_be_bytes());
        b[ivs + 2..ivs + 6].copy_from_slice(&12u32.to_be_bytes()); // vrlOff
        b[ivs + 6..ivs + 8].copy_from_slice(&1u16.to_be_bytes());
        b[ivs + 8..ivs + 12].copy_from_slice(&ivd_rel.to_be_bytes());
        // region list at IVS+12, single region with 1 axis (10 B)
        let rl = ivs + 12;
        b[rl..rl + 2].copy_from_slice(&1u16.to_be_bytes()); // axisCount
        b[rl + 2..rl + 4].copy_from_slice(&1u16.to_be_bytes()); // regionCount
        b[rl + 6..rl + 8].copy_from_slice(&16384i16.to_be_bytes());
        b[rl + 8..rl + 10].copy_from_slice(&16384i16.to_be_bytes());
        // IVD at IVS+22
        let ivd = ivs + ivd_rel as usize;
        b[ivd..ivd + 2].copy_from_slice(&1u16.to_be_bytes());
        b[ivd + 2..ivd + 4].copy_from_slice(&1u16.to_be_bytes());
        b[ivd + 4..ivd + 6].copy_from_slice(&1u16.to_be_bytes());
        b[ivd + 6..ivd + 8].copy_from_slice(&0u16.to_be_bytes());
        b[ivd + 8..ivd + 10].copy_from_slice(&(42i16).to_be_bytes());

        let m = MvarTable::parse(&b).expect("parse");
        assert_eq!(m.value_record_count(), 1);
        // Round-trip the delta: scalar=1 at peak ⇒ delta=42.
        let d = m.delta_for_tag(b"hasc", &[1.0]).expect("tag");
        assert!((d - 42.0).abs() < 1e-5);
    }

    #[test]
    fn region_scalar_zero_outside_span() {
        // Region: start=-1, peak=-1, end=0 (typical "wght going below default")
        let region = [(-1.0f32, -1.0, 0.0)];
        // Coord +0.5 has opposite sign from peak — scalar 0.
        assert_eq!(region_scalar(&region, &[0.5]), 0.0);
        // Coord 0 == end — scalar 0.
        assert_eq!(region_scalar(&region, &[0.0]), 0.0);
        // Coord -1 == peak — scalar 1.
        assert_eq!(region_scalar(&region, &[-1.0]), 1.0);
        // Coord -0.5 on the falling edge: (end - c)/(end - peak)
        //   = (0 - (-0.5)) / (0 - (-1)) = 0.5
        let s = region_scalar(&region, &[-0.5]);
        assert!((s - 0.5).abs() < 1e-5);
    }

    #[test]
    fn region_scalar_axis_ignored_when_peak_zero() {
        // Two-axis region: axis 0 ignored (peak=0), axis 1 active.
        let region = [(0.0f32, 0.0, 0.0), (0.0, 1.0, 1.0)];
        // Axis 0 coordinate is irrelevant.
        let s = region_scalar(&region, &[0.7, 1.0]);
        assert!((s - 1.0).abs() < 1e-5);
    }

    #[test]
    fn rejects_ivs_format_other_than_1() {
        // Same scaffold as the minimal-table builder but flip the IVS
        // format byte.
        let mut b = build_single_axis_mvar();
        let ivs = 20usize;
        b[ivs..ivs + 2].copy_from_slice(&2u16.to_be_bytes());
        assert!(matches!(MvarTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_out_of_range_region_index_in_ivd() {
        // Build a valid table then poke the regionIndexes[0] to 5
        // (region_count is 1, so 5 is out of range). IVD starts at
        // MVAR offset 20 (IVS) + 22 (IVS-relative IVD offset) = 42.
        let mut b = build_single_axis_mvar();
        let ivd = 20 + 22;
        b[ivd + 6..ivd + 8].copy_from_slice(&5u16.to_be_bytes());
        assert!(matches!(MvarTable::parse(&b), Err(Error::BadStructure(_))));
    }
}
