//! The GSUB stage: every planned substitution lookup applied across the
//! glyph buffer, in LookupList order.
//!
//! One routine, [`Subst::apply_at`], applies a lookup at one buffer
//! position. The stage loop calls it at every position the lookup can
//! start at; a contextual lookup calls it for each of its SequenceLookup
//! records, with the nested lookup's own `lookupFlag`.
//!
//! ## Ligature components
//!
//! Mark-to-ligature positioning needs to know which ligature component
//! each mark belongs to, which "is dependent on the original character
//! string and subsequent character- or glyph-sequence processing, not
//! the font data alone" (GPOS chapter, LookupType 5). So every ligature
//! formed here gets an identifier and a component count, and each mark
//! the ligature lookup stepped over — a mark sitting between two
//! components, or trailing the last one — is tagged with that identifier
//! and the index of the component it follows. A ligature of ligatures
//! keeps the finer component numbering of its parts.

use super::seq::{self, View};
use super::{Engine, LookupScratch, RunGlyphs, SkipFilter, WorkGlyph, mask_allows};
use crate::layout::{LookupAccel, PlanLookup, SubAccel};
use crate::tables::gsub;

/// A change of buffer length, recorded while a contextual lookup is
/// applying its nested lookups so the positions of its input sequence
/// can follow.
#[derive(Clone, Copy, Debug)]
pub(super) enum Edit {
    /// The glyph at this index was removed.
    Removed(usize),
    /// `count` glyphs were inserted starting at index `at`, as the tail
    /// of the substitution of the glyph at `at - 1`.
    Inserted { at: usize, count: usize },
}

/// Nested lookup applications one top-level match may trigger: bounds
/// the work a self-referential lookup graph can cause.
const NESTED_BUDGET: u32 = 2048;

pub(super) struct Subst<'s, 'r, 'f, 'a> {
    pub(super) eng: Engine<'f, 'a>,
    pub(super) buf: &'s mut Vec<WorkGlyph>,
    pub(super) run: &'s mut RunGlyphs<'r>,
    pub(super) sc: &'s mut LookupScratch,
    /// Longest the buffer may grow.
    pub(super) max_len: usize,
    /// Inside a contextual lookup: length changes are logged.
    pub(super) logging: bool,
    pub(super) budget: u32,
    /// Identifier of the most recently formed ligature (0: none yet).
    pub(super) last_lig_id: u16,
}

/// Where a sub-table's flattened Coverage puts a glyph.
enum Flat {
    /// Not covered: the sub-table does not apply.
    Miss,
    /// Covered, at this Coverage index.
    At(u16),
    /// No flattened Coverage: search the Coverage table.
    Unknown,
}

#[inline]
fn flat_index(sub: &SubAccel, gid: u16) -> Flat {
    match sub.coverage_index.as_deref().map(|ci| ci.get(gid as usize)) {
        Some(Some(0)) => Flat::Miss,
        Some(Some(&slot)) => Flat::At(slot - 1),
        // Not flattened, or a glyph id beyond the flattened range.
        Some(None) | None => Flat::Unknown,
    }
}

impl<'f, 'a> Subst<'_, '_, 'f, 'a> {
    /// Run the planned lookups.
    pub(super) fn run_all(&mut self, lookups: &[PlanLookup]) {
        for planned in lookups {
            let Some(lookup) = self.eng.lookup(planned.index) else {
                continue;
            };
            if self.run.rules_out(lookup) {
                continue;
            }
            let skip = self.eng.skip_filter(lookup);
            if lookup.kind == 8 {
                self.reverse_pass(lookup, &skip, planned.mask);
                continue;
            }
            let mut i = 0usize;
            while i < self.buf.len() {
                let glyph = self.buf[i];
                if lookup.first.contains(glyph.gid)
                    && mask_allows(planned.mask, glyph.mask)
                    && !skip.skips(glyph.gid)
                {
                    self.budget = NESTED_BUDGET;
                    if let Some(next) =
                        self.apply_at(lookup, &skip, planned.mask, planned.alternate, i, 0)
                    {
                        i = next;
                        continue;
                    }
                }
                i += 1;
            }
        }
    }

    /// Reverse chaining contextual single substitution (LookupType 8):
    /// "processing of input glyph sequence goes from end to start", so
    /// each substitution sees the already-substituted glyphs after it
    /// and the original glyphs before it.
    fn reverse_pass(&mut self, lookup: &'f LookupAccel, skip: &SkipFilter<'f, 'a>, mask: u32) {
        let eng = self.eng;
        for i in (0..self.buf.len()).rev() {
            let glyph = self.buf[i];
            if !lookup.first.contains(glyph.gid)
                || !mask_allows(mask, glyph.mask)
                || skip.skips(glyph.gid)
            {
                continue;
            }
            let view = View {
                buf: self.buf,
                skip,
                mask,
            };
            let positions = &mut self.sc.positions;
            let hit = lookup
                .subs
                .iter()
                .filter(|s| s.first.contains(glyph.gid))
                .find_map(|s| seq::match_reverse_chain(eng.sub_bytes(s), &view, i, positions));
            if let Some(substitute) = hit {
                self.set_gid(i, substitute);
            }
        }
    }

    /// Apply `lookup` with the glyph at `i` as its current glyph: the
    /// first sub-table that matches acts. Returns the index processing
    /// resumes at — just past the glyphs the lookup consumed — or `None`
    /// when no sub-table matched.
    fn apply_at(
        &mut self,
        lookup: &'f LookupAccel,
        skip: &SkipFilter<'f, 'a>,
        mask: u32,
        alternate: u16,
        i: usize,
        depth: u8,
    ) -> Option<usize> {
        let eng = self.eng;
        let gid = self.buf[i].gid;
        let mut subs = lookup.subs.iter().filter(|s| s.first.contains(gid));
        match lookup.kind {
            1 => {
                let substitute = subs.find_map(|s| {
                    let bytes = eng.sub_bytes(s);
                    match flat_index(s, gid) {
                        Flat::Miss => None,
                        Flat::At(index) => gsub::single_subst_covered(bytes, gid, index),
                        Flat::Unknown => gsub::single_subst_lookup(bytes, gid),
                    }
                })?;
                self.set_gid(i, substitute);
                Some(i + 1)
            }
            2 => {
                let sequence =
                    subs.find_map(|s| gsub::multiple_subst_lookup(eng.sub_bytes(s), gid))?;
                self.expand(i, &sequence).then_some(i + sequence.len())
            }
            3 => {
                let substitute = subs.find_map(|s| {
                    let bytes = eng.sub_bytes(s);
                    match flat_index(s, gid) {
                        Flat::Miss => None,
                        Flat::At(index) => gsub::alternate_subst_covered(bytes, index, alternate),
                        Flat::Unknown => gsub::alternate_subst_lookup(bytes, gid, alternate),
                    }
                })?;
                self.set_gid(i, substitute);
                Some(i + 1)
            }
            4 => self.ligate(lookup, skip, mask, i),
            5 | 6 => self.contextual(lookup, skip, mask, i, depth),
            _ => None,
        }
    }

    #[inline]
    fn set_gid(&mut self, i: usize, gid: u16) {
        self.buf[i].gid = gid;
        self.run.add(gid);
    }

    /// Replace the glyph at `i` by `sequence` (multiple substitution).
    /// Every output inherits the source glyph's cluster, mask and
    /// ligature association. An empty sequence removes the glyph.
    /// Returns `false`, changing nothing, when the buffer may not grow
    /// further.
    fn expand(&mut self, i: usize, sequence: &[u16]) -> bool {
        let Some((&head, tail)) = sequence.split_first() else {
            self.buf.remove(i);
            if self.logging {
                self.sc.edits.push(Edit::Removed(i));
            }
            return true;
        };
        if self.buf.len() + tail.len() > self.max_len {
            return false;
        }
        let source = self.buf[i];
        self.set_gid(i, head);
        if !tail.is_empty() {
            self.buf.splice(
                i + 1..i + 1,
                tail.iter().map(|&gid| WorkGlyph { gid, ..source }),
            );
            for &gid in tail {
                self.run.add(gid);
            }
            if self.logging {
                self.sc.edits.push(Edit::Inserted {
                    at: i + 1,
                    count: tail.len(),
                });
            }
        }
        true
    }

    /// Ligature substitution (LookupType 4) with its first component at
    /// `i`. The components are matched over the glyphs the lookup flag
    /// does not ignore; "the first glyph is substituted with the
    /// ligature. The remaining glyphs in the string are deleted, this
    /// does not include those glyphs that are skipped as a result of
    /// lookup flags."
    fn ligate(
        &mut self,
        lookup: &'f LookupAccel,
        skip: &SkipFilter<'f, 'a>,
        mask: u32,
        i: usize,
    ) -> Option<usize> {
        // The candidate vectors leave the scratch while the buffer is
        // edited, and go back for the next ligature.
        let mut cand_gids = std::mem::take(&mut self.sc.cand_gids);
        let mut cand_idx = std::mem::take(&mut self.sc.cand_idx);
        let next = self.ligate_with(lookup, skip, mask, i, &mut cand_gids, &mut cand_idx);
        self.sc.cand_gids = cand_gids;
        self.sc.cand_idx = cand_idx;
        next
    }

    fn ligate_with(
        &mut self,
        lookup: &'f LookupAccel,
        skip: &SkipFilter<'f, 'a>,
        mask: u32,
        i: usize,
        cand_gids: &mut Vec<u16>,
        cand_idx: &mut Vec<usize>,
    ) -> Option<usize> {
        let eng = self.eng;
        let first = self.buf[i].gid;
        // Every ligature of this lookup has a second component: nothing
        // can form unless the next visible glyph is one of them.
        if let Some(second) = lookup.second_components.as_ref()
            && !skip
                .next(self.buf, i)
                .is_some_and(|at| second.contains(self.buf[at].gid))
        {
            return None;
        }
        // The visible glyphs from `i` on, as far as any Ligature record
        // of the lookup can look, with their buffer indices.
        let window = lookup.max_components.max(1);
        cand_gids.clear();
        cand_idx.clear();
        cand_gids.push(first);
        cand_idx.push(i);
        let mut at = i;
        while cand_gids.len() < window {
            match skip.next(self.buf, at) {
                Some(next) => {
                    cand_gids.push(self.buf[next].gid);
                    cand_idx.push(next);
                    at = next;
                }
                None => break,
            }
        }
        let (ligature, consumed) = lookup
            .subs
            .iter()
            .filter(|s| s.first.contains(first))
            .find_map(|s| {
                let bytes = eng.sub_bytes(s);
                match flat_index(s, first) {
                    Flat::Miss => None,
                    Flat::At(index) => gsub::ligature_subst_covered(bytes, index, cand_gids),
                    Flat::Unknown => gsub::ligature_subst_lookup(bytes, cand_gids),
                }
            })?;
        if consumed == 0 || consumed > cand_idx.len() {
            return None;
        }
        let components = &cand_idx[..consumed];
        if mask != 0
            && !components
                .iter()
                .all(|&c| mask_allows(mask, self.buf[c].mask))
        {
            return None;
        }

        if consumed > 1 && !eng.is_mark(ligature) {
            // A new ligature: number its components and hand each mark
            // the lookup stepped over to the component it follows.
            self.last_lig_id = match self.last_lig_id.wrapping_add(1) {
                0 => 1,
                id => id,
            };
            let id = self.last_lig_id;
            let mut total = 0u32;
            for (k, &c) in components.iter().enumerate() {
                let part = self.buf[c];
                let part_components = u32::from(part.lig_comps.max(1));
                // Marks after this component, up to the next component
                // (after the last one: the marks that directly follow).
                let end = components.get(k + 1).copied().unwrap_or(self.buf.len());
                let is_last = k + 1 == components.len();
                for mark in &mut self.buf[c + 1..end] {
                    if !eng.is_mark(mark.gid) {
                        if is_last {
                            break;
                        }
                        continue;
                    }
                    let within = if part.lig_comps > 0 && mark.lig_id == part.lig_id {
                        // Already tied to a component of this part.
                        u32::from(mark.lig_comp).min(part_components - 1)
                    } else {
                        part_components - 1
                    };
                    mark.lig_id = id;
                    mark.lig_comp = (total + within).min(255) as u8;
                }
                total += part_components;
            }
            let head = &mut self.buf[i];
            head.lig_id = id;
            head.lig_comp = 0;
            head.lig_comps = total.min(255) as u8;
        }
        self.set_gid(i, ligature);
        // Remove the other components, last first so the indices of the
        // ones still to go stay valid.
        for &c in components[1..].iter().rev() {
            self.buf.remove(c);
            if self.logging {
                self.sc.edits.push(Edit::Removed(c));
            }
        }
        Some(i + 1)
    }

    /// Contextual (LookupType 5) / chained contexts (LookupType 6)
    /// substitution with the input sequence starting at `i`.
    ///
    /// "All of the sequence lookup records are processed in order, and
    /// each applies to the results of the actions indicated by the
    /// preceding record": `sequenceIndex` counts glyphs of the input
    /// sequence *as the earlier records left it* — a ligature formed by
    /// one record makes the sequence shorter for the next, a multiple
    /// substitution makes it longer.
    fn contextual(
        &mut self,
        lookup: &'f LookupAccel,
        skip: &SkipFilter<'f, 'a>,
        mask: u32,
        i: usize,
        depth: u8,
    ) -> Option<usize> {
        if depth >= super::MAX_NESTING {
            return None;
        }
        let eng = self.eng;
        let gid = self.buf[i].gid;
        let chained = lookup.kind == 6;
        let base = self.sc.positions.len();
        let records = {
            let view = View {
                buf: self.buf,
                skip,
                mask,
            };
            let positions = &mut self.sc.positions;
            lookup
                .subs
                .iter()
                .filter(|s| s.first.contains(gid))
                .find_map(|s| seq::match_context(eng.sub_bytes(s), chained, &view, i, positions))?
        };
        let was_logging = std::mem::replace(&mut self.logging, true);
        let log_base = self.sc.edits.len();
        for &[s0, s1, l0, l1] in records.as_chunks::<4>().0 {
            let sequence_index = u16::from_be_bytes([s0, s1]) as usize;
            let lookup_index = u16::from_be_bytes([l0, l1]);
            let Some(&at) = self.sc.positions.get(base + sequence_index) else {
                continue;
            };
            let Some(nested) = eng.lookup(lookup_index) else {
                continue;
            };
            if self.budget == 0 {
                break;
            }
            self.budget -= 1;
            // "Lookup flags in the main lookup table are not considered
            // while the nested lookups are being processed. Instead,
            // lookup flags in the nested lookups are considered."
            let nested_skip = eng.skip_filter(nested);
            let seen = self.sc.edits.len();
            let _ = self.apply_at(nested, &nested_skip, 0, 0, at, depth + 1);
            // Deeper levels have returned: this level's positions are
            // the tail of the stack again.
            for e in seen..self.sc.edits.len() {
                let edit = self.sc.edits[e];
                follow_edit(&mut self.sc.positions, base, edit);
            }
        }
        self.logging = was_logging;
        if !was_logging {
            self.sc.edits.truncate(log_base);
        }
        // Resume "at the glyph position immediately following the
        // matched input sequence".
        let next = self.sc.positions[base..].last().map_or(i, |&last| last + 1);
        self.sc.positions.truncate(base);
        Some(next)
    }
}

/// Bring the input-sequence positions `positions[base..]` (ascending
/// buffer indices) in line with one buffer edit.
fn follow_edit(positions: &mut Vec<usize>, base: usize, edit: Edit) {
    match edit {
        Edit::Removed(index) => {
            let mut k = base;
            while k < positions.len() {
                match positions[k].cmp(&index) {
                    std::cmp::Ordering::Less => k += 1,
                    std::cmp::Ordering::Equal => {
                        positions.remove(k);
                    }
                    std::cmp::Ordering::Greater => {
                        positions[k] -= 1;
                        k += 1;
                    }
                }
            }
        }
        Edit::Inserted { at, count } => {
            let mut source = None;
            for (k, p) in positions.iter_mut().enumerate().skip(base) {
                if *p + 1 == at {
                    source = Some(k);
                } else if *p >= at {
                    *p += count;
                }
            }
            // The outputs of a multiple substitution of an input-sequence
            // glyph are part of the resulting sequence.
            if let Some(k) = source {
                positions.splice(k + 1..k + 1, at..at + count);
            }
        }
    }
}
