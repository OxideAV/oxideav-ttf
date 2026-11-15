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
//!   table ([`GlyphSet`]), plus the union of those sets and a 64-bit
//!   digest of it. A lookup can only act at a position whose glyph is in
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

use crate::parser::read_u16;
use crate::parser::read_u32;
use crate::tables::gdef::lookup_table_slice;

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

/// A set of glyph ids as a bitmap over `0..capacity`, with a 64-bit
/// digest (bit `gid % 64` set for every member).
///
/// Membership is *conservative*: [`GlyphSet::contains`] answers `true`
/// for every member and for every glyph id beyond the bitmap's
/// capacity. A `false` answer is exact. The universal set ("no
/// filtering") is the zero-capacity bitmap with a saturated digest.
#[derive(Debug)]
pub(crate) struct GlyphSet {
    words: Box<[u64]>,
    digest: u64,
}

/// The digest bit of one glyph id.
#[inline]
pub(crate) fn digest_bit(gid: u16) -> u64 {
    1u64 << (gid & 63)
}

impl GlyphSet {
    fn with_capacity(glyphs: usize) -> Self {
        Self {
            words: vec![0u64; glyphs.div_ceil(64)].into_boxed_slice(),
            digest: 0,
        }
    }

    fn universal() -> Self {
        Self {
            words: Box::new([]),
            digest: u64::MAX,
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

    /// `true` when no glyph summarised by `run_digest` can be a member.
    #[inline]
    pub(crate) fn disjoint_from(&self, run_digest: u64) -> bool {
        self.digest & run_digest == 0
    }

    fn insert(&mut self, gid: u16) {
        self.digest |= digest_bit(gid);
        if let Some(w) = self.words.get_mut((gid >> 6) as usize) {
            *w |= 1u64 << (gid & 63);
        }
    }

    fn insert_range(&mut self, start: u16, end: u16) {
        if start > end {
            return;
        }
        if end - start >= 63 {
            self.digest = u64::MAX;
        } else {
            for g in start..=end {
                self.digest |= digest_bit(g);
            }
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
    pub(crate) coverage_index: Box<[u16]>,
    pub(crate) value_format1: u16,
    pub(crate) value_format2: u16,
}

impl PairSets {
    fn build(sub: &[u8], num_glyphs: usize) -> Option<Self> {
        if sub.len() < 8 || read_u16(sub, 0).ok()? != 1 {
            return None;
        }
        let coverage = sub.get(read_u16(sub, 2).ok()? as usize..)?;
        Some(Self {
            coverage_index: flatten_coverage(coverage, num_glyphs)?,
            value_format1: read_u16(sub, 4).ok()?,
            value_format2: read_u16(sub, 6).ok()?,
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
    pub(crate) value_format1: u16,
    /// Bytes of one `Class2Record` (both value records).
    pub(crate) class2_record_size: usize,
    /// Bytes of one `Class1Record` (`class2Count` Class2Records).
    pub(crate) class1_record_size: usize,
}

impl PairClasses {
    /// Offset, from the sub-table start, of the value record for a
    /// `(class1, class2)` pair. Header is 16 bytes.
    #[inline]
    pub(crate) fn record_offset(&self, class1: u16, class2: u16) -> usize {
        16 + class1 as usize * self.class1_record_size + class2 as usize * self.class2_record_size
    }

    fn build(sub: &[u8], num_glyphs: usize) -> Option<Self> {
        if sub.len() < 16 || read_u16(sub, 0).ok()? != 2 {
            return None;
        }
        let value_format1 = read_u16(sub, 4).ok()?;
        let value_format2 = read_u16(sub, 6).ok()?;
        let class_def1 = sub.get(read_u16(sub, 8).ok()? as usize..)?;
        let class_def2 = sub.get(read_u16(sub, 10).ok()? as usize..)?;
        let class2_count = read_u16(sub, 14).ok()? as usize;
        let class2_record_size =
            (value_format1.count_ones() + value_format2.count_ones()) as usize * 2;
        Some(Self {
            class1: flatten_class_def(class_def1, num_glyphs)?,
            class2: flatten_class_def(class_def2, num_glyphs)?,
            value_format1,
            class2_record_size,
            class1_record_size: class2_count * class2_record_size,
        })
    }
}

/// Per-lookup accelerator. See the module docs.
#[derive(Debug)]
pub(crate) struct LookupAccel {
    /// Glyphs at which the lookup can act (first-glyph coverage union).
    pub(crate) first: GlyphSet,
    /// The lookup's sub-tables of its effective type, in the order — and
    /// up to the point — the table walker visits them.
    pub(crate) subs: Box<[SubAccel]>,
    /// GSUB LigatureSubst only: the largest `componentCount` of any
    /// Ligature record in the lookup — the longest glyph sequence one
    /// match can inspect. `usize::MAX` when it could not be determined.
    pub(crate) max_components: usize,
    /// GSUB LigatureSubst only: every glyph that is the second component
    /// of some Ligature record, when every record of the lookup has at
    /// least two components and could be read. A ligature can then only
    /// form where the next visible glyph is in this set.
    pub(crate) second_components: Option<GlyphSet>,
}

/// How a table walker treats an Extension sub-table it cannot unwrap.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BadExtension {
    /// Skip the sub-table and go on to the next one.
    Skip,
    /// Give up on the lookup: later sub-tables are never visited.
    Stop,
}

impl LookupAccel {
    fn empty(num_glyphs: usize) -> Self {
        LookupAccel {
            first: GlyphSet::with_capacity(num_glyphs),
            subs: Box::new([]),
            max_components: 0,
            second_components: None,
        }
    }

    /// Resolve lookup `lookup_index` of effective type `kind` exactly
    /// the way the per-type table walkers in `tables::gsub` /
    /// `tables::gpos` iterate it.
    fn build(
        table: LayoutTable,
        bytes: &[u8],
        lookup_list_off: u32,
        lookup_index: u16,
        kind: u16,
        num_glyphs: usize,
    ) -> Option<Self> {
        let layout = table.coverage_layout(kind)?;
        // Walker preconditions: every walker but the GPOS pair /
        // mark-to-base / mark-to-mark ones treats a zero LookupList
        // offset as "no lookups"; all of them need a 6-byte Lookup
        // header.
        let zero_offset_is_empty = !(table == LayoutTable::Gpos && matches!(kind, 2 | 4 | 6));
        if zero_offset_is_empty && lookup_list_off == 0 {
            return Some(Self::empty(num_glyphs));
        }
        let Some(lookup) = lookup_table_slice(bytes, lookup_list_off, lookup_index) else {
            return Some(Self::empty(num_glyphs));
        };
        if lookup.len() < 6 {
            return Some(Self::empty(num_glyphs));
        }
        // `lookup` is a tail of `bytes`.
        let lookup_start = bytes.len() - lookup.len();
        let outer_kind = read_u16(lookup, 0).ok()?;
        let sub_count = read_u16(lookup, 4).ok()? as usize;
        // GPOS single / cursive / mark-to-ligature walkers abandon the
        // lookup at an Extension record they cannot unwrap; every other
        // walker moves on to the next sub-table.
        let bad_extension = if table == LayoutTable::Gpos && matches!(kind, 1 | 3 | 5) {
            BadExtension::Stop
        } else {
            BadExtension::Skip
        };
        let is_ligature = table == LayoutTable::Gsub && kind == 4;
        let is_pair = table == LayoutTable::Gpos && kind == 2;

        let mut first = GlyphSet::with_capacity(num_glyphs);
        let mut subs = Vec::new();
        let mut max_components = 0usize;
        let mut second = is_ligature.then(|| GlyphSet::with_capacity(num_glyphs));
        for s in 0..sub_count {
            // An unreadable or out-of-range sub-table offset ends every
            // walker's iteration.
            let Ok(sub_off) = read_u16(lookup, 6 + s * 2) else {
                break;
            };
            let Some(mut sub) = lookup.get(sub_off as usize..) else {
                break;
            };
            let mut sub_kind = outer_kind;
            if outer_kind == table.extension_kind() {
                // Extension format 1: u16 format, u16 extensionLookupType,
                // Offset32 extensionOffset.
                let target = if sub.len() < 8 {
                    None
                } else {
                    match (read_u16(sub, 2), read_u32(sub, 4)) {
                        (Ok(ext_kind), Ok(ext_off)) => {
                            sub.get(ext_off as usize..).map(|ext| (ext_kind, ext))
                        }
                        _ => None,
                    }
                };
                match (target, bad_extension) {
                    (Some((ext_kind, ext)), _) => {
                        sub_kind = ext_kind;
                        sub = ext;
                    }
                    (None, BadExtension::Skip) => continue,
                    (None, BadExtension::Stop) => break,
                }
            }
            if sub_kind != kind {
                continue;
            }
            let mut sub_first = GlyphSet::with_capacity(num_glyphs);
            let mut first_is_exact = false;
            match first_coverage(layout, sub) {
                FirstCoverage::At(cov) => {
                    sub_first.insert_coverage(cov);
                    first_is_exact = coverage_is_well_formed(cov);
                    first.insert_coverage(cov);
                }
                FirstCoverage::None => {}
                FirstCoverage::Unknown => {
                    sub_first = GlyphSet::universal();
                    first = GlyphSet::universal();
                }
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
            subs.push(SubAccel {
                // `sub` is a tail of `lookup`, itself a tail of `bytes`.
                offset: lookup_start + (lookup.len() - sub.len()),
                first: sub_first,
                first_is_exact,
                pair: match is_pair {
                    true => PairClasses::build(sub, num_glyphs)
                        .map(PairAccel::Classes)
                        .or_else(|| PairSets::build(sub, num_glyphs).map(PairAccel::Sets))
                        .unwrap_or(PairAccel::None),
                    false => PairAccel::None,
                },
            });
        }
        Some(LookupAccel {
            first,
            subs: subs.into_boxed_slice(),
            max_components: if is_ligature {
                max_components
            } else {
                usize::MAX
            },
            second_components: second,
        })
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

/// Lazily built accelerators for every lookup of one layout table.
#[derive(Debug, Default)]
struct TableAccels {
    slots: OnceLock<Box<[OnceLock<Option<LookupAccel>>]>>,
}

impl TableAccels {
    fn get(
        &self,
        table: LayoutTable,
        bytes: &[u8],
        lookup_list_off: u32,
        lookup_index: u16,
        kind: u16,
        num_glyphs: usize,
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
                kind,
                num_glyphs,
            )
        })
        .as_ref()
    }
}

/// One lookup of a shape plan.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlanLookup {
    pub(crate) index: u16,
    /// Effective lookup type (Extension wrappers unwrapped).
    pub(crate) kind: u16,
    pub(crate) flags: u16,
    pub(crate) mark_filtering_set: Option<u16>,
}

/// The lookups one shaping request runs, in application order.
#[derive(Debug, Default)]
pub(crate) struct ShapePlan {
    pub(crate) gsub: Vec<PlanLookup>,
    pub(crate) gpos: Vec<PlanLookup>,
}

#[derive(Debug)]
struct PlanEntry {
    script: [u8; 4],
    lang: Option<[u8; 4]>,
    features: Box<[[u8; 4]]>,
    plan: Arc<ShapePlan>,
}

/// Most distinct `(script, language, features)` requests kept per font.
const MAX_CACHED_PLANS: usize = 32;

/// All shaping accelerators of one font.
#[derive(Debug, Default)]
pub(crate) struct LayoutCache {
    gsub: TableAccels,
    gpos: TableAccels,
    /// GDEF GlyphClassDef class per glyph (`None` inside: no usable
    /// flat table could be built; callers fall back to the GDEF walker).
    glyph_classes: OnceLock<Option<Box<[u16]>>>,
    /// Most-recently-used first.
    plans: Mutex<Vec<PlanEntry>>,
}

impl LayoutCache {
    pub(crate) fn lookup_accel(
        &self,
        table: LayoutTable,
        bytes: &[u8],
        lookup_list_off: u32,
        lookup_index: u16,
        kind: u16,
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
            kind,
            num_glyphs,
        )
    }

    /// The flat GDEF class array, built on first use with `build`.
    pub(crate) fn glyph_classes(
        &self,
        build: impl FnOnce() -> Option<Box<[u16]>>,
    ) -> Option<&[u16]> {
        self.glyph_classes.get_or_init(build).as_deref()
    }

    /// The cached plan for a request, or the one `build` produces.
    pub(crate) fn plan(
        &self,
        script: [u8; 4],
        lang: Option<[u8; 4]>,
        features: &[[u8; 4]],
        build: impl FnOnce() -> ShapePlan,
    ) -> Arc<ShapePlan> {
        {
            let mut plans = self.plans.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(i) = plans
                .iter()
                .position(|e| e.script == script && e.lang == lang && *e.features == *features)
            {
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
                features: features.into(),
                plan: Arc::clone(&plan),
            },
        );
        plan
    }

    /// Drop every cached plan. Called when the variation instance
    /// changes: FeatureVariations can swap the lookups behind a feature.
    pub(crate) fn clear_plans(&mut self) {
        self.plans
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

/// Flatten a well-formed Coverage table (§6.2.5) into one entry per
/// glyph for glyph ids `0..num_glyphs`: Coverage index plus one, or 0
/// for a glyph the table does not list. `None` when the table is not
/// [well-formed](coverage_is_well_formed) (a binary search over it would
/// not be a plain membership test) or an index does not fit.
fn flatten_coverage(cov: &[u8], num_glyphs: usize) -> Option<Box<[u16]>> {
    if !coverage_is_well_formed(cov) {
        return None;
    }
    let mut out = vec![0u16; num_glyphs];
    let count = read_u16(cov, 2).ok()? as usize;
    match read_u16(cov, 0).ok()? {
        1 => {
            for i in 0..count {
                let glyph = read_u16(cov, 4 + i * 2).ok()? as usize;
                if let Some(slot) = out.get_mut(glyph) {
                    *slot = u16::try_from(i + 1).ok()?;
                }
            }
        }
        2 => {
            for i in 0..count {
                let off = 4 + i * 6;
                let start = read_u16(cov, off).ok()?;
                let end = read_u16(cov, off + 2).ok()?;
                let start_index = read_u16(cov, off + 4).ok()? as usize;
                for glyph in start..=end {
                    let Some(slot) = out.get_mut(glyph as usize) else {
                        break;
                    };
                    *slot = u16::try_from(start_index + (glyph - start) as usize + 1).ok()?;
                }
            }
        }
        _ => return None,
    }
    Some(out.into_boxed_slice())
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
        assert!(!s.disjoint_from(digest_bit(3)));
        assert!(GlyphSet::universal().contains(7));
        assert!(!GlyphSet::universal().disjoint_from(1));
        let mut small = GlyphSet::with_capacity(4096);
        small.insert(100);
        small.insert_range(110, 112);
        assert!(small.disjoint_from(digest_bit(5) | digest_bit(99)));
        assert!(!small.disjoint_from(digest_bit(111)));
        // Same digest bit as 100, not a member: the bitmap decides.
        assert!(!small.disjoint_from(digest_bit(164)));
        assert!(!small.contains(164));
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
                assert_eq!(
                    flat[g as usize].checked_sub(1),
                    coverage_lookup(cov, g),
                    "gid {g}"
                );
            }
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
