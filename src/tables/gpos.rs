//! `GPOS` — Glyph Positioning Table.
//!
//! Supported lookup types:
//! - **LookupType 1** (Single Adjustment Positioning) — Format 1: a
//!   single shared `ValueRecord` applied to every glyph the coverage
//!   table lists. Format 2: per-glyph `ValueRecord` indexed by the
//!   coverage index. Returns the four geometric fields (`xPlacement`,
//!   `yPlacement`, `xAdvance`, `yAdvance`) packed into a [`PosValue`]
//!   record. Used by `kern` features that don't need pair-context plus
//!   width-adjustment features (e.g. `cpsp`).
//! - **LookupType 2** (Pair Adjustment Positioning) — kerning. Both
//!   PairPosFormat1 (per-pair adjustments via Coverage + PairSet) and
//!   PairPosFormat2 (class-pair grid) are supported. We extract only
//!   the `xAdvance` adjustment of the first glyph in the pair — that's
//!   all "kerning" means for our consumer crate.
//! - **LookupType 3** (Cursive Attachment) — entry/exit anchor pairs
//!   on consecutive glyphs. Used by Arabic Nastaliq + script-font
//!   cursive chaining: the *exit* anchor of glyph N is chained to the
//!   *entry* anchor of glyph N+1, with the second glyph's pen origin
//!   shifted so the two anchors coincide. Returns
//!   [`CursiveAttachment`] (entry + exit points, each optional). Only
//!   CursivePosFormat1 is defined by the spec.
//! - **LookupType 4** (Mark-to-Base Attachment) — diacritic positioning
//!   for a mark glyph above / below a base glyph. Returns the offset
//!   `(dx, dy)` in font units that, when added to the mark's pen
//!   position, snaps the mark's anchor onto the base's anchor. Only
//!   MarkBasePosFormat1 is defined by the spec; both Anchor format 1
//!   (plain x/y) and format 3 (x/y + device offsets, which we ignore)
//!   are accepted. Format 2 (anchor point) is treated as format 1
//!   because we don't run the TT bytecode.
//! - **LookupType 5** (Mark-to-Ligature Attachment) — like LookupType 4
//!   but the second glyph is a *ligature* whose component the mark
//!   attaches to is selected by the caller. Each ligature carries one
//!   anchor *per (component, mark class)* slot. Returns `(dx, dy)` to
//!   shift the mark's pen origin so its class anchor lands on the
//!   selected component's anchor. Closes the "fi + dot-above"
//!   ligature+mark gap.
//! - **LookupType 6** (Mark-to-Mark Attachment) — mark-on-mark stacking
//!   used when a base glyph already carries one diacritic and a second
//!   diacritic must sit on top of (or below) the first. Layout-wise the
//!   sub-table is identical to MarkBasePos but interprets coverage 2 as
//!   the *previous mark* rather than the base. Returns `(dx, dy)` in
//!   font units to add to the second mark's pen origin.
//! - **LookupType 8** (Chained Contexts Positioning) — same wire shape
//!   as GSUB LookupType 6 (formats 1/2/3) but each
//!   `PosLookupRecord { sequenceIndex, lookupListIndex }` references
//!   another GPOS lookup. The walker returns a list of [`PosRecord`]s
//!   (`absolute index`, four geometric deltas) that the higher-level
//!   shaper folds into its glyph-position state. Nested LookupType 1 /
//!   2 / 4 / 6 / 8 dispatches are supported; the recursion fence is
//!   the same `MAX_NESTED_LOOKUP_DEPTH = 8` we use in GSUB.
//!
//! ExtensionPos (LookupType 9) is unwrapped transparently for every
//! supported sub-type AND when it sits as the outer wrapper around the
//! lookup itself (i.e. `apply_lookup_type_X` accepts an index whose
//! lookup is `kind=9, inner=X` exactly the same as a plain `kind=X`).
//!
//! Spec: Microsoft OpenType §"GPOS — Glyph Positioning Table",
//! §"Common Table Formats", Apple TrueType Reference §"GPOS",
//! ISO/IEC 14496-22 §6 (OFF).

use crate::parser::{read_i16, read_u16, read_u32};
use crate::tables::gdef::{
    class_def_lookup, coverage_lookup, lookup_table_slice, popcount_u16, GdefTable,
};
use crate::Error;

const LOOKUP_SINGLE_POS: u16 = 1;
const LOOKUP_PAIR_POS: u16 = 2;
const LOOKUP_CURSIVE_POS: u16 = 3;
const LOOKUP_MARK_BASE_POS: u16 = 4;
const LOOKUP_MARK_LIGATURE_POS: u16 = 5;
const LOOKUP_MARK_MARK_POS: u16 = 6;
const LOOKUP_CONTEXT_POS: u16 = 7;
const LOOKUP_CHAIN_CONTEXT_POS: u16 = 8;
const LOOKUP_EXTENSION_POS: u16 = 9;

/// Maximum recursion depth for nested chained-context positionings.
/// Prevents pathological self-referential lookup graphs from blowing
/// the stack — the spec doesn't bound this so we set the same
/// conservative fence as GSUB.
const MAX_NESTED_LOOKUP_DEPTH: u8 = 8;

// ValueFormat bits (low byte holds the four geometric flags).
const VF_X_PLACEMENT: u16 = 0x0001;
const VF_Y_PLACEMENT: u16 = 0x0002;
const VF_X_ADVANCE: u16 = 0x0004;
const VF_Y_ADVANCE: u16 = 0x0008;
// High byte holds device-table offsets which we ignore (no TT
// hinting); we still account for their on-disk size when walking
// ValueRecords so subsequent fields are read at the right offset.
const VF_X_PLA_DEVICE: u16 = 0x0010;
const VF_Y_PLA_DEVICE: u16 = 0x0020;
const VF_X_ADV_DEVICE: u16 = 0x0040;
const VF_Y_ADV_DEVICE: u16 = 0x0080;

/// A GPOS `ValueRecord` decoded into its four geometric fields.
///
/// Returned by [`GposTable::lookup_single_pos`] and
/// [`GposTable::apply_lookup_type_8`]. All four fields are in TT font
/// units (Y-up); fields the on-disk record's `valueFormat` does NOT
/// flag are returned as `0`. The four device-table offsets in the
/// high byte of `valueFormat` are skipped — we don't run the TT
/// bytecode interpreter so device-pixel snapping is out of scope.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PosValue {
    pub x_placement: i16,
    pub y_placement: i16,
    pub x_advance: i16,
    pub y_advance: i16,
}

/// One adjustment emitted by a chained-context positioning match.
///
/// `glyph_index` is an *absolute* offset into the input glyph run
/// (not the relative `sequenceIndex` from the on-disk
/// `PosLookupRecord`). The four `value` fields are in TT font units
/// (Y-up). Multiple records may target the same `glyph_index` if the
/// nested lookups stack adjustments — callers should add (not
/// replace) the deltas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PosRecord {
    pub glyph_index: usize,
    pub value: PosValue,
}

/// Cursive attachment anchors for one glyph (GPOS LookupType 3).
///
/// Each glyph in a cursive lookup carries an *entry* anchor (the
/// connecting point on its leading edge) and an *exit* anchor (the
/// connecting point on its trailing edge). Either can be absent
/// (null offset on disk → `None` here): a "joining" glyph has both,
/// a "first-of-cluster" glyph has only `exit`, a "last-of-cluster"
/// glyph has only `entry`.
///
/// Returned by [`GposTable::lookup_cursive_attachment`]. The shaper
/// chains glyphs by translating glyph N+1 so its `entry` lands on
/// glyph N's `exit`. Coordinates are in TT font units (Y-up).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CursiveAttachment {
    /// `(x, y)` of the entry anchor, or `None` if this glyph has no
    /// entry connection (i.e. it's the first glyph in a cursive run).
    pub entry: Option<(i16, i16)>,
    /// `(x, y)` of the exit anchor, or `None` if this glyph has no
    /// exit connection (i.e. it's the last glyph in a cursive run).
    pub exit: Option<(i16, i16)>,
}

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

    /// Enumerate every lookup in the LookupList as
    /// `(lookup_index, lookup_type, subtable_count)`.
    ///
    /// The reported `lookup_type` is the **effective** type after
    /// unwrapping any LookupType-9 ExtensionPos wrapper — i.e. the
    /// caller sees `2` for a kerning lookup whether it's stored as a
    /// plain LookupType-2 lookup or as a LookupType-9 wrapper around a
    /// LookupType-2 sub-table. `subtable_count` is the on-disk
    /// `subTableCount`, unchanged.
    ///
    /// Use this to find lookups of a specific type without probing
    /// every index — for example, "give me every chained-context
    /// positioning lookup" is `gpos_lookup_list().filter(|(_, t, _)| *t == 8)`.
    pub fn lookup_list(&self) -> impl Iterator<Item = (u16, u16, u16)> + '_ {
        let lookup_count = self
            .bytes
            .get(self.lookup_list_off as usize..)
            .and_then(|s| read_u16(s, 0).ok())
            .unwrap_or(0);
        (0..lookup_count).filter_map(move |i| {
            let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, i)?;
            if lookup.len() < 6 {
                return None;
            }
            let mut kind = read_u16(lookup, 0).ok()?;
            let sub_count = read_u16(lookup, 4).ok()?;
            // Peek through LookupType-9 ExtensionPos to report the
            // wrapped type — callers shouldn't have to know the
            // sub-table is wrapped.
            if kind == LOOKUP_EXTENSION_POS && sub_count > 0 {
                if let Some(t) = peek_extension_type(lookup) {
                    kind = t;
                }
            }
            Some((i, kind, sub_count))
        })
    }

    /// Apply GPOS LookupType 1 (Single Adjustment Positioning) lookup
    /// `lookup_index` to `gid`.
    ///
    /// Returns `Some(PosValue)` when the lookup's coverage covers
    /// `gid`, or `None` when no rule applies. Both formats are
    /// supported:
    ///
    /// - **Format 1** — one shared `ValueRecord` applied to every
    ///   covered glyph (typical for "shift this whole script up by N
    ///   units" features).
    /// - **Format 2** — per-glyph `ValueRecord` indexed by the
    ///   coverage index (per-glyph trim used by `cpsp` etc.).
    ///
    /// ExtensionPos (LookupType 9) wrappers are unwrapped
    /// transparently. Walks every sub-table; first hit wins per the
    /// OpenType "first matching subtable in lookup order" rule.
    pub fn apply_lookup_type_1(&self, lookup_index: u16, gid: u16) -> Option<PosValue> {
        if self.lookup_list_off == 0 {
            return None;
        }
        let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, lookup_index)?;
        if lookup.len() < 6 {
            return None;
        }
        let kind = read_u16(lookup, 0).ok()?;
        let sub_count = read_u16(lookup, 4).ok()? as usize;
        for s in 0..sub_count {
            let sub_off = read_u16(lookup, 6 + s * 2).ok()? as usize;
            let sub = lookup.get(sub_off..)?;
            let (effective_kind, effective_sub) = unwrap_extension(kind, sub)?;
            if effective_kind != LOOKUP_SINGLE_POS {
                continue;
            }
            if let Some(v) = single_pos_lookup(effective_sub, gid) {
                return Some(v);
            }
        }
        None
    }

    /// Apply GPOS LookupType 3 (Cursive Attachment) lookup
    /// `lookup_index` to `gid`.
    ///
    /// Returns `Some(CursiveAttachment { entry, exit })` when the
    /// lookup's coverage covers `gid`, or `None` when no rule applies.
    /// Either anchor may be `None` (the spec allows null offsets for
    /// glyphs that don't connect on one side: a "first-of-cluster"
    /// glyph carries only `exit`; a "last-of-cluster" carries only
    /// `entry`).
    ///
    /// Cursive attachment is what powers Arabic Nastaliq and most
    /// Brahmic-script "cursive" fonts: the shaper chains glyph N+1 by
    /// translating its pen origin so glyph N+1's `entry` anchor lands
    /// on glyph N's `exit` anchor — i.e. the per-glyph delta to apply
    /// is `prev.exit - this.entry` in (x, y) font units.
    ///
    /// Only CursivePosFormat1 is defined by the spec. Anchor formats
    /// 1, 2 and 3 are accepted (format 2's anchor point and format
    /// 3's device tables are silently ignored). ExtensionPos
    /// (LookupType 9) wrappers — both at the lookup level and at the
    /// sub-table level — are unwrapped transparently.
    pub fn apply_lookup_type_3(&self, lookup_index: u16, gid: u16) -> Option<CursiveAttachment> {
        if self.lookup_list_off == 0 {
            return None;
        }
        let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, lookup_index)?;
        if lookup.len() < 6 {
            return None;
        }
        let kind = read_u16(lookup, 0).ok()?;
        let sub_count = read_u16(lookup, 4).ok()? as usize;
        for s in 0..sub_count {
            let sub_off = read_u16(lookup, 6 + s * 2).ok()? as usize;
            let sub = lookup.get(sub_off..)?;
            let (effective_kind, effective_sub) = unwrap_extension(kind, sub)?;
            if effective_kind != LOOKUP_CURSIVE_POS {
                continue;
            }
            if let Some(v) = cursive_pos_lookup(effective_sub, gid) {
                return Some(v);
            }
        }
        None
    }

    /// Walk every GPOS LookupType-3 (Cursive Attachment) lookup looking
    /// for `gid`'s entry/exit anchor pair.
    ///
    /// Convenience wrapper that scans the entire LookupList rather than
    /// a single lookup index — useful when the caller hasn't resolved
    /// the active feature's lookup-index list yet, or when there's only
    /// one cursive lookup in the font (the common case for Arabic
    /// Nastaliq fonts that ship a single `curs` lookup). Returns the
    /// first hit in lookup order (matches the OpenType "first matching
    /// subtable in lookup order" rule).
    pub fn lookup_cursive_attachment(&self, gid: u16) -> Option<CursiveAttachment> {
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
                let (effective_kind, effective_sub) = unwrap_extension(kind, sub)?;
                if effective_kind != LOOKUP_CURSIVE_POS {
                    continue;
                }
                if let Some(v) = cursive_pos_lookup(effective_sub, gid) {
                    return Some(v);
                }
            }
        }
        None
    }

    /// Apply GPOS LookupType 5 (Mark-to-Ligature Attachment) lookup
    /// `lookup_index` to a `(ligature, ligature_component, mark)` triple.
    ///
    /// Returns `Some((dx, dy))` in font units (TT Y-up) — the offset
    /// to add to the mark's pen origin so its class anchor lands on the
    /// selected component's anchor on the ligature glyph. Returns
    /// `None` when no MarkLigPosFormat1 sub-table covers both glyphs,
    /// when `ligature_component` is out of range for the matched
    /// ligature, or when the mark's class has no anchor on that
    /// component.
    ///
    /// `ligature_component` is **0-indexed**: component 0 is the
    /// first component (e.g. `f` in the `fi` ligature), component 1
    /// is the second (e.g. `i`). The shaper picks the component
    /// based on Unicode-cluster boundaries — a base mark attaches to
    /// the component whose source codepoint it follows.
    ///
    /// Only MarkLigPosFormat1 is defined by the spec. Anchor formats
    /// 1, 2 and 3 are accepted (format 2's anchor point and format
    /// 3's device tables are silently ignored). ExtensionPos
    /// (LookupType 9) wrappers are unwrapped transparently.
    pub fn apply_lookup_type_5(
        &self,
        lookup_index: u16,
        ligature: u16,
        ligature_component: u16,
        mark: u16,
    ) -> Option<(i16, i16)> {
        if self.lookup_list_off == 0 {
            return None;
        }
        let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, lookup_index)?;
        if lookup.len() < 6 {
            return None;
        }
        let kind = read_u16(lookup, 0).ok()?;
        let sub_count = read_u16(lookup, 4).ok()? as usize;
        for s in 0..sub_count {
            let sub_off = read_u16(lookup, 6 + s * 2).ok()? as usize;
            let sub = lookup.get(sub_off..)?;
            let (effective_kind, effective_sub) = unwrap_extension(kind, sub)?;
            if effective_kind != LOOKUP_MARK_LIGATURE_POS {
                continue;
            }
            if let Some(v) =
                mark_ligature_pos_lookup(effective_sub, ligature, ligature_component, mark)
            {
                return Some(v);
            }
        }
        None
    }

    /// Walk every GPOS LookupType-5 (Mark-to-Ligature) lookup looking
    /// for the `(ligature, ligature_component, mark)` triple.
    ///
    /// Convenience wrapper that scans the LookupList rather than a
    /// single lookup index. First hit in lookup order wins.
    pub fn lookup_mark_to_ligature(
        &self,
        ligature: u16,
        ligature_component: u16,
        mark: u16,
    ) -> Option<(i16, i16)> {
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
                let (effective_kind, effective_sub) = unwrap_extension(kind, sub)?;
                if effective_kind != LOOKUP_MARK_LIGATURE_POS {
                    continue;
                }
                if let Some(v) =
                    mark_ligature_pos_lookup(effective_sub, ligature, ligature_component, mark)
                {
                    return Some(v);
                }
            }
        }
        None
    }

    /// Apply GPOS LookupType 8 (Chained Contexts Positioning) lookup
    /// `lookup_index` to the glyph run starting at `pos`.
    ///
    /// Returns `Some(records)` — a `Vec<PosRecord>` listing every
    /// per-glyph adjustment the matched chain rule emits — when one
    /// of the lookup's sub-tables matches the
    /// `(backtrack, input, lookahead)` window around `pos`. Each
    /// `PosRecord.glyph_index` is an absolute offset into `gids`. The
    /// caller folds the deltas into its own glyph-position state.
    ///
    /// All three sub-table formats are supported (same wire format as
    /// GSUB LookupType 6 — only the per-record dispatch differs):
    ///
    /// - **Format 1** — Coverage on the first input glyph + per-coverage
    ///   ChainPosRuleSet of explicit `(backtrack, input, lookahead)`
    ///   glyph sequences plus per-rule `PosLookupRecord[]`.
    /// - **Format 2** — Coverage on the first input glyph + three
    ///   ClassDefs (backtrack/input/lookahead) + per-input-class
    ///   ChainPosClassSet whose rules are class sequences instead of
    ///   glyph sequences.
    /// - **Format 3** — three independent Coverage[] arrays
    ///   (backtrack / input / lookahead) + a single
    ///   `PosLookupRecord[]`.
    ///
    /// Each `PosLookupRecord { sequenceIndex, lookupListIndex }` is
    /// recursively dispatched into LookupType 1 (single position),
    /// LookupType 2 (pair / kern), LookupType 4 (mark-to-base),
    /// LookupType 6 (mark-to-mark) or LookupType 8 (recursive chain).
    /// Recursion is bounded by `MAX_NESTED_LOOKUP_DEPTH = 8` to defuse
    /// pathological self-referential graphs. ExtensionPos (LookupType 9)
    /// wrappers are unwrapped transparently.
    pub fn apply_lookup_type_8(
        &self,
        lookup_index: u16,
        gids: &[u16],
        pos: usize,
    ) -> Option<Vec<PosRecord>> {
        self.apply_chain_context_at(lookup_index, gids, pos, 0)
    }

    /// Apply GPOS LookupType 7 (Contextual Positioning) lookup
    /// `lookup_index` to the glyph run starting at `pos`.
    ///
    /// LookupType 7 is the non-chained sibling of LookupType 8: it
    /// matches an input glyph sequence (no backtrack / lookahead window)
    /// and, on a hit, dispatches the rule's `SequenceLookupRecord[]`
    /// into nested per-glyph positioning lookups exactly as the chained
    /// path does. It is the GPOS analogue of GSUB LookupType 5.
    ///
    /// Returns `Some(records)` — a `Vec<PosRecord>` listing every
    /// per-glyph adjustment the matched rule emits — when one of the
    /// lookup's sub-tables matches the input window at `pos`. Each
    /// `PosRecord.glyph_index` is an absolute offset into `gids`; the
    /// caller folds the deltas into its own glyph-position state.
    ///
    /// All three sub-table formats are supported — the wire shapes are
    /// the shared `SequenceContext` tables of the OpenType Layout Common
    /// Table Formats chapter (`SequenceContextFormat1/2/3`):
    ///
    /// - **Format 1** — Coverage on the first input glyph + per-coverage
    ///   `SequenceRuleSet` of explicit input-glyph sequences plus per-rule
    ///   `SequenceLookupRecord[]`.
    /// - **Format 2** — Coverage on the first input glyph plus a
    ///   `ClassDef` and per-input-class `ClassSequenceRuleSet` whose
    ///   rules are class sequences instead of glyph sequences.
    /// - **Format 3** — an array of per-position Coverage tables (one per
    ///   input glyph) + a single `SequenceLookupRecord[]`.
    ///
    /// Each `SequenceLookupRecord { sequenceIndex, lookupListIndex }`
    /// is recursively dispatched into LookupType 1 / 2 / 3 / 4 / 6 / 8
    /// (the same nested-dispatch table the chained path uses), bounded
    /// by `MAX_NESTED_LOOKUP_DEPTH`. ExtensionPos (LookupType 9)
    /// wrappers are unwrapped transparently.
    pub fn apply_lookup_type_7(
        &self,
        lookup_index: u16,
        gids: &[u16],
        pos: usize,
    ) -> Option<Vec<PosRecord>> {
        self.apply_context_at(lookup_index, gids, pos, 0)
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

/// Walk a CursivePosFormat1 subtable looking for `gid` and returning
/// its `(entry, exit)` anchor pair.
///
/// CursivePosFormat1 layout (OpenType spec §"Cursive Attachment
/// Positioning Subtable"):
/// ```text
///   u16 format == 1
///   Offset16 coverageOffset
///   u16 entryExitCount
///   EntryExitRecord entryExitRecords[entryExitCount]
///       Offset16 entryAnchorOffset    // 0 = null = no entry
///       Offset16 exitAnchorOffset     // 0 = null = no exit
/// ```
///
/// Both offsets are relative to the **subtable** start. The Coverage
/// table indexes the EntryExitRecord array (record `i` belongs to the
/// `i`th covered glyph in coverage order).
fn cursive_pos_lookup(sub: &[u8], gid: u16) -> Option<CursiveAttachment> {
    if sub.len() < 6 {
        return None;
    }
    let format = read_u16(sub, 0).ok()?;
    if format != 1 {
        return None;
    }
    let coverage_off = read_u16(sub, 2).ok()? as usize;
    let entry_exit_count = read_u16(sub, 4).ok()? as usize;
    let cov = sub.get(coverage_off..)?;
    let cov_idx = coverage_lookup(cov, gid)? as usize;
    if cov_idx >= entry_exit_count {
        return None;
    }
    // Each EntryExitRecord = 4 bytes (two Offset16). Header = 6 bytes.
    let rec_off = 6 + cov_idx * 4;
    if sub.len() < rec_off + 4 {
        return None;
    }
    let entry_off = read_u16(sub, rec_off).ok()? as usize;
    let exit_off = read_u16(sub, rec_off + 2).ok()? as usize;
    let entry = if entry_off == 0 {
        None
    } else {
        sub.get(entry_off..).and_then(parse_anchor)
    };
    let exit = if exit_off == 0 {
        None
    } else {
        sub.get(exit_off..).and_then(parse_anchor)
    };
    // The spec allows both offsets to be 0 (degenerate); we still
    // surface that as Some({None, None}) so the caller can distinguish
    // "covered but no anchors" from "not covered".
    Some(CursiveAttachment { entry, exit })
}

/// Walk a MarkLigPosFormat1 subtable looking for the
/// `(ligature, ligature_component, mark)` triple and return the
/// `(dx, dy)` mark-attachment offset in font units.
///
/// MarkLigPosFormat1 layout (OpenType spec §"Mark-to-Ligature
/// Attachment Positioning Subtable"):
/// ```text
///   u16 format == 1
///   Offset16 markCoverageOffset       // covers all mark glyphs
///   Offset16 ligatureCoverageOffset   // covers all ligature glyphs
///   u16 markClassCount
///   Offset16 markArrayOffset
///   Offset16 ligatureArrayOffset
/// ```
///
/// MarkArray is identical to MarkBasePos / MarkMarkPos:
/// ```text
///   u16 markCount
///   markRecords[markCount] = { u16 markClass; Offset16 markAnchorOffset; }
/// ```
///
/// LigatureArray:
/// ```text
///   u16 ligatureCount
///   Offset16 ligatureAttachOffsets[ligatureCount]
///
///   LigatureAttach (per ligature):
///     u16 componentCount
///     componentRecords[componentCount]:
///       Offset16 ligatureAnchorOffsets[markClassCount]
/// ```
///
/// Returned offset is `ligature_anchor - mark_anchor` in TT (Y-up) font
/// units. The shaper applies it as `mark.x_offset += dx`,
/// `mark.y_offset += dy` minus the un-attached pen advance.
fn mark_ligature_pos_lookup(
    sub: &[u8],
    ligature: u16,
    ligature_component: u16,
    mark: u16,
) -> Option<(i16, i16)> {
    if sub.len() < 12 {
        return None;
    }
    let format = read_u16(sub, 0).ok()?;
    if format != 1 {
        return None;
    }
    let mark_cov_off = read_u16(sub, 2).ok()? as usize;
    let lig_cov_off = read_u16(sub, 4).ok()? as usize;
    let mark_class_count = read_u16(sub, 6).ok()? as usize;
    let mark_array_off = read_u16(sub, 8).ok()? as usize;
    let lig_array_off = read_u16(sub, 10).ok()? as usize;

    let mark_cov = sub.get(mark_cov_off..)?;
    let lig_cov = sub.get(lig_cov_off..)?;
    let mark_idx = coverage_lookup(mark_cov, mark)? as usize;
    let lig_idx = coverage_lookup(lig_cov, ligature)? as usize;

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
    let mark_anchor = mark_array.get(mark_anchor_off_local..)?;
    let (mx, my) = parse_anchor(mark_anchor)?;

    // LigatureArray: ligatureCount + ligatureAttachOffsets[lig_idx]
    let lig_array = sub.get(lig_array_off..)?;
    if lig_array.len() < 2 {
        return None;
    }
    let lig_count = read_u16(lig_array, 0).ok()? as usize;
    if lig_idx >= lig_count {
        return None;
    }
    let lig_attach_off = read_u16(lig_array, 2 + lig_idx * 2).ok()? as usize;
    let lig_attach = lig_array.get(lig_attach_off..)?;
    if lig_attach.len() < 2 {
        return None;
    }
    let component_count = read_u16(lig_attach, 0).ok()? as usize;
    if (ligature_component as usize) >= component_count {
        return None;
    }
    // Component record = markClassCount Offset16 anchor offsets.
    let comp_record_size = mark_class_count * 2;
    let comp_off = 2 + ligature_component as usize * comp_record_size;
    let lig_anchor_off_local = read_u16(lig_attach, comp_off + mark_class * 2).ok()? as usize;
    // Null offset → this component has no anchor for this mark class.
    if lig_anchor_off_local == 0 {
        return None;
    }
    // Component-anchor offsets are relative to the LigatureAttach start.
    let lig_anchor = lig_attach.get(lig_anchor_off_local..)?;
    let (lx, ly) = parse_anchor(lig_anchor)?;

    Some((lx.wrapping_sub(mx), ly.wrapping_sub(my)))
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
        1..=3 => Some((x, y)),
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

/// On-disk byte size of a ValueRecord with `value_format` set.
///
/// Each set bit in the low byte (placement / advance) contributes 2
/// bytes (`int16`); each set bit in the high byte (device-table
/// offsets) contributes 2 bytes (`Offset16`) which we read past but
/// never dereference.
fn value_record_size(value_format: u16) -> usize {
    popcount_u16(value_format) * 2
}

/// Decode a ValueRecord starting at `bytes[off]` per `value_format`.
///
/// The four geometric fields are populated from their corresponding
/// `valueFormat` bits; bits that aren't set leave the field at `0`.
/// Device-table offsets in the high byte are skipped over (we read
/// past them but never dereference them — TT bytecode hinting is out
/// of scope for this crate).
fn parse_value_record(bytes: &[u8], off: usize, value_format: u16) -> PosValue {
    let mut v = PosValue::default();
    let mut p = off;
    if value_format & VF_X_PLACEMENT != 0 {
        v.x_placement = read_i16(bytes, p).unwrap_or(0);
        p += 2;
    }
    if value_format & VF_Y_PLACEMENT != 0 {
        v.y_placement = read_i16(bytes, p).unwrap_or(0);
        p += 2;
    }
    if value_format & VF_X_ADVANCE != 0 {
        v.x_advance = read_i16(bytes, p).unwrap_or(0);
        p += 2;
    }
    if value_format & VF_Y_ADVANCE != 0 {
        v.y_advance = read_i16(bytes, p).unwrap_or(0);
        p += 2;
    }
    // Skip the four device-table offsets (we don't dereference them).
    if value_format & VF_X_PLA_DEVICE != 0 {
        p += 2;
    }
    if value_format & VF_Y_PLA_DEVICE != 0 {
        p += 2;
    }
    if value_format & VF_X_ADV_DEVICE != 0 {
        p += 2;
    }
    if value_format & VF_Y_ADV_DEVICE != 0 {
        p += 2;
    }
    let _ = p;
    v
}

/// Walk a SinglePos sub-table (format 1 or 2) looking for `gid`.
///
/// SinglePosFormat1 layout:
/// ```text
///   u16 format = 1
///   Offset16 coverageOffset
///   u16 valueFormat
///   ValueRecord value
/// ```
///
/// SinglePosFormat2 layout:
/// ```text
///   u16 format = 2
///   Offset16 coverageOffset
///   u16 valueFormat
///   u16 valueCount
///   ValueRecord values[valueCount]   // indexed by coverage index
/// ```
fn single_pos_lookup(sub: &[u8], gid: u16) -> Option<PosValue> {
    if sub.len() < 6 {
        return None;
    }
    let format = read_u16(sub, 0).ok()?;
    let coverage_off = read_u16(sub, 2).ok()? as usize;
    let value_format = read_u16(sub, 4).ok()?;
    let cov = sub.get(coverage_off..)?;
    let cov_idx = coverage_lookup(cov, gid)? as usize;
    let vr_size = value_record_size(value_format);
    match format {
        1 => {
            // Single shared ValueRecord at offset 6.
            Some(parse_value_record(sub, 6, value_format))
        }
        2 => {
            let value_count = read_u16(sub, 6).ok()? as usize;
            if cov_idx >= value_count {
                return None;
            }
            let vr_off = 8 + cov_idx * vr_size;
            if sub.len() < vr_off + vr_size {
                return None;
            }
            Some(parse_value_record(sub, vr_off, value_format))
        }
        _ => None,
    }
}

/// Unwrap a LookupType-9 ExtensionPos subtable to its inner
/// `(effective_kind, effective_sub)`. Returns `None` on truncation.
/// Non-extension sub-tables pass through unchanged.
fn unwrap_extension(kind: u16, sub: &[u8]) -> Option<(u16, &[u8])> {
    if kind == LOOKUP_EXTENSION_POS {
        if sub.len() < 8 {
            return None;
        }
        let ext_type = read_u16(sub, 2).ok()?;
        let ext_off = read_u32(sub, 4).ok()? as usize;
        let ext = sub.get(ext_off..)?;
        Some((ext_type, ext))
    } else {
        Some((kind, sub))
    }
}

/// Peek through a Lookup table that holds a single ExtensionPos
/// sub-table and return the wrapped lookup type.
fn peek_extension_type(lookup: &[u8]) -> Option<u16> {
    if lookup.len() < 8 {
        return None;
    }
    let sub_count = read_u16(lookup, 4).ok()? as usize;
    if sub_count == 0 {
        return None;
    }
    let sub_off = read_u16(lookup, 6).ok()? as usize;
    let sub = lookup.get(sub_off..)?;
    if sub.len() < 8 {
        return None;
    }
    // ExtensionPosFormat1: u16 format=1, u16 extensionLookupType, Offset32.
    read_u16(sub, 2).ok()
}

/// Decode a `PosLookupRecord` array of length `count` starting at
/// `offset` inside `bytes`. Returns `None` on truncation.
fn read_pos_lookup_records(
    bytes: &[u8],
    offset: usize,
    count: usize,
) -> Option<Vec<PosLookupRecord>> {
    if bytes.len() < offset + count * 4 {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let off = offset + i * 4;
        let seq = read_u16(bytes, off).ok()?;
        let lk = read_u16(bytes, off + 2).ok()?;
        out.push(PosLookupRecord {
            sequence_index: seq,
            lookup_index: lk,
        });
    }
    Some(out)
}

#[derive(Debug, Clone, Copy)]
struct PosLookupRecord {
    sequence_index: u16,
    lookup_index: u16,
}

/// Outcome of a chained-context positioning match: the input window
/// length plus the `PosLookupRecord` array to apply against it.
#[derive(Debug)]
struct ChainPosMatch {
    input_len: usize,
    records: Vec<PosLookupRecord>,
}

impl<'a> GposTable<'a> {
    /// Non-chained context dispatch (LookupType 7). Mirror of
    /// [`Self::apply_chain_context_at`] for the `SequenceContext`
    /// sub-table family — no backtrack / lookahead window, otherwise
    /// the same per-format match → `apply_pos_records` flow.
    fn apply_context_at(
        &self,
        lookup_index: u16,
        gids: &[u16],
        pos: usize,
        depth: u8,
    ) -> Option<Vec<PosRecord>> {
        if depth >= MAX_NESTED_LOOKUP_DEPTH {
            return None;
        }
        if pos >= gids.len() || self.lookup_list_off == 0 {
            return None;
        }
        let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, lookup_index)?;
        if lookup.len() < 6 {
            return None;
        }
        let kind = read_u16(lookup, 0).ok()?;
        let sub_count = read_u16(lookup, 4).ok()? as usize;
        for s in 0..sub_count {
            let sub_off = read_u16(lookup, 6 + s * 2).ok()? as usize;
            let sub = lookup.get(sub_off..)?;
            let (effective_kind, effective_sub) = match unwrap_extension(kind, sub) {
                Some(p) => p,
                None => continue,
            };
            if effective_kind != LOOKUP_CONTEXT_POS {
                continue;
            }
            if effective_sub.len() < 2 {
                continue;
            }
            let format = match read_u16(effective_sub, 0) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let matched = match format {
                1 => context_pos_format1_match(effective_sub, gids, pos),
                2 => context_pos_format2_match(effective_sub, gids, pos),
                3 => context_pos_format3_match(effective_sub, gids, pos),
                _ => None,
            };
            if let Some(m) = matched {
                return Some(self.apply_pos_records(gids, pos, &m, depth));
            }
        }
        None
    }

    fn apply_chain_context_at(
        &self,
        lookup_index: u16,
        gids: &[u16],
        pos: usize,
        depth: u8,
    ) -> Option<Vec<PosRecord>> {
        if depth >= MAX_NESTED_LOOKUP_DEPTH {
            return None;
        }
        if pos >= gids.len() || self.lookup_list_off == 0 {
            return None;
        }
        let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, lookup_index)?;
        if lookup.len() < 6 {
            return None;
        }
        let kind = read_u16(lookup, 0).ok()?;
        let sub_count = read_u16(lookup, 4).ok()? as usize;
        for s in 0..sub_count {
            let sub_off = read_u16(lookup, 6 + s * 2).ok()? as usize;
            let sub = lookup.get(sub_off..)?;
            let (effective_kind, effective_sub) = match unwrap_extension(kind, sub) {
                Some(p) => p,
                None => continue,
            };
            if effective_kind != LOOKUP_CHAIN_CONTEXT_POS {
                continue;
            }
            if effective_sub.len() < 2 {
                continue;
            }
            let format = match read_u16(effective_sub, 0) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let matched = match format {
                1 => chain_context_pos_format1_match(effective_sub, gids, pos),
                2 => chain_context_pos_format2_match(effective_sub, gids, pos),
                3 => chain_context_pos_format3_match(effective_sub, gids, pos),
                _ => None,
            };
            if let Some(m) = matched {
                return Some(self.apply_pos_records(gids, pos, &m, depth));
            }
        }
        None
    }

    /// Apply a chain-context match's `PosLookupRecord[]` against the
    /// input run, returning the accumulated [`PosRecord`] adjustments.
    ///
    /// `m.input_len` glyphs at `gids[pos..pos + input_len]` are the
    /// chained-context "input" window. We walk the records in declared
    /// order, dispatching each `(sequenceIndex, lookupListIndex)` into
    /// the appropriate per-type apply path. Unlike GSUB, GPOS doesn't
    /// rewrite the glyph stream — every nested lookup just emits more
    /// `PosRecord`s into the output list.
    fn apply_pos_records(
        &self,
        gids: &[u16],
        pos: usize,
        m: &ChainPosMatch,
        depth: u8,
    ) -> Vec<PosRecord> {
        let mut out: Vec<PosRecord> = Vec::new();
        for rec in &m.records {
            let seq_idx = rec.sequence_index as usize;
            if seq_idx >= m.input_len {
                continue;
            }
            let abs_idx = pos + seq_idx;
            if abs_idx >= gids.len() {
                continue;
            }
            // Resolve the referenced lookup type and dispatch.
            let lookup =
                match lookup_table_slice(self.bytes, self.lookup_list_off, rec.lookup_index) {
                    Some(s) => s,
                    None => continue,
                };
            if lookup.len() < 2 {
                continue;
            }
            let mut nested_kind = match read_u16(lookup, 0) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // ExtensionPos at the nested lookup level: peek through.
            if nested_kind == LOOKUP_EXTENSION_POS {
                if let Some(t) = peek_extension_type(lookup) {
                    nested_kind = t;
                }
            }
            match nested_kind {
                LOOKUP_SINGLE_POS => {
                    if let Some(v) = self.apply_lookup_type_1(rec.lookup_index, gids[abs_idx]) {
                        out.push(PosRecord {
                            glyph_index: abs_idx,
                            value: v,
                        });
                    }
                }
                LOOKUP_PAIR_POS if abs_idx + 1 < gids.len() => {
                    // Pair-pos under chain context: needs a *right*
                    // glyph at abs_idx + 1. Apply only the xAdvance
                    // delta on the left glyph, matching the standalone
                    // kerning entry point.
                    let dx = self.lookup_kerning(gids[abs_idx], gids[abs_idx + 1], None);
                    if dx != 0 {
                        out.push(PosRecord {
                            glyph_index: abs_idx,
                            value: PosValue {
                                x_advance: dx,
                                ..PosValue::default()
                            },
                        });
                    }
                }
                LOOKUP_CURSIVE_POS if abs_idx > 0 => {
                    // Cursive-pos under chain context: chain glyph N+1
                    // (= abs_idx) onto glyph N (= abs_idx - 1). Compute
                    // `prev.exit - this.entry`; emit only when both
                    // anchors exist for the pair.
                    let prev = self.apply_lookup_type_3(rec.lookup_index, gids[abs_idx - 1]);
                    let curr = self.apply_lookup_type_3(rec.lookup_index, gids[abs_idx]);
                    if let (Some(p), Some(c)) = (prev, curr) {
                        if let (Some((px, py)), Some((cx, cy))) = (p.exit, c.entry) {
                            out.push(PosRecord {
                                glyph_index: abs_idx,
                                value: PosValue {
                                    x_placement: px.wrapping_sub(cx),
                                    y_placement: py.wrapping_sub(cy),
                                    ..PosValue::default()
                                },
                            });
                        }
                    }
                }
                LOOKUP_MARK_BASE_POS if abs_idx + 1 < gids.len() => {
                    // Mark-to-base under chain context: the base is at
                    // abs_idx and the mark sits at abs_idx + 1. (The
                    // spec leaves the matching to the chain rule; the
                    // base/mark roles are implied by the rule's window
                    // shape.)
                    if let Some((dx, dy)) = self.lookup_mark_to_base_via_lookup(
                        rec.lookup_index,
                        gids[abs_idx],
                        gids[abs_idx + 1],
                    ) {
                        out.push(PosRecord {
                            glyph_index: abs_idx + 1,
                            value: PosValue {
                                x_placement: dx,
                                y_placement: dy,
                                ..PosValue::default()
                            },
                        });
                    }
                }
                LOOKUP_MARK_MARK_POS if abs_idx + 1 < gids.len() => {
                    if let Some((dx, dy)) = self.lookup_mark_to_mark_via_lookup(
                        rec.lookup_index,
                        gids[abs_idx],
                        gids[abs_idx + 1],
                    ) {
                        out.push(PosRecord {
                            glyph_index: abs_idx + 1,
                            value: PosValue {
                                x_placement: dx,
                                y_placement: dy,
                                ..PosValue::default()
                            },
                        });
                    }
                }
                LOOKUP_CONTEXT_POS => {
                    if let Some(mut nested) =
                        self.apply_context_at(rec.lookup_index, gids, abs_idx, depth + 1)
                    {
                        out.append(&mut nested);
                    }
                }
                LOOKUP_CHAIN_CONTEXT_POS => {
                    if let Some(mut nested) =
                        self.apply_chain_context_at(rec.lookup_index, gids, abs_idx, depth + 1)
                    {
                        out.append(&mut nested);
                    }
                }
                _ => {
                    // Unsupported nested lookup type — silently skip,
                    // matches our GSUB policy for unknown types.
                }
            }
        }
        out
    }

    /// Walk a single LookupType-4 lookup looking for `(base, mark)`.
    /// Used by the chain-context dispatcher when a `PosLookupRecord`
    /// references a specific mark-to-base lookup index. Mirror of
    /// [`Self::lookup_mark_to_base`] scoped to one lookup.
    fn lookup_mark_to_base_via_lookup(
        &self,
        lookup_index: u16,
        base: u16,
        mark: u16,
    ) -> Option<(i16, i16)> {
        let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, lookup_index)?;
        if lookup.len() < 6 {
            return None;
        }
        let kind = read_u16(lookup, 0).ok()?;
        let sub_count = read_u16(lookup, 4).ok()? as usize;
        for s in 0..sub_count {
            let sub_off = read_u16(lookup, 6 + s * 2).ok()? as usize;
            let sub = lookup.get(sub_off..)?;
            let (effective_kind, effective_sub) = unwrap_extension(kind, sub)?;
            if effective_kind != LOOKUP_MARK_BASE_POS {
                continue;
            }
            if let Some(v) = mark_base_pos_lookup(effective_sub, base, mark) {
                return Some(v);
            }
        }
        None
    }

    /// Walk a single LookupType-6 lookup looking for `(mark1, mark2)`.
    /// Companion of [`Self::lookup_mark_to_base_via_lookup`].
    fn lookup_mark_to_mark_via_lookup(
        &self,
        lookup_index: u16,
        mark1: u16,
        mark2: u16,
    ) -> Option<(i16, i16)> {
        let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, lookup_index)?;
        if lookup.len() < 6 {
            return None;
        }
        let kind = read_u16(lookup, 0).ok()?;
        let sub_count = read_u16(lookup, 4).ok()? as usize;
        for s in 0..sub_count {
            let sub_off = read_u16(lookup, 6 + s * 2).ok()? as usize;
            let sub = lookup.get(sub_off..)?;
            let (effective_kind, effective_sub) = unwrap_extension(kind, sub)?;
            if effective_kind != LOOKUP_MARK_MARK_POS {
                continue;
            }
            if let Some(v) = mark_mark_pos_lookup(effective_sub, mark1, mark2) {
                return Some(v);
            }
        }
        None
    }
}

/// Match a SequenceContextFormat1 sub-table against `gids[pos..]`
/// (GPOS LookupType 7, format 1).
///
/// Layout (per OpenType §"Sequence Context Format 1: simple glyph
/// contexts" in the Common Table Formats chapter):
/// ```text
///   u16 format = 1
///   Offset16 coverageOffset             (input[0] coverage)
///   u16 seqRuleSetCount
///   Offset16 seqRuleSetOffsets[seqRuleSetCount]   (may be NULL)
///
///   SequenceRuleSet { u16 seqRuleCount; Offset16 seqRuleOffsets[]; }
///   SequenceRule    { u16 glyphCount; u16 seqLookupCount;
///                     u16 inputSequence[glyphCount - 1];
///                     SequenceLookupRecord seqLookupRecords[seqLookupCount]; }
/// ```
///
/// The covered glyph's coverage index selects the `SequenceRuleSet`;
/// the first rule whose input sequence matches `gids` is used.
fn context_pos_format1_match(sub: &[u8], gids: &[u16], pos: usize) -> Option<ChainPosMatch> {
    if sub.len() < 6 {
        return None;
    }
    let coverage_off = read_u16(sub, 2).ok()? as usize;
    let set_count = read_u16(sub, 4).ok()? as usize;
    let coverage = sub.get(coverage_off..)?;
    let cov_idx = coverage_lookup(coverage, gids[pos])? as usize;
    if cov_idx >= set_count {
        return None;
    }
    let set_off = read_u16(sub, 6 + cov_idx * 2).ok()? as usize;
    if set_off == 0 {
        return None;
    }
    let set = sub.get(set_off..)?;
    if set.len() < 2 {
        return None;
    }
    let rule_count = read_u16(set, 0).ok()? as usize;
    for r in 0..rule_count {
        let rule_off = read_u16(set, 2 + r * 2).ok()? as usize;
        let rule = match set.get(rule_off..) {
            Some(b) => b,
            None => continue,
        };
        if let Some(m) = context_pos_format1_rule_match(rule, gids, pos) {
            return Some(m);
        }
    }
    None
}

fn context_pos_format1_rule_match(rule: &[u8], gids: &[u16], pos: usize) -> Option<ChainPosMatch> {
    if rule.len() < 4 {
        return None;
    }
    let glyph_count = read_u16(rule, 0).ok()? as usize;
    if glyph_count == 0 {
        return None;
    }
    let seq_lookup_count = read_u16(rule, 2).ok()? as usize;
    let in_extra = glyph_count - 1;
    let mut cur = 4usize;
    if rule.len() < cur + in_extra * 2 {
        return None;
    }
    if pos + glyph_count > gids.len() {
        return None;
    }
    // inputSequence starts with the SECOND glyph (index 0 == sequence
    // position 1); the first glyph was already matched by Coverage.
    for i in 0..in_extra {
        let want = read_u16(rule, cur + i * 2).ok()?;
        if gids[pos + 1 + i] != want {
            return None;
        }
    }
    cur += in_extra * 2;
    let records = read_pos_lookup_records(rule, cur, seq_lookup_count)?;
    Some(ChainPosMatch {
        input_len: glyph_count,
        records,
    })
}

/// Match a SequenceContextFormat2 sub-table against `gids[pos..]`
/// (GPOS LookupType 7, format 2).
///
/// Layout (per OpenType §"Sequence Context Format 2: class-based glyph
/// contexts"):
/// ```text
///   u16 format = 2
///   Offset16 coverageOffset
///   Offset16 classDefOffset
///   u16 classSeqRuleSetCount
///   Offset16 classSeqRuleSetOffsets[classSeqRuleSetCount]   (may be NULL)
///
///   ClassSequenceRuleSet { u16 classSeqRuleCount; Offset16 classSeqRuleOffsets[]; }
///   ClassSequenceRule    { u16 glyphCount; u16 seqLookupCount;
///                          u16 inputSequence[glyphCount - 1];   (class values)
///                          SequenceLookupRecord seqLookupRecords[seqLookupCount]; }
/// ```
///
/// Coverage gates participation; the first input glyph's class selects
/// the `ClassSequenceRuleSet`. Remaining positions are matched by class.
fn context_pos_format2_match(sub: &[u8], gids: &[u16], pos: usize) -> Option<ChainPosMatch> {
    if sub.len() < 8 {
        return None;
    }
    let coverage_off = read_u16(sub, 2).ok()? as usize;
    let class_def_off = read_u16(sub, 4).ok()? as usize;
    let set_count = read_u16(sub, 6).ok()? as usize;
    let coverage = sub.get(coverage_off..)?;
    coverage_lookup(coverage, gids[pos])?;
    let class_def = sub.get(class_def_off..)?;
    let in_class0 = class_def_lookup(class_def, gids[pos]).unwrap_or(0);
    if in_class0 as usize >= set_count {
        return None;
    }
    let set_off = read_u16(sub, 8 + in_class0 as usize * 2).ok()? as usize;
    if set_off == 0 {
        return None;
    }
    let set = sub.get(set_off..)?;
    if set.len() < 2 {
        return None;
    }
    let rule_count = read_u16(set, 0).ok()? as usize;
    for r in 0..rule_count {
        let rule_off = read_u16(set, 2 + r * 2).ok()? as usize;
        let rule = match set.get(rule_off..) {
            Some(b) => b,
            None => continue,
        };
        if let Some(m) = context_pos_format2_rule_match(rule, gids, pos, class_def) {
            return Some(m);
        }
    }
    None
}

fn context_pos_format2_rule_match(
    rule: &[u8],
    gids: &[u16],
    pos: usize,
    class_def: &[u8],
) -> Option<ChainPosMatch> {
    if rule.len() < 4 {
        return None;
    }
    let glyph_count = read_u16(rule, 0).ok()? as usize;
    if glyph_count == 0 {
        return None;
    }
    let seq_lookup_count = read_u16(rule, 2).ok()? as usize;
    let in_extra = glyph_count - 1;
    let mut cur = 4usize;
    if rule.len() < cur + in_extra * 2 {
        return None;
    }
    if pos + glyph_count > gids.len() {
        return None;
    }
    // inputSequence is class values starting at the second position.
    for i in 0..in_extra {
        let want = read_u16(rule, cur + i * 2).ok()?;
        let got = class_def_lookup(class_def, gids[pos + 1 + i]).unwrap_or(0);
        if want != got {
            return None;
        }
    }
    cur += in_extra * 2;
    let records = read_pos_lookup_records(rule, cur, seq_lookup_count)?;
    Some(ChainPosMatch {
        input_len: glyph_count,
        records,
    })
}

/// Match a SequenceContextFormat3 sub-table against `gids[pos..]`
/// (GPOS LookupType 7, format 3).
///
/// Layout (per OpenType §"Sequence Context Format 3: coverage-based
/// glyph contexts"):
/// ```text
///   u16 format = 3
///   u16 glyphCount
///   u16 seqLookupCount
///   Offset16 coverageOffsets[glyphCount]
///   SequenceLookupRecord seqLookupRecords[seqLookupCount]
/// ```
///
/// Each input position is gated by its own Coverage table; a single
/// record array applies when every position is covered.
fn context_pos_format3_match(sub: &[u8], gids: &[u16], pos: usize) -> Option<ChainPosMatch> {
    if sub.len() < 6 {
        return None;
    }
    let glyph_count = read_u16(sub, 2).ok()? as usize;
    if glyph_count == 0 {
        return None;
    }
    let seq_lookup_count = read_u16(sub, 4).ok()? as usize;
    let mut cur = 6usize;
    if sub.len() < cur + glyph_count * 2 {
        return None;
    }
    if pos + glyph_count > gids.len() {
        return None;
    }
    for i in 0..glyph_count {
        let cov_off = read_u16(sub, cur + i * 2).ok()? as usize;
        let cov = sub.get(cov_off..)?;
        coverage_lookup(cov, gids[pos + i])?;
    }
    cur += glyph_count * 2;
    let records = read_pos_lookup_records(sub, cur, seq_lookup_count)?;
    Some(ChainPosMatch {
        input_len: glyph_count,
        records,
    })
}

/// Match a ChainContextPosFormat1 sub-table against `gids[pos..]`.
///
/// Layout (per OpenType §"Chained Sequence Context Format 1: simple
/// glyph contexts" — the GPOS LookupType-8 wire format is identical
/// to GSUB LookupType-6 modulo the record array's name):
/// ```text
///   u16 format = 1
///   Offset16 coverageOffset             (input[0] coverage)
///   u16 chainPosRuleSetCount
///   Offset16 chainPosRuleSetOffsets[chainPosRuleSetCount]
///
///   ChainPosRuleSet { u16 chainPosRuleCount; Offset16 chainPosRuleOffsets[]; }
///   ChainPosRule    { u16 backtrackGlyphCount; u16 backtrackSequence[];
///                     u16 inputGlyphCount;     u16 inputSequence[inputGlyphCount-1];
///                     u16 lookaheadGlyphCount; u16 lookaheadSequence[];
///                     u16 posCount;            PosLookupRecord posRecords[]; }
/// ```
fn chain_context_pos_format1_match(sub: &[u8], gids: &[u16], pos: usize) -> Option<ChainPosMatch> {
    if sub.len() < 6 {
        return None;
    }
    let coverage_off = read_u16(sub, 2).ok()? as usize;
    let coverage = sub.get(coverage_off..)?;
    let cov_idx = coverage_lookup(coverage, gids[pos])? as usize;
    let set_count = read_u16(sub, 4).ok()? as usize;
    if cov_idx >= set_count {
        return None;
    }
    let set_off = read_u16(sub, 6 + cov_idx * 2).ok()? as usize;
    let set = sub.get(set_off..)?;
    if set.len() < 2 {
        return None;
    }
    let rule_count = read_u16(set, 0).ok()? as usize;
    for r in 0..rule_count {
        let rule_off = read_u16(set, 2 + r * 2).ok()? as usize;
        let rule = match set.get(rule_off..) {
            Some(b) => b,
            None => continue,
        };
        if let Some(m) = chain_context_pos_format1_rule_match(rule, gids, pos) {
            return Some(m);
        }
    }
    None
}

fn chain_context_pos_format1_rule_match(
    rule: &[u8],
    gids: &[u16],
    pos: usize,
) -> Option<ChainPosMatch> {
    let mut cur = 0usize;
    if rule.len() < cur + 2 {
        return None;
    }
    let bt_count = read_u16(rule, cur).ok()? as usize;
    cur += 2;
    if rule.len() < cur + bt_count * 2 {
        return None;
    }
    if pos < bt_count {
        return None;
    }
    // Backtrack sequence is reverse-text order (closest first).
    for i in 0..bt_count {
        let want = read_u16(rule, cur + i * 2).ok()?;
        if gids[pos - 1 - i] != want {
            return None;
        }
    }
    cur += bt_count * 2;
    if rule.len() < cur + 2 {
        return None;
    }
    let in_count = read_u16(rule, cur).ok()? as usize;
    if in_count == 0 {
        return None;
    }
    cur += 2;
    let in_extra = in_count - 1;
    if rule.len() < cur + in_extra * 2 {
        return None;
    }
    if pos + in_count > gids.len() {
        return None;
    }
    for i in 0..in_extra {
        let want = read_u16(rule, cur + i * 2).ok()?;
        if gids[pos + 1 + i] != want {
            return None;
        }
    }
    cur += in_extra * 2;
    if rule.len() < cur + 2 {
        return None;
    }
    let la_count = read_u16(rule, cur).ok()? as usize;
    cur += 2;
    if rule.len() < cur + la_count * 2 {
        return None;
    }
    if pos + in_count + la_count > gids.len() {
        return None;
    }
    for i in 0..la_count {
        let want = read_u16(rule, cur + i * 2).ok()?;
        if gids[pos + in_count + i] != want {
            return None;
        }
    }
    cur += la_count * 2;
    if rule.len() < cur + 2 {
        return None;
    }
    let pos_count = read_u16(rule, cur).ok()? as usize;
    cur += 2;
    let records = read_pos_lookup_records(rule, cur, pos_count)?;
    Some(ChainPosMatch {
        input_len: in_count,
        records,
    })
}

/// Match a ChainContextPosFormat2 sub-table against `gids[pos..]`.
///
/// Layout:
/// ```text
///   u16 format = 2
///   Offset16 coverageOffset
///   Offset16 backtrackClassDefOffset
///   Offset16 inputClassDefOffset
///   Offset16 lookaheadClassDefOffset
///   u16 chainPosClassSetCount
///   Offset16 chainPosClassSetOffsets[chainPosClassSetCount]
/// ```
fn chain_context_pos_format2_match(sub: &[u8], gids: &[u16], pos: usize) -> Option<ChainPosMatch> {
    if sub.len() < 12 {
        return None;
    }
    let coverage_off = read_u16(sub, 2).ok()? as usize;
    let bt_cd_off = read_u16(sub, 4).ok()? as usize;
    let in_cd_off = read_u16(sub, 6).ok()? as usize;
    let la_cd_off = read_u16(sub, 8).ok()? as usize;
    let set_count = read_u16(sub, 10).ok()? as usize;
    let coverage = sub.get(coverage_off..)?;
    coverage_lookup(coverage, gids[pos])?;
    let in_cd = sub.get(in_cd_off..)?;
    let in_class0 = class_def_lookup(in_cd, gids[pos]).unwrap_or(0);
    if in_class0 as usize >= set_count {
        return None;
    }
    let set_off = read_u16(sub, 12 + in_class0 as usize * 2).ok()? as usize;
    if set_off == 0 {
        return None;
    }
    let set = sub.get(set_off..)?;
    if set.len() < 2 {
        return None;
    }
    let rule_count = read_u16(set, 0).ok()? as usize;
    let bt_cd = sub.get(bt_cd_off..);
    let la_cd = sub.get(la_cd_off..);
    for r in 0..rule_count {
        let rule_off = read_u16(set, 2 + r * 2).ok()? as usize;
        let rule = match set.get(rule_off..) {
            Some(b) => b,
            None => continue,
        };
        if let Some(m) =
            chain_context_pos_format2_rule_match(rule, gids, pos, bt_cd, in_cd, la_cd, in_class0)
        {
            return Some(m);
        }
    }
    None
}

fn chain_context_pos_format2_rule_match(
    rule: &[u8],
    gids: &[u16],
    pos: usize,
    bt_cd: Option<&[u8]>,
    in_cd: &[u8],
    la_cd: Option<&[u8]>,
    in_class0: u16,
) -> Option<ChainPosMatch> {
    let mut cur = 0usize;
    if rule.len() < cur + 2 {
        return None;
    }
    let bt_count = read_u16(rule, cur).ok()? as usize;
    cur += 2;
    if rule.len() < cur + bt_count * 2 {
        return None;
    }
    if pos < bt_count {
        return None;
    }
    if bt_count > 0 {
        let bt_cd = bt_cd?;
        for i in 0..bt_count {
            let want = read_u16(rule, cur + i * 2).ok()?;
            let got = class_def_lookup(bt_cd, gids[pos - 1 - i]).unwrap_or(0);
            if want != got {
                return None;
            }
        }
    }
    cur += bt_count * 2;
    if rule.len() < cur + 2 {
        return None;
    }
    let in_count = read_u16(rule, cur).ok()? as usize;
    if in_count == 0 {
        return None;
    }
    cur += 2;
    let in_extra = in_count - 1;
    if rule.len() < cur + in_extra * 2 {
        return None;
    }
    if pos + in_count > gids.len() {
        return None;
    }
    for i in 0..in_extra {
        let want = read_u16(rule, cur + i * 2).ok()?;
        let got = class_def_lookup(in_cd, gids[pos + 1 + i]).unwrap_or(0);
        if want != got {
            return None;
        }
    }
    cur += in_extra * 2;
    let _ = in_class0;
    if rule.len() < cur + 2 {
        return None;
    }
    let la_count = read_u16(rule, cur).ok()? as usize;
    cur += 2;
    if rule.len() < cur + la_count * 2 {
        return None;
    }
    if pos + in_count + la_count > gids.len() {
        return None;
    }
    if la_count > 0 {
        let la_cd = la_cd?;
        for i in 0..la_count {
            let want = read_u16(rule, cur + i * 2).ok()?;
            let got = class_def_lookup(la_cd, gids[pos + in_count + i]).unwrap_or(0);
            if want != got {
                return None;
            }
        }
    }
    cur += la_count * 2;
    if rule.len() < cur + 2 {
        return None;
    }
    let pos_count = read_u16(rule, cur).ok()? as usize;
    cur += 2;
    let records = read_pos_lookup_records(rule, cur, pos_count)?;
    Some(ChainPosMatch {
        input_len: in_count,
        records,
    })
}

/// Match a ChainContextPosFormat3 sub-table against `gids[pos..]`.
///
/// Layout:
/// ```text
///   u16 format = 3
///   u16 backtrackGlyphCount
///   Offset16 backtrackCoverageOffsets[backtrackGlyphCount]
///   u16 inputGlyphCount
///   Offset16 inputCoverageOffsets[inputGlyphCount]
///   u16 lookaheadGlyphCount
///   Offset16 lookaheadCoverageOffsets[lookaheadGlyphCount]
///   u16 posCount
///   PosLookupRecord posRecords[posCount]
/// ```
fn chain_context_pos_format3_match(sub: &[u8], gids: &[u16], pos: usize) -> Option<ChainPosMatch> {
    if sub.len() < 4 {
        return None;
    }
    let mut cur = 2usize;
    let bt_count = read_u16(sub, cur).ok()? as usize;
    cur += 2;
    if sub.len() < cur + bt_count * 2 {
        return None;
    }
    if pos < bt_count {
        return None;
    }
    for i in 0..bt_count {
        let cov_off = read_u16(sub, cur + i * 2).ok()? as usize;
        let cov = sub.get(cov_off..)?;
        coverage_lookup(cov, gids[pos - 1 - i])?;
    }
    cur += bt_count * 2;
    if sub.len() < cur + 2 {
        return None;
    }
    let in_count = read_u16(sub, cur).ok()? as usize;
    if in_count == 0 {
        return None;
    }
    cur += 2;
    if sub.len() < cur + in_count * 2 {
        return None;
    }
    if pos + in_count > gids.len() {
        return None;
    }
    for i in 0..in_count {
        let cov_off = read_u16(sub, cur + i * 2).ok()? as usize;
        let cov = sub.get(cov_off..)?;
        coverage_lookup(cov, gids[pos + i])?;
    }
    cur += in_count * 2;
    if sub.len() < cur + 2 {
        return None;
    }
    let la_count = read_u16(sub, cur).ok()? as usize;
    cur += 2;
    if sub.len() < cur + la_count * 2 {
        return None;
    }
    if pos + in_count + la_count > gids.len() {
        return None;
    }
    for i in 0..la_count {
        let cov_off = read_u16(sub, cur + i * 2).ok()? as usize;
        let cov = sub.get(cov_off..)?;
        coverage_lookup(cov, gids[pos + in_count + i])?;
    }
    cur += la_count * 2;
    if sub.len() < cur + 2 {
        return None;
    }
    let pos_count = read_u16(sub, cur).ok()? as usize;
    cur += 2;
    let records = read_pos_lookup_records(sub, cur, pos_count)?;
    Some(ChainPosMatch {
        input_len: in_count,
        records,
    })
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

    // ---- LookupType 1 (single positioning) -------------------------

    /// Wrap a sub-table into a Lookup{type, flag=0, subCount=1, subOff=8}.
    fn wrap_lookup(lookup_type: u16, sub: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&lookup_type.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&8u16.to_be_bytes());
        out.extend_from_slice(sub);
        out
    }

    /// Wrap a single Lookup into a GPOS header (LookupList = [lookup]).
    fn wrap_gpos_single(lookup: &[u8]) -> Vec<u8> {
        // LookupList: count=1, offsets=[4]
        let mut lookup_list = Vec::new();
        lookup_list.extend_from_slice(&1u16.to_be_bytes());
        lookup_list.extend_from_slice(&4u16.to_be_bytes());
        lookup_list.extend_from_slice(lookup);
        // GPOS header: major, minor, scriptList, featureList, lookupList=10.
        let mut gpos = Vec::new();
        gpos.extend_from_slice(&1u16.to_be_bytes());
        gpos.extend_from_slice(&0u16.to_be_bytes());
        gpos.extend_from_slice(&0u16.to_be_bytes());
        gpos.extend_from_slice(&0u16.to_be_bytes());
        gpos.extend_from_slice(&10u16.to_be_bytes());
        gpos.extend_from_slice(&lookup_list);
        gpos
    }

    /// SinglePosFormat1 — coverage covers gid 7 + 9, all share xAdv = -150.
    fn build_single_pos_format1() -> Vec<u8> {
        // Coverage format 1: gids [7, 9].
        let mut cov = Vec::new();
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&2u16.to_be_bytes());
        cov.extend_from_slice(&7u16.to_be_bytes());
        cov.extend_from_slice(&9u16.to_be_bytes());
        // Sub-table: format=1, covOff=8, valueFormat=VF_X_ADVANCE,
        //            ValueRecord{x_adv = -150}
        let mut sub = Vec::new();
        sub.extend_from_slice(&1u16.to_be_bytes()); // format
        sub.extend_from_slice(&8u16.to_be_bytes()); // covOff
        sub.extend_from_slice(&VF_X_ADVANCE.to_be_bytes());
        sub.extend_from_slice(&(-150i16).to_be_bytes()); // x_adv
        sub.extend_from_slice(&cov);
        let lookup = wrap_lookup(LOOKUP_SINGLE_POS, &sub);
        wrap_gpos_single(&lookup)
    }

    /// SinglePosFormat2 — gids [7, 9], per-glyph (xPlace, yPlace, xAdv).
    fn build_single_pos_format2() -> Vec<u8> {
        let mut cov = Vec::new();
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&2u16.to_be_bytes());
        cov.extend_from_slice(&7u16.to_be_bytes());
        cov.extend_from_slice(&9u16.to_be_bytes());
        // valueFormat = X_PLACEMENT | Y_PLACEMENT | X_ADVANCE = 7
        let vf = VF_X_PLACEMENT | VF_Y_PLACEMENT | VF_X_ADVANCE;
        // valueRecord size = 6 bytes; valueCount = 2 → 12 bytes of records.
        // header = 8 bytes (format + cov_off + valueFormat + valueCount)
        // + 12 bytes records + cov.len() at the end → cov_off = 20.
        let cov_off = 20u16;
        let mut sub = Vec::new();
        sub.extend_from_slice(&2u16.to_be_bytes()); // format
        sub.extend_from_slice(&cov_off.to_be_bytes());
        sub.extend_from_slice(&vf.to_be_bytes());
        sub.extend_from_slice(&2u16.to_be_bytes()); // valueCount
                                                    // Record [0] (gid 7): x_pl=10 y_pl=20 x_adv=30
        sub.extend_from_slice(&10i16.to_be_bytes());
        sub.extend_from_slice(&20i16.to_be_bytes());
        sub.extend_from_slice(&30i16.to_be_bytes());
        // Record [1] (gid 9): x_pl=-5 y_pl=-15 x_adv=40
        sub.extend_from_slice(&(-5i16).to_be_bytes());
        sub.extend_from_slice(&(-15i16).to_be_bytes());
        sub.extend_from_slice(&40i16.to_be_bytes());
        sub.extend_from_slice(&cov);
        let lookup = wrap_lookup(LOOKUP_SINGLE_POS, &sub);
        wrap_gpos_single(&lookup)
    }

    #[test]
    fn single_pos_format1_returns_shared_value_for_every_covered_glyph() {
        let bytes = build_single_pos_format1();
        let g = GposTable::parse(&bytes).unwrap();
        let v7 = g.apply_lookup_type_1(0, 7).unwrap();
        let v9 = g.apply_lookup_type_1(0, 9).unwrap();
        assert_eq!(v7, v9);
        assert_eq!(v7.x_advance, -150);
        // Off-coverage glyph → None.
        assert_eq!(g.apply_lookup_type_1(0, 8), None);
    }

    #[test]
    fn single_pos_format2_returns_per_glyph_value() {
        let bytes = build_single_pos_format2();
        let g = GposTable::parse(&bytes).unwrap();
        let v7 = g.apply_lookup_type_1(0, 7).unwrap();
        assert_eq!(v7.x_placement, 10);
        assert_eq!(v7.y_placement, 20);
        assert_eq!(v7.x_advance, 30);
        let v9 = g.apply_lookup_type_1(0, 9).unwrap();
        assert_eq!(v9.x_placement, -5);
        assert_eq!(v9.y_placement, -15);
        assert_eq!(v9.x_advance, 40);
        assert_eq!(g.apply_lookup_type_1(0, 8), None);
    }

    #[test]
    fn single_pos_returns_none_when_lookup_index_out_of_range() {
        let bytes = build_single_pos_format1();
        let g = GposTable::parse(&bytes).unwrap();
        assert_eq!(g.apply_lookup_type_1(99, 7), None);
    }

    #[test]
    fn single_pos_returns_none_when_lookup_is_not_type_1() {
        // Re-use the pair-pos kerning fixture — its single lookup is
        // LookupType 2, not 1, so apply_lookup_type_1 must return None.
        let bytes = build_simple_pp1();
        let g = GposTable::parse(&bytes).unwrap();
        assert_eq!(g.apply_lookup_type_1(0, 50), None);
    }

    // ---- LookupType 8 (chained context positioning) ----------------

    /// Build a GPOS table with two lookups:
    ///   lookup 0: SinglePos Format 1, covers gid 10, x_adv = +50
    ///   lookup 1: ChainContextPos Format 1, matches bt=[1] in=[10]
    ///             la=[99], records=[(seq=0, lookupIndex=0)].
    fn build_chain_context_pos_format1() -> Vec<u8> {
        // ---- lookup 0 sub-table: SinglePos Format 1 ----
        let mut cov0 = Vec::new();
        cov0.extend_from_slice(&1u16.to_be_bytes());
        cov0.extend_from_slice(&1u16.to_be_bytes());
        cov0.extend_from_slice(&10u16.to_be_bytes());
        let mut sub0 = Vec::new();
        sub0.extend_from_slice(&1u16.to_be_bytes()); // format
        sub0.extend_from_slice(&8u16.to_be_bytes()); // covOff
        sub0.extend_from_slice(&VF_X_ADVANCE.to_be_bytes());
        sub0.extend_from_slice(&50i16.to_be_bytes());
        sub0.extend_from_slice(&cov0);
        let lookup0 = wrap_lookup(LOOKUP_SINGLE_POS, &sub0);

        // ---- lookup 1 sub-table: ChainContextPos Format 1 ----
        // Coverage covering input[0] = gid 10.
        let mut cov_in = Vec::new();
        cov_in.extend_from_slice(&1u16.to_be_bytes());
        cov_in.extend_from_slice(&1u16.to_be_bytes());
        cov_in.extend_from_slice(&10u16.to_be_bytes());
        // Rule body
        let mut rule = Vec::new();
        rule.extend_from_slice(&1u16.to_be_bytes()); // bt count
        rule.extend_from_slice(&1u16.to_be_bytes()); // bt[0]
        rule.extend_from_slice(&1u16.to_be_bytes()); // in count
                                                     // (no extra inputs for in_count=1)
        rule.extend_from_slice(&1u16.to_be_bytes()); // la count
        rule.extend_from_slice(&99u16.to_be_bytes()); // la[0]
        rule.extend_from_slice(&1u16.to_be_bytes()); // posCount
        rule.extend_from_slice(&0u16.to_be_bytes()); // seqIndex
        rule.extend_from_slice(&0u16.to_be_bytes()); // lookupIndex → 0
                                                     // RuleSet header: count=1 + offset[0] (after the header)
        let rule_set_header_len = 4u16;
        let mut rule_set = Vec::new();
        rule_set.extend_from_slice(&1u16.to_be_bytes());
        rule_set.extend_from_slice(&rule_set_header_len.to_be_bytes());
        rule_set.extend_from_slice(&rule);
        // Sub-table header: 8 bytes (format + cov_off + setCount + setOff[0])
        let header_len = 8u16;
        let cov_off = header_len;
        let set_off = cov_off + cov_in.len() as u16;
        let mut sub1 = Vec::new();
        sub1.extend_from_slice(&1u16.to_be_bytes()); // format
        sub1.extend_from_slice(&cov_off.to_be_bytes());
        sub1.extend_from_slice(&1u16.to_be_bytes()); // setCount
        sub1.extend_from_slice(&set_off.to_be_bytes());
        sub1.extend_from_slice(&cov_in);
        sub1.extend_from_slice(&rule_set);
        let lookup1 = wrap_lookup(LOOKUP_CHAIN_CONTEXT_POS, &sub1);

        // ---- LookupList: count=2, offsets[lookup0, lookup1] ----
        let lookup_list_header_len = 2 + 2 * 2;
        let mut lookup_list = Vec::new();
        lookup_list.extend_from_slice(&2u16.to_be_bytes());
        let mut running = lookup_list_header_len as u16;
        lookup_list.extend_from_slice(&running.to_be_bytes());
        running += lookup0.len() as u16;
        lookup_list.extend_from_slice(&running.to_be_bytes());
        lookup_list.extend_from_slice(&lookup0);
        lookup_list.extend_from_slice(&lookup1);

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
    fn chain_context_pos_format1_dispatches_nested_single_pos() {
        let bytes = build_chain_context_pos_format1();
        let g = GposTable::parse(&bytes).unwrap();
        // Run [1, 10, 99]; chain rule fires at pos=1 → emit one
        // PosRecord at glyph_index=1 with x_advance = +50.
        let recs = g.apply_lookup_type_8(1, &[1, 10, 99], 1).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].glyph_index, 1);
        assert_eq!(recs[0].value.x_advance, 50);
    }

    #[test]
    fn chain_context_pos_format1_no_match_when_backtrack_or_lookahead_misses() {
        let bytes = build_chain_context_pos_format1();
        let g = GposTable::parse(&bytes).unwrap();
        // Wrong backtrack glyph.
        assert_eq!(g.apply_lookup_type_8(1, &[2, 10, 99], 1), None);
        // No backtrack room.
        assert_eq!(g.apply_lookup_type_8(1, &[10, 99], 0), None);
        // Wrong lookahead.
        assert_eq!(g.apply_lookup_type_8(1, &[1, 10, 50], 1), None);
        // Out-of-range lookup index.
        assert_eq!(g.apply_lookup_type_8(99, &[1, 10, 99], 1), None);
    }

    /// Build a Format-3 chain-context pos sub-table:
    /// backtrack covers [1], input covers [10], lookahead covers [99],
    /// invokes single-pos lookup 0 (gid 10 → x_adv = +50).
    fn build_chain_context_pos_format3() -> Vec<u8> {
        // lookup 0: SinglePos Format 1, gid 10, x_adv = +50.
        let mut cov_lookup0 = Vec::new();
        cov_lookup0.extend_from_slice(&1u16.to_be_bytes());
        cov_lookup0.extend_from_slice(&1u16.to_be_bytes());
        cov_lookup0.extend_from_slice(&10u16.to_be_bytes());
        let mut sub0 = Vec::new();
        sub0.extend_from_slice(&1u16.to_be_bytes());
        sub0.extend_from_slice(&8u16.to_be_bytes());
        sub0.extend_from_slice(&VF_X_ADVANCE.to_be_bytes());
        sub0.extend_from_slice(&50i16.to_be_bytes());
        sub0.extend_from_slice(&cov_lookup0);
        let lookup0 = wrap_lookup(LOOKUP_SINGLE_POS, &sub0);

        // Three coverages.
        let mut cov_bt = Vec::new();
        cov_bt.extend_from_slice(&1u16.to_be_bytes());
        cov_bt.extend_from_slice(&1u16.to_be_bytes());
        cov_bt.extend_from_slice(&1u16.to_be_bytes());
        let mut cov_in = Vec::new();
        cov_in.extend_from_slice(&1u16.to_be_bytes());
        cov_in.extend_from_slice(&1u16.to_be_bytes());
        cov_in.extend_from_slice(&10u16.to_be_bytes());
        let mut cov_la = Vec::new();
        cov_la.extend_from_slice(&1u16.to_be_bytes());
        cov_la.extend_from_slice(&1u16.to_be_bytes());
        cov_la.extend_from_slice(&99u16.to_be_bytes());

        // Format-3 sub-table header: 18 bytes (same as GSUB version):
        // 2 (format) + 2 (btCount) + 2 (btOff[0]) + 2 (inCount)
        // + 2 (inOff[0]) + 2 (laCount) + 2 (laOff[0]) + 2 (posCount)
        // + 4 (PosLookupRecord[0]) = 20? Actually let's recount:
        // 2+2+2+2+2+2+2+2+4 = 20. Wait, GSUB says 18. But GSUB had bt+in+la
        // counts each followed by ONE offset, then substCount, then 4-byte
        // record. So 2+2+2+2+2+2+2+2+4 = 20. Hmm let me recount the GSUB
        // build: 2+2+2+2+2+2+2+2+4 = 20 in fact. The comment on GSUB said 18
        // but counted wrong. So header_len = 20.
        let header_len: u16 = 2 + 2 + 2 + 2 + 2 + 2 + 2 + 2 + 4;
        let bt_off = header_len;
        let in_off = bt_off + cov_bt.len() as u16;
        let la_off = in_off + cov_in.len() as u16;

        let mut sub1 = Vec::new();
        sub1.extend_from_slice(&3u16.to_be_bytes()); // format
        sub1.extend_from_slice(&1u16.to_be_bytes()); // btCount
        sub1.extend_from_slice(&bt_off.to_be_bytes());
        sub1.extend_from_slice(&1u16.to_be_bytes()); // inCount
        sub1.extend_from_slice(&in_off.to_be_bytes());
        sub1.extend_from_slice(&1u16.to_be_bytes()); // laCount
        sub1.extend_from_slice(&la_off.to_be_bytes());
        sub1.extend_from_slice(&1u16.to_be_bytes()); // posCount
        sub1.extend_from_slice(&0u16.to_be_bytes()); // seqIndex
        sub1.extend_from_slice(&0u16.to_be_bytes()); // lookupIndex → 0
        sub1.extend_from_slice(&cov_bt);
        sub1.extend_from_slice(&cov_in);
        sub1.extend_from_slice(&cov_la);
        let lookup1 = wrap_lookup(LOOKUP_CHAIN_CONTEXT_POS, &sub1);

        // LookupList.
        let lookup_list_header_len = 2 + 2 * 2;
        let mut lookup_list = Vec::new();
        lookup_list.extend_from_slice(&2u16.to_be_bytes());
        let mut running = lookup_list_header_len as u16;
        lookup_list.extend_from_slice(&running.to_be_bytes());
        running += lookup0.len() as u16;
        lookup_list.extend_from_slice(&running.to_be_bytes());
        lookup_list.extend_from_slice(&lookup0);
        lookup_list.extend_from_slice(&lookup1);

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
    fn chain_context_pos_format3_coverage_based_dispatch() {
        let bytes = build_chain_context_pos_format3();
        let g = GposTable::parse(&bytes).unwrap();
        let recs = g.apply_lookup_type_8(1, &[1, 10, 99], 1).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].glyph_index, 1);
        assert_eq!(recs[0].value.x_advance, 50);
    }

    #[test]
    fn chain_context_pos_format3_no_match_when_window_short_or_uncovered() {
        let bytes = build_chain_context_pos_format3();
        let g = GposTable::parse(&bytes).unwrap();
        // Backtrack required, pos=0 leaves none.
        assert_eq!(g.apply_lookup_type_8(1, &[10, 99], 0), None);
        // No lookahead.
        assert_eq!(g.apply_lookup_type_8(1, &[1, 10], 1), None);
        // Lookahead glyph not in coverage.
        assert_eq!(g.apply_lookup_type_8(1, &[1, 10, 12], 1), None);
    }

    // ---- Format-2 (class-based) chain-context positioning ----------

    /// Build ChainContextPos Format-2 sub-table where:
    ///   - input class def: gid 10 → class 1, gid 11 → class 1
    ///   - backtrack class def: gid 1 → class 1, gid 2 → class 1
    ///   - lookahead class def: gid 99 → class 1
    ///   - one rule under input class set 1: bt=[1] in=[1] la=[1] +
    ///     PosLookupRecord (seq=0, lookup=0) → SinglePos lookup 0
    fn build_chain_context_pos_format2() -> Vec<u8> {
        // Classdefs are format 2 ranges.
        // ClassDef format 2: u16 format=2, u16 rangeCount, ClassRangeRecord[].
        // Each record = u16 startGlyph, u16 endGlyph, u16 class.
        let mut in_cd = Vec::new();
        in_cd.extend_from_slice(&2u16.to_be_bytes());
        in_cd.extend_from_slice(&1u16.to_be_bytes()); // 1 range
        in_cd.extend_from_slice(&10u16.to_be_bytes()); // start
        in_cd.extend_from_slice(&11u16.to_be_bytes()); // end
        in_cd.extend_from_slice(&1u16.to_be_bytes()); // class

        let mut bt_cd = Vec::new();
        bt_cd.extend_from_slice(&2u16.to_be_bytes());
        bt_cd.extend_from_slice(&1u16.to_be_bytes());
        bt_cd.extend_from_slice(&1u16.to_be_bytes());
        bt_cd.extend_from_slice(&2u16.to_be_bytes());
        bt_cd.extend_from_slice(&1u16.to_be_bytes());

        let mut la_cd = Vec::new();
        la_cd.extend_from_slice(&2u16.to_be_bytes());
        la_cd.extend_from_slice(&1u16.to_be_bytes());
        la_cd.extend_from_slice(&99u16.to_be_bytes());
        la_cd.extend_from_slice(&99u16.to_be_bytes());
        la_cd.extend_from_slice(&1u16.to_be_bytes());

        // Coverage: covers input[0] candidates (gid 10).
        let mut cov = Vec::new();
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&10u16.to_be_bytes());

        // Rule body
        let mut rule = Vec::new();
        rule.extend_from_slice(&1u16.to_be_bytes()); // btCount
        rule.extend_from_slice(&1u16.to_be_bytes()); // bt[0] = class 1
        rule.extend_from_slice(&1u16.to_be_bytes()); // inCount = 1, no extras
        rule.extend_from_slice(&1u16.to_be_bytes()); // laCount
        rule.extend_from_slice(&1u16.to_be_bytes()); // la[0] = class 1
        rule.extend_from_slice(&1u16.to_be_bytes()); // posCount
        rule.extend_from_slice(&0u16.to_be_bytes()); // seq
        rule.extend_from_slice(&0u16.to_be_bytes()); // lookupIndex

        // Set: count=1 + offset to rule (after 4-byte set header)
        let mut set = Vec::new();
        set.extend_from_slice(&1u16.to_be_bytes()); // ruleCount
        set.extend_from_slice(&4u16.to_be_bytes()); // offset
        set.extend_from_slice(&rule);

        // Sub-table header: 12 bytes (format + cov + bt_cd + in_cd + la_cd
        // + setCount) + 2 * setCount = 14 bytes. setCount = 2 because
        // class 0 is implicit before class 1; offsets[0] = 0 (no set),
        // offsets[1] = real.
        // header_len: 14 bytes (12 + 2 set offsets)
        let set_count = 2u16;
        let header_len = 12 + (set_count as usize) * 2;
        let cov_off = header_len as u16;
        let bt_cd_off = cov_off + cov.len() as u16;
        let in_cd_off = bt_cd_off + bt_cd.len() as u16;
        let la_cd_off = in_cd_off + in_cd.len() as u16;
        let set_off = la_cd_off + la_cd.len() as u16;

        let mut sub1 = Vec::new();
        sub1.extend_from_slice(&2u16.to_be_bytes()); // format
        sub1.extend_from_slice(&cov_off.to_be_bytes());
        sub1.extend_from_slice(&bt_cd_off.to_be_bytes());
        sub1.extend_from_slice(&in_cd_off.to_be_bytes());
        sub1.extend_from_slice(&la_cd_off.to_be_bytes());
        sub1.extend_from_slice(&set_count.to_be_bytes());
        // setOffsets[0] (class 0) = 0 (no rules)
        sub1.extend_from_slice(&0u16.to_be_bytes());
        // setOffsets[1] (class 1) = set_off
        sub1.extend_from_slice(&set_off.to_be_bytes());
        sub1.extend_from_slice(&cov);
        sub1.extend_from_slice(&bt_cd);
        sub1.extend_from_slice(&in_cd);
        sub1.extend_from_slice(&la_cd);
        sub1.extend_from_slice(&set);

        // lookup 0: SinglePos Format 1, gid 10, x_adv = +50.
        let mut cov0 = Vec::new();
        cov0.extend_from_slice(&1u16.to_be_bytes());
        cov0.extend_from_slice(&1u16.to_be_bytes());
        cov0.extend_from_slice(&10u16.to_be_bytes());
        let mut sub0 = Vec::new();
        sub0.extend_from_slice(&1u16.to_be_bytes());
        sub0.extend_from_slice(&8u16.to_be_bytes());
        sub0.extend_from_slice(&VF_X_ADVANCE.to_be_bytes());
        sub0.extend_from_slice(&50i16.to_be_bytes());
        sub0.extend_from_slice(&cov0);
        let lookup0 = wrap_lookup(LOOKUP_SINGLE_POS, &sub0);

        let lookup1 = wrap_lookup(LOOKUP_CHAIN_CONTEXT_POS, &sub1);

        let lookup_list_header_len = 2 + 2 * 2;
        let mut lookup_list = Vec::new();
        lookup_list.extend_from_slice(&2u16.to_be_bytes());
        let mut running = lookup_list_header_len as u16;
        lookup_list.extend_from_slice(&running.to_be_bytes());
        running += lookup0.len() as u16;
        lookup_list.extend_from_slice(&running.to_be_bytes());
        lookup_list.extend_from_slice(&lookup0);
        lookup_list.extend_from_slice(&lookup1);

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
    fn chain_context_pos_format2_class_based_dispatch() {
        let bytes = build_chain_context_pos_format2();
        let g = GposTable::parse(&bytes).unwrap();
        let recs = g.apply_lookup_type_8(1, &[1, 10, 99], 1).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].glyph_index, 1);
        assert_eq!(recs[0].value.x_advance, 50);
    }

    #[test]
    fn chain_context_pos_format2_no_match_when_class_differs() {
        let bytes = build_chain_context_pos_format2();
        let g = GposTable::parse(&bytes).unwrap();
        // gid 5 isn't in any backtrack class → bt class 0 ≠ rule's class 1.
        assert_eq!(g.apply_lookup_type_8(1, &[5, 10, 99], 1), None);
    }

    // ---- LookupList enumeration ------------------------------------

    #[test]
    fn lookup_list_reports_index_type_and_subtable_count() {
        // pp1 fixture: single LookupType-2 lookup with 1 sub-table.
        let bytes = build_simple_pp1();
        let g = GposTable::parse(&bytes).unwrap();
        let v: Vec<_> = g.lookup_list().collect();
        assert_eq!(v, vec![(0u16, 2u16, 1u16)]);

        // Mark-base fixture: single LookupType-4 lookup with 1 sub-table.
        let bytes = build_simple_mark_base();
        let g = GposTable::parse(&bytes).unwrap();
        let v: Vec<_> = g.lookup_list().collect();
        assert_eq!(v, vec![(0u16, 4u16, 1u16)]);

        // Chain-context-pos fixture has two lookups: 1, 8.
        let bytes = build_chain_context_pos_format1();
        let g = GposTable::parse(&bytes).unwrap();
        let v: Vec<_> = g.lookup_list().collect();
        assert_eq!(v, vec![(0u16, 1u16, 1u16), (1u16, 8u16, 1u16)]);
    }

    #[test]
    fn value_record_size_packs_low_byte_only() {
        // VF_X_PLACEMENT | VF_Y_PLACEMENT = 2 fields × 2 bytes = 4.
        assert_eq!(value_record_size(VF_X_PLACEMENT | VF_Y_PLACEMENT), 4);
        // All four geometric + all four device = 8 fields × 2 bytes = 16.
        let all = VF_X_PLACEMENT
            | VF_Y_PLACEMENT
            | VF_X_ADVANCE
            | VF_Y_ADVANCE
            | VF_X_PLA_DEVICE
            | VF_Y_PLA_DEVICE
            | VF_X_ADV_DEVICE
            | VF_Y_ADV_DEVICE;
        assert_eq!(value_record_size(all), 16);
        // Empty value format → 0 bytes.
        assert_eq!(value_record_size(0), 0);
    }

    // ---- LookupType 3 (cursive attachment) -------------------------

    /// Build a CursivePosFormat1 sub-table covering glyphs 5 (entry only),
    /// 6 (both entry and exit), 7 (exit only). Anchor coords are
    /// (entry.x, entry.y) = (10*gid, 100), (exit.x, exit.y) = (20*gid, 200).
    fn build_cursive_pos_format1() -> Vec<u8> {
        // Anchor table format 1: u16 format + i16 x + i16 y = 6 bytes.
        fn anchor(x: i16, y: i16) -> Vec<u8> {
            let mut v = Vec::new();
            v.extend_from_slice(&1u16.to_be_bytes());
            v.extend_from_slice(&x.to_be_bytes());
            v.extend_from_slice(&y.to_be_bytes());
            v
        }
        // Anchors: 6 bytes each. We need:
        //   gid 5 entry (50, 100)
        //   gid 6 entry (60, 100), exit (120, 200)
        //   gid 7 exit  (140, 200)
        let a5_entry = anchor(50, 100);
        let a6_entry = anchor(60, 100);
        let a6_exit = anchor(120, 200);
        let a7_exit = anchor(140, 200);

        // Coverage format 1 covering [5, 6, 7].
        let mut cov = Vec::new();
        cov.extend_from_slice(&1u16.to_be_bytes()); // format
        cov.extend_from_slice(&3u16.to_be_bytes()); // glyphCount
        cov.extend_from_slice(&5u16.to_be_bytes());
        cov.extend_from_slice(&6u16.to_be_bytes());
        cov.extend_from_slice(&7u16.to_be_bytes());

        // Sub-table header: format(2) + covOff(2) + entryExitCount(2)
        // + 3 EntryExitRecords (4 bytes each) = 6 + 12 = 18 bytes
        let header_len = 6 + 3 * 4;
        let cov_off = (header_len + 4 * 6) as u16; // anchors live before coverage
                                                   // anchor placement: right after header.
        let a5_entry_off = header_len as u16;
        let a6_entry_off = a5_entry_off + 6;
        let a6_exit_off = a6_entry_off + 6;
        let a7_exit_off = a6_exit_off + 6;

        let mut sub = Vec::new();
        sub.extend_from_slice(&1u16.to_be_bytes()); // format
        sub.extend_from_slice(&cov_off.to_be_bytes());
        sub.extend_from_slice(&3u16.to_be_bytes()); // entryExitCount
                                                    // Record [0] gid 5: entry only (exit = 0/null)
        sub.extend_from_slice(&a5_entry_off.to_be_bytes());
        sub.extend_from_slice(&0u16.to_be_bytes());
        // Record [1] gid 6: both
        sub.extend_from_slice(&a6_entry_off.to_be_bytes());
        sub.extend_from_slice(&a6_exit_off.to_be_bytes());
        // Record [2] gid 7: exit only (entry = 0/null)
        sub.extend_from_slice(&0u16.to_be_bytes());
        sub.extend_from_slice(&a7_exit_off.to_be_bytes());
        // Anchors
        sub.extend_from_slice(&a5_entry);
        sub.extend_from_slice(&a6_entry);
        sub.extend_from_slice(&a6_exit);
        sub.extend_from_slice(&a7_exit);
        // Coverage
        sub.extend_from_slice(&cov);

        let lookup = wrap_lookup(LOOKUP_CURSIVE_POS, &sub);
        wrap_gpos_single(&lookup)
    }

    #[test]
    fn cursive_pos_format1_returns_entry_and_exit_anchors() {
        let bytes = build_cursive_pos_format1();
        let g = GposTable::parse(&bytes).unwrap();
        // gid 5 has only entry (50, 100).
        let r5 = g.apply_lookup_type_3(0, 5).unwrap();
        assert_eq!(r5.entry, Some((50, 100)));
        assert_eq!(r5.exit, None);
        // gid 6 has both.
        let r6 = g.apply_lookup_type_3(0, 6).unwrap();
        assert_eq!(r6.entry, Some((60, 100)));
        assert_eq!(r6.exit, Some((120, 200)));
        // gid 7 has only exit.
        let r7 = g.apply_lookup_type_3(0, 7).unwrap();
        assert_eq!(r7.entry, None);
        assert_eq!(r7.exit, Some((140, 200)));
    }

    #[test]
    fn cursive_pos_returns_none_off_coverage() {
        let bytes = build_cursive_pos_format1();
        let g = GposTable::parse(&bytes).unwrap();
        assert_eq!(g.apply_lookup_type_3(0, 4), None);
        assert_eq!(g.apply_lookup_type_3(0, 8), None);
    }

    #[test]
    fn cursive_pos_returns_none_when_lookup_is_not_type_3() {
        // Re-use a kerning-only GPOS — its single lookup is type 2.
        let bytes = build_simple_pp1();
        let g = GposTable::parse(&bytes).unwrap();
        assert_eq!(g.apply_lookup_type_3(0, 5), None);
    }

    #[test]
    fn lookup_cursive_attachment_walks_lookup_list() {
        let bytes = build_cursive_pos_format1();
        let g = GposTable::parse(&bytes).unwrap();
        // The convenience walker should find the same anchors
        // regardless of which lookup index hosts them.
        let r6 = g.lookup_cursive_attachment(6).unwrap();
        assert_eq!(r6.entry, Some((60, 100)));
        assert_eq!(r6.exit, Some((120, 200)));
        // No coverage hit on gid 99 in any cursive lookup.
        assert_eq!(g.lookup_cursive_attachment(99), None);
    }

    // ---- LookupType 5 (mark-to-ligature) ---------------------------

    /// Build a MarkLigPosFormat1 sub-table:
    ///   ligature gid 100 with 2 components,
    ///   mark gid 200 (class 0) with anchor (10, 0).
    ///   Component 0 anchor for class 0: (300, 800)
    ///   Component 1 anchor for class 0: (500, 850)
    /// Expected:
    ///   apply(lig=100, comp=0, mark=200) -> (300-10, 800-0) = (290, 800)
    ///   apply(lig=100, comp=1, mark=200) -> (500-10, 850-0) = (490, 850)
    fn build_mark_ligature_pos_format1() -> Vec<u8> {
        fn anchor(x: i16, y: i16) -> Vec<u8> {
            let mut v = Vec::new();
            v.extend_from_slice(&1u16.to_be_bytes());
            v.extend_from_slice(&x.to_be_bytes());
            v.extend_from_slice(&y.to_be_bytes());
            v
        }

        // ---- mark anchor + MarkArray ----
        let mark_anchor = anchor(10, 0);
        // MarkArray: markCount(2) + markRecord (4 bytes: class + offset)
        // = 6 bytes header. Anchor placed right after at offset 6.
        let mut mark_array = Vec::new();
        mark_array.extend_from_slice(&1u16.to_be_bytes()); // markCount
        mark_array.extend_from_slice(&0u16.to_be_bytes()); // class 0
        mark_array.extend_from_slice(&6u16.to_be_bytes()); // anchor offset
        mark_array.extend_from_slice(&mark_anchor);

        // ---- LigatureAttach for ligature 100, 2 components, 1 mark class ----
        // Header: componentCount(2). Each componentRecord = markClassCount * 2
        // = 2 bytes (1 anchor offset). Total component-record block = 4 bytes.
        // Anchors placed right after at offsets:
        //   header(2) + 2 component records (2 bytes each) = 6
        //   comp0 anchor at 6, comp1 anchor at 12.
        let comp0_anchor = anchor(300, 800);
        let comp1_anchor = anchor(500, 850);
        let mut lig_attach = Vec::new();
        lig_attach.extend_from_slice(&2u16.to_be_bytes()); // componentCount
        lig_attach.extend_from_slice(&6u16.to_be_bytes()); // comp0 → offset 6
        lig_attach.extend_from_slice(&12u16.to_be_bytes()); // comp1 → offset 12
        lig_attach.extend_from_slice(&comp0_anchor);
        lig_attach.extend_from_slice(&comp1_anchor);

        // ---- LigatureArray: ligatureCount(1) + offset to lig_attach
        // Header = 2 + 2 = 4 bytes. lig_attach starts at offset 4.
        let mut lig_array = Vec::new();
        lig_array.extend_from_slice(&1u16.to_be_bytes()); // ligatureCount
        lig_array.extend_from_slice(&4u16.to_be_bytes()); // offset
        lig_array.extend_from_slice(&lig_attach);

        // ---- Coverage tables ----
        let mut mark_cov = Vec::new();
        mark_cov.extend_from_slice(&1u16.to_be_bytes());
        mark_cov.extend_from_slice(&1u16.to_be_bytes());
        mark_cov.extend_from_slice(&200u16.to_be_bytes());

        let mut lig_cov = Vec::new();
        lig_cov.extend_from_slice(&1u16.to_be_bytes());
        lig_cov.extend_from_slice(&1u16.to_be_bytes());
        lig_cov.extend_from_slice(&100u16.to_be_bytes());

        // ---- MarkLigPosFormat1 sub-table (12-byte header) ----
        let header = 12usize;
        let mark_cov_off = header;
        let lig_cov_off = mark_cov_off + mark_cov.len();
        let mark_array_off = lig_cov_off + lig_cov.len();
        let lig_array_off = mark_array_off + mark_array.len();

        let mut mlp = Vec::new();
        mlp.extend_from_slice(&1u16.to_be_bytes()); // format
        mlp.extend_from_slice(&(mark_cov_off as u16).to_be_bytes());
        mlp.extend_from_slice(&(lig_cov_off as u16).to_be_bytes());
        mlp.extend_from_slice(&1u16.to_be_bytes()); // markClassCount
        mlp.extend_from_slice(&(mark_array_off as u16).to_be_bytes());
        mlp.extend_from_slice(&(lig_array_off as u16).to_be_bytes());
        mlp.extend_from_slice(&mark_cov);
        mlp.extend_from_slice(&lig_cov);
        mlp.extend_from_slice(&mark_array);
        mlp.extend_from_slice(&lig_array);

        let lookup = wrap_lookup(LOOKUP_MARK_LIGATURE_POS, &mlp);
        wrap_gpos_single(&lookup)
    }

    #[test]
    fn mark_to_ligature_attaches_to_each_component() {
        let bytes = build_mark_ligature_pos_format1();
        let g = GposTable::parse(&bytes).unwrap();
        // Component 0: (300 - 10, 800 - 0) = (290, 800)
        assert_eq!(g.apply_lookup_type_5(0, 100, 0, 200), Some((290, 800)));
        // Component 1: (500 - 10, 850 - 0) = (490, 850)
        assert_eq!(g.apply_lookup_type_5(0, 100, 1, 200), Some((490, 850)));
    }

    #[test]
    fn mark_to_ligature_returns_none_for_out_of_range_component() {
        let bytes = build_mark_ligature_pos_format1();
        let g = GposTable::parse(&bytes).unwrap();
        // Only 2 components (0, 1); component 2 is out of range.
        assert_eq!(g.apply_lookup_type_5(0, 100, 2, 200), None);
    }

    #[test]
    fn mark_to_ligature_returns_none_for_uncovered_glyphs() {
        let bytes = build_mark_ligature_pos_format1();
        let g = GposTable::parse(&bytes).unwrap();
        // Wrong ligature glyph.
        assert_eq!(g.apply_lookup_type_5(0, 101, 0, 200), None);
        // Wrong mark glyph.
        assert_eq!(g.apply_lookup_type_5(0, 100, 0, 201), None);
    }

    #[test]
    fn lookup_mark_to_ligature_walks_lookup_list() {
        let bytes = build_mark_ligature_pos_format1();
        let g = GposTable::parse(&bytes).unwrap();
        assert_eq!(g.lookup_mark_to_ligature(100, 1, 200), Some((490, 850)));
        assert_eq!(g.lookup_mark_to_ligature(99, 0, 200), None);
    }

    // ---- ExtensionPos (LookupType 9) wrapping at the lookup level --

    /// Wrap an arbitrary sub-table inside an ExtensionPosFormat1 wrapper
    /// so the resulting Lookup carries `lookupType = 9` even though its
    /// effective sub-table is `inner_type`.
    ///
    /// ExtensionPosFormat1 wire layout:
    ///   u16 format = 1
    ///   u16 extensionLookupType
    ///   Offset32 extensionOffset (relative to ExtensionPos sub-table)
    fn build_extension_wrapped_lookup(inner_type: u16, inner_sub: &[u8]) -> Vec<u8> {
        // Build the wrapper sub-table: 8-byte header + inner sub-table.
        let mut ext_sub = Vec::new();
        ext_sub.extend_from_slice(&1u16.to_be_bytes()); // format
        ext_sub.extend_from_slice(&inner_type.to_be_bytes()); // extensionLookupType
        ext_sub.extend_from_slice(&8u32.to_be_bytes()); // extensionOffset = 8
        ext_sub.extend_from_slice(inner_sub);
        // Wrap as an outer LookupType-9 lookup.
        let lookup = wrap_lookup(LOOKUP_EXTENSION_POS, &ext_sub);
        wrap_gpos_single(&lookup)
    }

    #[test]
    fn extension_wrapper_unwraps_for_cursive_pos_lookup() {
        // Build a CursivePosFormat1 sub-table by extracting it from the
        // build_cursive_pos_format1 fixture (which wraps it as a plain
        // type-3 lookup). We re-build the inner sub-table here so we
        // can wrap it with type 9 instead.
        // Easier: inline a tiny cursive sub-table covering only gid 6.
        let mut cursive_sub = Vec::new();
        // format=1, covOff=10, entryExitCount=1, then EntryExitRecord
        // (entryOff=20, exitOff=26), then anchors at 20+26.
        // header = 6 + 4 = 10.
        cursive_sub.extend_from_slice(&1u16.to_be_bytes()); // format
        cursive_sub.extend_from_slice(&22u16.to_be_bytes()); // covOff (after anchors)
        cursive_sub.extend_from_slice(&1u16.to_be_bytes()); // entryExitCount
                                                            // EntryExitRecord
        cursive_sub.extend_from_slice(&10u16.to_be_bytes()); // entryOff
        cursive_sub.extend_from_slice(&16u16.to_be_bytes()); // exitOff
                                                             // Entry anchor (1, x=70, y=110)
        cursive_sub.extend_from_slice(&1u16.to_be_bytes());
        cursive_sub.extend_from_slice(&70i16.to_be_bytes());
        cursive_sub.extend_from_slice(&110i16.to_be_bytes());
        // Exit anchor (1, x=130, y=210)
        cursive_sub.extend_from_slice(&1u16.to_be_bytes());
        cursive_sub.extend_from_slice(&130i16.to_be_bytes());
        cursive_sub.extend_from_slice(&210i16.to_be_bytes());
        // Coverage format 1 covering [6]
        cursive_sub.extend_from_slice(&1u16.to_be_bytes());
        cursive_sub.extend_from_slice(&1u16.to_be_bytes());
        cursive_sub.extend_from_slice(&6u16.to_be_bytes());

        let bytes = build_extension_wrapped_lookup(LOOKUP_CURSIVE_POS, &cursive_sub);
        let g = GposTable::parse(&bytes).unwrap();

        // The lookup is type 9 on disk; lookup_list reports the
        // *effective* type after unwrap.
        let v: Vec<_> = g.lookup_list().collect();
        assert_eq!(v, vec![(0u16, LOOKUP_CURSIVE_POS, 1u16)]);

        // apply_lookup_type_3 must transparently unwrap the LT9 wrapper.
        let r = g.apply_lookup_type_3(0, 6).unwrap();
        assert_eq!(r.entry, Some((70, 110)));
        assert_eq!(r.exit, Some((130, 210)));
    }

    // ---- LookupType 7 (contextual positioning) ---------------------

    /// SinglePos Format 1 sub-table: covers `gid`, x_adv = `adv`.
    fn single_pos_sub(gid: u16, adv: i16) -> Vec<u8> {
        let mut cov = Vec::new();
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&gid.to_be_bytes());
        let mut sub = Vec::new();
        sub.extend_from_slice(&1u16.to_be_bytes()); // format
        sub.extend_from_slice(&8u16.to_be_bytes()); // covOff
        sub.extend_from_slice(&VF_X_ADVANCE.to_be_bytes());
        sub.extend_from_slice(&adv.to_be_bytes());
        sub.extend_from_slice(&cov);
        sub
    }

    /// Assemble a 2-lookup GPOS: lookup 0 = the supplied single-pos
    /// sub-table, lookup 1 = the supplied context sub-table wrapped as
    /// LookupType 7.
    fn assemble_context_gpos(single_sub: &[u8], ctx_sub: &[u8]) -> Vec<u8> {
        let lookup0 = wrap_lookup(LOOKUP_SINGLE_POS, single_sub);
        let lookup1 = wrap_lookup(LOOKUP_CONTEXT_POS, ctx_sub);
        let lookup_list_header_len = 2 + 2 * 2;
        let mut lookup_list = Vec::new();
        lookup_list.extend_from_slice(&2u16.to_be_bytes());
        let mut running = lookup_list_header_len as u16;
        lookup_list.extend_from_slice(&running.to_be_bytes());
        running += lookup0.len() as u16;
        lookup_list.extend_from_slice(&running.to_be_bytes());
        lookup_list.extend_from_slice(&lookup0);
        lookup_list.extend_from_slice(&lookup1);

        let mut gpos = Vec::new();
        gpos.extend_from_slice(&1u16.to_be_bytes()); // major
        gpos.extend_from_slice(&0u16.to_be_bytes()); // minor
        gpos.extend_from_slice(&0u16.to_be_bytes()); // scriptList
        gpos.extend_from_slice(&0u16.to_be_bytes()); // featureList
        gpos.extend_from_slice(&10u16.to_be_bytes()); // lookupList
        gpos.extend_from_slice(&lookup_list);
        gpos
    }

    /// SequenceContextFormat1: input sequence [10, 20], emits nested
    /// single-pos lookup 0 at sequence index 1 (gid 20 → x_adv +50).
    fn build_context_pos_format1() -> Vec<u8> {
        let single_sub = single_pos_sub(20, 50);
        // Coverage covers input[0] = gid 10.
        let mut cov = Vec::new();
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&10u16.to_be_bytes());
        // SequenceRule: glyphCount=2, seqLookupCount=1,
        // inputSequence=[20] (second glyph), record (seq=1, lk=0).
        let mut rule = Vec::new();
        rule.extend_from_slice(&2u16.to_be_bytes()); // glyphCount
        rule.extend_from_slice(&1u16.to_be_bytes()); // seqLookupCount
        rule.extend_from_slice(&20u16.to_be_bytes()); // inputSequence[0]
        rule.extend_from_slice(&1u16.to_be_bytes()); // seqIndex = 1
        rule.extend_from_slice(&0u16.to_be_bytes()); // lookupIndex = 0
                                                     // SequenceRuleSet: count=1 + offset (after 4-byte header).
        let mut set = Vec::new();
        set.extend_from_slice(&1u16.to_be_bytes());
        set.extend_from_slice(&4u16.to_be_bytes());
        set.extend_from_slice(&rule);
        // Sub-table header: format + covOff + setCount + setOff[0] = 8.
        let header_len = 8u16;
        let cov_off = header_len;
        let set_off = cov_off + cov.len() as u16;
        let mut sub = Vec::new();
        sub.extend_from_slice(&1u16.to_be_bytes()); // format
        sub.extend_from_slice(&cov_off.to_be_bytes());
        sub.extend_from_slice(&1u16.to_be_bytes()); // setCount
        sub.extend_from_slice(&set_off.to_be_bytes());
        sub.extend_from_slice(&cov);
        sub.extend_from_slice(&set);
        assemble_context_gpos(&single_sub, &sub)
    }

    #[test]
    fn context_pos_format1_dispatches_nested_single_pos() {
        let bytes = build_context_pos_format1();
        let g = GposTable::parse(&bytes).unwrap();
        // Run [10, 20]; rule fires at pos=0 → emit one PosRecord at the
        // input glyph at sequence index 1 (abs index 1) with x_adv=+50.
        let recs = g.apply_lookup_type_7(1, &[10, 20], 0).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].glyph_index, 1);
        assert_eq!(recs[0].value.x_advance, 50);
    }

    #[test]
    fn context_pos_format1_no_match() {
        let bytes = build_context_pos_format1();
        let g = GposTable::parse(&bytes).unwrap();
        // Second input glyph differs.
        assert_eq!(g.apply_lookup_type_7(1, &[10, 21], 0), None);
        // First glyph not covered.
        assert_eq!(g.apply_lookup_type_7(1, &[11, 20], 0), None);
        // Window runs off the end.
        assert_eq!(g.apply_lookup_type_7(1, &[10], 0), None);
        // Out-of-range lookup index.
        assert_eq!(g.apply_lookup_type_7(99, &[10, 20], 0), None);
        // Wrong lookup type (lookup 0 is single-pos).
        assert_eq!(g.apply_lookup_type_7(0, &[10, 20], 0), None);
    }

    /// SequenceContextFormat2: class-based. Class 1 = {10}, class 2 =
    /// {20}; rule for class-1 first glyph requires class 2 at the
    /// second position; emits single-pos lookup 0 at sequence index 1.
    fn build_context_pos_format2() -> Vec<u8> {
        let single_sub = single_pos_sub(20, 50);
        // ClassDef format 2 with two ranges: 10→class1, 20→class2.
        let mut cd = Vec::new();
        cd.extend_from_slice(&2u16.to_be_bytes()); // format
        cd.extend_from_slice(&2u16.to_be_bytes()); // rangeCount
        cd.extend_from_slice(&10u16.to_be_bytes()); // start
        cd.extend_from_slice(&10u16.to_be_bytes()); // end
        cd.extend_from_slice(&1u16.to_be_bytes()); // class 1
        cd.extend_from_slice(&20u16.to_be_bytes());
        cd.extend_from_slice(&20u16.to_be_bytes());
        cd.extend_from_slice(&2u16.to_be_bytes()); // class 2
                                                   // Coverage covers the first-position glyph (gid 10).
        let mut cov = Vec::new();
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&10u16.to_be_bytes());
        // ClassSequenceRule: glyphCount=2, seqLookupCount=1,
        // inputSequence=[class 2], record (seq=1, lk=0).
        let mut rule = Vec::new();
        rule.extend_from_slice(&2u16.to_be_bytes());
        rule.extend_from_slice(&1u16.to_be_bytes());
        rule.extend_from_slice(&2u16.to_be_bytes()); // class value at pos 1
        rule.extend_from_slice(&1u16.to_be_bytes()); // seqIndex
        rule.extend_from_slice(&0u16.to_be_bytes()); // lookupIndex
        let mut set = Vec::new();
        set.extend_from_slice(&1u16.to_be_bytes());
        set.extend_from_slice(&4u16.to_be_bytes());
        set.extend_from_slice(&rule);
        // Header: format + covOff + cdOff + setCount + setOff[0..2].
        // setCount=2 (class 0 → NULL, class 1 → real). Header = 8 + 4.
        let set_count = 2u16;
        let header_len = 8 + (set_count as usize) * 2;
        let cov_off = header_len as u16;
        let cd_off = cov_off + cov.len() as u16;
        let set_off = cd_off + cd.len() as u16;
        let mut sub = Vec::new();
        sub.extend_from_slice(&2u16.to_be_bytes()); // format
        sub.extend_from_slice(&cov_off.to_be_bytes());
        sub.extend_from_slice(&cd_off.to_be_bytes());
        sub.extend_from_slice(&set_count.to_be_bytes());
        sub.extend_from_slice(&0u16.to_be_bytes()); // class 0 → NULL
        sub.extend_from_slice(&set_off.to_be_bytes()); // class 1 → set
        sub.extend_from_slice(&cov);
        sub.extend_from_slice(&cd);
        sub.extend_from_slice(&set);
        assemble_context_gpos(&single_sub, &sub)
    }

    #[test]
    fn context_pos_format2_class_based_dispatch() {
        let bytes = build_context_pos_format2();
        let g = GposTable::parse(&bytes).unwrap();
        let recs = g.apply_lookup_type_7(1, &[10, 20], 0).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].glyph_index, 1);
        assert_eq!(recs[0].value.x_advance, 50);
    }

    #[test]
    fn context_pos_format2_no_match_when_class_differs() {
        let bytes = build_context_pos_format2();
        let g = GposTable::parse(&bytes).unwrap();
        // gid 30 maps to class 0, not the rule's class 2 at pos 1.
        // It is also not in coverage at pos 0, so a [10, 30] run misses.
        assert_eq!(g.apply_lookup_type_7(1, &[10, 30], 0), None);
    }

    /// SequenceContextFormat3: per-position coverage. pos 0 covers
    /// {10}, pos 1 covers {20}; single record (seq=1, lk=0).
    fn build_context_pos_format3() -> Vec<u8> {
        let single_sub = single_pos_sub(20, 50);
        let mut cov0 = Vec::new();
        cov0.extend_from_slice(&1u16.to_be_bytes());
        cov0.extend_from_slice(&1u16.to_be_bytes());
        cov0.extend_from_slice(&10u16.to_be_bytes());
        let mut cov1 = Vec::new();
        cov1.extend_from_slice(&1u16.to_be_bytes());
        cov1.extend_from_slice(&1u16.to_be_bytes());
        cov1.extend_from_slice(&20u16.to_be_bytes());
        // Header: format + glyphCount + seqLookupCount + covOff[0..2]
        //         + record = 2+2+2+ (2*2) + 4 = 14.
        let glyph_count = 2u16;
        let header_len = 6 + (glyph_count as usize) * 2 + 4;
        let cov0_off = header_len as u16;
        let cov1_off = cov0_off + cov0.len() as u16;
        let mut sub = Vec::new();
        sub.extend_from_slice(&3u16.to_be_bytes()); // format
        sub.extend_from_slice(&glyph_count.to_be_bytes());
        sub.extend_from_slice(&1u16.to_be_bytes()); // seqLookupCount
        sub.extend_from_slice(&cov0_off.to_be_bytes());
        sub.extend_from_slice(&cov1_off.to_be_bytes());
        sub.extend_from_slice(&1u16.to_be_bytes()); // seqIndex
        sub.extend_from_slice(&0u16.to_be_bytes()); // lookupIndex
        sub.extend_from_slice(&cov0);
        sub.extend_from_slice(&cov1);
        assemble_context_gpos(&single_sub, &sub)
    }

    #[test]
    fn context_pos_format3_coverage_based_dispatch() {
        let bytes = build_context_pos_format3();
        let g = GposTable::parse(&bytes).unwrap();
        let recs = g.apply_lookup_type_7(1, &[10, 20], 0).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].glyph_index, 1);
        assert_eq!(recs[0].value.x_advance, 50);
    }

    #[test]
    fn context_pos_format3_no_match_when_window_short_or_uncovered() {
        let bytes = build_context_pos_format3();
        let g = GposTable::parse(&bytes).unwrap();
        // Second-position glyph not covered.
        assert_eq!(g.apply_lookup_type_7(1, &[10, 21], 0), None);
        // Window too short.
        assert_eq!(g.apply_lookup_type_7(1, &[10], 0), None);
    }
}
