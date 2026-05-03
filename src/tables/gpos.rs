//! `GPOS` — Glyph Positioning Table.
//!
//! Round 1 only walks LookupType 2 (Pair Adjustment Positioning) sub-tables,
//! which is what fonts use to encode kerning. Both PairPosFormat1 (per-pair
//! adjustments via Coverage + PairSet) and PairPosFormat2 (class-pair grid)
//! are supported. We extract only the `xAdvance` adjustment of the first
//! glyph in the pair — that's all "kerning" means for our consumer crate.
//!
//! ExtensionPos (LookupType 9) is unwrapped transparently.

use crate::parser::{read_i16, read_u16, read_u32};
use crate::tables::gdef::{
    class_def_lookup, coverage_lookup, lookup_table_slice, popcount_u16, GdefTable,
};
use crate::Error;

const LOOKUP_PAIR_POS: u16 = 2;
const LOOKUP_EXTENSION_POS: u16 = 9;

// ValueFormat bits (low byte holds the four geometric flags).
const VF_X_PLACEMENT: u16 = 0x0001;
const VF_Y_PLACEMENT: u16 = 0x0002;
const VF_X_ADVANCE: u16 = 0x0004;
#[allow(dead_code)]
const VF_Y_ADVANCE: u16 = 0x0008;

#[derive(Debug, Clone)]
pub struct GposTable<'a> {
    bytes: &'a [u8],
    lookup_list_off: u32,
}

impl<'a> GposTable<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < 10 {
            return Err(Error::UnexpectedEof);
        }
        let major = read_u16(bytes, 0)?;
        if major != 1 {
            return Err(Error::BadStructure("GPOS: unsupported major version"));
        }
        let lookup_list_off = read_u16(bytes, 8)? as u32;
        if lookup_list_off as usize >= bytes.len() {
            return Err(Error::BadOffset);
        }
        Ok(Self {
            bytes,
            lookup_list_off,
        })
    }

    /// Look up the kerning adjustment between an ordered glyph pair.
    /// Returns `xAdvance` of the first glyph's ValueRecord1 — the only
    /// field a kerning lookup is ever expected to set.
    ///
    /// `gdef` is consulted to skip mark glyphs per the spec's
    /// IGNORE_MARKS lookup-flag. Round 1 honours IGNORE_MARKS by simply
    /// refusing to attempt a lookup whose left or right glyph is a mark
    /// (per the spec, marks shouldn't kern with bases anyway).
    pub fn lookup_kerning(&self, left: u16, right: u16, gdef: Option<&GdefTable<'_>>) -> i16 {
        let lookup_list = match self.bytes.get(self.lookup_list_off as usize..) {
            Some(s) => s,
            None => return 0,
        };
        if lookup_list.len() < 2 {
            return 0;
        }
        let lookup_count = match read_u16(lookup_list, 0) {
            Ok(c) => c,
            Err(_) => return 0,
        };

        for i in 0..lookup_count {
            let lookup = match lookup_table_slice(self.bytes, self.lookup_list_off, i) {
                Some(s) => s,
                None => continue,
            };
            if lookup.len() < 6 {
                continue;
            }
            let kind = match read_u16(lookup, 0) {
                Ok(k) => k,
                Err(_) => continue,
            };
            let flag = read_u16(lookup, 2).unwrap_or(0);
            let ignore_marks = (flag & 0x0008) != 0;
            if ignore_marks {
                if let Some(g) = gdef {
                    if g.is_mark(left) || g.is_mark(right) {
                        continue;
                    }
                }
            }
            let sub_count = read_u16(lookup, 4).unwrap_or(0) as usize;
            for s in 0..sub_count {
                let sub_off = match read_u16(lookup, 6 + s * 2) {
                    Ok(o) => o as usize,
                    Err(_) => continue,
                };
                let sub = match lookup.get(sub_off..) {
                    Some(b) => b,
                    None => continue,
                };
                let (effective_kind, effective_sub) = if kind == LOOKUP_EXTENSION_POS {
                    if sub.len() < 8 {
                        continue;
                    }
                    let ext_type = read_u16(sub, 2).unwrap_or(0);
                    let ext_off = read_u32(sub, 4).unwrap_or(0) as usize;
                    let ext = match sub.get(ext_off..) {
                        Some(s) => s,
                        None => continue,
                    };
                    (ext_type, ext)
                } else {
                    (kind, sub)
                };
                if effective_kind != LOOKUP_PAIR_POS {
                    continue;
                }
                if let Some(v) = pair_pos_lookup(effective_sub, left, right) {
                    return v;
                }
            }
        }
        0
    }
}

/// Walk a PairPos subtable (format 1 or 2) looking for `(left, right)`.
fn pair_pos_lookup(sub: &[u8], left: u16, right: u16) -> Option<i16> {
    if sub.len() < 8 {
        return None;
    }
    let format = read_u16(sub, 0).ok()?;
    let coverage_off = read_u16(sub, 2).ok()? as usize;
    let value_format1 = read_u16(sub, 4).ok()?;
    let value_format2 = read_u16(sub, 6).ok()?;
    let cov = sub.get(coverage_off..)?;
    let cov_idx = coverage_lookup(cov, left)?;
    let v1_size = popcount_u16(value_format1) * 2;
    let v2_size = popcount_u16(value_format2) * 2;
    match format {
        1 => pair_pos_format1(sub, cov_idx, right, value_format1, v1_size, v2_size),
        2 => pair_pos_format2(sub, left, right, value_format1, v1_size, v2_size),
        _ => None,
    }
}

fn pair_pos_format1(
    sub: &[u8],
    cov_idx: u16,
    right: u16,
    value_format1: u16,
    v1_size: usize,
    v2_size: usize,
) -> Option<i16> {
    // Header (10 bytes) + pairSetOffsets[pairSetCount].
    let pair_set_count = read_u16(sub, 8).ok()?;
    if cov_idx >= pair_set_count {
        return None;
    }
    let pair_set_off = read_u16(sub, 10 + cov_idx as usize * 2).ok()? as usize;
    let pair_set = sub.get(pair_set_off..)?;
    if pair_set.len() < 2 {
        return None;
    }
    let pair_value_count = read_u16(pair_set, 0).ok()? as usize;
    // Each PairValueRecord = u16 secondGlyph + valueRecord1 + valueRecord2.
    let record_size = 2 + v1_size + v2_size;
    // Binary-search by secondGlyph.
    let mut lo = 0usize;
    let mut hi = pair_value_count;
    while lo < hi {
        let mid = (lo + hi) / 2;
        let off = 2 + mid * record_size;
        let sg = read_u16(pair_set, off).ok()?;
        if sg == right {
            return Some(extract_x_advance(pair_set, off + 2, value_format1));
        }
        if sg < right {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    None
}

fn pair_pos_format2(
    sub: &[u8],
    left: u16,
    right: u16,
    value_format1: u16,
    v1_size: usize,
    v2_size: usize,
) -> Option<i16> {
    // Header (16 bytes): format, cov, vf1, vf2, classDef1Offset,
    // classDef2Offset, class1Count, class2Count.
    let class_def1_off = read_u16(sub, 8).ok()? as usize;
    let class_def2_off = read_u16(sub, 10).ok()? as usize;
    let _class1_count = read_u16(sub, 12).ok()?;
    let class2_count = read_u16(sub, 14).ok()? as usize;
    let cd1 = sub.get(class_def1_off..)?;
    let cd2 = sub.get(class_def2_off..)?;
    let class1 = class_def_lookup(cd1, left).unwrap_or(0);
    let class2 = class_def_lookup(cd2, right).unwrap_or(0);
    let class2_record_size = v1_size + v2_size;
    let class1_record_size = class2_count * class2_record_size;
    let class1_records_start = 16usize;
    let off = class1_records_start
        + class1 as usize * class1_record_size
        + class2 as usize * class2_record_size;
    if v1_size == 0 {
        return None;
    }
    Some(extract_x_advance(sub, off, value_format1))
}

/// Read the `xAdvance` field out of a ValueRecord starting at `bytes[off]`,
/// given its `valueFormat`. Returns 0 if `xAdvance` isn't present.
fn extract_x_advance(bytes: &[u8], off: usize, value_format: u16) -> i16 {
    let mut p = off;
    if value_format & VF_X_PLACEMENT != 0 {
        p += 2;
    }
    if value_format & VF_Y_PLACEMENT != 0 {
        p += 2;
    }
    if value_format & VF_X_ADVANCE != 0 {
        return read_i16(bytes, p).unwrap_or(0);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tiny GPOS with one PairPosFormat1 subtable: glyph 50
    /// pairs with glyph 60 → xAdvance=-100.
    fn build_simple_pp1() -> Vec<u8> {
        // PairValueRecord: u16 secondGlyph + value record 1 (xAdv only, 2 bytes).
        let mut pvr = Vec::new();
        pvr.extend_from_slice(&60u16.to_be_bytes());
        pvr.extend_from_slice(&(-100i16).to_be_bytes());

        // PairSet: u16 pairValueCount + pairValueRecords.
        let mut pair_set = Vec::new();
        pair_set.extend_from_slice(&1u16.to_be_bytes());
        pair_set.extend_from_slice(&pvr);

        // Coverage format 1: covers glyph 50.
        let mut cov = Vec::new();
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&50u16.to_be_bytes());

        // PairPosFormat1 header (10 bytes) + pairSetOffsets[1].
        let header = 10;
        let pair_set_offsets_size = 2;
        let cov_off = header + pair_set_offsets_size;
        let pair_set_off = cov_off + cov.len();
        let mut pp1 = Vec::new();
        pp1.extend_from_slice(&1u16.to_be_bytes()); // format
        pp1.extend_from_slice(&(cov_off as u16).to_be_bytes());
        pp1.extend_from_slice(&VF_X_ADVANCE.to_be_bytes()); // value_format1
        pp1.extend_from_slice(&0u16.to_be_bytes()); // value_format2
        pp1.extend_from_slice(&1u16.to_be_bytes()); // pairSetCount
        pp1.extend_from_slice(&(pair_set_off as u16).to_be_bytes());
        pp1.extend_from_slice(&cov);
        pp1.extend_from_slice(&pair_set);

        // Lookup: type=2, flag=0, subCount=1, subOffsets=[8].
        let mut lookup = Vec::new();
        lookup.extend_from_slice(&2u16.to_be_bytes());
        lookup.extend_from_slice(&0u16.to_be_bytes());
        lookup.extend_from_slice(&1u16.to_be_bytes());
        lookup.extend_from_slice(&8u16.to_be_bytes());
        lookup.extend_from_slice(&pp1);

        // LookupList.
        let mut lookup_list = Vec::new();
        lookup_list.extend_from_slice(&1u16.to_be_bytes());
        lookup_list.extend_from_slice(&4u16.to_be_bytes());
        lookup_list.extend_from_slice(&lookup);

        // GPOS header.
        let mut gpos = Vec::new();
        gpos.extend_from_slice(&1u16.to_be_bytes());
        gpos.extend_from_slice(&0u16.to_be_bytes());
        gpos.extend_from_slice(&0u16.to_be_bytes());
        gpos.extend_from_slice(&0u16.to_be_bytes());
        gpos.extend_from_slice(&10u16.to_be_bytes());
        gpos.extend_from_slice(&lookup_list);
        gpos
    }

    #[test]
    fn pair_pos_format1_round_trip() {
        let bytes = build_simple_pp1();
        let g = GposTable::parse(&bytes).unwrap();
        assert_eq!(g.lookup_kerning(50, 60, None), -100);
        assert_eq!(g.lookup_kerning(50, 61, None), 0);
        assert_eq!(g.lookup_kerning(99, 60, None), 0);
    }
}
