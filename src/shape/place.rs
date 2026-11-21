//! The GPOS stage: advances seeded from the metrics tables, then every
//! planned positioning lookup applied in LookupList order.
//!
//! ## Deferred attachment
//!
//! A mark attachment (LookupTypes 4 – 6) says "this mark's anchor
//! coincides with that anchor of the glyph it attaches to". The offset
//! that achieves it depends on where the attached-to glyph finally sits
//! and on every advance between the two — all of which later lookups
//! may still change. So an attachment is *recorded* (target glyph plus
//! anchor difference) and turned into an offset once every lookup has
//! run. Resolving in buffer order makes chains accumulate: a mark
//! attached to a mark attached to a base is resolved against the first
//! mark's final position. The cross-stream part of a cursive attachment
//! (LookupType 3) is deferred the same way, because it too chains from
//! glyph to glyph.

use super::seq::{self, View};
use super::wire::{self, Adjust, VarCtx};
use super::{Direction, Engine, RunGlyphs, ShapedGlyph, SkipFilter, WorkGlyph, mask_allows};
use crate::layout::{LookupAccel, PlanLookup, digest_bit};

/// "No glyph" in an [`Attach`] link.
const UNLINKED: u32 = u32::MAX;

/// Pending attachments of one glyph.
#[derive(Clone, Copy, Debug)]
pub(super) struct Attach {
    /// Buffer index of the glyph this mark is attached to.
    mark_to: u32,
    /// Attached-to anchor minus mark anchor.
    mark_dx: i32,
    mark_dy: i32,
    /// Buffer index of the glyph this one follows in the cross-stream
    /// direction of a cursive connection.
    cursive_to: u32,
    /// Cross-stream distance from that glyph.
    cursive_shift: i32,
}

impl Default for Attach {
    fn default() -> Self {
        Attach {
            mark_to: UNLINKED,
            mark_dx: 0,
            mark_dy: 0,
            cursive_to: UNLINKED,
            cursive_shift: 0,
        }
    }
}

pub(super) struct Place<'s, 'f, 'a> {
    pub(super) eng: Engine<'f, 'a>,
    pub(super) buf: &'s [WorkGlyph],
    pub(super) out: &'s mut [ShapedGlyph],
    /// Empty until the first attachment, then one entry per glyph.
    pub(super) attach: &'s mut Vec<Attach>,
    pub(super) positions: &'s mut Vec<usize>,
    pub(super) var: VarCtx<'f>,
    pub(super) direction: Direction,
    /// Marks keep a zero advance: advance adjustments skip them.
    pub(super) zero_marks: bool,
    pub(super) budget: u32,
}

const NESTED_BUDGET: u32 = 2048;

/// Position arithmetic saturates: a hostile font can pile adjustments
/// up past 32 bits, and that must not abort the run.
#[inline]
fn plus(a: i32, b: i32) -> i32 {
    a.saturating_add(b)
}

#[inline]
fn minus(a: i32, b: i32) -> i32 {
    a.saturating_sub(b)
}

impl<'f, 'a> Place<'_, 'f, 'a> {
    /// Run the planned lookups.
    pub(super) fn run_all(&mut self, lookups: &[PlanLookup], run: &RunGlyphs<'_>) {
        for planned in lookups {
            if run.misses(planned) {
                continue;
            }
            let Some(lookup) = self.eng.lookup(planned.index) else {
                continue;
            };
            let candidates = run.candidates(lookup);
            if candidates == 0 {
                continue;
            }
            let skip = self.eng.skip_filter(lookup);
            if lookup.kind == 2
                && lookup.plain_kerning
                && planned.mask == 0
                && self.direction != Direction::TopToBottom
            {
                self.plain_kerning_pass(lookup, &skip, candidates);
                continue;
            }
            let mut i = 0usize;
            while i < self.buf.len() {
                let glyph = self.buf[i];
                // The residue mask first: no memory touched for most
                // glyphs the lookup cannot act on.
                if candidates & digest_bit(glyph.gid) != 0
                    && lookup.first.contains(glyph.gid)
                    && mask_allows(planned.mask, glyph.mask)
                    && !skip.skips(glyph.gid)
                {
                    self.budget = NESTED_BUDGET;
                    if let Some(next) = self.apply_at(lookup, &skip, planned.mask, i, 0) {
                        // Every lookup type moves forward, except a pair
                        // whose second glyph has no ValueRecord: that
                        // glyph is the next first glyph.
                        i = next.max(i + 1);
                        continue;
                    }
                }
                i += 1;
            }
        }
    }

    /// A pair-adjustment lookup that only ever changes the first glyph's
    /// x-advance: one memoised number per glyph pair (at the current
    /// variation instance).
    fn plain_kerning_pass(
        &mut self,
        lookup: &'f LookupAccel,
        skip: &SkipFilter<'f, 'a>,
        candidates: u64,
    ) {
        let eng = self.eng;
        let var = self.var;
        for i in 0..self.buf.len() {
            let left = self.buf[i].gid;
            if candidates & digest_bit(left) == 0
                || !lookup.first.contains(left)
                || skip.skips(left)
            {
                continue;
            }
            let Some(j) = skip.next(self.buf, i) else {
                break;
            };
            let right = self.buf[j].gid;
            let adjust = || {
                lookup
                    .subs
                    .iter()
                    .filter(|s| s.first.contains(left))
                    .find_map(|s| wire::pair_adjust(eng.sub_bytes(s), Some(s), left, right, &var))
                    .map_or(0, |hit| hit.first.x_advance)
            };
            let key = (left as u32) << 16 | right as u32;
            // An adjustment outside 16 bits is not memoised.
            let memoised = lookup
                .pair_memo
                .get_or_compute(key, || i16::try_from(adjust()).ok().map(|v| v as u16));
            let kerning = match memoised {
                Some(value) => value as i16 as i32,
                None => adjust(),
            };
            if !(self.zero_marks && eng.is_mark(left)) {
                self.out[i].x_advance = plus(self.out[i].x_advance, kerning);
            }
        }
    }

    /// Add a ValueRecord to a glyph. `xAdvance` is "only used for
    /// horizontal layout", `yAdvance` "only used for vertical layout",
    /// where it lengthens the (downward) advance.
    #[inline]
    fn add(&mut self, i: usize, adjust: &Adjust) {
        let keep_advance = self.zero_marks
            && (adjust.x_advance != 0 || adjust.y_advance != 0)
            && self.eng.is_mark(self.buf[i].gid);
        let glyph = &mut self.out[i];
        glyph.x_offset = plus(glyph.x_offset, adjust.x_placement);
        glyph.y_offset = plus(glyph.y_offset, adjust.y_placement);
        if keep_advance {
            // A zeroed mark advance stays zero.
        } else if self.direction == Direction::TopToBottom {
            glyph.y_advance = minus(glyph.y_advance, adjust.y_advance);
        } else {
            glyph.x_advance = plus(glyph.x_advance, adjust.x_advance);
        }
    }

    fn attach_mut(&mut self, i: usize) -> &mut Attach {
        if self.attach.len() != self.out.len() {
            self.attach.clear();
            self.attach.resize(self.out.len(), Attach::default());
        }
        &mut self.attach[i]
    }

    /// Apply `lookup` with the glyph at `i` as its current glyph: the
    /// first sub-table that matches acts. Returns the index processing
    /// resumes at, or `None` when no sub-table matched.
    fn apply_at(
        &mut self,
        lookup: &'f LookupAccel,
        skip: &SkipFilter<'f, 'a>,
        mask: u32,
        i: usize,
        depth: u8,
    ) -> Option<usize> {
        let eng = self.eng;
        let var = self.var;
        let gid = self.buf[i].gid;
        let mut subs = lookup.subs.iter().filter(|s| s.first.contains(gid));
        match lookup.kind {
            // Single adjustment.
            1 => {
                let adjust = subs.find_map(|s| wire::single_adjust(eng.sub_bytes(s), gid, &var))?;
                self.add(i, &adjust);
                Some(i + 1)
            }
            // Pair adjustment: the current glyph and the next glyph the
            // lookup does not ignore.
            2 => {
                let j = skip.next(self.buf, i)?;
                if !mask_allows(mask, self.buf[j].mask) {
                    return None;
                }
                let second = self.buf[j].gid;
                let hit = subs.find_map(|s| {
                    wire::pair_adjust(eng.sub_bytes(s), Some(s), gid, second, &var)
                })?;
                self.add(i, &hit.first);
                self.add(j, &hit.second);
                // "If valueFormat2 is set to 0, then the second glyph of
                // the pair is the 'next' glyph for which a lookup should
                // be performed."
                Some(if hit.second_has_record { j + 1 } else { j })
            }
            // Cursive attachment: the current glyph's exit anchor meets
            // the entry anchor of the next glyph.
            3 => {
                let j = skip.next(self.buf, i)?;
                if !mask_allows(mask, self.buf[j].mask) {
                    return None;
                }
                let second = self.buf[j].gid;
                let (exit, entry) = subs.find_map(|s| {
                    let bytes = eng.sub_bytes(s);
                    let (_, exit) = wire::cursive_anchors(bytes, gid, &var)?;
                    let (entry, _) = wire::cursive_anchors(bytes, second, &var)?;
                    Some((exit?, entry?))
                })?;
                self.connect(i, j, exit, entry, lookup.flags & 0x0001 != 0);
                Some(j)
            }
            // Mark-to-base: "look backward in the glyph string from the
            // mark to the preceding base glyph".
            4 => {
                let j = self.preceding_non_mark(skip, i)?;
                let base = self.buf[j].gid;
                let hit = subs.find_map(|s| {
                    wire::mark_to_single(eng.sub_bytes(s), Some(s), gid, base, &var)
                })?;
                self.attach_mark(i, j, hit);
                Some(i + 1)
            }
            // Mark-to-ligature: as mark-to-base, on the component the
            // mark belongs to.
            5 => {
                let j = self.preceding_non_mark(skip, i)?;
                let (mark, ligature) = (self.buf[i], self.buf[j]);
                let tied = mark.lig_id != 0 && mark.lig_id == ligature.lig_id;
                let hit = subs.find_map(|s| {
                    wire::mark_to_ligature(
                        eng.sub_bytes(s),
                        Some(s),
                        gid,
                        ligature.gid,
                        // A mark with no recorded component follows the
                        // whole ligature: it sits on the last component.
                        |count| match tied {
                            true => u16::from(mark.lig_comp),
                            false => count - 1,
                        },
                        &var,
                    )
                })?;
                self.attach_mark(i, j, hit);
                Some(i + 1)
            }
            // Mark-to-mark: "the glyph preceding the mark1 glyph in
            // glyph string order (skipping glyphs according to
            // LookupFlags)".
            6 => {
                let j = skip.prev(self.buf, i)?;
                let (mark, under) = (self.buf[i], self.buf[j]);
                // Marks on different components of one ligature do not
                // stack on each other.
                if mark.lig_id != 0
                    && mark.lig_id == under.lig_id
                    && under.lig_comps == 0
                    && mark.lig_comp != under.lig_comp
                {
                    return None;
                }
                let hit = subs.find_map(|s| {
                    wire::mark_to_single(eng.sub_bytes(s), Some(s), gid, under.gid, &var)
                })?;
                self.attach_mark(i, j, hit);
                Some(i + 1)
            }
            7 | 8 => self.contextual(lookup, skip, mask, i, depth),
            _ => None,
        }
    }

    /// The nearest glyph before `i` that the lookup does not ignore and
    /// that is not a mark.
    fn preceding_non_mark(&self, skip: &SkipFilter<'f, 'a>, i: usize) -> Option<usize> {
        let mut at = i;
        loop {
            at = skip.prev(self.buf, at)?;
            if !self.eng.is_mark(self.buf[at].gid) {
                return Some(at);
            }
        }
    }

    /// Record that the mark at `i` attaches to the glyph at `j`. The
    /// attachment fixes the mark's position: placement it had so far is
    /// dropped, placement added by later lookups is kept.
    fn attach_mark(&mut self, i: usize, j: usize, hit: wire::MarkHit) {
        let link = self.attach_mut(i);
        link.mark_to = j as u32;
        link.mark_dx = hit.offset.0;
        link.mark_dy = hit.offset.1;
        self.out[i].x_offset = 0;
        self.out[i].y_offset = 0;
    }

    /// Connect glyph `i` (through its `exit` anchor) to the following
    /// glyph `j` (through its `entry` anchor).
    ///
    /// Along the line the pen is made to pass through the anchors: the
    /// glyph drawn first advances exactly to its connecting anchor, and
    /// the glyph drawn second is placed with its connecting anchor on
    /// the pen, its advance shortened by the same amount so the glyphs
    /// after it stay put. Across the line "placement of one glyph is
    /// adjusted to make the anchors align": the second glyph follows the
    /// first, or — with the RIGHT_TO_LEFT lookup flag — the first
    /// follows the second, so that "the last glyph in the connected
    /// sequence keeps its initial position".
    fn connect(
        &mut self,
        i: usize,
        j: usize,
        exit: (i32, i32),
        entry: (i32, i32),
        last_glyph_stays: bool,
    ) {
        // Glyphs the lookup stepped over between the two still move the
        // pen: their advances come off the connecting advance.
        let (mut between_x, mut between_y) = (0i32, 0i32);
        for glyph in &self.out[i + 1..j] {
            between_x = plus(between_x, glyph.x_advance);
            between_y = plus(between_y, glyph.y_advance);
        }
        // (cross-stream coordinate of exit, of entry)
        let cross = match self.direction {
            Direction::LeftToRight => {
                let first = &mut self.out[i];
                first.x_advance = minus(plus(first.x_offset, exit.0), between_x);
                let second = &mut self.out[j];
                let shift = plus(second.x_offset, entry.0);
                second.x_advance = minus(second.x_advance, shift);
                second.x_offset = minus(second.x_offset, shift);
                (exit.1, entry.1)
            }
            Direction::RightToLeft => {
                // Drawn in reverse: `j` is to the left of `i`.
                let second = &mut self.out[j];
                second.x_advance = minus(plus(second.x_offset, entry.0), between_x);
                let first = &mut self.out[i];
                let shift = plus(first.x_offset, exit.0);
                first.x_advance = minus(first.x_advance, shift);
                first.x_offset = minus(first.x_offset, shift);
                (exit.1, entry.1)
            }
            Direction::TopToBottom => {
                let first = &mut self.out[i];
                first.y_advance = minus(plus(first.y_offset, exit.1), between_y);
                let second = &mut self.out[j];
                let shift = plus(second.y_offset, entry.1);
                second.y_advance = minus(second.y_advance, shift);
                second.y_offset = minus(second.y_offset, shift);
                (exit.0, entry.0)
            }
        };
        let (child, parent, shift) = if last_glyph_stays {
            (i, j, cross.1 - cross.0)
        } else {
            (j, i, cross.0 - cross.1)
        };
        let link = self.attach_mut(child);
        link.cursive_to = parent as u32;
        link.cursive_shift = shift;
        match self.direction {
            Direction::TopToBottom => self.out[child].x_offset = 0,
            _ => self.out[child].y_offset = 0,
        }
    }

    /// Contextual (LookupType 7) / chained contexts (LookupType 8)
    /// positioning with the input sequence starting at `i`.
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
        // Every rule of this lookup looks at the glyph after its first:
        // nothing can match unless that glyph is one a rule accepts.
        if let Some(second) = lookup.second_components.as_ref()
            && !skip
                .next(self.buf, i)
                .is_some_and(|at| second.contains(self.buf[at].gid))
        {
            return None;
        }
        let eng = self.eng;
        let gid = self.buf[i].gid;
        let chained = lookup.kind == 8;
        let base = self.positions.len();
        let records = {
            let view = View {
                buf: self.buf,
                skip,
                mask,
            };
            let positions = &mut *self.positions;
            lookup
                .subs
                .iter()
                .filter(|s| s.first.contains(gid))
                .find_map(|s| seq::match_context(eng.sub_bytes(s), chained, &view, i, positions))?
        };
        for &[s0, s1, l0, l1] in records.as_chunks::<4>().0 {
            let sequence_index = u16::from_be_bytes([s0, s1]) as usize;
            let lookup_index = u16::from_be_bytes([l0, l1]);
            let Some(&at) = self.positions.get(base + sequence_index) else {
                continue;
            };
            let Some(nested) = eng.lookup(lookup_index) else {
                continue;
            };
            if self.budget == 0 {
                break;
            }
            self.budget -= 1;
            let nested_skip = eng.skip_filter(nested);
            let _ = self.apply_at(nested, &nested_skip, 0, at, depth + 1);
        }
        let next = self.positions[base..].last().map_or(i, |&last| last) + 1;
        self.positions.truncate(base);
        Some(next)
    }

    /// Turn the recorded attachments into offsets (see the module docs).
    pub(super) fn resolve_attachments(&mut self) {
        if self.attach.len() != self.out.len() {
            return;
        }
        let vertical = self.direction == Direction::TopToBottom;
        // Cursive chains, cross-stream: each glyph takes the resolved
        // offset of the glyph it follows plus its own shift. A chain may
        // run forwards or backwards through the buffer.
        let count = self.out.len();
        let mut chain: Vec<usize> = Vec::new();
        for start in 0..count {
            if self.attach[start].cursive_to == UNLINKED {
                continue;
            }
            chain.clear();
            chain.push(start);
            // Walk towards the root; a chain longer than the buffer is a
            // cycle and is cut where the walk stops.
            while chain.len() <= count {
                let parent = self.attach[chain[chain.len() - 1]].cursive_to as usize;
                if parent >= count || self.attach[parent].cursive_to == UNLINKED {
                    break;
                }
                chain.push(parent);
            }
            for &node in chain.iter().rev() {
                let link = self.attach[node];
                let parent = link.cursive_to as usize;
                if let Some(&root) = self.out.get(parent) {
                    let glyph = &mut self.out[node];
                    if vertical {
                        glyph.x_offset =
                            plus(glyph.x_offset, plus(root.x_offset, link.cursive_shift));
                    } else {
                        glyph.y_offset =
                            plus(glyph.y_offset, plus(root.y_offset, link.cursive_shift));
                    }
                }
                self.attach[node].cursive_to = UNLINKED;
            }
        }
        // Marks, in buffer order: the glyph a mark attaches to always
        // precedes it, so its final position is known by now.
        let reversed = self.direction == Direction::RightToLeft;
        for i in 0..count {
            let link = self.attach[i];
            let j = link.mark_to as usize;
            if j >= i {
                continue;
            }
            // The pen travels from the attached-to glyph to the mark
            // (or, drawn in reverse, from the mark to that glyph).
            let (mut pen_x, mut pen_y) = (0i32, 0i32);
            let between = if reversed { j + 1..i + 1 } else { j..i };
            for glyph in &self.out[between] {
                pen_x = plus(pen_x, glyph.x_advance);
                pen_y = plus(pen_y, glyph.y_advance);
            }
            if !reversed {
                pen_x = minus(0, pen_x);
                pen_y = minus(0, pen_y);
            }
            let target = self.out[j];
            let mark = &mut self.out[i];
            mark.x_offset = plus(
                mark.x_offset,
                plus(plus(target.x_offset, link.mark_dx), pen_x),
            );
            mark.y_offset = plus(
                mark.y_offset,
                plus(plus(target.y_offset, link.mark_dy), pen_y),
            );
        }
    }
}
