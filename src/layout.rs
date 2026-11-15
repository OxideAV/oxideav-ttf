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
//! * **Lookup accelerators** — per lookup, the set of glyphs that appear
//!   in the *first-glyph* Coverage table of any of its sub-tables
//!   ([`GlyphSet`]), plus a 64-bit digest of that set. A lookup can only
//!   act at a position whose glyph is in that set (every sub-table
//!   format of every lookup type starts its match with a Coverage test
//!   on the current glyph — ISO/IEC 14496-22 §6.2 / GSUB / GPOS
//!   chapters), so a miss in the set skips the position, and a run whose
//!   digest does not intersect the lookup's digest skips the lookup.
//! * **Glyph classes** — the GDEF GlyphClassDef class of every glyph as
//!   a flat byte array, for the `lookupFlag` skip predicate.
//!
//! Every accelerator is a *filter in front of* the byte-walking lookup
//! code, never a replacement for its decisions: the sets are built as
//! supersets of what the walkers accept, and anything the builders
//! cannot read with confidence degrades to "no filtering".

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

/// Per-lookup accelerator. See the module docs.
#[derive(Debug)]
pub(crate) struct LookupAccel {
    /// Glyphs at which the lookup can act (first-glyph coverage union).
    pub(crate) first: GlyphSet,
    /// GSUB LigatureSubst only: the largest `componentCount` of any
    /// Ligature record in the lookup — the longest glyph sequence one
    /// match can inspect. `usize::MAX` when it could not be determined.
    pub(crate) max_components: usize,
}

impl LookupAccel {
    fn build(
        table: LayoutTable,
        bytes: &[u8],
        lookup_list_off: u32,
        lookup_index: u16,
        kind: u16,
        num_glyphs: usize,
    ) -> Self {
        let unfiltered = || LookupAccel {
            first: GlyphSet::universal(),
            max_components: usize::MAX,
        };
        let Some(layout) = table.coverage_layout(kind) else {
            return unfiltered();
        };
        let Some(lookup) = lookup_table_slice(bytes, lookup_list_off, lookup_index) else {
            return unfiltered();
        };
        let (Ok(outer_kind), Ok(sub_count)) = (read_u16(lookup, 0), read_u16(lookup, 4)) else {
            return unfiltered();
        };
        let mut first = GlyphSet::with_capacity(num_glyphs);
        let mut max_components = 0usize;
        let track_components = table == LayoutTable::Gsub && kind == 4;
        for s in 0..sub_count as usize {
            let Ok(sub_off) = read_u16(lookup, 6 + s * 2) else {
                break;
            };
            let Some(mut sub) = lookup.get(sub_off as usize..) else {
                continue;
            };
            let mut sub_kind = outer_kind;
            if outer_kind == table.extension_kind() {
                // Extension format 1: u16 format, u16 extensionLookupType,
                // Offset32 extensionOffset.
                let (Ok(ext_kind), Ok(ext_off)) = (read_u16(sub, 2), read_u32(sub, 4)) else {
                    continue;
                };
                let Some(ext) = sub.get(ext_off as usize..) else {
                    continue;
                };
                sub_kind = ext_kind;
                sub = ext;
            }
            if sub_kind != kind {
                continue;
            }
            match first_coverage(layout, sub) {
                FirstCoverage::At(cov) => first.insert_coverage(cov),
                FirstCoverage::None => {}
                FirstCoverage::Unknown => return unfiltered(),
            }
            if track_components {
                max_components = match ligature_max_components(sub) {
                    Some(n) => max_components.max(n),
                    None => usize::MAX,
                };
            }
        }
        LookupAccel {
            first,
            max_components: if track_components {
                max_components
            } else {
                usize::MAX
            },
        }
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

/// Largest `componentCount` over every Ligature record of a
/// LigatureSubstFormat1 sub-table, or `None` when a record could not be
/// read.
fn ligature_max_components(sub: &[u8]) -> Option<usize> {
    let set_count = read_u16(sub, 4).ok()? as usize;
    let mut max = 0usize;
    for s in 0..set_count {
        let set_off = read_u16(sub, 6 + s * 2).ok()? as usize;
        let set = sub.get(set_off..)?;
        let lig_count = read_u16(set, 0).ok()? as usize;
        for l in 0..lig_count {
            let lig_off = read_u16(set, 2 + l * 2).ok()? as usize;
            let lig = set.get(lig_off..)?;
            max = max.max(read_u16(lig, 2).ok()? as usize);
        }
    }
    Some(max)
}

/// Lazily built accelerators for every lookup of one layout table.
#[derive(Debug, Default)]
struct TableAccels {
    slots: OnceLock<Box<[OnceLock<LookupAccel>]>>,
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
        Some(slot.get_or_init(|| {
            LookupAccel::build(
                table,
                bytes,
                lookup_list_off,
                lookup_index,
                kind,
                num_glyphs,
            )
        }))
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
    glyph_classes: OnceLock<Option<Box<[u8]>>>,
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
    pub(crate) fn glyph_classes(&self, build: impl FnOnce() -> Option<Box<[u8]>>) -> Option<&[u8]> {
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

/// Flatten a ClassDef table (§6.2.6) into one class byte per glyph for
/// glyph ids `0..num_glyphs` (classes above 254 saturate to 255).
///
/// Returns `None` unless the table is one the per-glyph walker reads
/// unambiguously — every record readable and, for format 2, ranges in
/// strictly increasing, non-overlapping order — so the flat array and
/// the walker always agree.
pub(crate) fn flatten_class_def(class_def: &[u8], num_glyphs: usize) -> Option<Box<[u8]>> {
    let mut out = vec![0u8; num_glyphs];
    let clamp = |class: u16| class.min(255) as u8;
    match read_u16(class_def, 0).ok()? {
        1 => {
            let start = read_u16(class_def, 2).ok()? as usize;
            let count = read_u16(class_def, 4).ok()? as usize;
            for i in 0..count {
                let class = read_u16(class_def, 6 + i * 2).ok()?;
                if let Some(slot) = out.get_mut(start + i) {
                    *slot = clamp(class);
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
                    slots.fill(clamp(class));
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
    fn class_def_flattening_matches_walker() {
        use crate::tables::gdef::class_def_lookup;
        // Format 2: 2..=4 → 1, 7..=7 → 3, 9..=300 → 2.
        let cd = [
            0, 2, 0, 3, 0, 2, 0, 4, 0, 1, 0, 7, 0, 7, 0, 3, 0, 9, 1, 44, 0, 2,
        ];
        let flat = flatten_class_def(&cd, 40).unwrap();
        for g in 0..40u16 {
            assert_eq!(
                flat[g as usize] as u16,
                class_def_lookup(&cd, g).unwrap_or(0)
            );
        }
        // Format 1: start 3, classes [1, 0, 3].
        let cd1 = [0, 1, 0, 3, 0, 3, 0, 1, 0, 0, 0, 3];
        let flat = flatten_class_def(&cd1, 10).unwrap();
        for g in 0..10u16 {
            assert_eq!(
                flat[g as usize] as u16,
                class_def_lookup(&cd1, g).unwrap_or(0)
            );
        }
        // Overlapping ranges: refuse to flatten.
        let bad = [0, 2, 0, 2, 0, 2, 0, 9, 0, 1, 0, 5, 0, 12, 0, 2];
        assert!(flatten_class_def(&bad, 40).is_none());
    }
}
