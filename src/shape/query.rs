//! Per-lookup queries: what one GSUB / GPOS lookup says about a glyph,
//! a glyph pair or a mark attachment, without running a shaping pass.
//!
//! These are the readers [`Font::shape_glyphs`] itself uses, exposed for
//! a text-layout engine that drives lookups on its own — caret
//! placement inside a ligature, a custom mark-stacking policy, kerning
//! across a run boundary. Every value is in font design units at the
//! font's current variation instance. Extension lookups are followed
//! transparently.

use super::wire::{self, Adjust};
use super::{Engine, LayoutTable};
use crate::Font;
use crate::layout::{LookupAccel, first_glyph_coverage};
use crate::tables::gdef::coverage_lookup;
use crate::tables::gpos::{CursiveAttachment, PosValue};

/// What a pair-adjustment lookup (GPOS LookupType 2) does to one glyph
/// pair. Returned by [`Font::gpos_pair_adjustment`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PairAdjustment {
    /// Adjustment of the first glyph of the pair.
    pub first: PosValue,
    /// Adjustment of the second glyph.
    pub second: PosValue,
    /// `true` when the sub-table carries a ValueRecord for the second
    /// glyph (`valueFormat2 != 0`). The second glyph is then consumed
    /// by the pair; otherwise it is also the first glyph of the next
    /// pair.
    pub second_has_record: bool,
}

impl PairAdjustment {
    /// A pair adjustment that moves nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder: set the first glyph's adjustment.
    #[must_use]
    pub fn with_first(mut self, value: PosValue) -> Self {
        self.first = value;
        self
    }

    /// Builder: set the second glyph's adjustment (and mark the second
    /// glyph as carrying a ValueRecord).
    #[must_use]
    pub fn with_second(mut self, value: PosValue) -> Self {
        self.second = value;
        self.second_has_record = true;
        self
    }
}

/// The two anchors of one mark attachment (GPOS LookupTypes 4, 5, 6),
/// each in the coordinate space of its own glyph. Returned by
/// [`Font::gpos_mark_base_anchors`] and its siblings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct MarkAnchors {
    /// Mark class of the attaching mark in this sub-table.
    pub mark_class: u16,
    /// `(x, y)` of the anchor on the attaching mark.
    pub mark_anchor: (i32, i32),
    /// `(x, y)` of the anchor on the glyph attached to: the base glyph,
    /// the ligature component, or the preceding mark.
    pub attach_anchor: (i32, i32),
}

impl MarkAnchors {
    /// Anchors of a mark of class `mark_class`.
    pub fn new(mark_class: u16, mark_anchor: (i32, i32), attach_anchor: (i32, i32)) -> Self {
        MarkAnchors {
            mark_class,
            mark_anchor,
            attach_anchor,
        }
    }

    /// Where the mark's origin goes relative to the origin of the glyph
    /// it attaches to, so that the two anchors coincide.
    pub fn offset(&self) -> (i32, i32) {
        (
            self.attach_anchor.0 - self.mark_anchor.0,
            self.attach_anchor.1 - self.mark_anchor.1,
        )
    }
}

fn clamp(value: i32) -> i16 {
    value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

fn pos_value(adjust: &Adjust) -> PosValue {
    PosValue {
        x_placement: clamp(adjust.x_placement),
        y_placement: clamp(adjust.y_placement),
        x_advance: clamp(adjust.x_advance),
        y_advance: clamp(adjust.y_advance),
    }
}

fn anchors(hit: wire::MarkHit) -> MarkAnchors {
    MarkAnchors::new(hit.class, hit.mark, hit.target)
}

impl<'a> Font<'a> {
    fn gsub_engine(&self) -> Option<Engine<'_, 'a>> {
        let gsub = self.gsub()?;
        Some(Engine::new(
            self,
            LayoutTable::Gsub,
            gsub.lookup_list_location(),
        ))
    }

    fn gpos_engine(&self) -> Option<Engine<'_, 'a>> {
        let gpos = self.gpos()?;
        Some(Engine::new(
            self,
            LayoutTable::Gpos,
            gpos.lookup_list_location(),
        ))
    }

    /// GPOS lookup `lookup_index` when its effective type is `kind`.
    fn gpos_lookup_of(
        &self,
        lookup_index: u16,
        kind: u16,
    ) -> Option<(Engine<'_, 'a>, &LookupAccel)> {
        let eng = self.gpos_engine()?;
        let lookup = eng.lookup(lookup_index)?;
        (lookup.kind == kind).then_some((eng, lookup))
    }

    /// Every glyph GSUB lookup `lookup_index` can act on as its current
    /// glyph — the union of the Coverage tables its sub-tables test the
    /// current glyph against (for a contextual lookup: the first glyph
    /// of the input sequence) — in ascending glyph-id order. Empty for a
    /// lookup that does not exist.
    pub fn gsub_lookup_coverage(&self, lookup_index: u16) -> Vec<u16> {
        self.gsub_engine()
            .map(|eng| lookup_coverage(eng, lookup_index))
            .unwrap_or_default()
    }

    /// `true` when `glyph_id` is in [`Font::gsub_lookup_coverage`] of
    /// lookup `lookup_index`.
    pub fn gsub_lookup_covers(&self, lookup_index: u16, glyph_id: u16) -> bool {
        self.gsub_engine()
            .is_some_and(|eng| lookup_covers(eng, lookup_index, glyph_id))
    }

    /// Every glyph GPOS lookup `lookup_index` can act on as its current
    /// glyph, in ascending glyph-id order: the adjusted glyph of a
    /// single adjustment, the first glyph of a pair, a glyph with
    /// cursive anchors, the attaching *mark* of a mark attachment, the
    /// first input glyph of a contextual lookup.
    pub fn gpos_lookup_coverage(&self, lookup_index: u16) -> Vec<u16> {
        self.gpos_engine()
            .map(|eng| lookup_coverage(eng, lookup_index))
            .unwrap_or_default()
    }

    /// `true` when `glyph_id` is in [`Font::gpos_lookup_coverage`] of
    /// lookup `lookup_index`.
    pub fn gpos_lookup_covers(&self, lookup_index: u16, glyph_id: u16) -> bool {
        self.gpos_engine()
            .is_some_and(|eng| lookup_covers(eng, lookup_index, glyph_id))
    }

    /// The adjustment pair-adjustment lookup `lookup_index` (GPOS
    /// LookupType 2, either format) makes to the glyph pair
    /// `(first, second)`: every ValueRecord field of both glyphs, with
    /// variation deltas applied. `None` when the lookup is not a pair
    /// adjustment or has no entry for the pair.
    pub fn gpos_pair_adjustment(
        &self,
        lookup_index: u16,
        first: u16,
        second: u16,
    ) -> Option<PairAdjustment> {
        let (eng, lookup) = self.gpos_lookup_of(lookup_index, 2)?;
        let var = self.gpos_variation();
        let hit = lookup
            .subs
            .iter()
            .filter(|s| s.first.contains(first))
            .find_map(|s| wire::pair_adjust(eng.sub_bytes(s), Some(s), first, second, &var))?;
        Some(PairAdjustment {
            first: pos_value(&hit.first),
            second: pos_value(&hit.second),
            second_has_record: hit.second_has_record,
        })
    }

    /// The anchors attaching `mark` to `base` in mark-to-base lookup
    /// `lookup_index` (GPOS LookupType 4). `None` when the lookup is not
    /// a mark-to-base attachment, a glyph is not covered, or the base
    /// defines no anchor for the mark's class.
    pub fn gpos_mark_base_anchors(
        &self,
        lookup_index: u16,
        base: u16,
        mark: u16,
    ) -> Option<MarkAnchors> {
        self.mark_single_anchors(lookup_index, 4, base, mark)
    }

    /// The anchors attaching `mark` to the mark `under` it in
    /// mark-to-mark lookup `lookup_index` (GPOS LookupType 6).
    pub fn gpos_mark_mark_anchors(
        &self,
        lookup_index: u16,
        under: u16,
        mark: u16,
    ) -> Option<MarkAnchors> {
        self.mark_single_anchors(lookup_index, 6, under, mark)
    }

    fn mark_single_anchors(
        &self,
        lookup_index: u16,
        kind: u16,
        target: u16,
        mark: u16,
    ) -> Option<MarkAnchors> {
        let (eng, lookup) = self.gpos_lookup_of(lookup_index, kind)?;
        let var = self.gpos_variation();
        lookup
            .subs
            .iter()
            .filter(|s| s.first.contains(mark))
            .find_map(|s| wire::mark_to_single(eng.sub_bytes(s), mark, target, &var))
            .map(anchors)
    }

    /// The anchors attaching `mark` to component `component`
    /// (zero-based, in writing order) of `ligature` in mark-to-ligature
    /// lookup `lookup_index` (GPOS LookupType 5). `None` when the
    /// component does not exist or defines no anchor for the mark's
    /// class.
    pub fn gpos_mark_ligature_anchors(
        &self,
        lookup_index: u16,
        ligature: u16,
        component: u16,
        mark: u16,
    ) -> Option<MarkAnchors> {
        let (eng, lookup) = self.gpos_lookup_of(lookup_index, 5)?;
        let var = self.gpos_variation();
        lookup
            .subs
            .iter()
            .filter(|s| s.first.contains(mark))
            .find_map(|s| {
                let bytes = eng.sub_bytes(s);
                if component >= wire::ligature_component_count(bytes, ligature)? {
                    return None;
                }
                wire::mark_to_ligature(bytes, mark, ligature, |_| component, &var)
            })
            .map(anchors)
    }

    /// How many components mark-to-ligature lookup `lookup_index`
    /// defines for `ligature`.
    pub fn gpos_ligature_component_count(&self, lookup_index: u16, ligature: u16) -> Option<u16> {
        let (eng, lookup) = self.gpos_lookup_of(lookup_index, 5)?;
        lookup
            .subs
            .iter()
            .find_map(|s| wire::ligature_component_count(eng.sub_bytes(s), ligature))
    }

    /// The entry / exit anchors cursive-attachment lookup `lookup_index`
    /// (GPOS LookupType 3) gives `glyph_id`, at the current variation
    /// instance.
    pub fn gpos_cursive_anchors(
        &self,
        lookup_index: u16,
        glyph_id: u16,
    ) -> Option<CursiveAttachment> {
        let (eng, lookup) = self.gpos_lookup_of(lookup_index, 3)?;
        let var = self.gpos_variation();
        let (entry, exit) = lookup
            .subs
            .iter()
            .filter(|s| s.first.contains(glyph_id))
            .find_map(|s| wire::cursive_anchors(eng.sub_bytes(s), glyph_id, &var))?;
        let narrow = |point: (i32, i32)| (clamp(point.0), clamp(point.1));
        Some(CursiveAttachment {
            entry: entry.map(narrow),
            exit: exit.map(narrow),
        })
    }
}

fn lookup_coverage(eng: Engine<'_, '_>, lookup_index: u16) -> Vec<u16> {
    let Some(lookup) = eng.lookup(lookup_index) else {
        return Vec::new();
    };
    // One bit per glyph id.
    let mut listed = vec![0u64; 1 << 10];
    for sub in lookup.subs.iter() {
        if let Some(coverage) = first_glyph_coverage(eng.which, lookup.kind, eng.sub_bytes(sub)) {
            mark_coverage_glyphs(coverage, &mut listed);
        }
    }
    (0..=u16::MAX)
        .filter(|&g| listed[(g >> 6) as usize] >> (g & 63) & 1 != 0)
        // Keep what the lookup can really reach: a listed glyph must be
        // one the Coverage search finds.
        .filter(|&g| lookup_covers(eng, lookup_index, g))
        .collect()
}

fn lookup_covers(eng: Engine<'_, '_>, lookup_index: u16, glyph_id: u16) -> bool {
    let Some(lookup) = eng.lookup(lookup_index) else {
        return false;
    };
    lookup.subs.iter().any(|sub| {
        first_glyph_coverage(eng.which, lookup.kind, eng.sub_bytes(sub))
            .is_some_and(|coverage| coverage_lookup(coverage, glyph_id).is_some())
    })
}

/// Set the bit of every glyph id a Coverage table lists.
fn mark_coverage_glyphs(coverage: &[u8], listed: &mut [u64]) {
    let (Some(format), Some(count)) = (wire::rd16(coverage, 0), wire::rd16(coverage, 2)) else {
        return;
    };
    let mut mark = |glyph: u16| listed[(glyph >> 6) as usize] |= 1u64 << (glyph & 63);
    for i in 0..count as usize {
        match format {
            1 => match wire::rd16(coverage, 4 + i * 2) {
                Some(glyph) => mark(glyph),
                None => return,
            },
            2 => {
                let at = 4 + i * 6;
                match (wire::rd16(coverage, at), wire::rd16(coverage, at + 2)) {
                    (Some(start), Some(end)) => (start..=end).for_each(&mut mark),
                    _ => return,
                }
            }
            _ => return,
        }
    }
}
