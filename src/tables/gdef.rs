//! `GDEF` — Glyph Definition Table.
//!
//! Per the OpenType spec, the GDEF header carries up to six sub-table
//! offsets:
//!
//! * `glyphClassDef` (a ClassDef whose values are
//!   `1=Base, 2=Ligature, 3=Mark, 4=Component`),
//! * `attachList` (per-glyph attachment-point indices, used to cache
//!   coordinates alongside glyph bitmaps),
//! * `ligCaretList` (per-ligature caret coordinates, used to split
//!   ligature components during text selection / cursor placement),
//! * `markAttachClassDef` (a ClassDef whose values are the mark
//!   attachment classes referenced by `lookupFlag.markAttachmentType`
//!   in GSUB / GPOS), and
//! * `markGlyphSetsDef` (v1.2+, an array of Coverage tables referenced
//!   by `lookupFlag.useMarkFilteringSet`).
//!
//! Header layout:
//!
//! * v1.0 — major / minor + four Offset16 (12 bytes, no MarkGlyphSets,
//!   no ItemVariationStore).
//! * v1.2 — adds `markGlyphSetsDefOffset` Offset16 (14-byte header).
//! * v1.3 — adds `itemVarStoreOffset` Offset32 (18-byte header).
//!
//! Unknown / null offsets decode to "absent" — the spec marks every
//! sub-table offset as "may be NULL". The ItemVariationStore (v1.3) is
//! parsed lazily by other tables (CaretValueFormat3 VariationIndex
//! references) and is currently exposed only as a raw byte slice; the
//! interpolation re-uses the shared `ItemVariationStore` decoder
//! already in the MVAR / HVAR / VVAR pipeline if/when callers need it.

use crate::parser::{read_i16, read_u16, read_u32};
use crate::tables::device::resolve_device_delta;
use crate::tables::mvar::ItemVariationStore;
use crate::Error;

/// Predefined glyph classes per the GDEF spec.
#[allow(dead_code)]
pub const CLASS_BASE: u16 = 1;
#[allow(dead_code)]
pub const CLASS_LIGATURE: u16 = 2;
pub const CLASS_MARK: u16 = 3;
#[allow(dead_code)]
pub const CLASS_COMPONENT: u16 = 4;

/// A single caret-coordinate slot within a ligature glyph.
///
/// CaretValueFormat 1 / 2 / 3 collapse into a tagged enum so callers
/// don't have to know which encoding the font picked. Format-3 device
/// adjustments are exposed as a raw byte slice for the time being —
/// the parser keeps the offset so a VariationIndex-aware shaper can
/// resolve it through the GDEF ItemVariationStore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaretValue {
    /// Format 1: a single design-unit X (or Y) coordinate.
    DesignUnits(i16),
    /// Format 2: a contour-point index whose post-hinting position is
    /// the caret coordinate.
    ContourPoint(u16),
    /// Format 3: a design-unit X (or Y) coordinate plus a non-zero
    /// offset to a Device / VariationIndex table.
    DesignUnitsWithDevice { coordinate: i16, device_offset: u16 },
}

#[derive(Debug, Clone)]
pub struct GdefTable<'a> {
    bytes: &'a [u8],
    glyph_class_def_off: Option<u32>,
    attach_list_off: Option<u32>,
    lig_caret_list_off: Option<u32>,
    mark_attach_class_def_off: Option<u32>,
    mark_glyph_sets_def_off: Option<u32>,
    item_var_store_off: Option<u32>,
}

impl<'a> GdefTable<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        // Header version: u16 major, u16 minor; offsets to subtables.
        // Minimum header (v1.0) is 12 bytes.
        if bytes.len() < 12 {
            return Err(Error::UnexpectedEof);
        }
        let major = read_u16(bytes, 0)?;
        let minor = read_u16(bytes, 2)?;
        if major != 1 {
            return Err(Error::BadStructure("GDEF: unsupported version"));
        }

        let glyph_class_def_off = nz_off16(bytes, 4)?;
        let attach_list_off = nz_off16(bytes, 6)?;
        let lig_caret_list_off = nz_off16(bytes, 8)?;
        let mark_attach_class_def_off = nz_off16(bytes, 10)?;

        // v1.2 adds markGlyphSetsDefOffset (Offset16 at 12),
        // v1.3 adds itemVarStoreOffset (Offset32 at 14).
        let mark_glyph_sets_def_off = if minor >= 2 {
            if bytes.len() < 14 {
                return Err(Error::UnexpectedEof);
            }
            nz_off16(bytes, 12)?
        } else {
            None
        };
        let item_var_store_off = if minor >= 3 {
            if bytes.len() < 18 {
                return Err(Error::UnexpectedEof);
            }
            let raw = read_u32(bytes, 14)?;
            if raw == 0 {
                None
            } else {
                Some(raw)
            }
        } else {
            None
        };

        Ok(Self {
            bytes,
            glyph_class_def_off,
            attach_list_off,
            lig_caret_list_off,
            mark_attach_class_def_off,
            mark_glyph_sets_def_off,
            item_var_store_off,
        })
    }

    /// Look up the GlyphClassDef class for `glyph_id`. Returns 0 if no
    /// table is present or the glyph isn't enumerated.
    pub fn glyph_class(&self, glyph_id: u16) -> u16 {
        let off = match self.glyph_class_def_off {
            Some(o) => o as usize,
            None => return 0,
        };
        let sub = match self.bytes.get(off..) {
            Some(s) => s,
            None => return 0,
        };
        class_def_lookup(sub, glyph_id).unwrap_or(0)
    }

    /// Convenience: is this glyph a mark per the spec?
    pub fn is_mark(&self, glyph_id: u16) -> bool {
        self.glyph_class(glyph_id) == CLASS_MARK
    }

    /// Look up the MarkAttachClassDef class for `glyph_id`. Returns 0
    /// when the font has no `markAttachClassDef` sub-table or the
    /// glyph isn't enumerated. The returned class is what
    /// `lookupFlag.markAttachmentType` (the high byte of `lookupFlag`)
    /// is compared against when GSUB / GPOS filters mark glyphs.
    pub fn mark_attach_class(&self, glyph_id: u16) -> u16 {
        let off = match self.mark_attach_class_def_off {
            Some(o) => o as usize,
            None => return 0,
        };
        let sub = match self.bytes.get(off..) {
            Some(s) => s,
            None => return 0,
        };
        class_def_lookup(sub, glyph_id).unwrap_or(0)
    }

    /// Returns the list of contour-point indices defined for
    /// `glyph_id` in the GDEF `AttachList` sub-table, or `None` when
    /// the font has no AttachList (or the glyph isn't covered). The
    /// indices are in the increasing numerical order mandated by the
    /// spec for the on-disk AttachPoint table.
    pub fn attach_points(&self, glyph_id: u16) -> Option<Vec<u16>> {
        let base = self.attach_list_off? as usize;
        let sub = self.bytes.get(base..)?;
        // AttachList header: u16 coverageOffset, u16 glyphCount, u16
        // attachPointOffsets[glyphCount].
        if sub.len() < 4 {
            return None;
        }
        let cov_off = read_u16(sub, 0).ok()? as usize;
        let count = read_u16(sub, 2).ok()? as usize;
        let cov = sub.get(cov_off..)?;
        let cov_idx = coverage_lookup(cov, glyph_id)? as usize;
        if cov_idx >= count {
            return None;
        }
        let attach_off = read_u16(sub, 4 + cov_idx * 2).ok()? as usize;
        let ap = sub.get(attach_off..)?;
        // AttachPoint: u16 pointCount, u16 pointIndices[pointCount].
        if ap.len() < 2 {
            return None;
        }
        let n = read_u16(ap, 0).ok()? as usize;
        if ap.len() < 2 + n * 2 {
            return None;
        }
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            out.push(read_u16(ap, 2 + i * 2).ok()?);
        }
        Some(out)
    }

    /// Returns the caret coordinates defined for a ligature glyph in
    /// the GDEF `LigCaretList` sub-table.
    ///
    /// Per spec, the number of carets for a ligature equals
    /// `components - 1`, and the array is delivered in increasing
    /// coordinate order. Each [`CaretValue`] is one of the three
    /// `CaretValueFormat` encodings: design-unit (1), contour-point
    /// (2), or design-unit + Device / VariationIndex offset (3).
    pub fn ligature_carets(&self, glyph_id: u16) -> Option<Vec<CaretValue>> {
        let base = self.lig_caret_list_off? as usize;
        let sub = self.bytes.get(base..)?;
        // LigCaretList header: u16 coverageOffset, u16 ligGlyphCount,
        // u16 ligGlyphOffsets[ligGlyphCount].
        if sub.len() < 4 {
            return None;
        }
        let cov_off = read_u16(sub, 0).ok()? as usize;
        let count = read_u16(sub, 2).ok()? as usize;
        let cov = sub.get(cov_off..)?;
        let cov_idx = coverage_lookup(cov, glyph_id)? as usize;
        if cov_idx >= count {
            return None;
        }
        let lg_off = read_u16(sub, 4 + cov_idx * 2).ok()? as usize;
        let lg = sub.get(lg_off..)?;
        // LigGlyph: u16 caretCount, u16 caretValueOffsets[caretCount].
        if lg.len() < 2 {
            return None;
        }
        let caret_count = read_u16(lg, 0).ok()? as usize;
        if lg.len() < 2 + caret_count * 2 {
            return None;
        }
        let mut out = Vec::with_capacity(caret_count);
        for i in 0..caret_count {
            let cv_off = read_u16(lg, 2 + i * 2).ok()? as usize;
            let cv = lg.get(cv_off..)?;
            out.push(parse_caret_value(cv)?);
        }
        Some(out)
    }

    /// Like [`Self::ligature_carets`] but resolves each caret to a
    /// concrete font-unit coordinate at the current variation instance.
    ///
    /// * **Format 1** (`DesignUnits`) → its coordinate, unchanged.
    /// * **Format 3** (`DesignUnitsWithDevice`) → the coordinate plus
    ///   the VariationIndex delta resolved against `ivs` at
    ///   `normalised_coords` (a classic Device table adds nothing at the
    ///   font-unit layer). The device offset is relative to the
    ///   CaretValueFormat3 table base, per the spec.
    /// * **Format 2** (`ContourPoint`) → `None` in that slot: resolving
    ///   a contour-point caret needs the TrueType bytecode interpreter,
    ///   which this crate does not run. The slot is preserved so callers
    ///   keep the caret-index alignment.
    ///
    /// `ivs` is the GDEF `ItemVariationStore` (see
    /// [`Self::item_var_store_bytes`]); pass `None` for a non-variable
    /// font, in which case every Format-3 caret resolves to its static
    /// coordinate.
    pub fn ligature_carets_resolved(
        &self,
        glyph_id: u16,
        ivs: Option<&ItemVariationStore>,
        normalised_coords: &[f32],
    ) -> Option<Vec<Option<i16>>> {
        let base = self.lig_caret_list_off? as usize;
        let sub = self.bytes.get(base..)?;
        if sub.len() < 4 {
            return None;
        }
        let cov_off = read_u16(sub, 0).ok()? as usize;
        let count = read_u16(sub, 2).ok()? as usize;
        let cov = sub.get(cov_off..)?;
        let cov_idx = coverage_lookup(cov, glyph_id)? as usize;
        if cov_idx >= count {
            return None;
        }
        let lg_off = read_u16(sub, 4 + cov_idx * 2).ok()? as usize;
        let lg = sub.get(lg_off..)?;
        if lg.len() < 2 {
            return None;
        }
        let caret_count = read_u16(lg, 0).ok()? as usize;
        if lg.len() < 2 + caret_count * 2 {
            return None;
        }
        let mut out = Vec::with_capacity(caret_count);
        for i in 0..caret_count {
            let cv_off = read_u16(lg, 2 + i * 2).ok()? as usize;
            let cv = lg.get(cv_off..)?;
            out.push(resolve_caret_value(cv, ivs, normalised_coords));
        }
        Some(out)
    }

    /// Number of mark glyph sets defined by the GDEF `MarkGlyphSets`
    /// sub-table (v1.2+). Returns 0 if the table is absent.
    pub fn mark_glyph_set_count(&self) -> u16 {
        self.mark_glyph_set_sub()
            .and_then(|sub| {
                if sub.len() < 4 {
                    return None;
                }
                // u16 format, u16 markGlyphSetCount.
                read_u16(sub, 2).ok()
            })
            .unwrap_or(0)
    }

    /// Tests whether `glyph_id` is a member of mark glyph set
    /// `set_index` (referenced by `lookupFlag.useMarkFilteringSet`).
    /// Returns `false` if the font has no MarkGlyphSets sub-table or
    /// the set index is out of range.
    pub fn mark_glyph_set_contains(&self, set_index: u16, glyph_id: u16) -> bool {
        let sub = match self.mark_glyph_set_sub() {
            Some(s) => s,
            None => return false,
        };
        // u16 format (must be 1), u16 markGlyphSetCount, Offset32
        // coverageOffsets[markGlyphSetCount]. The "format" byte is
        // pinned at 1 in the spec; reject anything else.
        if sub.len() < 4 {
            return false;
        }
        let format = match read_u16(sub, 0) {
            Ok(v) => v,
            Err(_) => return false,
        };
        if format != 1 {
            return false;
        }
        let count = match read_u16(sub, 2) {
            Ok(v) => v as usize,
            Err(_) => return false,
        };
        if set_index as usize >= count {
            return false;
        }
        // Note: spec calls out Offset32, not Offset16.
        let cov_off_pos = 4 + set_index as usize * 4;
        if sub.len() < cov_off_pos + 4 {
            return false;
        }
        let cov_off = match read_u32(sub, cov_off_pos) {
            Ok(v) => v as usize,
            Err(_) => return false,
        };
        let cov = match sub.get(cov_off..) {
            Some(s) => s,
            None => return false,
        };
        coverage_lookup(cov, glyph_id).is_some()
    }

    fn mark_glyph_set_sub(&self) -> Option<&[u8]> {
        let off = self.mark_glyph_sets_def_off? as usize;
        self.bytes.get(off..)
    }

    /// Raw slice of the GDEF `ItemVariationStore` sub-table (v1.3+),
    /// or `None` when the GDEF table predates v1.3 or the offset is
    /// null. The slice starts at the IVS header (u16 format, …) and is
    /// fed verbatim to the shared `ItemVariationStore` decoder.
    pub fn item_var_store_bytes(&self) -> Option<&'a [u8]> {
        let off = self.item_var_store_off? as usize;
        self.bytes.get(off..)
    }
}

/// Decode a CaretValueFormat 1 / 2 / 3 table starting at `bytes[0]`.
fn parse_caret_value(bytes: &[u8]) -> Option<CaretValue> {
    if bytes.len() < 4 {
        return None;
    }
    let format = read_u16(bytes, 0).ok()?;
    match format {
        1 => {
            // u16 format=1, int16 coordinate.
            let v = read_i16(bytes, 2).ok()?;
            Some(CaretValue::DesignUnits(v))
        }
        2 => {
            // u16 format=2, u16 caretValuePointIndex.
            let p = read_u16(bytes, 2).ok()?;
            Some(CaretValue::ContourPoint(p))
        }
        3 => {
            // u16 format=3, int16 coordinate, Offset16 deviceOffset.
            if bytes.len() < 6 {
                return None;
            }
            let v = read_i16(bytes, 2).ok()?;
            let dev = read_u16(bytes, 4).ok()?;
            Some(CaretValue::DesignUnitsWithDevice {
                coordinate: v,
                device_offset: dev,
            })
        }
        _ => None,
    }
}

/// Resolve a CaretValue table (offset 0 at the format word) to a
/// concrete font-unit coordinate at the current instance.
///
/// Returns `None` for Format 2 (contour point — needs the TT bytecode
/// interpreter) or a structurally invalid table. The Format-3 device
/// offset is relative to the CaretValueFormat3 table base (`bytes`).
fn resolve_caret_value(
    bytes: &[u8],
    ivs: Option<&ItemVariationStore>,
    normalised_coords: &[f32],
) -> Option<i16> {
    match parse_caret_value(bytes)? {
        CaretValue::DesignUnits(v) => Some(v),
        CaretValue::ContourPoint(_) => None,
        CaretValue::DesignUnitsWithDevice {
            coordinate,
            device_offset,
        } => {
            let delta = resolve_device_delta(bytes, device_offset, ivs, normalised_coords);
            let rounded = delta.round() as i32;
            Some((coordinate as i32 + rounded).clamp(i16::MIN as i32, i16::MAX as i32) as i16)
        }
    }
}

/// Read an Offset16 at `bytes[off]` and return `None` when it is zero
/// (the spec's "may be NULL" sentinel) or the field falls off the end.
fn nz_off16(bytes: &[u8], off: usize) -> Result<Option<u32>, Error> {
    let raw = read_u16(bytes, off)? as u32;
    Ok(if raw == 0 { None } else { Some(raw) })
}

/// Decode a generic ClassDef table starting at `bytes[0]`.
///
/// Returns the class assigned to `glyph_id`, or 0 when unmatched.
pub(crate) fn class_def_lookup(bytes: &[u8], glyph_id: u16) -> Option<u16> {
    if bytes.len() < 2 {
        return None;
    }
    let format = read_u16(bytes, 0).ok()?;
    match format {
        1 => {
            // Format 1: u16 startGlyphID, u16 glyphCount, u16 classValueArray[glyphCount].
            if bytes.len() < 6 {
                return None;
            }
            let start = read_u16(bytes, 2).ok()?;
            let count = read_u16(bytes, 4).ok()?;
            if glyph_id < start {
                return None;
            }
            let idx = glyph_id - start;
            if idx >= count {
                return None;
            }
            let val = read_u16(bytes, 6 + idx as usize * 2).ok()?;
            if val == 0 {
                None
            } else {
                Some(val)
            }
        }
        2 => {
            // Format 2: u16 classRangeCount, ClassRangeRecord[count]
            // (u16 startGlyph, u16 endGlyph, u16 class).
            if bytes.len() < 4 {
                return None;
            }
            let n = read_u16(bytes, 2).ok()? as usize;
            let header = 4usize;
            // Binary search by startGlyph.
            let mut lo = 0usize;
            let mut hi = n;
            while lo < hi {
                let mid = (lo + hi) / 2;
                let off = header + mid * 6;
                let s = read_u16(bytes, off).ok()?;
                let e = read_u16(bytes, off + 2).ok()?;
                if glyph_id < s {
                    hi = mid;
                } else if glyph_id > e {
                    lo = mid + 1;
                } else {
                    let v = read_u16(bytes, off + 4).ok()?;
                    return if v == 0 { None } else { Some(v) };
                }
            }
            None
        }
        _ => None,
    }
}

/// Coverage table lookup: returns `Some(coverage_index)` if `glyph_id`
/// is covered, or `None` otherwise. The coverage index is needed by
/// e.g. `GSUB` LigatureSubst (it indexes into the LigatureSet array).
pub(crate) fn coverage_lookup(bytes: &[u8], glyph_id: u16) -> Option<u16> {
    if bytes.len() < 4 {
        return None;
    }
    let format = read_u16(bytes, 0).ok()?;
    match format {
        1 => {
            // u16 format=1, u16 glyphCount, u16 glyphArray[glyphCount].
            let count = read_u16(bytes, 2).ok()? as usize;
            // Binary search.
            let mut lo = 0usize;
            let mut hi = count;
            while lo < hi {
                let mid = (lo + hi) / 2;
                let g = read_u16(bytes, 4 + mid * 2).ok()?;
                if g == glyph_id {
                    return Some(mid as u16);
                }
                if g < glyph_id {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            None
        }
        2 => {
            // u16 format=2, u16 rangeCount, RangeRecord[count]
            // (u16 startGlyph, u16 endGlyph, u16 startCoverageIndex).
            let n = read_u16(bytes, 2).ok()? as usize;
            let header = 4usize;
            let mut lo = 0usize;
            let mut hi = n;
            while lo < hi {
                let mid = (lo + hi) / 2;
                let off = header + mid * 6;
                let s = read_u16(bytes, off).ok()?;
                let e = read_u16(bytes, off + 2).ok()?;
                let start_idx = read_u16(bytes, off + 4).ok()?;
                if glyph_id < s {
                    hi = mid;
                } else if glyph_id > e {
                    lo = mid + 1;
                } else {
                    return Some(start_idx + (glyph_id - s));
                }
            }
            None
        }
        _ => None,
    }
}

/// `popcount` of a u16 — used to size ValueRecords.
pub(crate) fn popcount_u16(v: u16) -> usize {
    v.count_ones() as usize
}

/// Parse a top-level `LookupList` table reference: returns the slice
/// for lookup `index`, or `None` if absent.
pub(crate) fn lookup_table_slice(
    table_bytes: &[u8],
    lookup_list_off: u32,
    lookup_index: u16,
) -> Option<&[u8]> {
    let lookup_list = table_bytes.get(lookup_list_off as usize..)?;
    if lookup_list.len() < 2 {
        return None;
    }
    let count = read_u16(lookup_list, 0).ok()?;
    if lookup_index >= count {
        return None;
    }
    let off = read_u16(lookup_list, 2 + lookup_index as usize * 2).ok()? as usize;
    let lookup_off_abs = lookup_list_off as usize + off;
    table_bytes.get(lookup_off_abs..)
}

/// Read an `Offset16` and add to a base, returning a possibly-empty slice
/// reference relative to `parent` (parent_offset bytes from the table
/// start).
#[allow(dead_code)]
pub(crate) fn offset16(table_bytes: &[u8], abs_off: usize) -> Result<u32, Error> {
    Ok(read_u16(table_bytes, abs_off)? as u32)
}

/// Read a u32 — re-export for parity with `read_u32`.
#[allow(dead_code)]
pub(crate) fn offset32(table_bytes: &[u8], abs_off: usize) -> Result<u32, Error> {
    read_u32(table_bytes, abs_off)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_def_format1_lookup() {
        // startGlyph=10, count=3, classes [1,3,2].
        let mut b = vec![0u8; 12];
        b[0..2].copy_from_slice(&1u16.to_be_bytes());
        b[2..4].copy_from_slice(&10u16.to_be_bytes());
        b[4..6].copy_from_slice(&3u16.to_be_bytes());
        b[6..8].copy_from_slice(&1u16.to_be_bytes());
        b[8..10].copy_from_slice(&3u16.to_be_bytes());
        b[10..12].copy_from_slice(&2u16.to_be_bytes());
        assert_eq!(class_def_lookup(&b, 10), Some(1));
        assert_eq!(class_def_lookup(&b, 11), Some(3));
        assert_eq!(class_def_lookup(&b, 12), Some(2));
        assert_eq!(class_def_lookup(&b, 13), None);
        assert_eq!(class_def_lookup(&b, 9), None);
    }

    #[test]
    fn coverage_format1() {
        // Cover glyphs [5, 10, 15].
        let mut b = vec![0u8; 4 + 6];
        b[0..2].copy_from_slice(&1u16.to_be_bytes());
        b[2..4].copy_from_slice(&3u16.to_be_bytes());
        b[4..6].copy_from_slice(&5u16.to_be_bytes());
        b[6..8].copy_from_slice(&10u16.to_be_bytes());
        b[8..10].copy_from_slice(&15u16.to_be_bytes());
        assert_eq!(coverage_lookup(&b, 5), Some(0));
        assert_eq!(coverage_lookup(&b, 10), Some(1));
        assert_eq!(coverage_lookup(&b, 15), Some(2));
        assert_eq!(coverage_lookup(&b, 11), None);
    }

    #[test]
    fn coverage_format2_indexes_correctly() {
        // Range 1: glyphs 10..=12, startCoverageIndex=0.
        // Range 2: glyphs 50..=51, startCoverageIndex=3.
        let mut b = vec![0u8; 4 + 12];
        b[0..2].copy_from_slice(&2u16.to_be_bytes());
        b[2..4].copy_from_slice(&2u16.to_be_bytes());
        b[4..6].copy_from_slice(&10u16.to_be_bytes());
        b[6..8].copy_from_slice(&12u16.to_be_bytes());
        b[8..10].copy_from_slice(&0u16.to_be_bytes());
        b[10..12].copy_from_slice(&50u16.to_be_bytes());
        b[12..14].copy_from_slice(&51u16.to_be_bytes());
        b[14..16].copy_from_slice(&3u16.to_be_bytes());
        assert_eq!(coverage_lookup(&b, 10), Some(0));
        assert_eq!(coverage_lookup(&b, 12), Some(2));
        assert_eq!(coverage_lookup(&b, 50), Some(3));
        assert_eq!(coverage_lookup(&b, 51), Some(4));
        assert_eq!(coverage_lookup(&b, 13), None);
    }

    #[test]
    fn gdef_class_marks_correctly() {
        // Build a tiny GDEF: header + ClassDef format 1 listing one
        // mark glyph (id 99 → class 3).
        let class_def_off: u16 = 12;
        let mut t = vec![0u8; class_def_off as usize];
        t[0..2].copy_from_slice(&1u16.to_be_bytes()); // major
        t[2..4].copy_from_slice(&0u16.to_be_bytes()); // minor
        t[4..6].copy_from_slice(&class_def_off.to_be_bytes()); // GlyphClassDef
                                                               // attachList/ligCaretList/markAttachClassDef offsets: leave 0.
                                                               // ClassDef format 1: startGlyph=99, count=1, [3].
        let mut cd = vec![0u8; 8];
        cd[0..2].copy_from_slice(&1u16.to_be_bytes());
        cd[2..4].copy_from_slice(&99u16.to_be_bytes());
        cd[4..6].copy_from_slice(&1u16.to_be_bytes());
        cd[6..8].copy_from_slice(&3u16.to_be_bytes());
        t.extend_from_slice(&cd);
        let g = GdefTable::parse(&t).unwrap();
        assert!(g.is_mark(99));
        assert!(!g.is_mark(100));
        // Optional sub-tables absent ⇒ accessors return None / 0 / false.
        assert!(g.attach_points(99).is_none());
        assert!(g.ligature_carets(99).is_none());
        assert_eq!(g.mark_attach_class(99), 0);
        assert_eq!(g.mark_glyph_set_count(), 0);
        assert!(!g.mark_glyph_set_contains(0, 99));
    }

    /// Build a v1.0 GDEF table around a single sub-table whose bytes
    /// are supplied by the caller. Layout: [header (12 B)][sub_bytes].
    /// `slot` picks which header offset gets the sub-table:
    ///   - 4 = glyphClassDef
    ///   - 6 = attachList
    ///   - 8 = ligCaretList
    ///   - 10 = markAttachClassDef
    fn build_v10(slot: usize, sub_bytes: &[u8]) -> Vec<u8> {
        let mut t = vec![0u8; 12];
        t[0..2].copy_from_slice(&1u16.to_be_bytes());
        t[2..4].copy_from_slice(&0u16.to_be_bytes());
        let sub_off: u16 = 12;
        t[slot..slot + 2].copy_from_slice(&sub_off.to_be_bytes());
        t.extend_from_slice(sub_bytes);
        t
    }

    #[test]
    fn attach_list_returns_point_indices_in_order() {
        // AttachList layout:
        //   u16 coverageOffset, u16 glyphCount, u16 attachPointOffsets[1]
        //   <coverage> <attach_point_0>
        // Cover only one glyph (gid=42) → coverage index 0 → 3 points.
        let mut sub = Vec::new();
        // Header (6 B): coverageOffset (2) + glyphCount (2) +
        // attachPointOffsets[1] (2). Coverage immediately after, then
        // the AttachPoint.
        let header_len: u16 = 2 + 2 + 2;
        let cov_rel: u16 = header_len;
        let cov_bytes = {
            // Coverage format 1, count=1, glyph=42.
            let mut c = Vec::new();
            c.extend_from_slice(&1u16.to_be_bytes());
            c.extend_from_slice(&1u16.to_be_bytes());
            c.extend_from_slice(&42u16.to_be_bytes());
            c
        };
        let ap_rel: u16 = cov_rel + cov_bytes.len() as u16;
        let ap_bytes = {
            // AttachPoint: pointCount=3, indices [4, 9, 17].
            let mut a = Vec::new();
            a.extend_from_slice(&3u16.to_be_bytes());
            a.extend_from_slice(&4u16.to_be_bytes());
            a.extend_from_slice(&9u16.to_be_bytes());
            a.extend_from_slice(&17u16.to_be_bytes());
            a
        };
        sub.extend_from_slice(&cov_rel.to_be_bytes());
        sub.extend_from_slice(&1u16.to_be_bytes()); // glyphCount
        sub.extend_from_slice(&ap_rel.to_be_bytes()); // attachPointOffsets[0]
        sub.extend_from_slice(&cov_bytes);
        sub.extend_from_slice(&ap_bytes);

        let t = build_v10(6, &sub);
        let g = GdefTable::parse(&t).unwrap();
        assert_eq!(g.attach_points(42), Some(vec![4, 9, 17]));
        // Glyph not in coverage ⇒ None.
        assert!(g.attach_points(7).is_none());
    }

    #[test]
    fn lig_caret_list_mixes_format1_and_format2() {
        // LigCaretList layout for a single ligature glyph (gid=77)
        // carrying two carets — one Format 1 (design units = -50),
        // one Format 2 (contour point = 12).
        let mut sub = Vec::new();
        // LigCaretList header: coverageOffset (2) + ligGlyphCount (2) +
        // ligGlyphOffsets[1] (2) = 6 B.
        let header_len: u16 = 2 + 2 + 2;
        let cov_rel: u16 = header_len;
        let cov_bytes = {
            let mut c = Vec::new();
            c.extend_from_slice(&1u16.to_be_bytes()); // format 1
            c.extend_from_slice(&1u16.to_be_bytes()); // count
            c.extend_from_slice(&77u16.to_be_bytes());
            c
        };
        let lg_rel: u16 = cov_rel + cov_bytes.len() as u16;
        // LigGlyph: caretCount=2, caretValueOffsets[0], [1].
        let lg_header_len: u16 = 2 + 2 * 2;
        let cv0_rel: u16 = lg_header_len;
        let cv0_bytes = {
            let mut c = Vec::new();
            c.extend_from_slice(&1u16.to_be_bytes()); // format 1
            c.extend_from_slice(&(-50i16).to_be_bytes()); // coord
            c
        };
        let cv1_rel: u16 = cv0_rel + cv0_bytes.len() as u16;
        let cv1_bytes = {
            let mut c = Vec::new();
            c.extend_from_slice(&2u16.to_be_bytes()); // format 2
            c.extend_from_slice(&12u16.to_be_bytes()); // contour point
            c
        };
        let mut lg_bytes = Vec::new();
        lg_bytes.extend_from_slice(&2u16.to_be_bytes()); // caretCount
        lg_bytes.extend_from_slice(&cv0_rel.to_be_bytes());
        lg_bytes.extend_from_slice(&cv1_rel.to_be_bytes());
        lg_bytes.extend_from_slice(&cv0_bytes);
        lg_bytes.extend_from_slice(&cv1_bytes);

        sub.extend_from_slice(&cov_rel.to_be_bytes());
        sub.extend_from_slice(&1u16.to_be_bytes()); // ligGlyphCount
        sub.extend_from_slice(&lg_rel.to_be_bytes()); // ligGlyphOffsets[0]
        sub.extend_from_slice(&cov_bytes);
        sub.extend_from_slice(&lg_bytes);

        let t = build_v10(8, &sub);
        let g = GdefTable::parse(&t).unwrap();
        let carets = g.ligature_carets(77).unwrap();
        assert_eq!(
            carets,
            vec![CaretValue::DesignUnits(-50), CaretValue::ContourPoint(12),]
        );
        assert!(g.ligature_carets(0).is_none());
    }

    #[test]
    fn lig_caret_format3_preserves_device_offset() {
        let mut sub = Vec::new();
        // LigCaretList header (1 ligGlyph): coverageOffset (2) +
        // ligGlyphCount (2) + ligGlyphOffsets[1] (2) = 6 B.
        let header_len: u16 = 2 + 2 + 2;
        let cov_rel: u16 = header_len;
        let cov_bytes = {
            let mut c = Vec::new();
            c.extend_from_slice(&1u16.to_be_bytes());
            c.extend_from_slice(&1u16.to_be_bytes());
            c.extend_from_slice(&5u16.to_be_bytes());
            c
        };
        let lg_rel: u16 = cov_rel + cov_bytes.len() as u16;
        // LigGlyph header (1 caret): caretCount (2) +
        // caretValueOffsets[1] (2) = 4 B.
        let lg_header_len: u16 = 2 + 2;
        let cv0_rel: u16 = lg_header_len;
        let cv0_bytes = {
            let mut c = Vec::new();
            c.extend_from_slice(&3u16.to_be_bytes()); // format 3
            c.extend_from_slice(&123i16.to_be_bytes()); // coord
            c.extend_from_slice(&0xCAFEu16.to_be_bytes()); // device offset
            c
        };
        let mut lg_bytes = Vec::new();
        lg_bytes.extend_from_slice(&1u16.to_be_bytes());
        lg_bytes.extend_from_slice(&cv0_rel.to_be_bytes());
        lg_bytes.extend_from_slice(&cv0_bytes);

        sub.extend_from_slice(&cov_rel.to_be_bytes());
        sub.extend_from_slice(&1u16.to_be_bytes());
        sub.extend_from_slice(&lg_rel.to_be_bytes());
        sub.extend_from_slice(&cov_bytes);
        sub.extend_from_slice(&lg_bytes);

        let t = build_v10(8, &sub);
        let g = GdefTable::parse(&t).unwrap();
        let carets = g.ligature_carets(5).unwrap();
        assert_eq!(carets.len(), 1);
        match carets[0] {
            CaretValue::DesignUnitsWithDevice {
                coordinate,
                device_offset,
            } => {
                assert_eq!(coordinate, 123);
                assert_eq!(device_offset, 0xCAFE);
            }
            _ => panic!("expected DesignUnitsWithDevice variant"),
        }
    }

    /// Build a standalone single-region IVS (rising-edge region peaking
    /// at +1, one IVD row carrying `[delta]`) — same shape used across
    /// the variation-table tests.
    fn build_single_region_ivs(delta: i16) -> Vec<u8> {
        let mut b = vec![0u8; 32];
        b[0..2].copy_from_slice(&1u16.to_be_bytes());
        b[2..6].copy_from_slice(&12u32.to_be_bytes());
        b[6..8].copy_from_slice(&1u16.to_be_bytes());
        b[8..12].copy_from_slice(&22u32.to_be_bytes());
        b[12..14].copy_from_slice(&1u16.to_be_bytes());
        b[14..16].copy_from_slice(&1u16.to_be_bytes());
        b[16..18].copy_from_slice(&0i16.to_be_bytes());
        b[18..20].copy_from_slice(&16384i16.to_be_bytes());
        b[20..22].copy_from_slice(&16384i16.to_be_bytes());
        b[22..24].copy_from_slice(&1u16.to_be_bytes());
        b[24..26].copy_from_slice(&1u16.to_be_bytes());
        b[26..28].copy_from_slice(&1u16.to_be_bytes());
        b[28..30].copy_from_slice(&0u16.to_be_bytes());
        b[30..32].copy_from_slice(&delta.to_be_bytes());
        b
    }

    #[test]
    fn lig_caret_format3_resolves_variation_index() {
        // ligCaretList for ligature glyph 5 with one Format-3 caret:
        //   coord = 200, device offset → a VariationIndex (0, 0).
        let cov_rel: u16 = 6; // after 6-byte ligCaretList header
        let cov_bytes = {
            let mut c = Vec::new();
            c.extend_from_slice(&1u16.to_be_bytes());
            c.extend_from_slice(&1u16.to_be_bytes());
            c.extend_from_slice(&5u16.to_be_bytes());
            c
        };
        let lg_rel: u16 = cov_rel + cov_bytes.len() as u16;
        // Format-3 CaretValue table: format(2) + coord(2) + devOff(2) +
        // VariationIndex{outer,inner,fmt}(6). devOff = 6 (caret-relative).
        let cv0_bytes = {
            let mut c = Vec::new();
            c.extend_from_slice(&3u16.to_be_bytes()); // format 3
            c.extend_from_slice(&200i16.to_be_bytes()); // coord
            c.extend_from_slice(&6u16.to_be_bytes()); // device offset
            c.extend_from_slice(&0u16.to_be_bytes()); // VarIdx.outer
            c.extend_from_slice(&0u16.to_be_bytes()); // VarIdx.inner
            c.extend_from_slice(&0x8000u16.to_be_bytes()); // deltaFormat
            c
        };
        let cv0_rel: u16 = 4; // after 4-byte LigGlyph header
        let mut lg_bytes = Vec::new();
        lg_bytes.extend_from_slice(&1u16.to_be_bytes()); // caretCount
        lg_bytes.extend_from_slice(&cv0_rel.to_be_bytes());
        lg_bytes.extend_from_slice(&cv0_bytes);

        let mut sub = Vec::new();
        sub.extend_from_slice(&cov_rel.to_be_bytes());
        sub.extend_from_slice(&1u16.to_be_bytes()); // ligGlyphCount
        sub.extend_from_slice(&lg_rel.to_be_bytes()); // ligGlyphOffsets[0]
        sub.extend_from_slice(&cov_bytes);
        sub.extend_from_slice(&lg_bytes);

        // GDEF v1.3 header (18 bytes): ligCaretList at slot 8,
        // itemVarStore Offset32 at 14.
        let mut t = vec![0u8; 18];
        t[0..2].copy_from_slice(&1u16.to_be_bytes());
        t[2..4].copy_from_slice(&3u16.to_be_bytes()); // minor 1.3
        let lig_off: u16 = 18;
        t[8..10].copy_from_slice(&lig_off.to_be_bytes());
        let ivs_off: u32 = 18 + sub.len() as u32;
        t[14..18].copy_from_slice(&ivs_off.to_be_bytes());
        t.extend_from_slice(&sub);
        t.extend_from_slice(&build_single_region_ivs(60));

        let g = GdefTable::parse(&t).unwrap();
        let ivs_bytes = g.item_var_store_bytes().unwrap();
        let ivs = crate::tables::mvar::ItemVariationStore::parse(ivs_bytes).unwrap();

        // Static (unresolved) variant still surfaces the raw offset.
        let raw = g.ligature_carets(5).unwrap();
        assert!(matches!(
            raw[0],
            CaretValue::DesignUnitsWithDevice {
                coordinate: 200,
                ..
            }
        ));

        // Resolved: default instance → 200, max → 260, half → 230.
        let at0 = g.ligature_carets_resolved(5, Some(&ivs), &[0.0]).unwrap();
        assert_eq!(at0, vec![Some(200)]);
        let at1 = g.ligature_carets_resolved(5, Some(&ivs), &[1.0]).unwrap();
        assert_eq!(at1, vec![Some(260)]);
        let at_half = g.ligature_carets_resolved(5, Some(&ivs), &[0.5]).unwrap();
        assert_eq!(at_half, vec![Some(230)]);
        // No IVS → static coordinate.
        let no_ivs = g.ligature_carets_resolved(5, None, &[1.0]).unwrap();
        assert_eq!(no_ivs, vec![Some(200)]);
    }

    #[test]
    fn mark_attach_class_reads_high_byte_class() {
        // ClassDef format 2 — one range (gid 100..=110) → class 7.
        let mut cd = Vec::new();
        cd.extend_from_slice(&2u16.to_be_bytes()); // format
        cd.extend_from_slice(&1u16.to_be_bytes()); // classRangeCount
        cd.extend_from_slice(&100u16.to_be_bytes());
        cd.extend_from_slice(&110u16.to_be_bytes());
        cd.extend_from_slice(&7u16.to_be_bytes());

        let t = build_v10(10, &cd);
        let g = GdefTable::parse(&t).unwrap();
        assert_eq!(g.mark_attach_class(99), 0);
        assert_eq!(g.mark_attach_class(100), 7);
        assert_eq!(g.mark_attach_class(110), 7);
        assert_eq!(g.mark_attach_class(111), 0);
    }

    #[test]
    fn mark_glyph_sets_handles_two_sets_with_offset32() {
        // Build a v1.2 GDEF header (14 bytes) → MarkGlyphSets sub-table.
        // Set 0 covers glyphs [3, 4]; set 1 covers glyphs [3, 5].
        let mut t = vec![0u8; 14];
        t[0..2].copy_from_slice(&1u16.to_be_bytes()); // major
        t[2..4].copy_from_slice(&2u16.to_be_bytes()); // minor 1.2
                                                      // glyphClassDef / attachList / ligCaretList / markAttach all NULL.
        let mgs_off: u16 = 14;
        t[12..14].copy_from_slice(&mgs_off.to_be_bytes());

        // MarkGlyphSets header: u16 format=1, u16 count=2, Offset32 cov[2].
        let mut mgs = Vec::new();
        mgs.extend_from_slice(&1u16.to_be_bytes());
        mgs.extend_from_slice(&2u16.to_be_bytes());
        let header_len: u32 = 4 + 4 * 2;
        let cov0_rel: u32 = header_len;
        let cov0 = {
            let mut c = Vec::new();
            c.extend_from_slice(&1u16.to_be_bytes()); // format 1
            c.extend_from_slice(&2u16.to_be_bytes()); // count
            c.extend_from_slice(&3u16.to_be_bytes());
            c.extend_from_slice(&4u16.to_be_bytes());
            c
        };
        let cov1_rel: u32 = cov0_rel + cov0.len() as u32;
        let cov1 = {
            let mut c = Vec::new();
            c.extend_from_slice(&1u16.to_be_bytes());
            c.extend_from_slice(&2u16.to_be_bytes());
            c.extend_from_slice(&3u16.to_be_bytes());
            c.extend_from_slice(&5u16.to_be_bytes());
            c
        };
        mgs.extend_from_slice(&cov0_rel.to_be_bytes());
        mgs.extend_from_slice(&cov1_rel.to_be_bytes());
        mgs.extend_from_slice(&cov0);
        mgs.extend_from_slice(&cov1);

        t.extend_from_slice(&mgs);
        let g = GdefTable::parse(&t).unwrap();
        assert_eq!(g.mark_glyph_set_count(), 2);
        assert!(g.mark_glyph_set_contains(0, 3));
        assert!(g.mark_glyph_set_contains(0, 4));
        assert!(!g.mark_glyph_set_contains(0, 5));
        assert!(g.mark_glyph_set_contains(1, 3));
        assert!(g.mark_glyph_set_contains(1, 5));
        assert!(!g.mark_glyph_set_contains(1, 4));
        // Out-of-range set index ⇒ false (no panic).
        assert!(!g.mark_glyph_set_contains(2, 3));
    }

    #[test]
    fn v13_header_with_item_var_store_exposes_raw_slice() {
        // Header for v1.3: 18 bytes (major, minor, four Offset16, one
        // Offset16 markGlyphSetsDefOffset, one Offset32 itemVarStoreOffset).
        let mut t = vec![0u8; 18];
        t[0..2].copy_from_slice(&1u16.to_be_bytes());
        t[2..4].copy_from_slice(&3u16.to_be_bytes()); // minor 1.3
        let ivs_off: u32 = 18;
        t[14..18].copy_from_slice(&ivs_off.to_be_bytes());
        // IVS payload: dummy marker bytes so we can test slice retrieval.
        t.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let g = GdefTable::parse(&t).unwrap();
        assert_eq!(
            g.item_var_store_bytes(),
            Some(&[0xDE, 0xAD, 0xBE, 0xEF][..])
        );
    }

    #[test]
    fn rejects_truncated_v12_header() {
        // Minor=2 but only 13 bytes ⇒ header read of the
        // markGlyphSetsDefOffset would walk past the buffer.
        let mut t = vec![0u8; 13];
        t[0..2].copy_from_slice(&1u16.to_be_bytes());
        t[2..4].copy_from_slice(&2u16.to_be_bytes());
        // v1.0 portion has 12 bytes; we left byte 13 zeroed so v1.0
        // length-check passes, but the v1.2 nz_off16 at byte 12 needs
        // the byte at offset 13 too — make the input one byte short.
        let _ = GdefTable::parse(&t[..12]);
        let err = GdefTable::parse(&t).err();
        // Either UnexpectedEof from the inner read or the explicit guard.
        assert!(matches!(err, Some(Error::UnexpectedEof)));
    }

    #[test]
    fn rejects_unknown_major_version() {
        let mut t = vec![0u8; 12];
        t[0..2].copy_from_slice(&2u16.to_be_bytes());
        let err = GdefTable::parse(&t).unwrap_err();
        assert!(matches!(err, Error::BadStructure(_)));
    }
}
