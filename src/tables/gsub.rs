//! `GSUB` — Glyph Substitution Table.
//!
//! Round 1 only walks LookupType 4 (Ligature Substitution) sub-tables.
//! All other lookup types (single, multiple, alternate, contextual,
//! chaining, extension, reverse) are silently ignored.

use crate::parser::{read_u16, read_u32};
use crate::tables::gdef::{coverage_lookup, lookup_table_slice};
use crate::Error;

const LOOKUP_LIGATURE_SUBST: u16 = 4;
const LOOKUP_EXTENSION_SUBST: u16 = 7;

#[derive(Debug, Clone)]
pub struct GsubTable<'a> {
    bytes: &'a [u8],
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
        let lookup_list_off = read_u16(bytes, 8)? as u32;
        if lookup_list_off as usize >= bytes.len() {
            return Err(Error::BadOffset);
        }
        Ok(Self {
            bytes,
            lookup_list_off,
        })
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
}
