//! `FeatureVariations` — the GSUB/GPOS feature-variation substructure.
//!
//! ISO/IEC 14496-22:2019 §6.2.9. A feature variations table lets a
//! variable font swap the default set of lookups for a given feature
//! with an alternate set under particular runtime conditions — almost
//! always "the current variation instance falls inside a normalised
//! range on some `fvar` axis". The canonical use is optical-size and
//! weight-driven feature toggling (e.g. turning a contextual lookup on
//! only at small `opsz`).
//!
//! The table hangs off the **version 1.1** header of `GSUB` / `GPOS`:
//! those headers carry an `Offset32 featureVariationsOffset` after the
//! three v1.0 offsets when `minorVersion == 1`. This module decodes the
//! substructure once and is shared by both layout tables.
//!
//! On-wire shape per §6.2.9:
//!
//! ```text
//! FeatureVariations
//!   uint16 majorVersion                 (= 1)
//!   uint16 minorVersion                 (= 0)
//!   uint32 featureVariationRecordCount
//!   FeatureVariationRecord[count]:
//!     Offset32 conditionSetOffset                  (0 = universal match)
//!     Offset32 featureTableSubstitutionOffset      (0 = no substitution)
//!
//! ConditionSet
//!   uint16   conditionCount
//!   Offset32 conditions[conditionCount]            (from ConditionSet start)
//!
//! Condition (format 1: Font Variation Axis Range)
//!   uint16  format            (= 1)
//!   uint16  axisIndex         (zero-based into fvar)
//!   F2DOT14 filterRangeMinValue
//!   F2DOT14 filterRangeMaxValue
//!
//! FeatureTableSubstitution
//!   uint16 majorVersion       (= 1)
//!   uint16 minorVersion       (= 0)
//!   uint16 substitutionCount
//!   FeatureTableSubstitutionRecord[count]:
//!     uint16   featureIndex
//!     Offset32 alternateFeatureTableOffset         (from FeatureTableSubstitution start)
//! ```
//!
//! The matching algorithm (§6.2.9): condition sets are evaluated in
//! array order; the conditions inside one set are AND-ed; the first
//! record whose condition set matches (and whose substitution-table
//! version is supported) wins, and no later records are considered. A
//! zero `conditionSetOffset` is the universal match. A zero
//! `featureTableSubstitutionOffset` makes no substitutions.

use crate::parser::{read_u16, read_u32};
use crate::Error;

/// The only Condition table format defined by §6.2.9.
const CONDITION_FORMAT_AXIS_RANGE: u16 = 1;

/// Decode an F2DOT14 (signed 2.14 fixed point) into an `f32`, matching
/// the normalised-coordinate scale `Font::normalised_coords` produces.
fn f2dot14(raw: i16) -> f32 {
    raw as f32 / 16384.0
}

/// A parsed `FeatureVariations` table borrowing the layout-table bytes.
///
/// `base` is the byte slice of the *whole* `GSUB` / `GPOS` table; the
/// table-internal offset `fv_off` locates the FeatureVariations table,
/// and every offset inside the substructure is resolved relative to its
/// containing table per §6.2.9 (records are relative to the
/// FeatureVariations start; condition offsets relative to the
/// ConditionSet start; alternate-feature offsets relative to the
/// FeatureTableSubstitution start).
#[derive(Debug, Clone)]
pub struct FeatureVariations<'a> {
    /// The whole GSUB/GPOS byte slice (offsets in the FeatureVariations
    /// table are relative to its own start, not to this slice; we keep
    /// the parent slice only to honour the table-internal offset model).
    table: &'a [u8],
    /// Byte offset of the FeatureVariations table within `table`.
    fv_off: u32,
    record_count: u32,
}

impl<'a> FeatureVariations<'a> {
    /// Parse the FeatureVariations table located at `fv_off` inside the
    /// `table` byte slice (the whole GSUB/GPOS table). Returns `None`
    /// when `fv_off` is zero (no table) or out of range, and an error
    /// only for a structurally invalid header.
    ///
    /// An unsupported `majorVersion` is rejected as `BadStructure`:
    /// §6.2.9 sets it to 1 and a layout engine that doesn't recognise a
    /// later major version cannot safely interpret the records.
    pub fn parse(table: &'a [u8], fv_off: u32) -> Result<Option<Self>, Error> {
        if fv_off == 0 {
            return Ok(None);
        }
        let start = fv_off as usize;
        let body = table.get(start..).ok_or(Error::BadOffset)?;
        if body.len() < 8 {
            return Err(Error::UnexpectedEof);
        }
        let major = read_u16(body, 0)?;
        if major != 1 {
            return Err(Error::BadStructure(
                "FeatureVariations: unsupported major version",
            ));
        }
        // minor at +2 (tolerated)
        let record_count = read_u32(body, 4)?;
        // Each FeatureVariationRecord is 8 bytes; bound the array.
        let need = 8usize
            .checked_add(
                (record_count as usize)
                    .checked_mul(8)
                    .ok_or(Error::BadStructure(
                        "FeatureVariations: record count overflow",
                    ))?,
            )
            .ok_or(Error::BadStructure(
                "FeatureVariations: record array overflow",
            ))?;
        if body.len() < need {
            return Err(Error::UnexpectedEof);
        }
        Ok(Some(Self {
            table,
            fv_off,
            record_count,
        }))
    }

    /// Number of FeatureVariationRecords.
    pub fn record_count(&self) -> u32 {
        self.record_count
    }

    /// Find the FeatureTableSubstitution that applies at the supplied
    /// normalised variation coordinates, per the §6.2.9 first-match
    /// rule. Returns `None` when no record matches.
    ///
    /// `normalised_coords` is the avar-bent normalised axis vector
    /// (`Font::normalised_coords`); each entry is in `[-1, 1]` and is
    /// indexed by `fvar` axis order. A condition referencing an axis
    /// beyond the supplied vector is treated as not satisfied, which —
    /// because conditions are AND-ed — makes that condition set fail to
    /// match (the §6.2.9 "If the AxisIndex is invalid … is ignored"
    /// rule degrades the whole record to non-matching).
    pub fn active_substitution(
        &self,
        normalised_coords: &[f32],
    ) -> Option<FeatureTableSubstitution<'a>> {
        let fv = self.table.get(self.fv_off as usize..)?;
        for i in 0..self.record_count as usize {
            let rec = 8 + i * 8;
            let condition_set_off = read_u32(fv, rec).ok()?;
            let subst_off = read_u32(fv, rec + 4).ok()?;
            if !self.condition_set_matches(fv, condition_set_off, normalised_coords) {
                continue;
            }
            // §6.2.9: a zero substitution offset = no substitutions, but
            // it still counts as the matched record (processing stops).
            if subst_off == 0 {
                return None;
            }
            let subst_body = fv.get(subst_off as usize..)?;
            match FeatureTableSubstitution::parse(self.table, self.fv_off + subst_off, subst_body) {
                // Supported version → this record wins; stop here.
                Ok(Some(s)) => return Some(s),
                // Unsupported version → §6.2.9 says reject this record
                // and move on to the next one.
                Ok(None) => continue,
                Err(_) => return None,
            }
        }
        None
    }

    /// Evaluate one ConditionSet at `condition_set_off` (relative to the
    /// FeatureVariations start). An offset of zero is the universal
    /// match (§6.2.9). Conditions are conjunctive (boolean AND).
    fn condition_set_matches(&self, fv: &[u8], condition_set_off: u32, coords: &[f32]) -> bool {
        if condition_set_off == 0 {
            // Universal condition: matches all contexts.
            return true;
        }
        let cs = match fv.get(condition_set_off as usize..) {
            Some(s) => s,
            None => return false,
        };
        let count = match read_u16(cs, 0) {
            Ok(v) => v as usize,
            Err(_) => return false,
        };
        // §6.2.9: "If a given condition set contains no conditions, then
        // it matches all contexts."
        if count == 0 {
            return true;
        }
        if cs.len() < 2 + count * 4 {
            return false;
        }
        for i in 0..count {
            let cond_off = match read_u32(cs, 2 + i * 4) {
                Ok(v) => v as usize,
                Err(_) => return false,
            };
            let cond = match cs.get(cond_off..) {
                Some(s) => s,
                None => return false,
            };
            if !condition_matches(cond, coords) {
                return false;
            }
        }
        true
    }
}

/// Evaluate a single Condition table (only format 1 is defined).
///
/// An unrecognised format makes the condition fail to match, so the
/// whole AND-ed condition set fails — this is the §6.2.9
/// forward-compatibility behaviour ("it should fail to match the
/// condition set whenever an unrecognized condition format is
/// encountered").
fn condition_matches(cond: &[u8], coords: &[f32]) -> bool {
    let format = match read_u16(cond, 0) {
        Ok(v) => v,
        Err(_) => return false,
    };
    if format != CONDITION_FORMAT_AXIS_RANGE {
        return false;
    }
    let axis_index = match read_u16(cond, 2) {
        Ok(v) => v as usize,
        Err(_) => return false,
    };
    let min = match crate::parser::read_i16(cond, 4) {
        Ok(v) => f2dot14(v),
        Err(_) => return false,
    };
    let max = match crate::parser::read_i16(cond, 6) {
        Ok(v) => f2dot14(v),
        Err(_) => return false,
    };
    // §6.2.9: "If the AxisIndex is invalid, the feature variation record
    // containing this condition table is ignored." We map "invalid axis"
    // (beyond the supplied coordinate vector) to non-matching so the
    // AND-ed set fails and the record is skipped.
    let value = match coords.get(axis_index) {
        Some(v) => *v,
        None => return false,
    };
    value >= min && value <= max
}

/// A parsed FeatureTableSubstitution table (§6.2.9). Looks up the
/// alternate feature table for a feature index and resolves its
/// lookup-index list.
#[derive(Debug, Clone)]
pub struct FeatureTableSubstitution<'a> {
    /// The whole GSUB/GPOS slice (for completeness / future use).
    #[allow(dead_code)]
    table: &'a [u8],
    /// Byte offset of this FeatureTableSubstitution table within the
    /// whole GSUB/GPOS slice; alternate-feature offsets are relative to
    /// the FeatureTableSubstitution start, so we keep a slice that
    /// begins exactly there.
    body: &'a [u8],
    substitution_count: u16,
}

impl<'a> FeatureTableSubstitution<'a> {
    /// Parse the FeatureTableSubstitution table. `subst_off` is its
    /// offset within the whole GSUB/GPOS slice; `body` is the slice that
    /// begins at that offset.
    ///
    /// Returns `Ok(None)` for an unsupported `majorVersion` so the
    /// caller can apply the §6.2.9 "reject this record and continue"
    /// rule, and `Err` only for a truncated header.
    fn parse(table: &'a [u8], _subst_off: u32, body: &'a [u8]) -> Result<Option<Self>, Error> {
        if body.len() < 6 {
            return Err(Error::UnexpectedEof);
        }
        let major = read_u16(body, 0)?;
        if major != 1 {
            return Ok(None);
        }
        // minor at +2 (tolerated)
        let substitution_count = read_u16(body, 4)?;
        if body.len() < 6 + substitution_count as usize * 6 {
            return Err(Error::UnexpectedEof);
        }
        Ok(Some(Self {
            table,
            body,
            substitution_count,
        }))
    }

    /// Number of FeatureTableSubstitutionRecords.
    pub fn substitution_count(&self) -> u16 {
        self.substitution_count
    }

    /// Resolve the alternate feature table for `feature_index` to its
    /// lookup-index list, when this substitution table overrides that
    /// feature. Returns `None` when the feature is not substituted.
    ///
    /// §6.2.9: records are ordered by increasing `featureIndex` with no
    /// duplicates; "the first record having that index is matched, and
    /// searching ends if a record is encountered with a higher index
    /// value." We honour the early-out on a higher index.
    pub fn lookup_indices_for_feature(&self, feature_index: u16) -> Option<Vec<u16>> {
        for i in 0..self.substitution_count as usize {
            let r = 6 + i * 6;
            let fi = read_u16(self.body, r).ok()?;
            if fi == feature_index {
                let alt_off = read_u32(self.body, r + 2).ok()? as usize;
                let alt = self.body.get(alt_off..)?;
                // An alternate feature table has the same layout as a
                // normal Feature table: Offset16 featureParamsOffset;
                // u16 lookupIndexCount; u16 lookupListIndices[count].
                if alt.len() < 4 {
                    return None;
                }
                let count = read_u16(alt, 2).ok()? as usize;
                if alt.len() < 4 + count * 2 {
                    return None;
                }
                let mut idxs = Vec::with_capacity(count);
                for k in 0..count {
                    idxs.push(read_u16(alt, 4 + k * 2).ok()?);
                }
                return Some(idxs);
            }
            if fi > feature_index {
                // Records are sorted ascending; no match possible.
                return None;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build a FeatureVariations table with one record whose condition
    // set has a single format-1 axis-range condition, substituting
    // feature index `feat` with a single-lookup alternate feature.
    //
    // Layout (all offsets relative to FeatureVariations start):
    //   FeatureVariations header (8)
    //   FeatureVariationRecord (8)        -> conditionSet @ cs, subst @ ss
    //   ConditionSet @ cs:  u16 count=1, u32 condOff (rel to cs)
    //   Condition  @ cs+condOff: u16 fmt=1, u16 axis, i16 min, i16 max
    //   FeatureTableSubstitution @ ss: u16 1, u16 0, u16 count=1,
    //                                  rec{ u16 feat, u32 altOff (rel ss) }
    //   AltFeature @ ss+altOff: u16 0 (params), u16 1 (count), u16 lookup
    // The FeatureVariations table never sits at offset 0 of a real
    // GSUB/GPOS table (the header occupies the start), and offset 0 is
    // the "no table" sentinel. Prefix every synthetic table with this
    // many pad bytes and parse at `FV_BASE`.
    const FV_BASE: u32 = 16;

    fn build_fv(axis: u16, min: i16, max: i16, feat: u16, lookup: u16) -> Vec<u8> {
        let mut b = vec![0u8; FV_BASE as usize];
        // header
        b.extend_from_slice(&1u16.to_be_bytes()); // major
        b.extend_from_slice(&0u16.to_be_bytes()); // minor
        b.extend_from_slice(&1u32.to_be_bytes()); // recordCount

        // We'll fill the record offsets after laying out the bodies.
        let rec_pos = b.len();
        b.extend_from_slice(&0u32.to_be_bytes()); // conditionSetOffset (patch)
        b.extend_from_slice(&0u32.to_be_bytes()); // featureTableSubstOffset (patch)

        // ConditionSet
        let cs = b.len() as u32;
        b.extend_from_slice(&1u16.to_be_bytes()); // conditionCount
        let cond_off_pos = b.len();
        b.extend_from_slice(&0u32.to_be_bytes()); // condition offset (patch, rel cs)
        let cond_rel = (b.len() as u32) - cs;
        // Condition format 1
        b.extend_from_slice(&1u16.to_be_bytes()); // format
        b.extend_from_slice(&axis.to_be_bytes()); // axisIndex
        b.extend_from_slice(&min.to_be_bytes()); // filterRangeMin (F2DOT14)
        b.extend_from_slice(&max.to_be_bytes()); // filterRangeMax (F2DOT14)

        // FeatureTableSubstitution
        let ss = b.len() as u32;
        b.extend_from_slice(&1u16.to_be_bytes()); // major
        b.extend_from_slice(&0u16.to_be_bytes()); // minor
        b.extend_from_slice(&1u16.to_be_bytes()); // substitutionCount
        b.extend_from_slice(&feat.to_be_bytes()); // featureIndex
        let alt_off_pos = b.len();
        b.extend_from_slice(&0u32.to_be_bytes()); // altFeatureOffset (patch, rel ss)
        let alt_rel = (b.len() as u32) - ss;
        // Alternate Feature table
        b.extend_from_slice(&0u16.to_be_bytes()); // featureParamsOffset
        b.extend_from_slice(&1u16.to_be_bytes()); // lookupIndexCount
        b.extend_from_slice(&lookup.to_be_bytes()); // lookupListIndices[0]

        // patch offsets — record offsets are relative to the
        // FeatureVariations start (FV_BASE); cond/alt offsets are
        // relative to their own containing tables.
        b[rec_pos..rec_pos + 4].copy_from_slice(&(cs - FV_BASE).to_be_bytes());
        b[rec_pos + 4..rec_pos + 8].copy_from_slice(&(ss - FV_BASE).to_be_bytes());
        b[cond_off_pos..cond_off_pos + 4].copy_from_slice(&cond_rel.to_be_bytes());
        b[alt_off_pos..alt_off_pos + 4].copy_from_slice(&alt_rel.to_be_bytes());
        b
    }

    fn f2(v: f32) -> i16 {
        (v * 16384.0).round() as i16
    }

    #[test]
    fn parse_rejects_unsupported_major() {
        let mut b = build_fv(0, f2(0.5), f2(1.0), 3, 7);
        b[FV_BASE as usize] = 0;
        b[FV_BASE as usize + 1] = 2; // major = 2
        assert!(matches!(
            FeatureVariations::parse(&b, FV_BASE),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn zero_offset_is_no_table() {
        assert!(FeatureVariations::parse(&[], 0).unwrap().is_none());
    }

    #[test]
    fn axis_in_range_substitutes() {
        let b = build_fv(0, f2(0.5), f2(1.0), 3, 7);
        let fv = FeatureVariations::parse(&b, FV_BASE).unwrap().unwrap();
        // wght normalised at 0.75 → in [0.5, 1.0]
        let sub = fv.active_substitution(&[0.75]).unwrap();
        assert_eq!(sub.substitution_count(), 1);
        assert_eq!(sub.lookup_indices_for_feature(3), Some(vec![7]));
        // A different feature index is not substituted.
        assert_eq!(sub.lookup_indices_for_feature(2), None);
        assert_eq!(sub.lookup_indices_for_feature(4), None);
    }

    #[test]
    fn axis_out_of_range_no_substitution() {
        let b = build_fv(0, f2(0.5), f2(1.0), 3, 7);
        let fv = FeatureVariations::parse(&b, FV_BASE).unwrap().unwrap();
        // 0.25 below the min → record does not match.
        assert!(fv.active_substitution(&[0.25]).is_none());
        // exactly at the boundaries → inclusive per §6.2.9.
        assert!(fv.active_substitution(&[0.5]).is_some());
        assert!(fv.active_substitution(&[1.0]).is_some());
    }

    #[test]
    fn missing_axis_coordinate_fails_match() {
        // condition references axis index 1, but we only supply one coord
        let b = build_fv(1, f2(-1.0), f2(1.0), 3, 7);
        let fv = FeatureVariations::parse(&b, FV_BASE).unwrap().unwrap();
        assert!(fv.active_substitution(&[0.0]).is_none());
    }

    #[test]
    fn universal_condition_set_always_matches() {
        // Hand-build a record with conditionSetOffset = 0 (universal).
        // The FeatureVariations table starts at FV_BASE; record offsets
        // are relative to that.
        let mut b = vec![0u8; FV_BASE as usize];
        b.extend_from_slice(&1u16.to_be_bytes()); // major
        b.extend_from_slice(&0u16.to_be_bytes()); // minor
        b.extend_from_slice(&1u32.to_be_bytes()); // recordCount
        let rec = b.len();
        b.extend_from_slice(&0u32.to_be_bytes()); // conditionSet = 0 (universal)
        b.extend_from_slice(&0u32.to_be_bytes()); // subst (patch)
        let ss = b.len() as u32;
        b.extend_from_slice(&1u16.to_be_bytes()); // major
        b.extend_from_slice(&0u16.to_be_bytes()); // minor
        b.extend_from_slice(&1u16.to_be_bytes()); // count
        b.extend_from_slice(&9u16.to_be_bytes()); // featureIndex 9
        let alt_pos = b.len();
        b.extend_from_slice(&0u32.to_be_bytes());
        let alt_rel = (b.len() as u32) - ss;
        b.extend_from_slice(&0u16.to_be_bytes()); // params
        b.extend_from_slice(&2u16.to_be_bytes()); // count 2
        b.extend_from_slice(&4u16.to_be_bytes());
        b.extend_from_slice(&5u16.to_be_bytes());
        b[rec + 4..rec + 8].copy_from_slice(&(ss - FV_BASE).to_be_bytes());
        b[alt_pos..alt_pos + 4].copy_from_slice(&alt_rel.to_be_bytes());

        let fv = FeatureVariations::parse(&b, FV_BASE).unwrap().unwrap();
        // No axes at all → universal still matches.
        let sub = fv.active_substitution(&[]).unwrap();
        assert_eq!(sub.lookup_indices_for_feature(9), Some(vec![4, 5]));
    }

    #[test]
    fn unsupported_subst_version_skips_record() {
        let mut b = build_fv(0, f2(-1.0), f2(1.0), 3, 7);
        // The FeatureTableSubstitution offset is stored in the record at
        // FV_BASE + 12..16, relative to FV_BASE.
        let rec_subst = FV_BASE as usize + 12;
        let ss_rel = u32::from_be_bytes([
            b[rec_subst],
            b[rec_subst + 1],
            b[rec_subst + 2],
            b[rec_subst + 3],
        ]) as usize;
        let ss = FV_BASE as usize + ss_rel;
        b[ss] = 0;
        b[ss + 1] = 2; // major = 2 (unsupported)
        let fv = FeatureVariations::parse(&b, FV_BASE).unwrap().unwrap();
        // Record's condition matches but the subst version is rejected;
        // no further records → no substitution.
        assert!(fv.active_substitution(&[0.0]).is_none());
    }
}
