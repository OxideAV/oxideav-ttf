//! `GDEF` — Glyph Definition Table.
//!
//! Round 1 only consumes `glyphClassDef` so GPOS can skip mark glyphs
//! when looking up a kerning pair. Per spec, GlyphClassDef is a generic
//! ClassDef table whose values are `1=Base, 2=Ligature, 3=Mark,
//! 4=Component`. Glyphs not enumerated default to class 0.

use crate::parser::{read_u16, read_u32};
use crate::Error;

/// Predefined glyph classes per the GDEF spec.
#[allow(dead_code)]
pub const CLASS_BASE: u16 = 1;
#[allow(dead_code)]
pub const CLASS_LIGATURE: u16 = 2;
pub const CLASS_MARK: u16 = 3;
#[allow(dead_code)]
pub const CLASS_COMPONENT: u16 = 4;

#[derive(Debug, Clone)]
pub struct GdefTable<'a> {
    bytes: &'a [u8],
    glyph_class_def_off: Option<u32>,
}

impl<'a> GdefTable<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        // Header version: u16 major, u16 minor; offsets to subtables.
        if bytes.len() < 12 {
            return Err(Error::UnexpectedEof);
        }
        let major = read_u16(bytes, 0)?;
        let minor = read_u16(bytes, 2)?;
        if major != 1 {
            return Err(Error::BadStructure("GDEF: unsupported version"));
        }
        let _ = minor; // we tolerate any minor.
        let glyph_class_def_off = read_u16(bytes, 4)? as u32;
        let glyph_class_def_off = if glyph_class_def_off == 0 {
            None
        } else {
            Some(glyph_class_def_off)
        };
        Ok(Self {
            bytes,
            glyph_class_def_off,
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
    }
}
