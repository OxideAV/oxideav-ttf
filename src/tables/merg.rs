//! `MERG` — Merge Table (ISO/IEC 14496-22:2019 §5.7.5).
//!
//! The `MERG` table declares, for pairs of glyph *merge classes*, whether a
//! renderer should merge (or group) the glyphs before antialias filtering
//! to avoid rendering artefacts where glyphs visually interact. Glyphs are
//! sorted into merge classes by one or more OFF-layout ClassDef tables, and
//! a square `mergeClassCount × mergeClassCount` array of `uint8` merge-entry
//! bit-fields gives the behaviour for each ordered `(firstClass,
//! secondClass)` pair.
//!
//! ```text
//! Merge header
//!   uint16   version                 (0)
//!   uint16   mergeClassCount
//!   Offset16 mergeDataOffset         (to the mergeClassCount² entry array)
//!   uint16   classDefCount
//!   Offset16 offsetToClassDefOffsets (to an array of Offset16 to ClassDefs)
//!
//! Merge entry (one uint8 per (firstClass, secondClass) cell):
//!   0x01 MERGE_LTR
//!   0x02 GROUP_LTR
//!   0x04 SECOND_IS_SUBORDINATE_LTR
//!   0x08 reserved
//!   0x10 MERGE_RTL
//!   0x20 GROUP_RTL
//!   0x40 SECOND_IS_SUBORDINATE_RTL
//!   0x80 reserved
//! ```
//!
//! This module decodes the table structure and resolves a glyph to its
//! merge class (consulting every ClassDef table in order, with class 0 the
//! implicit default) and a `(firstClass, secondClass)` pair to its
//! merge-entry byte. The §5.7.5.3 stateful run-processing algorithm (which
//! consumes these entries to decide which glyph *sequences* get
//! antialiased together) is a renderer concern left to the consumer crate.

use crate::parser::read_u16;
use crate::tables::gdef::class_def_lookup;
use crate::Error;

/// `MERGE_LTR` — merge the glyph pair before antialiasing, for left-to-right
/// visual order.
pub const MERGE_LTR: u8 = 0x01;
/// `GROUP_LTR` — treat the pair as a unit (without yet deciding merge), LTR.
pub const GROUP_LTR: u8 = 0x02;
/// `SECOND_IS_SUBORDINATE_LTR` — the merged/grouped sequence takes the merge
/// class of the *first* element rather than the second, LTR.
pub const SECOND_IS_SUBORDINATE_LTR: u8 = 0x04;
/// `MERGE_RTL` — merge the glyph pair before antialiasing, right-to-left.
pub const MERGE_RTL: u8 = 0x10;
/// `GROUP_RTL` — treat the pair as a unit, RTL.
pub const GROUP_RTL: u8 = 0x20;
/// `SECOND_IS_SUBORDINATE_RTL` — the sequence takes the first element's
/// class, RTL.
pub const SECOND_IS_SUBORDINATE_RTL: u8 = 0x40;

/// A single merge-entry byte for an ordered `(firstClass, secondClass)`
/// pair, with the six defined flags decoded through accessors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeEntry(pub u8);

impl MergeEntry {
    /// `MERGE_LTR`: merge for left-to-right visual order.
    pub fn merge_ltr(self) -> bool {
        self.0 & MERGE_LTR != 0
    }
    /// `GROUP_LTR`: group as a unit for left-to-right visual order.
    pub fn group_ltr(self) -> bool {
        self.0 & GROUP_LTR != 0
    }
    /// `SECOND_IS_SUBORDINATE_LTR`. Per the spec this is honoured only when
    /// the LTR merge or group flag is also set; otherwise it is ignored.
    pub fn second_is_subordinate_ltr(self) -> bool {
        self.0 & SECOND_IS_SUBORDINATE_LTR != 0
    }
    /// `MERGE_RTL`: merge for right-to-left visual order.
    pub fn merge_rtl(self) -> bool {
        self.0 & MERGE_RTL != 0
    }
    /// `GROUP_RTL`: group as a unit for right-to-left visual order.
    pub fn group_rtl(self) -> bool {
        self.0 & GROUP_RTL != 0
    }
    /// `SECOND_IS_SUBORDINATE_RTL`. Honoured only when the RTL merge or
    /// group flag is also set.
    pub fn second_is_subordinate_rtl(self) -> bool {
        self.0 & SECOND_IS_SUBORDINATE_RTL != 0
    }
}

/// Parsed `MERG` table.
#[derive(Debug, Clone)]
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub struct MergTable {
    merge_class_count: usize,
    /// The flattened `mergeClassCount × mergeClassCount` merge-entry array,
    /// row-major (row = first class, column = second class).
    entries: Vec<u8>,
    /// ClassDef tables, each holding its own byte slice copied out at parse
    /// time so the table carries no lifetime.
    class_defs: Vec<Vec<u8>>,
}

impl MergTable {
    /// Decode a `MERG` table from its byte slice.
    ///
    /// Validates `version == 0` and bounds-checks the merge-entry array
    /// (size `mergeClassCount²`) and every ClassDef offset against the
    /// table. ClassDef table byte ranges are taken up to the next ClassDef
    /// offset (or the merge-data array / table end), so each ClassDef parses
    /// against an in-bounds slice. Returns `BadStructure` for a malformed
    /// layout — per §5.7.5.2 a malformed `MERG` is ignored, which the caller
    /// achieves by discarding the error.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 10 {
            return Err(Error::UnexpectedEof);
        }
        let version = read_u16(bytes, 0)?;
        if version != 0 {
            return Err(Error::BadStructure("MERG: unsupported version"));
        }
        let merge_class_count = read_u16(bytes, 2)? as usize;
        let merge_data_offset = read_u16(bytes, 4)? as usize;
        let class_def_count = read_u16(bytes, 6)? as usize;
        let offset_to_class_def_offsets = read_u16(bytes, 8)? as usize;

        // Merge-entry array: mergeClassCount² uint8 cells at mergeDataOffset.
        let entry_count = merge_class_count
            .checked_mul(merge_class_count)
            .ok_or(Error::BadStructure("MERG: class count overflow"))?;
        let entries = if entry_count == 0 {
            Vec::new()
        } else {
            let end = merge_data_offset
                .checked_add(entry_count)
                .ok_or(Error::BadStructure("MERG: merge-data overflow"))?;
            if merge_data_offset == 0 || end > bytes.len() {
                return Err(Error::BadStructure("MERG: merge-data out of bounds"));
            }
            bytes[merge_data_offset..end].to_vec()
        };

        // ClassDef offsets array: classDefCount Offset16 entries at
        // offsetToClassDefOffsets.
        let mut class_defs = Vec::with_capacity(class_def_count);
        if class_def_count != 0 {
            let arr_end = offset_to_class_def_offsets
                .checked_add(class_def_count * 2)
                .ok_or(Error::BadStructure("MERG: class-def offset array overflow"))?;
            if offset_to_class_def_offsets == 0 || arr_end > bytes.len() {
                return Err(Error::BadStructure("MERG: class-def offsets out of bounds"));
            }
            for i in 0..class_def_count {
                let off = read_u16(bytes, offset_to_class_def_offsets + i * 2)? as usize;
                if off == 0 || off > bytes.len() {
                    return Err(Error::BadStructure("MERG: class-def offset out of bounds"));
                }
                // The ClassDef slice runs to the table end; the ClassDef
                // parser reads only the bytes it needs.
                class_defs.push(bytes[off..].to_vec());
            }
        }

        Ok(Self {
            merge_class_count,
            entries,
            class_defs,
        })
    }

    /// The number of merge classes (`mergeClassCount`). Merge entries exist
    /// for classes `0..merge_class_count`.
    pub fn merge_class_count(&self) -> usize {
        self.merge_class_count
    }

    /// The number of ClassDef tables the `MERG` table carries.
    pub fn class_def_count(&self) -> usize {
        self.class_defs.len()
    }

    /// Resolve a glyph to its merge class. Consults every ClassDef table in
    /// order; the first table that assigns the glyph a non-zero class wins
    /// (the spec requires each glyph to be in at most one class). Glyphs
    /// assigned to no table take the implicit class 0.
    pub fn merge_class(&self, glyph_id: u16) -> u16 {
        for cd in &self.class_defs {
            if let Some(c) = class_def_lookup(cd, glyph_id) {
                return c;
            }
        }
        0
    }

    /// The merge entry for an ordered class pair (`first` = row,
    /// `second` = column). Returns `None` when either class is out of range
    /// (`>= merge_class_count`), in which case no merge data applies and the
    /// renderer treats the pair as never requiring a merge per the §5.7.5.2
    /// NOTE 1.
    pub fn merge_entry(&self, first: u16, second: u16) -> Option<MergeEntry> {
        let f = first as usize;
        let s = second as usize;
        if f >= self.merge_class_count || s >= self.merge_class_count {
            return None;
        }
        self.entries
            .get(f * self.merge_class_count + s)
            .copied()
            .map(MergeEntry)
    }

    /// Convenience: the merge entry for an ordered *glyph* pair, resolving
    /// each glyph's merge class first.
    pub fn merge_entry_for_glyphs(&self, first: u16, second: u16) -> Option<MergeEntry> {
        self.merge_entry(self.merge_class(first), self.merge_class(second))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a MERG with `class_count` merge classes, the given row-major
    /// merge-entry bytes, and a single ClassDefFormat1 mapping glyphs
    /// `[first_glyph..first_glyph+classes.len())` to `classes`.
    fn build_merg(class_count: u16, entries: &[u8], first_glyph: u16, classes: &[u16]) -> Vec<u8> {
        // Header (10 bytes) | classDef offsets array | ClassDef | merge data.
        let mut out = vec![0u8; 10];
        let cd_offsets_off = 10u16;
        // One classDef offset entry.
        let cd_off = cd_offsets_off + 2;
        let class_def_start = cd_off as usize;
        // ClassDefFormat1: format(2) startGlyph(2) count(2) values[count].
        let mut cd = Vec::new();
        cd.extend_from_slice(&1u16.to_be_bytes());
        cd.extend_from_slice(&first_glyph.to_be_bytes());
        cd.extend_from_slice(&(classes.len() as u16).to_be_bytes());
        for &c in classes {
            cd.extend_from_slice(&c.to_be_bytes());
        }
        // Lay out: header | offsets array (2 bytes) | classdef | merge data.
        out.extend_from_slice(&cd_off.to_be_bytes()); // the single classDef offset
        let cd_at = out.len();
        debug_assert_eq!(cd_at, class_def_start);
        out.extend_from_slice(&cd);
        let merge_data_off = out.len() as u16;
        out.extend_from_slice(entries);

        // Patch header.
        out[0..2].copy_from_slice(&0u16.to_be_bytes()); // version
        out[2..4].copy_from_slice(&class_count.to_be_bytes());
        out[4..6].copy_from_slice(&merge_data_off.to_be_bytes());
        out[6..8].copy_from_slice(&1u16.to_be_bytes()); // classDefCount
        out[8..10].copy_from_slice(&cd_offsets_off.to_be_bytes());
        out
    }

    #[test]
    fn decodes_classes_and_entries() {
        // 2 merge classes; entry array [r0c0, r0c1, r1c0, r1c1].
        // Mark r1c1 = MERGE_LTR | SECOND_IS_SUBORDINATE_RTL.
        let entries = [0x00, 0x00, 0x00, MERGE_LTR | SECOND_IS_SUBORDINATE_RTL];
        // glyphs 10,11 -> classes 1,0 (others default 0).
        let bytes = build_merg(2, &entries, 10, &[1, 0]);
        let m = MergTable::parse(&bytes).unwrap();
        assert_eq!(m.merge_class_count(), 2);
        assert_eq!(m.class_def_count(), 1);
        assert_eq!(m.merge_class(10), 1);
        assert_eq!(m.merge_class(11), 0); // ClassDef value 0 -> class 0
        assert_eq!(m.merge_class(99), 0); // unmapped -> class 0
        let e = m.merge_entry(1, 1).unwrap();
        assert!(e.merge_ltr());
        assert!(e.second_is_subordinate_rtl());
        assert!(!e.merge_rtl());
        // (0,0) entry is empty.
        assert_eq!(m.merge_entry(0, 0).unwrap(), MergeEntry(0));
        // Out-of-range class -> None.
        assert!(m.merge_entry(2, 0).is_none());
    }

    #[test]
    fn merge_entry_for_glyphs_resolves_classes() {
        // glyph 10 -> class 1, glyph 20 -> class 0.
        let entries = [0, 0, GROUP_RTL, 0]; // r1c0 = GROUP_RTL
        let bytes = build_merg(2, &entries, 10, &[1]);
        let m = MergTable::parse(&bytes).unwrap();
        // (glyph10=class1, glyph20=class0) -> entry r1c0 = GROUP_RTL.
        let e = m.merge_entry_for_glyphs(10, 20).unwrap();
        assert!(e.group_rtl());
        assert!(!e.merge_ltr());
    }

    #[test]
    fn empty_merge_class_count() {
        // mergeClassCount = 0: no entries, no classes.
        let mut out = vec![0u8; 10];
        out[8..10].copy_from_slice(&0u16.to_be_bytes());
        let m = MergTable::parse(&out).unwrap();
        assert_eq!(m.merge_class_count(), 0);
        assert!(m.merge_entry(0, 0).is_none());
        assert_eq!(m.merge_class(5), 0);
    }

    #[test]
    fn rejects_bad_version() {
        let mut bytes = build_merg(2, &[0, 0, 0, 0], 10, &[1, 0]);
        bytes[0..2].copy_from_slice(&1u16.to_be_bytes());
        assert!(matches!(
            MergTable::parse(&bytes),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn rejects_merge_data_out_of_bounds() {
        let mut bytes = build_merg(2, &[0, 0, 0, 0], 10, &[1, 0]);
        // Patch mergeDataOffset to past the table.
        bytes[4..6].copy_from_slice(&9999u16.to_be_bytes());
        assert!(matches!(
            MergTable::parse(&bytes),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn rejects_short_header() {
        assert!(matches!(
            MergTable::parse(&[0u8; 6]),
            Err(Error::UnexpectedEof)
        ));
    }
}
