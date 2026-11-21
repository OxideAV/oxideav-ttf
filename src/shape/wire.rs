//! Readers for the GPOS sub-table records the shaping engine and the
//! per-lookup queries share: ValueRecords, Anchor tables, pair
//! adjustment, cursive entry / exit, and the three mark-attachment
//! layouts. Every function takes the sub-table bytes (Extension wrapper
//! already removed) and answers for one glyph or glyph pair; nothing
//! here walks a glyph run.
//!
//! All values come back in font design units with the variation deltas
//! of the current instance folded in ([`VarCtx`]): a ValueRecord or
//! Anchor device offset that is a VariationIndex table is resolved
//! through the GDEF ItemVariationStore. A classic Device table adjusts
//! by whole pixels at one ppem, which has no font-unit equivalent, and
//! contributes nothing.

use crate::layout::{PairAccel, SubAccel};
use crate::tables::device::resolve_device_delta;
use crate::tables::gdef::{class_def_lookup, coverage_lookup};
use crate::tables::mvar::ItemVariationStore;

/// Big-endian `uint16` at `at`, or `None` past the end.
#[inline]
pub(crate) fn rd16(bytes: &[u8], at: usize) -> Option<u16> {
    let pair = bytes.get(at..at.checked_add(2)?)?;
    Some(u16::from_be_bytes([pair[0], pair[1]]))
}

/// Big-endian `int16` at `at`.
#[inline]
pub(crate) fn rd16s(bytes: &[u8], at: usize) -> Option<i16> {
    rd16(bytes, at).map(|v| v as i16)
}

/// The table an `Offset16` stored at `at` points to, measured from the
/// start of `bytes`. `None` for a NULL offset or one past the end.
#[inline]
pub(crate) fn follow16(bytes: &[u8], at: usize) -> Option<&[u8]> {
    match rd16(bytes, at)? {
        0 => None,
        off => bytes.get(off as usize..),
    }
}

/// Class of `gid` in a ClassDef table (class 0 for an unlisted glyph).
#[inline]
pub(crate) fn class_of(class_def: &[u8], gid: u16) -> u16 {
    class_def_lookup(class_def, gid).unwrap_or(0)
}

/// The variation instance GPOS values are resolved at.
#[derive(Clone, Copy)]
pub(crate) struct VarCtx<'f> {
    /// GDEF ItemVariationStore; `None` when the font has none or sits at
    /// its default instance (every delta is then zero).
    pub(crate) ivs: Option<&'f ItemVariationStore>,
    /// Normalised axis coordinates.
    pub(crate) coords: &'f [f32],
}

impl VarCtx<'_> {
    /// The default instance: no deltas.
    pub(crate) const DEFAULT: VarCtx<'static> = VarCtx {
        ivs: None,
        coords: &[],
    };

    /// Rounded font-unit delta of the Device / VariationIndex table at
    /// `offset` from the start of `base` (0 for a NULL offset).
    #[inline]
    pub(crate) fn delta(&self, base: &[u8], offset: u16) -> i32 {
        if offset == 0 || self.ivs.is_none() {
            return 0;
        }
        resolve_device_delta(base, offset, self.ivs, self.coords).round() as i32
    }
}

/// A decoded ValueRecord: placement and advance adjustments.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct Adjust {
    pub(crate) x_placement: i32,
    pub(crate) y_placement: i32,
    pub(crate) x_advance: i32,
    pub(crate) y_advance: i32,
}

/// Bytes of a ValueRecord of `format`: one 16-bit field per set bit.
#[inline]
pub(crate) fn value_size(format: u16) -> usize {
    (format & 0x00FF).count_ones() as usize * 2
}

/// Decode the ValueRecord of `format` at `at` in `bytes`. Its device
/// offsets are measured from the start of `device_base` — the immediate
/// parent table of the record (the SinglePos or PairPosFormat2
/// sub-table, or the PairSet table of a PairPosFormat1 sub-table).
pub(crate) fn read_value(
    bytes: &[u8],
    at: usize,
    format: u16,
    device_base: &[u8],
    var: &VarCtx<'_>,
) -> Adjust {
    let mut out = Adjust::default();
    if format & 0x00FF == 0 {
        return out;
    }
    let mut cursor = at;
    let mut next = |present: bool| -> Option<u16> {
        if !present {
            return None;
        }
        let v = rd16(bytes, cursor);
        cursor += 2;
        Some(v.unwrap_or(0))
    };
    if let Some(v) = next(format & 0x0001 != 0) {
        out.x_placement = v as i16 as i32;
    }
    if let Some(v) = next(format & 0x0002 != 0) {
        out.y_placement = v as i16 as i32;
    }
    if let Some(v) = next(format & 0x0004 != 0) {
        out.x_advance = v as i16 as i32;
    }
    if let Some(v) = next(format & 0x0008 != 0) {
        out.y_advance = v as i16 as i32;
    }
    if format & 0x00F0 != 0 {
        if let Some(off) = next(format & 0x0010 != 0) {
            out.x_placement += var.delta(device_base, off);
        }
        if let Some(off) = next(format & 0x0020 != 0) {
            out.y_placement += var.delta(device_base, off);
        }
        if let Some(off) = next(format & 0x0040 != 0) {
            out.x_advance += var.delta(device_base, off);
        }
        if let Some(off) = next(format & 0x0080 != 0) {
            out.y_advance += var.delta(device_base, off);
        }
    }
    out
}

/// Decode an Anchor table (formats 1 – 3). Format 2 names a contour
/// point for hinted rendering; its design-unit coordinates are what an
/// unhinted layout uses. Format 3 adds the device / variation deltas.
pub(crate) fn read_anchor(anchor: &[u8], var: &VarCtx<'_>) -> Option<(i32, i32)> {
    let format = rd16(anchor, 0)?;
    let x = rd16s(anchor, 2)? as i32;
    let y = rd16s(anchor, 4)? as i32;
    match format {
        1 | 2 => Some((x, y)),
        3 => {
            let dx = rd16(anchor, 6).map_or(0, |off| var.delta(anchor, off));
            let dy = rd16(anchor, 8).map_or(0, |off| var.delta(anchor, off));
            Some((x + dx, y + dy))
        }
        _ => None,
    }
}

/// SinglePos (GPOS LookupType 1): the adjustment of `gid`.
pub(crate) fn single_adjust(sub: &[u8], gid: u16, var: &VarCtx<'_>) -> Option<Adjust> {
    let index = coverage_lookup(follow16(sub, 2)?, gid)? as usize;
    let format = rd16(sub, 4)?;
    match rd16(sub, 0)? {
        1 => Some(read_value(sub, 6, format, sub, var)),
        2 => {
            if index >= rd16(sub, 6)? as usize {
                return None;
            }
            let at = 8 + index * value_size(format);
            // The whole record must be present.
            sub.get(at..at + value_size(format))?;
            Some(read_value(sub, at, format, sub, var))
        }
        _ => None,
    }
}

/// What a PairPos sub-table says about one glyph pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PairHit {
    pub(crate) first: Adjust,
    pub(crate) second: Adjust,
    /// `valueFormat2 != 0`: the second glyph has a ValueRecord, so it is
    /// consumed by the pair and is not the first glyph of the next one.
    pub(crate) second_has_record: bool,
}

/// PairPos (GPOS LookupType 2), both formats: the adjustments of the
/// pair `(first, second)`, or `None` when the sub-table does not apply
/// to it (first glyph not covered; format 1: no PairValue record for
/// the second glyph).
pub(crate) fn pair_adjust(
    sub: &[u8],
    accel: Option<&SubAccel>,
    first: u16,
    second: u16,
    var: &VarCtx<'_>,
) -> Option<PairHit> {
    let format1 = rd16(sub, 4)?;
    let format2 = rd16(sub, 6)?;
    let (size1, size2) = (value_size(format1), value_size(format2));
    match rd16(sub, 0)? {
        1 => {
            let index = match accel.map(|a| &a.pair) {
                Some(PairAccel::Sets(sets)) => match sets.coverage_index.get(first as usize) {
                    Some(&slot) => slot.checked_sub(1)?,
                    None => coverage_lookup(follow16(sub, 2)?, first)?,
                },
                _ => coverage_lookup(follow16(sub, 2)?, first)?,
            } as usize;
            if index >= rd16(sub, 8)? as usize {
                return None;
            }
            let set = follow16(sub, 10 + index * 2)?;
            let count = rd16(set, 0)? as usize;
            let stride = 2 + size1 + size2;
            // PairValue records are sorted by the second glyph id.
            let (mut lo, mut hi) = (0usize, count);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let at = 2 + mid * stride;
                let glyph = rd16(set, at)?;
                if glyph == second {
                    set.get(at..at + stride)?;
                    return Some(PairHit {
                        first: read_value(set, at + 2, format1, set, var),
                        second: read_value(set, at + 2 + size1, format2, set, var),
                        second_has_record: format2 != 0,
                    });
                }
                if glyph < second {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            None
        }
        2 => {
            let (class1, class2) = match accel.map(|a| (&a.pair, a.first_is_exact)) {
                Some((PairAccel::Classes(flat), true))
                    if (first as usize) < flat.class1.len()
                        && (second as usize) < flat.class2.len() =>
                {
                    // `first_is_exact`: the accelerator's glyph set is
                    // the Coverage table, and the caller tested it.
                    (flat.class1[first as usize], flat.class2[second as usize])
                }
                _ => {
                    coverage_lookup(follow16(sub, 2)?, first)?;
                    (
                        class_of(follow16(sub, 8)?, first),
                        class_of(follow16(sub, 10)?, second),
                    )
                }
            };
            let class1_count = rd16(sub, 12)?;
            let class2_count = rd16(sub, 14)?;
            if class1 >= class1_count || class2 >= class2_count {
                return None;
            }
            let stride = size1 + size2;
            let at = 16 + (class1 as usize * class2_count as usize + class2 as usize) * stride;
            sub.get(at..at + stride)?;
            Some(PairHit {
                first: read_value(sub, at, format1, sub, var),
                second: read_value(sub, at + size1, format2, sub, var),
                second_has_record: format2 != 0,
            })
        }
        _ => None,
    }
}

/// CursivePos (GPOS LookupType 3): the `(entry, exit)` anchors of `gid`,
/// each `None` when its offset is NULL.
#[allow(clippy::type_complexity)]
pub(crate) fn cursive_anchors(
    sub: &[u8],
    gid: u16,
    var: &VarCtx<'_>,
) -> Option<(Option<(i32, i32)>, Option<(i32, i32)>)> {
    if rd16(sub, 0)? != 1 {
        return None;
    }
    let index = coverage_lookup(follow16(sub, 2)?, gid)? as usize;
    if index >= rd16(sub, 4)? as usize {
        return None;
    }
    let record = 6 + index * 4;
    // Both offsets must be readable; either may be NULL.
    rd16(sub, record + 2)?;
    let entry = follow16(sub, record).and_then(|a| read_anchor(a, var));
    let exit = follow16(sub, record + 2).and_then(|a| read_anchor(a, var));
    Some((entry, exit))
}

/// One resolved mark attachment: where the two anchors sit, each in the
/// coordinate space of its own glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MarkHit {
    /// Mark class of the attaching mark.
    pub(crate) class: u16,
    /// Anchor on the attaching mark.
    pub(crate) mark: (i32, i32),
    /// Anchor on the glyph attached to (base, ligature component, or
    /// preceding mark).
    pub(crate) target: (i32, i32),
}

/// `(class, anchor)` of `mark` from the MarkArray of a mark-attachment
/// sub-table (mark Coverage offset at byte 2, MarkArray offset at 8).
fn mark_record(sub: &[u8], mark: u16, var: &VarCtx<'_>) -> Option<(u16, (i32, i32))> {
    let index = coverage_lookup(follow16(sub, 2)?, mark)? as usize;
    let array = follow16(sub, 8)?;
    if index >= rd16(array, 0)? as usize {
        return None;
    }
    let record = 2 + index * 4;
    let class = rd16(array, record)?;
    let anchor = read_anchor(follow16(array, record + 2)?, var)?;
    Some((class, anchor))
}

/// MarkBasePos (GPOS LookupType 4) and MarkMarkPos (LookupType 6), which
/// share one layout: the anchors attaching `mark` to `target` (the base
/// glyph, respectively the preceding mark). `None` when either glyph is
/// not covered or the target defines no anchor for the mark's class.
pub(crate) fn mark_to_single(
    sub: &[u8],
    mark: u16,
    target: u16,
    var: &VarCtx<'_>,
) -> Option<MarkHit> {
    if rd16(sub, 0)? != 1 {
        return None;
    }
    let (class, mark_anchor) = mark_record(sub, mark, var)?;
    let class_count = rd16(sub, 6)? as usize;
    if class as usize >= class_count {
        return None;
    }
    let target_index = coverage_lookup(follow16(sub, 4)?, target)? as usize;
    let array = follow16(sub, 10)?;
    if target_index >= rd16(array, 0)? as usize {
        return None;
    }
    let at = 2 + (target_index * class_count + class as usize) * 2;
    let target_anchor = read_anchor(follow16(array, at)?, var)?;
    Some(MarkHit {
        class,
        mark: mark_anchor,
        target: target_anchor,
    })
}

/// Number of components the LigatureAttach table of `ligature` defines
/// in a MarkLigPos sub-table.
pub(crate) fn ligature_component_count(sub: &[u8], ligature: u16) -> Option<u16> {
    if rd16(sub, 0)? != 1 {
        return None;
    }
    let index = coverage_lookup(follow16(sub, 4)?, ligature)? as usize;
    let array = follow16(sub, 10)?;
    if index >= rd16(array, 0)? as usize {
        return None;
    }
    rd16(follow16(array, 2 + index * 2)?, 0)
}

/// MarkLigPos (GPOS LookupType 5): the anchors attaching `mark` to one
/// component of `ligature`. `component` receives the ligature's
/// component count and picks the (zero-based) component; a pick past
/// the end is clamped to the last component.
pub(crate) fn mark_to_ligature(
    sub: &[u8],
    mark: u16,
    ligature: u16,
    component: impl FnOnce(u16) -> u16,
    var: &VarCtx<'_>,
) -> Option<MarkHit> {
    if rd16(sub, 0)? != 1 {
        return None;
    }
    let (class, mark_anchor) = mark_record(sub, mark, var)?;
    let class_count = rd16(sub, 6)? as usize;
    if class as usize >= class_count {
        return None;
    }
    let lig_index = coverage_lookup(follow16(sub, 4)?, ligature)? as usize;
    let array = follow16(sub, 10)?;
    if lig_index >= rd16(array, 0)? as usize {
        return None;
    }
    let attach = follow16(array, 2 + lig_index * 2)?;
    let count = rd16(attach, 0)?;
    if count == 0 {
        return None;
    }
    let picked = component(count).min(count - 1) as usize;
    let at = 2 + (picked * class_count + class as usize) * 2;
    let target_anchor = read_anchor(follow16(attach, at)?, var)?;
    Some(MarkHit {
        class,
        mark: mark_anchor,
        target: target_anchor,
    })
}
