//! Shaping accelerators: data derived once from the raw GSUB / GPOS /
//! GDEF bytes and kept for the life of a [`Font`](crate::Font), so
//! [`Font::shape`](crate::Font::shape) does not re-walk table bytes for
//! glyphs a lookup cannot touch.
//!
//! Three kinds of derived data live here, all lifetime-free (they copy
//! what they need out of the table bytes):
//!
//! * **Shape plans** — for one `(script, language, feature list)`
//!   request, the GSUB and GPOS lookups to run, in LookupList order,
//!   each with its effective type, `lookupFlag` and mark filtering set.
//!   Resolving these means walking ScriptList → LangSys → FeatureList
//!   (and FeatureVariations) and is identical for every run shaped with
//!   the same request, so plans are cached.
//! * **Lookup accelerators** — per lookup, its sub-tables resolved once
//!   (Extension wrappers unwrapped, in the order the table walkers visit
//!   them), each with the set of glyphs in its *first-glyph* Coverage
//!   table ([`GlyphSet`]), plus the union of those sets and a
//!   [`Digest`] of it. A lookup can only act at a position whose glyph is in
//!   that set (every sub-table format of every lookup type starts its
//!   match with a Coverage test on the current glyph — ISO/IEC 14496-22
//!   §6.2 / GSUB / GPOS chapters), so a miss in the union skips the
//!   position, a miss in one sub-table's set skips that sub-table, and a
//!   run whose digest does not intersect the lookup's digest skips the
//!   lookup. Ligature lookups also record which glyphs can be a second
//!   component; class-based pair-positioning sub-tables carry their two
//!   ClassDef tables flattened to per-glyph arrays.
//! * **Glyph classes** — the GDEF GlyphClassDef class of every glyph as
//!   a flat array, for the `lookupFlag` skip predicate.
//!
//! Every accelerator is a *filter in front of* the byte-walking
//! sub-table matchers, never a replacement for their decisions: the sets
//! are built as supersets of what the matchers accept, flattened tables
//! are only produced from tables the matchers read unambiguously, and
//! anything the builders cannot read with confidence degrades to "no
//! filtering".

use std::sync::{Arc, Mutex, OnceLock};

use crate::memo::Memo;
use crate::parser::read_u16;
use crate::parser::read_u32;
use crate::tables::gdef::lookup_table_slice;
use crate::tables::mvar::ItemVariationStore;

/// Which layout table a lookup belongs to. The two tables share every
/// structure this module reads; they differ in the lookup-type numbering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayoutTable {
    Gsub,
    Gpos,
}

impl LayoutTable {
    /// The Extension lookup type (GSUB 7 / GPOS 9).
    fn extension_kind(self) -> u16 {
        match self {
            LayoutTable::Gsub => 7,
            LayoutTable::Gpos => 9,
        }
    }

    /// How a sub-table of effective type `kind` locates the Coverage
    /// table tested against the glyph at the current position.
    fn coverage_layout(self, kind: u16) -> Option<CoverageLayout> {
        match (self, kind) {
            // Single / Multiple / Alternate / Ligature / ReverseChain.
            (LayoutTable::Gsub, 1..=4 | 8) => Some(CoverageLayout::AtOffset2),
            (LayoutTable::Gsub, 5) => Some(CoverageLayout::Context),
            (LayoutTable::Gsub, 6) => Some(CoverageLayout::ChainContext),
            // Single / Pair / Cursive / MarkBase / MarkLig / MarkMark.
            (LayoutTable::Gpos, 1..=6) => Some(CoverageLayout::AtOffset2),
            (LayoutTable::Gpos, 7) => Some(CoverageLayout::Context),
            (LayoutTable::Gpos, 8) => Some(CoverageLayout::ChainContext),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum CoverageLayout {
    /// `Offset16 coverageOffset` directly after the format word.
    AtOffset2,
    /// SequenceContext: formats 1 / 2 as [`Self::AtOffset2`]; format 3
    /// carries `coverageOffsets[glyphCount]` at byte 6.
    Context,
    /// ChainedSequenceContext: formats 1 / 2 as [`Self::AtOffset2`];
    /// format 3 carries the input coverage array after the backtrack
    /// coverage array.
    ChainContext,
}

/// A two-part summary of a set of glyph ids: which residues modulo 64
/// its members have, and which of 64 equal blocks of the font's glyph id
/// range they fall in. Two sets whose summaries do not
/// [intersect](Digest::intersects) share no glyph; letters and marks of
/// a text font usually sit in different blocks, so the block half
/// separates what the residue half (saturated by any large set) cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Digest {
    residues: u64,
    blocks: u64,
}

/// `log2` of the block size that splits the glyph ids `0..num_glyphs`
/// into at most 64 blocks.
pub(crate) fn block_shift(num_glyphs: usize) -> u8 {
    let bits = usize::BITS - num_glyphs.saturating_sub(1).leading_zeros();
    bits.saturating_sub(6) as u8
}

impl Digest {
    /// Summary of the empty set.
    pub(crate) const EMPTY: Digest = Digest {
        residues: 0,
        blocks: 0,
    };
    /// Summary that intersects every non-empty summary.
    pub(crate) const FULL: Digest = Digest {
        residues: u64::MAX,
        blocks: u64::MAX,
    };

    /// Summary of one glyph id. Ids past the last block (not glyphs of
    /// the font) all count for the last block.
    #[inline]
    pub(crate) fn of(gid: u16, shift: u8) -> Digest {
        Digest {
            residues: digest_bit(gid),
            blocks: 1u64 << ((gid >> shift).min(63)),
        }
    }

    #[inline]
    pub(crate) fn add(&mut self, other: Digest) {
        self.residues |= other.residues;
        self.blocks |= other.blocks;
    }

    /// `false` only when the two summarised sets share no glyph.
    #[inline]
    pub(crate) fn intersects(&self, other: &Digest) -> bool {
        self.residues & other.residues != 0 && self.blocks & other.blocks != 0
    }
}

/// A set of glyph ids as a bitmap over `0..capacity`, with a
/// [`Digest`] of its members.
///
/// Membership is *conservative*: [`GlyphSet::contains`] answers `true`
/// for every member and for every glyph id beyond the bitmap's
/// capacity. A `false` answer is exact. The universal set ("no
/// filtering") is the zero-capacity bitmap with a saturated digest.
#[derive(Debug)]
pub(crate) struct GlyphSet {
    words: Box<[u64]>,
    digest: Digest,
    /// Block shift of the digest ([`block_shift`] of the capacity).
    shift: u8,
}

/// Bit `gid % 64`: the position of a glyph id inside its bitmap word.
#[inline]
pub(crate) fn digest_bit(gid: u16) -> u64 {
    1u64 << (gid & 63)
}

impl GlyphSet {
    fn with_capacity(glyphs: usize) -> Self {
        Self {
            words: vec![0u64; glyphs.div_ceil(64)].into_boxed_slice(),
            digest: Digest::EMPTY,
            shift: block_shift(glyphs),
        }
    }

    fn universal() -> Self {
        Self {
            words: Box::new([]),
            digest: Digest::FULL,
            shift: 0,
        }
    }

    #[inline]
    pub(crate) fn contains(&self, gid: u16) -> bool {
        match self.words.get((gid >> 6) as usize) {
            Some(w) => (w >> (gid & 63)) & 1 != 0,
            // Beyond the bitmap (or a universal set): cannot rule it out.
            None => true,
        }
    }

    /// The summary of the set's members.
    #[inline]
    pub(crate) fn digest(&self) -> Digest {
        self.digest
    }

    fn insert(&mut self, gid: u16) {
        self.digest.add(Digest::of(gid, self.shift));
        if let Some(w) = self.words.get_mut((gid >> 6) as usize) {
            *w |= 1u64 << (gid & 63);
        }
    }

    fn insert_range(&mut self, start: u16, end: u16) {
        if start > end {
            return;
        }
        if end - start >= 63 {
            self.digest.residues = u64::MAX;
        } else {
            for g in start..=end {
                self.digest.residues |= digest_bit(g);
            }
        }
        for block in (start >> self.shift).min(63)..=(end >> self.shift).min(63) {
            self.digest.blocks |= 1u64 << block;
        }
        let cap = self.words.len() * 64;
        let lo = start as usize;
        let hi = (end as usize + 1).min(cap);
        if lo >= hi {
            return;
        }
        let (first_word, last_word) = (lo / 64, (hi - 1) / 64);
        for wi in first_word..=last_word {
            let from = if wi == first_word { lo % 64 } else { 0 };
            let to = if wi == last_word { (hi - 1) % 64 } else { 63 };
            let mask = (u64::MAX >> (63 - to)) & (u64::MAX << from);
            self.words[wi] |= mask;
        }
    }

    /// Add every member of `other` (a set of the same capacity; a
    /// universal `other` makes `self` universal).
    fn union_with(&mut self, other: &GlyphSet) {
        self.digest.add(other.digest);
        if other.words.len() != self.words.len() {
            *self = GlyphSet::universal();
            return;
        }
        for (word, &more) in self.words.iter_mut().zip(other.words.iter()) {
            *word |= more;
        }
    }

    /// Add every glyph a Coverage table (§6.2.5) lists. Reads as far as
    /// the bytes allow; an unreadable tail or unknown format adds
    /// nothing (the byte-walking lookup rejects those too).
    fn insert_coverage(&mut self, cov: &[u8]) {
        if cov.len() < 4 {
            return;
        }
        let (Ok(format), Ok(count)) = (read_u16(cov, 0), read_u16(cov, 2)) else {
            return;
        };
        match format {
            1 => {
                for i in 0..count as usize {
                    match read_u16(cov, 4 + i * 2) {
                        Ok(g) => self.insert(g),
                        Err(_) => break,
                    }
                }
            }
            2 => {
                for i in 0..count as usize {
                    let off = 4 + i * 6;
                    match (read_u16(cov, off), read_u16(cov, off + 2)) {
                        (Ok(s), Ok(e)) => self.insert_range(s, e),
                        _ => break,
                    }
                }
            }
            _ => {}
        }
    }
}

/// One resolved sub-table of a lookup.
#[derive(Debug)]
pub(crate) struct SubAccel {
    /// Byte offset of the sub-table (the Extension target, when the
    /// lookup wraps its sub-tables) from the start of the layout table.
    pub(crate) offset: usize,
    /// Glyphs in the sub-table's first-glyph Coverage table.
    pub(crate) first: GlyphSet,
    /// `true` when `first` is exactly the set the sub-table matcher
    /// accepts for glyph ids below the font's glyph count (a well-formed
    /// Coverage table), not merely a superset of it.
    pub(crate) first_is_exact: bool,
    /// GSUB Single / Alternate / Ligature substitution with a
    /// well-formed Coverage table: Coverage index plus one per glyph (0:
    /// not covered) — the index of the glyph's substitute, AlternateSet
    /// or LigatureSet.
    pub(crate) coverage_index: Option<CoverageIndex>,
    /// GPOS mark attachment (LookupTypes 4 – 6) with a well-formed,
    /// dense second Coverage table (base, ligature or mark2 Coverage):
    /// its flattened index.
    pub(crate) second_index: Option<CoverageIndex>,
    /// PairPos only: the flattened glyph → value-record addressing.
    pub(crate) pair: PairAccel,
}

/// Flattened addressing data of one PairPos sub-table.
#[derive(Debug)]
pub(crate) enum PairAccel {
    /// Not a PairPos sub-table, or one whose tables are not well-formed
    /// enough to flatten: go through the sub-table matcher.
    None,
    /// PairPosFormat1 (glyph pairs).
    Sets(PairSets),
    /// PairPosFormat2 (class pairs).
    Classes(PairClasses),
}

/// PairPosFormat1 (§6.3.3.3 pair adjustment, glyph pairs) with its
/// Coverage table flattened: `coverage_index[g]` is one more than the
/// Coverage index of glyph `g` — the index of its PairSet — or 0 when
/// the sub-table does not cover `g`.
#[derive(Debug)]
pub(crate) struct PairSets {
    pub(crate) coverage_index: CoverageIndex,
}

impl PairSets {
    fn build(sub: &[u8], num_glyphs: usize) -> Option<Self> {
        if sub.len() < 8 || read_u16(sub, 0).ok()? != 1 {
            return None;
        }
        let coverage = sub.get(read_u16(sub, 2).ok()? as usize..)?;
        Some(Self {
            coverage_index: flatten_coverage(coverage, num_glyphs)?,
        })
    }
}

/// PairPosFormat2 (§6.3.3.3 class pair adjustment) with both ClassDef
/// tables flattened: the value record of a glyph pair is addressed by
/// two array reads instead of two ClassDef searches.
#[derive(Debug)]
pub(crate) struct PairClasses {
    pub(crate) class1: Box<[u16]>,
    pub(crate) class2: Box<[u16]>,
}

impl PairClasses {
    fn build(sub: &[u8], num_glyphs: usize) -> Option<Self> {
        if sub.len() < 16 || read_u16(sub, 0).ok()? != 2 {
            return None;
        }
        let class_def1 = sub.get(read_u16(sub, 8).ok()? as usize..)?;
        let class_def2 = sub.get(read_u16(sub, 10).ok()? as usize..)?;
        Some(Self {
            class1: flatten_class_def(class_def1, num_glyphs)?,
            class2: flatten_class_def(class_def2, num_glyphs)?,
        })
    }
}

/// One resolved lookup: its effective type, `lookupFlag`, and
/// sub-tables, with the accelerators described in the module docs
/// (absent when they are switched off — the glyph sets are then
/// universal and nothing is flattened).
#[derive(Debug)]
pub(crate) struct LookupAccel {
    /// Effective lookup type (Extension wrappers unwrapped).
    pub(crate) kind: u16,
    /// `lookupFlag`.
    pub(crate) flags: u16,
    /// `markFilteringSet`, when the flag word announces one.
    pub(crate) mark_filtering_set: Option<u16>,
    /// Glyphs at which the lookup can act (first-glyph coverage union).
    pub(crate) first: GlyphSet,
    /// The lookup's sub-tables of its effective type, in the order the
    /// Lookup table lists them.
    pub(crate) subs: Box<[SubAccel]>,
    /// GSUB LigatureSubst only: the largest `componentCount` of any
    /// Ligature record in the lookup — the longest glyph sequence one
    /// match can inspect. `usize::MAX` when it could not be determined.
    pub(crate) max_components: usize,
    /// Ligature and (chained) contextual lookups: every glyph that some
    /// rule of the lookup accepts directly after its first glyph — the
    /// second component of a Ligature record, the second input glyph of
    /// a sequence rule, or the first lookahead glyph of a rule with a
    /// one-glyph input sequence. Present only when *every* rule of the
    /// lookup looks at such a glyph and could be read; the lookup can
    /// then only act where the next visible glyph is in this set.
    pub(crate) second_components: Option<GlyphSet>,
    /// GPOS PairPos only: every sub-table adjusts nothing but the first
    /// glyph's x-advance (ValueFormat1 is `X_ADVANCE`, optionally with
    /// its device / variation offset; ValueFormat2 is empty). The
    /// lookup's whole effect on a glyph pair is then one number, which
    /// `pair_memo` caches.
    pub(crate) plain_kerning: bool,
    /// For a lookup whose flag qualifies which marks it ignores (a mark
    /// filtering set or a mark attachment class): the marks it does
    /// *not* ignore, built on first use (`None` inside: not derivable;
    /// ask GDEF per glyph).
    pub(crate) kept_marks: OnceLock<Option<GlyphSet>>,
    /// GPOS PairPos with `plain_kerning`: memo of the lookup's
    /// adjustment per `(left, right)` glyph pair at the current
    /// variation instance. Text repeats letter pairs heavily, so most
    /// pairs are answered here without visiting a sub-table.
    pub(crate) pair_memo: Memo,
}

/// log2 of the pair memo's slot count: 4096 slots, 32 KiB, allocated
/// once the lookup has positioned a few hundred pairs.
const PAIR_MEMO_BITS: u32 = 12;

impl LookupAccel {
    /// Resolve lookup `lookup_index` of a layout table. `filters ==
    /// false` skips every glyph set and flattened table: the result then
    /// only lists the sub-tables. `None` when the lookup does not exist
    /// or its type is not one the shaping engine applies.
    fn build(
        table: LayoutTable,
        bytes: &[u8],
        lookup_list_off: u32,
        lookup_index: u16,
        num_glyphs: usize,
        filters: bool,
    ) -> Option<Self> {
        if lookup_list_off == 0 {
            return None;
        }
        let lookup = lookup_table_slice(bytes, lookup_list_off, lookup_index)?;
        // `lookup` is a tail of `bytes`.
        let lookup_start = bytes.len() - lookup.len();
        let outer_kind = read_u16(lookup, 0).ok()?;
        let flags = read_u16(lookup, 2).ok()?;
        let sub_count = read_u16(lookup, 4).ok()? as usize;
        let mark_filtering_set = if flags & 0x0010 != 0 {
            read_u16(lookup, 6 + sub_count * 2).ok()
        } else {
            None
        };
        let is_extension = outer_kind == table.extension_kind();

        // Sub-tables, Extension wrappers removed: (type, bytes).
        let resolved = (0..sub_count).filter_map(|s| {
            let sub_off = read_u16(lookup, 6 + s * 2).ok()?;
            let sub = lookup.get(sub_off as usize..)?;
            if !is_extension {
                return Some((outer_kind, sub));
            }
            // Extension format 1: u16 format, u16 extensionLookupType,
            // Offset32 extensionOffset (from the Extension sub-table).
            let ext_kind = read_u16(sub, 2).ok()?;
            let ext_off = read_u32(sub, 4).ok()?;
            Some((ext_kind, sub.get(ext_off as usize..)?))
        });
        // "All subtables within a Lookup table must be of the same
        // lookup type": the first readable one names it.
        let kind = resolved.clone().next().map_or(outer_kind, |(k, _)| k);
        let layout = table.coverage_layout(kind)?;

        let is_ligature = table == LayoutTable::Gsub && kind == 4;
        // Sub-table formats whose matcher is "Coverage index of the
        // current glyph, then index an array with it".
        let is_mark_attach = table == LayoutTable::Gpos && matches!(kind, 4..=6);
        let wants_coverage_index =
            (table == LayoutTable::Gsub && matches!(kind, 1 | 3 | 4)) || is_mark_attach;
        let is_pair = table == LayoutTable::Gpos && kind == 2;

        let glyph_set = || match filters {
            true => GlyphSet::with_capacity(num_glyphs),
            false => GlyphSet::universal(),
        };
        let mut first = glyph_set();
        let mut subs = Vec::new();
        let mut max_components = 0usize;
        // Sequence context (`Some(false)`) or chained (`Some(true)`).
        let context = match (table, kind) {
            (LayoutTable::Gsub, 5) | (LayoutTable::Gpos, 7) => Some(false),
            (LayoutTable::Gsub, 6) | (LayoutTable::Gpos, 8) => Some(true),
            _ => None,
        };
        let mut second = ((is_ligature || context.is_some()) && filters)
            .then(|| GlyphSet::with_capacity(num_glyphs));
        let mut plain_kerning = is_pair && filters;
        for (sub_kind, sub) in resolved {
            if sub_kind != kind {
                continue;
            }
            if is_pair {
                // X_ADVANCE, with or without its device / variation
                // offset, and nothing for the second glyph.
                plain_kerning &= matches!(
                    (read_u16(sub, 4), read_u16(sub, 6)),
                    (Ok(0x0004 | 0x0044), Ok(0))
                );
            }
            let mut sub_first = glyph_set();
            let mut first_is_exact = false;
            let mut coverage_index = None;
            if filters {
                match first_coverage(layout, sub) {
                    FirstCoverage::At(cov) => {
                        sub_first.insert_coverage(cov);
                        first_is_exact = coverage_is_well_formed(cov);
                        first.union_with(&sub_first);
                        // The matchers want a 6-byte sub-table header.
                        if wants_coverage_index && sub.len() >= 6 {
                            coverage_index = match is_mark_attach {
                                true => flatten_dense_coverage(cov, num_glyphs),
                                false => flatten_coverage(cov, num_glyphs),
                            };
                        }
                    }
                    FirstCoverage::None => {}
                    FirstCoverage::Unknown => {
                        sub_first = GlyphSet::universal();
                        first = GlyphSet::universal();
                    }
                }
                if let Some(chained) = context
                    && let Some(followers) = second.as_mut()
                    && context_followers(sub, chained, followers).is_none()
                {
                    second = None;
                }
                if is_ligature {
                    match ligature_components(sub, second.as_mut()) {
                        Some(LigatureShape {
                            max_components: n,
                            all_multi_component,
                        }) => {
                            max_components = max_components.max(n);
                            if !all_multi_component {
                                second = None;
                            }
                        }
                        None => {
                            max_components = usize::MAX;
                            second = None;
                        }
                    }
                }
            }
            subs.push(SubAccel {
                // `sub` is a tail of `lookup`, itself a tail of `bytes`.
                offset: lookup_start + (lookup.len() - sub.len()),
                first: sub_first,
                first_is_exact,
                coverage_index,
                // Offset16 base / ligature / mark2 Coverage at byte 4.
                second_index: match is_mark_attach && filters {
                    true => read_u16(sub, 4)
                        .ok()
                        .and_then(|off| sub.get(off as usize..))
                        .and_then(|cov| flatten_dense_coverage(cov, num_glyphs)),
                    false => None,
                },
                pair: match is_pair && filters {
                    true => PairClasses::build(sub, num_glyphs)
                        .map(PairAccel::Classes)
                        .or_else(|| PairSets::build(sub, num_glyphs).map(PairAccel::Sets))
                        .unwrap_or(PairAccel::None),
                    false => PairAccel::None,
                },
            });
        }
        Some(LookupAccel {
            kind,
            flags,
            mark_filtering_set,
            first,
            subs: subs.into_boxed_slice(),
            max_components: if is_ligature && filters {
                max_components
            } else {
                usize::MAX
            },
            second_components: second,
            plain_kerning,
            kept_marks: OnceLock::new(),
            pair_memo: Memo::with_bits(PAIR_MEMO_BITS),
        })
    }
}

/// The Coverage table a sub-table of effective type `kind` tests the
/// current glyph against, when it has one that can be located.
pub(crate) fn first_glyph_coverage(table: LayoutTable, kind: u16, sub: &[u8]) -> Option<&[u8]> {
    match first_coverage(table.coverage_layout(kind)?, sub) {
        FirstCoverage::At(coverage) => Some(coverage),
        FirstCoverage::None | FirstCoverage::Unknown => None,
    }
}

enum FirstCoverage<'b> {
    /// The Coverage table tested against the current glyph.
    At(&'b [u8]),
    /// The sub-table can never match (e.g. an empty input sequence).
    None,
    /// Could not be located; the caller must not filter.
    Unknown,
}

/// Locate the Coverage table a sub-table tests against the glyph at the
/// current position.
fn first_coverage(layout: CoverageLayout, sub: &[u8]) -> FirstCoverage<'_> {
    let at = |off_pos: usize| match read_u16(sub, off_pos) {
        Ok(off) => match sub.get(off as usize..) {
            Some(cov) => FirstCoverage::At(cov),
            None => FirstCoverage::None,
        },
        Err(_) => FirstCoverage::Unknown,
    };
    let Ok(format) = read_u16(sub, 0) else {
        return FirstCoverage::Unknown;
    };
    match layout {
        CoverageLayout::AtOffset2 => at(2),
        CoverageLayout::Context => match format {
            1 | 2 => at(2),
            3 => match read_u16(sub, 2) {
                // glyphCount == 0: no input sequence, never matches.
                Ok(0) => FirstCoverage::None,
                Ok(_) => at(6),
                Err(_) => FirstCoverage::Unknown,
            },
            _ => FirstCoverage::None,
        },
        CoverageLayout::ChainContext => match format {
            1 | 2 => at(2),
            3 => {
                let Ok(backtrack_count) = read_u16(sub, 2) else {
                    return FirstCoverage::Unknown;
                };
                let input_count_pos = 4 + backtrack_count as usize * 2;
                match read_u16(sub, input_count_pos) {
                    Ok(0) => FirstCoverage::None,
                    Ok(_) => at(input_count_pos + 2),
                    Err(_) => FirstCoverage::Unknown,
                }
            }
            _ => FirstCoverage::None,
        },
    }
}

struct LigatureShape {
    /// Largest `componentCount` of any Ligature record.
    max_components: usize,
    /// Every Ligature record has `componentCount >= 2`.
    all_multi_component: bool,
}

/// Walk every Ligature record of a LigatureSubstFormat1 sub-table,
/// adding each record's second component to `second`. `None` when a
/// record could not be read (the caller then stops filtering on
/// component data).
fn ligature_components(sub: &[u8], mut second: Option<&mut GlyphSet>) -> Option<LigatureShape> {
    let set_count = read_u16(sub, 4).ok()? as usize;
    let mut shape = LigatureShape {
        max_components: 0,
        all_multi_component: true,
    };
    for s in 0..set_count {
        let set_off = read_u16(sub, 6 + s * 2).ok()? as usize;
        let set = sub.get(set_off..)?;
        let lig_count = read_u16(set, 0).ok()? as usize;
        for l in 0..lig_count {
            let lig_off = read_u16(set, 2 + l * 2).ok()? as usize;
            let lig = set.get(lig_off..)?;
            let components = read_u16(lig, 2).ok()? as usize;
            shape.max_components = shape.max_components.max(components);
            match components {
                // Never matches (the matcher skips empty records).
                0 => {}
                1 => shape.all_multi_component = false,
                _ => {
                    let glyph = read_u16(lig, 4).ok()?;
                    if let Some(set) = second.as_deref_mut() {
                        set.insert(glyph);
                    }
                }
            }
        }
    }
    Some(shape)
}

/// Add every glyph a ClassDef table (§6.2.6) puts in `class` (not 0).
/// `None` when the table cannot be read.
fn insert_class(set: &mut GlyphSet, class_def: &[u8], class: u16) -> Option<()> {
    match read_u16(class_def, 0).ok()? {
        1 => {
            let start = read_u16(class_def, 2).ok()?;
            let count = read_u16(class_def, 4).ok()?;
            for i in 0..count {
                if read_u16(class_def, 6 + i as usize * 2).ok()? == class {
                    match start.checked_add(i) {
                        Some(glyph) => set.insert(glyph),
                        None => break,
                    }
                }
            }
        }
        2 => {
            let count = read_u16(class_def, 2).ok()? as usize;
            for i in 0..count {
                let at = 4 + i * 6;
                if read_u16(class_def, at + 4).ok()? == class {
                    set.insert_range(
                        read_u16(class_def, at).ok()?,
                        read_u16(class_def, at + 2).ok()?,
                    );
                }
            }
        }
        // An unknown format assigns no class.
        _ => {}
    }
    Some(())
}

/// The table an `Offset16` at `at` points to; `Ok(None)` for NULL.
fn optional_table(bytes: &[u8], at: usize) -> Option<Option<&[u8]>> {
    match read_u16(bytes, at).ok()? {
        0 => Some(None),
        offset => Some(Some(bytes.get(offset as usize..)?)),
    }
}

/// Add to `followers` every glyph some rule of a sequence-context
/// (`chained == false`) or chained-sequence-context sub-table accepts
/// directly after its first input glyph: the second input glyph, or —
/// for a one-glyph input sequence — the first lookahead glyph.
///
/// `None` when a rule looks at no glyph after its first (so the
/// sub-table can match whatever follows), when a rule asks for class 0
/// there (any glyph outside the ClassDef), or when the sub-table cannot
/// be read with confidence; the caller then does not filter.
fn context_followers(sub: &[u8], chained: bool, followers: &mut GlyphSet) -> Option<()> {
    /// What a rule asks of the glyph after its first input glyph:
    /// `(from the lookahead sequence, value)`. `Ok(None)`: the rule can
    /// never match. `Err`: no such glyph, or unreadable.
    fn follower(rule: &[u8], chained: bool) -> Result<Option<(bool, u16)>, ()> {
        let word = |at: usize| read_u16(rule, at).map_err(|_| ());
        if !chained {
            return match word(0)? {
                0 => Ok(None),
                1 => Err(()),
                _ => Ok(Some((false, word(4)?))),
            };
        }
        let input_at = 2 + word(0)? as usize * 2;
        match word(input_at)? {
            0 => Ok(None),
            1 => {
                let lookahead_at = input_at + 2;
                match word(lookahead_at)? {
                    0 => Err(()),
                    _ => Ok(Some((true, word(lookahead_at + 2)?))),
                }
            }
            _ => Ok(Some((false, word(input_at + 2)?))),
        }
    }
    /// Every rule of the rule set whose offset is stored at `at`.
    fn each_rule(
        sub: &[u8],
        at: usize,
        chained: bool,
        mut visit: impl FnMut(bool, u16) -> Option<()>,
    ) -> Option<()> {
        let Some(set) = optional_table(sub, at)? else {
            return Some(());
        };
        for r in 0..read_u16(set, 0).ok()? as usize {
            let Some(rule) = optional_table(set, 2 + r * 2)? else {
                continue;
            };
            if let Some((from_lookahead, value)) = follower(rule, chained).ok()? {
                visit(from_lookahead, value)?;
            }
        }
        Some(())
    }

    match read_u16(sub, 0).ok()? {
        1 => {
            for s in 0..read_u16(sub, 4).ok()? as usize {
                each_rule(sub, 6 + s * 2, chained, |_, glyph| {
                    followers.insert(glyph);
                    Some(())
                })?;
            }
        }
        2 => {
            let (input_def_at, lookahead_def_at, sets_at) =
                if chained { (6, 8, 10) } else { (4, 4, 6) };
            let input_def = optional_table(sub, input_def_at)?.unwrap_or(&[]);
            let lookahead_def = optional_table(sub, lookahead_def_at)?.unwrap_or(&[]);
            // Each (sequence, class) once.
            let mut seen: Vec<(bool, u16)> = Vec::new();
            for s in 0..read_u16(sub, sets_at).ok()? as usize {
                each_rule(
                    sub,
                    sets_at + 2 + s * 2,
                    chained,
                    |from_lookahead, class| {
                        if class == 0 {
                            return None;
                        }
                        if seen.contains(&(from_lookahead, class)) {
                            return Some(());
                        }
                        seen.push((from_lookahead, class));
                        let def = if from_lookahead {
                            lookahead_def
                        } else {
                            input_def
                        };
                        // An empty (NULL) ClassDef assigns no class.
                        if def.is_empty() {
                            return Some(());
                        }
                        insert_class(followers, def, class)
                    },
                )?;
            }
        }
        3 => {
            let coverage_at = if chained {
                let input_at = 4 + read_u16(sub, 2).ok()? as usize * 2;
                match read_u16(sub, input_at).ok()? {
                    0 => return Some(()),
                    1 => {
                        let lookahead_at = input_at + 4;
                        match read_u16(sub, lookahead_at).ok()? {
                            0 => return None,
                            _ => lookahead_at + 2,
                        }
                    }
                    _ => input_at + 4,
                }
            } else {
                match read_u16(sub, 2).ok()? {
                    0 => return Some(()),
                    1 => return None,
                    _ => 8,
                }
            };
            if let Some(coverage) = optional_table(sub, coverage_at)? {
                followers.insert_coverage(coverage);
            }
        }
        // An unknown format never matches.
        _ => {}
    }
    Some(())
}

/// `true` when a Coverage table is one a binary search reads
/// unambiguously: every record readable and in strictly increasing
/// order (format 2: non-overlapping ranges with `start <= end`).
fn coverage_is_well_formed(cov: &[u8]) -> bool {
    let (Ok(format), Ok(count)) = (read_u16(cov, 0), read_u16(cov, 2)) else {
        return false;
    };
    let mut next_free = 0u32;
    match format {
        1 => (0..count as usize).all(|i| match read_u16(cov, 4 + i * 2) {
            Ok(g) if g as u32 >= next_free => {
                next_free = g as u32 + 1;
                true
            }
            _ => false,
        }),
        2 => (0..count as usize).all(|i| {
            let off = 4 + i * 6;
            match (read_u16(cov, off), read_u16(cov, off + 2)) {
                (Ok(s), Ok(e)) if s <= e && s as u32 >= next_free => {
                    next_free = e as u32 + 1;
                    true
                }
                _ => false,
            }
        }),
        _ => false,
    }
}

/// One resolved lookup, built on first request (`None` inside: no such
/// lookup, or a type the engine does not apply).
type AccelSlot = OnceLock<Option<Box<LookupAccel>>>;

/// Lazily resolved lookups of one layout table.
#[derive(Debug, Default)]
struct TableAccels {
    slots: OnceLock<Box<[AccelSlot]>>,
}

impl TableAccels {
    fn get(
        &self,
        table: LayoutTable,
        bytes: &[u8],
        lookup_list_off: u32,
        lookup_index: u16,
        num_glyphs: usize,
        filters: bool,
    ) -> Option<&LookupAccel> {
        let slots = self.slots.get_or_init(|| {
            let count = bytes
                .get(lookup_list_off as usize..)
                .and_then(|l| read_u16(l, 0).ok())
                .unwrap_or(0);
            (0..count).map(|_| OnceLock::new()).collect()
        });
        let slot = slots.get(lookup_index as usize)?;
        slot.get_or_init(|| {
            LookupAccel::build(
                table,
                bytes,
                lookup_list_off,
                lookup_index,
                num_glyphs,
                filters,
            )
            .map(Box::new)
        })
        .as_deref()
    }
}

/// One lookup of a shape plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlanLookup {
    pub(crate) index: u16,
    /// Glyph mask the lookup is conditional on: it applies where the
    /// glyphs of its input sequence all share a bit with this mask. `0`:
    /// applies everywhere.
    pub(crate) mask: u32,
    /// Alternate picked by an AlternateSubst lookup (zero-based).
    pub(crate) alternate: u16,
    /// Digest of the lookup's first-glyph set, and of its second-glyph
    /// set (full when it has none): a run whose glyph digest misses
    /// either cannot be touched by the lookup, which is decided here
    /// without visiting the lookup.
    pub(crate) first_digest: Digest,
    pub(crate) second_digest: Digest,
}

/// One requested feature of a shape plan key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlanFeature {
    pub(crate) tag: [u8; 4],
    pub(crate) mask: u32,
    pub(crate) alternate: u16,
}

/// The lookups one shaping request runs, in application order.
#[derive(Debug, Default)]
pub(crate) struct ShapePlan {
    pub(crate) gsub: Vec<PlanLookup>,
    pub(crate) gpos: Vec<PlanLookup>,
    /// `Some(mask)`: the request asks for `kern` (under glyph mask
    /// `mask`), GPOS has no `kern` feature for the language system, and
    /// the font has a `kern` table — kerning comes from that table.
    pub(crate) legacy_kern: Option<u32>,
}

#[derive(Debug)]
struct PlanEntry {
    script: [u8; 4],
    lang: Option<[u8; 4]>,
    features: Box<[PlanFeature]>,
    plan: Arc<ShapePlan>,
}

/// Most distinct `(script, language, features)` requests kept per font.
const MAX_CACHED_PLANS: usize = 32;

/// Variation data of the current instance that positioning reads.
#[derive(Debug)]
pub(crate) struct InstanceData {
    /// Normalised axis coordinates.
    pub(crate) coords: Box<[f32]>,
    /// `true` when every coordinate is zero: no delta applies.
    pub(crate) is_default: bool,
}

/// Memo of varied nominal advances: 1024 slots, 8 KiB, allocated once
/// a few hundred glyphs have been asked for.
struct VariedAdvances(Memo);

impl Default for VariedAdvances {
    fn default() -> Self {
        VariedAdvances(Memo::with_bits(10))
    }
}

impl std::fmt::Debug for VariedAdvances {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VariedAdvances")
    }
}

/// All shaping accelerators of one font.
#[derive(Debug, Default)]
pub(crate) struct LayoutCache {
    /// When set, no glyph set or flattened table is built: shaping
    /// walks the table bytes for every lookup at every position. The
    /// resolved sub-table lists and shape plans are still kept (they
    /// are a few dozen bytes each).
    lean: bool,
    gsub: TableAccels,
    gpos: TableAccels,
    /// GDEF GlyphClassDef class per glyph (`None` inside: no usable
    /// flat table could be built; callers fall back to the GDEF walker).
    glyph_classes: OnceLock<Option<Box<[u16]>>>,
    /// The GDEF ItemVariationStore, parsed once (`None` inside: absent
    /// or unreadable).
    gdef_store: OnceLock<Option<ItemVariationStore>>,
    /// Derived data of the current variation instance.
    instance: OnceLock<InstanceData>,
    /// Nominal advance per glyph at the current variation instance
    /// (`hmtx` + `HVAR`), for a font away from its default instance.
    varied_advances: VariedAdvances,
    /// Most-recently-used first.
    plans: Mutex<Vec<PlanEntry>>,
}

impl LayoutCache {
    /// Lookup `lookup_index` of a layout table, resolved on first use.
    pub(crate) fn lookup(
        &self,
        table: LayoutTable,
        bytes: &[u8],
        lookup_list_off: u32,
        lookup_index: u16,
        num_glyphs: usize,
    ) -> Option<&LookupAccel> {
        let accels = match table {
            LayoutTable::Gsub => &self.gsub,
            LayoutTable::Gpos => &self.gpos,
        };
        accels.get(
            table,
            bytes,
            lookup_list_off,
            lookup_index,
            num_glyphs,
            !self.lean,
        )
    }

    /// The flat GDEF class array, built on first use with `build`.
    pub(crate) fn glyph_classes(
        &self,
        build: impl FnOnce() -> Option<Box<[u16]>>,
    ) -> Option<&[u16]> {
        if self.lean {
            return None;
        }
        self.glyph_classes.get_or_init(build).as_deref()
    }

    /// The GDEF ItemVariationStore, parsed on first use with `parse`.
    pub(crate) fn gdef_store(
        &self,
        parse: impl FnOnce() -> Option<ItemVariationStore>,
    ) -> Option<&ItemVariationStore> {
        self.gdef_store.get_or_init(parse).as_ref()
    }

    /// Data of the current variation instance, derived on first use
    /// from the normalised coordinates `coords` produces.
    pub(crate) fn instance(&self, coords: impl FnOnce() -> Vec<f32>) -> &InstanceData {
        self.instance.get_or_init(|| {
            let coords = coords().into_boxed_slice();
            InstanceData {
                is_default: coords.iter().all(|&c| c == 0.0),
                coords,
            }
        })
    }

    /// The nominal advance of `glyph_id` at the current variation
    /// instance: memoised `compute()` (accelerators on, and a value
    /// that fits 16 bits), else `compute()` itself.
    pub(crate) fn varied_advance(&self, glyph_id: u16, compute: impl Fn() -> i32) -> i32 {
        if self.lean {
            return compute();
        }
        self.varied_advances
            .0
            .get_or_compute(glyph_id as u32, || u16::try_from(compute()).ok())
            .map_or_else(&compute, i32::from)
    }

    /// `true` when accelerators are switched off.
    pub(crate) fn is_lean(&self) -> bool {
        self.lean
    }

    /// A cache with accelerators on (`lean == false`) or off, holding
    /// nothing yet.
    pub(crate) fn with_lean(lean: bool) -> Self {
        Self {
            lean,
            ..Self::default()
        }
    }

    /// The cached plan for a request, or the one `build` produces.
    /// `features` yields the request's features; two requests are the
    /// same when they list equal features in the same order.
    pub(crate) fn plan<I>(
        &self,
        script: [u8; 4],
        lang: Option<[u8; 4]>,
        features: I,
        build: impl FnOnce() -> ShapePlan,
    ) -> Arc<ShapePlan>
    where
        I: Iterator<Item = PlanFeature> + Clone,
    {
        {
            let mut plans = self.plans.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(i) = plans.iter().position(|e| {
                e.script == script
                    && e.lang == lang
                    && e.features.iter().copied().eq(features.clone())
            }) {
                if i != 0 {
                    plans[..=i].rotate_right(1);
                }
                return Arc::clone(&plans[0].plan);
            }
        }
        // Built outside the lock: resolution walks table bytes and may
        // itself touch lazily parsed tables.
        let plan = Arc::new(build());
        let mut plans = self.plans.lock().unwrap_or_else(|e| e.into_inner());
        if plans.len() >= MAX_CACHED_PLANS {
            plans.pop();
        }
        plans.insert(
            0,
            PlanEntry {
                script,
                lang,
                features: features.collect(),
                plan: Arc::clone(&plan),
            },
        );
        plan
    }

    /// Forget everything derived from the variation instance. Called
    /// when the instance changes: FeatureVariations can swap the lookups
    /// behind a feature, and every delta moves.
    pub(crate) fn clear_plans(&mut self) {
        self.plans
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.instance = OnceLock::new();
        // Memoised values of the old instance.
        self.varied_advances = VariedAdvances::default();
        if let Some(slots) = self.gpos.slots.get_mut() {
            for slot in slots.iter_mut() {
                if let Some(Some(lookup)) = slot.get_mut() {
                    lookup.pair_memo = Memo::with_bits(PAIR_MEMO_BITS);
                }
            }
        }
    }
}

/// Where a flattened Coverage table puts a glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Covered {
    /// Not covered.
    Miss,
    /// Covered, at this Coverage index.
    At(u16),
    /// Not a glyph of the font: the flattened form does not say; search
    /// the Coverage table.
    Unknown,
}

/// A well-formed Coverage table (§6.2.5) flattened to an array: the
/// Coverage index of each glyph, held for the span of glyph ids the
/// table covers.
#[derive(Debug)]
pub(crate) struct CoverageIndex {
    /// Glyph id of `slots[0]`.
    first: u16,
    /// Glyph count of the font: the flattened form answers below it.
    limit: u32,
    /// Coverage index plus one; 0 for a glyph that is not covered.
    slots: Box<[u16]>,
}

impl CoverageIndex {
    #[inline]
    pub(crate) fn get(&self, gid: u16) -> Covered {
        if gid as u32 >= self.limit {
            return Covered::Unknown;
        }
        let slot = gid
            .checked_sub(self.first)
            .and_then(|at| self.slots.get(at as usize));
        match slot {
            Some(&slot) if slot != 0 => Covered::At(slot - 1),
            _ => Covered::Miss,
        }
    }
}

/// Flatten a well-formed Coverage table for the glyph ids
/// `0..num_glyphs`. `None` when the table is not
/// [well-formed](coverage_is_well_formed) (a binary search over it would
/// not be a plain membership test) or an index does not fit.
fn flatten_coverage(cov: &[u8], num_glyphs: usize) -> Option<CoverageIndex> {
    flatten_coverage_within(cov, num_glyphs, usize::MAX)
}

/// [`flatten_coverage`] for a Coverage table whose covered glyphs sit
/// close together: `None` when the span of glyph ids from its first to
/// its last glyph is more than four times the number of glyphs it
/// covers (plus a small allowance), so the flattened form never costs
/// more than a few bytes per covered glyph.
fn flatten_dense_coverage(cov: &[u8], num_glyphs: usize) -> Option<CoverageIndex> {
    let format = read_u16(cov, 0).ok()?;
    let count = read_u16(cov, 2).ok()? as usize;
    let covered = match format {
        1 => count,
        2 => (0..count)
            .map(|i| {
                let at = 4 + i * 6;
                match (read_u16(cov, at), read_u16(cov, at + 2)) {
                    (Ok(start), Ok(end)) => (end as usize + 1).saturating_sub(start as usize),
                    _ => 0,
                }
            })
            .sum(),
        _ => return None,
    };
    flatten_coverage_within(
        cov,
        num_glyphs,
        covered.saturating_mul(4).saturating_add(64),
    )
}

/// [`flatten_coverage`], giving up (`None`) when the covered span of
/// glyph ids is longer than `max_span`.
fn flatten_coverage_within(
    cov: &[u8],
    num_glyphs: usize,
    max_span: usize,
) -> Option<CoverageIndex> {
    if !coverage_is_well_formed(cov) {
        return None;
    }
    let format = read_u16(cov, 0).ok()?;
    let count = read_u16(cov, 2).ok()? as usize;
    // Records are in increasing glyph order: the first and the last
    // bound the covered span, cut to the glyphs of the font.
    let span = match (format, count.checked_sub(1)) {
        (1, Some(last)) => Some((read_u16(cov, 4).ok()?, read_u16(cov, 4 + last * 2).ok()?)),
        (2, Some(last)) => Some((read_u16(cov, 4).ok()?, read_u16(cov, 6 + last * 6).ok()?)),
        _ => None,
    };
    let limit = num_glyphs.min(u32::MAX as usize) as u32;
    let Some((first, last)) = span.filter(|&(first, _)| (first as usize) < num_glyphs) else {
        return Some(CoverageIndex {
            first: 0,
            limit,
            slots: Box::new([]),
        });
    };
    let last = (last as usize).min(num_glyphs - 1);
    if last - first as usize >= max_span {
        return None;
    }
    let mut slots = vec![0u16; last - first as usize + 1];
    for i in 0..count {
        let (start, end, start_index) = match format {
            1 => {
                let glyph = read_u16(cov, 4 + i * 2).ok()?;
                (glyph, glyph, i)
            }
            _ => {
                let at = 4 + i * 6;
                (
                    read_u16(cov, at).ok()?,
                    read_u16(cov, at + 2).ok()?,
                    read_u16(cov, at + 4).ok()? as usize,
                )
            }
        };
        for glyph in start..=end {
            let Some(slot) = slots.get_mut((glyph - first) as usize) else {
                break;
            };
            *slot = u16::try_from(start_index + (glyph - start) as usize + 1).ok()?;
        }
    }
    Some(CoverageIndex {
        first,
        limit,
        slots: slots.into_boxed_slice(),
    })
}

/// The marks a lookup does not ignore when its flag carries a mark
/// filtering set ("skips over all mark glyphs not in the mark filtering
/// set indicated") or, failing that, a mark attachment class ("skips
/// over all marks not in the specified mark attachment class"): the
/// members of that set or class, as GDEF defines them.
///
/// `None` when the GDEF data is not flat-readable (out-of-order
/// records); the caller then asks GDEF glyph by glyph.
pub(crate) fn kept_marks(
    gdef: &crate::tables::gdef::GdefTable<'_>,
    flags: u16,
    mark_filtering_set: Option<u16>,
    num_glyphs: usize,
) -> Option<GlyphSet> {
    let mut kept = GlyphSet::with_capacity(num_glyphs);
    match mark_filtering_set {
        Some(set) => {
            // No such set: no mark is in it.
            if let Some(coverage) = gdef.mark_glyph_set_coverage(set) {
                if coverage.len() >= 4 && !coverage_is_well_formed(coverage) {
                    return None;
                }
                kept.insert_coverage(coverage);
            }
        }
        None => {
            let class = flags >> 8;
            // No MarkAttachClassDef: every mark is in class 0.
            if let Some(class_def) = gdef.mark_attach_class_def_bytes() {
                let classes = flatten_class_def(class_def, num_glyphs)?;
                for (glyph, _) in classes.iter().enumerate().filter(|&(_, &c)| c == class) {
                    kept.insert(glyph as u16);
                }
            }
        }
    }
    Some(kept)
}

/// Flatten a ClassDef table (§6.2.6) into one class per glyph for glyph
/// ids `0..num_glyphs`.
///
/// Returns `None` unless the table is one the per-glyph walker reads
/// unambiguously — every record readable and, for format 2, ranges in
/// strictly increasing, non-overlapping order — so the flat array and
/// the walker always agree.
pub(crate) fn flatten_class_def(class_def: &[u8], num_glyphs: usize) -> Option<Box<[u16]>> {
    let mut out = vec![0u16; num_glyphs];
    match read_u16(class_def, 0).ok()? {
        1 => {
            let start = read_u16(class_def, 2).ok()? as usize;
            let count = read_u16(class_def, 4).ok()? as usize;
            for i in 0..count {
                let class = read_u16(class_def, 6 + i * 2).ok()?;
                if let Some(slot) = out.get_mut(start + i) {
                    *slot = class;
                }
            }
        }
        2 => {
            let count = read_u16(class_def, 2).ok()? as usize;
            let mut next_free = 0u32;
            for i in 0..count {
                let off = 4 + i * 6;
                let start = read_u16(class_def, off).ok()?;
                let end = read_u16(class_def, off + 2).ok()?;
                let class = read_u16(class_def, off + 4).ok()?;
                if start > end || (start as u32) < next_free {
                    return None;
                }
                next_free = end as u32 + 1;
                let hi = (end as usize + 1).min(num_glyphs);
                if let Some(slots) = out.get_mut(start as usize..hi) {
                    slots.fill(class);
                }
            }
        }
        _ => return None,
    }
    Some(out.into_boxed_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_set_ranges_and_conservative_tail() {
        let mut s = GlyphSet::with_capacity(200);
        s.insert(3);
        s.insert_range(60, 70);
        s.insert_range(127, 129);
        s.insert_range(190, 400);
        for g in 0..200u16 {
            let want = g == 3 || (60..=70).contains(&g) || (127..=129).contains(&g) || g >= 190;
            assert_eq!(s.contains(g), want, "gid {g}");
        }
        // Beyond the bitmap capacity (rounded up to a word): conservative.
        assert!(s.contains(300));
        assert!(s.contains(u16::MAX));
        let one = |g: u16, set: &GlyphSet| Digest::of(g, set.shift);
        assert!(s.digest().intersects(&one(3, &s)));
        let mut union = GlyphSet::with_capacity(200);
        union.insert(5);
        union.union_with(&s);
        for g in 0..200u16 {
            assert_eq!(union.contains(g), s.contains(g) || g == 5, "gid {g}");
        }
        assert!(union.digest().intersects(&one(5, &union)));
        union.union_with(&GlyphSet::universal());
        assert!(union.contains(1) && union.digest().intersects(&Digest::of(1, 0)));
        assert!(GlyphSet::universal().contains(7));
        assert!(GlyphSet::universal().digest().intersects(&Digest::of(1, 9)));
        let mut small = GlyphSet::with_capacity(8192);
        assert_eq!(small.shift, 7);
        small.insert(130);
        small.insert_range(140, 142);
        let mut others = one(5, &small);
        others.add(one(99, &small));
        assert!(!small.digest().intersects(&others));
        assert!(small.digest().intersects(&one(141, &small)));
        // Same residue as 130 and the same block of 128, not a member:
        // the bitmap decides.
        assert!(small.digest().intersects(&one(194, &small)));
        assert!(!small.contains(194));
        // Same residue, a different block: the digest already says no.
        assert!(!small.digest().intersects(&one(130 + 1024, &small)));
        // Glyph ids past the last block share it.
        assert_eq!(Digest::of(u16::MAX, 2).blocks, 1u64 << 63);
        assert_eq!(block_shift(0), 0);
        assert_eq!(block_shift(64), 0);
        assert_eq!(block_shift(65), 1);
        assert_eq!(block_shift(65536), 10);
    }

    #[test]
    fn coverage_formats_fill_the_set() {
        // Format 1: glyphs 5, 9. Format 2: range 20..=22.
        let f1 = [0, 1, 0, 2, 0, 5, 0, 9];
        let f2 = [0, 2, 0, 1, 0, 20, 0, 22, 0, 0];
        let mut s = GlyphSet::with_capacity(64);
        s.insert_coverage(&f1);
        s.insert_coverage(&f2);
        let members: Vec<u16> = (0..64).filter(|&g| s.contains(g)).collect();
        assert_eq!(members, [5, 9, 20, 21, 22]);
    }

    #[test]
    fn coverage_flattening_matches_walker() {
        use crate::tables::gdef::coverage_lookup;
        let f1 = [0, 1, 0, 3, 0, 5, 0, 9, 0, 40];
        let f2 = [0, 2, 0, 2, 0, 3, 0, 5, 0, 7, 0, 20, 0, 60, 0, 0];
        for cov in [&f1[..], &f2[..]] {
            let flat = flatten_coverage(cov, 50).unwrap();
            for g in 0..50u16 {
                let expected = match coverage_lookup(cov, g) {
                    Some(index) => Covered::At(index),
                    None => Covered::Miss,
                };
                assert_eq!(flat.get(g), expected, "gid {g}");
            }
            // Past the font's glyphs the flattened form does not answer.
            assert_eq!(flat.get(50), Covered::Unknown);
            assert_eq!(flat.get(u16::MAX), Covered::Unknown);
            // Only the covered span is stored.
            assert!(flat.slots.len() < 50);
        }
        assert!(flatten_coverage(&[0, 1, 0, 2, 0, 9, 0, 5], 50).is_none());
    }

    #[test]
    fn coverage_well_formedness() {
        assert!(coverage_is_well_formed(&[0, 1, 0, 2, 0, 5, 0, 9]));
        // Unsorted glyph array.
        assert!(!coverage_is_well_formed(&[0, 1, 0, 2, 0, 9, 0, 5]));
        // Truncated glyph array.
        assert!(!coverage_is_well_formed(&[0, 1, 0, 3, 0, 5, 0, 9]));
        assert!(coverage_is_well_formed(&[
            0, 2, 0, 2, 0, 3, 0, 5, 0, 0, 0, 6, 0, 6, 0, 3
        ]));
        // Overlapping ranges.
        assert!(!coverage_is_well_formed(&[
            0, 2, 0, 2, 0, 3, 0, 5, 0, 0, 0, 5, 0, 6, 0, 3
        ]));
        assert!(!coverage_is_well_formed(&[0, 3, 0, 0]));
    }

    #[test]
    fn class_def_flattening_matches_walker() {
        use crate::tables::gdef::class_def_lookup;
        // Format 2: 2..=4 → 1, 7..=7 → 3, 9..=300 → 2.
        let cd = [
            0, 2, 0, 3, 0, 2, 0, 4, 0, 1, 0, 7, 0, 7, 0, 3, 0, 9, 1, 44, 0, 2,
        ];
        let flat = flatten_class_def(&cd, 40).unwrap();
        for g in 0..40u16 {
            assert_eq!(flat[g as usize], class_def_lookup(&cd, g).unwrap_or(0));
        }
        // Format 1: start 3, classes [1, 0, 3].
        let cd1 = [0, 1, 0, 3, 0, 3, 0, 1, 0, 0, 0, 3];
        let flat = flatten_class_def(&cd1, 10).unwrap();
        for g in 0..10u16 {
            assert_eq!(flat[g as usize], class_def_lookup(&cd1, g).unwrap_or(0));
        }
        // Overlapping ranges: refuse to flatten.
        let bad = [0, 2, 0, 2, 0, 2, 0, 9, 0, 1, 0, 5, 0, 12, 0, 2];
        assert!(flatten_class_def(&bad, 40).is_none());
    }
}
