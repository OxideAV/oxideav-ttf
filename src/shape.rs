//! OpenType GSUB/GPOS shaping pipeline.
//!
//! This module wires the per-lookup-type GSUB substitution and GPOS
//! positioning primitives implemented in [`crate::tables::gsub`] and
//! [`crate::tables::gpos`] into a single coherent
//! [`Font::shape`](crate::Font::shape) entry point that turns a run of
//! Unicode text into a sequence of positioned glyphs.
//!
//! ## Pipeline (ISO/IEC 14496-22:2019 §6 "OFF Layout Common Table
//! Formats" + the GSUB/GPOS chapters)
//!
//! 1. **Character-to-glyph mapping.** Each input `char` is mapped to a
//!    nominal glyph id through the `cmap` table
//!    ([`Font::glyph_index`](crate::Font::glyph_index)). Characters with
//!    no mapping resolve to glyph 0 (`.notdef`).
//!
//! 2. **GSUB substitution stage.** The features the caller requested are
//!    resolved against the active script/language through the GSUB
//!    ScriptList → FeatureList → LangSys walk. Per the common-table-format
//!    rules, the *union* of the lookup indices referenced by the active
//!    features is gathered and processed **in LookupList order** (not
//!    feature order): "the client … processes the lookups referenced by
//!    these features in the order the lookup definitions occur in the
//!    LookupList … lookups from several different features may be
//!    interleaved during text processing." Each lookup is applied across
//!    the whole glyph buffer left-to-right (reverse-chaining LookupType 8
//!    is walked right-to-left).
//!
//! 3. **GPOS positioning stage.** Advances are seeded from `hmtx`. The
//!    active GPOS features' lookups are likewise gathered and applied in
//!    LookupList order, accumulating x/y placement and advance
//!    adjustments plus mark-attachment, cursive-attachment, and
//!    pair-kerning offsets onto each glyph.
//!
//! The result is a `Vec<`[`ShapedGlyph`]`>`: one entry per output glyph,
//! carrying the glyph id, the originating cluster (byte index into the
//! input text), and the placement/advance in font units (TT Y-up
//! convention, scale by `units_per_em` for a target ppem).
//!
//! ## Scope
//!
//! This is a *general* OpenType shaper: it applies whatever lookups the
//! requested features reference, for any script, without script-specific
//! reordering logic (the spec explicitly places complex-script glyph
//! reordering — e.g. Indic syllable reordering — outside its scope, in
//! the text-processing client). For scripts whose joining/positional
//! behaviour is fully expressed through GSUB/GPOS lookups keyed off
//! contextual rules (Latin ligatures and kerning, Arabic joining forms
//! driven by `init`/`medi`/`fina` + `mark`/`mkmk`/`curs`), the requested
//! feature set drives correct output directly.
//!
//! ## Cost model
//!
//! Lookup application is filtered, not brute-forced. The lookups behind
//! one `(script, language, features)` request are resolved once and
//! cached on the [`Font`] as a shape plan. Each lookup carries a
//! first-glyph set (the union of the Coverage tables its sub-tables test
//! against the current glyph) and a 64-bit digest of it: a run whose
//! glyph digest misses the lookup's digest skips the lookup outright,
//! and within a run only positions whose glyph is in the set reach the
//! table-walking match code. The `lookupFlag` skip predicate reads a
//! flat per-glyph GDEF class array. The filters only ever discard
//! positions the table-walking match code would reject, so the output
//! is identical to walking every lookup at every position —
//! [`Font::set_shaping_accelerators`](crate::Font::set_shaping_accelerators)
//! switches them off, and the test suite compares the two.

use std::sync::Arc;

use crate::Font;
use crate::layout::{
    GlyphSet, LayoutTable, LookupAccel, PairAccel, PlanLookup, ShapePlan, digest_bit,
    flatten_class_def,
};
use crate::tables::gdef::GdefTable;
use crate::tables::gpos::{self, GposTable, PosRecord};
use crate::tables::gsub::{self, EditExtent, GsubTable};

/// Maximum number of GSUB lookup passes over the buffer, as a guard
/// against a pathological self-growing lookup graph (a multiple- or
/// contextual-substitution chain that keeps expanding the buffer).
/// Real fonts converge in a handful of passes; this only bounds
/// adversarial inputs.
const MAX_GSUB_BUFFER_GROWTH: usize = 64;

/// One positioned glyph emitted by [`Font::shape`].
///
/// All four positioning fields are in font design units (the same units
/// as `head.unitsPerEm`), in the TrueType Y-up convention. To render at
/// a target pixel-per-em `ppem`, scale by `ppem / units_per_em`.
///
/// * `glyph_id` — the final glyph id after all GSUB substitutions.
/// * `cluster` — the byte offset into the original `&str` of the
///   character (or first character of the ligated group) this glyph
///   originated from. Stable across substitutions: a ligature inherits
///   the cluster of its first component; a multiple-substitution
///   expansion shares the source glyph's cluster across every output.
/// * `x_offset` / `y_offset` — placement adjustment applied to the pen
///   position *for drawing this glyph only* (does not move the pen).
///   Marks attach to bases through this field.
/// * `x_advance` / `y_advance` — how far the pen moves after drawing
///   this glyph. Seeded from the horizontal `hmtx` advance, then
///   adjusted by GPOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ShapedGlyph {
    pub glyph_id: u16,
    pub cluster: u32,
    pub x_offset: i32,
    pub y_offset: i32,
    pub x_advance: i32,
    pub y_advance: i32,
}

/// Internal working item during the GSUB stage. The position fields are
/// not populated until the GPOS stage; we carry the glyph id + cluster
/// here and materialise [`ShapedGlyph`] at the boundary.
#[derive(Debug, Clone, Copy)]
struct WorkGlyph {
    gid: u16,
    cluster: u32,
}

/// The `lookupFlag` skip predicate (§6.2 LookupFlag bit enumeration) of
/// one lookup, bound to a font. Answers exactly what
/// [`Font::lookup_skips_glyph`] answers, reading the flat GDEF class
/// array where it can.
struct SkipFilter<'f, 'a> {
    font: &'f Font<'a>,
    gdef: Option<&'f GdefTable<'a>>,
    classes: Option<&'f [u16]>,
    flags: u16,
    mark_filtering_set: Option<u16>,
    /// `false` when the lookup can never skip a glyph (no skip bit, no
    /// mark-attachment class, no filtering set, or no GDEF at all).
    active: bool,
    /// Marks need the GDEF walker: a mark-attachment class or a mark
    /// filtering set qualifies which marks are skipped.
    marks_need_walker: bool,
}

impl<'f, 'a> SkipFilter<'f, 'a> {
    fn new(font: &'f Font<'a>, flags: u16, mark_filtering_set: Option<u16>) -> Self {
        let gdef = font.gdef();
        let active = gdef.is_some() && (flags & 0xFF1E != 0 || mark_filtering_set.is_some());
        Self {
            font,
            gdef,
            // The flat class array is only consulted when a skip bit is
            // set (every use below is behind `active` or IGNORE_MARKS).
            classes: if active { font.glyph_classes() } else { None },
            flags,
            mark_filtering_set,
            active,
            marks_need_walker: flags & 0xFF00 != 0 || mark_filtering_set.is_some(),
        }
    }

    #[inline]
    fn skips(&self, gid: u16) -> bool {
        if !self.active {
            return false;
        }
        let Some(&class) = self.classes.and_then(|c| c.get(gid as usize)) else {
            return self
                .font
                .lookup_skips_glyph(self.flags, self.mark_filtering_set, gid);
        };
        match class {
            crate::tables::gdef::CLASS_BASE => self.flags & 0x0002 != 0,
            crate::tables::gdef::CLASS_LIGATURE => self.flags & 0x0004 != 0,
            crate::tables::gdef::CLASS_MARK => {
                self.flags & 0x0008 != 0
                    || (self.marks_need_walker
                        && self
                            .font
                            .lookup_skips_glyph(self.flags, self.mark_filtering_set, gid))
            }
            _ => false,
        }
    }

    /// `true` when the font has a GDEF table that classes `gid` as a
    /// mark.
    #[inline]
    fn is_gdef_mark(&self, gid: u16) -> bool {
        match self.classes.and_then(|c| c.get(gid as usize)) {
            Some(&class) => class == crate::tables::gdef::CLASS_MARK,
            None => self.gdef.is_some_and(|g| g.is_mark(gid)),
        }
    }
}

/// One planned lookup bound to its accelerator: the first-glyph filter
/// plus the resolved sub-tables. `accel == None` means "no accelerator —
/// every glyph may match, go through the table walker".
#[derive(Clone, Copy)]
struct Resolved<'f> {
    accel: Option<&'f LookupAccel>,
    /// The whole GSUB / GPOS table; sub-table offsets index into it.
    table: &'f [u8],
    index: u16,
}

impl<'f> Resolved<'f> {
    /// Bind a planned lookup to the bytes of its layout table.
    #[inline]
    fn new(lookup: &'f PlanLookup, table: &'f [u8]) -> Self {
        Resolved {
            accel: lookup.accel.as_deref(),
            table,
            index: lookup.index,
        }
    }

    /// `false` only when the lookup provably cannot act at `gid`.
    #[inline]
    fn may_start(&self, gid: u16) -> bool {
        self.accel.is_none_or(|a| a.first.contains(gid))
    }

    /// `true` when no glyph summarised by `run_digest` can start a match.
    #[inline]
    fn rules_out(&self, run_digest: u64) -> bool {
        self.accel
            .is_some_and(|a| a.first.disjoint_from(run_digest))
    }

    /// Longest glyph sequence one ligature match can inspect.
    fn max_components(&self) -> usize {
        self.accel.map_or(usize::MAX, |a| a.max_components)
    }

    /// Glyphs that can be the second component of a ligature, when every
    /// ligature of the lookup is known to have one.
    fn second_components(&self) -> Option<&'f GlyphSet> {
        self.accel.and_then(|a| a.second_components.as_ref())
    }

    /// The sub-tables (in walker order) whose first-glyph Coverage may
    /// list `gid`, as byte slices starting at each sub-table.
    #[inline]
    fn subtables_for(
        accel: &'f LookupAccel,
        table: &'f [u8],
        gid: u16,
    ) -> impl Iterator<Item = &'f [u8]> {
        accel
            .subs
            .iter()
            .filter(move |s| s.first.contains(gid))
            .map(move |s| table.get(s.offset..).unwrap_or(&[]))
    }

    /// Run `matcher` on each sub-table (in walker order) whose
    /// first-glyph Coverage may list `gid`, until one answers. The
    /// matcher gets the sub-table bytes and, where the sub-table's
    /// Coverage table is flattened, the Coverage index of `gid`
    /// (`Some`: `gid` is covered at that index; `None`: not flattened —
    /// search the Coverage table). A sub-table whose flattened Coverage
    /// does not list `gid` is passed over.
    #[inline]
    fn first_hit<T>(
        &self,
        accel: &'f LookupAccel,
        gid: u16,
        mut matcher: impl FnMut(&'f [u8], Option<u16>) -> Option<T>,
    ) -> Option<T> {
        for sub in accel.subs.iter().filter(|s| s.first.contains(gid)) {
            let bytes = self.table.get(sub.offset..).unwrap_or(&[]);
            let covered = match sub.coverage_index.as_deref().map(|ci| ci.get(gid as usize)) {
                // Flattened and not covered.
                Some(Some(0)) => continue,
                Some(Some(&index)) => Some(index - 1),
                // Not flattened, or a glyph id beyond the flattened range.
                Some(None) | None => None,
            };
            if let Some(hit) = matcher(bytes, covered) {
                return Some(hit);
            }
        }
        None
    }

    // ---- GSUB: first matching sub-table wins -------------------------

    fn single(&self, gsub: &GsubTable<'_>, gid: u16) -> Option<u16> {
        match self.accel {
            Some(a) => self.first_hit(a, gid, |sub, covered| match covered {
                Some(index) => gsub::single_subst_covered(sub, gid, index),
                None => gsub::single_subst_lookup(sub, gid),
            }),
            None => gsub.apply_lookup_type_1(self.index, gid),
        }
    }

    fn multiple(&self, gsub: &GsubTable<'_>, gid: u16) -> Option<Vec<u16>> {
        match self.accel {
            Some(a) => Self::subtables_for(a, self.table, gid)
                .find_map(|sub| gsub::multiple_subst_lookup(sub, gid)),
            None => gsub.apply_lookup_type_2(self.index, gid),
        }
    }

    fn alternate(&self, gsub: &GsubTable<'_>, gid: u16) -> Option<u16> {
        match self.accel {
            Some(a) => self.first_hit(a, gid, |sub, covered| match covered {
                Some(index) => gsub::alternate_subst_covered(sub, index, 0),
                None => gsub::alternate_subst_lookup(sub, gid, 0),
            }),
            None => gsub.apply_lookup_type_3(self.index, gid, 0),
        }
    }

    /// `glyphs` must be non-empty.
    fn ligature(&self, gsub: &GsubTable<'_>, glyphs: &[u16]) -> Option<(u16, usize)> {
        match self.accel {
            Some(a) => self.first_hit(a, glyphs[0], |sub, covered| match covered {
                Some(index) => gsub::ligature_subst_covered(sub, index, glyphs),
                None => gsub::ligature_subst_lookup(sub, glyphs),
            }),
            None => gsub.apply_lookup_type_4(self.index, glyphs),
        }
    }

    /// Contextual / chained-context substitution at `pos` (in range of
    /// `gids`): on a match, `gids` is rewritten in place and `extent`
    /// covers the rewritten region. Returns whether a rule matched.
    fn substitute_in_context(
        &self,
        gsub: &GsubTable<'_>,
        chained: bool,
        gids: &mut Vec<u16>,
        pos: usize,
        extent: &mut EditExtent,
    ) -> bool {
        match self.accel {
            Some(a) => {
                let table = self.table;
                let glyph = gids[pos];
                a.subs.iter().filter(|s| s.first.contains(glyph)).any(|s| {
                    let sub = table.get(s.offset..).unwrap_or(&[]);
                    gsub.apply_context_subtable_in_place(chained, sub, gids, pos, extent)
                })
            }
            None => gsub.apply_context_lookup_in_place(chained, self.index, gids, pos, extent),
        }
    }

    /// `pos` must be in range of `gids`.
    fn reverse_chain(&self, gsub: &GsubTable<'_>, gids: &[u16], pos: usize) -> Option<u16> {
        match self.accel {
            Some(a) => Self::subtables_for(a, self.table, gids[pos])
                .find_map(|sub| gsub::reverse_chain_context_lookup(sub, gids, pos)),
            None => gsub.apply_lookup_type_8(self.index, gids, pos),
        }
    }

    // ---- GPOS ----------------------------------------------------------

    fn single_pos(&self, gpos: &GposTable<'_>, gid: u16) -> Option<gpos::PosValue> {
        match self.accel {
            Some(a) => Self::subtables_for(a, self.table, gid)
                .find_map(|sub| gpos::single_pos_subtable(sub, gid)),
            None => gpos.apply_lookup_type_1(self.index, gid),
        }
    }

    /// Pair adjustment: the x-advance change of `left` when followed by
    /// `right`. Mirrors `GposTable::lookup_kerning_at`: an IGNORE_MARKS
    /// lookup never kerns a pair with a (GDEF) mark member.
    fn pair(&self, gpos: &GposTable<'_>, skip: &SkipFilter<'_, '_>, left: u16, right: u16) -> i16 {
        let Some(accel) = self.accel else {
            return gpos.lookup_kerning_at(self.index, left, right, skip.gdef);
        };
        // The adjustment is a pure function of the pair for this lookup.
        let pair_key = (left as u32) << 16 | right as u32;
        accel
            .pair_memo
            .get_or_compute(pair_key, || {
                Some(self.pair_uncached(accel, skip, left, right) as u16)
            })
            .unwrap_or(0) as i16
    }

    fn pair_uncached(
        &self,
        accel: &LookupAccel,
        skip: &SkipFilter<'_, '_>,
        left: u16,
        right: u16,
    ) -> i16 {
        if skip.flags & 0x0008 != 0 && (skip.is_gdef_mark(left) || skip.is_gdef_mark(right)) {
            return 0;
        }
        for sub in accel.subs.iter().filter(|s| s.first.contains(left)) {
            let bytes = self.table.get(sub.offset..).unwrap_or(&[]);
            // `first` being exact means `left` is covered, which is all
            // the flattened forms need to know before addressing the
            // pair's value record directly.
            let glyphs = (left as usize, right as usize);
            let hit = match &sub.pair {
                PairAccel::Classes(classes) if sub.first_is_exact => {
                    match (classes.class1.get(glyphs.0), classes.class2.get(glyphs.1)) {
                        // An empty first value record carries no
                        // adjustment: the sub-table does not apply.
                        (Some(_), Some(_)) if classes.value_format1 == 0 => None,
                        (Some(&c1), Some(&c2)) => Some(gpos::pair_class_x_advance(
                            bytes,
                            classes.record_offset(c1, c2),
                            classes.value_format1,
                        )),
                        _ => gpos::pair_pos_subtable(bytes, left, right),
                    }
                }
                PairAccel::Sets(sets) => match sets.coverage_index.get(glyphs.0) {
                    Some(0) => None,
                    Some(&index) => gpos::pair_set_x_advance(
                        bytes,
                        index - 1,
                        right,
                        sets.value_format1,
                        sets.value_format2,
                    ),
                    None => gpos::pair_pos_subtable(bytes, left, right),
                },
                _ => gpos::pair_pos_subtable(bytes, left, right),
            };
            if let Some(adjust) = hit {
                return adjust;
            }
        }
        0
    }

    fn cursive(&self, gpos: &GposTable<'_>, gid: u16) -> Option<gpos::CursiveAttachment> {
        match self.accel {
            Some(a) => Self::subtables_for(a, self.table, gid)
                .find_map(|sub| gpos::cursive_pos_subtable(sub, gid)),
            None => gpos.apply_lookup_type_3(self.index, gid),
        }
    }

    /// Mark attachment of lookup type `kind` (4 mark-to-base, 5
    /// mark-to-ligature component 0, 6 mark-to-mark): the offset placing
    /// `mark` on `target`.
    fn mark_attachment(
        &self,
        gpos: &GposTable<'_>,
        kind: u16,
        target: u16,
        mark: u16,
    ) -> Option<(i16, i16)> {
        match (self.accel, kind) {
            (Some(a), 4) => Self::subtables_for(a, self.table, mark)
                .find_map(|sub| gpos::mark_base_pos_subtable(sub, target, mark)),
            (Some(a), 5) => Self::subtables_for(a, self.table, mark)
                .find_map(|sub| gpos::mark_ligature_pos_subtable(sub, target, 0, mark)),
            (Some(a), _) => Self::subtables_for(a, self.table, mark)
                .find_map(|sub| gpos::mark_mark_pos_subtable(sub, target, mark)),
            (None, 4) => gpos.apply_mark_to_base_at(self.index, target, mark),
            (None, 5) => gpos.apply_lookup_type_5(self.index, target, 0, mark),
            (None, _) => gpos.apply_mark_to_mark_at(self.index, target, mark),
        }
    }

    /// `pos` must be in range of `gids`.
    fn position_in_context(
        &self,
        gpos: &GposTable<'_>,
        chained: bool,
        gids: &[u16],
        pos: usize,
    ) -> Option<Vec<PosRecord>> {
        match self.accel {
            Some(a) => Self::subtables_for(a, self.table, gids[pos])
                .find_map(|sub| gpos.apply_context_subtable(chained, sub, gids, pos)),
            None if chained => gpos.apply_lookup_type_8(self.index, gids, pos),
            None => gpos.apply_lookup_type_7(self.index, gids, pos),
        }
    }
}

/// Which glyph ids a run holds, for ruling whole lookups out. Always a
/// superset: a glyph substituted away stays listed (that only costs a
/// missed shortcut), a glyph written into the run is added.
struct RunGlyphs<'s> {
    /// OR of [`digest_bit`] over the run.
    digest: u64,
    /// Each distinct glyph id once. Text repeats glyphs heavily, so
    /// probing a lookup's first-glyph set with this list is far cheaper
    /// than probing it at every position of a long run.
    distinct: &'s mut Vec<u16>,
    /// Membership bitmap of `distinct` over the whole glyph id space.
    seen: &'s mut [u64],
}

/// Words in a bitmap over every glyph id.
const GLYPH_BITMAP_WORDS: usize = 1 << 10;

/// Working storage of one shaping call, recycled per thread so a call
/// allocates nothing but its result (see [`ShapeScratch::take`]).
#[derive(Default)]
struct ShapeScratch {
    /// The glyph buffer.
    buf: Vec<WorkGlyph>,
    /// [`RunGlyphs::distinct`].
    distinct: Vec<u16>,
    /// [`RunGlyphs::seen`]: `GLYPH_BITMAP_WORDS` words, all zero between
    /// calls.
    seen: Vec<u64>,
    lookups: LookupScratch,
}

/// Scratch vectors individual lookups reuse.
#[derive(Default)]
struct LookupScratch {
    /// Glyph-id mirror of the buffer (contextual lookups).
    gids: Vec<u16>,
    /// Visible glyphs of a ligature candidate run, and their buffer
    /// indices.
    cand_gids: Vec<u16>,
    cand_idx: Vec<usize>,
}

thread_local! {
    static SHAPE_SCRATCH: std::cell::Cell<ShapeScratch> = const {
        std::cell::Cell::new(ShapeScratch {
            buf: Vec::new(),
            distinct: Vec::new(),
            seen: Vec::new(),
            lookups: LookupScratch {
                gids: Vec::new(),
                cand_gids: Vec::new(),
                cand_idx: Vec::new(),
            },
        })
    };
}

/// Largest glyph buffer (in glyphs) kept for the thread's next call; a
/// longer one is released rather than pinned.
const SCRATCH_KEEP_GLYPHS: usize = 1 << 14;

impl ShapeScratch {
    /// This thread's recycled scratch (or a fresh one: first call on the
    /// thread, a nested call, or thread teardown).
    fn take() -> Self {
        let mut scratch = SHAPE_SCRATCH
            .try_with(|cell| cell.take())
            .unwrap_or_default();
        if scratch.seen.len() != GLYPH_BITMAP_WORDS {
            scratch.seen = vec![0u64; GLYPH_BITMAP_WORDS];
        }
        scratch
    }

    /// Hand the scratch back for the thread's next call.
    fn recycle(mut self) {
        if self.buf.capacity() > SCRATCH_KEEP_GLYPHS {
            return;
        }
        // Restore the all-zero bitmap by clearing only the touched words.
        for &gid in &self.distinct {
            self.seen[(gid >> 6) as usize] = 0;
        }
        self.distinct.clear();
        self.buf.clear();
        let _ = SHAPE_SCRATCH.try_with(|cell| cell.set(self));
    }
}

impl<'s> RunGlyphs<'s> {
    /// Summarise `buf`. `distinct` must be empty and `seen` all zero.
    fn of(buf: &[WorkGlyph], distinct: &'s mut Vec<u16>, seen: &'s mut [u64]) -> Self {
        let mut run = RunGlyphs {
            digest: 0,
            distinct,
            seen,
        };
        for w in buf {
            run.add(w.gid);
        }
        run
    }

    #[inline]
    fn add(&mut self, gid: u16) {
        let bit = digest_bit(gid);
        let word = &mut self.seen[(gid >> 6) as usize];
        if *word & bit == 0 {
            *word |= bit;
            self.distinct.push(gid);
            self.digest |= bit;
        }
    }

    /// `true` when `lookup` provably cannot act anywhere in the run.
    #[inline]
    fn rules_out(&self, lookup: &Resolved<'_>) -> bool {
        lookup.rules_out(self.digest) || !self.distinct.iter().any(|&g| lookup.may_start(g))
    }
}

impl<'a> Font<'a> {
    /// Shape a run of text into positioned glyphs under `script` /
    /// `lang`, applying the listed `features`.
    ///
    /// `script` and `lang` are OpenType tags (`*b"latn"`, `*b"arab"`,
    /// `*b"DFLT"`; `lang = None` selects the script's default language
    /// system). `features` is the ordered list of feature tags the
    /// caller wants enabled (e.g. `[*b"ccmp", *b"liga", *b"kern"]`); a
    /// feature tag the font does not list under the active script is
    /// silently ignored. The relative order of `features` does not by
    /// itself dictate application order — the GSUB/GPOS lookups behind
    /// the *union* of requested features run in LookupList order, per the
    /// OpenType common-table-format rules — but it determines which
    /// features are active.
    ///
    /// Returns one [`ShapedGlyph`] per output glyph. For a font with no
    /// GSUB/GPOS, this degenerates to nominal cmap mapping with `hmtx`
    /// advances (i.e. unshaped glyph runs still come back correctly
    /// positioned for simple scripts).
    ///
    /// The variation-instance-aware feature resolution
    /// ([`Font::gsub_features_for_script_at_instance`]) is used, so a
    /// variable font shaped after [`Font::set_variation_coords`] honours
    /// its FeatureVariations substitutions.
    pub fn shape(
        &self,
        text: &str,
        script: [u8; 4],
        lang: Option<[u8; 4]>,
        features: &[[u8; 4]],
    ) -> Vec<ShapedGlyph> {
        let mut scratch = ShapeScratch::take();
        let shaped = self.shape_in(&mut scratch, text, script, lang, features);
        scratch.recycle();
        shaped
    }

    fn shape_in(
        &self,
        scratch: &mut ShapeScratch,
        text: &str,
        script: [u8; 4],
        lang: Option<[u8; 4]>,
        features: &[[u8; 4]],
    ) -> Vec<ShapedGlyph> {
        // --- 1. character-to-glyph mapping --------------------------------
        let buf = &mut scratch.buf;
        for (byte_idx, ch) in text.char_indices() {
            let gid = self.glyph_index(ch).unwrap_or(0);
            buf.push(WorkGlyph {
                gid,
                cluster: byte_idx as u32,
            });
        }

        let plan = self.shape_plan(script, lang, features);
        if plan.gsub.is_empty() && plan.gpos.is_empty() {
            return self.seed_advances(buf);
        }
        let mut run = RunGlyphs::of(buf, &mut scratch.distinct, &mut scratch.seen);

        // --- 2. GSUB substitution stage -----------------------------------
        if !plan.gsub.is_empty() {
            self.run_gsub(buf, &plan.gsub, &mut run, &mut scratch.lookups);
        }

        // --- 3. GPOS positioning stage ------------------------------------
        self.run_gpos(buf, &plan.gpos, &run, &mut scratch.lookups.gids)
    }

    /// The (cached) shape plan for a request: the GSUB and GPOS lookups
    /// behind the union of the requested features, in LookupList order.
    fn shape_plan(
        &self,
        script: [u8; 4],
        lang: Option<[u8; 4]>,
        features: &[[u8; 4]],
    ) -> Arc<ShapePlan> {
        self.layout.plan(script, lang, features, || ShapePlan {
            gsub: self.plan_gsub(script, lang, features),
            gpos: self.plan_gpos(script, lang, features),
        })
    }

    /// Resolve the active GSUB lookups for the requested features.
    fn plan_gsub(
        &self,
        script: [u8; 4],
        lang: Option<[u8; 4]>,
        features: &[[u8; 4]],
    ) -> Vec<PlanLookup> {
        let Some(gsub) = self.gsub() else {
            return Vec::new();
        };
        let resolved = self.gsub_features_for_script_at_instance(script, lang);
        // Gather the union of lookup indices referenced by every active
        // requested feature.
        let mut active: Vec<u16> = Vec::new();
        for feat in &resolved {
            if features.contains(&feat.tag) {
                active.extend_from_slice(&feat.lookup_indices);
            }
        }
        // Process in LookupList order, not feature order.
        active.sort_unstable();
        active.dedup();
        // Map each active lookup index to its (effective) type so the
        // right per-type apply path is picked.
        let types: Vec<(u16, u16, u16)> = gsub.lookup_list().collect();
        active
            .into_iter()
            .map(|li| (li, lookup_kind(&types, li)))
            // Drop types the substitution stage has no apply path for.
            .filter(|&(_, kind)| matches!(kind, 1..=6 | 8))
            .map(|(li, kind)| PlanLookup {
                index: li,
                kind,
                flags: gsub.lookup_flags(li),
                mark_filtering_set: gsub.mark_filtering_set(li),
                accel: self.lookup_accel(LayoutTable::Gsub, gsub.lookup_list_location(), li, kind),
            })
            .collect()
    }

    /// Resolve the active GPOS lookups for the requested features.
    fn plan_gpos(
        &self,
        script: [u8; 4],
        lang: Option<[u8; 4]>,
        features: &[[u8; 4]],
    ) -> Vec<PlanLookup> {
        let Some(gpos) = self.gpos() else {
            return Vec::new();
        };
        let resolved = self.gpos_features_for_script_at_instance(script, lang);
        let mut active: Vec<u16> = Vec::new();
        for feat in &resolved {
            if features.contains(&feat.tag) {
                active.extend_from_slice(&feat.lookup_indices);
            }
        }
        active.sort_unstable();
        active.dedup();
        let types: Vec<(u16, u16, u16)> = gpos.lookup_list().collect();
        active
            .into_iter()
            .map(|li| (li, lookup_kind(&types, li)))
            .filter(|&(_, kind)| matches!(kind, 1..=8))
            .map(|(li, kind)| PlanLookup {
                index: li,
                kind,
                flags: gpos.lookup_flags(li),
                mark_filtering_set: gpos.mark_filtering_set(li),
                accel: self.lookup_accel(LayoutTable::Gpos, gpos.lookup_list_location(), li, kind),
            })
            .collect()
    }

    /// Flat GDEF GlyphClassDef classes (one entry per glyph), or `None`
    /// when the font has no GDEF or its class table is not one the flat
    /// form reproduces exactly.
    fn glyph_classes(&self) -> Option<&[u16]> {
        self.layout.glyph_classes(|| {
            let gdef = self.gdef()?;
            let num_glyphs = self.maxp.num_glyphs as usize;
            match gdef.glyph_class_def_bytes() {
                Some(class_def) => flatten_class_def(class_def, num_glyphs),
                // No GlyphClassDef: every glyph is class 0.
                None => Some(vec![0u16; num_glyphs].into_boxed_slice()),
            }
        })
    }

    /// The (shared, built-once) accelerator of one lookup.
    fn lookup_accel(
        &self,
        table: LayoutTable,
        location: (&[u8], u32),
        index: u16,
        kind: u16,
    ) -> Option<Arc<LookupAccel>> {
        let (bytes, lookup_list_off) = location;
        self.layout.lookup_accel(
            table,
            bytes,
            lookup_list_off,
            index,
            kind,
            self.maxp.num_glyphs as usize,
        )
    }

    /// One output glyph per buffer glyph, each with its nominal
    /// horizontal advance from `hmtx` and no placement offset.
    fn seed_advances(&self, buf: &[WorkGlyph]) -> Vec<ShapedGlyph> {
        buf.iter()
            .map(|w| ShapedGlyph {
                glyph_id: w.gid,
                cluster: w.cluster,
                x_offset: 0,
                y_offset: 0,
                x_advance: self.glyph_advance(w.gid) as i32,
                y_advance: 0,
            })
            .collect()
    }

    /// Apply the planned GSUB lookups, in LookupList order, across `buf`.
    fn run_gsub(
        &self,
        buf: &mut Vec<WorkGlyph>,
        lookups: &[PlanLookup],
        run: &mut RunGlyphs<'_>,
        scratch: &mut LookupScratch,
    ) {
        let Some(gsub) = self.gsub() else {
            return;
        };
        let (table, _) = gsub.lookup_list_location();
        for lookup in lookups {
            let resolved = Resolved::new(lookup, table);
            if run.rules_out(&resolved) {
                continue;
            }
            self.apply_gsub_lookup(gsub, buf, lookup, resolved, run, scratch);
        }
    }

    /// Apply one GSUB lookup across the whole buffer. The lookup's
    /// IGNORE_MARKS / IGNORE_BASE_GLYPHS / IGNORE_LIGATURES skip bits
    /// are honoured where they affect substitution (most consequentially
    /// IGNORE_MARKS on ligature lookups, so a combining mark sitting
    /// between two ligature components doesn't block the ligature).
    ///
    /// `resolved` prunes positions whose glyph the lookup's first-glyph
    /// Coverage tables do not list; the match code would reject those
    /// anyway. `run` learns every glyph written.
    fn apply_gsub_lookup(
        &self,
        gsub: &GsubTable<'_>,
        buf: &mut Vec<WorkGlyph>,
        lookup: &PlanLookup,
        resolved: Resolved<'_>,
        run: &mut RunGlyphs<'_>,
        scratch: &mut LookupScratch,
    ) {
        match lookup.kind {
            1 => {
                // Single substitution: 1:1, no length change.
                for w in buf.iter_mut() {
                    if resolved.may_start(w.gid)
                        && let Some(g) = resolved.single(gsub, w.gid)
                    {
                        w.gid = g;
                        run.add(g);
                    }
                }
            }
            2 => {
                // Multiple substitution: 1 → N (or 0 = deletion). All
                // outputs inherit the source cluster.
                if !buf.iter().any(|w| resolved.may_start(w.gid)) {
                    return;
                }
                let mut out: Vec<WorkGlyph> = Vec::with_capacity(buf.len());
                let mut growth = 0usize;
                for w in buf.iter() {
                    let expansion = if resolved.may_start(w.gid) {
                        resolved.multiple(gsub, w.gid)
                    } else {
                        None
                    };
                    match expansion {
                        Some(seq) => {
                            growth += seq.len();
                            for g in seq {
                                out.push(WorkGlyph {
                                    gid: g,
                                    cluster: w.cluster,
                                });
                            }
                        }
                        None => out.push(*w),
                    }
                    if growth > buf.len() + MAX_GSUB_BUFFER_GROWTH {
                        // Pathological expansion guard: keep the rest
                        // unsubstituted.
                        break;
                    }
                }
                if growth <= buf.len() + MAX_GSUB_BUFFER_GROWTH {
                    *buf = out;
                    for w in buf.iter() {
                        run.add(w.gid);
                    }
                }
            }
            3 => {
                // Alternate substitution: default to alternate 0.
                for w in buf.iter_mut() {
                    if resolved.may_start(w.gid)
                        && let Some(g) = resolved.alternate(gsub, w.gid)
                    {
                        w.gid = g;
                        run.add(g);
                    }
                }
            }
            4 => {
                // Ligature substitution: N → 1, consuming a prefix from
                // each position. The ligature inherits the cluster of its
                // first component. The lookup's skip filter (§2) decides
                // which glyphs are invisible to the match: a lookup with
                // IGNORE_MARKS matches over the *non-mark* glyphs and
                // removes only the consumed visible components, leaving
                // interspersed marks in place (they re-anchor to the
                // ligature during GPOS); IGNORE_LIGATURES /
                // MARK_ATTACHMENT_CLASS_FILTER / USE_MARK_FILTERING_SET
                // narrow the match the same way.
                let skip = SkipFilter::new(self, lookup.flags, lookup.mark_filtering_set);
                // No Ligature record inspects more than this many
                // visible glyphs, so the candidate run stops there.
                let window = resolved.max_components().max(1);
                let second = resolved.second_components();
                let LookupScratch {
                    cand_gids,
                    cand_idx,
                    ..
                } = scratch;
                let mut i = 0usize;
                while i < buf.len() {
                    let first = buf[i].gid;
                    if !resolved.may_start(first) || skip.skips(first) {
                        i += 1;
                        continue;
                    }
                    // Every ligature of this lookup has a second
                    // component: nothing can form unless the next
                    // visible glyph is one of them.
                    if let Some(second) = second
                        && !buf[i + 1..]
                            .iter()
                            .map(|w| w.gid)
                            .find(|&g| !skip.skips(g))
                            .is_some_and(|g| second.contains(g))
                    {
                        i += 1;
                        continue;
                    }
                    // Build the candidate run from position i, recording
                    // which absolute indices the non-skipped gids came from.
                    cand_gids.clear();
                    cand_idx.clear();
                    for (off, w) in buf[i..].iter().enumerate() {
                        if skip.skips(w.gid) {
                            continue;
                        }
                        cand_gids.push(w.gid);
                        cand_idx.push(i + off);
                        if cand_gids.len() >= window {
                            break;
                        }
                    }
                    if let Some((lig, consumed)) = resolved.ligature(gsub, cand_gids)
                        && consumed >= 1
                    {
                        let cluster = buf[i].cluster;
                        buf[i] = WorkGlyph { gid: lig, cluster };
                        run.add(lig);
                        // Remove the consumed components 1..consumed
                        // (their absolute indices), highest first so
                        // earlier removals don't shift later indices.
                        for &idx in cand_idx[1..consumed.min(cand_idx.len())].iter().rev() {
                            if idx < buf.len() {
                                buf.remove(idx);
                            }
                        }
                    }
                    i += 1;
                }
            }
            5 | 6 => {
                // Contextual (5) / chained-context (6) substitution. A
                // hit at `pos` rewrites `gids` — the glyph-id mirror of
                // the buffer — in place; the buffer then takes over the
                // rewritten region.
                let gids = &mut scratch.gids;
                gids.clear();
                gids.extend(buf.iter().map(|w| w.gid));
                let chained = lookup.kind == 6;
                let mut pos = 0usize;
                while pos < buf.len() {
                    if resolved.may_start(gids[pos]) {
                        let mut extent = EditExtent::starting_at(pos);
                        if resolved.substitute_in_context(gsub, chained, gids, pos, &mut extent) {
                            reconcile_context_rewrite(buf, gids, pos, extent, run);
                        }
                    }
                    pos += 1;
                }
            }
            8 => {
                // Reverse chained-context single substitution: 1:1, walked
                // right-to-left so a later substitution's lookahead sees
                // the original (not yet substituted) glyphs.
                let gids = &mut scratch.gids;
                gids.clear();
                gids.extend(buf.iter().map(|w| w.gid));
                for pos in (0..buf.len()).rev() {
                    if resolved.may_start(gids[pos])
                        && let Some(g) = resolved.reverse_chain(gsub, gids, pos)
                    {
                        buf[pos].gid = g;
                        run.add(g);
                    }
                }
            }
            _ => {}
        }
    }

    /// GPOS positioning stage. Seeds advances from `hmtx`, then applies
    /// the planned GPOS lookups in LookupList order.
    fn run_gpos(
        &self,
        buf: &[WorkGlyph],
        lookups: &[PlanLookup],
        run: &RunGlyphs<'_>,
        gids: &mut Vec<u16>,
    ) -> Vec<ShapedGlyph> {
        let mut out = self.seed_advances(buf);
        let Some(gpos) = self.gpos() else {
            return out;
        };
        if lookups.is_empty() {
            return out;
        }
        let (table, _) = gpos.lookup_list_location();
        // Positioning never changes glyph ids: the run summary and one
        // glyph id array serve every lookup.
        for lookup in lookups {
            let resolved = Resolved::new(lookup, table);
            if run.rules_out(&resolved) {
                continue;
            }
            self.apply_gpos_lookup(gpos, &mut out, lookup, resolved, gids);
        }
        out
    }

    /// Apply one GPOS lookup across the positioned buffer. `resolved`
    /// prunes positions whose glyph is not in the lookup's first-glyph
    /// Coverage tables (for the mark-attachment types that is the
    /// MarkCoverage — the attaching mark is the "current" glyph).
    fn apply_gpos_lookup(
        &self,
        gpos: &GposTable<'_>,
        out: &mut [ShapedGlyph],
        lookup: &PlanLookup,
        resolved: Resolved<'_>,
        gids: &mut Vec<u16>,
    ) {
        match lookup.kind {
            1 => {
                // Single adjustment.
                for g in out.iter_mut() {
                    if resolved.may_start(g.glyph_id)
                        && let Some(v) = resolved.single_pos(gpos, g.glyph_id)
                    {
                        g.x_offset += v.x_placement as i32;
                        g.y_offset += v.y_placement as i32;
                        g.x_advance += v.x_advance as i32;
                        g.y_advance += v.y_advance as i32;
                    }
                }
            }
            2 => {
                // Pair adjustment (kerning): the x-advance applied to the
                // left glyph of each pair. The pair members are the
                // current glyph and the *next non-skipped* glyph per the
                // lookup's §2 skip filter — so a kern pair separated by an
                // (ignored) combining mark still kerns, the canonical
                // IGNORE_MARKS-on-kern case.
                let skip = SkipFilter::new(self, lookup.flags, lookup.mark_filtering_set);
                for i in 0..out.len() {
                    let left = out[i].glyph_id;
                    if !resolved.may_start(left) || skip.skips(left) {
                        continue;
                    }
                    // Find the next glyph the lookup does not skip.
                    let Some(right) = out[i + 1..]
                        .iter()
                        .map(|g| g.glyph_id)
                        .find(|&g| !skip.skips(g))
                    else {
                        break;
                    };
                    out[i].x_advance += resolved.pair(gpos, &skip, left, right) as i32;
                }
            }
            3 => {
                // Cursive attachment: glyph N+1's entry anchor lands on
                // glyph N's exit anchor. The per-glyph delta moves N+1 so
                // its entry aligns with N's exit (x via offset, the
                // baseline shift via y_offset). Glyphs the lookup skips
                // (§2) are invisible to the chain, so the exit of N is
                // matched against the entry of the next *non-skipped*
                // glyph.
                let skip = SkipFilter::new(self, lookup.flags, lookup.mark_filtering_set);
                let mut prev_exit: Option<(i16, i16)> = None;
                for g in out.iter_mut() {
                    if skip.skips(g.glyph_id) {
                        continue;
                    }
                    let attachment = if resolved.may_start(g.glyph_id) {
                        resolved.cursive(gpos, g.glyph_id)
                    } else {
                        None
                    };
                    if let Some(att) = attachment {
                        if let (Some((px, py)), Some((ex, ey))) = (prev_exit, att.entry) {
                            g.x_offset += (px - ex) as i32;
                            g.y_offset += (py - ey) as i32;
                        }
                        prev_exit = att.exit;
                    } else {
                        prev_exit = None;
                    }
                }
            }
            // Mark-to-base (4): a mark glyph attaches to the nearest
            // preceding base glyph. Mark-to-ligature (5): a mark attaches
            // to a component of a preceding ligature — the last preceding
            // ligature, component 0 (a reasonable default without
            // per-component cluster tracking from the substitution
            // stage). Mark-to-mark (6): a mark attaches to the
            // immediately preceding mark.
            4..=6 => self.apply_mark_attach(gpos, out, lookup, resolved),
            7 | 8 => {
                // Contextual (7) / chained-context (8) positioning.
                gids.clear();
                gids.extend(out.iter().map(|g| g.glyph_id));
                let chained = lookup.kind == 8;
                for pos in 0..out.len() {
                    if resolved.may_start(gids[pos])
                        && let Some(records) =
                            resolved.position_in_context(gpos, chained, gids, pos)
                    {
                        apply_pos_records(out, &records);
                    }
                }
            }
            _ => {}
        }
    }

    /// Shared mark-to-base (type 4) / mark-to-ligature (type 5) /
    /// mark-to-mark (type 6) attachment. For each mark glyph, find the
    /// nearest preceding attachment glyph (a base, a ligature, or a mark)
    /// that the lookup binds it to, and offset the mark so its anchor
    /// lands on the attachment glyph's anchor.
    ///
    /// The candidate attachment glyph is the nearest preceding glyph the
    /// lookup's §2 skip filter does *not* ignore: a mark-to-base or
    /// mark-to-ligature lookup almost always sets IGNORE_MARKS so the
    /// scan steps over interspersed marks and lands on the base /
    /// ligature, while a mark-to-mark (`mkmk`) lookup leaves marks
    /// visible so it pairs with the immediately preceding mark. The
    /// current mark itself is left unattached when the lookup skips it.
    fn apply_mark_attach(
        &self,
        gpos: &GposTable<'_>,
        out: &mut [ShapedGlyph],
        lookup: &PlanLookup,
        resolved: Resolved<'_>,
    ) {
        let skip = SkipFilter::new(self, lookup.flags, lookup.mark_filtering_set);
        for i in 0..out.len() {
            let mark = out[i].glyph_id;
            // Only a glyph in one of the lookup's MarkCoverage tables can
            // attach.
            if !resolved.may_start(mark) || skip.skips(mark) {
                continue;
            }
            // The nearest non-skipped predecessor is the only attachment
            // candidate, whether or not it produces a hit.
            let Some(j) = (0..i).rev().find(|&j| !skip.skips(out[j].glyph_id)) else {
                continue;
            };
            let target = out[j].glyph_id;
            if let Some((dx, dy)) = resolved.mark_attachment(gpos, lookup.kind, target, mark) {
                // Place the mark relative to the attachment glyph's pen
                // origin. That glyph sits at the accumulated advance from
                // j to i; a mark has (typically) zero advance, so its
                // drawing origin is the current pen. We express
                // attachment as a placement offset that pulls the mark
                // back over the target by the advance run plus the
                // anchor delta.
                let between: i32 = out[j..i].iter().map(|g| g.x_advance).sum();
                out[i].x_offset += dx as i32 - between;
                out[i].y_offset += dy as i32;
            }
        }
    }
}

/// Bring the `WorkGlyph` buffer in line with `new`, the glyph-id run a
/// contextual / chained GSUB rule at `pos` just rewrote (`buf` still
/// holds the run as it was), preserving clusters as best we can. The
/// rewrite may change the run length (a nested multiple- or
/// ligature-substitution record). The longest unchanged prefix and
/// suffix keep their glyphs' clusters; every glyph in the changed middle
/// takes the cluster of the glyph at `pos`.
///
/// `extent` bounds where `new` can differ from the old run, so the
/// comparison touches the rewritten region rather than the whole run.
/// `run` learns the glyph ids written into the buffer.
fn reconcile_context_rewrite(
    buf: &mut Vec<WorkGlyph>,
    new: &[u16],
    pos: usize,
    extent: EditExtent,
    run: &mut RunGlyphs<'_>,
) {
    let (old_len, new_len) = (buf.len(), new.len());
    // Per `EditExtent`: new[..pos] == old[..pos] and
    // new[extent.end..] == old[old_end..].
    let old_end = extent.end.wrapping_add_signed(-extent.delta);
    debug_assert!(extent.start == pos && old_end <= old_len && extent.end <= new_len);
    if extent.delta == 0 && (pos..extent.end).all(|k| buf[k].gid == new[k]) {
        // The rule matched but left every glyph as it was.
        return;
    }
    // Longest common prefix (everything before `pos` is known equal).
    let mut pre = pos;
    while pre < old_len && pre < new_len && buf[pre].gid == new[pre] {
        pre += 1;
    }
    // Longest common suffix that stays clear of the prefix (everything
    // from the end of the rewritten region on is known equal).
    let mut suf = (old_len - old_end).min(old_len - pre).min(new_len - pre);
    while suf < old_len - pre
        && suf < new_len - pre
        && buf[old_len - 1 - suf].gid == new[new_len - 1 - suf]
    {
        suf += 1;
    }
    let cluster = buf[pos].cluster;
    let middle = &new[pre..new_len - suf];
    buf.splice(
        pre..old_len - suf,
        middle.iter().map(|&gid| WorkGlyph { gid, cluster }),
    );
    for &gid in middle {
        run.add(gid);
    }
}

/// Effective type of lookup `index` in a `(index, type, subtable count)`
/// listing sorted by index, or 0 when the listing omits it.
fn lookup_kind(types: &[(u16, u16, u16)], index: u16) -> u16 {
    types
        .binary_search_by_key(&index, |&(i, _, _)| i)
        .map(|at| types[at].1)
        .unwrap_or(0)
}

/// Apply a set of [`PosRecord`]s (absolute-indexed) from a contextual /
/// chained positioning match onto the output buffer.
fn apply_pos_records(out: &mut [ShapedGlyph], records: &[PosRecord]) {
    for r in records {
        if let Some(g) = out.get_mut(r.glyph_index) {
            g.x_offset += r.value.x_placement as i32;
            g.y_offset += r.value.y_placement as i32;
            g.x_advance += r.value.x_advance as i32;
            g.y_advance += r.value.y_advance as i32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole-run formulation of the contextual-rewrite reconcile:
    /// align the longest unchanged prefix / suffix of `old` and `new`
    /// and give the changed middle the cluster of the glyph at `pos`.
    fn reconcile_whole_run(buf: &[WorkGlyph], new: &[u16], pos: usize) -> Vec<WorkGlyph> {
        let old: Vec<u16> = buf.iter().map(|w| w.gid).collect();
        if new == old {
            return buf.to_vec();
        }
        let mut pre = 0usize;
        while pre < old.len() && pre < new.len() && old[pre] == new[pre] {
            pre += 1;
        }
        let mut suf = 0usize;
        while suf < (old.len() - pre)
            && suf < (new.len() - pre)
            && old[old.len() - 1 - suf] == new[new.len() - 1 - suf]
        {
            suf += 1;
        }
        let cluster = buf[pos].cluster;
        let mut rebuilt = buf[..pre].to_vec();
        for &gid in &new[pre..new.len() - suf] {
            rebuilt.push(WorkGlyph { gid, cluster });
        }
        rebuilt.extend_from_slice(&buf[old.len() - suf..]);
        rebuilt
    }

    /// The extent-bounded reconcile gives exactly what the whole-run
    /// comparison gives, for random in-place edits at or after `pos` —
    /// replacements, deletions and insertions over a two-glyph alphabet,
    /// so coincidentally equal neighbours are the norm.
    #[test]
    fn extent_bounded_reconcile_matches_whole_run_comparison() {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % n as u64) as usize
        };
        for _ in 0..20_000 {
            let len = 1 + next(12);
            let buf: Vec<WorkGlyph> = (0..len)
                .map(|i| WorkGlyph {
                    gid: next(2) as u16,
                    cluster: i as u32 * 3,
                })
                .collect();
            let pos = next(len);
            let mut new: Vec<u16> = buf.iter().map(|w| w.gid).collect();
            let mut extent = EditExtent::starting_at(pos);
            for _ in 0..next(4) {
                if new.len() <= pos {
                    break;
                }
                let at = pos + next(new.len() - pos);
                let removed = 1 + next((new.len() - at).min(3));
                let inserted = next(4);
                new.splice(at..at + removed, (0..inserted).map(|_| next(2) as u16));
                extent.end = extent.end.max(at + removed) - removed + inserted;
                extent.delta += inserted as isize - removed as isize;
            }
            let expected = reconcile_whole_run(&buf, &new, pos);
            let mut actual = buf.clone();
            let (mut distinct, mut seen) = (Vec::new(), vec![0u64; GLYPH_BITMAP_WORDS]);
            let mut run = RunGlyphs::of(&actual, &mut distinct, &mut seen);
            reconcile_context_rewrite(&mut actual, &new, pos, extent, &mut run);
            let key = |b: &[WorkGlyph]| b.iter().map(|w| (w.gid, w.cluster)).collect::<Vec<_>>();
            assert_eq!(key(&actual), key(&expected), "pos {pos}, new {new:?}");
        }
    }
}
