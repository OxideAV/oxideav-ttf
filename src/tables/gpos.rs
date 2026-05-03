//! `GPOS` — Glyph Positioning Table.
//!
//! Supported lookup types:
//! - **LookupType 2** (Pair Adjustment Positioning) — kerning. Both
//!   PairPosFormat1 (per-pair adjustments via Coverage + PairSet) and
//!   PairPosFormat2 (class-pair grid) are supported. We extract only
//!   the `xAdvance` adjustment of the first glyph in the pair — that's
//!   all "kerning" means for our consumer crate.
//! - **LookupType 4** (Mark-to-Base Attachment) — diacritic positioning
//!   for a mark glyph above / below a base glyph. Returns the offset
//!   `(dx, dy)` in font units that, when added to the mark's pen
//!   position, snaps the mark's anchor onto the base's anchor. Only
//!   MarkBasePosFormat1 is defined by the spec; both Anchor format 1
//!   (plain x/y) and format 3 (x/y + device offsets, which we ignore)
//!   are accepted. Format 2 (anchor point) is treated as format 1
//!   because we don't run the TT bytecode.
//! - **LookupType 6** (Mark-to-Mark Attachment) — mark-on-mark stacking
//!   used when a base glyph already carries one diacritic and a second
//!   diacritic must sit on top of (or below) the first. Layout-wise the
//!   sub-table is identical to MarkBasePos but interprets coverage 2 as
//!   the *previous mark* rather than the base. Returns `(dx, dy)` in
//!   font units to add to the second mark's pen origin.
//!
//! ExtensionPos (LookupType 9) is unwrapped transparently for all
//! supported sub-types.

use crate::parser::{read_i16, read_u16, read_u32};
use crate::tables::gdef::{
    class_def_lookup, coverage_lookup, lookup_table_slice, popcount_u16, GdefTable,
};
use crate::Error;

const LOOKUP_PAIR_POS: u16 = 2;
const LOOKUP_MARK_BASE_POS: u16 = 4;
const LOOKUP_MARK_MARK_POS: u16 = 6;
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

    /// Look up the mark-to-base attachment offset for a `(base, mark)`
    /// glyph pair. Returns `(dx, dy)` in font units (TT Y-up convention)
    /// to add to the mark's pen origin so its anchor lands on the base's
    /// anchor for the mark's class.
    ///
    /// Returns `None` if no MarkBasePosFormat1 sub-table covers both
    /// glyphs, or if the mark's class has no anchor on this base.
    ///
    /// Walks every LookupType 4 sub-table; the first hit wins (matches
    /// the OpenType "first matching subtable in lookup order" rule).
    pub fn lookup_mark_to_base(&self, base: u16, mark: u16) -> Option<(i16, i16)> {
        let lookup_list = self.bytes.get(self.lookup_list_off as usize..)?;
        if lookup_list.len() < 2 {
            return None;
        }
        let lookup_count = read_u16(lookup_list, 0).ok()?;
        for i in 0..lookup_count {
            let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, i)?;
            if lookup.len() < 6 {
                continue;
            }
            let kind = read_u16(lookup, 0).ok()?;
            let sub_count = read_u16(lookup, 4).ok()? as usize;
            for s in 0..sub_count {
                let sub_off = read_u16(lookup, 6 + s * 2).ok()? as usize;
                let sub = lookup.get(sub_off..)?;
                let (effective_kind, effective_sub) = if kind == LOOKUP_EXTENSION_POS {
                    if sub.len() < 8 {
                        continue;
                    }
                    let ext_type = read_u16(sub, 2).ok()?;
                    let ext_off = read_u32(sub, 4).ok()? as usize;
                    let ext = sub.get(ext_off..)?;
                    (ext_type, ext)
                } else {
                    (kind, sub)
                };
                if effective_kind != LOOKUP_MARK_BASE_POS {
                    continue;
                }
                if let Some(v) = mark_base_pos_lookup(effective_sub, base, mark) {
                    return Some(v);
                }
            }
        }
        None
    }

    /// Look up the mark-to-mark attachment offset for a `(mark1, mark2)`
    /// glyph pair, where `mark1` is the *previous* (already-attached)
    /// mark and `mark2` is the new mark we want to stack on top of (or
    /// below) it. Returns `(dx, dy)` in font units (TT Y-up convention)
    /// to add to `mark2`'s pen origin so its anchor lands on `mark1`'s
    /// anchor for `mark2`'s class.
    ///
    /// Returns `None` if no MarkMarkPosFormat1 sub-table covers both
    /// glyphs, or if the mark2's class has no anchor on mark1.
    ///
    /// Walks every LookupType 6 sub-table; the first hit wins (matches
    /// the OpenType "first matching subtable in lookup order" rule).
    pub fn lookup_mark_to_mark(&self, mark1: u16, mark2: u16) -> Option<(i16, i16)> {
        let lookup_list = self.bytes.get(self.lookup_list_off as usize..)?;
        if lookup_list.len() < 2 {
            return None;
        }
        let lookup_count = read_u16(lookup_list, 0).ok()?;
        for i in 0..lookup_count {
            let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, i)?;
            if lookup.len() < 6 {
                continue;
            }
            let kind = read_u16(lookup, 0).ok()?;
            let sub_count = read_u16(lookup, 4).ok()? as usize;
            for s in 0..sub_count {
                let sub_off = read_u16(lookup, 6 + s * 2).ok()? as usize;
                let sub = lookup.get(sub_off..)?;
                let (effective_kind, effective_sub) = if kind == LOOKUP_EXTENSION_POS {
                    if sub.len() < 8 {
                        continue;
                    }
                    let ext_type = read_u16(sub, 2).ok()?;
                    let ext_off = read_u32(sub, 4).ok()? as usize;
                    let ext = sub.get(ext_off..)?;
                    (ext_type, ext)
                } else {
                    (kind, sub)
                };
                if effective_kind != LOOKUP_MARK_MARK_POS {
                    continue;
                }
                if let Some(v) = mark_mark_pos_lookup(effective_sub, mark1, mark2) {
                    return Some(v);
                }
            }
        }
        None
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

/// Walk a MarkBasePosFormat1 subtable looking for `(base, mark)` and
/// return the `(dx, dy)` mark-attachment offset in font units.
///
/// MarkBasePosFormat1 layout (OpenType spec § GPOS):
/// ```text
///   u16 format == 1
///   Offset16 markCoverageOffset       // covers all mark glyphs
///   Offset16 baseCoverageOffset       // covers all base glyphs
///   u16 markClassCount
///   Offset16 markArrayOffset
///   Offset16 baseArrayOffset
/// ```
///
/// MarkArray:
/// ```text
///   u16 markCount
///   markRecords[markCount] = { u16 markClass; Offset16 markAnchorOffset; }
/// ```
///
/// BaseArray:
/// ```text
///   u16 baseCount
///   baseRecords[baseCount] = { Offset16 baseAnchorOffset[markClassCount]; }
/// ```
///
/// The returned offset is computed as `base_anchor - mark_anchor` in TT
/// (Y-up) font units. The shaper applies it as `mark.x_offset += dx`,
/// `mark.y_offset += dy` minus the un-attached pen advance for the
/// mark, but the consumer crate handles that — this function returns
/// the raw anchor delta only.
fn mark_base_pos_lookup(sub: &[u8], base: u16, mark: u16) -> Option<(i16, i16)> {
    if sub.len() < 12 {
        return None;
    }
    let format = read_u16(sub, 0).ok()?;
    if format != 1 {
        return None;
    }
    let mark_cov_off = read_u16(sub, 2).ok()? as usize;
    let base_cov_off = read_u16(sub, 4).ok()? as usize;
    let mark_class_count = read_u16(sub, 6).ok()? as usize;
    let mark_array_off = read_u16(sub, 8).ok()? as usize;
    let base_array_off = read_u16(sub, 10).ok()? as usize;

    let mark_cov = sub.get(mark_cov_off..)?;
    let base_cov = sub.get(base_cov_off..)?;
    let mark_idx = coverage_lookup(mark_cov, mark)? as usize;
    let base_idx = coverage_lookup(base_cov, base)? as usize;

    // MarkArray: markCount + markRecord[mark_idx] = (class u16, anchor_off u16)
    let mark_array = sub.get(mark_array_off..)?;
    if mark_array.len() < 2 {
        return None;
    }
    let mark_count = read_u16(mark_array, 0).ok()? as usize;
    if mark_idx >= mark_count {
        return None;
    }
    let mr_off = 2 + mark_idx * 4;
    let mark_class = read_u16(mark_array, mr_off).ok()? as usize;
    let mark_anchor_off_local = read_u16(mark_array, mr_off + 2).ok()? as usize;
    if mark_class >= mark_class_count {
        return None;
    }
    // MarkRecord's markAnchorOffset is relative to the MarkArray start.
    let mark_anchor = mark_array.get(mark_anchor_off_local..)?;
    let (mx, my) = parse_anchor(mark_anchor)?;

    // BaseArray: baseCount + baseRecord[base_idx] = baseAnchorOffset[mark_class_count]
    let base_array = sub.get(base_array_off..)?;
    if base_array.len() < 2 {
        return None;
    }
    let base_count = read_u16(base_array, 0).ok()? as usize;
    if base_idx >= base_count {
        return None;
    }
    let br_off = 2 + base_idx * mark_class_count * 2;
    let base_anchor_off_local = read_u16(base_array, br_off + mark_class * 2).ok()? as usize;
    // A null offset (0) means "no anchor for this class on this base".
    if base_anchor_off_local == 0 {
        return None;
    }
    let base_anchor = base_array.get(base_anchor_off_local..)?;
    let (bx, by) = parse_anchor(base_anchor)?;

    // Mark gets pulled from its own anchor onto the base's anchor:
    //   (dx, dy) = base_anchor - mark_anchor
    Some((bx.wrapping_sub(mx), by.wrapping_sub(my)))
}

/// Walk a MarkMarkPosFormat1 subtable looking for `(mark1, mark2)` and
/// return the `(dx, dy)` mark-on-mark attachment offset in font units.
///
/// MarkMarkPosFormat1 layout (OpenType spec § GPOS LookupType 6) is
/// structurally identical to MarkBasePosFormat1 — only the role of
/// "second glyph" differs (it's a previous mark, not a base). Same
/// MarkArray (mark1 records: class + anchor) and same outer Mark2Array
/// (mark2 records: anchor per class). We share `parse_anchor` and the
/// arithmetic with the mark-to-base path.
fn mark_mark_pos_lookup(sub: &[u8], mark1: u16, mark2: u16) -> Option<(i16, i16)> {
    if sub.len() < 12 {
        return None;
    }
    let format = read_u16(sub, 0).ok()?;
    if format != 1 {
        return None;
    }
    let mark1_cov_off = read_u16(sub, 2).ok()? as usize;
    let mark2_cov_off = read_u16(sub, 4).ok()? as usize;
    let mark_class_count = read_u16(sub, 6).ok()? as usize;
    let mark1_array_off = read_u16(sub, 8).ok()? as usize;
    let mark2_array_off = read_u16(sub, 10).ok()? as usize;

    // Per the OpenType spec the *attaching* mark is mark1 (which we
    // emit as the second mark in source order — the spec uses "mark1"
    // for the to-be-attached glyph); the *attached-to* mark is mark2
    // (the previous, already-positioned mark). The MarkArray covers
    // mark1 (the new glyph) and Mark2Array covers mark2 (the previous
    // glyph). Our argument naming follows source order: `mark1` here
    // is the previous mark, `mark2` is the new one. Map accordingly.
    let mark2_cov = sub.get(mark1_cov_off..)?; // covers the new attaching mark
    let mark1_cov = sub.get(mark2_cov_off..)?; // covers the already-placed mark
    let mark2_idx = coverage_lookup(mark2_cov, mark2)? as usize;
    let mark1_idx = coverage_lookup(mark1_cov, mark1)? as usize;

    // MarkArray (mark1 records — really "the new mark" per spec):
    // markCount + markRecord[mark2_idx] = (class u16, anchor_off u16).
    let new_mark_array = sub.get(mark1_array_off..)?;
    if new_mark_array.len() < 2 {
        return None;
    }
    let new_mark_count = read_u16(new_mark_array, 0).ok()? as usize;
    if mark2_idx >= new_mark_count {
        return None;
    }
    let nr_off = 2 + mark2_idx * 4;
    let new_mark_class = read_u16(new_mark_array, nr_off).ok()? as usize;
    let new_anchor_off_local = read_u16(new_mark_array, nr_off + 2).ok()? as usize;
    if new_mark_class >= mark_class_count {
        return None;
    }
    let new_mark_anchor = new_mark_array.get(new_anchor_off_local..)?;
    let (mx, my) = parse_anchor(new_mark_anchor)?;

    // Mark2Array: mark2Count + mark2Record[mark1_idx] =
    // mark2AnchorOffset[mark_class_count].
    let prev_array = sub.get(mark2_array_off..)?;
    if prev_array.len() < 2 {
        return None;
    }
    let prev_count = read_u16(prev_array, 0).ok()? as usize;
    if mark1_idx >= prev_count {
        return None;
    }
    let pr_off = 2 + mark1_idx * mark_class_count * 2;
    let prev_anchor_off_local = read_u16(prev_array, pr_off + new_mark_class * 2).ok()? as usize;
    if prev_anchor_off_local == 0 {
        return None;
    }
    let prev_anchor = prev_array.get(prev_anchor_off_local..)?;
    let (bx, by) = parse_anchor(prev_anchor)?;

    // Same arithmetic as mark-to-base: pull the attaching mark from its
    // own anchor onto the previous mark's anchor for that class.
    Some((bx.wrapping_sub(mx), by.wrapping_sub(my)))
}

/// Parse an Anchor table. Supports format 1 (plain x/y) and format 3
/// (x/y + device tables which we ignore — not relevant without TT
/// hinting). Format 2 (x/y + anchor point) is read like format 1 since
/// we don't run the bytecode interpreter to resolve the anchor point.
fn parse_anchor(bytes: &[u8]) -> Option<(i16, i16)> {
    if bytes.len() < 6 {
        return None;
    }
    let format = read_u16(bytes, 0).ok()?;
    let x = read_i16(bytes, 2).ok()?;
    let y = read_i16(bytes, 4).ok()?;
    match format {
        1 | 2 | 3 => Some((x, y)),
        _ => None,
    }
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

    /// Build a tiny GPOS with one MarkBasePosFormat1 subtable: base
    /// glyph 10 (anchor 100, 800) with mark glyph 200 (mark class 0,
    /// anchor 50, 0). Expected delta when attaching mark→base:
    /// `(100 - 50, 800 - 0) = (50, 800)`.
    fn build_simple_mark_base() -> Vec<u8> {
        // ---- Anchor tables (format 1: u16 format + i16 x + i16 y) ----
        let mut base_anchor = Vec::new();
        base_anchor.extend_from_slice(&1u16.to_be_bytes());
        base_anchor.extend_from_slice(&100i16.to_be_bytes());
        base_anchor.extend_from_slice(&800i16.to_be_bytes());

        let mut mark_anchor = Vec::new();
        mark_anchor.extend_from_slice(&1u16.to_be_bytes());
        mark_anchor.extend_from_slice(&50i16.to_be_bytes());
        mark_anchor.extend_from_slice(&0i16.to_be_bytes());

        // ---- MarkArray: 1 mark record ----
        // Header (markCount=1) + 1 markRecord (4 bytes: class + offset)
        // = 6 bytes. mark_anchor placed right after, so offset = 6.
        let mut mark_array = Vec::new();
        mark_array.extend_from_slice(&1u16.to_be_bytes());
        mark_array.extend_from_slice(&0u16.to_be_bytes()); // class 0
        mark_array.extend_from_slice(&6u16.to_be_bytes()); // anchor offset
        mark_array.extend_from_slice(&mark_anchor);

        // ---- BaseArray: 1 base record, 1 mark class ----
        // Header (baseCount=1) + 1 baseRecord (1 anchor offset = 2 bytes)
        // = 4 bytes. base_anchor placed right after, so offset = 4.
        let mut base_array = Vec::new();
        base_array.extend_from_slice(&1u16.to_be_bytes());
        base_array.extend_from_slice(&4u16.to_be_bytes());
        base_array.extend_from_slice(&base_anchor);

        // ---- Coverage tables (format 1) ----
        let mut mark_cov = Vec::new();
        mark_cov.extend_from_slice(&1u16.to_be_bytes());
        mark_cov.extend_from_slice(&1u16.to_be_bytes());
        mark_cov.extend_from_slice(&200u16.to_be_bytes());

        let mut base_cov = Vec::new();
        base_cov.extend_from_slice(&1u16.to_be_bytes());
        base_cov.extend_from_slice(&1u16.to_be_bytes());
        base_cov.extend_from_slice(&10u16.to_be_bytes());

        // ---- MarkBasePosFormat1 subtable ----
        // Header is 12 bytes:
        //   format (2) + markCovOff (2) + baseCovOff (2)
        //   + markClassCount (2) + markArrayOff (2) + baseArrayOff (2)
        let header = 12usize;
        let mark_cov_off = header;
        let base_cov_off = mark_cov_off + mark_cov.len();
        let mark_array_off = base_cov_off + base_cov.len();
        let base_array_off = mark_array_off + mark_array.len();
        let mut mbp = Vec::new();
        mbp.extend_from_slice(&1u16.to_be_bytes()); // format
        mbp.extend_from_slice(&(mark_cov_off as u16).to_be_bytes());
        mbp.extend_from_slice(&(base_cov_off as u16).to_be_bytes());
        mbp.extend_from_slice(&1u16.to_be_bytes()); // markClassCount
        mbp.extend_from_slice(&(mark_array_off as u16).to_be_bytes());
        mbp.extend_from_slice(&(base_array_off as u16).to_be_bytes());
        mbp.extend_from_slice(&mark_cov);
        mbp.extend_from_slice(&base_cov);
        mbp.extend_from_slice(&mark_array);
        mbp.extend_from_slice(&base_array);

        // ---- Lookup: type=4, flag=0, subCount=1, subOffsets=[8] ----
        let mut lookup = Vec::new();
        lookup.extend_from_slice(&4u16.to_be_bytes());
        lookup.extend_from_slice(&0u16.to_be_bytes());
        lookup.extend_from_slice(&1u16.to_be_bytes());
        lookup.extend_from_slice(&8u16.to_be_bytes());
        lookup.extend_from_slice(&mbp);

        // ---- LookupList: count=1, lookupOffsets=[4] ----
        let mut lookup_list = Vec::new();
        lookup_list.extend_from_slice(&1u16.to_be_bytes());
        lookup_list.extend_from_slice(&4u16.to_be_bytes());
        lookup_list.extend_from_slice(&lookup);

        // ---- GPOS header ----
        let mut gpos = Vec::new();
        gpos.extend_from_slice(&1u16.to_be_bytes()); // major
        gpos.extend_from_slice(&0u16.to_be_bytes()); // minor
        gpos.extend_from_slice(&0u16.to_be_bytes()); // scriptList
        gpos.extend_from_slice(&0u16.to_be_bytes()); // featureList
        gpos.extend_from_slice(&10u16.to_be_bytes()); // lookupList offset
        gpos.extend_from_slice(&lookup_list);
        gpos
    }

    #[test]
    fn mark_to_base_round_trip() {
        let bytes = build_simple_mark_base();
        let g = GposTable::parse(&bytes).unwrap();
        // Expected: base_anchor (100, 800) - mark_anchor (50, 0) = (50, 800).
        assert_eq!(g.lookup_mark_to_base(10, 200), Some((50, 800)));
        // Pair not in coverage → None.
        assert_eq!(g.lookup_mark_to_base(11, 200), None);
        assert_eq!(g.lookup_mark_to_base(10, 201), None);
    }

    #[test]
    fn mark_to_base_missing_table_returns_none() {
        // Reuse the kerning-only fixture: it has no LookupType 4, so
        // lookup_mark_to_base must return None for any pair.
        let bytes = build_simple_pp1();
        let g = GposTable::parse(&bytes).unwrap();
        assert_eq!(g.lookup_mark_to_base(50, 60), None);
    }

    /// Build a tiny GPOS with one MarkMarkPosFormat1 subtable: previous
    /// mark glyph 30 (anchor 60, 1200) and new mark glyph 40 (mark
    /// class 0, anchor 30, 0). Expected delta when stacking new on
    /// previous: `(60 - 30, 1200 - 0) = (30, 1200)`.
    fn build_simple_mark_mark() -> Vec<u8> {
        // ---- Anchor tables (format 1: u16 format + i16 x + i16 y) ----
        let mut prev_anchor = Vec::new();
        prev_anchor.extend_from_slice(&1u16.to_be_bytes());
        prev_anchor.extend_from_slice(&60i16.to_be_bytes());
        prev_anchor.extend_from_slice(&1200i16.to_be_bytes());

        let mut new_anchor = Vec::new();
        new_anchor.extend_from_slice(&1u16.to_be_bytes());
        new_anchor.extend_from_slice(&30i16.to_be_bytes());
        new_anchor.extend_from_slice(&0i16.to_be_bytes());

        // ---- New-mark MarkArray (sub.mark1_array, the *attaching*
        // mark) — covers mark2 (the new glyph in our shaper API).
        // markCount=1 + record (class=0, off=6) + anchor at offset 6.
        let mut new_mark_array = Vec::new();
        new_mark_array.extend_from_slice(&1u16.to_be_bytes());
        new_mark_array.extend_from_slice(&0u16.to_be_bytes()); // class 0
        new_mark_array.extend_from_slice(&6u16.to_be_bytes()); // anchor offset
        new_mark_array.extend_from_slice(&new_anchor);

        // ---- Previous-mark Mark2Array (sub.mark2_array, the
        // already-placed mark) — covers mark1 in our shaper API.
        // mark2Count=1 + record (1 anchor offset = 2 bytes) + anchor
        // at offset 4.
        let mut prev_array = Vec::new();
        prev_array.extend_from_slice(&1u16.to_be_bytes());
        prev_array.extend_from_slice(&4u16.to_be_bytes()); // anchor off
        prev_array.extend_from_slice(&prev_anchor);

        // ---- Coverage tables (format 1) ----
        // sub.mark1_cov covers the new attaching mark (gid 40).
        let mut new_cov = Vec::new();
        new_cov.extend_from_slice(&1u16.to_be_bytes());
        new_cov.extend_from_slice(&1u16.to_be_bytes());
        new_cov.extend_from_slice(&40u16.to_be_bytes());

        // sub.mark2_cov covers the already-placed mark (gid 30).
        let mut prev_cov = Vec::new();
        prev_cov.extend_from_slice(&1u16.to_be_bytes());
        prev_cov.extend_from_slice(&1u16.to_be_bytes());
        prev_cov.extend_from_slice(&30u16.to_be_bytes());

        // ---- MarkMarkPosFormat1 subtable (12-byte header) ----
        let header = 12usize;
        let new_cov_off = header;
        let prev_cov_off = new_cov_off + new_cov.len();
        let new_mark_array_off = prev_cov_off + prev_cov.len();
        let prev_array_off = new_mark_array_off + new_mark_array.len();
        let mut mmp = Vec::new();
        mmp.extend_from_slice(&1u16.to_be_bytes()); // format
        mmp.extend_from_slice(&(new_cov_off as u16).to_be_bytes()); // mark1Cov
        mmp.extend_from_slice(&(prev_cov_off as u16).to_be_bytes()); // mark2Cov
        mmp.extend_from_slice(&1u16.to_be_bytes()); // markClassCount
        mmp.extend_from_slice(&(new_mark_array_off as u16).to_be_bytes());
        mmp.extend_from_slice(&(prev_array_off as u16).to_be_bytes());
        mmp.extend_from_slice(&new_cov);
        mmp.extend_from_slice(&prev_cov);
        mmp.extend_from_slice(&new_mark_array);
        mmp.extend_from_slice(&prev_array);

        // ---- Lookup: type=6, flag=0, subCount=1, subOffsets=[8] ----
        let mut lookup = Vec::new();
        lookup.extend_from_slice(&6u16.to_be_bytes());
        lookup.extend_from_slice(&0u16.to_be_bytes());
        lookup.extend_from_slice(&1u16.to_be_bytes());
        lookup.extend_from_slice(&8u16.to_be_bytes());
        lookup.extend_from_slice(&mmp);

        let mut lookup_list = Vec::new();
        lookup_list.extend_from_slice(&1u16.to_be_bytes());
        lookup_list.extend_from_slice(&4u16.to_be_bytes());
        lookup_list.extend_from_slice(&lookup);

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
    fn mark_to_mark_round_trip() {
        let bytes = build_simple_mark_mark();
        let g = GposTable::parse(&bytes).unwrap();
        // Expected: prev_anchor (60, 1200) - new_anchor (30, 0) = (30, 1200).
        assert_eq!(g.lookup_mark_to_mark(30, 40), Some((30, 1200)));
        // Pair not in coverage → None.
        assert_eq!(g.lookup_mark_to_mark(31, 40), None);
        assert_eq!(g.lookup_mark_to_mark(30, 41), None);
    }

    #[test]
    fn mark_to_mark_missing_table_returns_none() {
        // Reuse the kerning-only fixture: no LookupType 6, so
        // lookup_mark_to_mark must return None for any pair.
        let bytes = build_simple_pp1();
        let g = GposTable::parse(&bytes).unwrap();
        assert_eq!(g.lookup_mark_to_mark(50, 60), None);
        // Mark-to-base fixture also has no LookupType 6.
        let bytes = build_simple_mark_base();
        let g = GposTable::parse(&bytes).unwrap();
        assert_eq!(g.lookup_mark_to_mark(10, 200), None);
    }
}
