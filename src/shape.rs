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
//! flat per-glyph GDEF class array. See [`crate::layout`] internals for
//! the invariants that keep the filtered path output-identical to
//! walking every lookup at every position.

use std::sync::Arc;

use crate::Font;
use crate::layout::{
    LayoutTable, LookupAccel, PlanLookup, ShapePlan, digest_bit, flatten_class_def,
};
use crate::tables::gpos::PosRecord;

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
    classes: Option<&'f [u8]>,
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
        let active = font.gdef().is_some() && (flags & 0xFF1E != 0 || mark_filtering_set.is_some());
        Self {
            font,
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
        match class as u16 {
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
}

/// First-glyph filter of one lookup: `None` means "no accelerator —
/// every glyph may match".
#[derive(Clone, Copy)]
struct StartFilter<'f>(Option<&'f LookupAccel>);

impl StartFilter<'_> {
    /// `false` only when the lookup provably cannot act at `gid`.
    #[inline]
    fn may_start(&self, gid: u16) -> bool {
        self.0.is_none_or(|a| a.first.contains(gid))
    }

    /// `true` when no glyph summarised by `run_digest` can start a match.
    #[inline]
    fn rules_out(&self, run_digest: u64) -> bool {
        self.0.is_some_and(|a| a.first.disjoint_from(run_digest))
    }

    /// Longest glyph sequence one ligature match can inspect.
    fn max_components(&self) -> usize {
        self.0.map_or(usize::MAX, |a| a.max_components)
    }
}

fn digest_of(gids: impl Iterator<Item = u16>) -> u64 {
    gids.fold(0u64, |d, g| d | digest_bit(g))
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
        // --- 1. character-to-glyph mapping --------------------------------
        let mut buf: Vec<WorkGlyph> = Vec::with_capacity(text.len());
        for (byte_idx, ch) in text.char_indices() {
            let gid = self.glyph_index(ch).unwrap_or(0);
            buf.push(WorkGlyph {
                gid,
                cluster: byte_idx as u32,
            });
        }

        let plan = self.shape_plan(script, lang, features);

        // --- 2. GSUB substitution stage -----------------------------------
        if !plan.gsub.is_empty() {
            self.run_gsub(&mut buf, &plan.gsub);
        }

        // --- 3. GPOS positioning stage ------------------------------------
        self.run_gpos(buf, &plan.gpos)
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
            .map(|li| PlanLookup {
                index: li,
                kind: lookup_kind(&types, li),
                flags: gsub.lookup_flags(li),
                mark_filtering_set: gsub.mark_filtering_set(li),
            })
            // Types the substitution stage has no apply path for.
            .filter(|l| matches!(l.kind, 1..=6 | 8))
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
            .map(|li| PlanLookup {
                index: li,
                kind: lookup_kind(&types, li),
                flags: gpos.lookup_flags(li),
                mark_filtering_set: gpos.mark_filtering_set(li),
            })
            .filter(|l| matches!(l.kind, 1..=8))
            .collect()
    }

    /// Flat GDEF GlyphClassDef classes (one byte per glyph), or `None`
    /// when the font has no GDEF or its class table is not one the flat
    /// form reproduces exactly.
    fn glyph_classes(&self) -> Option<&[u8]> {
        self.layout.glyph_classes(|| {
            let gdef = self.gdef()?;
            let num_glyphs = self.maxp.num_glyphs as usize;
            match gdef.glyph_class_def_bytes() {
                Some(class_def) => flatten_class_def(class_def, num_glyphs),
                // No GlyphClassDef: every glyph is class 0.
                None => Some(vec![0u8; num_glyphs].into_boxed_slice()),
            }
        })
    }

    fn gsub_start_filter(&self, lookup: &PlanLookup) -> StartFilter<'_> {
        StartFilter(self.gsub().and_then(|g| {
            let (bytes, lookup_list_off) = g.lookup_list_location();
            self.layout.lookup_accel(
                LayoutTable::Gsub,
                bytes,
                lookup_list_off,
                lookup.index,
                lookup.kind,
                self.maxp.num_glyphs as usize,
            )
        }))
    }

    fn gpos_start_filter(&self, lookup: &PlanLookup) -> StartFilter<'_> {
        StartFilter(self.gpos().and_then(|g| {
            let (bytes, lookup_list_off) = g.lookup_list_location();
            self.layout.lookup_accel(
                LayoutTable::Gpos,
                bytes,
                lookup_list_off,
                lookup.index,
                lookup.kind,
                self.maxp.num_glyphs as usize,
            )
        }))
    }

    /// Apply the planned GSUB lookups, in LookupList order, across `buf`.
    fn run_gsub(&self, buf: &mut Vec<WorkGlyph>, lookups: &[PlanLookup]) {
        // Digest of every glyph id the buffer holds (or has held — bits
        // are only ever added, which keeps it a superset).
        let mut run_digest = digest_of(buf.iter().map(|w| w.gid));
        // Scratch reused across lookups.
        let mut gids: Vec<u16> = Vec::new();
        let mut cand_gids: Vec<u16> = Vec::new();
        let mut cand_idx: Vec<usize> = Vec::new();
        for lookup in lookups {
            let filter = self.gsub_start_filter(lookup);
            if filter.rules_out(run_digest) {
                continue;
            }
            self.apply_gsub_lookup(
                buf,
                lookup,
                filter,
                &mut run_digest,
                &mut gids,
                &mut cand_gids,
                &mut cand_idx,
            );
        }
    }

    /// Apply one GSUB lookup across the whole buffer. The lookup's
    /// IGNORE_MARKS / IGNORE_BASE_GLYPHS / IGNORE_LIGATURES skip bits
    /// are honoured where they affect substitution (most consequentially
    /// IGNORE_MARKS on ligature lookups, so a combining mark sitting
    /// between two ligature components doesn't block the ligature).
    ///
    /// `filter` prunes positions whose glyph the lookup's first-glyph
    /// Coverage tables do not list; the match code would reject those
    /// anyway. `run_digest` gains the digest bit of every glyph written.
    #[allow(clippy::too_many_arguments)]
    fn apply_gsub_lookup(
        &self,
        buf: &mut Vec<WorkGlyph>,
        lookup: &PlanLookup,
        filter: StartFilter<'_>,
        run_digest: &mut u64,
        gids: &mut Vec<u16>,
        cand_gids: &mut Vec<u16>,
        cand_idx: &mut Vec<usize>,
    ) {
        let li = lookup.index;
        match lookup.kind {
            1 => {
                // Single substitution: 1:1, no length change.
                for w in buf.iter_mut() {
                    if filter.may_start(w.gid)
                        && let Some(g) = self.gsub_apply_lookup_type_1(li, w.gid)
                    {
                        w.gid = g;
                        *run_digest |= digest_bit(g);
                    }
                }
            }
            2 => {
                // Multiple substitution: 1 → N (or 0 = deletion). All
                // outputs inherit the source cluster.
                if !buf.iter().any(|w| filter.may_start(w.gid)) {
                    return;
                }
                let mut out: Vec<WorkGlyph> = Vec::with_capacity(buf.len());
                let mut growth = 0usize;
                for w in buf.iter() {
                    let expansion = if filter.may_start(w.gid) {
                        self.gsub_apply_lookup_type_2(li, w.gid)
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
                    *run_digest |= digest_of(buf.iter().map(|w| w.gid));
                }
            }
            3 => {
                // Alternate substitution: default to alternate 0.
                for w in buf.iter_mut() {
                    if filter.may_start(w.gid)
                        && let Some(g) = self.gsub_apply_lookup_type_3(li, w.gid, 0)
                    {
                        w.gid = g;
                        *run_digest |= digest_bit(g);
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
                let window = filter.max_components().max(1);
                let mut i = 0usize;
                while i < buf.len() {
                    let first = buf[i].gid;
                    if !filter.may_start(first) || skip.skips(first) {
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
                    if let Some((lig, consumed)) = self.gsub_apply_lookup_type_4(li, cand_gids)
                        && consumed >= 1
                    {
                        let cluster = buf[i].cluster;
                        buf[i] = WorkGlyph { gid: lig, cluster };
                        *run_digest |= digest_bit(lig);
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
                // Contextual (5) / chained-context (6) substitution. The
                // per-type apply returns the rewritten run (full buffer)
                // on a hit at `pos`. `gids` mirrors the buffer's glyph
                // ids and is refreshed only when a rewrite changes them.
                gids.clear();
                gids.extend(buf.iter().map(|w| w.gid));
                let mut pos = 0usize;
                while pos < buf.len() {
                    if filter.may_start(gids[pos]) {
                        let rewritten = if lookup.kind == 5 {
                            self.gsub_apply_lookup_type_5(li, gids, pos)
                        } else {
                            self.gsub_apply_lookup_type_6(li, gids, pos)
                        };
                        if let Some(rewritten) = rewritten
                            && rewritten != *gids
                        {
                            self.reconcile_context_rewrite(buf, gids, &rewritten, pos);
                            *run_digest |= digest_of(rewritten.iter().copied());
                            *gids = rewritten;
                        }
                    }
                    pos += 1;
                }
            }
            8 => {
                // Reverse chained-context single substitution: 1:1, walked
                // right-to-left so a later substitution's lookahead sees
                // the original (not yet substituted) glyphs.
                gids.clear();
                gids.extend(buf.iter().map(|w| w.gid));
                for pos in (0..buf.len()).rev() {
                    if filter.may_start(gids[pos])
                        && let Some(g) = self.gsub_apply_lookup_type_8(li, gids, pos)
                    {
                        buf[pos].gid = g;
                        *run_digest |= digest_bit(g);
                    }
                }
            }
            _ => {}
        }
    }

    /// Reconcile a contextual/chained GSUB rewrite (which returns a full
    /// rewritten gid run) back into the `WorkGlyph` buffer, preserving
    /// clusters as best we can. The rewrite may change the buffer length
    /// (a nested multiple- or ligature-substitution record). We align the
    /// unchanged prefix/suffix and assign the source cluster of `pos` to
    /// any glyphs in the changed middle.
    fn reconcile_context_rewrite(
        &self,
        buf: &mut Vec<WorkGlyph>,
        old: &[u16],
        new: &[u16],
        pos: usize,
    ) {
        // Common unchanged prefix.
        let mut pre = 0usize;
        while pre < old.len() && pre < new.len() && old[pre] == new[pre] {
            pre += 1;
        }
        // Common unchanged suffix.
        let mut suf = 0usize;
        while suf < (old.len() - pre)
            && suf < (new.len() - pre)
            && old[old.len() - 1 - suf] == new[new.len() - 1 - suf]
        {
            suf += 1;
        }
        let cluster = buf.get(pos).map(|w| w.cluster).unwrap_or(0);
        let mut rebuilt: Vec<WorkGlyph> = Vec::with_capacity(new.len());
        for &g in &new[..pre] {
            let c = buf.get(rebuilt.len()).map(|w| w.cluster).unwrap_or(cluster);
            rebuilt.push(WorkGlyph { gid: g, cluster: c });
        }
        for &g in &new[pre..new.len() - suf] {
            rebuilt.push(WorkGlyph { gid: g, cluster });
        }
        let suffix_start_old = old.len() - suf;
        for (k, &g) in new[new.len() - suf..].iter().enumerate() {
            let c = buf
                .get(suffix_start_old + k)
                .map(|w| w.cluster)
                .unwrap_or(cluster);
            rebuilt.push(WorkGlyph { gid: g, cluster: c });
        }
        *buf = rebuilt;
    }

    /// GPOS positioning stage. Seeds advances from `hmtx`, then applies
    /// the planned GPOS lookups in LookupList order.
    fn run_gpos(&self, buf: Vec<WorkGlyph>, lookups: &[PlanLookup]) -> Vec<ShapedGlyph> {
        // Seed every glyph with its nominal horizontal advance.
        let mut out: Vec<ShapedGlyph> = buf
            .iter()
            .map(|w| ShapedGlyph {
                glyph_id: w.gid,
                cluster: w.cluster,
                x_offset: 0,
                y_offset: 0,
                x_advance: self.glyph_advance(w.gid) as i32,
                y_advance: 0,
            })
            .collect();
        if lookups.is_empty() {
            return out;
        }
        // Positioning never changes glyph ids: one digest and one glyph
        // id array serve every lookup.
        let run_digest = digest_of(out.iter().map(|g| g.glyph_id));
        let mut gids: Vec<u16> = Vec::new();
        for lookup in lookups {
            let filter = self.gpos_start_filter(lookup);
            if filter.rules_out(run_digest) {
                continue;
            }
            self.apply_gpos_lookup(&mut out, lookup, filter, &mut gids);
        }
        out
    }

    /// Apply one GPOS lookup across the positioned buffer. `filter`
    /// prunes positions whose glyph is not in the lookup's first-glyph
    /// Coverage tables (for the mark-attachment types that is the
    /// MarkCoverage — the attaching mark is the "current" glyph).
    fn apply_gpos_lookup(
        &self,
        out: &mut [ShapedGlyph],
        lookup: &PlanLookup,
        filter: StartFilter<'_>,
        gids: &mut Vec<u16>,
    ) {
        let li = lookup.index;
        match lookup.kind {
            1 => {
                // Single adjustment.
                for g in out.iter_mut() {
                    if filter.may_start(g.glyph_id)
                        && let Some(v) = self.gpos_apply_lookup_type_1(li, g.glyph_id)
                    {
                        g.x_offset += v.x_placement as i32;
                        g.y_offset += v.y_placement as i32;
                        g.x_advance += v.x_advance as i32;
                        g.y_advance += v.y_advance as i32;
                    }
                }
            }
            2 => {
                // Pair adjustment (kerning). The legacy single-value
                // `lookup_kerning` path extracts the x-advance applied to
                // the left glyph of each pair. The pair members are the
                // current glyph and the *next non-skipped* glyph per the
                // lookup's §2 skip filter — so a kern pair separated by an
                // (ignored) combining mark still kerns, the canonical
                // IGNORE_MARKS-on-kern case.
                let gdef = self.gdef();
                let skip = SkipFilter::new(self, lookup.flags, lookup.mark_filtering_set);
                let Some(gpos) = self.gpos() else {
                    return;
                };
                for i in 0..out.len() {
                    let left = out[i].glyph_id;
                    if !filter.may_start(left) || skip.skips(left) {
                        continue;
                    }
                    // Find the next glyph the lookup does not skip.
                    let Some(right_idx) =
                        ((i + 1)..out.len()).find(|&k| !skip.skips(out[k].glyph_id))
                    else {
                        break;
                    };
                    let right = out[right_idx].glyph_id;
                    out[i].x_advance += gpos.lookup_kerning_at(li, left, right, gdef) as i32;
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
                    let attachment = if filter.may_start(g.glyph_id) {
                        self.gpos_apply_lookup_type_3(li, g.glyph_id)
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
            4..=6 => self.apply_mark_attach(out, lookup, filter),
            7 | 8 => {
                // Contextual (7) / chained-context (8) positioning.
                gids.clear();
                gids.extend(out.iter().map(|g| g.glyph_id));
                for pos in 0..out.len() {
                    if !filter.may_start(gids[pos]) {
                        continue;
                    }
                    let records = if lookup.kind == 7 {
                        self.gpos_apply_lookup_type_7(li, gids, pos)
                    } else {
                        self.gpos_apply_lookup_type_8(li, gids, pos)
                    };
                    if let Some(records) = records {
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
        out: &mut [ShapedGlyph],
        lookup: &PlanLookup,
        filter: StartFilter<'_>,
    ) {
        let li = lookup.index;
        let Some(gpos) = self.gpos() else {
            return;
        };
        let skip = SkipFilter::new(self, lookup.flags, lookup.mark_filtering_set);
        for i in 0..out.len() {
            let mark = out[i].glyph_id;
            // Only a glyph in one of the lookup's MarkCoverage tables can
            // attach.
            if !filter.may_start(mark) || skip.skips(mark) {
                continue;
            }
            // The nearest non-skipped predecessor is the only attachment
            // candidate, whether or not it produces a hit.
            let Some(j) = (0..i).rev().find(|&j| !skip.skips(out[j].glyph_id)) else {
                continue;
            };
            let target = out[j].glyph_id;
            let hit = match lookup.kind {
                4 => gpos.apply_mark_to_base_at(li, target, mark),
                5 => gpos.apply_lookup_type_5(li, target, 0, mark),
                _ => gpos.apply_mark_to_mark_at(li, target, mark),
            };
            if let Some((dx, dy)) = hit {
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
