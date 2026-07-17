//! `GSUB` — Glyph Substitution Table.
//!
//! Implemented lookup types:
//! - **LookupType 1** (Single Substitution) — formats 1 (delta) and 2
//!   (indexed substitute array). Used by Arabic shaping (`init`/`medi`/
//!   `fina`/`isol`), small-caps, vertical alternates, and most other
//!   one-in/one-out feature lookups.
//! - **LookupType 2** (Multiple Substitution) — format 1. Splits one
//!   input glyph into a sequence of N replacement glyphs (`Sequence`
//!   record per coverage index). Used for some script normalisations
//!   (e.g. expanding a precomposed glyph into its base + mark cluster).
//! - **LookupType 3** (Alternate Substitution) — format 1. Each
//!   covered input glyph carries an `AlternateSet` of substitute
//!   glyphs; the caller picks an index (default 0). Used by `aalt` /
//!   `salt` for stylistic alternates.
//! - **LookupType 4** (Ligature Substitution) — format 1. Both the
//!   "walk every lookup" entry point ([`GsubTable::lookup_ligature`])
//!   and the lookup-index-specific entry point
//!   ([`GsubTable::apply_lookup_type_4`]) are exposed; the latter is
//!   how a feature-driven shaper dispatches `liga` / `rlig` / `dlig`.
//! - **LookupType 5** (Contextual Substitution) — formats 1 (glyph
//!   sequence), 2 (class-based) and 3 (coverage-based). Predecessor of
//!   LookupType 6 minus backtrack/lookahead — the input window is the
//!   only context. Older fonts encode contextual rules here.
//! - **LookupType 6** (Chained Contexts Substitution) — formats 1
//!   (glyph sequence), 2 (class-based) and 3 (coverage-based). Each
//!   match runs the referenced sub-lookups (typically LookupType 1 or
//!   LookupType 4) at the recorded sequence positions and returns the
//!   rewritten glyph run via [`GsubTable::apply_lookup_type_6`].
//! - **LookupType 8** (Reverse Chained Context Substitution) — format
//!   1. A single-glyph substitution under
//!      `(backtrack, input_coverage, lookahead)` context, processed in
//!      reverse text order. Used by some Arabic fonts for isolated forms.
//!
//! ExtensionSubst (LookupType 7) is unwrapped transparently for every
//! lookup type above.
//!
//! In addition to the per-lookup walkers, this module decodes the
//! **ScriptList** + **FeatureList** at parse time so callers can ask
//! "which lookup indices implement feature `init` for script `arab`?"
//! via [`Font::gsub_features_for_script`]. The lookup-index list is
//! the bridge between feature tags (what a shaper asks for) and the
//! per-lookup substitution machinery (what changes the glyph stream).
//!
//! Spec: Microsoft OpenType §"GSUB — Glyph Substitution Table",
//! §"Common Table Formats" (ScriptList / FeatureList / LookupList),
//! Apple TrueType Reference §"GSUB", ISO/IEC 14496-22 §6 (OFF).

use crate::parser::{read_u16, read_u32};
use crate::tables::gdef::{class_def_lookup, coverage_lookup, lookup_table_slice};
use crate::Error;

const LOOKUP_SINGLE_SUBST: u16 = 1;
const LOOKUP_MULTIPLE_SUBST: u16 = 2;
const LOOKUP_ALTERNATE_SUBST: u16 = 3;
const LOOKUP_LIGATURE_SUBST: u16 = 4;
const LOOKUP_CONTEXT_SUBST: u16 = 5;
const LOOKUP_CHAIN_CONTEXT_SUBST: u16 = 6;
const LOOKUP_EXTENSION_SUBST: u16 = 7;
const LOOKUP_REVERSE_CHAIN_CONTEXT_SUBST: u16 = 8;

/// Maximum recursion depth for nested chained-context substitutions.
/// Prevents pathological self-referential lookup graphs from blowing
/// the stack — the spec doesn't bound this so we set a conservative
/// fence well below stack-blow territory.
const MAX_NESTED_LOOKUP_DEPTH: u8 = 8;

/// One feature record from the GSUB FeatureList, resolved to the
/// list of lookup indices that implement it. Returned by
/// [`super::super::Font::gsub_features_for_script`] in the order the
/// active LangSys lists its features.
///
/// The `tag` field is a four-byte ASCII feature identifier such as
/// `*b"init"`, `*b"medi"`, `*b"fina"`, `*b"isol"`, `*b"liga"`,
/// `*b"smcp"` — the OpenType registered-feature catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GsubFeature {
    pub tag: [u8; 4],
    pub lookup_indices: Vec<u16>,
}

#[derive(Debug, Clone)]
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub struct GsubTable<'a> {
    bytes: &'a [u8],
    script_list_off: u32,
    feature_list_off: u32,
    lookup_list_off: u32,
    /// `Offset32 featureVariationsOffset` from a version-1.1 header
    /// (ISO/IEC 14496-22:2019 §6.2.9). `0` for v1.0 fonts and for v1.1
    /// fonts that ship no feature variations.
    feature_variations_off: u32,
}

impl<'a> GsubTable<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < 10 {
            return Err(Error::UnexpectedEof);
        }
        let major = read_u16(bytes, 0)?;
        if major != 1 {
            return Err(Error::BadStructure("GSUB: unsupported major version"));
        }
        // u16 minor at +2 (we tolerate 0 or 1)
        // Offset16 scriptList at +4
        // Offset16 featureList at +6
        // Offset16 lookupList at +8
        // Offset32 featureVariations at +10 (version 1.1 only)
        let minor = read_u16(bytes, 2)?;
        let script_list_off = read_u16(bytes, 4)? as u32;
        let feature_list_off = read_u16(bytes, 6)? as u32;
        let lookup_list_off = read_u16(bytes, 8)? as u32;
        // The v1.1 header (§6.2.9) carries a 4-byte featureVariations
        // offset after the three v1.0 offsets. v1.0 fonts stop at +10.
        let feature_variations_off = if minor >= 1 && bytes.len() >= 14 {
            read_u32(bytes, 10)?
        } else {
            0
        };
        // Each offset must either be 0 (table absent) or fit inside `bytes`.
        for off in [
            script_list_off,
            feature_list_off,
            lookup_list_off,
            feature_variations_off,
        ] {
            if off != 0 && off as usize >= bytes.len() {
                return Err(Error::BadOffset);
            }
        }
        Ok(Self {
            bytes,
            script_list_off,
            feature_list_off,
            lookup_list_off,
            feature_variations_off,
        })
    }

    /// Enumerate every lookup in the LookupList as
    /// `(lookup_index, lookup_type, subtable_count)`.
    ///
    /// The reported `lookup_type` is the **effective** type after
    /// unwrapping any LookupType-7 ExtensionSubst wrapper — i.e. the
    /// caller sees `4` for a ligature lookup whether it's stored as
    /// a plain LookupType-4 lookup or as a LookupType-7 wrapper. This
    /// lets downstream find lookups of a specific type without
    /// probing every index.
    pub fn lookup_list(&self) -> impl Iterator<Item = (u16, u16, u16)> + '_ {
        let lookup_count = if self.lookup_list_off == 0 {
            0
        } else {
            self.bytes
                .get(self.lookup_list_off as usize..)
                .and_then(|s| read_u16(s, 0).ok())
                .unwrap_or(0)
        };
        (0..lookup_count).filter_map(move |i| {
            let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, i)?;
            if lookup.len() < 6 {
                return None;
            }
            let mut kind = read_u16(lookup, 0).ok()?;
            let sub_count = read_u16(lookup, 4).ok()?;
            if kind == LOOKUP_EXTENSION_SUBST && sub_count > 0 {
                if let Some(t) = peek_extension_type(lookup) {
                    kind = t;
                }
            }
            Some((i, kind, sub_count))
        })
    }

    /// Return the `lookupFlag` of lookup `lookup_index`, or `0` when the
    /// index is out of range / the table has no LookupList.
    ///
    /// The Lookup table layout (OFF §6.2.2) is
    /// `{ u16 lookupType, u16 lookupFlag, u16 subTableCount, … }`, so the
    /// flag sits at offset +2. The low-byte bits are the skip filters a
    /// shaper honours when matching a lookup's input — RIGHT_TO_LEFT
    /// (`0x0001`, cursive-only), IGNORE_BASE_GLYPHS (`0x0002`),
    /// IGNORE_LIGATURES (`0x0004`), IGNORE_MARKS (`0x0008`),
    /// USE_MARK_FILTERING_SET (`0x0010`) — and the high byte is the
    /// `markAttachmentType` class. The flag is read from the *outer*
    /// lookup (the LookupType-7 ExtensionSubst wrapper, when present,
    /// carries the operative flag, not the wrapped inner lookup).
    pub fn lookup_flags(&self, lookup_index: u16) -> u16 {
        lookup_table_slice(self.bytes, self.lookup_list_off, lookup_index)
            .filter(|l| l.len() >= 6)
            .and_then(|l| read_u16(l, 2).ok())
            .unwrap_or(0)
    }

    /// Return the `markFilteringSet` index of lookup `lookup_index`, or
    /// `None` when the lookup does not carry the `USE_MARK_FILTERING_SET`
    /// (`0x0010`) bit.
    ///
    /// Per the §6.2.2 Lookup table layout, `markFilteringSet` is the
    /// trailing `uint16` after the `subtableOffsets[subTableCount]` array
    /// — i.e. at byte offset `6 + 2 * subTableCount` — and is "only
    /// present if the USE_MARK_FILTERING_SET lookup flag is set". The
    /// value indexes the GDEF `MarkGlyphSets` structure; the layout
    /// engine then skips all mark glyphs *not* in that set.
    pub fn mark_filtering_set(&self, lookup_index: u16) -> Option<u16> {
        let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, lookup_index)?;
        let flags = read_u16(lookup, 2).ok()?;
        if flags & 0x0010 == 0 {
            return None;
        }
        let sub_count = read_u16(lookup, 4).ok()? as usize;
        read_u16(lookup, 6 + sub_count * 2).ok()
    }

    /// Return all features active for `script_tag` under `lang_tag`.
    ///
    /// `lang_tag = None` → use the script's `DefaultLangSys`. If
    /// `lang_tag` is supplied but isn't enumerated for the script, we
    /// fall back to `DefaultLangSys`. If neither resolves the script
    /// at all (or the table has no ScriptList) the returned `Vec` is
    /// empty.
    ///
    /// The order of returned features matches the LangSys's
    /// `featureIndices` array order (which is the order a shaper
    /// should apply them). Each `GsubFeature` carries the resolved
    /// lookup-index list ready for [`Self::apply_lookup_type_1`].
    pub fn features_for_script(
        &self,
        script_tag: [u8; 4],
        lang_tag: Option<[u8; 4]>,
    ) -> Vec<GsubFeature> {
        self.features_for_script_inner(script_tag, lang_tag, None)
    }

    /// Like [`Self::features_for_script`], but applies the §6.2.9
    /// FeatureVariations substitution active at `normalised_coords` (the
    /// avar-bent normalised axis vector, e.g.
    /// [`super::super::Font::normalised_coords`]).
    ///
    /// For each feature whose index is overridden by the matching
    /// FeatureTableSubstitution, the returned [`GsubFeature`] carries the
    /// alternate feature's lookup-index list instead of the default
    /// one — the feature tag stays the same per §6.2.9 ("An alternate
    /// feature table maintains the same feature tag association as the
    /// default feature table"). Non-substituted features are unchanged.
    ///
    /// Static fonts, v1.0 GSUB headers, and fonts whose feature
    /// variations match no record all behave identically to
    /// [`Self::features_for_script`].
    pub fn features_for_script_at_coords(
        &self,
        script_tag: [u8; 4],
        lang_tag: Option<[u8; 4]>,
        normalised_coords: &[f32],
    ) -> Vec<GsubFeature> {
        let fv = match super::feature_variations::FeatureVariations::parse(
            self.bytes,
            self.feature_variations_off,
        ) {
            Ok(Some(fv)) => fv,
            _ => return self.features_for_script_inner(script_tag, lang_tag, None),
        };
        let subst = fv.active_substitution(normalised_coords);
        self.features_for_script_inner(script_tag, lang_tag, subst.as_ref())
    }

    /// `true` when this GSUB table carries a non-empty
    /// FeatureVariations table (a v1.1 header with a non-zero offset).
    pub fn has_feature_variations(&self) -> bool {
        self.feature_variations_off != 0
    }

    fn features_for_script_inner(
        &self,
        script_tag: [u8; 4],
        lang_tag: Option<[u8; 4]>,
        subst: Option<&super::feature_variations::FeatureTableSubstitution<'a>>,
    ) -> Vec<GsubFeature> {
        let mut out = Vec::new();
        if self.script_list_off == 0 || self.feature_list_off == 0 {
            return out;
        }
        let script_list = match self.bytes.get(self.script_list_off as usize..) {
            Some(s) => s,
            None => return out,
        };
        let feature_list = match self.bytes.get(self.feature_list_off as usize..) {
            Some(s) => s,
            None => return out,
        };

        // ScriptList layout: u16 scriptCount; ScriptRecord{ Tag tag,
        // Offset16 scriptOffset } scriptRecords[scriptCount];
        // Each scriptOffset is RELATIVE to the ScriptList start.
        let script_count = match read_u16(script_list, 0) {
            Ok(v) => v as usize,
            Err(_) => return out,
        };
        if script_list.len() < 2 + script_count * 6 {
            return out;
        }
        let mut script_off: Option<usize> = None;
        for i in 0..script_count {
            let r = 2 + i * 6;
            let tag = [
                script_list[r],
                script_list[r + 1],
                script_list[r + 2],
                script_list[r + 3],
            ];
            if tag == script_tag {
                let o = match read_u16(script_list, r + 4) {
                    Ok(v) => v as usize,
                    Err(_) => return out,
                };
                script_off = Some(o);
                break;
            }
        }
        let script_off = match script_off {
            Some(o) => o,
            None => return out,
        };
        let script = match script_list.get(script_off..) {
            Some(s) => s,
            None => return out,
        };

        // Script layout:
        //   Offset16 defaultLangSysOffset    (0 if absent)
        //   u16      langSysCount
        //   LangSysRecord{ Tag tag, Offset16 langSysOffset }
        //                                    langSysRecords[langSysCount];
        // langSysOffsets are relative to the Script table start.
        if script.len() < 4 {
            return out;
        }
        let default_off = match read_u16(script, 0) {
            Ok(v) => v as usize,
            Err(_) => return out,
        };
        let lang_count = match read_u16(script, 2) {
            Ok(v) => v as usize,
            Err(_) => return out,
        };
        let mut chosen_off: Option<usize> = None;
        if let Some(want) = lang_tag {
            // Linear scan — there's only ever a handful of LangSysRecords.
            for i in 0..lang_count {
                let r = 4 + i * 6;
                if script.len() < r + 6 {
                    break;
                }
                let tag = [script[r], script[r + 1], script[r + 2], script[r + 3]];
                if tag == want {
                    chosen_off = match read_u16(script, r + 4) {
                        Ok(v) => Some(v as usize),
                        Err(_) => None,
                    };
                    break;
                }
            }
        }
        let chosen_off = chosen_off.or(if default_off == 0 {
            None
        } else {
            Some(default_off)
        });
        let chosen_off = match chosen_off {
            Some(o) if o != 0 => o,
            _ => return out,
        };
        let langsys = match script.get(chosen_off..) {
            Some(s) => s,
            None => return out,
        };

        // LangSys layout:
        //   Offset16 lookupOrderOffset (= 0 reserved)
        //   u16      requiredFeatureIndex (0xFFFF = none)
        //   u16      featureIndexCount
        //   u16      featureIndices[featureIndexCount]
        if langsys.len() < 6 {
            return out;
        }
        let required = match read_u16(langsys, 2) {
            Ok(v) => v,
            Err(_) => return out,
        };
        let feat_count = match read_u16(langsys, 4) {
            Ok(v) => v as usize,
            Err(_) => return out,
        };
        if langsys.len() < 6 + feat_count * 2 {
            return out;
        }

        // FeatureList layout:
        //   u16 featureCount;
        //   FeatureRecord{ Tag tag, Offset16 featureOffset } records[];
        // featureOffsets are relative to the FeatureList start.
        let total_features = match read_u16(feature_list, 0) {
            Ok(v) => v as usize,
            Err(_) => return out,
        };

        // Helper to resolve one feature index → GsubFeature. When a
        // §6.2.9 FeatureTableSubstitution is active and overrides this
        // feature index, the alternate feature's lookup-index list
        // replaces the default one (the tag is unchanged per spec).
        let push_feature = |fi: u16, into: &mut Vec<GsubFeature>| {
            if (fi as usize) >= total_features {
                return;
            }
            let r = 2 + fi as usize * 6;
            if feature_list.len() < r + 6 {
                return;
            }
            let tag = [
                feature_list[r],
                feature_list[r + 1],
                feature_list[r + 2],
                feature_list[r + 3],
            ];
            // §6.2.9 substitution: if this feature index is overridden,
            // use the alternate feature's lookup-index list directly.
            if let Some(s) = subst {
                if let Some(idxs) = s.lookup_indices_for_feature(fi) {
                    into.push(GsubFeature {
                        tag,
                        lookup_indices: idxs,
                    });
                    return;
                }
            }
            let foff = match read_u16(feature_list, r + 4) {
                Ok(v) => v as usize,
                Err(_) => return,
            };
            let feature = match feature_list.get(foff..) {
                Some(s) => s,
                None => return,
            };
            // Feature layout: Offset16 featureParamsOffset; u16
            // lookupIndexCount; u16 lookupListIndices[count].
            if feature.len() < 4 {
                return;
            }
            let count = match read_u16(feature, 2) {
                Ok(v) => v as usize,
                Err(_) => return,
            };
            if feature.len() < 4 + count * 2 {
                return;
            }
            let mut idxs = Vec::with_capacity(count);
            for i in 0..count {
                if let Ok(v) = read_u16(feature, 4 + i * 2) {
                    idxs.push(v);
                }
            }
            into.push(GsubFeature {
                tag,
                lookup_indices: idxs,
            });
        };

        if required != 0xFFFF {
            push_feature(required, &mut out);
        }
        for i in 0..feat_count {
            let fi = match read_u16(langsys, 6 + i * 2) {
                Ok(v) => v,
                Err(_) => continue,
            };
            push_feature(fi, &mut out);
        }
        out
    }

    /// Apply GSUB LookupType 1 (Single Substitution) lookup `lookup_index`
    /// to `gid`. Returns `Some(replacement_gid)` when the lookup's
    /// coverage covers `gid`, or `None` when no substitution applies
    /// (caller keeps the input glyph unchanged).
    ///
    /// Walks every sub-table in the lookup; the first hit (per spec
    /// "first matching subtable in lookup order") wins. ExtensionSubst
    /// (LookupType 7) wrappers are unwrapped transparently.
    pub fn apply_lookup_type_1(&self, lookup_index: u16, gid: u16) -> Option<u16> {
        if self.lookup_list_off == 0 {
            return None;
        }
        let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, lookup_index)?;
        if lookup.len() < 6 {
            return None;
        }
        let kind = read_u16(lookup, 0).ok()?;
        let sub_count = read_u16(lookup, 4).ok()? as usize;
        for s in 0..sub_count {
            let sub_off = read_u16(lookup, 6 + s * 2).ok()? as usize;
            let sub = lookup.get(sub_off..)?;
            let (effective_kind, effective_sub) = if kind == LOOKUP_EXTENSION_SUBST {
                if sub.len() < 8 {
                    continue;
                }
                let ext_type = read_u16(sub, 2).ok()?;
                let ext_off = read_u32(sub, 4).ok()? as usize;
                let ext = match sub.get(ext_off..) {
                    Some(s) => s,
                    None => continue,
                };
                (ext_type, ext)
            } else {
                (kind, sub)
            };
            if effective_kind != LOOKUP_SINGLE_SUBST {
                continue;
            }
            if let Some(hit) = single_subst_lookup(effective_sub, gid) {
                return Some(hit);
            }
        }
        None
    }

    /// Apply GSUB LookupType 4 (Ligature Substitution) lookup
    /// `lookup_index` to a prefix of `glyphs`.
    ///
    /// Returns `Some((replacement_gid, consumed))` when one of the
    /// lookup's sub-tables matches a prefix of `glyphs` of length
    /// `consumed >= 1` (in practice always `>= 2` for real ligatures —
    /// `componentCount = 1` would be a no-op single substitution and
    /// is allowed but vanishingly rare). Returns `None` when no
    /// ligature applies. ExtensionSubst (LookupType 7) wrappers are
    /// unwrapped transparently.
    ///
    /// This is the lookup-index-specific counterpart of
    /// [`Self::lookup_ligature`] and is what a feature-driven shaper
    /// calls after resolving the `liga` / `rlig` / `dlig` feature for
    /// the active script via [`Self::features_for_script`].
    pub fn apply_lookup_type_4(&self, lookup_index: u16, glyphs: &[u16]) -> Option<(u16, usize)> {
        if glyphs.is_empty() || self.lookup_list_off == 0 {
            return None;
        }
        let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, lookup_index)?;
        if lookup.len() < 6 {
            return None;
        }
        let kind = read_u16(lookup, 0).ok()?;
        let sub_count = read_u16(lookup, 4).ok()? as usize;
        for s in 0..sub_count {
            let sub_off = read_u16(lookup, 6 + s * 2).ok()? as usize;
            let sub = lookup.get(sub_off..)?;
            let (effective_kind, effective_sub) = if kind == LOOKUP_EXTENSION_SUBST {
                if sub.len() < 8 {
                    continue;
                }
                let ext_type = read_u16(sub, 2).ok()?;
                let ext_off = read_u32(sub, 4).ok()? as usize;
                let ext = match sub.get(ext_off..) {
                    Some(s) => s,
                    None => continue,
                };
                (ext_type, ext)
            } else {
                (kind, sub)
            };
            if effective_kind != LOOKUP_LIGATURE_SUBST {
                continue;
            }
            if let Some(hit) = ligature_subst_lookup(effective_sub, glyphs) {
                return Some(hit);
            }
        }
        None
    }

    /// Apply GSUB LookupType 6 (Chained Contexts Substitution) lookup
    /// `lookup_index` to the glyph run starting at `pos`.
    ///
    /// Returns `Some(rewritten)` — a fresh `Vec<u16>` containing the
    /// full run (`gids[..pos]` unchanged followed by the substituted
    /// tail) — when one of the lookup's sub-tables matches the
    /// `(backtrack, input, lookahead)` window around `pos`. Returns
    /// `None` when no chained-context rule applies.
    ///
    /// All three sub-table formats are supported:
    ///
    /// - **Format 1** — Coverage on the first input glyph + per-coverage
    ///   ChainSubRuleSet of explicit `(backtrack, input, lookahead)`
    ///   glyph sequences plus per-rule `SubstLookupRecord[]`.
    /// - **Format 2** — Coverage on the first input glyph + three
    ///   ClassDefs (backtrack/input/lookahead) + per-input-class
    ///   ChainSubClassSet whose rules are class sequences instead of
    ///   glyph sequences.
    /// - **Format 3** — three independent Coverage[] arrays
    ///   (backtrack / input / lookahead) + a single
    ///   `SubstLookupRecord[]`.
    ///
    /// Each `SubstLookupRecord { sequenceIndex, lookupListIndex }` is
    /// recursively dispatched: LookupType 1 substitutes one glyph,
    /// LookupType 4 substitutes `componentCount` glyphs starting at
    /// the relative `sequenceIndex`. ExtensionSubst (LookupType 7)
    /// wrappers are unwrapped transparently. Recursive sub-lookup
    /// expansion is bounded by `MAX_NESTED_LOOKUP_DEPTH` (8) to
    /// defuse pathological self-referential lookup graphs.
    pub fn apply_lookup_type_6(
        &self,
        lookup_index: u16,
        gids: &[u16],
        pos: usize,
    ) -> Option<Vec<u16>> {
        self.apply_chain_context_at(lookup_index, gids, pos, 0)
    }

    /// Apply GSUB LookupType 2 (Multiple Substitution) lookup
    /// `lookup_index` to a single input glyph `gid`.
    ///
    /// Multiple-substitution rules expand one glyph into a sequence of
    /// glyphs (the inverse of a ligature). Returns
    /// `Some(substitute_sequence)` — the `Sequence` record's glyph IDs
    /// — when the lookup's coverage covers `gid`, or `None` when no
    /// rule applies. ExtensionSubst (LookupType 7) wrappers are
    /// unwrapped transparently.
    ///
    /// The OpenType spec allows `glyphCount = 0` (deletion); we surface
    /// such entries as `Some(Vec::new())`. Per the spec the same
    /// `Sequence` is shared by every coverage index when only one is
    /// listed, but each coverage index can have its own.
    pub fn apply_lookup_type_2(&self, lookup_index: u16, gid: u16) -> Option<Vec<u16>> {
        if self.lookup_list_off == 0 {
            return None;
        }
        let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, lookup_index)?;
        if lookup.len() < 6 {
            return None;
        }
        let kind = read_u16(lookup, 0).ok()?;
        let sub_count = read_u16(lookup, 4).ok()? as usize;
        for s in 0..sub_count {
            let sub_off = read_u16(lookup, 6 + s * 2).ok()? as usize;
            let sub = lookup.get(sub_off..)?;
            let (effective_kind, effective_sub) = if kind == LOOKUP_EXTENSION_SUBST {
                if sub.len() < 8 {
                    continue;
                }
                let ext_type = read_u16(sub, 2).ok()?;
                let ext_off = read_u32(sub, 4).ok()? as usize;
                let ext = match sub.get(ext_off..) {
                    Some(s) => s,
                    None => continue,
                };
                (ext_type, ext)
            } else {
                (kind, sub)
            };
            if effective_kind != LOOKUP_MULTIPLE_SUBST {
                continue;
            }
            if let Some(hit) = multiple_subst_lookup(effective_sub, gid) {
                return Some(hit);
            }
        }
        None
    }

    /// Apply GSUB LookupType 3 (Alternate Substitution) lookup
    /// `lookup_index` to a single input glyph `gid`, picking
    /// `alternate_index` from the resolved `AlternateSet`.
    ///
    /// Returns `Some(replacement_gid)` when the lookup's coverage
    /// covers `gid` AND `alternate_index` is in range for that
    /// coverage's `AlternateSet`. Returns `None` on coverage miss,
    /// out-of-range alternate index, or non-alternate-substitution
    /// referenced lookup. ExtensionSubst (LookupType 7) wrappers are
    /// unwrapped transparently.
    ///
    /// Default callers should pass `alternate_index = 0` — the spec
    /// does not register a per-feature variant index, so picking the
    /// first alternate is the conventional `aalt` / `salt` default.
    /// Stylistic-set features like `ss01..ss20` typically encode their
    /// choice as a separate single-substitution lookup instead.
    pub fn apply_lookup_type_3(
        &self,
        lookup_index: u16,
        gid: u16,
        alternate_index: u16,
    ) -> Option<u16> {
        if self.lookup_list_off == 0 {
            return None;
        }
        let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, lookup_index)?;
        if lookup.len() < 6 {
            return None;
        }
        let kind = read_u16(lookup, 0).ok()?;
        let sub_count = read_u16(lookup, 4).ok()? as usize;
        for s in 0..sub_count {
            let sub_off = read_u16(lookup, 6 + s * 2).ok()? as usize;
            let sub = lookup.get(sub_off..)?;
            let (effective_kind, effective_sub) = if kind == LOOKUP_EXTENSION_SUBST {
                if sub.len() < 8 {
                    continue;
                }
                let ext_type = read_u16(sub, 2).ok()?;
                let ext_off = read_u32(sub, 4).ok()? as usize;
                let ext = match sub.get(ext_off..) {
                    Some(s) => s,
                    None => continue,
                };
                (ext_type, ext)
            } else {
                (kind, sub)
            };
            if effective_kind != LOOKUP_ALTERNATE_SUBST {
                continue;
            }
            if let Some(hit) = alternate_subst_lookup(effective_sub, gid, alternate_index) {
                return Some(hit);
            }
        }
        None
    }

    /// Apply GSUB LookupType 5 (Contextual Substitution) lookup
    /// `lookup_index` to the glyph run starting at `pos`.
    ///
    /// LookupType 5 is the predecessor of LookupType 6 minus the
    /// backtrack and lookahead arrays — the input window IS the
    /// context. All three sub-table formats are decoded:
    ///
    /// - **Format 1** — Coverage on the first input glyph + per-coverage
    ///   `SubRuleSet` of explicit input glyph sequences plus per-rule
    ///   `SubstLookupRecord[]`.
    /// - **Format 2** — Coverage on the first input glyph + a single
    ///   `ClassDef` + per-input-class `SubClassSet` whose rules are
    ///   class sequences instead of glyph sequences.
    /// - **Format 3** — `Coverage[]` array (one per input position) +
    ///   a single `SubstLookupRecord[]`.
    ///
    /// Each match's `SubstLookupRecord` is dispatched the same way as
    /// LookupType 6's records (LookupType 1 / 4 / 5 / 6 nested, bounded
    /// recursion). ExtensionSubst (LookupType 7) wrappers are unwrapped
    /// transparently. Returns `Some(rewritten_run)` on a match or
    /// `None` when no contextual rule fires.
    pub fn apply_lookup_type_5(
        &self,
        lookup_index: u16,
        gids: &[u16],
        pos: usize,
    ) -> Option<Vec<u16>> {
        self.apply_context_at(lookup_index, gids, pos, 0)
    }

    /// Apply GSUB LookupType 8 (Reverse Chained Context Substitution)
    /// lookup `lookup_index` to the glyph at `gids[pos]`.
    ///
    /// LookupType 8 has only Format 1: coverage on the input glyph,
    /// plus backtrack and lookahead `Coverage[]` arrays, plus a
    /// `substituteGlyphIDs[]` array indexed by the input coverage
    /// index. Unlike LookupType 6, the substitution is single-glyph
    /// (no `SubstLookupRecord[]`) and the spec mandates reverse-text
    /// processing of the input run — a higher-level shaper is what
    /// honours that ordering; this entry point answers "does this rule
    /// fire at `pos`?".
    ///
    /// Returns `Some(replacement_gid)` when the input coverage covers
    /// `gids[pos]` AND every backtrack / lookahead coverage matches
    /// the surrounding glyphs. Returns `None` otherwise.
    /// ExtensionSubst (LookupType 7) wrappers are unwrapped
    /// transparently.
    pub fn apply_lookup_type_8(&self, lookup_index: u16, gids: &[u16], pos: usize) -> Option<u16> {
        if self.lookup_list_off == 0 || pos >= gids.len() {
            return None;
        }
        let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, lookup_index)?;
        if lookup.len() < 6 {
            return None;
        }
        let kind = read_u16(lookup, 0).ok()?;
        let sub_count = read_u16(lookup, 4).ok()? as usize;
        for s in 0..sub_count {
            let sub_off = read_u16(lookup, 6 + s * 2).ok()? as usize;
            let sub = lookup.get(sub_off..)?;
            let (effective_kind, effective_sub) = if kind == LOOKUP_EXTENSION_SUBST {
                if sub.len() < 8 {
                    continue;
                }
                let ext_type = read_u16(sub, 2).ok()?;
                let ext_off = read_u32(sub, 4).ok()? as usize;
                let ext = match sub.get(ext_off..) {
                    Some(s) => s,
                    None => continue,
                };
                (ext_type, ext)
            } else {
                (kind, sub)
            };
            if effective_kind != LOOKUP_REVERSE_CHAIN_CONTEXT_SUBST {
                continue;
            }
            if let Some(hit) = reverse_chain_context_lookup(effective_sub, gids, pos) {
                return Some(hit);
            }
        }
        None
    }

    fn apply_context_at(
        &self,
        lookup_index: u16,
        gids: &[u16],
        pos: usize,
        depth: u8,
    ) -> Option<Vec<u16>> {
        if depth >= MAX_NESTED_LOOKUP_DEPTH {
            return None;
        }
        if pos >= gids.len() || self.lookup_list_off == 0 {
            return None;
        }
        let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, lookup_index)?;
        if lookup.len() < 6 {
            return None;
        }
        let kind = read_u16(lookup, 0).ok()?;
        let sub_count = read_u16(lookup, 4).ok()? as usize;
        for s in 0..sub_count {
            let sub_off = read_u16(lookup, 6 + s * 2).ok()? as usize;
            let sub = lookup.get(sub_off..)?;
            let (effective_kind, effective_sub) = if kind == LOOKUP_EXTENSION_SUBST {
                if sub.len() < 8 {
                    continue;
                }
                let ext_type = read_u16(sub, 2).ok()?;
                let ext_off = read_u32(sub, 4).ok()? as usize;
                let ext = match sub.get(ext_off..) {
                    Some(s) => s,
                    None => continue,
                };
                (ext_type, ext)
            } else {
                (kind, sub)
            };
            if effective_kind != LOOKUP_CONTEXT_SUBST {
                continue;
            }
            if effective_sub.len() < 2 {
                continue;
            }
            let format = match read_u16(effective_sub, 0) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let matched = match format {
                1 => context_format1_match(effective_sub, gids, pos),
                2 => context_format2_match(effective_sub, gids, pos),
                3 => context_format3_match(effective_sub, gids, pos),
                _ => None,
            };
            if let Some(m) = matched {
                return self.apply_subst_records(gids, pos, &m, depth);
            }
        }
        None
    }

    fn apply_chain_context_at(
        &self,
        lookup_index: u16,
        gids: &[u16],
        pos: usize,
        depth: u8,
    ) -> Option<Vec<u16>> {
        if depth >= MAX_NESTED_LOOKUP_DEPTH {
            return None;
        }
        if pos >= gids.len() || self.lookup_list_off == 0 {
            return None;
        }
        let lookup = lookup_table_slice(self.bytes, self.lookup_list_off, lookup_index)?;
        if lookup.len() < 6 {
            return None;
        }
        let kind = read_u16(lookup, 0).ok()?;
        let sub_count = read_u16(lookup, 4).ok()? as usize;
        for s in 0..sub_count {
            let sub_off = read_u16(lookup, 6 + s * 2).ok()? as usize;
            let sub = lookup.get(sub_off..)?;
            let (effective_kind, effective_sub) = if kind == LOOKUP_EXTENSION_SUBST {
                if sub.len() < 8 {
                    continue;
                }
                let ext_type = read_u16(sub, 2).ok()?;
                let ext_off = read_u32(sub, 4).ok()? as usize;
                let ext = match sub.get(ext_off..) {
                    Some(s) => s,
                    None => continue,
                };
                (ext_type, ext)
            } else {
                (kind, sub)
            };
            if effective_kind != LOOKUP_CHAIN_CONTEXT_SUBST {
                continue;
            }
            if effective_sub.len() < 2 {
                continue;
            }
            let format = match read_u16(effective_sub, 0) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let matched = match format {
                1 => chain_context_format1_match(effective_sub, gids, pos),
                2 => chain_context_format2_match(effective_sub, gids, pos),
                3 => chain_context_format3_match(effective_sub, gids, pos),
                _ => None,
            };
            if let Some(m) = matched {
                return self.apply_subst_records(gids, pos, &m, depth);
            }
        }
        None
    }

    /// Apply a chain-context match's `SubstLookupRecord[]` against the
    /// input run, returning the rewritten glyph slice.
    ///
    /// `m.input_len` glyphs at `gids[pos..pos + input_len]` are the
    /// chained-context "input" window. We walk the records in declared
    /// order, dispatching each (sequenceIndex, lookupListIndex) into
    /// the appropriate per-type apply path. Glyph indices in the input
    /// window are remapped as substitutions consume / replace glyphs.
    fn apply_subst_records(
        &self,
        gids: &[u16],
        pos: usize,
        m: &ChainMatch,
        depth: u8,
    ) -> Option<Vec<u16>> {
        // Working buffer: gids with the input window mutable.
        let mut out: Vec<u16> = gids.to_vec();
        // Track the current logical→physical remapping inside the input
        // window. Each entry maps a *logical* (pre-subst) sequenceIndex
        // to its current physical offset inside `out`. When a ligature
        // substitution consumes N input glyphs, the entries beyond it
        // collapse into a single physical position.
        let mut logical_to_phys: Vec<Option<usize>> =
            (0..m.input_len).map(|i| Some(pos + i)).collect();
        for rec in &m.records {
            let seq_idx = rec.sequence_index as usize;
            if seq_idx >= logical_to_phys.len() {
                continue;
            }
            let phys = match logical_to_phys[seq_idx] {
                Some(p) => p,
                None => continue,
            };
            // Resolve the referenced lookup type and dispatch.
            let lookup =
                match lookup_table_slice(self.bytes, self.lookup_list_off, rec.lookup_index) {
                    Some(s) => s,
                    None => continue,
                };
            if lookup.len() < 2 {
                continue;
            }
            let mut nested_kind = match read_u16(lookup, 0) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // ExtensionSubst (LookupType 7) at the nested-lookup
            // top-level: peek at the first sub-table's effective type.
            if nested_kind == LOOKUP_EXTENSION_SUBST {
                if let Some(t) = peek_extension_type(lookup) {
                    nested_kind = t;
                }
            }
            match nested_kind {
                LOOKUP_SINGLE_SUBST => {
                    if let Some(replacement) = self.apply_lookup_type_1(rec.lookup_index, out[phys])
                    {
                        out[phys] = replacement;
                    }
                }
                LOOKUP_LIGATURE_SUBST => {
                    let tail = &out[phys..];
                    if let Some((lig_glyph, consumed)) =
                        self.apply_lookup_type_4(rec.lookup_index, tail)
                    {
                        if consumed >= 1 && phys + consumed <= out.len() {
                            // Replace `consumed` glyphs at `phys` with
                            // a single ligature glyph.
                            out.splice(phys..phys + consumed, std::iter::once(lig_glyph));
                            // Collapse the consumed logical indices
                            // and shift later positions left.
                            let removed = consumed - 1;
                            // Mark consumed logical slots None.
                            for slot in logical_to_phys.iter_mut().skip(seq_idx + 1) {
                                if let Some(p) = *slot {
                                    if p < phys + consumed {
                                        *slot = None;
                                    } else {
                                        *slot = Some(p - removed);
                                    }
                                }
                            }
                        }
                    }
                }
                LOOKUP_CHAIN_CONTEXT_SUBST => {
                    if let Some(rewritten) =
                        self.apply_chain_context_at(rec.lookup_index, &out, phys, depth + 1)
                    {
                        // Recompute remap based on length delta.
                        let old_len = out.len();
                        out = rewritten;
                        let delta = out.len() as isize - old_len as isize;
                        if delta != 0 {
                            for slot in logical_to_phys.iter_mut().skip(seq_idx + 1) {
                                if let Some(p) = *slot {
                                    let np = p as isize + delta;
                                    if np < phys as isize {
                                        *slot = None;
                                    } else {
                                        *slot = Some(np as usize);
                                    }
                                }
                            }
                        }
                    }
                }
                LOOKUP_CONTEXT_SUBST => {
                    if let Some(rewritten) =
                        self.apply_context_at(rec.lookup_index, &out, phys, depth + 1)
                    {
                        let old_len = out.len();
                        out = rewritten;
                        let delta = out.len() as isize - old_len as isize;
                        if delta != 0 {
                            for slot in logical_to_phys.iter_mut().skip(seq_idx + 1) {
                                if let Some(p) = *slot {
                                    let np = p as isize + delta;
                                    if np < phys as isize {
                                        *slot = None;
                                    } else {
                                        *slot = Some(np as usize);
                                    }
                                }
                            }
                        }
                    }
                }
                LOOKUP_MULTIPLE_SUBST => {
                    if let Some(seq) = self.apply_lookup_type_2(rec.lookup_index, out[phys]) {
                        let inserted = seq.len();
                        out.splice(phys..phys + 1, seq);
                        // Length delta = inserted - 1.
                        let delta = inserted as isize - 1;
                        if delta != 0 {
                            for slot in logical_to_phys.iter_mut().skip(seq_idx + 1) {
                                if let Some(p) = *slot {
                                    let np = p as isize + delta;
                                    if np < phys as isize {
                                        *slot = None;
                                    } else {
                                        *slot = Some(np as usize);
                                    }
                                }
                            }
                        }
                    }
                }
                LOOKUP_ALTERNATE_SUBST => {
                    // Default to alternate index 0 — the spec doesn't
                    // register a per-feature variant index for nested
                    // lookups.
                    if let Some(replacement) =
                        self.apply_lookup_type_3(rec.lookup_index, out[phys], 0)
                    {
                        out[phys] = replacement;
                    }
                }
                _ => {
                    // Other nested lookup types (8 reverse-chain) are
                    // not used as nested sub-lookups by the spec; skip
                    // silently rather than abort the whole record.
                }
            }
        }
        Some(out)
    }

    /// Look up a ligature substitution that matches a prefix of `glyphs`.
    /// Walks every lookup; returns the *first* hit by lookup order.
    pub fn lookup_ligature(&self, glyphs: &[u16]) -> Option<(u16, usize)> {
        if glyphs.is_empty() {
            return None;
        }
        let lookup_list = self.bytes.get(self.lookup_list_off as usize..)?;
        if lookup_list.len() < 2 {
            return None;
        }
        let lookup_count = read_u16(lookup_list, 0).ok()?;
        for i in 0..lookup_count {
            let lookup = match lookup_table_slice(self.bytes, self.lookup_list_off, i) {
                Some(s) => s,
                None => continue,
            };
            if lookup.len() < 6 {
                continue;
            }
            let kind = read_u16(lookup, 0).ok()?;
            // Subtable count + offsets.
            let sub_count = read_u16(lookup, 4).ok()? as usize;
            for s in 0..sub_count {
                let sub_off = match read_u16(lookup, 6 + s * 2) {
                    Ok(o) => o as usize,
                    Err(_) => continue,
                };
                let sub = match lookup.get(sub_off..) {
                    Some(b) => b,
                    None => continue,
                };
                let (effective_kind, effective_sub) = if kind == LOOKUP_EXTENSION_SUBST {
                    // ExtensionSubst format 1:
                    //   u16 format=1, u16 extensionLookupType, Offset32 extensionOffset
                    if sub.len() < 8 {
                        continue;
                    }
                    let ext_type = read_u16(sub, 2).ok().unwrap_or(0);
                    let ext_off = read_u32(sub, 4).ok().unwrap_or(0) as usize;
                    let ext = match sub.get(ext_off..) {
                        Some(s) => s,
                        None => continue,
                    };
                    (ext_type, ext)
                } else {
                    (kind, sub)
                };
                if effective_kind != LOOKUP_LIGATURE_SUBST {
                    continue;
                }
                if let Some(hit) = ligature_subst_lookup(effective_sub, glyphs) {
                    return Some(hit);
                }
            }
        }
        None
    }
}

/// Walk a SingleSubst sub-table looking for a substitution.
///
/// Two formats per the OpenType spec:
///
///   Format 1: u16 format=1, Offset16 coverageOffset, i16 deltaGlyphID
///     - if `gid` is in coverage, return `gid + delta` (mod 2^16).
///   Format 2: u16 format=2, Offset16 coverageOffset, u16 glyphCount,
///             u16 substituteGlyphIDs[glyphCount]
///     - if `gid` is at coverage_index `i`, return
///       `substituteGlyphIDs[i]`.
fn single_subst_lookup(sub: &[u8], gid: u16) -> Option<u16> {
    if sub.len() < 6 {
        return None;
    }
    let format = read_u16(sub, 0).ok()?;
    let coverage_off = read_u16(sub, 2).ok()? as usize;
    let coverage = sub.get(coverage_off..)?;
    let cov_idx = coverage_lookup(coverage, gid)?;
    match format {
        1 => {
            // delta is signed; spec adds modulo 65536.
            let delta = read_u16(sub, 4).ok()? as i16 as i32;
            let result = (gid as i32 + delta) & 0xFFFF;
            Some(result as u16)
        }
        2 => {
            // glyphCount at +4, substituteGlyphIDs starts at +6.
            let count = read_u16(sub, 4).ok()? as usize;
            if cov_idx as usize >= count {
                return None;
            }
            let off = 6 + cov_idx as usize * 2;
            if sub.len() < off + 2 {
                return None;
            }
            read_u16(sub, off).ok()
        }
        _ => None,
    }
}

/// Walk a LigatureSubstFormat1 sub-table looking for a match.
///
/// Layout:
///   u16 format == 1
///   Offset16 coverageOffset             // glyph[0] coverage
///   u16 ligatureSetCount
///   Offset16 ligatureSetOffsets[ligatureSetCount]
///   ...
///   LigatureSet { u16 ligatureCount; Offset16 ligatureOffsets[]; }
///   Ligature    { u16 ligGlyph; u16 componentCount;
///                 u16 componentGlyphIDs[componentCount - 1]; }
fn ligature_subst_lookup(sub: &[u8], glyphs: &[u16]) -> Option<(u16, usize)> {
    if sub.len() < 6 {
        return None;
    }
    let format = read_u16(sub, 0).ok()?;
    if format != 1 {
        return None;
    }
    let coverage_off = read_u16(sub, 2).ok()? as usize;
    let coverage = sub.get(coverage_off..)?;
    let cov_idx = coverage_lookup(coverage, glyphs[0])? as usize;

    let lig_set_count = read_u16(sub, 4).ok()? as usize;
    if cov_idx >= lig_set_count {
        return None;
    }
    let lig_set_off = read_u16(sub, 6 + cov_idx * 2).ok()? as usize;
    let lig_set = sub.get(lig_set_off..)?;

    if lig_set.len() < 2 {
        return None;
    }
    let lig_count = read_u16(lig_set, 0).ok()? as usize;
    // For each ligature in this set, see if its component sequence
    // matches a prefix of `glyphs` after the first.
    for i in 0..lig_count {
        let lig_off = read_u16(lig_set, 2 + i * 2).ok()? as usize;
        let lig = lig_set.get(lig_off..)?;
        if lig.len() < 4 {
            continue;
        }
        let lig_glyph = read_u16(lig, 0).ok()?;
        let comp_count = read_u16(lig, 2).ok()? as usize;
        if comp_count < 1 {
            continue;
        }
        if comp_count > glyphs.len() {
            continue;
        }
        // First glyph already matched via coverage; compare remaining
        // (comp_count - 1) glyphs against componentGlyphIDs.
        let remaining = comp_count - 1;
        if lig.len() < 4 + remaining * 2 {
            continue;
        }
        let mut ok = true;
        for j in 0..remaining {
            let want = read_u16(lig, 4 + j * 2).ok()?;
            if glyphs[1 + j] != want {
                ok = false;
                break;
            }
        }
        if ok {
            return Some((lig_glyph, comp_count));
        }
    }
    None
}

/// Walk a MultipleSubstFormat1 sub-table looking for a Sequence record.
///
/// Layout (per OpenType §"Multiple Substitution Subtable"):
///   u16 format = 1
///   Offset16 coverageOffset
///   u16 sequenceCount
///   Offset16 sequenceOffsets[sequenceCount]
///
///   Sequence { u16 glyphCount; u16 substituteGlyphIDs[glyphCount]; }
///
/// All offsets are relative to the start of the sub-table.
fn multiple_subst_lookup(sub: &[u8], gid: u16) -> Option<Vec<u16>> {
    if sub.len() < 6 {
        return None;
    }
    let format = read_u16(sub, 0).ok()?;
    if format != 1 {
        return None;
    }
    let coverage_off = read_u16(sub, 2).ok()? as usize;
    let coverage = sub.get(coverage_off..)?;
    let cov_idx = coverage_lookup(coverage, gid)? as usize;
    let seq_count = read_u16(sub, 4).ok()? as usize;
    if cov_idx >= seq_count {
        return None;
    }
    let seq_off = read_u16(sub, 6 + cov_idx * 2).ok()? as usize;
    let seq = sub.get(seq_off..)?;
    if seq.len() < 2 {
        return None;
    }
    let glyph_count = read_u16(seq, 0).ok()? as usize;
    if seq.len() < 2 + glyph_count * 2 {
        return None;
    }
    let mut out = Vec::with_capacity(glyph_count);
    for i in 0..glyph_count {
        out.push(read_u16(seq, 2 + i * 2).ok()?);
    }
    Some(out)
}

/// Walk an AlternateSubstFormat1 sub-table looking for an alternate.
///
/// Layout (per OpenType §"Alternate Substitution Subtable"):
///   u16 format = 1
///   Offset16 coverageOffset
///   u16 alternateSetCount
///   Offset16 alternateSetOffsets[alternateSetCount]
///
///   AlternateSet { u16 glyphCount; u16 alternateGlyphIDs[glyphCount]; }
///
/// All offsets are relative to the start of the sub-table.
fn alternate_subst_lookup(sub: &[u8], gid: u16, alternate_index: u16) -> Option<u16> {
    if sub.len() < 6 {
        return None;
    }
    let format = read_u16(sub, 0).ok()?;
    if format != 1 {
        return None;
    }
    let coverage_off = read_u16(sub, 2).ok()? as usize;
    let coverage = sub.get(coverage_off..)?;
    let cov_idx = coverage_lookup(coverage, gid)? as usize;
    let alt_count = read_u16(sub, 4).ok()? as usize;
    if cov_idx >= alt_count {
        return None;
    }
    let alt_off = read_u16(sub, 6 + cov_idx * 2).ok()? as usize;
    let alt_set = sub.get(alt_off..)?;
    if alt_set.len() < 2 {
        return None;
    }
    let glyph_count = read_u16(alt_set, 0).ok()? as usize;
    let idx = alternate_index as usize;
    if idx >= glyph_count {
        return None;
    }
    if alt_set.len() < 2 + (idx + 1) * 2 {
        return None;
    }
    read_u16(alt_set, 2 + idx * 2).ok()
}

/// Walk a ReverseChainSingleSubstFormat1 sub-table.
///
/// Layout (per OpenType §"Reverse Chaining Contextual Single
/// Substitution Subtable"):
///   u16 format = 1
///   Offset16 coverageOffset                  (input glyph coverage)
///   u16 backtrackGlyphCount
///   Offset16 backtrackCoverageOffsets[backtrackGlyphCount]
///   u16 lookaheadGlyphCount
///   Offset16 lookaheadCoverageOffsets[lookaheadGlyphCount]
///   u16 glyphCount                           (= input coverage size)
///   u16 substituteGlyphIDs[glyphCount]
///
/// Backtrack coverages are listed in reverse-text order
/// (`backtrackCoverageOffsets[0]` covers the glyph immediately before
/// `gids[pos]`). Substitute glyph at `substituteGlyphIDs[cov_idx]`
/// where `cov_idx` is the input coverage index of `gids[pos]`.
fn reverse_chain_context_lookup(sub: &[u8], gids: &[u16], pos: usize) -> Option<u16> {
    if sub.len() < 6 {
        return None;
    }
    let format = read_u16(sub, 0).ok()?;
    if format != 1 {
        return None;
    }
    let coverage_off = read_u16(sub, 2).ok()? as usize;
    let coverage = sub.get(coverage_off..)?;
    let cov_idx = coverage_lookup(coverage, gids[pos])? as usize;
    let mut cur = 4usize;
    if sub.len() < cur + 2 {
        return None;
    }
    let bt_count = read_u16(sub, cur).ok()? as usize;
    cur += 2;
    if sub.len() < cur + bt_count * 2 {
        return None;
    }
    if pos < bt_count {
        return None;
    }
    for i in 0..bt_count {
        let cov_off = read_u16(sub, cur + i * 2).ok()? as usize;
        let cov = sub.get(cov_off..)?;
        coverage_lookup(cov, gids[pos - 1 - i])?;
    }
    cur += bt_count * 2;
    if sub.len() < cur + 2 {
        return None;
    }
    let la_count = read_u16(sub, cur).ok()? as usize;
    cur += 2;
    if sub.len() < cur + la_count * 2 {
        return None;
    }
    if pos + 1 + la_count > gids.len() {
        return None;
    }
    for i in 0..la_count {
        let cov_off = read_u16(sub, cur + i * 2).ok()? as usize;
        let cov = sub.get(cov_off..)?;
        coverage_lookup(cov, gids[pos + 1 + i])?;
    }
    cur += la_count * 2;
    if sub.len() < cur + 2 {
        return None;
    }
    let glyph_count = read_u16(sub, cur).ok()? as usize;
    cur += 2;
    if cov_idx >= glyph_count {
        return None;
    }
    if sub.len() < cur + (cov_idx + 1) * 2 {
        return None;
    }
    read_u16(sub, cur + cov_idx * 2).ok()
}

/// Outcome of a chained-context match: how many input glyphs the rule
/// covers, plus the SubstLookupRecord array to apply against them.
#[derive(Debug)]
struct ChainMatch {
    input_len: usize,
    records: Vec<SubstLookupRecord>,
}

#[derive(Debug, Clone, Copy)]
struct SubstLookupRecord {
    sequence_index: u16,
    lookup_index: u16,
}

/// Peek through a Lookup table that holds a single ExtensionSubst
/// sub-table and return the wrapped lookup type. Returns `None` if the
/// shape doesn't match (no sub-tables, malformed extension header, …).
fn peek_extension_type(lookup: &[u8]) -> Option<u16> {
    if lookup.len() < 8 {
        return None;
    }
    let sub_count = read_u16(lookup, 4).ok()? as usize;
    if sub_count == 0 {
        return None;
    }
    let sub_off = read_u16(lookup, 6).ok()? as usize;
    let sub = lookup.get(sub_off..)?;
    if sub.len() < 8 {
        return None;
    }
    // ExtensionSubstFormat1: u16 format=1, u16 extensionLookupType, Offset32.
    read_u16(sub, 2).ok()
}

/// Decode a SubstLookupRecord array of length `count` starting at
/// `offset` inside `bytes`. Returns `None` on truncation.
fn read_subst_lookup_records(
    bytes: &[u8],
    offset: usize,
    count: usize,
) -> Option<Vec<SubstLookupRecord>> {
    if bytes.len() < offset + count * 4 {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let off = offset + i * 4;
        let seq = read_u16(bytes, off).ok()?;
        let lk = read_u16(bytes, off + 2).ok()?;
        out.push(SubstLookupRecord {
            sequence_index: seq,
            lookup_index: lk,
        });
    }
    Some(out)
}

/// Match a ChainContextSubstFormat1 sub-table against `gids[pos..]`.
///
/// Layout (per OpenType §"Chained Sequence Context Format 1: simple
/// glyph contexts"):
///   u16 format = 1
///   Offset16 coverageOffset             (input[0] coverage)
///   u16 chainSubRuleSetCount
///   Offset16 chainSubRuleSetOffsets[chainSubRuleSetCount]
///
///   ChainSubRuleSet { u16 chainSubRuleCount; Offset16 chainSubRuleOffsets[]; }
///   ChainSubRule    { u16 backtrackGlyphCount; u16 backtrackSequence[];
///                     u16 inputGlyphCount;     u16 inputSequence[inputGlyphCount-1];
///                     u16 lookaheadGlyphCount; u16 lookaheadSequence[];
///                     u16 substCount;          SubstLookupRecord substRecords[]; }
///
/// All offsets are relative to the start of the sub-table (Format 1
/// header), then to each ChainSubRuleSet, in turn.
fn chain_context_format1_match(sub: &[u8], gids: &[u16], pos: usize) -> Option<ChainMatch> {
    if sub.len() < 6 {
        return None;
    }
    let coverage_off = read_u16(sub, 2).ok()? as usize;
    let coverage = sub.get(coverage_off..)?;
    let cov_idx = coverage_lookup(coverage, gids[pos])? as usize;
    let set_count = read_u16(sub, 4).ok()? as usize;
    if cov_idx >= set_count {
        return None;
    }
    let set_off = read_u16(sub, 6 + cov_idx * 2).ok()? as usize;
    let set = sub.get(set_off..)?;
    if set.len() < 2 {
        return None;
    }
    let rule_count = read_u16(set, 0).ok()? as usize;
    for r in 0..rule_count {
        let rule_off = read_u16(set, 2 + r * 2).ok()? as usize;
        let rule = match set.get(rule_off..) {
            Some(b) => b,
            None => continue,
        };
        if let Some(m) = chain_context_format1_rule_match(rule, gids, pos) {
            return Some(m);
        }
    }
    None
}

fn chain_context_format1_rule_match(rule: &[u8], gids: &[u16], pos: usize) -> Option<ChainMatch> {
    // backtrackGlyphCount + sequence
    let mut cur = 0usize;
    if rule.len() < cur + 2 {
        return None;
    }
    let bt_count = read_u16(rule, cur).ok()? as usize;
    cur += 2;
    if rule.len() < cur + bt_count * 2 {
        return None;
    }
    if pos < bt_count {
        return None;
    }
    // Backtrack sequence is stored in *reverse text* order (closest to
    // the input window first). Compare against gids working backwards
    // from pos.
    for i in 0..bt_count {
        let want = read_u16(rule, cur + i * 2).ok()?;
        if gids[pos - 1 - i] != want {
            return None;
        }
    }
    cur += bt_count * 2;
    // inputGlyphCount + inputSequence (length = count - 1)
    if rule.len() < cur + 2 {
        return None;
    }
    let in_count = read_u16(rule, cur).ok()? as usize;
    if in_count == 0 {
        return None;
    }
    cur += 2;
    let in_extra = in_count - 1;
    if rule.len() < cur + in_extra * 2 {
        return None;
    }
    if pos + in_count > gids.len() {
        return None;
    }
    for i in 0..in_extra {
        let want = read_u16(rule, cur + i * 2).ok()?;
        if gids[pos + 1 + i] != want {
            return None;
        }
    }
    cur += in_extra * 2;
    // lookaheadGlyphCount + sequence
    if rule.len() < cur + 2 {
        return None;
    }
    let la_count = read_u16(rule, cur).ok()? as usize;
    cur += 2;
    if rule.len() < cur + la_count * 2 {
        return None;
    }
    if pos + in_count + la_count > gids.len() {
        return None;
    }
    for i in 0..la_count {
        let want = read_u16(rule, cur + i * 2).ok()?;
        if gids[pos + in_count + i] != want {
            return None;
        }
    }
    cur += la_count * 2;
    // substCount + records
    if rule.len() < cur + 2 {
        return None;
    }
    let subst_count = read_u16(rule, cur).ok()? as usize;
    cur += 2;
    let records = read_subst_lookup_records(rule, cur, subst_count)?;
    Some(ChainMatch {
        input_len: in_count,
        records,
    })
}

/// Match a ChainContextSubstFormat2 sub-table against `gids[pos..]`.
///
/// Layout:
///   u16 format = 2
///   Offset16 coverageOffset
///   Offset16 backtrackClassDefOffset
///   Offset16 inputClassDefOffset
///   Offset16 lookaheadClassDefOffset
///   u16 chainSubClassSetCount
///   Offset16 chainSubClassSetOffsets[chainSubClassSetCount]
///
///   ChainSubClassSet { u16 chainSubClassRuleCount; Offset16 chainSubClassRuleOffsets[]; }
///   ChainSubClassRule { u16 backtrackGlyphCount; u16 backtrackSequence[]; (class IDs)
///                       u16 inputGlyphCount;     u16 inputSequence[inputGlyphCount-1];
///                       u16 lookaheadGlyphCount; u16 lookaheadSequence[];
///                       u16 substCount;          SubstLookupRecord substRecords[]; }
fn chain_context_format2_match(sub: &[u8], gids: &[u16], pos: usize) -> Option<ChainMatch> {
    if sub.len() < 12 {
        return None;
    }
    let coverage_off = read_u16(sub, 2).ok()? as usize;
    let bt_cd_off = read_u16(sub, 4).ok()? as usize;
    let in_cd_off = read_u16(sub, 6).ok()? as usize;
    let la_cd_off = read_u16(sub, 8).ok()? as usize;
    let set_count = read_u16(sub, 10).ok()? as usize;
    let coverage = sub.get(coverage_off..)?;
    coverage_lookup(coverage, gids[pos])?;
    // The Class of the input[0] glyph picks the ChainSubClassSet.
    let in_cd = sub.get(in_cd_off..)?;
    let in_class0 = class_def_lookup(in_cd, gids[pos]).unwrap_or(0);
    if in_class0 as usize >= set_count {
        return None;
    }
    let set_off = read_u16(sub, 12 + in_class0 as usize * 2).ok()? as usize;
    if set_off == 0 {
        // Empty set offsets are valid per spec; nothing matches.
        return None;
    }
    let set = sub.get(set_off..)?;
    if set.len() < 2 {
        return None;
    }
    let rule_count = read_u16(set, 0).ok()? as usize;
    let bt_cd = sub.get(bt_cd_off..);
    let la_cd = sub.get(la_cd_off..);
    for r in 0..rule_count {
        let rule_off = read_u16(set, 2 + r * 2).ok()? as usize;
        let rule = match set.get(rule_off..) {
            Some(b) => b,
            None => continue,
        };
        if let Some(m) =
            chain_context_format2_rule_match(rule, gids, pos, bt_cd, in_cd, la_cd, in_class0)
        {
            return Some(m);
        }
    }
    None
}

fn chain_context_format2_rule_match(
    rule: &[u8],
    gids: &[u16],
    pos: usize,
    bt_cd: Option<&[u8]>,
    in_cd: &[u8],
    la_cd: Option<&[u8]>,
    in_class0: u16,
) -> Option<ChainMatch> {
    let mut cur = 0usize;
    if rule.len() < cur + 2 {
        return None;
    }
    let bt_count = read_u16(rule, cur).ok()? as usize;
    cur += 2;
    if rule.len() < cur + bt_count * 2 {
        return None;
    }
    if pos < bt_count {
        return None;
    }
    // Backtrack classes; reverse text order.
    if bt_count > 0 {
        let bt_cd = bt_cd?;
        for i in 0..bt_count {
            let want = read_u16(rule, cur + i * 2).ok()?;
            let got = class_def_lookup(bt_cd, gids[pos - 1 - i]).unwrap_or(0);
            if want != got {
                return None;
            }
        }
    }
    cur += bt_count * 2;
    if rule.len() < cur + 2 {
        return None;
    }
    let in_count = read_u16(rule, cur).ok()? as usize;
    if in_count == 0 {
        return None;
    }
    cur += 2;
    let in_extra = in_count - 1;
    if rule.len() < cur + in_extra * 2 {
        return None;
    }
    if pos + in_count > gids.len() {
        return None;
    }
    // input[0] class is implicitly in_class0 (the caller-picked set);
    // verify input[1..] classes against in_cd.
    for i in 0..in_extra {
        let want = read_u16(rule, cur + i * 2).ok()?;
        let got = class_def_lookup(in_cd, gids[pos + 1 + i]).unwrap_or(0);
        if want != got {
            return None;
        }
    }
    cur += in_extra * 2;
    // (in_class0 is implicit, so no rule field consumed for it.)
    let _ = in_class0;
    if rule.len() < cur + 2 {
        return None;
    }
    let la_count = read_u16(rule, cur).ok()? as usize;
    cur += 2;
    if rule.len() < cur + la_count * 2 {
        return None;
    }
    if pos + in_count + la_count > gids.len() {
        return None;
    }
    if la_count > 0 {
        let la_cd = la_cd?;
        for i in 0..la_count {
            let want = read_u16(rule, cur + i * 2).ok()?;
            let got = class_def_lookup(la_cd, gids[pos + in_count + i]).unwrap_or(0);
            if want != got {
                return None;
            }
        }
    }
    cur += la_count * 2;
    if rule.len() < cur + 2 {
        return None;
    }
    let subst_count = read_u16(rule, cur).ok()? as usize;
    cur += 2;
    let records = read_subst_lookup_records(rule, cur, subst_count)?;
    Some(ChainMatch {
        input_len: in_count,
        records,
    })
}

/// Match a ChainContextSubstFormat3 sub-table against `gids[pos..]`.
///
/// Layout:
///   u16 format = 3
///   u16 backtrackGlyphCount
///   Offset16 backtrackCoverageOffsets[backtrackGlyphCount]
///   u16 inputGlyphCount
///   Offset16 inputCoverageOffsets[inputGlyphCount]
///   u16 lookaheadGlyphCount
///   Offset16 lookaheadCoverageOffsets[lookaheadGlyphCount]
///   u16 seqLookupCount
///   SubstLookupRecord seqLookupRecords[seqLookupCount]
///
/// All coverage offsets are relative to the start of the sub-table.
/// Backtrack coverages are listed in *reverse text* order (the spec is
/// explicit: "backtrackCoverageOffsets[0] points to the Coverage table
/// for the glyph immediately preceding input[0]").
fn chain_context_format3_match(sub: &[u8], gids: &[u16], pos: usize) -> Option<ChainMatch> {
    if sub.len() < 4 {
        return None;
    }
    let mut cur = 2usize;
    let bt_count = read_u16(sub, cur).ok()? as usize;
    cur += 2;
    if sub.len() < cur + bt_count * 2 {
        return None;
    }
    if pos < bt_count {
        return None;
    }
    for i in 0..bt_count {
        let cov_off = read_u16(sub, cur + i * 2).ok()? as usize;
        let cov = sub.get(cov_off..)?;
        coverage_lookup(cov, gids[pos - 1 - i])?;
    }
    cur += bt_count * 2;
    if sub.len() < cur + 2 {
        return None;
    }
    let in_count = read_u16(sub, cur).ok()? as usize;
    if in_count == 0 {
        return None;
    }
    cur += 2;
    if sub.len() < cur + in_count * 2 {
        return None;
    }
    if pos + in_count > gids.len() {
        return None;
    }
    for i in 0..in_count {
        let cov_off = read_u16(sub, cur + i * 2).ok()? as usize;
        let cov = sub.get(cov_off..)?;
        coverage_lookup(cov, gids[pos + i])?;
    }
    cur += in_count * 2;
    if sub.len() < cur + 2 {
        return None;
    }
    let la_count = read_u16(sub, cur).ok()? as usize;
    cur += 2;
    if sub.len() < cur + la_count * 2 {
        return None;
    }
    if pos + in_count + la_count > gids.len() {
        return None;
    }
    for i in 0..la_count {
        let cov_off = read_u16(sub, cur + i * 2).ok()? as usize;
        let cov = sub.get(cov_off..)?;
        coverage_lookup(cov, gids[pos + in_count + i])?;
    }
    cur += la_count * 2;
    if sub.len() < cur + 2 {
        return None;
    }
    let subst_count = read_u16(sub, cur).ok()? as usize;
    cur += 2;
    let records = read_subst_lookup_records(sub, cur, subst_count)?;
    Some(ChainMatch {
        input_len: in_count,
        records,
    })
}

/// Match a SequenceContextFormat1 sub-table (LookupType 5 Format 1)
/// against `gids[pos..]`.
///
/// Layout (per OpenType §"Sequence Context Format 1: simple glyph
/// contexts"):
///   u16 format = 1
///   Offset16 coverageOffset                  (input[0] coverage)
///   u16 subRuleSetCount
///   Offset16 subRuleSetOffsets[subRuleSetCount]
///
///   SubRuleSet { u16 subRuleCount; Offset16 subRuleOffsets[]; }
///   SubRule    { u16 inputGlyphCount; u16 inputSequence[inputGlyphCount-1];
///                u16 substCount;       SubstLookupRecord substRecords[]; }
///
/// All offsets are relative to the start of the sub-table, then to
/// each SubRuleSet, in turn.
fn context_format1_match(sub: &[u8], gids: &[u16], pos: usize) -> Option<ChainMatch> {
    if sub.len() < 6 {
        return None;
    }
    let coverage_off = read_u16(sub, 2).ok()? as usize;
    let coverage = sub.get(coverage_off..)?;
    let cov_idx = coverage_lookup(coverage, gids[pos])? as usize;
    let set_count = read_u16(sub, 4).ok()? as usize;
    if cov_idx >= set_count {
        return None;
    }
    let set_off = read_u16(sub, 6 + cov_idx * 2).ok()? as usize;
    let set = sub.get(set_off..)?;
    if set.len() < 2 {
        return None;
    }
    let rule_count = read_u16(set, 0).ok()? as usize;
    for r in 0..rule_count {
        let rule_off = read_u16(set, 2 + r * 2).ok()? as usize;
        let rule = match set.get(rule_off..) {
            Some(b) => b,
            None => continue,
        };
        if let Some(m) = context_format1_rule_match(rule, gids, pos) {
            return Some(m);
        }
    }
    None
}

fn context_format1_rule_match(rule: &[u8], gids: &[u16], pos: usize) -> Option<ChainMatch> {
    let mut cur = 0usize;
    if rule.len() < cur + 2 {
        return None;
    }
    let in_count = read_u16(rule, cur).ok()? as usize;
    if in_count == 0 {
        return None;
    }
    cur += 2;
    let in_extra = in_count - 1;
    if rule.len() < cur + in_extra * 2 {
        return None;
    }
    if pos + in_count > gids.len() {
        return None;
    }
    for i in 0..in_extra {
        let want = read_u16(rule, cur + i * 2).ok()?;
        if gids[pos + 1 + i] != want {
            return None;
        }
    }
    cur += in_extra * 2;
    if rule.len() < cur + 2 {
        return None;
    }
    let subst_count = read_u16(rule, cur).ok()? as usize;
    cur += 2;
    let records = read_subst_lookup_records(rule, cur, subst_count)?;
    Some(ChainMatch {
        input_len: in_count,
        records,
    })
}

/// Match a SequenceContextFormat2 sub-table (LookupType 5 Format 2)
/// against `gids[pos..]`.
///
/// Layout:
///   u16 format = 2
///   Offset16 coverageOffset
///   Offset16 classDefOffset
///   u16 subClassSetCount
///   Offset16 subClassSetOffsets[subClassSetCount]
///
///   SubClassSet  { u16 subClassRuleCount; Offset16 subClassRuleOffsets[]; }
///   SubClassRule { u16 glyphCount; u16 inputSequence[glyphCount-1]; (class IDs)
///                  u16 substCount; SubstLookupRecord substRecords[]; }
fn context_format2_match(sub: &[u8], gids: &[u16], pos: usize) -> Option<ChainMatch> {
    if sub.len() < 8 {
        return None;
    }
    let coverage_off = read_u16(sub, 2).ok()? as usize;
    let cd_off = read_u16(sub, 4).ok()? as usize;
    let set_count = read_u16(sub, 6).ok()? as usize;
    let coverage = sub.get(coverage_off..)?;
    coverage_lookup(coverage, gids[pos])?;
    let cd = sub.get(cd_off..)?;
    let class0 = class_def_lookup(cd, gids[pos]).unwrap_or(0);
    if class0 as usize >= set_count {
        return None;
    }
    let set_off = read_u16(sub, 8 + class0 as usize * 2).ok()? as usize;
    if set_off == 0 {
        return None;
    }
    let set = sub.get(set_off..)?;
    if set.len() < 2 {
        return None;
    }
    let rule_count = read_u16(set, 0).ok()? as usize;
    for r in 0..rule_count {
        let rule_off = read_u16(set, 2 + r * 2).ok()? as usize;
        let rule = match set.get(rule_off..) {
            Some(b) => b,
            None => continue,
        };
        if let Some(m) = context_format2_rule_match(rule, gids, pos, cd) {
            return Some(m);
        }
    }
    None
}

fn context_format2_rule_match(
    rule: &[u8],
    gids: &[u16],
    pos: usize,
    cd: &[u8],
) -> Option<ChainMatch> {
    let mut cur = 0usize;
    if rule.len() < cur + 2 {
        return None;
    }
    let in_count = read_u16(rule, cur).ok()? as usize;
    if in_count == 0 {
        return None;
    }
    cur += 2;
    let in_extra = in_count - 1;
    if rule.len() < cur + in_extra * 2 {
        return None;
    }
    if pos + in_count > gids.len() {
        return None;
    }
    // input[0] class is implicit (the caller-picked set selects it).
    for i in 0..in_extra {
        let want = read_u16(rule, cur + i * 2).ok()?;
        let got = class_def_lookup(cd, gids[pos + 1 + i]).unwrap_or(0);
        if want != got {
            return None;
        }
    }
    cur += in_extra * 2;
    if rule.len() < cur + 2 {
        return None;
    }
    let subst_count = read_u16(rule, cur).ok()? as usize;
    cur += 2;
    let records = read_subst_lookup_records(rule, cur, subst_count)?;
    Some(ChainMatch {
        input_len: in_count,
        records,
    })
}

/// Match a SequenceContextFormat3 sub-table (LookupType 5 Format 3)
/// against `gids[pos..]`.
///
/// Layout:
///   u16 format = 3
///   u16 glyphCount
///   u16 substCount
///   Offset16 coverageOffsets[glyphCount]
///   SubstLookupRecord substRecords[substCount]
///
/// All coverage offsets are relative to the start of the sub-table.
fn context_format3_match(sub: &[u8], gids: &[u16], pos: usize) -> Option<ChainMatch> {
    if sub.len() < 6 {
        return None;
    }
    let glyph_count = read_u16(sub, 2).ok()? as usize;
    let subst_count = read_u16(sub, 4).ok()? as usize;
    if glyph_count == 0 {
        return None;
    }
    let mut cur = 6usize;
    if sub.len() < cur + glyph_count * 2 {
        return None;
    }
    if pos + glyph_count > gids.len() {
        return None;
    }
    for i in 0..glyph_count {
        let cov_off = read_u16(sub, cur + i * 2).ok()? as usize;
        let cov = sub.get(cov_off..)?;
        coverage_lookup(cov, gids[pos + i])?;
    }
    cur += glyph_count * 2;
    let records = read_subst_lookup_records(sub, cur, subst_count)?;
    Some(ChainMatch {
        input_len: glyph_count,
        records,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a self-contained GSUB blob with one LookupType-4 sub-table
    /// covering glyph 100 → ligature glyph 999 with components [200,300].
    fn build_simple_gsub() -> (Vec<u8>, u16, u16, u16, u16) {
        // We hand-build offsets in nested tables.
        // Layout plan (relative to start of GSUB table):
        //   0..10  GSUB header (v1.0). lookupListOffset = 10.
        //  10..14  LookupList: count=1, offset to lookup
        //  14..22  Lookup: type=4, flag=0, subTableCount=1, subOffset=8
        //  22..30  LigatureSubstFormat1 header (we'll write it)
        //   ...

        // Build sub-objects bottom-up.
        // Ligature table: ligGlyph=999, componentCount=3, componentGlyphIDs=[200,300]
        let mut lig = Vec::new();
        lig.extend_from_slice(&999u16.to_be_bytes());
        lig.extend_from_slice(&3u16.to_be_bytes());
        lig.extend_from_slice(&200u16.to_be_bytes());
        lig.extend_from_slice(&300u16.to_be_bytes());

        // LigatureSet: count=1, offset to lig (after the 4-byte header).
        let mut lig_set = Vec::new();
        lig_set.extend_from_slice(&1u16.to_be_bytes());
        lig_set.extend_from_slice(&4u16.to_be_bytes()); // ligature offset (2 + 2)
        lig_set.extend_from_slice(&lig);

        // Coverage Format 1 covering glyph 100.
        let mut cov = Vec::new();
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&100u16.to_be_bytes());

        // LigatureSubstFormat1: format=1, coverageOffset, ligSetCount=1, ligSetOffsets[1].
        // Header is 8 bytes (format(2) + cov(2) + count(2) + offset(2)).
        let lig_subst_header_len = 8;
        let cov_off = lig_subst_header_len;
        let lig_set_off = cov_off + cov.len();
        let mut lig_subst = Vec::new();
        lig_subst.extend_from_slice(&1u16.to_be_bytes());
        lig_subst.extend_from_slice(&(cov_off as u16).to_be_bytes());
        lig_subst.extend_from_slice(&1u16.to_be_bytes());
        lig_subst.extend_from_slice(&(lig_set_off as u16).to_be_bytes());
        lig_subst.extend_from_slice(&cov);
        lig_subst.extend_from_slice(&lig_set);

        // Lookup table: type=4, flag=0, subCount=1, subOffsets=[8].
        // Header is 6 bytes; one subtable offset = 2 bytes; subtable
        // starts at offset 8 from the lookup-table start.
        let mut lookup = Vec::new();
        lookup.extend_from_slice(&4u16.to_be_bytes());
        lookup.extend_from_slice(&0u16.to_be_bytes());
        lookup.extend_from_slice(&1u16.to_be_bytes());
        lookup.extend_from_slice(&8u16.to_be_bytes());
        lookup.extend_from_slice(&lig_subst);

        // LookupList: lookupCount=1, lookupOffsets=[4].
        let mut lookup_list = Vec::new();
        lookup_list.extend_from_slice(&1u16.to_be_bytes());
        lookup_list.extend_from_slice(&4u16.to_be_bytes());
        lookup_list.extend_from_slice(&lookup);

        // GSUB header: v1.0, scriptList=NULL(0), featureList=NULL(0), lookupListOffset=10.
        let mut gsub = Vec::new();
        gsub.extend_from_slice(&1u16.to_be_bytes()); // major
        gsub.extend_from_slice(&0u16.to_be_bytes()); // minor
        gsub.extend_from_slice(&0u16.to_be_bytes()); // scriptList
        gsub.extend_from_slice(&0u16.to_be_bytes()); // featureList
        gsub.extend_from_slice(&10u16.to_be_bytes()); // lookupList
        gsub.extend_from_slice(&lookup_list);

        (gsub, 100, 200, 300, 999)
    }

    #[test]
    fn round_trip_3_glyph_ligature() {
        let (bytes, a, b, c, lig) = build_simple_gsub();
        let g = GsubTable::parse(&bytes).unwrap();
        assert_eq!(g.lookup_ligature(&[a, b, c]), Some((lig, 3)));
        // Wrong second glyph → no match.
        assert_eq!(g.lookup_ligature(&[a, b, 999]), None);
        // First glyph not covered → no match.
        assert_eq!(g.lookup_ligature(&[42, b, c]), None);
        // Too short input.
        assert_eq!(g.lookup_ligature(&[a]), None);
    }

    /// Build a GSUB blob exercising:
    /// - one ScriptList with `arab` → DefaultLangSys → 4 feature indices
    /// - one FeatureList with the four entries `init`/`medi`/`fina`/`isol`,
    ///   each pointing at one lookup index (0..=3)
    /// - one LookupList with two LookupType-1 subtables:
    ///   * Format 1 (delta = +5) at lookup index 0
    ///   * Format 2 (substituteGlyphIDs) at lookup index 1
    ///   * (lookups 2 and 3 are stubs that share the same payload)
    ///
    /// We hand-pack offsets at build time and return `(blob, off_table)`
    /// where `off_table` lists the offsets the tests want to assert
    /// against.
    fn build_feature_tagged_gsub() -> Vec<u8> {
        // ----- LookupList -----
        //
        // Lookup #0 — SingleSubst Format 1: covers gid 10, delta = +5.
        //   Coverage Format 1: count=1, [10]
        //   SingleSubstFmt1: format=1, coverageOffset(=6), delta=5
        let mut cov0 = Vec::new();
        cov0.extend_from_slice(&1u16.to_be_bytes()); // format
        cov0.extend_from_slice(&1u16.to_be_bytes()); // glyphCount
        cov0.extend_from_slice(&10u16.to_be_bytes());
        let mut sub0 = Vec::new();
        sub0.extend_from_slice(&1u16.to_be_bytes()); // format
        sub0.extend_from_slice(&6u16.to_be_bytes()); // coverageOffset (after 6-byte header)
        sub0.extend_from_slice(&5i16.to_be_bytes()); // delta
        sub0.extend_from_slice(&cov0);

        // Lookup #1 — SingleSubst Format 2: covers gids [20,21,22] →
        //   substitutes [200,201,202].
        let mut cov1 = Vec::new();
        cov1.extend_from_slice(&1u16.to_be_bytes()); // format
        cov1.extend_from_slice(&3u16.to_be_bytes()); // glyphCount
        cov1.extend_from_slice(&20u16.to_be_bytes());
        cov1.extend_from_slice(&21u16.to_be_bytes());
        cov1.extend_from_slice(&22u16.to_be_bytes());
        let mut sub1 = Vec::new();
        sub1.extend_from_slice(&2u16.to_be_bytes()); // format
                                                     // header is 6 bytes (format + cov + count); we put coverage AFTER the
                                                     // substitute-array. substituteGlyphIDs[3] = 6 bytes.
        let cov1_off_in_sub1: u16 = 6 + 6; // = 12
        sub1.extend_from_slice(&cov1_off_in_sub1.to_be_bytes());
        sub1.extend_from_slice(&3u16.to_be_bytes()); // glyphCount
        sub1.extend_from_slice(&200u16.to_be_bytes());
        sub1.extend_from_slice(&201u16.to_be_bytes());
        sub1.extend_from_slice(&202u16.to_be_bytes());
        sub1.extend_from_slice(&cov1);

        // Build a Lookup wrapper for one subtable:
        //   u16 lookupType, u16 lookupFlag, u16 subTableCount, u16 subTableOffsets[]
        // Header is 6 bytes + 2 per subtable; the subtable lives
        // immediately after.
        fn wrap_lookup(lookup_type: u16, sub: &[u8]) -> Vec<u8> {
            let mut out = Vec::new();
            out.extend_from_slice(&lookup_type.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes()); // flag
            out.extend_from_slice(&1u16.to_be_bytes()); // subTableCount
            out.extend_from_slice(&8u16.to_be_bytes()); // subTableOffset = 8 (right after the offset)
            out.extend_from_slice(sub);
            out
        }

        let lookup0 = wrap_lookup(LOOKUP_SINGLE_SUBST, &sub0);
        let lookup1 = wrap_lookup(LOOKUP_SINGLE_SUBST, &sub1);
        let lookup2 = wrap_lookup(LOOKUP_SINGLE_SUBST, &sub0);
        let lookup3 = wrap_lookup(LOOKUP_SINGLE_SUBST, &sub0);

        // LookupList: u16 count, Offset16 lookupOffsets[count].
        // The LookupList is a self-contained sub-table; lookupOffsets
        // are RELATIVE TO the LookupList start.
        let lookup_list_header_len = 2 + 4 * 2; // count + 4 offsets
        let mut lookup_list = Vec::new();
        lookup_list.extend_from_slice(&4u16.to_be_bytes());
        let mut running = lookup_list_header_len as u16;
        // Offsets first…
        for lk in [&lookup0, &lookup1, &lookup2, &lookup3] {
            lookup_list.extend_from_slice(&running.to_be_bytes());
            running += lk.len() as u16;
        }
        // …then payloads in declaration order.
        for lk in [&lookup0, &lookup1, &lookup2, &lookup3] {
            lookup_list.extend_from_slice(lk);
        }

        // ----- FeatureList -----
        //
        // Feature 0: init → lookup [0]
        // Feature 1: medi → lookup [1]
        // Feature 2: fina → lookup [2]
        // Feature 3: isol → lookup [3]
        fn build_feature(lookup_idx: u16) -> Vec<u8> {
            let mut f = Vec::new();
            f.extend_from_slice(&0u16.to_be_bytes()); // featureParamsOffset = NULL
            f.extend_from_slice(&1u16.to_be_bytes()); // lookupIndexCount
            f.extend_from_slice(&lookup_idx.to_be_bytes());
            f
        }
        let feat_init = build_feature(0);
        let feat_medi = build_feature(1);
        let feat_fina = build_feature(2);
        let feat_isol = build_feature(3);
        let tags: [&[u8; 4]; 4] = [b"init", b"medi", b"fina", b"isol"];

        // FeatureList: u16 featureCount, FeatureRecord{ Tag(4),
        // Offset16 featureOffset } records[count];
        // featureOffsets are relative to FeatureList start.
        let feature_list_header_len = 2 + 4 * 6; // count + 4 records of 6 bytes
        let mut feature_list = Vec::new();
        feature_list.extend_from_slice(&4u16.to_be_bytes());
        let mut running_f = feature_list_header_len as u16;
        let payloads = [&feat_init, &feat_medi, &feat_fina, &feat_isol];
        for (tag, fbytes) in tags.iter().zip(payloads.iter()) {
            feature_list.extend_from_slice(*tag);
            feature_list.extend_from_slice(&running_f.to_be_bytes());
            running_f += fbytes.len() as u16;
        }
        for fbytes in payloads {
            feature_list.extend_from_slice(fbytes);
        }

        // ----- ScriptList -----
        //
        // One script: arab → DefaultLangSys → feature indices [0,1,2,3].
        //
        // LangSys: lookupOrderOffset(0), required(0xFFFF), featCount(4),
        //   featureIndices[4] = [0,1,2,3]
        let mut langsys = Vec::new();
        langsys.extend_from_slice(&0u16.to_be_bytes());
        langsys.extend_from_slice(&0xFFFFu16.to_be_bytes());
        langsys.extend_from_slice(&4u16.to_be_bytes());
        langsys.extend_from_slice(&0u16.to_be_bytes());
        langsys.extend_from_slice(&1u16.to_be_bytes());
        langsys.extend_from_slice(&2u16.to_be_bytes());
        langsys.extend_from_slice(&3u16.to_be_bytes());

        // Script: defaultLangSysOffset, langSysCount(0), then the
        //   DefaultLangSys payload immediately after (its offset = 4).
        let mut script = Vec::new();
        script.extend_from_slice(&4u16.to_be_bytes()); // defaultLangSysOffset
        script.extend_from_slice(&0u16.to_be_bytes()); // langSysCount
        script.extend_from_slice(&langsys);

        // ScriptList: u16 scriptCount, ScriptRecord{ Tag(4), Offset16
        //   scriptOffset } [count]; scriptOffsets relative to
        //   ScriptList start.
        let mut script_list = Vec::new();
        script_list.extend_from_slice(&1u16.to_be_bytes());
        script_list.extend_from_slice(b"arab");
        let script_off: u16 = 2 + 6; // after 1 record
        script_list.extend_from_slice(&script_off.to_be_bytes());
        script_list.extend_from_slice(&script);

        // ----- GSUB header -----
        //
        // u16 major=1, u16 minor=0, Offset16 scriptList, Offset16
        //   featureList, Offset16 lookupList. All offsets are relative
        //   to the GSUB start.
        let header_len = 10u16;
        let script_list_off = header_len;
        let feature_list_off = script_list_off + script_list.len() as u16;
        let lookup_list_off = feature_list_off + feature_list.len() as u16;

        let mut gsub = Vec::new();
        gsub.extend_from_slice(&1u16.to_be_bytes());
        gsub.extend_from_slice(&0u16.to_be_bytes());
        gsub.extend_from_slice(&script_list_off.to_be_bytes());
        gsub.extend_from_slice(&feature_list_off.to_be_bytes());
        gsub.extend_from_slice(&lookup_list_off.to_be_bytes());
        gsub.extend_from_slice(&script_list);
        gsub.extend_from_slice(&feature_list);
        gsub.extend_from_slice(&lookup_list);
        gsub
    }

    #[test]
    fn gsub_header_parses_version_1_0() {
        let bytes = build_feature_tagged_gsub();
        let g = GsubTable::parse(&bytes).expect("v1.0 header parses");
        assert!(g.script_list_off > 0);
        assert!(g.feature_list_off > 0);
        assert!(g.lookup_list_off > 0);
    }

    #[test]
    fn script_list_finds_arab_script() {
        let bytes = build_feature_tagged_gsub();
        let g = GsubTable::parse(&bytes).unwrap();
        let feats = g.features_for_script(*b"arab", None);
        assert_eq!(feats.len(), 4, "arab script should expose 4 features");
        // Unknown script → empty.
        let none = g.features_for_script(*b"latn", None);
        assert!(none.is_empty(), "latn isn't in the test ScriptList");
    }

    #[test]
    fn feature_list_returns_init_medi_fina_isol_for_arab() {
        let bytes = build_feature_tagged_gsub();
        let g = GsubTable::parse(&bytes).unwrap();
        let feats = g.features_for_script(*b"arab", None);
        let tags: Vec<[u8; 4]> = feats.iter().map(|f| f.tag).collect();
        assert_eq!(
            tags,
            vec![*b"init", *b"medi", *b"fina", *b"isol"],
            "expected the four positional-form tags in declaration order"
        );
        // Each feature points at exactly one lookup, in matching order.
        for (i, f) in feats.iter().enumerate() {
            assert_eq!(f.lookup_indices, vec![i as u16]);
        }
    }

    #[test]
    fn lookup_type_1_format_1_delta_substitution_applies() {
        let bytes = build_feature_tagged_gsub();
        let g = GsubTable::parse(&bytes).unwrap();
        // Lookup 0 is Format 1 with delta = +5 over coverage [10].
        assert_eq!(g.apply_lookup_type_1(0, 10), Some(15));
    }

    #[test]
    fn lookup_type_1_format_2_array_substitution_applies() {
        let bytes = build_feature_tagged_gsub();
        let g = GsubTable::parse(&bytes).unwrap();
        // Lookup 1 is Format 2 mapping [20,21,22] → [200,201,202].
        assert_eq!(g.apply_lookup_type_1(1, 20), Some(200));
        assert_eq!(g.apply_lookup_type_1(1, 21), Some(201));
        assert_eq!(g.apply_lookup_type_1(1, 22), Some(202));
    }

    #[test]
    fn gsub_apply_returns_none_when_gid_not_in_coverage() {
        let bytes = build_feature_tagged_gsub();
        let g = GsubTable::parse(&bytes).unwrap();
        // gid 99 isn't in either coverage table.
        assert_eq!(g.apply_lookup_type_1(0, 99), None);
        assert_eq!(g.apply_lookup_type_1(1, 99), None);
        // Out-of-range lookup index → None (no panic).
        assert_eq!(g.apply_lookup_type_1(123, 10), None);
    }

    #[test]
    fn signed_delta_wraps_modulo_65536() {
        // Build a SingleSubstFormat1 with a NEGATIVE delta covering glyph
        // 5 → 5 + (-3) = 2.
        let mut cov = Vec::new();
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&5u16.to_be_bytes());
        let mut sub = Vec::new();
        sub.extend_from_slice(&1u16.to_be_bytes()); // format
        sub.extend_from_slice(&6u16.to_be_bytes()); // coverageOffset
        sub.extend_from_slice(&(-3i16).to_be_bytes()); // delta
        sub.extend_from_slice(&cov);
        assert_eq!(single_subst_lookup(&sub, 5), Some(2));
        // Wrap: glyph 1 + (-3) = 65534.
        let mut cov2 = Vec::new();
        cov2.extend_from_slice(&1u16.to_be_bytes());
        cov2.extend_from_slice(&1u16.to_be_bytes());
        cov2.extend_from_slice(&1u16.to_be_bytes());
        let mut sub2 = Vec::new();
        sub2.extend_from_slice(&1u16.to_be_bytes());
        sub2.extend_from_slice(&6u16.to_be_bytes());
        sub2.extend_from_slice(&(-3i16).to_be_bytes());
        sub2.extend_from_slice(&cov2);
        assert_eq!(single_subst_lookup(&sub2, 1), Some(65534));
    }

    #[test]
    fn apply_lookup_type_4_consumes_correct_count() {
        // Re-use build_simple_gsub: lookup 0 covers gid 100 + [200,300]
        // -> ligature 999, componentCount=3.
        let (bytes, a, b, c, lig) = build_simple_gsub();
        let g = GsubTable::parse(&bytes).unwrap();
        assert_eq!(g.apply_lookup_type_4(0, &[a, b, c]), Some((lig, 3)));
        // Out-of-range lookup index → None.
        assert_eq!(g.apply_lookup_type_4(99, &[a, b, c]), None);
        // Wrong second glyph → None.
        assert_eq!(g.apply_lookup_type_4(0, &[a, b, 555]), None);
        // Empty input → None.
        assert_eq!(g.apply_lookup_type_4(0, &[]), None);
    }

    #[test]
    fn apply_lookup_type_4_skips_non_ligature_lookups() {
        // build_feature_tagged_gsub publishes only LookupType-1 (single
        // substitution) lookups; apply_lookup_type_4 should silently
        // return None for any of them.
        let bytes = build_feature_tagged_gsub();
        let g = GsubTable::parse(&bytes).unwrap();
        for li in 0..4 {
            assert_eq!(g.apply_lookup_type_4(li, &[10, 20, 30]), None);
        }
    }

    /// Build a GSUB blob with two lookups:
    /// - Lookup 0 — SingleSubst Format 1 covering gid 5 → +100 (= 105).
    /// - Lookup 1 — ChainContextSubstFormat 1: at gid 10 with backtrack
    ///   [1] and lookahead [99], invoke lookup 0 at sequenceIndex 0
    ///   for input [10] → 110 (single subst delta = +100).
    ///
    /// Wait: lookup 0 covers gid 5, not gid 10. Let's instead make
    /// lookup 0 cover gids 5 AND 10 with delta +100.
    fn build_chain_context_format1_gsub() -> Vec<u8> {
        // SingleSubst Format 1: covers [5, 10], delta = +100.
        let mut cov0 = Vec::new();
        cov0.extend_from_slice(&1u16.to_be_bytes()); // format
        cov0.extend_from_slice(&2u16.to_be_bytes()); // glyphCount
        cov0.extend_from_slice(&5u16.to_be_bytes());
        cov0.extend_from_slice(&10u16.to_be_bytes());
        let mut sub0 = Vec::new();
        sub0.extend_from_slice(&1u16.to_be_bytes()); // format
        sub0.extend_from_slice(&6u16.to_be_bytes()); // coverageOffset
        sub0.extend_from_slice(&100i16.to_be_bytes()); // delta
        sub0.extend_from_slice(&cov0);

        // ChainContextSubstFormat1
        // Layout we want:
        //   header (8 bytes: format + cov_off + setCount + setOffset[1])
        //   coverage covering gid 10
        //   ChainSubRuleSet: count=1, offset to rule
        //   ChainSubRule: bt=[1], in=[10] (count=1, no extra), la=[99],
        //                 substRecords=[(seq=0, lookup=0)]
        let mut cov_in = Vec::new();
        cov_in.extend_from_slice(&1u16.to_be_bytes());
        cov_in.extend_from_slice(&1u16.to_be_bytes());
        cov_in.extend_from_slice(&10u16.to_be_bytes());
        // Rule body
        let mut rule = Vec::new();
        rule.extend_from_slice(&1u16.to_be_bytes()); // backtrackGlyphCount
        rule.extend_from_slice(&1u16.to_be_bytes()); // backtrack[0] = gid 1
        rule.extend_from_slice(&1u16.to_be_bytes()); // inputGlyphCount = 1
                                                     // (no extra inputs)
        rule.extend_from_slice(&1u16.to_be_bytes()); // lookaheadGlyphCount
        rule.extend_from_slice(&99u16.to_be_bytes()); // lookahead[0]
        rule.extend_from_slice(&1u16.to_be_bytes()); // substCount
        rule.extend_from_slice(&0u16.to_be_bytes()); // sequenceIndex
        rule.extend_from_slice(&0u16.to_be_bytes()); // lookupListIndex

        // ChainSubRuleSet
        let rule_set_header_len = 4u16; // count + 1 offset
        let mut rule_set = Vec::new();
        rule_set.extend_from_slice(&1u16.to_be_bytes()); // ruleCount
        rule_set.extend_from_slice(&rule_set_header_len.to_be_bytes()); // offset
        rule_set.extend_from_slice(&rule);

        // Sub-table header
        let header_len = 8u16; // format + cov + setCount + setOffset
        let cov_off = header_len;
        let set_off = cov_off + cov_in.len() as u16;
        let mut sub1 = Vec::new();
        sub1.extend_from_slice(&1u16.to_be_bytes()); // format
        sub1.extend_from_slice(&cov_off.to_be_bytes());
        sub1.extend_from_slice(&1u16.to_be_bytes()); // setCount
        sub1.extend_from_slice(&set_off.to_be_bytes());
        sub1.extend_from_slice(&cov_in);
        sub1.extend_from_slice(&rule_set);

        fn wrap_lookup(lookup_type: u16, sub: &[u8]) -> Vec<u8> {
            let mut out = Vec::new();
            out.extend_from_slice(&lookup_type.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes()); // flag
            out.extend_from_slice(&1u16.to_be_bytes()); // subCount
            out.extend_from_slice(&8u16.to_be_bytes()); // subOffset
            out.extend_from_slice(sub);
            out
        }
        let lookup0 = wrap_lookup(LOOKUP_SINGLE_SUBST, &sub0);
        let lookup1 = wrap_lookup(LOOKUP_CHAIN_CONTEXT_SUBST, &sub1);

        // LookupList
        let lookup_list_header_len = 2 + 2 * 2;
        let mut lookup_list = Vec::new();
        lookup_list.extend_from_slice(&2u16.to_be_bytes());
        let mut running = lookup_list_header_len as u16;
        lookup_list.extend_from_slice(&running.to_be_bytes());
        running += lookup0.len() as u16;
        lookup_list.extend_from_slice(&running.to_be_bytes());
        lookup_list.extend_from_slice(&lookup0);
        lookup_list.extend_from_slice(&lookup1);

        // GSUB header — no script/feature lists.
        let mut gsub = Vec::new();
        gsub.extend_from_slice(&1u16.to_be_bytes()); // major
        gsub.extend_from_slice(&0u16.to_be_bytes()); // minor
        gsub.extend_from_slice(&0u16.to_be_bytes()); // scriptList
        gsub.extend_from_slice(&0u16.to_be_bytes()); // featureList
        gsub.extend_from_slice(&10u16.to_be_bytes()); // lookupList
        gsub.extend_from_slice(&lookup_list);
        gsub
    }

    #[test]
    fn gsub_lookup_type_6_format_1_chained_context_simple_sequence() {
        let bytes = build_chain_context_format1_gsub();
        let g = GsubTable::parse(&bytes).unwrap();
        // Run is [1, 10, 99]. Apply chain-context lookup 1 at pos=1.
        // Backtrack [1] matches; input [10] matches; lookahead [99]
        // matches; sub-record (seq=0, lookup=0) → SingleSubst delta+100.
        let out = g.apply_lookup_type_6(1, &[1, 10, 99], 1).unwrap();
        assert_eq!(out, vec![1, 110, 99]);
    }

    #[test]
    fn chain_context_format1_no_match_when_backtrack_differs() {
        let bytes = build_chain_context_format1_gsub();
        let g = GsubTable::parse(&bytes).unwrap();
        // Backtrack expects gid 1 immediately before pos. Use gid 2.
        assert_eq!(g.apply_lookup_type_6(1, &[2, 10, 99], 1), None);
        // No backtrack room (pos=0).
        assert_eq!(g.apply_lookup_type_6(1, &[10, 99], 0), None);
        // Wrong lookahead.
        assert_eq!(g.apply_lookup_type_6(1, &[1, 10, 50], 1), None);
        // Out-of-range lookup index → None.
        assert_eq!(g.apply_lookup_type_6(99, &[1, 10, 99], 1), None);
    }

    /// Build a Format-3 chain-context sub-table:
    /// backtrack covers [1], input covers [10], lookahead covers [99],
    /// invoke single-subst lookup 0 (gids [5,10] → +100).
    fn build_chain_context_format3_gsub() -> Vec<u8> {
        // Same SingleSubst lookup 0 as Format-1 build.
        let mut cov_lookup0 = Vec::new();
        cov_lookup0.extend_from_slice(&1u16.to_be_bytes());
        cov_lookup0.extend_from_slice(&2u16.to_be_bytes());
        cov_lookup0.extend_from_slice(&5u16.to_be_bytes());
        cov_lookup0.extend_from_slice(&10u16.to_be_bytes());
        let mut sub0 = Vec::new();
        sub0.extend_from_slice(&1u16.to_be_bytes());
        sub0.extend_from_slice(&6u16.to_be_bytes());
        sub0.extend_from_slice(&100i16.to_be_bytes());
        sub0.extend_from_slice(&cov_lookup0);

        // Three coverages for the chain.
        let mut cov_bt = Vec::new();
        cov_bt.extend_from_slice(&1u16.to_be_bytes());
        cov_bt.extend_from_slice(&1u16.to_be_bytes());
        cov_bt.extend_from_slice(&1u16.to_be_bytes());
        let mut cov_in = Vec::new();
        cov_in.extend_from_slice(&1u16.to_be_bytes());
        cov_in.extend_from_slice(&1u16.to_be_bytes());
        cov_in.extend_from_slice(&10u16.to_be_bytes());
        let mut cov_la = Vec::new();
        cov_la.extend_from_slice(&1u16.to_be_bytes());
        cov_la.extend_from_slice(&1u16.to_be_bytes());
        cov_la.extend_from_slice(&99u16.to_be_bytes());

        // Format-3 sub-table:
        // u16 format=3
        // u16 bt_count=1, Offset16 bt_off[1]
        // u16 in_count=1, Offset16 in_off[1]
        // u16 la_count=1, Offset16 la_off[1]
        // u16 substCount=1, SubstLookupRecord[1] = (0, 0)
        // header = 2 + (2+2) + (2+2) + (2+2) + 2 + 4 = 18 bytes
        let header_len: u16 = 2 + 2 + 2 + 2 + 2 + 2 + 2 + 2 + 4;
        let bt_off = header_len;
        let in_off = bt_off + cov_bt.len() as u16;
        let la_off = in_off + cov_in.len() as u16;

        let mut sub1 = Vec::new();
        sub1.extend_from_slice(&3u16.to_be_bytes()); // format
        sub1.extend_from_slice(&1u16.to_be_bytes()); // btCount
        sub1.extend_from_slice(&bt_off.to_be_bytes());
        sub1.extend_from_slice(&1u16.to_be_bytes()); // inCount
        sub1.extend_from_slice(&in_off.to_be_bytes());
        sub1.extend_from_slice(&1u16.to_be_bytes()); // laCount
        sub1.extend_from_slice(&la_off.to_be_bytes());
        sub1.extend_from_slice(&1u16.to_be_bytes()); // substCount
        sub1.extend_from_slice(&0u16.to_be_bytes()); // seqIndex
        sub1.extend_from_slice(&0u16.to_be_bytes()); // lookupIndex
        sub1.extend_from_slice(&cov_bt);
        sub1.extend_from_slice(&cov_in);
        sub1.extend_from_slice(&cov_la);

        fn wrap_lookup(lookup_type: u16, sub: &[u8]) -> Vec<u8> {
            let mut out = Vec::new();
            out.extend_from_slice(&lookup_type.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(&1u16.to_be_bytes());
            out.extend_from_slice(&8u16.to_be_bytes());
            out.extend_from_slice(sub);
            out
        }
        let lookup0 = wrap_lookup(LOOKUP_SINGLE_SUBST, &sub0);
        let lookup1 = wrap_lookup(LOOKUP_CHAIN_CONTEXT_SUBST, &sub1);

        let lookup_list_header_len = 2 + 2 * 2;
        let mut lookup_list = Vec::new();
        lookup_list.extend_from_slice(&2u16.to_be_bytes());
        let mut running = lookup_list_header_len as u16;
        lookup_list.extend_from_slice(&running.to_be_bytes());
        running += lookup0.len() as u16;
        lookup_list.extend_from_slice(&running.to_be_bytes());
        lookup_list.extend_from_slice(&lookup0);
        lookup_list.extend_from_slice(&lookup1);

        let mut gsub = Vec::new();
        gsub.extend_from_slice(&1u16.to_be_bytes());
        gsub.extend_from_slice(&0u16.to_be_bytes());
        gsub.extend_from_slice(&0u16.to_be_bytes());
        gsub.extend_from_slice(&0u16.to_be_bytes());
        gsub.extend_from_slice(&10u16.to_be_bytes());
        gsub.extend_from_slice(&lookup_list);
        gsub
    }

    #[test]
    fn gsub_lookup_type_6_format_3_coverage_based_chained_context() {
        let bytes = build_chain_context_format3_gsub();
        let g = GsubTable::parse(&bytes).unwrap();
        let out = g.apply_lookup_type_6(1, &[1, 10, 99], 1).unwrap();
        assert_eq!(out, vec![1, 110, 99]);
    }

    #[test]
    fn chain_context_format3_no_match_when_window_short() {
        let bytes = build_chain_context_format3_gsub();
        let g = GsubTable::parse(&bytes).unwrap();
        // Backtrack required, pos=0 leaves none.
        assert_eq!(g.apply_lookup_type_6(1, &[10, 99], 0), None);
        // No lookahead.
        assert_eq!(g.apply_lookup_type_6(1, &[1, 10], 1), None);
        // Lookahead glyph not covered.
        assert_eq!(g.apply_lookup_type_6(1, &[1, 10, 12], 1), None);
    }

    /// Build a Format-2 (class-based) chain-context sub-table:
    /// - inputClassDef: gid 10 → class 1
    /// - backtrackClassDef: gid 1 → class 2
    /// - lookaheadClassDef: gid 99 → class 3
    /// - rule under set[class=1]: bt=[2], in=[1] (count=1), la=[3],
    ///   subst=(0, 0) → invokes single-subst lookup 0.
    fn build_chain_context_format2_gsub() -> Vec<u8> {
        // SingleSubst lookup 0 — covers gid 10 with delta +100.
        let mut cov_lookup0 = Vec::new();
        cov_lookup0.extend_from_slice(&1u16.to_be_bytes());
        cov_lookup0.extend_from_slice(&1u16.to_be_bytes());
        cov_lookup0.extend_from_slice(&10u16.to_be_bytes());
        let mut sub0 = Vec::new();
        sub0.extend_from_slice(&1u16.to_be_bytes());
        sub0.extend_from_slice(&6u16.to_be_bytes());
        sub0.extend_from_slice(&100i16.to_be_bytes());
        sub0.extend_from_slice(&cov_lookup0);

        // Format-2 sub-table header:
        //   u16 format=2, Offset16 cov, Offset16 btCD, Offset16 inCD,
        //     Offset16 laCD, u16 setCount, Offset16 setOffsets[setCount]
        // Coverage on input[0]: covers gid 10.
        let mut cov_in = Vec::new();
        cov_in.extend_from_slice(&1u16.to_be_bytes());
        cov_in.extend_from_slice(&1u16.to_be_bytes());
        cov_in.extend_from_slice(&10u16.to_be_bytes());

        // ClassDefs (format 1: startGlyph + count + classes[])
        // bt: gid 1 → class 2
        let mut bt_cd = Vec::new();
        bt_cd.extend_from_slice(&1u16.to_be_bytes()); // format
        bt_cd.extend_from_slice(&1u16.to_be_bytes()); // startGlyph
        bt_cd.extend_from_slice(&1u16.to_be_bytes()); // count
        bt_cd.extend_from_slice(&2u16.to_be_bytes()); // class
                                                      // in: gid 10 → class 1
        let mut in_cd = Vec::new();
        in_cd.extend_from_slice(&1u16.to_be_bytes());
        in_cd.extend_from_slice(&10u16.to_be_bytes());
        in_cd.extend_from_slice(&1u16.to_be_bytes());
        in_cd.extend_from_slice(&1u16.to_be_bytes());
        // la: gid 99 → class 3
        let mut la_cd = Vec::new();
        la_cd.extend_from_slice(&1u16.to_be_bytes());
        la_cd.extend_from_slice(&99u16.to_be_bytes());
        la_cd.extend_from_slice(&1u16.to_be_bytes());
        la_cd.extend_from_slice(&3u16.to_be_bytes());

        // ChainSubClassRule:
        //   u16 backtrackGlyphCount=1, u16 bt[1]=2
        //   u16 inputGlyphCount=1, (no extra)
        //   u16 lookaheadGlyphCount=1, u16 la[1]=3
        //   u16 substCount=1, SubstLookupRecord (0,0)
        let mut rule = Vec::new();
        rule.extend_from_slice(&1u16.to_be_bytes()); // bt count
        rule.extend_from_slice(&2u16.to_be_bytes()); // bt class 2
        rule.extend_from_slice(&1u16.to_be_bytes()); // in count
        rule.extend_from_slice(&1u16.to_be_bytes()); // la count
        rule.extend_from_slice(&3u16.to_be_bytes()); // la class 3
        rule.extend_from_slice(&1u16.to_be_bytes()); // substCount
        rule.extend_from_slice(&0u16.to_be_bytes()); // seqIndex
        rule.extend_from_slice(&0u16.to_be_bytes()); // lookupIndex

        // ChainSubClassSet for input class 1:
        //   u16 ruleCount=1, Offset16 ruleOffsets[1]
        let rule_set_header_len = 4u16;
        let mut rule_set = Vec::new();
        rule_set.extend_from_slice(&1u16.to_be_bytes());
        rule_set.extend_from_slice(&rule_set_header_len.to_be_bytes());
        rule_set.extend_from_slice(&rule);

        // setCount = number of distinct input classes including 0. We
        // need entries [0, 1] → setCount=2; offset for class 0 = 0
        // (no rules), offset for class 1 = past header + coverage + 3 CDs.
        // Header layout is:
        //   2 (fmt) + 2 (cov) + 2 (btCD) + 2 (inCD) + 2 (laCD)
        //   + 2 (setCount) + 2*2 (setOffsets[2]) = 16 bytes
        let header_len: u16 = 2 + 2 + 2 + 2 + 2 + 2 + 4;
        let cov_off = header_len;
        let bt_cd_off = cov_off + cov_in.len() as u16;
        let in_cd_off = bt_cd_off + bt_cd.len() as u16;
        let la_cd_off = in_cd_off + in_cd.len() as u16;
        let class1_set_off = la_cd_off + la_cd.len() as u16;

        let mut sub1 = Vec::new();
        sub1.extend_from_slice(&2u16.to_be_bytes()); // format
        sub1.extend_from_slice(&cov_off.to_be_bytes());
        sub1.extend_from_slice(&bt_cd_off.to_be_bytes());
        sub1.extend_from_slice(&in_cd_off.to_be_bytes());
        sub1.extend_from_slice(&la_cd_off.to_be_bytes());
        sub1.extend_from_slice(&2u16.to_be_bytes()); // setCount
        sub1.extend_from_slice(&0u16.to_be_bytes()); // class 0 set offset
        sub1.extend_from_slice(&class1_set_off.to_be_bytes()); // class 1
        sub1.extend_from_slice(&cov_in);
        sub1.extend_from_slice(&bt_cd);
        sub1.extend_from_slice(&in_cd);
        sub1.extend_from_slice(&la_cd);
        sub1.extend_from_slice(&rule_set);

        fn wrap_lookup(lookup_type: u16, sub: &[u8]) -> Vec<u8> {
            let mut out = Vec::new();
            out.extend_from_slice(&lookup_type.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(&1u16.to_be_bytes());
            out.extend_from_slice(&8u16.to_be_bytes());
            out.extend_from_slice(sub);
            out
        }
        let lookup0 = wrap_lookup(LOOKUP_SINGLE_SUBST, &sub0);
        let lookup1 = wrap_lookup(LOOKUP_CHAIN_CONTEXT_SUBST, &sub1);

        let lookup_list_header_len = 2 + 2 * 2;
        let mut lookup_list = Vec::new();
        lookup_list.extend_from_slice(&2u16.to_be_bytes());
        let mut running = lookup_list_header_len as u16;
        lookup_list.extend_from_slice(&running.to_be_bytes());
        running += lookup0.len() as u16;
        lookup_list.extend_from_slice(&running.to_be_bytes());
        lookup_list.extend_from_slice(&lookup0);
        lookup_list.extend_from_slice(&lookup1);

        let mut gsub = Vec::new();
        gsub.extend_from_slice(&1u16.to_be_bytes());
        gsub.extend_from_slice(&0u16.to_be_bytes());
        gsub.extend_from_slice(&0u16.to_be_bytes());
        gsub.extend_from_slice(&0u16.to_be_bytes());
        gsub.extend_from_slice(&10u16.to_be_bytes());
        gsub.extend_from_slice(&lookup_list);
        gsub
    }

    #[test]
    fn chain_context_format2_class_based_substitutes() {
        let bytes = build_chain_context_format2_gsub();
        let g = GsubTable::parse(&bytes).unwrap();
        let out = g.apply_lookup_type_6(1, &[1, 10, 99], 1).unwrap();
        assert_eq!(out, vec![1, 110, 99]);
    }

    #[test]
    fn chain_context_format2_no_match_when_class_differs() {
        let bytes = build_chain_context_format2_gsub();
        let g = GsubTable::parse(&bytes).unwrap();
        // Replace gid 1 (class 2) with gid 2 (class 0) in backtrack.
        assert_eq!(g.apply_lookup_type_6(1, &[2, 10, 99], 1), None);
    }

    /// Wrap a single sub-table in a Lookup record.
    fn wrap_lookup_helper(lookup_type: u16, sub: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&lookup_type.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes()); // flag
        out.extend_from_slice(&1u16.to_be_bytes()); // subTableCount
        out.extend_from_slice(&8u16.to_be_bytes()); // subTableOffset = 8
        out.extend_from_slice(sub);
        out
    }

    /// Build a GSUB blob that holds a single lookup at index 0 of the
    /// given lookup_type, wrapping the supplied sub-table bytes.
    fn build_singleton_gsub(lookup_type: u16, sub: &[u8]) -> Vec<u8> {
        let lookup = wrap_lookup_helper(lookup_type, sub);
        let lookup_list_header_len = 2 + 2; // count + 1 offset
        let mut lookup_list = Vec::new();
        lookup_list.extend_from_slice(&1u16.to_be_bytes());
        lookup_list.extend_from_slice(&(lookup_list_header_len as u16).to_be_bytes());
        lookup_list.extend_from_slice(&lookup);

        let mut gsub = Vec::new();
        gsub.extend_from_slice(&1u16.to_be_bytes()); // major
        gsub.extend_from_slice(&0u16.to_be_bytes()); // minor
        gsub.extend_from_slice(&0u16.to_be_bytes()); // scriptList
        gsub.extend_from_slice(&0u16.to_be_bytes()); // featureList
        gsub.extend_from_slice(&10u16.to_be_bytes()); // lookupList
        gsub.extend_from_slice(&lookup_list);
        gsub
    }

    // ----- LookupType 2: Multiple Substitution -----

    /// MultipleSubstFormat1 with one Sequence: gid 7 → [10, 11, 12].
    fn build_mult_subst_sub() -> Vec<u8> {
        // Sequence: glyphCount=3, [10, 11, 12]
        let mut seq = Vec::new();
        seq.extend_from_slice(&3u16.to_be_bytes());
        seq.extend_from_slice(&10u16.to_be_bytes());
        seq.extend_from_slice(&11u16.to_be_bytes());
        seq.extend_from_slice(&12u16.to_be_bytes());

        // Coverage Format 1 covering gid 7.
        let mut cov = Vec::new();
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&7u16.to_be_bytes());

        // Header: u16 fmt(=1), Off16 cov, u16 seqCount, Off16 seqOffsets[1]
        let header_len = 8u16;
        let cov_off = header_len;
        let seq_off = cov_off + cov.len() as u16;

        let mut sub = Vec::new();
        sub.extend_from_slice(&1u16.to_be_bytes()); // format
        sub.extend_from_slice(&cov_off.to_be_bytes());
        sub.extend_from_slice(&1u16.to_be_bytes()); // seqCount
        sub.extend_from_slice(&seq_off.to_be_bytes());
        sub.extend_from_slice(&cov);
        sub.extend_from_slice(&seq);
        sub
    }

    #[test]
    fn lookup_type_2_expands_one_glyph_into_sequence() {
        let bytes = build_singleton_gsub(LOOKUP_MULTIPLE_SUBST, &build_mult_subst_sub());
        let g = GsubTable::parse(&bytes).unwrap();
        assert_eq!(g.apply_lookup_type_2(0, 7), Some(vec![10, 11, 12]));
    }

    #[test]
    fn lookup_type_2_returns_none_off_coverage() {
        let bytes = build_singleton_gsub(LOOKUP_MULTIPLE_SUBST, &build_mult_subst_sub());
        let g = GsubTable::parse(&bytes).unwrap();
        assert_eq!(g.apply_lookup_type_2(0, 99), None);
        // Wrong lookup index.
        assert_eq!(g.apply_lookup_type_2(99, 7), None);
        // Wrong lookup type.
        let single_bytes = build_singleton_gsub(LOOKUP_SINGLE_SUBST, &build_mult_subst_sub());
        let g2 = GsubTable::parse(&single_bytes).unwrap();
        assert_eq!(g2.apply_lookup_type_2(0, 7), None);
    }

    #[test]
    fn lookup_type_2_zero_glyph_count_means_deletion() {
        // Build a Sequence record with glyphCount = 0 (legal per spec).
        let mut seq = Vec::new();
        seq.extend_from_slice(&0u16.to_be_bytes());
        let mut cov = Vec::new();
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&7u16.to_be_bytes());
        let header_len = 8u16;
        let cov_off = header_len;
        let seq_off = cov_off + cov.len() as u16;
        let mut sub = Vec::new();
        sub.extend_from_slice(&1u16.to_be_bytes());
        sub.extend_from_slice(&cov_off.to_be_bytes());
        sub.extend_from_slice(&1u16.to_be_bytes());
        sub.extend_from_slice(&seq_off.to_be_bytes());
        sub.extend_from_slice(&cov);
        sub.extend_from_slice(&seq);
        let bytes = build_singleton_gsub(LOOKUP_MULTIPLE_SUBST, &sub);
        let g = GsubTable::parse(&bytes).unwrap();
        assert_eq!(g.apply_lookup_type_2(0, 7), Some(Vec::new()));
    }

    // ----- LookupType 3: Alternate Substitution -----

    /// AlternateSubstFormat1 with one AlternateSet: gid 5 → [50, 51, 52].
    fn build_alt_subst_sub() -> Vec<u8> {
        let mut alt_set = Vec::new();
        alt_set.extend_from_slice(&3u16.to_be_bytes()); // glyphCount
        alt_set.extend_from_slice(&50u16.to_be_bytes());
        alt_set.extend_from_slice(&51u16.to_be_bytes());
        alt_set.extend_from_slice(&52u16.to_be_bytes());

        let mut cov = Vec::new();
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&5u16.to_be_bytes());

        let header_len = 8u16;
        let cov_off = header_len;
        let alt_off = cov_off + cov.len() as u16;

        let mut sub = Vec::new();
        sub.extend_from_slice(&1u16.to_be_bytes()); // format
        sub.extend_from_slice(&cov_off.to_be_bytes());
        sub.extend_from_slice(&1u16.to_be_bytes()); // altCount
        sub.extend_from_slice(&alt_off.to_be_bytes());
        sub.extend_from_slice(&cov);
        sub.extend_from_slice(&alt_set);
        sub
    }

    #[test]
    fn lookup_type_3_picks_default_alternate_zero() {
        let bytes = build_singleton_gsub(LOOKUP_ALTERNATE_SUBST, &build_alt_subst_sub());
        let g = GsubTable::parse(&bytes).unwrap();
        assert_eq!(g.apply_lookup_type_3(0, 5, 0), Some(50));
    }

    #[test]
    fn lookup_type_3_picks_indexed_alternates() {
        let bytes = build_singleton_gsub(LOOKUP_ALTERNATE_SUBST, &build_alt_subst_sub());
        let g = GsubTable::parse(&bytes).unwrap();
        assert_eq!(g.apply_lookup_type_3(0, 5, 1), Some(51));
        assert_eq!(g.apply_lookup_type_3(0, 5, 2), Some(52));
    }

    #[test]
    fn lookup_type_3_out_of_range_alternate_returns_none() {
        let bytes = build_singleton_gsub(LOOKUP_ALTERNATE_SUBST, &build_alt_subst_sub());
        let g = GsubTable::parse(&bytes).unwrap();
        // AlternateSet has 3 entries; index 3 is out of range.
        assert_eq!(g.apply_lookup_type_3(0, 5, 3), None);
        // Off-coverage glyph.
        assert_eq!(g.apply_lookup_type_3(0, 99, 0), None);
        // Wrong lookup type silently returns None.
        let other = build_singleton_gsub(LOOKUP_SINGLE_SUBST, &build_alt_subst_sub());
        let g2 = GsubTable::parse(&other).unwrap();
        assert_eq!(g2.apply_lookup_type_3(0, 5, 0), None);
    }

    // ----- lookupFlag.markFilteringSet (§6.2.2) -----

    /// Hand-build a GSUB whose single lookup carries
    /// `flags = 0x0210` (MARK_ATTACHMENT_CLASS_FILTER class 2 +
    /// USE_MARK_FILTERING_SET) and a trailing `markFilteringSet = 3`,
    /// then confirm the accessor reads the field at
    /// `6 + 2 * subTableCount` and reports `None` when the flag is clear.
    #[test]
    fn mark_filtering_set_reads_trailing_field() {
        // Lookup with USE_MARK_FILTERING_SET → markFilteringSet present.
        // Layout: lookupType(1) flag(0x0210) subTableCount(1)
        //         subtableOffsets[1] markFilteringSet(3) [subtable body].
        // The subtable body is irrelevant to this accessor; point the
        // offset past the markFilteringSet field at byte 8.
        let mut lookup = Vec::new();
        lookup.extend_from_slice(&1u16.to_be_bytes()); // lookupType
        lookup.extend_from_slice(&0x0210u16.to_be_bytes()); // lookupFlag
        lookup.extend_from_slice(&1u16.to_be_bytes()); // subTableCount
        lookup.extend_from_slice(&10u16.to_be_bytes()); // subtableOffsets[0]
        lookup.extend_from_slice(&3u16.to_be_bytes()); // markFilteringSet
                                                       // pad to the subtable offset (10) with two dummy bytes.
        lookup.extend_from_slice(&[0u8, 0u8]);

        let mut lookup_list = Vec::new();
        lookup_list.extend_from_slice(&1u16.to_be_bytes()); // count
        lookup_list.extend_from_slice(&4u16.to_be_bytes()); // offset[0]
        lookup_list.extend_from_slice(&lookup);

        let mut gsub = Vec::new();
        gsub.extend_from_slice(&1u16.to_be_bytes()); // major
        gsub.extend_from_slice(&0u16.to_be_bytes()); // minor
        gsub.extend_from_slice(&0u16.to_be_bytes()); // scriptList
        gsub.extend_from_slice(&0u16.to_be_bytes()); // featureList
        gsub.extend_from_slice(&10u16.to_be_bytes()); // lookupList
        gsub.extend_from_slice(&lookup_list);

        let g = GsubTable::parse(&gsub).unwrap();
        assert_eq!(g.lookup_flags(0), 0x0210);
        assert_eq!(g.mark_filtering_set(0), Some(3));

        // A lookup without the 0x0010 bit reports None even though the
        // trailing bytes would parse as a u16.
        let plain = build_singleton_gsub(LOOKUP_ALTERNATE_SUBST, &build_alt_subst_sub());
        let gp = GsubTable::parse(&plain).unwrap();
        assert_eq!(gp.mark_filtering_set(0), None);
        // Out-of-range index is None.
        assert_eq!(g.mark_filtering_set(99), None);
    }

    // ----- LookupType 5: Contextual Substitution -----

    /// Build a SingleSubst Format-1 lookup body: covers [10], delta = +100.
    fn build_singlesubst_for_10_plus_100() -> Vec<u8> {
        let mut cov = Vec::new();
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&10u16.to_be_bytes());
        let mut sub = Vec::new();
        sub.extend_from_slice(&1u16.to_be_bytes()); // format
        sub.extend_from_slice(&6u16.to_be_bytes()); // coverageOffset
        sub.extend_from_slice(&100i16.to_be_bytes()); // delta
        sub.extend_from_slice(&cov);
        sub
    }

    /// Build a SequenceContextFormat1 sub-table: at gid 10 with input
    /// run [10] (count=1), invoke single-subst lookup 0.
    fn build_context_format1_sub() -> Vec<u8> {
        // Coverage covering gid 10.
        let mut cov = Vec::new();
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&1u16.to_be_bytes());
        cov.extend_from_slice(&10u16.to_be_bytes());

        // Rule: inputGlyphCount=1, no extras, substCount=1, (seq=0,
        // lookup=0)
        let mut rule = Vec::new();
        rule.extend_from_slice(&1u16.to_be_bytes()); // inputGlyphCount
        rule.extend_from_slice(&1u16.to_be_bytes()); // substCount
        rule.extend_from_slice(&0u16.to_be_bytes()); // seqIndex
        rule.extend_from_slice(&0u16.to_be_bytes()); // lookupIndex

        // SubRuleSet: count=1, offset to rule.
        let rule_set_header = 4u16; // count + 1 offset
        let mut rule_set = Vec::new();
        rule_set.extend_from_slice(&1u16.to_be_bytes());
        rule_set.extend_from_slice(&rule_set_header.to_be_bytes());
        rule_set.extend_from_slice(&rule);

        // Header: u16 fmt(=1), Off16 cov, u16 setCount, Off16 setOffsets[1]
        let header_len = 8u16;
        let cov_off = header_len;
        let set_off = cov_off + cov.len() as u16;
        let mut sub = Vec::new();
        sub.extend_from_slice(&1u16.to_be_bytes()); // format
        sub.extend_from_slice(&cov_off.to_be_bytes());
        sub.extend_from_slice(&1u16.to_be_bytes()); // setCount
        sub.extend_from_slice(&set_off.to_be_bytes());
        sub.extend_from_slice(&cov);
        sub.extend_from_slice(&rule_set);
        sub
    }

    fn build_context_lookup_gsub(context_format_sub: Vec<u8>) -> Vec<u8> {
        let lookup0 = wrap_lookup_helper(LOOKUP_SINGLE_SUBST, &build_singlesubst_for_10_plus_100());
        let lookup1 = wrap_lookup_helper(LOOKUP_CONTEXT_SUBST, &context_format_sub);
        let lookup_list_header = 2 + 2 * 2;
        let mut lookup_list = Vec::new();
        lookup_list.extend_from_slice(&2u16.to_be_bytes());
        let mut running = lookup_list_header as u16;
        lookup_list.extend_from_slice(&running.to_be_bytes());
        running += lookup0.len() as u16;
        lookup_list.extend_from_slice(&running.to_be_bytes());
        lookup_list.extend_from_slice(&lookup0);
        lookup_list.extend_from_slice(&lookup1);

        let mut gsub = Vec::new();
        gsub.extend_from_slice(&1u16.to_be_bytes());
        gsub.extend_from_slice(&0u16.to_be_bytes());
        gsub.extend_from_slice(&0u16.to_be_bytes());
        gsub.extend_from_slice(&0u16.to_be_bytes());
        gsub.extend_from_slice(&10u16.to_be_bytes());
        gsub.extend_from_slice(&lookup_list);
        gsub
    }

    #[test]
    fn lookup_type_5_format_1_simple_glyph_context() {
        let bytes = build_context_lookup_gsub(build_context_format1_sub());
        let g = GsubTable::parse(&bytes).unwrap();
        // Run [10] at pos=0: input matches; SingleSubst delta+100.
        let out = g.apply_lookup_type_5(1, &[10], 0).unwrap();
        assert_eq!(out, vec![110]);
        // Off-coverage glyph → no match.
        assert_eq!(g.apply_lookup_type_5(1, &[11], 0), None);
        // Out-of-range lookup → no match.
        assert_eq!(g.apply_lookup_type_5(99, &[10], 0), None);
    }

    /// SequenceContextFormat3: glyphCount=1 covering gid 10, invoke
    /// single-subst lookup 0.
    fn build_context_format3_sub() -> Vec<u8> {
        let mut cov_in = Vec::new();
        cov_in.extend_from_slice(&1u16.to_be_bytes());
        cov_in.extend_from_slice(&1u16.to_be_bytes());
        cov_in.extend_from_slice(&10u16.to_be_bytes());

        // Header: u16 fmt(=3), u16 glyphCount, u16 substCount,
        // Off16 covOffsets[1], SubstLookupRecord rec(=4 bytes)
        let header_len: u16 = 2 + 2 + 2 + 2 + 4;
        let cov_off = header_len;
        let mut sub = Vec::new();
        sub.extend_from_slice(&3u16.to_be_bytes()); // format
        sub.extend_from_slice(&1u16.to_be_bytes()); // glyphCount
        sub.extend_from_slice(&1u16.to_be_bytes()); // substCount
        sub.extend_from_slice(&cov_off.to_be_bytes());
        sub.extend_from_slice(&0u16.to_be_bytes()); // seqIndex
        sub.extend_from_slice(&0u16.to_be_bytes()); // lookupIndex
        sub.extend_from_slice(&cov_in);
        sub
    }

    #[test]
    fn lookup_type_5_format_3_coverage_based_context() {
        let bytes = build_context_lookup_gsub(build_context_format3_sub());
        let g = GsubTable::parse(&bytes).unwrap();
        let out = g.apply_lookup_type_5(1, &[10], 0).unwrap();
        assert_eq!(out, vec![110]);
        // Coverage misses gid 11.
        assert_eq!(g.apply_lookup_type_5(1, &[11], 0), None);
    }

    /// SequenceContextFormat2: ClassDef maps gid 10 → class 1,
    /// rule under class-1 set runs single-subst lookup 0.
    fn build_context_format2_sub() -> Vec<u8> {
        let mut cov_in = Vec::new();
        cov_in.extend_from_slice(&1u16.to_be_bytes());
        cov_in.extend_from_slice(&1u16.to_be_bytes());
        cov_in.extend_from_slice(&10u16.to_be_bytes());

        // ClassDef format 1: startGlyph=10, count=1, classes=[1]
        let mut cd = Vec::new();
        cd.extend_from_slice(&1u16.to_be_bytes());
        cd.extend_from_slice(&10u16.to_be_bytes());
        cd.extend_from_slice(&1u16.to_be_bytes());
        cd.extend_from_slice(&1u16.to_be_bytes());

        // Rule: glyphCount=1, no extras, substCount=1, (seq=0, lookup=0)
        let mut rule = Vec::new();
        rule.extend_from_slice(&1u16.to_be_bytes());
        rule.extend_from_slice(&1u16.to_be_bytes());
        rule.extend_from_slice(&0u16.to_be_bytes());
        rule.extend_from_slice(&0u16.to_be_bytes());

        let rule_set_header = 4u16;
        let mut rule_set = Vec::new();
        rule_set.extend_from_slice(&1u16.to_be_bytes());
        rule_set.extend_from_slice(&rule_set_header.to_be_bytes());
        rule_set.extend_from_slice(&rule);

        // Header: u16 fmt(=2), Off16 cov, Off16 cd, u16 setCount,
        // Off16 setOffsets[2] (class 0 + class 1)
        let header_len: u16 = 2 + 2 + 2 + 2 + 4;
        let cov_off = header_len;
        let cd_off = cov_off + cov_in.len() as u16;
        let class1_set_off = cd_off + cd.len() as u16;

        let mut sub = Vec::new();
        sub.extend_from_slice(&2u16.to_be_bytes()); // format
        sub.extend_from_slice(&cov_off.to_be_bytes());
        sub.extend_from_slice(&cd_off.to_be_bytes());
        sub.extend_from_slice(&2u16.to_be_bytes()); // setCount
        sub.extend_from_slice(&0u16.to_be_bytes()); // class 0 set offset
        sub.extend_from_slice(&class1_set_off.to_be_bytes());
        sub.extend_from_slice(&cov_in);
        sub.extend_from_slice(&cd);
        sub.extend_from_slice(&rule_set);
        sub
    }

    #[test]
    fn lookup_type_5_format_2_class_based_context() {
        let bytes = build_context_lookup_gsub(build_context_format2_sub());
        let g = GsubTable::parse(&bytes).unwrap();
        let out = g.apply_lookup_type_5(1, &[10], 0).unwrap();
        assert_eq!(out, vec![110]);
        // gid 11 isn't covered.
        assert_eq!(g.apply_lookup_type_5(1, &[11], 0), None);
    }

    // ----- LookupType 8: Reverse Chained Context Substitution -----

    /// ReverseChainSingleSubstFormat1:
    /// - input coverage covers [10] → coverage index 0
    /// - backtrack[0] covers [1] (immediately preceding glyph)
    /// - lookahead[0] covers [99]
    /// - substituteGlyphIDs[0] = 200
    fn build_reverse_chain_sub() -> Vec<u8> {
        let mut cov_in = Vec::new();
        cov_in.extend_from_slice(&1u16.to_be_bytes());
        cov_in.extend_from_slice(&1u16.to_be_bytes());
        cov_in.extend_from_slice(&10u16.to_be_bytes());
        let mut cov_bt = Vec::new();
        cov_bt.extend_from_slice(&1u16.to_be_bytes());
        cov_bt.extend_from_slice(&1u16.to_be_bytes());
        cov_bt.extend_from_slice(&1u16.to_be_bytes());
        let mut cov_la = Vec::new();
        cov_la.extend_from_slice(&1u16.to_be_bytes());
        cov_la.extend_from_slice(&1u16.to_be_bytes());
        cov_la.extend_from_slice(&99u16.to_be_bytes());

        // Header: u16 fmt(=1), Off16 cov, u16 btCount, Off16 btCov[1],
        //         u16 laCount, Off16 laCov[1], u16 glyphCount, u16 sub[1]
        // size = 2 + 2 + 2+2 + 2+2 + 2+2 = 16
        let header_len: u16 = 2 + 2 + 2 + 2 + 2 + 2 + 2 + 2;
        let cov_off = header_len;
        let bt_off = cov_off + cov_in.len() as u16;
        let la_off = bt_off + cov_bt.len() as u16;

        let mut sub = Vec::new();
        sub.extend_from_slice(&1u16.to_be_bytes()); // format
        sub.extend_from_slice(&cov_off.to_be_bytes());
        sub.extend_from_slice(&1u16.to_be_bytes()); // btCount
        sub.extend_from_slice(&bt_off.to_be_bytes());
        sub.extend_from_slice(&1u16.to_be_bytes()); // laCount
        sub.extend_from_slice(&la_off.to_be_bytes());
        sub.extend_from_slice(&1u16.to_be_bytes()); // glyphCount
        sub.extend_from_slice(&200u16.to_be_bytes()); // substituteGlyphIDs[0]
        sub.extend_from_slice(&cov_in);
        sub.extend_from_slice(&cov_bt);
        sub.extend_from_slice(&cov_la);
        sub
    }

    #[test]
    fn lookup_type_8_reverse_chain_substitutes_under_context() {
        let bytes = build_singleton_gsub(
            LOOKUP_REVERSE_CHAIN_CONTEXT_SUBST,
            &build_reverse_chain_sub(),
        );
        let g = GsubTable::parse(&bytes).unwrap();
        // Run [1, 10, 99] at pos=1.
        assert_eq!(g.apply_lookup_type_8(0, &[1, 10, 99], 1), Some(200));
    }

    #[test]
    fn lookup_type_8_no_match_when_backtrack_or_lookahead_misses() {
        let bytes = build_singleton_gsub(
            LOOKUP_REVERSE_CHAIN_CONTEXT_SUBST,
            &build_reverse_chain_sub(),
        );
        let g = GsubTable::parse(&bytes).unwrap();
        // Wrong backtrack glyph.
        assert_eq!(g.apply_lookup_type_8(0, &[2, 10, 99], 1), None);
        // Wrong lookahead glyph.
        assert_eq!(g.apply_lookup_type_8(0, &[1, 10, 50], 1), None);
        // No backtrack room (pos=0).
        assert_eq!(g.apply_lookup_type_8(0, &[10, 99], 0), None);
        // No lookahead room.
        assert_eq!(g.apply_lookup_type_8(0, &[1, 10], 1), None);
        // Off-coverage input.
        assert_eq!(g.apply_lookup_type_8(0, &[1, 11, 99], 1), None);
        // Out-of-range lookup index.
        assert_eq!(g.apply_lookup_type_8(99, &[1, 10, 99], 1), None);
    }

    // ----- Nested-lookup dispatch through chained context -----

    /// Verify that a Chain-Context (LT6) record referencing a
    /// LookupType-2 (multiple subst) lookup correctly expands.
    #[test]
    fn chain_context_can_dispatch_nested_lookup_type_2() {
        // Lookup 0: MultipleSubst gid 10 → [10, 99] (split a glyph).
        let mult_sub = {
            let mut seq = Vec::new();
            seq.extend_from_slice(&2u16.to_be_bytes());
            seq.extend_from_slice(&10u16.to_be_bytes());
            seq.extend_from_slice(&99u16.to_be_bytes());
            let mut cov = Vec::new();
            cov.extend_from_slice(&1u16.to_be_bytes());
            cov.extend_from_slice(&1u16.to_be_bytes());
            cov.extend_from_slice(&10u16.to_be_bytes());
            let header_len = 8u16;
            let cov_off = header_len;
            let seq_off = cov_off + cov.len() as u16;
            let mut sub = Vec::new();
            sub.extend_from_slice(&1u16.to_be_bytes());
            sub.extend_from_slice(&cov_off.to_be_bytes());
            sub.extend_from_slice(&1u16.to_be_bytes());
            sub.extend_from_slice(&seq_off.to_be_bytes());
            sub.extend_from_slice(&cov);
            sub.extend_from_slice(&seq);
            sub
        };
        // Lookup 1: ChainContextFormat1 over gid 10 (no bt/la), invokes lookup 0.
        let chain_sub = {
            let mut cov = Vec::new();
            cov.extend_from_slice(&1u16.to_be_bytes());
            cov.extend_from_slice(&1u16.to_be_bytes());
            cov.extend_from_slice(&10u16.to_be_bytes());
            let mut rule = Vec::new();
            rule.extend_from_slice(&0u16.to_be_bytes()); // bt count
            rule.extend_from_slice(&1u16.to_be_bytes()); // input count
            rule.extend_from_slice(&0u16.to_be_bytes()); // la count
            rule.extend_from_slice(&1u16.to_be_bytes()); // substCount
            rule.extend_from_slice(&0u16.to_be_bytes()); // seqIndex
            rule.extend_from_slice(&0u16.to_be_bytes()); // lookupIndex
            let rule_set_header = 4u16;
            let mut rule_set = Vec::new();
            rule_set.extend_from_slice(&1u16.to_be_bytes());
            rule_set.extend_from_slice(&rule_set_header.to_be_bytes());
            rule_set.extend_from_slice(&rule);
            let header_len = 8u16;
            let cov_off = header_len;
            let set_off = cov_off + cov.len() as u16;
            let mut sub = Vec::new();
            sub.extend_from_slice(&1u16.to_be_bytes());
            sub.extend_from_slice(&cov_off.to_be_bytes());
            sub.extend_from_slice(&1u16.to_be_bytes());
            sub.extend_from_slice(&set_off.to_be_bytes());
            sub.extend_from_slice(&cov);
            sub.extend_from_slice(&rule_set);
            sub
        };
        let lookup0 = wrap_lookup_helper(LOOKUP_MULTIPLE_SUBST, &mult_sub);
        let lookup1 = wrap_lookup_helper(LOOKUP_CHAIN_CONTEXT_SUBST, &chain_sub);
        let lookup_list_header = 2 + 2 * 2;
        let mut lookup_list = Vec::new();
        lookup_list.extend_from_slice(&2u16.to_be_bytes());
        let mut running = lookup_list_header as u16;
        lookup_list.extend_from_slice(&running.to_be_bytes());
        running += lookup0.len() as u16;
        lookup_list.extend_from_slice(&running.to_be_bytes());
        lookup_list.extend_from_slice(&lookup0);
        lookup_list.extend_from_slice(&lookup1);

        let mut gsub = Vec::new();
        gsub.extend_from_slice(&1u16.to_be_bytes());
        gsub.extend_from_slice(&0u16.to_be_bytes());
        gsub.extend_from_slice(&0u16.to_be_bytes());
        gsub.extend_from_slice(&0u16.to_be_bytes());
        gsub.extend_from_slice(&10u16.to_be_bytes());
        gsub.extend_from_slice(&lookup_list);

        let g = GsubTable::parse(&gsub).unwrap();
        let out = g.apply_lookup_type_6(1, &[10], 0).unwrap();
        // Multiple subst expands gid 10 → [10, 99].
        assert_eq!(out, vec![10, 99]);
    }
}
