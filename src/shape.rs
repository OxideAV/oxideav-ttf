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

use crate::tables::gpos::PosRecord;
use crate::Font;

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

        // --- 2. GSUB substitution stage -----------------------------------
        self.run_gsub(&mut buf, script, lang, features);

        // --- 3. GPOS positioning stage ------------------------------------
        self.run_gpos(buf, script, lang, features)
    }

    /// Resolve the active GSUB lookup indices for the requested features
    /// and apply them, in LookupList order, across `buf`.
    fn run_gsub(
        &self,
        buf: &mut Vec<WorkGlyph>,
        script: [u8; 4],
        lang: Option<[u8; 4]>,
        features: &[[u8; 4]],
    ) {
        if self.gsub.is_none() {
            return;
        }
        let resolved = self.gsub_features_for_script_at_instance(script, lang);
        // Gather the union of lookup indices referenced by every active
        // requested feature.
        let mut active: Vec<u16> = Vec::new();
        for feat in &resolved {
            if !features.contains(&feat.tag) {
                continue;
            }
            for &li in &feat.lookup_indices {
                if !active.contains(&li) {
                    active.push(li);
                }
            }
        }
        if active.is_empty() {
            return;
        }
        // Process in LookupList order, not feature order.
        active.sort_unstable();

        // Map each active lookup index to its (effective) type so we can
        // pick the right per-type apply path.
        let types = self.gsub_lookup_list();
        for &li in &active {
            let kind = types
                .iter()
                .find(|(idx, _, _)| *idx == li)
                .map(|(_, k, _)| *k)
                .unwrap_or(0);
            self.apply_gsub_lookup(buf, li, kind);
        }
    }

    /// Apply one GSUB lookup of the given effective `kind` across the
    /// whole buffer.
    fn apply_gsub_lookup(&self, buf: &mut Vec<WorkGlyph>, li: u16, kind: u16) {
        match kind {
            1 => {
                // Single substitution: 1:1, no length change.
                for w in buf.iter_mut() {
                    if let Some(g) = self.gsub_apply_lookup_type_1(li, w.gid) {
                        w.gid = g;
                    }
                }
            }
            2 => {
                // Multiple substitution: 1 → N (or 0 = deletion). All
                // outputs inherit the source cluster.
                let mut out: Vec<WorkGlyph> = Vec::with_capacity(buf.len());
                let mut growth = 0usize;
                for w in buf.iter() {
                    match self.gsub_apply_lookup_type_2(li, w.gid) {
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
                }
            }
            3 => {
                // Alternate substitution: default to alternate 0.
                for w in buf.iter_mut() {
                    if let Some(g) = self.gsub_apply_lookup_type_3(li, w.gid, 0) {
                        w.gid = g;
                    }
                }
            }
            4 => {
                // Ligature substitution: N → 1, consuming a prefix from
                // each position. The ligature inherits the cluster of its
                // first component.
                let mut i = 0usize;
                while i < buf.len() {
                    let tail: Vec<u16> = buf[i..].iter().map(|w| w.gid).collect();
                    if let Some((lig, consumed)) = self.gsub_apply_lookup_type_4(li, &tail) {
                        if consumed >= 1 {
                            let cluster = buf[i].cluster;
                            buf[i] = WorkGlyph { gid: lig, cluster };
                            // Remove the remaining consumed components.
                            for _ in 1..consumed {
                                if i + 1 < buf.len() {
                                    buf.remove(i + 1);
                                }
                            }
                            i += 1;
                            continue;
                        }
                    }
                    i += 1;
                }
            }
            5 => {
                // Contextual substitution. apply_lookup_type_5 returns the
                // rewritten run (full buffer) on a hit at `pos`.
                let mut pos = 0usize;
                while pos < buf.len() {
                    let gids: Vec<u16> = buf.iter().map(|w| w.gid).collect();
                    if let Some(rewritten) = self.gsub_apply_lookup_type_5(li, &gids, pos) {
                        self.reconcile_context_rewrite(buf, &gids, rewritten, pos);
                    }
                    pos += 1;
                }
            }
            6 => {
                // Chained-context substitution.
                let mut pos = 0usize;
                while pos < buf.len() {
                    let gids: Vec<u16> = buf.iter().map(|w| w.gid).collect();
                    if let Some(rewritten) = self.gsub_apply_lookup_type_6(li, &gids, pos) {
                        self.reconcile_context_rewrite(buf, &gids, rewritten, pos);
                    }
                    pos += 1;
                }
            }
            8 => {
                // Reverse chained-context single substitution: 1:1, walked
                // right-to-left so a later substitution's lookahead sees
                // the original (not yet substituted) glyphs.
                let gids: Vec<u16> = buf.iter().map(|w| w.gid).collect();
                for pos in (0..buf.len()).rev() {
                    if let Some(g) = self.gsub_apply_lookup_type_8(li, &gids, pos) {
                        buf[pos].gid = g;
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
        new: Vec<u16>,
        pos: usize,
    ) {
        if new == old {
            return;
        }
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
    /// the active GPOS lookups in LookupList order.
    fn run_gpos(
        &self,
        buf: Vec<WorkGlyph>,
        script: [u8; 4],
        lang: Option<[u8; 4]>,
        features: &[[u8; 4]],
    ) -> Vec<ShapedGlyph> {
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

        if self.gpos.is_none() {
            return out;
        }
        let resolved = self.gpos_features_for_script_at_instance(script, lang);
        let mut active: Vec<u16> = Vec::new();
        for feat in &resolved {
            if !features.contains(&feat.tag) {
                continue;
            }
            for &li in &feat.lookup_indices {
                if !active.contains(&li) {
                    active.push(li);
                }
            }
        }
        if active.is_empty() {
            return out;
        }
        active.sort_unstable();

        let types = self.gpos_lookup_list();
        for &li in &active {
            let kind = types
                .iter()
                .find(|(idx, _, _)| *idx == li)
                .map(|(_, k, _)| *k)
                .unwrap_or(0);
            self.apply_gpos_lookup(&mut out, li, kind);
        }
        out
    }

    /// Apply one GPOS lookup of the given effective `kind` across the
    /// positioned buffer.
    fn apply_gpos_lookup(&self, out: &mut [ShapedGlyph], li: u16, kind: u16) {
        match kind {
            1 => {
                // Single adjustment.
                for g in out.iter_mut() {
                    if let Some(v) = self.gpos_apply_lookup_type_1(li, g.glyph_id) {
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
                // the left glyph of each adjacent pair. We apply it to the
                // first glyph of each consecutive pair.
                let gdef = self.gdef.as_ref();
                for i in 0..out.len().saturating_sub(1) {
                    let left = out[i].glyph_id;
                    let right = out[i + 1].glyph_id;
                    let adj = self
                        .gpos
                        .as_ref()
                        .map(|g| g.lookup_kerning_at(li, left, right, gdef))
                        .unwrap_or(0);
                    out[i].x_advance += adj as i32;
                }
            }
            3 => {
                // Cursive attachment: glyph N+1's entry anchor lands on
                // glyph N's exit anchor. The per-glyph delta moves N+1 so
                // its entry aligns with N's exit (x via offset, the
                // baseline shift via y_offset).
                let mut prev_exit: Option<(i16, i16)> = None;
                for g in out.iter_mut() {
                    if let Some(att) = self.gpos_apply_lookup_type_3(li, g.glyph_id) {
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
            4 => {
                // Mark-to-base: a mark glyph attaches to the nearest
                // preceding base glyph.
                self.apply_mark_attach(out, li, false);
            }
            5 => {
                // Mark-to-ligature: a mark attaches to a component of a
                // preceding ligature. We attach to the last preceding
                // ligature, component 0 (a reasonable default without
                // per-component cluster tracking from the substitution
                // stage); the per-lookup apply path handles component
                // resolution when given an explicit component.
                self.apply_mark_to_ligature(out, li);
            }
            6 => {
                // Mark-to-mark: a mark attaches to the immediately
                // preceding mark.
                self.apply_mark_attach(out, li, true);
            }
            7 => {
                // Contextual positioning.
                let gids: Vec<u16> = out.iter().map(|g| g.glyph_id).collect();
                for pos in 0..out.len() {
                    if let Some(records) = self.gpos_apply_lookup_type_7(li, &gids, pos) {
                        apply_pos_records(out, &records);
                    }
                }
            }
            8 => {
                // Chained-context positioning.
                let gids: Vec<u16> = out.iter().map(|g| g.glyph_id).collect();
                for pos in 0..out.len() {
                    if let Some(records) = self.gpos_apply_lookup_type_8(li, &gids, pos) {
                        apply_pos_records(out, &records);
                    }
                }
            }
            _ => {}
        }
    }

    /// Shared mark-to-base (`to_mark = false`) / mark-to-mark
    /// (`to_mark = true`) attachment. For each mark glyph, find the
    /// nearest preceding attachment glyph (a base for mark-to-base, a
    /// mark for mark-to-mark) that the lookup binds it to, and offset the
    /// mark so its anchor lands on the base's anchor.
    fn apply_mark_attach(&self, out: &mut [ShapedGlyph], li: u16, to_mark: bool) {
        for i in 0..out.len() {
            let mark = out[i].glyph_id;
            // Scan backwards for the attachment glyph.
            for j in (0..i).rev() {
                let base = out[j].glyph_id;
                let hit = if to_mark {
                    self.gpos
                        .as_ref()
                        .and_then(|g| g.apply_mark_to_mark_at(li, base, mark))
                } else {
                    self.gpos
                        .as_ref()
                        .and_then(|g| g.apply_mark_to_base_at(li, base, mark))
                };
                if let Some((dx, dy)) = hit {
                    // Place the mark relative to the base's pen origin.
                    // The base sits at the accumulated advance from j to i;
                    // a mark has (typically) zero advance, so its drawing
                    // origin is the current pen. We express attachment as a
                    // placement offset that pulls the mark back over the
                    // base by the base's advance run plus the anchor delta.
                    let between: i32 = out[j..i].iter().map(|g| g.x_advance).sum();
                    out[i].x_offset += dx as i32 - between;
                    out[i].y_offset += dy as i32;
                    break;
                }
                // For mark-to-base, stop at the first non-mark glyph
                // (the base) regardless of a hit; for mark-to-mark keep
                // scanning only across marks.
                if !to_mark && !self.is_mark_glyph(base) {
                    break;
                }
                if to_mark && !self.is_mark_glyph(base) {
                    break;
                }
            }
        }
    }

    /// Mark-to-ligature attachment (LookupType 5). Attaches each mark to
    /// the nearest preceding ligature glyph at component 0.
    fn apply_mark_to_ligature(&self, out: &mut [ShapedGlyph], li: u16) {
        for i in 0..out.len() {
            let mark = out[i].glyph_id;
            for j in (0..i).rev() {
                let lig = out[j].glyph_id;
                if let Some((dx, dy)) = self
                    .gpos
                    .as_ref()
                    .and_then(|g| g.apply_lookup_type_5(li, lig, 0, mark))
                {
                    let between: i32 = out[j..i].iter().map(|g| g.x_advance).sum();
                    out[i].x_offset += dx as i32 - between;
                    out[i].y_offset += dy as i32;
                    break;
                }
                if !self.is_mark_glyph(lig) {
                    break;
                }
            }
        }
    }
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
