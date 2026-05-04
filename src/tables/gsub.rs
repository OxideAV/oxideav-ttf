//! `GSUB` — Glyph Substitution Table.
//!
//! Implemented lookup types:
//! - **LookupType 1** (Single Substitution) — formats 1 (delta) and 2
//!   (indexed substitute array). Used by Arabic shaping (`init`/`medi`/
//!   `fina`/`isol`), small-caps, vertical alternates, and most other
//!   one-in/one-out feature lookups.
//! - **LookupType 4** (Ligature Substitution) — format 1.
//!
//! ExtensionSubst (LookupType 7) is unwrapped transparently for both.
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
use crate::tables::gdef::{coverage_lookup, lookup_table_slice};
use crate::Error;

const LOOKUP_SINGLE_SUBST: u16 = 1;
const LOOKUP_LIGATURE_SUBST: u16 = 4;
const LOOKUP_EXTENSION_SUBST: u16 = 7;

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
pub struct GsubTable<'a> {
    bytes: &'a [u8],
    script_list_off: u32,
    feature_list_off: u32,
    lookup_list_off: u32,
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
        let script_list_off = read_u16(bytes, 4)? as u32;
        let feature_list_off = read_u16(bytes, 6)? as u32;
        let lookup_list_off = read_u16(bytes, 8)? as u32;
        // Each offset must either be 0 (table absent) or fit inside `bytes`.
        for off in [script_list_off, feature_list_off, lookup_list_off] {
            if off != 0 && off as usize >= bytes.len() {
                return Err(Error::BadOffset);
            }
        }
        Ok(Self {
            bytes,
            script_list_off,
            feature_list_off,
            lookup_list_off,
        })
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

        // Helper to resolve one feature index → GsubFeature.
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
}
