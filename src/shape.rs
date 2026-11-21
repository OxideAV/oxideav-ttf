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

use crate::Font;
use crate::layout::{
    Digest, GlyphSet, LayoutTable, LookupAccel, PlanFeature, PlanLookup, ShapePlan, SubAccel,
    block_shift, digest_bit, flatten_class_def,
};

mod place;
mod query;
mod seq;
mod subst;
mod wire;

pub use query::{MarkAnchors, PairAdjustment};

/// How deep contextual lookups may nest (a contextual lookup whose
/// nested lookup is contextual, and so on): a guard against a
/// self-referential lookup graph.
const MAX_NESTING: u8 = 8;

/// One positioned glyph emitted by [`Font::shape`] and
/// [`Font::shape_glyphs`].
///
/// All four positioning fields are in font design units (the same units
/// as `head.unitsPerEm`), in the TrueType Y-up convention. To render at
/// a target pixel-per-em `ppem`, scale by `ppem / units_per_em`.
///
/// * `glyph_id` — the final glyph id after all GSUB substitutions.
/// * `cluster` — the cluster value of the input item this glyph
///   originated from (for text input: the byte offset of the character
///   into the original `&str`). Stable across substitutions: a ligature
///   inherits the cluster of its first component; a
///   multiple-substitution expansion shares the source glyph's cluster
///   across every output.
/// * `x_offset` / `y_offset` — placement adjustment applied to the pen
///   position *for drawing this glyph only* (does not move the pen).
///   Marks attach to bases through this field.
/// * `x_advance` / `y_advance` — how far the pen moves after drawing
///   this glyph. Seeded from the `hmtx` advance (the `vmtx` advance
///   height, negated, for a top-to-bottom run) with the `HVAR` / `VVAR`
///   delta of the current variation instance, then adjusted by GPOS.
///
/// The glyphs come back in drawing order: start with the pen at the
/// line origin and, for each glyph in turn, draw it at pen + offset and
/// then move the pen by its advance.
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

/// One input glyph of [`Font::shape_glyphs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunGlyph {
    /// Glyph id (normally the `cmap` mapping of a character).
    pub glyph_id: u16,
    /// Caller-defined cluster value, carried to every output glyph that
    /// derives from this one.
    pub cluster: u32,
    /// Feature mask: a feature requested with a non-zero mask
    /// ([`ShapeFeature::with_mask`]) acts on this glyph only when the
    /// two masks share a bit.
    pub mask: u32,
}

impl RunGlyph {
    /// A glyph with an empty feature mask.
    pub fn new(glyph_id: u16, cluster: u32) -> Self {
        RunGlyph {
            glyph_id,
            cluster,
            mask: 0,
        }
    }

    /// Builder: set the feature [`mask`](Self::mask).
    #[must_use]
    pub fn with_mask(mut self, mask: u32) -> Self {
        self.mask = mask;
        self
    }
}

/// Direction a run is laid out in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Direction {
    /// Horizontal, logical order drawn left to right.
    #[default]
    LeftToRight,
    /// Horizontal, logical order drawn right to left. The shaped glyphs
    /// come back in *drawing* order — the reverse of the input — so the
    /// pen still moves left to right over them.
    RightToLeft,
    /// Vertical: the pen moves down (`y_advance` is negative), advances
    /// come from `vmtx` / `VVAR`.
    TopToBottom,
}

/// One feature of a [`ShapeOptions`] request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ShapeFeature {
    /// OpenType feature tag.
    pub tag: [u8; 4],
    /// `0`: the feature acts on the whole run. Otherwise it acts where
    /// every glyph of a lookup's input sequence has a
    /// [`RunGlyph::mask`] sharing a bit with this mask.
    pub mask: u32,
    /// Which alternate an alternate-substitution lookup of this feature
    /// picks (zero-based index into the AlternateSet).
    pub alternate: u16,
}

impl ShapeFeature {
    /// `tag`, acting on the whole run, first alternate.
    pub fn new(tag: [u8; 4]) -> Self {
        ShapeFeature {
            tag,
            mask: 0,
            alternate: 0,
        }
    }

    /// Builder: restrict the feature to glyphs whose mask shares a bit
    /// with `mask`.
    #[must_use]
    pub fn with_mask(mut self, mask: u32) -> Self {
        self.mask = mask;
        self
    }

    /// Builder: pick alternate `index` (zero-based) in alternate
    /// substitutions.
    #[must_use]
    pub fn with_alternate(mut self, index: u16) -> Self {
        self.alternate = index;
        self
    }
}

/// A shaping request for [`Font::shape_glyphs`] / [`Font::shape_text`]:
/// script, language system, direction and the features to apply.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ShapeOptions {
    /// OpenType script tag.
    pub script: [u8; 4],
    /// OpenType language-system tag (`None`: the script's default).
    pub language: Option<[u8; 4]>,
    pub direction: Direction,
    pub features: Vec<ShapeFeature>,
    /// Give every glyph GDEF classes as a mark a zero nominal advance,
    /// whatever `hmtx` says, so a combining mark never moves the pen.
    /// The positioning lookups themselves leave advances alone ("Advance
    /// widths are not affected" by a mark attachment); this is a layout
    /// policy for fonts whose marks carry a spacing width. Off by
    /// default.
    pub zero_mark_advances: bool,
}

impl ShapeOptions {
    /// A left-to-right request for `script` with its default language
    /// system and no optional feature.
    pub fn new(script: [u8; 4]) -> Self {
        ShapeOptions {
            script,
            language: None,
            direction: Direction::LeftToRight,
            features: Vec::new(),
            zero_mark_advances: false,
        }
    }

    /// Builder: select a language system.
    #[must_use]
    pub fn with_language(mut self, language: [u8; 4]) -> Self {
        self.language = Some(language);
        self
    }

    /// Builder: set the [`Direction`].
    #[must_use]
    pub fn with_direction(mut self, direction: Direction) -> Self {
        self.direction = direction;
        self
    }

    /// Builder: set [`zero_mark_advances`](Self::zero_mark_advances).
    #[must_use]
    pub fn with_zero_mark_advances(mut self, zero: bool) -> Self {
        self.zero_mark_advances = zero;
        self
    }

    /// Builder: apply the feature `tag` to the whole run.
    #[must_use]
    pub fn with_feature(mut self, tag: [u8; 4]) -> Self {
        self.features.push(ShapeFeature::new(tag));
        self
    }

    /// Builder: apply the feature `tag` to the glyphs whose
    /// [`RunGlyph::mask`] shares a bit with `mask`.
    #[must_use]
    pub fn with_masked_feature(mut self, tag: [u8; 4], mask: u32) -> Self {
        self.features.push(ShapeFeature::new(tag).with_mask(mask));
        self
    }

    /// Builder: add a fully specified feature.
    #[must_use]
    pub fn with_shape_feature(mut self, feature: ShapeFeature) -> Self {
        self.features.push(feature);
        self
    }
}

/// `true` when a lookup conditional on `lookup_mask` may act on a glyph
/// carrying `glyph_mask`.
#[inline]
fn mask_allows(lookup_mask: u32, glyph_mask: u32) -> bool {
    lookup_mask == 0 || lookup_mask & glyph_mask != 0
}

/// One glyph of the working buffer.
#[derive(Debug, Clone, Copy)]
struct WorkGlyph {
    gid: u16,
    /// Ligature association (0: none). On a ligature glyph
    /// (`lig_comps > 0`) its own identifier; on a mark the identifier of
    /// the ligature it belongs to.
    lig_id: u16,
    cluster: u32,
    mask: u32,
    /// On a mark tied to a ligature: the zero-based component it sits
    /// on.
    lig_comp: u8,
    /// On a ligature glyph: how many components it stands for.
    lig_comps: u8,
}

/// The `lookupFlag` skip predicate (§6.2 LookupFlag bit enumeration) of
/// one lookup, bound to a font. Answers exactly what
/// [`Font::lookup_skips_glyph`] answers, reading the flat GDEF class
/// array where it can.
struct SkipFilter<'f, 'a> {
    font: &'f Font<'a>,
    classes: Option<&'f [u16]>,
    flags: u16,
    mark_filtering_set: Option<u16>,
    /// `false` when the lookup can never skip a glyph (no skip bit, no
    /// mark-attachment class, no filtering set, or no GDEF at all).
    active: bool,
    /// A mark-attachment class or a mark filtering set qualifies which
    /// marks are skipped.
    marks_are_filtered: bool,
    /// With `marks_are_filtered`: the marks that are *not* skipped, when
    /// the lookup has that set flattened; otherwise GDEF is asked.
    kept_marks: Option<&'f GlyphSet>,
}

impl<'f, 'a> SkipFilter<'f, 'a> {
    fn new(font: &'f Font<'a>, flags: u16, mark_filtering_set: Option<u16>) -> Self {
        let active = (flags & 0xFF0E != 0 || mark_filtering_set.is_some()) && font.gdef().is_some();
        Self {
            font,
            // The flat class array is only consulted when a skip bit is
            // set (every use below is behind `active`).
            classes: if active { font.glyph_classes() } else { None },
            flags,
            mark_filtering_set,
            active,
            marks_are_filtered: flags & 0xFF00 != 0 || mark_filtering_set.is_some(),
            kept_marks: None,
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
                    || (self.marks_are_filtered
                        && match self.kept_marks {
                            Some(kept) => !kept.contains(gid),
                            None => self.font.lookup_skips_glyph(
                                self.flags,
                                self.mark_filtering_set,
                                gid,
                            ),
                        })
            }
            _ => false,
        }
    }

    /// Index of the first glyph after `from` the lookup does not ignore.
    #[inline]
    fn next(&self, buf: &[WorkGlyph], from: usize) -> Option<usize> {
        if !self.active {
            return (from + 1 < buf.len()).then_some(from + 1);
        }
        (from + 1..buf.len()).find(|&at| !self.skips(buf[at].gid))
    }

    /// Index of the last glyph before `from` the lookup does not ignore.
    #[inline]
    fn prev(&self, buf: &[WorkGlyph], from: usize) -> Option<usize> {
        if !self.active {
            return from.checked_sub(1);
        }
        (0..from).rev().find(|&at| !self.skips(buf[at].gid))
    }
}

/// One layout table of a font, as the shaping stages address it.
#[derive(Clone, Copy)]
struct Engine<'f, 'a> {
    font: &'f Font<'a>,
    which: LayoutTable,
    /// The whole GSUB / GPOS table; sub-table offsets index into it.
    table: &'f [u8],
    lookup_list_off: u32,
}

impl<'f, 'a> Engine<'f, 'a> {
    fn new(font: &'f Font<'a>, which: LayoutTable, location: (&'f [u8], u32)) -> Self {
        Engine {
            font,
            which,
            table: location.0,
            lookup_list_off: location.1,
        }
    }

    /// Lookup `index`, resolved once per font. `None`: no such lookup,
    /// or a type that is never applied.
    #[inline]
    fn lookup(&self, index: u16) -> Option<&'f LookupAccel> {
        self.font.layout.lookup(
            self.which,
            self.table,
            self.lookup_list_off,
            index,
            self.font.maxp.num_glyphs as usize,
        )
    }

    /// The bytes of a resolved sub-table.
    #[inline]
    fn sub_bytes(&self, sub: &SubAccel) -> &'f [u8] {
        self.table.get(sub.offset..).unwrap_or(&[])
    }

    #[inline]
    fn skip_filter(&self, lookup: &'f LookupAccel) -> SkipFilter<'f, 'a> {
        let mut filter = SkipFilter::new(self.font, lookup.flags, lookup.mark_filtering_set);
        // Which marks a filtering lookup keeps is the same for every
        // run: derived once per lookup (accelerators on).
        if filter.marks_are_filtered && filter.classes.is_some() && lookup.flags & 0x0008 == 0 {
            filter.kept_marks = lookup
                .kept_marks
                .get_or_init(|| {
                    crate::layout::kept_marks(
                        self.font.gdef()?,
                        lookup.flags,
                        lookup.mark_filtering_set,
                        self.font.maxp.num_glyphs as usize,
                    )
                })
                .as_ref();
        }
        filter
    }

    /// `true` when GDEF classes `gid` as a mark.
    #[inline]
    fn is_mark(&self, gid: u16) -> bool {
        // The flat class array is built the first time a mark test is
        // needed, not for every font that is shaped.
        match self.font.glyph_classes().and_then(|c| c.get(gid as usize)) {
            Some(&class) => class == crate::tables::gdef::CLASS_MARK,
            None => self.font.gdef().is_some_and(|g| g.is_mark(gid)),
        }
    }
}

/// Which glyph ids a run holds, for ruling whole lookups out. Always a
/// superset: a glyph substituted away stays listed (that only costs a
/// missed shortcut), a glyph written into the run is added.
struct RunGlyphs<'s> {
    /// Summary of the run's glyphs.
    digest: Digest,
    /// Block shift of the font ([`block_shift`] of its glyph count).
    shift: u8,
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
    /// Pending attachments of the positioning stage.
    attach: Vec<place::Attach>,
}

/// Scratch vectors individual lookups reuse.
#[derive(Default)]
struct LookupScratch {
    /// Stack of matched input-sequence positions, one frame per
    /// contextual lookup being applied.
    positions: Vec<usize>,
    /// Buffer length changes made by nested lookups.
    edits: Vec<subst::Edit>,
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
                positions: Vec::new(),
                edits: Vec::new(),
                cand_gids: Vec::new(),
                cand_idx: Vec::new(),
            },
            attach: Vec::new(),
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
        self.attach.clear();
        self.lookups.positions.clear();
        self.lookups.edits.clear();
        let _ = SHAPE_SCRATCH.try_with(|cell| cell.set(self));
    }
}

impl<'s> RunGlyphs<'s> {
    /// Summarise `buf`. `distinct` must be empty and `seen` all zero.
    fn of(
        buf: &[WorkGlyph],
        num_glyphs: usize,
        distinct: &'s mut Vec<u16>,
        seen: &'s mut [u64],
    ) -> Self {
        let mut run = RunGlyphs {
            digest: Digest::EMPTY,
            shift: block_shift(num_glyphs),
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
            self.digest.add(Digest::of(gid, self.shift));
        }
    }

    /// `true` when the digests of a planned lookup already show it
    /// cannot act anywhere in the run.
    #[inline]
    fn misses(&self, planned: &PlanLookup) -> bool {
        !planned.first_digest.intersects(&self.digest)
            || !planned.second_digest.intersects(&self.digest)
    }

    /// Which glyphs of the run `lookup` can act on, as a mask of their
    /// glyph id residues modulo 64 (a position whose glyph has another
    /// residue cannot be a candidate). `0`: the lookup provably cannot
    /// act anywhere in the run — no glyph of the run can start it, or,
    /// for a lookup whose every rule looks at a second glyph, no glyph
    /// of the run can be that glyph.
    #[inline]
    fn candidates(&self, lookup: &LookupAccel) -> u64 {
        let first: &GlyphSet = &lookup.first;
        let residues = self
            .distinct
            .iter()
            .filter(|&&g| first.contains(g))
            .fold(0, |residues, &g| residues | digest_bit(g));
        match lookup.second_components.as_ref() {
            Some(second) if residues != 0 && !self.distinct.iter().any(|&g| second.contains(g)) => {
                0
            }
            _ => residues,
        }
    }
}

impl<'a> Font<'a> {
    /// Shape a run of text into positioned glyphs under `script` /
    /// `lang`, applying the listed `features`.
    ///
    /// `script` and `lang` are OpenType tags (`*b"latn"`, `*b"arab"`,
    /// `*b"DFLT"`; `lang = None` selects the script's default language
    /// system). `features` is the list of feature tags the caller wants
    /// enabled (e.g. `[*b"ccmp", *b"liga", *b"kern"]`); a feature tag
    /// the font does not list under the active script is silently
    /// ignored, and the language system's *required* feature applies
    /// whether listed or not. The relative order of `features` does not
    /// dictate application order — the GSUB/GPOS lookups behind the
    /// *union* of requested features run in LookupList order, per the
    /// OpenType common-table-format rules.
    ///
    /// Returns one [`ShapedGlyph`] per output glyph, in the order of the
    /// text, positioned for a pen that moves left to right. For a font
    /// with no GSUB/GPOS, this degenerates to nominal cmap mapping with
    /// `hmtx` advances. When `kern` is requested and the font's GPOS has
    /// no `kern` feature for the language system, pair kerning comes
    /// from the legacy `kern` table if there is one. Use [`Font::shape_text`] for a right-to-left or
    /// vertical run, and [`Font::shape_glyphs`] to shape glyphs with
    /// per-glyph feature masks.
    ///
    /// Shaping happens at the font's current variation instance
    /// ([`Font::set_variation_coords`]): FeatureVariations pick the
    /// lookups, `HVAR` varies the advances, and GPOS values and anchors
    /// follow their VariationIndex tables.
    pub fn shape(
        &self,
        text: &str,
        script: [u8; 4],
        lang: Option<[u8; 4]>,
        features: &[[u8; 4]],
    ) -> Vec<ShapedGlyph> {
        let mut scratch = ShapeScratch::take();
        self.load_text(&mut scratch.buf, text);
        let requested = features.iter().map(|&tag| PlanFeature {
            tag,
            mask: 0,
            alternate: 0,
        });
        let shaped = self.shape_buffer(
            &mut scratch,
            script,
            lang,
            requested,
            Direction::LeftToRight,
            false,
        );
        scratch.recycle();
        shaped
    }

    /// Shape a run of text under `options`: [`Font::shape`] with a
    /// direction and per-feature settings. Each character maps to its
    /// nominal `cmap` glyph with the character's byte offset as cluster.
    pub fn shape_text(&self, text: &str, options: &ShapeOptions) -> Vec<ShapedGlyph> {
        let mut scratch = ShapeScratch::take();
        self.load_text(&mut scratch.buf, text);
        let shaped = self.shape_with(&mut scratch, options);
        scratch.recycle();
        shaped
    }

    /// Shape a run of glyphs under `options`.
    ///
    /// This is the entry for a text-layout engine that does its own
    /// character analysis: it maps characters to glyphs, orders them,
    /// decides which features act on which glyphs — positional forms of
    /// a cursive script, the phases of a syllable — and expresses that
    /// as a [`RunGlyph::mask`] per glyph against a
    /// [`ShapeFeature::mask`] per feature. `glyphs` is in logical
    /// order.
    ///
    /// A feature with mask 0 acts on the whole run. A feature with a
    /// non-zero mask acts at a position only when every glyph of the
    /// lookup's input sequence there (the glyph itself; both glyphs of
    /// a pair; all components of a ligature; the input sequence of a
    /// contextual rule, not its backtrack / lookahead context) shares a
    /// bit with it. A lookup reached from several requested features
    /// acts wherever any of them does.
    ///
    /// The result is in drawing order (see [`ShapedGlyph`]); each output
    /// glyph carries the cluster of the input glyph it derives from.
    pub fn shape_glyphs(&self, glyphs: &[RunGlyph], options: &ShapeOptions) -> Vec<ShapedGlyph> {
        let mut scratch = ShapeScratch::take();
        scratch.buf.extend(glyphs.iter().map(|g| WorkGlyph {
            gid: g.glyph_id,
            lig_id: 0,
            cluster: g.cluster,
            mask: g.mask,
            lig_comp: 0,
            lig_comps: 0,
        }));
        let shaped = self.shape_with(&mut scratch, options);
        scratch.recycle();
        shaped
    }

    /// Character-to-glyph mapping: one buffer glyph per `char`, glyph 0
    /// (`.notdef`) for an unmapped one.
    fn load_text(&self, buf: &mut Vec<WorkGlyph>, text: &str) {
        buf.extend(text.char_indices().map(|(byte_idx, ch)| WorkGlyph {
            gid: self.glyph_index(ch).unwrap_or(0),
            lig_id: 0,
            cluster: byte_idx as u32,
            mask: 0,
            lig_comp: 0,
            lig_comps: 0,
        }));
    }

    fn shape_with(&self, scratch: &mut ShapeScratch, options: &ShapeOptions) -> Vec<ShapedGlyph> {
        let requested = options.features.iter().map(|f| PlanFeature {
            tag: f.tag,
            mask: f.mask,
            alternate: f.alternate,
        });
        self.shape_buffer(
            scratch,
            options.script,
            options.language,
            requested,
            options.direction,
            options.zero_mark_advances,
        )
    }

    /// Shape the glyphs loaded into `scratch.buf`.
    fn shape_buffer<I>(
        &self,
        scratch: &mut ShapeScratch,
        script: [u8; 4],
        lang: Option<[u8; 4]>,
        features: I,
        direction: Direction,
        zero_marks: bool,
    ) -> Vec<ShapedGlyph>
    where
        I: Iterator<Item = PlanFeature> + Clone,
    {
        let plan = self.layout.plan(script, lang, features.clone(), || {
            let requested: Vec<PlanFeature> = features.collect();
            ShapePlan {
                gsub: self.plan_gsub(script, lang, &requested),
                gpos: self.plan_gpos(script, lang, &requested),
                legacy_kern: self.plan_legacy_kern(script, lang, &requested),
            }
        });
        let ShapeScratch {
            buf,
            distinct,
            seen,
            lookups,
            attach,
        } = scratch;
        let mut out = Vec::new();
        if plan.gsub.is_empty() && plan.gpos.is_empty() {
            self.seed_advances(buf, direction, zero_marks, &mut out);
            self.apply_legacy_kern(&plan, buf, direction, &mut out);
            if direction == Direction::RightToLeft {
                out.reverse();
            }
            return out;
        }
        let mut run = RunGlyphs::of(buf, self.maxp.num_glyphs as usize, distinct, seen);

        // --- GSUB substitution stage --------------------------------------
        if let Some(gsub) = self.gsub()
            && !plan.gsub.is_empty()
        {
            let max_len = buf.len().saturating_mul(8).saturating_add(64);
            subst::Subst {
                eng: Engine::new(self, LayoutTable::Gsub, gsub.lookup_list_location()),
                buf: &mut *buf,
                run: &mut run,
                sc: &mut *lookups,
                max_len,
                logging: false,
                budget: 0,
                last_lig_id: 0,
                candidates: 0,
            }
            .run_all(&plan.gsub);
        }

        // --- GPOS positioning stage ---------------------------------------
        self.seed_advances(buf, direction, zero_marks, &mut out);
        self.apply_legacy_kern(&plan, buf, direction, &mut out);
        if let Some(gpos) = self.gpos()
            && !plan.gpos.is_empty()
        {
            attach.clear();
            let mut stage = place::Place {
                eng: Engine::new(self, LayoutTable::Gpos, gpos.lookup_list_location()),
                buf,
                out: &mut out,
                attach,
                positions: &mut lookups.positions,
                var: self.gpos_variation(),
                direction,
                zero_marks: zero_marks && self.gdef().is_some(),
                budget: 0,
            };
            stage.run_all(&plan.gpos, &run);
            stage.resolve_attachments();
        }
        if direction == Direction::RightToLeft {
            out.reverse();
        }
        out
    }

    /// Data of the current variation instance. A font whose coordinates
    /// were never set sits at its default instance, which is known
    /// without reading `fvar` / `avar`.
    fn instance(&self) -> &crate::layout::InstanceData {
        self.layout.instance(|| match self.var_coords {
            Some(_) => self.normalised_coords(),
            None => Vec::new(),
        })
    }

    /// The variation instance GPOS values are read at: the GDEF
    /// ItemVariationStore and the normalised coordinates, or "no deltas"
    /// at the default instance.
    fn gpos_variation(&self) -> wire::VarCtx<'_> {
        let instance = self.instance();
        if instance.is_default {
            return wire::VarCtx::DEFAULT;
        }
        let store = self.layout.gdef_store(|| {
            crate::tables::mvar::ItemVariationStore::parse(self.gdef()?.item_var_store_bytes()?)
                .ok()
        });
        wire::VarCtx {
            ivs: store,
            coords: &instance.coords,
        }
    }

    /// Resolve the active GSUB lookups for the requested features.
    fn plan_gsub(
        &self,
        script: [u8; 4],
        lang: Option<[u8; 4]>,
        requested: &[PlanFeature],
    ) -> Vec<PlanLookup> {
        let Some(gsub) = self.gsub() else {
            return Vec::new();
        };
        let eng = Engine::new(self, LayoutTable::Gsub, gsub.lookup_list_location());
        let resolved = self.gsub_features_for_script_at_instance(script, lang);
        plan_lookups(
            eng,
            requested,
            resolved
                .iter()
                .map(|f| (f.tag, f.required, f.lookup_indices.as_slice())),
        )
    }

    /// Resolve the active GPOS lookups for the requested features.
    fn plan_gpos(
        &self,
        script: [u8; 4],
        lang: Option<[u8; 4]>,
        requested: &[PlanFeature],
    ) -> Vec<PlanLookup> {
        let Some(gpos) = self.gpos() else {
            return Vec::new();
        };
        let eng = Engine::new(self, LayoutTable::Gpos, gpos.lookup_list_location());
        let resolved = self.gpos_features_for_script_at_instance(script, lang);
        plan_lookups(
            eng,
            requested,
            resolved
                .iter()
                .map(|f| (f.tag, f.required, f.lookup_indices.as_slice())),
        )
    }

    /// The glyph mask of the requested `kern` feature when kerning has to
    /// come from the `kern` table: the font has one, and its GPOS lists
    /// no `kern` feature for this language system.
    fn plan_legacy_kern(
        &self,
        script: [u8; 4],
        lang: Option<[u8; 4]>,
        requested: &[PlanFeature],
    ) -> Option<u32> {
        const KERN: [u8; 4] = *b"kern";
        let asked = requested.iter().find(|f| f.tag == KERN)?;
        self.kern()?;
        let in_gpos = self
            .gpos_features_for_script_at_instance(script, lang)
            .iter()
            .any(|f| f.tag == KERN);
        (!in_gpos).then_some(asked.mask)
    }

    /// Kerning from the `kern` table, for a plan that asks for it: each
    /// pair of neighbouring non-mark glyphs is looked up as (left,
    /// right) in the order the glyphs are drawn, and the pair's value
    /// goes to the advance of the glyph drawn first.
    fn apply_legacy_kern(
        &self,
        plan: &ShapePlan,
        buf: &[WorkGlyph],
        direction: Direction,
        out: &mut [ShapedGlyph],
    ) {
        let (Some(mask), Some(kern)) = (plan.legacy_kern, self.kern()) else {
            return;
        };
        if direction == Direction::TopToBottom {
            return;
        }
        let has_gdef = self.gdef().is_some();
        let is_mark = |gid: u16| has_gdef && self.is_mark_glyph(gid);
        let mut previous: Option<usize> = None;
        for (i, glyph) in buf.iter().enumerate() {
            if is_mark(glyph.gid) {
                continue;
            }
            if let Some(p) = previous
                && mask_allows(mask, buf[p].mask)
                && mask_allows(mask, glyph.mask)
            {
                // `p` precedes `i` in the text; drawn right to left, `i`
                // is the left glyph of the pair.
                let (left, right) = match direction {
                    Direction::RightToLeft => (i, p),
                    _ => (p, i),
                };
                let value = kern.lookup(buf[left].gid, buf[right].gid) as i32;
                out[left].x_advance = out[left].x_advance.saturating_add(value);
            }
            previous = Some(i);
        }
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

    /// One output glyph per buffer glyph with its nominal advance at
    /// the current variation instance and no placement offset: the
    /// `hmtx` advance width plus its `HVAR` delta, or — top to bottom —
    /// the `vmtx` advance height plus its `VVAR` delta, pointing down.
    fn seed_advances(
        &self,
        buf: &[WorkGlyph],
        direction: Direction,
        zero_marks: bool,
        out: &mut Vec<ShapedGlyph>,
    ) {
        let zero_marks = zero_marks && self.gdef().is_some();
        let instance = self.instance();
        // The variation tables are only touched away from the default
        // instance.
        let coords: Option<&[f32]> = (!instance.is_default).then_some(&*instance.coords);
        let hvar = coords.and_then(|c| Some((self.hvar()?, c)));
        let vvar = coords.and_then(|c| Some((self.vvar()?, c)));
        let vertical = direction == Direction::TopToBottom;
        let advance = |gid: u16| -> (i32, i32) {
            if zero_marks && self.is_mark_glyph(gid) {
                return (0, 0);
            }
            if vertical {
                // Without vertical metrics the em height stands in.
                let mut height = self
                    .glyph_advance_height(gid)
                    .map_or(self.units_per_em() as i32, i32::from);
                if let Some((vvar, coords)) = vvar
                    && let Some(delta) = vvar.advance_height_delta(gid, coords)
                {
                    height = (height as f32 + delta).round() as i32;
                }
                return (0, -height);
            }
            let width = || self.glyph_advance(gid) as u16 as i32;
            let width = match hvar {
                Some((hvar, coords)) => self.layout.varied_advance(gid, || {
                    match hvar.advance_width_delta(gid, coords) {
                        // Rounded as a whole, like `glyph_advance_varied`.
                        Some(delta) => (width() as f32 + delta).round() as i32,
                        None => width(),
                    }
                }),
                None => width(),
            };
            (width, 0)
        };
        out.reserve_exact(buf.len());
        out.extend(buf.iter().map(|w| {
            let (x_advance, y_advance) = advance(w.gid);
            ShapedGlyph {
                glyph_id: w.gid,
                cluster: w.cluster,
                x_offset: 0,
                y_offset: 0,
                x_advance,
                y_advance,
            }
        }));
    }
}

/// The lookups behind the requested features of one layout table, in
/// LookupList order ("lookups from several different features may be
/// interleaved during text processing"). `resolved` lists the features
/// of the active language system as `(tag, required, lookup indices)`.
fn plan_lookups<'r>(
    eng: Engine<'_, '_>,
    requested: &[PlanFeature],
    resolved: impl Iterator<Item = ([u8; 4], bool, &'r [u16])>,
) -> Vec<PlanLookup> {
    let mut active: Vec<PlanLookup> = Vec::new();
    for (tag, required, lookup_indices) in resolved {
        // The required feature applies to the whole run regardless of
        // what was asked for.
        let unconditional = PlanFeature {
            tag,
            mask: 0,
            alternate: 0,
        };
        let settings = requested
            .iter()
            .filter(|f| f.tag == tag)
            .copied()
            .chain(required.then_some(unconditional));
        for setting in settings {
            active.extend(lookup_indices.iter().map(|&index| PlanLookup {
                index,
                mask: setting.mask,
                alternate: setting.alternate,
                first_digest: Digest::FULL,
                second_digest: Digest::FULL,
            }));
        }
    }
    // LookupList order; a lookup reached more than once keeps one entry
    // that acts wherever any of its features does.
    active.sort_by_key(|l| l.index);
    let mut merged: Vec<PlanLookup> = Vec::with_capacity(active.len());
    for lookup in active {
        match merged.last_mut() {
            Some(last) if last.index == lookup.index => {
                last.mask = match (last.mask, lookup.mask) {
                    (0, _) | (_, 0) => 0,
                    (a, b) => a | b,
                };
                last.alternate = last.alternate.max(lookup.alternate);
            }
            _ => merged.push(lookup),
        }
    }
    // Drop lookups that do not exist or have a type nothing applies;
    // note the digests of the others.
    merged.retain_mut(|planned| match eng.lookup(planned.index) {
        Some(lookup) => {
            planned.first_digest = lookup.first.digest();
            if let Some(second) = lookup.second_components.as_ref() {
                planned.second_digest = second.digest();
            }
            true
        }
        None => false,
    });
    merged
}
