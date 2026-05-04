//! `cmap` — character → glyph map.
//!
//! We pick a single subtable at parse time (preferred order: 32-bit
//! formats first, BMP formats second, legacy single-byte last) and run
//! all `lookup` calls through it. Round-1 supports formats 0, 4, 6, 12
//! for the base codepoint→glyph map, plus format 14 (Unicode Variation
//! Sequences) for codepoint+variation-selector → variant-glyph lookups.
//!
//! Format 14 is layered on top of the picked base subtable: it never
//! competes with formats 0/4/6/12 for the "best base map" rank and is
//! always stored alongside it when present. Real-world fonts that ship
//! format 14 include Noto Color Emoji (variant emoji presentation /
//! skin-tone modifiers), Apple Color Emoji, and most CJK fonts that
//! expose Unicode Ideographic Variation Sequences (registered IVD
//! collections).

use crate::parser::{read_u16, read_u24, read_u32};
use crate::Error;

/// Decoded cmap subtable, preselected from the candidate list.
#[derive(Debug, Clone)]
pub struct CmapTable<'a> {
    subtable: Subtable<'a>,
    /// Optional Unicode Variation Sequences subtable (format 14).
    /// Used by `lookup_variation`; never replaces the base `lookup`.
    variation: Option<&'a [u8]>,
}

#[derive(Debug, Clone)]
enum Subtable<'a> {
    Format0(&'a [u8]),
    Format4(&'a [u8]),
    Format6(&'a [u8]),
    Format12(&'a [u8]),
}

impl<'a> CmapTable<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        // Header: u16 version, u16 numTables, then numTables * 8 byte
        // EncodingRecord { platformID, encodingID, offset(u32) }.
        if bytes.len() < 4 {
            return Err(Error::UnexpectedEof);
        }
        let _version = read_u16(bytes, 0)?;
        let num_tables = read_u16(bytes, 2)?;
        let header_end = 4 + (num_tables as usize) * 8;
        if bytes.len() < header_end {
            return Err(Error::UnexpectedEof);
        }

        // We want the *richest* base subtable: prefer Unicode 32-bit
        // (format 12), then any BMP format-4, then format-6, then
        // format-0. Walk all encoding records and collect candidates.
        //
        // IMPORTANT: filter on subtable format BEFORE running per-format
        // length validation. Some real-world fonts (e.g. Noto Color
        // Emoji, many CJK fonts) ship a format-14 (Unicode Variation
        // Selectors) subtable alongside a supported format-12 / format-4
        // base map. Format 14 has a different header layout (no u16
        // length at offset+2), so calling the length helper on it would
        // bail with `UnsupportedCmapFormat(14)` and reject the entire
        // font even though the format-12 sibling is perfectly usable.
        let mut best: Option<Subtable<'_>> = None;
        let mut best_rank = i32::MIN;
        let mut variation: Option<&'a [u8]> = None;

        for i in 0..num_tables as usize {
            let off = 4 + i * 8;
            let platform_id = read_u16(bytes, off)?;
            let encoding_id = read_u16(bytes, off + 2)?;
            let sub_off = read_u32(bytes, off + 4)? as usize;
            if sub_off + 2 > bytes.len() {
                return Err(Error::BadOffset);
            }
            let format = read_u16(bytes, sub_off)?;

            // Format 14 (Unicode Variation Sequences) is a sidecar — it
            // lives alongside one of the supported base subtables and
            // contributes to `lookup_variation`, never to the base
            // `lookup`. Pull it out separately and skip the base-map
            // ranking entirely.
            if format == 14 {
                if sub_off + 6 > bytes.len() {
                    return Err(Error::BadOffset);
                }
                let length = read_u32(bytes, sub_off + 2)? as usize;
                let sub = bytes
                    .get(sub_off..sub_off + length)
                    .ok_or(Error::BadOffset)?;
                // Per spec only one format-14 subtable is allowed per
                // cmap; if a malformed font ships several, keep the
                // first one.
                if variation.is_none() {
                    variation = Some(sub);
                }
                continue;
            }

            // Skip formats we don't decode in round 1 BEFORE touching
            // their length field — different formats place `length` at
            // different offsets / widths, and unrecognised formats may
            // not have one in the same place at all.
            if !is_supported_format(format) {
                continue;
            }

            let length = subtable_length(bytes, sub_off, format)?;
            let sub = bytes
                .get(sub_off..sub_off + length)
                .ok_or(Error::BadOffset)?;

            let candidate = match format {
                0 => Subtable::Format0(sub),
                4 => Subtable::Format4(sub),
                6 => Subtable::Format6(sub),
                12 => Subtable::Format12(sub),
                _ => unreachable!("filtered by is_supported_format above"),
            };
            let rank = subtable_rank(format, platform_id, encoding_id);
            if rank > best_rank {
                best_rank = rank;
                best = Some(candidate);
            }
        }

        Ok(Self {
            subtable: best.ok_or(Error::UnsupportedCmapFormat(0xFFFF))?,
            variation,
        })
    }

    /// Map a Unicode codepoint to a glyph id, or `None` if absent.
    pub fn lookup(&self, codepoint: u32) -> Option<u16> {
        match &self.subtable {
            Subtable::Format0(b) => lookup_format0(b, codepoint),
            Subtable::Format4(b) => lookup_format4(b, codepoint),
            Subtable::Format6(b) => lookup_format6(b, codepoint),
            Subtable::Format12(b) => lookup_format12(b, codepoint),
        }
    }

    /// Look up the variant glyph for a `(codepoint, variation_selector)`
    /// pair using the cmap format-14 (Unicode Variation Sequences)
    /// subtable. Returns:
    ///
    /// - `Some(glyph_id)` when the non-default UVS table maps the pair
    ///   to a custom variant glyph.
    /// - `Some(base_glyph)` when the pair is in the *default* UVS table
    ///   — semantically "render the base glyph; the variation selector
    ///   chooses the default presentation". This matches HarfBuzz's
    ///   `hb_font_get_variation_glyph` contract.
    /// - `None` if the font has no format-14 subtable, the variation
    ///   selector record isn't listed, or the codepoint isn't in either
    ///   of the record's two UVS tables.
    ///
    /// Note that returning `Some(base_glyph)` for default UVS hits is
    /// *not* the same as falling through to [`Self::lookup`]: callers
    /// that want pure base-map behaviour should call `lookup` directly.
    pub fn lookup_variation(&self, codepoint: u32, variation_selector: u32) -> Option<u16> {
        let bytes = self.variation?;
        // Header: u16 format (=14), u32 length, u32 numVarSelectorRecords.
        let num_records = read_u32(bytes, 6).ok()? as usize;
        let records_off = 10usize;
        // VariationSelectorRecord layout (11 bytes each):
        //   u24 varSelector
        //   Offset32 defaultUVSOffset    (0 = no default UVS table)
        //   Offset32 nonDefaultUVSOffset (0 = no non-default UVS table)
        //
        // Records are sorted by varSelector ascending — binary search.
        let rec_size = 11;
        if records_off + num_records * rec_size > bytes.len() {
            return None;
        }
        let mut lo = 0usize;
        let mut hi = num_records;
        let rec_off = loop {
            if lo >= hi {
                return None;
            }
            let mid = (lo + hi) / 2;
            let off = records_off + mid * rec_size;
            let vs = read_u24(bytes, off).ok()?;
            match vs.cmp(&variation_selector) {
                core::cmp::Ordering::Less => lo = mid + 1,
                core::cmp::Ordering::Greater => hi = mid,
                core::cmp::Ordering::Equal => break off,
            }
        };
        let default_off = read_u32(bytes, rec_off + 3).ok()? as usize;
        let non_default_off = read_u32(bytes, rec_off + 7).ok()? as usize;

        // 1. Non-default UVS lookup wins — it carries an explicit glyph.
        if non_default_off != 0 {
            if let Some(g) = lookup_non_default_uvs(bytes, non_default_off, codepoint) {
                return Some(g);
            }
        }
        // 2. Default UVS hit — semantically "use the base glyph". Return
        //    Some(base) so callers can rely on a single result type.
        if default_off != 0 && range_contains(bytes, default_off, codepoint) {
            return self.lookup(codepoint);
        }
        None
    }
}

/// Walk a NonDefaultUVS table looking for `codepoint`. Returns the
/// per-pair glyph ID when found.
///
/// Layout (from the Microsoft spec):
///   u32 numUVSMappings
///   UVSMapping[numUVSMappings]:
///     u24 unicodeValue
///     u16 glyphID
///
/// Mappings are sorted by unicodeValue — binary search.
fn lookup_non_default_uvs(bytes: &[u8], table_off: usize, codepoint: u32) -> Option<u16> {
    if table_off + 4 > bytes.len() {
        return None;
    }
    let n = read_u32(bytes, table_off).ok()? as usize;
    let entries_off = table_off + 4;
    let entry_size = 5;
    if entries_off + n * entry_size > bytes.len() {
        return None;
    }
    let mut lo = 0usize;
    let mut hi = n;
    while lo < hi {
        let mid = (lo + hi) / 2;
        let off = entries_off + mid * entry_size;
        let cp = read_u24(bytes, off).ok()?;
        match cp.cmp(&codepoint) {
            core::cmp::Ordering::Less => lo = mid + 1,
            core::cmp::Ordering::Greater => hi = mid,
            core::cmp::Ordering::Equal => return read_u16(bytes, off + 3).ok(),
        }
    }
    None
}

/// Test whether `codepoint` lives in any UnicodeRange of a DefaultUVS
/// table.
///
/// Layout:
///   u32 numUnicodeValueRanges
///   UnicodeRange[numUnicodeValueRanges]:
///     u24 startUnicodeValue
///     u8  additionalCount  (range covers start..=start+additionalCount)
///
/// Ranges are sorted by startUnicodeValue; binary-search to the first
/// range whose start ≤ codepoint, then check the inclusive end bound.
fn range_contains(bytes: &[u8], table_off: usize, codepoint: u32) -> bool {
    if table_off + 4 > bytes.len() {
        return false;
    }
    let Ok(n_u32) = read_u32(bytes, table_off) else {
        return false;
    };
    let n = n_u32 as usize;
    let entries_off = table_off + 4;
    let entry_size = 4;
    if entries_off + n * entry_size > bytes.len() {
        return false;
    }
    // Find the largest index whose start ≤ codepoint, then test the
    // upper bound. Standard "rightmost-≤ binary search".
    let mut lo = 0usize;
    let mut hi = n;
    while lo < hi {
        let mid = (lo + hi) / 2;
        let off = entries_off + mid * entry_size;
        let Ok(start) = read_u24(bytes, off) else {
            return false;
        };
        if start <= codepoint {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == 0 {
        return false;
    }
    let cand = lo - 1;
    let off = entries_off + cand * entry_size;
    let Ok(start) = read_u24(bytes, off) else {
        return false;
    };
    let Ok(extra) = bytes
        .get(off + 3)
        .copied()
        .ok_or(crate::Error::UnexpectedEof)
    else {
        return false;
    };
    let end = start + extra as u32;
    codepoint >= start && codepoint <= end
}

fn is_supported_format(format: u16) -> bool {
    matches!(format, 0 | 4 | 6 | 12)
}

fn subtable_length(bytes: &[u8], off: usize, format: u16) -> Result<usize, Error> {
    // Formats 0/4/6 have a u16 length at offset+2. Formats 8/10/12/13
    // have a u32 length at offset+4. Format 14 has its own u32 length
    // at offset+2 (different layout entirely) but the picker filters
    // it out before we get here, so we don't need to handle it.
    Ok(match format {
        0 | 2 | 4 | 6 => read_u16(bytes, off + 2)? as usize,
        8 | 10 | 12 | 13 => read_u32(bytes, off + 4)? as usize,
        _ => return Err(Error::UnsupportedCmapFormat(format)),
    })
}

fn subtable_rank(format: u16, platform: u16, encoding: u16) -> i32 {
    // Ranking heuristic — higher = preferred.
    //  - format 12 wins over format 4 (full Unicode > BMP).
    //  - Unicode platform (0) wins over Windows (3) wins over Mac (1).
    let format_score = match format {
        12 => 400,
        4 => 300,
        6 => 200,
        0 => 100,
        _ => 0,
    };
    let platform_score = match (platform, encoding) {
        (0, _) => 30,
        (3, 10) => 25, // Windows Unicode UCS-4
        (3, 1) => 20,  // Windows Unicode BMP
        _ => 5,
    };
    format_score + platform_score
}

// --- Format 0 --------------------------------------------------------------

fn lookup_format0(bytes: &[u8], codepoint: u32) -> Option<u16> {
    if codepoint > 0xFF {
        return None;
    }
    // Header: u16 format, u16 length, u16 language, then 256 u8 glyphIdArray.
    let glyph_array_off = 6;
    if bytes.len() < glyph_array_off + 256 {
        return None;
    }
    let g = bytes[glyph_array_off + codepoint as usize];
    if g == 0 {
        None
    } else {
        Some(g as u16)
    }
}

// --- Format 4 --------------------------------------------------------------

fn lookup_format4(bytes: &[u8], codepoint: u32) -> Option<u16> {
    // Format 4: BMP only.
    if codepoint > 0xFFFF {
        return None;
    }
    let cp = codepoint as u16;
    // Header (offsets):
    //   0  / format (2)
    //   2  / length (2)
    //   4  / language (2)
    //   6  / segCountX2 (2)
    //   8  / searchRange / entrySelector / rangeShift (each 2)
    //  14  / endCode[segCount] u16
    //   ...
    let seg_count_x2 = read_u16(bytes, 6).ok()? as usize;
    let seg_count = seg_count_x2 / 2;
    if seg_count == 0 {
        return None;
    }
    let end_code_off = 14usize;
    let reserved_pad = end_code_off + seg_count_x2; // u16 = 0
    let start_code_off = reserved_pad + 2;
    let id_delta_off = start_code_off + seg_count_x2;
    let id_range_offset_off = id_delta_off + seg_count_x2;
    let glyph_id_array_off = id_range_offset_off + seg_count_x2;
    if bytes.len() < glyph_id_array_off {
        return None;
    }
    // Linear-scan to find the segment whose endCode >= cp.
    // Could binary-search; small fonts have ~50-200 segments so linear is fine.
    let mut seg = None;
    for i in 0..seg_count {
        let end = read_u16(bytes, end_code_off + i * 2).ok()?;
        if end >= cp {
            seg = Some(i);
            break;
        }
    }
    let seg = seg?;
    let start = read_u16(bytes, start_code_off + seg * 2).ok()?;
    if start > cp {
        return None;
    }
    let id_delta = read_u16(bytes, id_delta_off + seg * 2).ok()? as i32 as i16;
    let id_range_offset = read_u16(bytes, id_range_offset_off + seg * 2).ok()?;
    if id_range_offset == 0 {
        // Direct: glyph = (cp + id_delta) mod 65536.
        let g = (cp as i32 + id_delta as i32) & 0xFFFF;
        if g == 0 {
            return None;
        }
        return Some(g as u16);
    }
    // Indirect: spec formula
    //   *(idRangeOffset[i]/2 + (cp - startCode[i]) + &idRangeOffset[i])
    // Equivalent absolute byte offset:
    //   id_range_offset_off + seg*2 + id_range_offset + 2*(cp - start)
    let target = id_range_offset_off
        + seg * 2
        + id_range_offset as usize
        + 2 * (cp as usize - start as usize);
    let raw = read_u16(bytes, target).ok()?;
    if raw == 0 {
        return None;
    }
    let g = (raw as i32 + id_delta as i32) & 0xFFFF;
    Some(g as u16)
}

// --- Format 6 --------------------------------------------------------------

fn lookup_format6(bytes: &[u8], codepoint: u32) -> Option<u16> {
    if codepoint > 0xFFFF {
        return None;
    }
    let cp = codepoint as u16;
    // Header:
    //   0 / format (2)
    //   2 / length (2)
    //   4 / language (2)
    //   6 / firstCode (2)
    //   8 / entryCount (2)
    //  10 / glyphIdArray[entryCount] u16
    let first_code = read_u16(bytes, 6).ok()?;
    let entry_count = read_u16(bytes, 8).ok()?;
    if cp < first_code {
        return None;
    }
    let idx = cp - first_code;
    if idx >= entry_count {
        return None;
    }
    let g = read_u16(bytes, 10 + idx as usize * 2).ok()?;
    if g == 0 {
        None
    } else {
        Some(g)
    }
}

// --- Format 12 -------------------------------------------------------------

fn lookup_format12(bytes: &[u8], codepoint: u32) -> Option<u16> {
    // Header:
    //   0  / format (2)
    //   2  / reserved (2)
    //   4  / length (4)
    //   8  / language (4)
    //  12  / numGroups (4)
    //  16  / SequentialMapGroup[numGroups]
    //         u32 startCharCode, u32 endCharCode, u32 startGlyphID
    let num_groups = read_u32(bytes, 12).ok()? as usize;
    if 16 + num_groups * 12 > bytes.len() {
        return None;
    }
    // Binary search by start ≤ cp ≤ end.
    let mut lo = 0usize;
    let mut hi = num_groups;
    while lo < hi {
        let mid = (lo + hi) / 2;
        let off = 16 + mid * 12;
        let start = read_u32(bytes, off).ok()?;
        let end = read_u32(bytes, off + 4).ok()?;
        if codepoint < start {
            hi = mid;
        } else if codepoint > end {
            lo = mid + 1;
        } else {
            let start_glyph = read_u32(bytes, off + 8).ok()?;
            let g = start_glyph.checked_add(codepoint - start)?;
            if g > u16::MAX as u32 {
                return None;
            }
            return Some(g as u16);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_cmap_with_subtable(format: u16, sub: &[u8]) -> Vec<u8> {
        // 1 encoding record, Windows (3,1) for format 4, Unicode (0,3)
        // for format 12 — picked just so the rank ordering picks our sole
        // subtable.
        let mut out = vec![0u8; 4 + 8];
        out[0..2].copy_from_slice(&0u16.to_be_bytes()); // version
        out[2..4].copy_from_slice(&1u16.to_be_bytes()); // numTables
        out[4..6].copy_from_slice(&3u16.to_be_bytes()); // platform
        let enc: u16 = if format == 12 { 10 } else { 1 };
        out[6..8].copy_from_slice(&enc.to_be_bytes());
        out[8..12].copy_from_slice(&12u32.to_be_bytes()); // offset to subtable
        out.extend_from_slice(sub);
        // Patch length field of the subtable header.
        let _ = format;
        out
    }

    #[test]
    fn format0_round_trip() {
        // Map codepoint 65 ('A') to glyph 7.
        let mut sub = vec![0u8; 6 + 256];
        sub[0..2].copy_from_slice(&0u16.to_be_bytes()); // format
        sub[2..4].copy_from_slice(&((6 + 256) as u16).to_be_bytes()); // length
        sub[6 + 65] = 7;
        let cmap_bytes = build_cmap_with_subtable(0, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        assert_eq!(cmap.lookup(65), Some(7));
        assert_eq!(cmap.lookup(64), None);
        assert_eq!(cmap.lookup(0x10000), None);
    }

    #[test]
    fn format6_round_trip() {
        let mut sub = vec![0u8; 10 + 4];
        sub[0..2].copy_from_slice(&6u16.to_be_bytes());
        sub[2..4].copy_from_slice(&((10 + 4) as u16).to_be_bytes());
        sub[6..8].copy_from_slice(&100u16.to_be_bytes()); // firstCode
        sub[8..10].copy_from_slice(&2u16.to_be_bytes()); // entryCount
        sub[10..12].copy_from_slice(&77u16.to_be_bytes()); // glyph for 100
        sub[12..14].copy_from_slice(&0u16.to_be_bytes()); // glyph for 101 = missing
        let cmap_bytes = build_cmap_with_subtable(6, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        assert_eq!(cmap.lookup(100), Some(77));
        assert_eq!(cmap.lookup(101), None);
        assert_eq!(cmap.lookup(99), None);
    }

    #[test]
    fn format12_round_trip() {
        // Two groups: 0x4E00..0x4E02 → glyph 1000..1002; 0x1F600 → glyph 5000.
        let mut sub = vec![0u8; 16 + 24];
        sub[0..2].copy_from_slice(&12u16.to_be_bytes());
        sub[4..8].copy_from_slice(&((16 + 24) as u32).to_be_bytes());
        sub[12..16].copy_from_slice(&2u32.to_be_bytes()); // numGroups
                                                          // Group 0: start=0x4E00 end=0x4E02 startGlyph=1000
        sub[16..20].copy_from_slice(&0x4E00u32.to_be_bytes());
        sub[20..24].copy_from_slice(&0x4E02u32.to_be_bytes());
        sub[24..28].copy_from_slice(&1000u32.to_be_bytes());
        // Group 1: start=0x1F600 end=0x1F600 startGlyph=5000
        sub[28..32].copy_from_slice(&0x1F600u32.to_be_bytes());
        sub[32..36].copy_from_slice(&0x1F600u32.to_be_bytes());
        sub[36..40].copy_from_slice(&5000u32.to_be_bytes());

        let cmap_bytes = build_cmap_with_subtable(12, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        assert_eq!(cmap.lookup(0x4E00), Some(1000));
        assert_eq!(cmap.lookup(0x4E01), Some(1001));
        assert_eq!(cmap.lookup(0x4E02), Some(1002));
        assert_eq!(cmap.lookup(0x4E03), None);
        assert_eq!(cmap.lookup(0x1F600), Some(5000));
    }

    /// Regression: a cmap that ships a format-14 (Unicode Variation
    /// Selectors) subtable alongside a supported format must NOT fail
    /// the parse. The format-14 entry is silently skipped and the
    /// format-12 sibling is selected as the active subtable. This is
    /// the layout used by Noto Color Emoji, many CJK fonts, and any
    /// font that wants to expose emoji-presentation variation
    /// sequences (codepoint + U+FE0F / U+FE0E).
    #[test]
    fn format14_subtable_is_skipped_not_rejected() {
        // Build the format-12 subtable: one group, U+1F600 → glyph 5.
        let mut sub12 = vec![0u8; 16 + 12];
        sub12[0..2].copy_from_slice(&12u16.to_be_bytes()); // format
        sub12[4..8].copy_from_slice(&((16 + 12) as u32).to_be_bytes()); // length
        sub12[12..16].copy_from_slice(&1u32.to_be_bytes()); // numGroups
        sub12[16..20].copy_from_slice(&0x1F600u32.to_be_bytes()); // start
        sub12[20..24].copy_from_slice(&0x1F600u32.to_be_bytes()); // end
        sub12[24..28].copy_from_slice(&5u32.to_be_bytes()); // startGlyph

        // Build a minimal format-14 subtable: 0 variation selector
        // records (length = 10 bytes header). Per spec:
        //   u16 format (= 14), u32 length, u32 numVarSelectorRecords.
        // Even with zero records the layout differs from formats 0/4/6
        // (which have a u16 length at offset+2). Before this fix, the
        // length probe would mis-read offset+2 as u16 and either crash
        // or — worse — bail with UnsupportedCmapFormat(14) BEFORE the
        // format-12 sibling could be picked.
        let mut sub14 = vec![0u8; 10];
        sub14[0..2].copy_from_slice(&14u16.to_be_bytes()); // format
        sub14[2..6].copy_from_slice(&10u32.to_be_bytes()); // length
        sub14[6..10].copy_from_slice(&0u32.to_be_bytes()); // numVarSelectorRecords

        // Hand-roll the cmap header: 2 encoding records.
        //   record 0: (3, 10) → format-12 subtable
        //   record 1: (0, 5)  → format-14 subtable (Unicode Variation Selectors)
        let header_len = 4 + 2 * 8;
        let sub12_off = header_len;
        let sub14_off = sub12_off + sub12.len();
        let mut out = vec![0u8; header_len];
        out[0..2].copy_from_slice(&0u16.to_be_bytes()); // version
        out[2..4].copy_from_slice(&2u16.to_be_bytes()); // numTables
                                                        // record 0
        out[4..6].copy_from_slice(&3u16.to_be_bytes());
        out[6..8].copy_from_slice(&10u16.to_be_bytes());
        out[8..12].copy_from_slice(&(sub12_off as u32).to_be_bytes());
        // record 1
        out[12..14].copy_from_slice(&0u16.to_be_bytes());
        out[14..16].copy_from_slice(&5u16.to_be_bytes());
        out[16..20].copy_from_slice(&(sub14_off as u32).to_be_bytes());
        out.extend_from_slice(&sub12);
        out.extend_from_slice(&sub14);

        let cmap = CmapTable::parse(&out).expect("format-14 sibling must not fail parse");
        assert_eq!(cmap.lookup(0x1F600), Some(5));
        assert_eq!(cmap.lookup(0x1F601), None);
    }

    // Build a cmap with one format-12 base subtable and one
    // format-14 (UVS) subtable carrying:
    //   - varSelector = 0xFE0F (emoji presentation)
    //       defaultUVS    = [0x1F600..=0x1F600] (default-render this emoji)
    //       nonDefaultUVS = { 0x2728: 9999 }    (sparkles → custom glyph)
    //
    // Plus base format-12 groups:
    //   0x2728..=0x2728  → glyph 7
    //   0x1F600..=0x1F600 → glyph 5
    //
    // Lookup expectations:
    //   lookup_variation(0x1F600, 0xFE0F) -> Some(5)    (default UVS hit, base glyph)
    //   lookup_variation(0x2728,  0xFE0F) -> Some(9999) (non-default override)
    //   lookup_variation(0x1F600, 0xFE0E) -> None       (no record for VS-15)
    //   lookup_variation(0x1F601, 0xFE0F) -> None       (covered VS but cp absent)
    fn build_cmap_with_format12_and_format14() -> Vec<u8> {
        // -- format-12 subtable: two single-cp groups (must be sorted by
        //    startCharCode ascending — the format-12 lookup binary-searches
        //    them).
        let num_groups: u32 = 2;
        let sub12_len: usize = 16 + num_groups as usize * 12;
        let mut sub12 = vec![0u8; sub12_len];
        sub12[0..2].copy_from_slice(&12u16.to_be_bytes());
        sub12[4..8].copy_from_slice(&(sub12_len as u32).to_be_bytes());
        sub12[12..16].copy_from_slice(&num_groups.to_be_bytes());
        // group 0: U+2728 → 7   (sparkles, BMP)
        sub12[16..20].copy_from_slice(&0x2728u32.to_be_bytes());
        sub12[20..24].copy_from_slice(&0x2728u32.to_be_bytes());
        sub12[24..28].copy_from_slice(&7u32.to_be_bytes());
        // group 1: U+1F600 → 5  (grinning face, supplementary plane)
        sub12[28..32].copy_from_slice(&0x1F600u32.to_be_bytes());
        sub12[32..36].copy_from_slice(&0x1F600u32.to_be_bytes());
        sub12[36..40].copy_from_slice(&5u32.to_be_bytes());

        // -- format-14 subtable -------------------------------------------
        // 1 record (varSelector = 0xFE0F).
        // DefaultUVS: 1 range starting at 0x1F600 with additionalCount = 0.
        // NonDefaultUVS: 1 mapping (0x2728 → 9999).
        let header_len = 10usize; // u16 fmt + u32 length + u32 numRecords
        let record_len = 11usize;
        let default_table_len = 4 + 4; // u32 count + 1 range (3 + 1)
        let non_default_table_len = 4 + 5; // u32 count + 1 mapping (3 + 2)
        let sub14_len = header_len + record_len + default_table_len + non_default_table_len;
        let mut sub14 = vec![0u8; sub14_len];
        sub14[0..2].copy_from_slice(&14u16.to_be_bytes());
        sub14[2..6].copy_from_slice(&(sub14_len as u32).to_be_bytes());
        sub14[6..10].copy_from_slice(&1u32.to_be_bytes()); // numVarSelectorRecords

        // Layout offsets:
        //   record at 10..21
        //   defaultUVS table at 21..29
        //   nonDefaultUVS table at 29..38
        let default_off = (header_len + record_len) as u32; // 21
        let non_default_off = default_off + default_table_len as u32; // 29

        // record 0: varSelector = 0xFE0F (encoded as u24)
        let vs_bytes = 0xFE0Fu32.to_be_bytes();
        sub14[10..13].copy_from_slice(&vs_bytes[1..4]);
        sub14[13..17].copy_from_slice(&default_off.to_be_bytes());
        sub14[17..21].copy_from_slice(&non_default_off.to_be_bytes());

        // DefaultUVS: 1 range, start=0x1F600, additional=0
        let off = default_off as usize;
        sub14[off..off + 4].copy_from_slice(&1u32.to_be_bytes());
        let r = off + 4;
        let start_bytes = 0x1F600u32.to_be_bytes();
        sub14[r..r + 3].copy_from_slice(&start_bytes[1..4]);
        sub14[r + 3] = 0; // additionalCount

        // NonDefaultUVS: 1 mapping: 0x2728 → 9999
        let off = non_default_off as usize;
        sub14[off..off + 4].copy_from_slice(&1u32.to_be_bytes());
        let m = off + 4;
        let cp_bytes = 0x2728u32.to_be_bytes();
        sub14[m..m + 3].copy_from_slice(&cp_bytes[1..4]);
        sub14[m + 3..m + 5].copy_from_slice(&9999u16.to_be_bytes());

        // -- cmap header: 2 encoding records ------------------------------
        let header_len = 4 + 2 * 8;
        let sub12_off = header_len;
        let sub14_off = sub12_off + sub12.len();
        let mut out = vec![0u8; header_len];
        out[0..2].copy_from_slice(&0u16.to_be_bytes());
        out[2..4].copy_from_slice(&2u16.to_be_bytes());
        // record 0: (3, 10) → format-12
        out[4..6].copy_from_slice(&3u16.to_be_bytes());
        out[6..8].copy_from_slice(&10u16.to_be_bytes());
        out[8..12].copy_from_slice(&(sub12_off as u32).to_be_bytes());
        // record 1: (0, 5) → format-14
        out[12..14].copy_from_slice(&0u16.to_be_bytes());
        out[14..16].copy_from_slice(&5u16.to_be_bytes());
        out[16..20].copy_from_slice(&(sub14_off as u32).to_be_bytes());
        out.extend_from_slice(&sub12);
        out.extend_from_slice(&sub14);
        out
    }

    #[test]
    fn variation_lookup_default_returns_base_glyph() {
        let cmap_bytes = build_cmap_with_format12_and_format14();
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        // Base lookup unchanged.
        assert_eq!(cmap.lookup(0x1F600), Some(5));
        assert_eq!(cmap.lookup(0x2728), Some(7));
        // Default UVS hit on grinning-face emoji + VS-16 → base glyph.
        assert_eq!(cmap.lookup_variation(0x1F600, 0xFE0F), Some(5));
    }

    #[test]
    fn variation_lookup_non_default_overrides_base() {
        let cmap_bytes = build_cmap_with_format12_and_format14();
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        // U+2728 + VS-16 → custom glyph 9999, NOT the base glyph 7.
        assert_eq!(cmap.lookup_variation(0x2728, 0xFE0F), Some(9999));
    }

    #[test]
    fn variation_lookup_misses_return_none() {
        let cmap_bytes = build_cmap_with_format12_and_format14();
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        // Variation selector not enumerated.
        assert_eq!(cmap.lookup_variation(0x1F600, 0xFE0E), None);
        // Variation selector enumerated but codepoint not in either UVS.
        assert_eq!(cmap.lookup_variation(0x1F601, 0xFE0F), None);
    }

    #[test]
    fn variation_lookup_returns_none_when_no_format14() {
        // The cmap from the original format12_round_trip test has no
        // format-14 subtable.
        let mut sub = vec![0u8; 16 + 12];
        sub[0..2].copy_from_slice(&12u16.to_be_bytes());
        sub[4..8].copy_from_slice(&((16 + 12) as u32).to_be_bytes());
        sub[12..16].copy_from_slice(&1u32.to_be_bytes());
        sub[16..20].copy_from_slice(&0x1F600u32.to_be_bytes());
        sub[20..24].copy_from_slice(&0x1F600u32.to_be_bytes());
        sub[24..28].copy_from_slice(&5u32.to_be_bytes());
        let cmap_bytes = build_cmap_with_subtable(12, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        assert_eq!(cmap.lookup(0x1F600), Some(5));
        assert_eq!(cmap.lookup_variation(0x1F600, 0xFE0F), None);
    }

    /// A cmap with ONLY unsupported subtables (here: just format 14)
    /// still has to fail — the picker has nothing to map base codepoints
    /// through. Make sure the failure mode is the existing
    /// `UnsupportedCmapFormat(0xFFFF)` sentinel and not a length-validation
    /// crash on the format-14 header.
    #[test]
    fn cmap_with_only_format14_fails_cleanly() {
        let mut sub14 = vec![0u8; 10];
        sub14[0..2].copy_from_slice(&14u16.to_be_bytes());
        sub14[2..6].copy_from_slice(&10u32.to_be_bytes());
        sub14[6..10].copy_from_slice(&0u32.to_be_bytes());

        let header_len = 4 + 8;
        let mut out = vec![0u8; header_len];
        out[0..2].copy_from_slice(&0u16.to_be_bytes());
        out[2..4].copy_from_slice(&1u16.to_be_bytes());
        out[4..6].copy_from_slice(&0u16.to_be_bytes());
        out[6..8].copy_from_slice(&5u16.to_be_bytes());
        out[8..12].copy_from_slice(&(header_len as u32).to_be_bytes());
        out.extend_from_slice(&sub14);

        match CmapTable::parse(&out) {
            Err(Error::UnsupportedCmapFormat(0xFFFF)) => {}
            other => panic!("expected UnsupportedCmapFormat(0xFFFF), got {other:?}"),
        }
    }

    #[test]
    fn format4_round_trip() {
        // One real segment: 'A'..'C' (65..67) → glyphs 100..102 (id_delta = +35).
        // Plus the mandatory terminator segment 0xFFFF..0xFFFF id_delta=1.
        let seg_count: u16 = 2;
        let seg_count_x2: u16 = seg_count * 2;
        let header = 14;
        let arrays_len = seg_count_x2 as usize * 4 + 2 /*reserved pad*/;
        let length = header + arrays_len;
        let mut sub = vec![0u8; length];
        sub[0..2].copy_from_slice(&4u16.to_be_bytes()); // format
        sub[2..4].copy_from_slice(&(length as u16).to_be_bytes());
        sub[6..8].copy_from_slice(&seg_count_x2.to_be_bytes());
        // searchRange/entrySelector/rangeShift left zero — readers ignore.

        // endCode[segCount]
        sub[14..16].copy_from_slice(&67u16.to_be_bytes());
        sub[16..18].copy_from_slice(&0xFFFFu16.to_be_bytes());
        // reservedPad
        sub[18..20].copy_from_slice(&0u16.to_be_bytes());
        // startCode[segCount]
        sub[20..22].copy_from_slice(&65u16.to_be_bytes());
        sub[22..24].copy_from_slice(&0xFFFFu16.to_be_bytes());
        // idDelta[segCount]
        sub[24..26].copy_from_slice(&35u16.to_be_bytes());
        sub[26..28].copy_from_slice(&1u16.to_be_bytes());
        // idRangeOffset[segCount] all zero (direct mapping).

        let cmap_bytes = build_cmap_with_subtable(4, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        assert_eq!(cmap.lookup('A' as u32), Some(100));
        assert_eq!(cmap.lookup('B' as u32), Some(101));
        assert_eq!(cmap.lookup('C' as u32), Some(102));
        // 'D' (68) > end 67 < terminator 0xFFFF: still finds the
        // terminator segment which yields glyph 0 (skipped → None).
        assert_eq!(cmap.lookup('D' as u32), None);
    }
}
