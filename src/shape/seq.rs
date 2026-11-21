//! Sequence-context matching shared by GSUB (LookupTypes 5, 6, 8) and
//! GPOS (LookupTypes 7, 8): the three sub-table formats of the
//! "Sequence context" and "Chained sequence context" structures of the
//! common layout chapter, matched against the glyph buffer *through the
//! lookup's skip filter*.
//!
//! A glyph the lookup flag ignores is treated "just as though [it] were
//! not present in the glyph sequence": the input, backtrack and
//! lookahead sequences are all matched over the glyphs that remain. The
//! backtrack sequence is stored in reverse logical order (its element 0
//! is the glyph just before the input sequence); the lookahead sequence
//! starts after the last glyph of the input sequence.

use super::wire::{class_of, follow16, rd16};
use super::{SkipFilter, WorkGlyph, mask_allows};
use crate::tables::gdef::coverage_lookup;

/// Which of a rule's three sequences a value belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Backtrack,
    Input,
    Lookahead,
}

/// The buffer as one lookup sees it.
pub(super) struct View<'v, 'f, 'a> {
    pub(super) buf: &'v [WorkGlyph],
    pub(super) skip: &'v SkipFilter<'f, 'a>,
    /// Glyph mask the lookup is conditional on (0: unconditional). Every
    /// glyph of a matched input sequence must share a bit with it.
    pub(super) mask: u32,
}

/// `count` big-endian `uint16` values at `at`, as raw bytes.
#[inline]
fn words(bytes: &[u8], at: usize, count: usize) -> Option<&[u8]> {
    bytes.get(at..at.checked_add(count.checked_mul(2)?)?)
}

#[inline]
fn each_word(bytes: &[u8]) -> impl Iterator<Item = u16> + '_ {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_be_bytes(pair))
}

impl View<'_, '_, '_> {
    /// Match the rest of a rule whose first input glyph is at `pos`:
    /// `input` describes the input glyphs after the first, `backtrack`
    /// and `lookahead` the context around the input sequence. On success
    /// the buffer indices of the whole input sequence are appended to
    /// `positions`; on failure `positions` is left as it was.
    fn rule(
        &self,
        pos: usize,
        backtrack: &[u8],
        input: &[u8],
        lookahead: &[u8],
        accepts: &impl Fn(Part, u16, u16) -> bool,
        positions: &mut Vec<usize>,
    ) -> bool {
        let base = positions.len();
        positions.push(pos);
        let mut last = pos;
        for value in each_word(input) {
            match self.skip.next(self.buf, last) {
                Some(at)
                    if accepts(Part::Input, value, self.buf[at].gid)
                        && mask_allows(self.mask, self.buf[at].mask) =>
                {
                    positions.push(at);
                    last = at;
                }
                _ => {
                    positions.truncate(base);
                    return false;
                }
            }
        }
        let mut before = pos;
        for value in each_word(backtrack) {
            match self.skip.prev(self.buf, before) {
                Some(at) if accepts(Part::Backtrack, value, self.buf[at].gid) => before = at,
                _ => {
                    positions.truncate(base);
                    return false;
                }
            }
        }
        let mut after = last;
        for value in each_word(lookahead) {
            match self.skip.next(self.buf, after) {
                Some(at) if accepts(Part::Lookahead, value, self.buf[at].gid) => after = at,
                _ => {
                    positions.truncate(base);
                    return false;
                }
            }
        }
        true
    }

    /// Try each rule of a (Chained)SequenceRuleSet in order; the first
    /// that matches wins. Returns its SequenceLookup records.
    fn rule_set<'t>(
        &self,
        set: &'t [u8],
        chained: bool,
        pos: usize,
        accepts: &impl Fn(Part, u16, u16) -> bool,
        positions: &mut Vec<usize>,
    ) -> Option<&'t [u8]> {
        let rule_count = rd16(set, 0)? as usize;
        for r in 0..rule_count {
            let Some(rule) = follow16(set, 2 + r * 2) else {
                continue;
            };
            let parsed = if chained {
                parse_chained_rule(rule)
            } else {
                parse_rule(rule)
            };
            let Some((backtrack, input, lookahead, records)) = parsed else {
                continue;
            };
            if self.rule(pos, backtrack, input, lookahead, accepts, positions) {
                return Some(records);
            }
        }
        None
    }
}

/// The SequenceLookup records at `at`: `count` 4-byte records, cut to
/// what the table holds.
fn records(bytes: &[u8], at: usize, count: usize) -> &[u8] {
    let tail = bytes.get(at..).unwrap_or(&[]);
    let whole = tail.len().min(count * 4) / 4 * 4;
    &tail[..whole]
}

/// SequenceRule / ClassSequenceRule: `glyphCount`, `seqLookupCount`,
/// `inputSequence[glyphCount - 1]`, `seqLookupRecords[seqLookupCount]`.
#[allow(clippy::type_complexity)]
fn parse_rule(rule: &[u8]) -> Option<(&[u8], &[u8], &[u8], &[u8])> {
    let glyph_count = rd16(rule, 0)? as usize;
    let record_count = rd16(rule, 2)? as usize;
    if glyph_count == 0 {
        return None;
    }
    let input = words(rule, 4, glyph_count - 1)?;
    let none: &[u8] = &[];
    Some((
        none,
        input,
        none,
        records(rule, 4 + input.len(), record_count),
    ))
}

/// ChainedSequenceRule / ChainedClassSequenceRule: backtrack, input
/// (minus its first glyph) and lookahead arrays, each behind its count,
/// then the SequenceLookup records.
#[allow(clippy::type_complexity)]
fn parse_chained_rule(rule: &[u8]) -> Option<(&[u8], &[u8], &[u8], &[u8])> {
    let backtrack_count = rd16(rule, 0)? as usize;
    let backtrack = words(rule, 2, backtrack_count)?;
    let mut at = 2 + backtrack.len();
    let input_count = rd16(rule, at)? as usize;
    if input_count == 0 {
        return None;
    }
    let input = words(rule, at + 2, input_count - 1)?;
    at += 2 + input.len();
    let lookahead_count = rd16(rule, at)? as usize;
    let lookahead = words(rule, at + 2, lookahead_count)?;
    at += 2 + lookahead.len();
    let record_count = rd16(rule, at)? as usize;
    Some((
        backtrack,
        input,
        lookahead,
        records(rule, at + 2, record_count),
    ))
}

/// Match a SequenceContext (`chained == false`) or
/// ChainedSequenceContext (`chained == true`) sub-table with the input
/// sequence starting at `pos`. On a match, appends the buffer indices of
/// the input-sequence glyphs to `positions` and returns the rule's
/// SequenceLookup records (4 bytes each: `sequenceIndex`,
/// `lookupListIndex`).
pub(super) fn match_context<'t>(
    sub: &'t [u8],
    chained: bool,
    view: &View<'_, '_, '_>,
    pos: usize,
    positions: &mut Vec<usize>,
) -> Option<&'t [u8]> {
    let gid = view.buf[pos].gid;
    match rd16(sub, 0)? {
        // Simple glyph contexts: rule sets by Coverage index.
        1 => {
            let index = coverage_lookup(follow16(sub, 2)?, gid)? as usize;
            if index >= rd16(sub, 4)? as usize {
                return None;
            }
            let set = follow16(sub, 6 + index * 2)?;
            let same_glyph = |_: Part, want: u16, got: u16| want == got;
            view.rule_set(set, chained, pos, &same_glyph, positions)
        }
        // Class-based contexts: rule sets by the class of the first
        // input glyph.
        2 => {
            coverage_lookup(follow16(sub, 2)?, gid)?;
            // A NULL ClassDef offset leaves every glyph in class 0.
            let class_def = |at: usize| follow16(sub, at).unwrap_or(&[]);
            let (defs, sets_at) = if chained {
                ([class_def(4), class_def(6), class_def(8)], 10)
            } else {
                let input = class_def(4);
                ([input, input, input], 6)
            };
            let class = class_of(defs[1], gid) as usize;
            if class >= rd16(sub, sets_at)? as usize {
                return None;
            }
            let set = follow16(sub, sets_at + 2 + class * 2)?;
            let same_class = |part: Part, want: u16, got: u16| {
                let def = match part {
                    Part::Backtrack => defs[0],
                    Part::Input => defs[1],
                    Part::Lookahead => defs[2],
                };
                class_of(def, got) == want
            };
            view.rule_set(set, chained, pos, &same_class, positions)
        }
        // Coverage-based contexts: one rule, a Coverage table per glyph.
        3 => {
            let covered = |_: Part, offset: u16, got: u16| {
                offset != 0
                    && sub
                        .get(offset as usize..)
                        .is_some_and(|cov| coverage_lookup(cov, got).is_some())
            };
            let (backtrack, input, lookahead, recs) = if chained {
                let backtrack_count = rd16(sub, 2)? as usize;
                let backtrack = words(sub, 4, backtrack_count)?;
                let mut at = 4 + backtrack.len();
                let input_count = rd16(sub, at)? as usize;
                let input = words(sub, at + 2, input_count)?;
                at += 2 + input.len();
                let lookahead_count = rd16(sub, at)? as usize;
                let lookahead = words(sub, at + 2, lookahead_count)?;
                at += 2 + lookahead.len();
                let record_count = rd16(sub, at)? as usize;
                (
                    backtrack,
                    input,
                    lookahead,
                    records(sub, at + 2, record_count),
                )
            } else {
                let glyph_count = rd16(sub, 2)? as usize;
                let record_count = rd16(sub, 4)? as usize;
                let input = words(sub, 6, glyph_count)?;
                let none: &[u8] = &[];
                (
                    none,
                    input,
                    none,
                    records(sub, 6 + input.len(), record_count),
                )
            };
            // The first input Coverage table tests the current glyph.
            let first = rd16(input, 0)?;
            if !covered(Part::Input, first, gid) {
                return None;
            }
            view.rule(pos, backtrack, &input[2..], lookahead, &covered, positions)
                .then_some(recs)
        }
        _ => None,
    }
}

/// ReverseChainSingleSubstFormat1 (GSUB LookupType 8): the substitute
/// for the glyph at `pos` when its backtrack and lookahead Coverage
/// sequences match around it.
pub(super) fn match_reverse_chain(
    sub: &[u8],
    view: &View<'_, '_, '_>,
    pos: usize,
    positions: &mut Vec<usize>,
) -> Option<u16> {
    if rd16(sub, 0)? != 1 {
        return None;
    }
    let index = coverage_lookup(follow16(sub, 2)?, view.buf[pos].gid)? as usize;
    let backtrack_count = rd16(sub, 4)? as usize;
    let backtrack = words(sub, 6, backtrack_count)?;
    let mut at = 6 + backtrack.len();
    let lookahead_count = rd16(sub, at)? as usize;
    let lookahead = words(sub, at + 2, lookahead_count)?;
    at += 2 + lookahead.len();
    let glyph_count = rd16(sub, at)? as usize;
    if index >= glyph_count {
        return None;
    }
    let substitute = rd16(sub, at + 2 + index * 2)?;
    let covered = |_: Part, offset: u16, got: u16| {
        offset != 0
            && sub
                .get(offset as usize..)
                .is_some_and(|cov| coverage_lookup(cov, got).is_some())
    };
    let base = positions.len();
    let matched = view.rule(pos, backtrack, &[], lookahead, &covered, positions);
    positions.truncate(base);
    matched.then_some(substitute)
}
