//! `cmap` — character → glyph map.
//!
//! We pick a single subtable at parse time (preferred order: 32-bit
//! formats first, BMP formats second, legacy single-byte last) and run
//! all `lookup` calls through it. Supported base formats: 0, 2, 4, 6,
//! 12, 13. Plus format 14 (Unicode Variation Sequences) as a sidecar
//! that drives `lookup_variation` for codepoint+variation-selector pairs.
//!
//! Format 2 (high-byte mapping through table) is the legacy
//! mixed-8-/16-bit encoding the cmap chapter describes for older
//! Japanese / Chinese / Korean fonts. It is "not commonly used today"
//! per the spec but still appears in pre-Unicode CJK system fonts.
//! Lookup input is the raw codeunit (single byte for 1-byte chars, the
//! 2-byte value `(high << 8) | low` for 2-byte chars). We do NOT do
//! Shift-JIS / GB2312 / KSC-5601 → Unicode transcoding; the caller
//! does that mapping itself if it wants to drive a format-2 font from
//! a Unicode codepoint.
//!
//! Format 13 (many-to-one range mappings) shares its on-wire structure
//! with format 12 but differs in semantics: every codepoint inside a
//! group maps to the SAME `glyphID` rather than to a running sequence
//! anchored on `startGlyphID`. Per the OpenType cmap chapter, this is
//! the "last-resort" font layout — a single tofu glyph is reused
//! across thousands of codepoints to indicate "this codepoint exists
//! but the font cannot render it specifically". The `head` table's
//! flag bit 14 marks a font as last-resort; we do not gate format-13
//! decoding on that bit, since a non-last-resort font is also free to
//! ship format 13.
//!
//! Format 14 is layered on top of the picked base subtable: it never
//! competes with the base-map ranking and is always stored alongside
//! it when present. Real-world fonts that ship format 14 include Noto
//! Color Emoji (variant emoji presentation / skin-tone modifiers),
//! Apple Color Emoji, and most CJK fonts that expose Unicode
//! Ideographic Variation Sequences (registered IVD collections).

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
    Format2(&'a [u8]),
    Format4(&'a [u8]),
    Format6(&'a [u8]),
    Format12(&'a [u8]),
    Format13(&'a [u8]),
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
                2 => Subtable::Format2(sub),
                4 => Subtable::Format4(sub),
                6 => Subtable::Format6(sub),
                12 => Subtable::Format12(sub),
                13 => Subtable::Format13(sub),
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
    ///
    /// For format-2 subtables (legacy mixed-8-/16-bit CJK fonts) the
    /// `codepoint` argument is interpreted as a raw codeunit in the
    /// font's native encoding — not as a Unicode scalar value. See the
    /// `Format2` discussion in the module-level docs.
    pub fn lookup(&self, codepoint: u32) -> Option<u16> {
        match &self.subtable {
            Subtable::Format0(b) => lookup_format0(b, codepoint),
            Subtable::Format2(b) => lookup_format2(b, codepoint),
            Subtable::Format4(b) => lookup_format4(b, codepoint),
            Subtable::Format6(b) => lookup_format6(b, codepoint),
            Subtable::Format12(b) => lookup_format12(b, codepoint),
            Subtable::Format13(b) => lookup_format13(b, codepoint),
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
    ///   chooses the default presentation", per OpenType cmap
    ///   format-14 default-UVS semantics.
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
    matches!(format, 0 | 2 | 4 | 6 | 12 | 13)
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
    //  - Format 13 is the lowest-rank base-map because its semantic is
    //    "this codepoint is intentionally not rendered as itself" —
    //    a real font would never want it chosen ahead of a format-4
    //    BMP subtable that maps actual glyphs. The OpenType cmap
    //    chapter pairs format 13 with platform (0, 6) "Unicode full
    //    repertoire — for use with subtable format 13"; we give that
    //    pair the standard Unicode-platform bonus so that, when a font
    //    really does ship ONLY format 13 (a true last-resort font),
    //    the picker still selects it.
    let format_score = match format {
        12 => 400,
        4 => 300,
        6 => 200,
        0 => 100,
        // Format 2 is a legacy mixed-8/16-bit CJK encoding ("not
        // commonly used today" per the spec). Rank it BELOW the
        // legacy 256-glyph format 0 and just above the last-resort
        // format 13: a font that ships both a real Unicode subtable
        // and a format-2 sidecar always picks the Unicode subtable;
        // a font that ships ONLY format 2 (a true legacy CJK font)
        // can still be parsed.
        2 => 60,
        13 => 50,
        _ => 0,
    };
    let platform_score = match (platform, encoding) {
        (0, _) => 30,
        (3, 10) => 25, // Windows Unicode UCS-4
        (3, 1) => 20,  // Windows Unicode BMP
        // Legacy Macintosh ScriptManager codes ride here; we don't
        // single any encoding ID out because format-2 sees the same
        // platform/encoding shape across Shift-JIS / GB2312 / Big5 /
        // KSC-5601 (platform 1, encoding 1/2/3/5 respectively).
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

// --- Format 2 --------------------------------------------------------------

/// Format 2 — high-byte mapping through table.
///
/// Layout per the OpenType cmap chapter, "Format 2: High byte mapping
/// through table":
///
/// ```text
///   0  / 2 / format (= 2)
///   2  / 2 / length
///   4  / 2 / language
///   6  / 512 / subHeaderKeys[256]   each entry = (subHeader index) × 8
///  518 / 8 * N / subHeaders[]       SubHeader { firstCode, entryCount, idDelta, idRangeOffset }
///   ?  /     / glyphIdArray[]        u16 entries; offsets into here come from idRangeOffset
/// ```
///
/// Encoding the lookup of one codeunit:
///
/// 1. Split the input. For a single-byte codeunit (e.g. ASCII in a
///    Shift-JIS font), let `high = 0`, `low = codepoint`. For a 2-byte
///    codeunit, `high = (codepoint >> 8) & 0xFF`, `low = codepoint & 0xFF`.
/// 2. Index `subHeaderKeys[high]` → `k`. The selected SubHeader is the
///    one at byte-offset `518 + k` (i.e. `subHeaders[k / 8]`). `k = 0`
///    is the special "single-byte" SubHeader (SubHeader 0). Per the
///    spec a non-zero `k` for a single-byte code means the high byte
///    is a lead byte and the caller must follow with a second byte.
/// 3. Inside the SubHeader, the `low` byte must lie in
///    `[firstCode, firstCode + entryCount)`; otherwise the codepoint
///    maps to the missing glyph.
/// 4. Read the per-entry `u16` from the glyph-id sub-array. The spec
///    formula for that location is
///    `*(idRangeOffset/2 + (low - firstCode) + &idRangeOffset)`,
///    which expands in bytes to
///    `(&idRangeOffset) + idRangeOffset + 2 * (low - firstCode)`.
/// 5. If the raw value is 0 the glyph is missing (`None`). Otherwise
///    add `idDelta` (modulo 65536) to obtain the final glyph id.
///
/// SubHeader 0 ("single-byte chars use this") is used when the input
/// is a single byte AND `subHeaderKeys[0] == 0`. For a 2-byte input
/// whose high byte's key is also zero we still go through SubHeader 0
/// — but the input value must fit in 8 bits; if `high != 0` and the
/// key is zero, the spec considers it an invalid encoding and we
/// return `None`.
fn lookup_format2(bytes: &[u8], codepoint: u32) -> Option<u16> {
    // The input is a raw codeunit, not a Unicode scalar value. The
    // largest legal value is 0xFFFF (a 2-byte codeunit).
    if codepoint > 0xFFFF {
        return None;
    }
    let high = ((codepoint >> 8) & 0xFF) as u8;
    let low = (codepoint & 0xFF) as u8;

    // Header sanity: we need at least the 6-byte fixed prefix plus the
    // 256-entry subHeaderKeys array (= 518 bytes) before the first
    // SubHeader can begin.
    let sub_header_keys_off = 6usize;
    let sub_headers_off = sub_header_keys_off + 512;
    if bytes.len() < sub_headers_off {
        return None;
    }

    // subHeaderKeys[high] is a u16 already pre-multiplied by 8 — the
    // value is the byte offset *into the subHeaders array region* of
    // the SubHeader for this lead byte.
    let key = read_u16(bytes, sub_header_keys_off + (high as usize) * 2).ok()?;
    if key % 8 != 0 {
        // Malformed: each entry indexes 8-byte SubHeader records.
        return None;
    }
    let sub_header_offset = sub_headers_off + key as usize;
    if sub_header_offset + 8 > bytes.len() {
        return None;
    }

    let first_code = read_u16(bytes, sub_header_offset).ok()?;
    let entry_count = read_u16(bytes, sub_header_offset + 2).ok()?;
    // idDelta is documented int16 — read as u16 then reinterpret. The
    // spec mandates modulo-65536 arithmetic for the final addition so
    // we do the math in i32 and mask the low 16 bits at the end.
    let id_delta = read_u16(bytes, sub_header_offset + 4).ok()? as i16 as i32;
    let id_range_offset = read_u16(bytes, sub_header_offset + 6).ok()? as usize;

    // The SubHeader at offset 0 ("k = 0") is the special single-byte
    // SubHeader. The high byte for an ASCII-style single-byte char is
    // 0; for those, the low byte IS the whole codeunit and falls into
    // SubHeader 0's range. If the user hands us a 2-byte codeunit whose
    // high byte maps to SubHeader 0 (key = 0), the spec considers that
    // an encoder-side malformed input; we return None rather than
    // silently double-counting it.
    if key == 0 && high != 0 {
        return None;
    }

    // Bounds-check the low byte against [firstCode, firstCode + entryCount).
    // Both firstCode and entryCount are u16-bounded and entryCount may
    // legally be 0 for a SubHeader that exists only to act as a lead
    // byte sentinel; a zero-entry SubHeader matches nothing.
    if entry_count == 0 {
        return None;
    }
    let low_u16 = low as u16;
    if low_u16 < first_code {
        return None;
    }
    let idx = (low_u16 - first_code) as usize;
    if idx >= entry_count as usize {
        return None;
    }

    // Spec formula in bytes:
    //   target = (address of idRangeOffset field) + idRangeOffset + 2 * idx
    // The idRangeOffset field sits at sub_header_offset + 6.
    let id_range_field_addr = sub_header_offset + 6;
    let target = id_range_field_addr
        .checked_add(id_range_offset)?
        .checked_add(2 * idx)?;
    let raw = read_u16(bytes, target).ok()?;
    if raw == 0 {
        return None;
    }
    // (raw + idDelta) mod 65536. Cast back to u16 via the low 16 bits.
    let g = (raw as i32 + id_delta) & 0xFFFF;
    Some(g as u16)
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
    //  14 + 2*segCount        / reservedPad (u16, = 0)
    //  16 + 2*segCount        / startCode[segCount]
    //  16 + 4*segCount        / idDelta[segCount]
    //  16 + 6*segCount        / idRangeOffset[segCount]
    //  16 + 8*segCount        / glyphIdArray[…] (variable, addressed
    //                                            relative to idRangeOffset[i])
    //
    // Format-4 mandates a terminator segment whose endCode is 0xFFFF;
    // a well-formed cmap always has at least one segment.
    let seg_count_x2 = read_u16(bytes, 6).ok()? as usize;
    let seg_count = seg_count_x2 / 2;
    if seg_count == 0 {
        // Malformed: a real font cannot have segCount = 0 (the
        // terminator alone forces seg_count >= 1). Refuse to look up.
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
    // Binary-search the endCode[] array for the first segment whose
    // endCode >= cp. endCode[] is sorted ascending per the spec (the
    // searchRange / entrySelector / rangeShift triple at offset 8..14
    // exists precisely so a hardware-constrained reader can binary-search
    // it without a divide); we don't need those values because we have
    // a real binary search. This makes BMP lookups O(log N) instead of
    // O(N), which matters for Asian fonts that ship 100+ segments.
    let mut lo = 0usize;
    let mut hi = seg_count;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let end = read_u16(bytes, end_code_off + mid * 2).ok()?;
        if end < cp {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo >= seg_count {
        // cp is greater than every endCode — only possible if the font
        // is malformed (the 0xFFFF terminator should always match).
        return None;
    }
    let seg = lo;
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
    //
    // Bail rather than wrap if a malformed font causes either side to
    // exceed `usize`. In practice all three operands are u16-bounded so
    // the sum fits trivially; the checked_* chain is belt-and-braces.
    let target = (id_range_offset_off + seg * 2)
        .checked_add(id_range_offset as usize)?
        .checked_add(2 * (cp as usize - start as usize))?;
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

// --- Format 13 -------------------------------------------------------------

/// Format 13 — many-to-one range mappings.
///
/// On-wire structure is identical to format 12 (`u32 numGroups`
/// followed by groups of `u32 startCharCode`, `u32 endCharCode`,
/// `u32 glyphID`). The semantic difference: every codepoint in
/// `[startCharCode..=endCharCode]` maps to the SAME `glyphID`, not to
/// `glyphID + (cp - startCharCode)`. The cmap chapter calls out this
/// distinction explicitly under "Subtable format 13 has the same
/// structure as format 12; it differs only in the interpretation of
/// the startGlyphID/glyphID fields."
///
/// glyphID 0 is the `.notdef` slot and is treated the same way the
/// other formats treat a hit on glyph 0: returned as `None`. A real
/// last-resort font would map gaps in its coverage to glyph 0 to mean
/// "no opinion on this codepoint" while pointing the covered ranges
/// at a distinct tofu glyph; the `None` is the explicit "no opinion"
/// signal.
fn lookup_format13(bytes: &[u8], codepoint: u32) -> Option<u16> {
    let num_groups = read_u32(bytes, 12).ok()? as usize;
    if 16 + num_groups * 12 > bytes.len() {
        return None;
    }
    // Binary search by start ≤ cp ≤ end. Identical traversal to
    // format 12; only the per-group glyph derivation differs.
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
            let glyph = read_u32(bytes, off + 8).ok()?;
            if glyph == 0 || glyph > u16::MAX as u32 {
                return None;
            }
            return Some(glyph as u16);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_cmap_with_subtable(format: u16, sub: &[u8]) -> Vec<u8> {
        // 1 encoding record. Platform / encoding picked so the rank
        // ordering picks our sole subtable:
        //   format 13 → (0, 6) "Unicode full repertoire — for use with
        //                       subtable format 13" per spec.
        //   format 12 → (3, 10) Windows Unicode UCS-4
        //   format 2  → (1, 1)  Macintosh Japanese (Shift-JIS-ish) — a
        //                       legacy script-platform pair that goes
        //                       through the catch-all platform branch
        //                       of subtable_rank.
        //   else      → (3, 1)  Windows Unicode BMP
        let mut out = vec![0u8; 4 + 8];
        out[0..2].copy_from_slice(&0u16.to_be_bytes()); // version
        out[2..4].copy_from_slice(&1u16.to_be_bytes()); // numTables
        let (platform, enc): (u16, u16) = match format {
            13 => (0, 6),
            12 => (3, 10),
            2 => (1, 1),
            _ => (3, 1),
        };
        out[4..6].copy_from_slice(&platform.to_be_bytes());
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

    /// Build a format-4 subtable from an arbitrary list of
    /// (startCode, endCode, idDelta) direct-mapping segments. The
    /// 0xFFFF..0xFFFF terminator with id_delta=1 is appended automatically
    /// (per the spec: "the last endCode must always be 0xFFFF"). All
    /// `idRangeOffset`s are zero (direct mapping; the `glyph_id_array`
    /// is empty).
    fn build_format4_direct(segs: &[(u16, u16, u16)]) -> Vec<u8> {
        let mut all: Vec<(u16, u16, u16)> = segs.to_vec();
        all.push((0xFFFF, 0xFFFF, 1));
        let seg_count = all.len() as u16;
        let seg_count_x2 = seg_count * 2;
        let header = 14;
        let arrays_len = seg_count_x2 as usize * 4 + 2 /*reserved pad*/;
        let length = header + arrays_len;
        let mut sub = vec![0u8; length];
        sub[0..2].copy_from_slice(&4u16.to_be_bytes());
        sub[2..4].copy_from_slice(&(length as u16).to_be_bytes());
        sub[6..8].copy_from_slice(&seg_count_x2.to_be_bytes());
        // endCode[]
        for (i, (_, end, _)) in all.iter().enumerate() {
            let off = 14 + i * 2;
            sub[off..off + 2].copy_from_slice(&end.to_be_bytes());
        }
        // reservedPad already zero.
        let start_off = 14 + seg_count_x2 as usize + 2;
        let delta_off = start_off + seg_count_x2 as usize;
        // idRangeOffset[] follows at delta_off + seg_count_x2; already zero.
        for (i, (start, _, delta)) in all.iter().enumerate() {
            sub[start_off + i * 2..start_off + i * 2 + 2].copy_from_slice(&start.to_be_bytes());
            sub[delta_off + i * 2..delta_off + i * 2 + 2].copy_from_slice(&delta.to_be_bytes());
        }
        sub
    }

    /// Regression for the binary-search rewrite: a cmap with 200
    /// single-codepoint segments (Asian / large-coverage fonts ship
    /// counts in this range) must resolve every covered codepoint
    /// correctly AND must miss correctly on the gaps between segments.
    /// The previous linear-scan implementation also passed this but
    /// at O(N); the binary search must produce identical answers.
    #[test]
    fn format4_binary_search_resolves_many_segments() {
        // 200 segments at codepoints 0x0100, 0x0102, 0x0104, ...; each
        // covers one codepoint that maps to glyph (cp - 0x0100 + 1).
        let mut segs: Vec<(u16, u16, u16)> = Vec::with_capacity(200);
        for i in 0..200u16 {
            let cp = 0x0100 + i * 2;
            // id_delta is applied modulo 65536; to land glyph `i+1` for
            // codepoint `cp`, we want delta = (i + 1 - cp) mod 65536.
            let want_glyph = i + 1;
            let delta = want_glyph.wrapping_sub(cp);
            segs.push((cp, cp, delta));
        }
        let sub = build_format4_direct(&segs);
        let cmap_bytes = build_cmap_with_subtable(4, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        for i in 0..200u16 {
            let cp = 0x0100 + i * 2;
            assert_eq!(
                cmap.lookup(cp as u32),
                Some(i + 1),
                "codepoint {cp:#06x} expected glyph {}",
                i + 1
            );
            // The gap immediately after each covered codepoint must miss:
            // the next segment's startCode > cp+1, so the search lands
            // on a higher-endCode segment whose startCode rejects us.
            // Don't test the gap after segment 199 because the terminator
            // (0xFFFF) would match it; the spec's terminator hands back
            // glyph 0 → None which is correct but tested elsewhere.
            if i + 1 < 200 {
                assert_eq!(
                    cmap.lookup((cp + 1) as u32),
                    None,
                    "codepoint {:#06x} unexpectedly mapped",
                    cp + 1
                );
            }
        }
        // Codepoint below the first segment must miss.
        assert_eq!(cmap.lookup(0x00FF), None);
    }

    /// Indirect mapping (idRangeOffset != 0) — the format-4 path that
    /// reads glyph IDs out of the `glyphIdArray` rather than computing
    /// them directly from id_delta. Covers a single 4-codepoint segment
    /// whose glyphIdArray entries are picked to verify both the
    /// `target` offset arithmetic and the `id_delta` post-fold.
    #[test]
    fn format4_indirect_mapping_resolves_through_glyph_id_array() {
        // One real segment covering 'A'..'D' (65..68), one terminator.
        // idRangeOffset[0] points at the start of glyphIdArray (the
        // byte immediately after the last idRangeOffset entry); per the
        // spec formula this means offset = 2 * segCount - 2*0 (we are
        // the first segment, so the byte after the second
        // idRangeOffset). The glyphIdArray then holds 4 entries: the
        // raw u16s that get summed with id_delta to produce the final
        // glyph.
        let seg_count: u16 = 2;
        let seg_count_x2: u16 = seg_count * 2;
        let header = 14;
        // glyphIdArray: 4 u16 entries (one per codepoint covered).
        let glyph_id_array_bytes: usize = 4 * 2;
        let arrays_len = seg_count_x2 as usize * 4 + 2 /*reserved pad*/ + glyph_id_array_bytes;
        let length = header + arrays_len;
        let mut sub = vec![0u8; length];
        sub[0..2].copy_from_slice(&4u16.to_be_bytes());
        sub[2..4].copy_from_slice(&(length as u16).to_be_bytes());
        sub[6..8].copy_from_slice(&seg_count_x2.to_be_bytes());
        // endCode[]
        sub[14..16].copy_from_slice(&68u16.to_be_bytes());
        sub[16..18].copy_from_slice(&0xFFFFu16.to_be_bytes());
        // reservedPad at [18..20] zero.
        // startCode[]
        sub[20..22].copy_from_slice(&65u16.to_be_bytes());
        sub[22..24].copy_from_slice(&0xFFFFu16.to_be_bytes());
        // idDelta[] — pick +10 so the raw u16s above are easy to read.
        sub[24..26].copy_from_slice(&10u16.to_be_bytes());
        sub[26..28].copy_from_slice(&1u16.to_be_bytes());
        // idRangeOffset[]:
        //   For seg 0 the spec formula:
        //     target_byte = &idRangeOffset[0] + idRangeOffset[0]
        //                                     + 2 * (cp - startCode[0])
        //   We want target_byte to land on `glyph_id_array_off`
        //   (= id_range_offset_off + seg_count_x2). So
        //   idRangeOffset[0] = (glyph_id_array_off - &idRangeOffset[0])
        //                   = seg_count_x2 - 0
        //                   = 4.
        let id_range_offset_off = 28usize;
        sub[id_range_offset_off..id_range_offset_off + 2].copy_from_slice(&4u16.to_be_bytes());
        // seg 1 (terminator) idRangeOffset = 0 (direct).

        // glyphIdArray entries: raw values 100, 200, 300, 400. After
        // adding id_delta=10 the resolved glyphs are 110, 210, 310, 410.
        let glyph_id_array_off = id_range_offset_off + seg_count_x2 as usize; // 32
        for (i, &raw) in [100u16, 200, 300, 400].iter().enumerate() {
            let off = glyph_id_array_off + i * 2;
            sub[off..off + 2].copy_from_slice(&raw.to_be_bytes());
        }

        let cmap_bytes = build_cmap_with_subtable(4, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        assert_eq!(cmap.lookup('A' as u32), Some(110));
        assert_eq!(cmap.lookup('B' as u32), Some(210));
        assert_eq!(cmap.lookup('C' as u32), Some(310));
        assert_eq!(cmap.lookup('D' as u32), Some(410));
        // Out of range BMP cp still misses cleanly.
        assert_eq!(cmap.lookup('E' as u32), None);
    }

    /// A malformed format-4 subtable that *claims* a length large
    /// enough to enclose its arrays but is actually truncated past
    /// the idRangeOffset[] table. The lookup must NOT panic and must
    /// return None rather than reading out-of-bounds. We deliberately
    /// shrink the cmap bytes after building it to expose the bounds
    /// check.
    #[test]
    fn format4_truncated_arrays_does_not_panic() {
        let segs = vec![(65u16, 67u16, 35u16)];
        let sub = build_format4_direct(&segs);
        let mut cmap_bytes = build_cmap_with_subtable(4, &sub);
        // Lop off the last 4 bytes — the trailing idRangeOffset entries
        // for the terminator segment. CmapTable::parse currently does
        // not bounds-check beyond `sub_off + length`, so the parse
        // succeeds with a too-short subtable slice; the lookup must
        // cope.
        let cut = cmap_bytes.len() - 4;
        cmap_bytes.truncate(cut);
        // Note: also adjust the subtable's claimed length so that
        // CmapTable::parse's `sub_off + length` slice-take doesn't fail.
        // The subtable starts at offset 12 in our test wrapper; the
        // length u16 is at sub_off + 2 = 14.
        let new_len = (sub.len() - 4) as u16;
        cmap_bytes[14..16].copy_from_slice(&new_len.to_be_bytes());
        let cmap = CmapTable::parse(&cmap_bytes).expect("parse must tolerate length=trimmed");
        // 'A' would have been at glyph 100. Either we get it (the
        // binary search lands in the truncated terminator segment and
        // misses cleanly) or None — but never a panic, and never a
        // spurious glyph from out-of-bounds bytes.
        let _ = cmap.lookup('A' as u32);
        let _ = cmap.lookup('Z' as u32);
    }

    // --- Format 13 ---------------------------------------------------------

    /// Build a format-13 subtable from a slice of
    /// `(startCharCode, endCharCode, glyphID)` ConstantMapGroup
    /// records. Caller is responsible for keeping the records sorted
    /// by `startCharCode` ascending (the lookup binary-searches them).
    fn build_format13(groups: &[(u32, u32, u32)]) -> Vec<u8> {
        let num_groups = groups.len() as u32;
        let sub_len = 16 + num_groups as usize * 12;
        let mut sub = vec![0u8; sub_len];
        sub[0..2].copy_from_slice(&13u16.to_be_bytes()); // format
                                                         // reserved at 2..4 stays zero
        sub[4..8].copy_from_slice(&(sub_len as u32).to_be_bytes()); // length
                                                                    // language at 8..12 stays zero
        sub[12..16].copy_from_slice(&num_groups.to_be_bytes()); // numGroups
        for (i, &(start, end, glyph)) in groups.iter().enumerate() {
            let off = 16 + i * 12;
            sub[off..off + 4].copy_from_slice(&start.to_be_bytes());
            sub[off + 4..off + 8].copy_from_slice(&end.to_be_bytes());
            sub[off + 8..off + 12].copy_from_slice(&glyph.to_be_bytes());
        }
        sub
    }

    /// Smallest format-13 case: a single range across the whole BMP
    /// pointing at a single tofu glyph. This is the canonical
    /// "LastResort"-style layout.
    #[test]
    fn format13_single_range_maps_all_to_one_glyph() {
        let sub = build_format13(&[(0x0000, 0xFFFF, 1)]);
        let cmap_bytes = build_cmap_with_subtable(13, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        assert_eq!(cmap.lookup(0x0041), Some(1)); // 'A'
        assert_eq!(cmap.lookup(0x4E00), Some(1)); // CJK U+4E00
        assert_eq!(cmap.lookup(0xFFFF), Some(1));
        // Above the covered range: out of the only group, so misses.
        assert_eq!(cmap.lookup(0x10000), None);
    }

    /// Multiple ranges that each point at distinct glyphs. The
    /// many-to-one property is per-range: every codepoint inside a
    /// given range collapses to that range's `glyphID`.
    #[test]
    fn format13_multi_range_each_collapses_to_its_glyph() {
        let sub = build_format13(&[
            // BMP Hiragana → glyph 2 (one tofu for "Japanese hiragana
            // is here but we don't have proper coverage").
            (0x3040, 0x309F, 2),
            // BMP CJK Unified Ideographs → glyph 3.
            (0x4E00, 0x9FFF, 3),
            // Supplementary plane emoji → glyph 4.
            (0x1F600, 0x1F64F, 4),
        ]);
        let cmap_bytes = build_cmap_with_subtable(13, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        // Every codepoint in each range hits the range's glyph.
        assert_eq!(cmap.lookup(0x3040), Some(2));
        assert_eq!(cmap.lookup(0x3060), Some(2));
        assert_eq!(cmap.lookup(0x309F), Some(2));
        assert_eq!(cmap.lookup(0x4E00), Some(3));
        assert_eq!(cmap.lookup(0x5000), Some(3));
        assert_eq!(cmap.lookup(0x9FFF), Some(3));
        assert_eq!(cmap.lookup(0x1F600), Some(4));
        assert_eq!(cmap.lookup(0x1F62D), Some(4));
        assert_eq!(cmap.lookup(0x1F64F), Some(4));
        // Codepoints between ranges miss cleanly.
        assert_eq!(cmap.lookup(0x303F), None);
        assert_eq!(cmap.lookup(0x30A0), None);
        assert_eq!(cmap.lookup(0xA000), None);
        assert_eq!(cmap.lookup(0x1F5FF), None);
    }

    /// Format 13 semantic difference from format 12: a 3-codepoint
    /// range with `glyphID = 7` resolves to glyph 7 for ALL three
    /// inputs, NOT to 7/8/9. This is the headline test that
    /// distinguishes the two formats.
    #[test]
    fn format13_does_not_add_running_offset() {
        // If we mis-decoded as format 12 we'd see (7, 8, 9) for the
        // three covered codepoints. Format 13 must hand back (7, 7, 7).
        let sub = build_format13(&[(0x0061, 0x0063, 7)]);
        let cmap_bytes = build_cmap_with_subtable(13, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        assert_eq!(cmap.lookup(0x0061), Some(7));
        assert_eq!(cmap.lookup(0x0062), Some(7));
        assert_eq!(cmap.lookup(0x0063), Some(7));
        // Boundary off-by-one: cp = end + 1 must miss.
        assert_eq!(cmap.lookup(0x0064), None);
    }

    /// A group whose `glyphID = 0` is a "no opinion" marker. We
    /// surface it the same way the other formats do: as `None`. A
    /// real last-resort font wouldn't use glyph 0 as the tofu (it
    /// would pick a visible one); the 0 hit means "explicitly absent".
    #[test]
    fn format13_glyph_zero_returns_none() {
        let sub = build_format13(&[(0x0030, 0x0039, 0)]);
        let cmap_bytes = build_cmap_with_subtable(13, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        for cp in 0x0030..=0x0039 {
            assert_eq!(cmap.lookup(cp), None, "cp {cp:#04x} should miss");
        }
    }

    /// Binary-search regression: many single-codepoint ranges all
    /// pointing at the same glyph. The binary search must find every
    /// covered codepoint and reject every gap.
    #[test]
    fn format13_binary_search_resolves_many_ranges() {
        let mut groups: Vec<(u32, u32, u32)> = Vec::with_capacity(200);
        for i in 0..200u32 {
            let cp = 0x10000 + i * 2; // every other supplementary cp
            groups.push((cp, cp, 5));
        }
        let sub = build_format13(&groups);
        let cmap_bytes = build_cmap_with_subtable(13, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        for i in 0..200u32 {
            let cp = 0x10000 + i * 2;
            assert_eq!(cmap.lookup(cp), Some(5), "cp {cp:#06x}");
            // The gap cp+1 is between two covered single-cp ranges.
            if i + 1 < 200 {
                assert_eq!(cmap.lookup(cp + 1), None, "gap after {cp:#06x}");
            }
        }
        // Below the first range.
        assert_eq!(cmap.lookup(0x0FFFF), None);
    }

    /// When a font ships both format 12 and format 13, the picker
    /// must keep the format-12 (sequential, real glyphs) subtable as
    /// the active base map. Format 13 is intentionally ranked below
    /// format 0 so it never displaces a real-coverage subtable.
    #[test]
    fn format13_does_not_displace_format12() {
        // -- format-12 subtable: U+0041 ('A') → glyph 100, distinct.
        let sub12_len = 16 + 12;
        let mut sub12 = vec![0u8; sub12_len];
        sub12[0..2].copy_from_slice(&12u16.to_be_bytes());
        sub12[4..8].copy_from_slice(&(sub12_len as u32).to_be_bytes());
        sub12[12..16].copy_from_slice(&1u32.to_be_bytes());
        sub12[16..20].copy_from_slice(&0x0041u32.to_be_bytes());
        sub12[20..24].copy_from_slice(&0x0041u32.to_be_bytes());
        sub12[24..28].copy_from_slice(&100u32.to_be_bytes());

        // -- format-13 subtable: U+0041 → glyph 1 (tofu).
        let sub13 = build_format13(&[(0x0041, 0x0041, 1)]);

        // Hand-roll the cmap header: 2 encoding records.
        let header_len = 4 + 2 * 8;
        let sub12_off = header_len;
        let sub13_off = sub12_off + sub12.len();
        let mut out = vec![0u8; header_len];
        out[0..2].copy_from_slice(&0u16.to_be_bytes());
        out[2..4].copy_from_slice(&2u16.to_be_bytes());
        // record 0: (3, 10) format-12 — rank 425.
        out[4..6].copy_from_slice(&3u16.to_be_bytes());
        out[6..8].copy_from_slice(&10u16.to_be_bytes());
        out[8..12].copy_from_slice(&(sub12_off as u32).to_be_bytes());
        // record 1: (0, 6) format-13 — rank 80 (= 50 + 30).
        out[12..14].copy_from_slice(&0u16.to_be_bytes());
        out[14..16].copy_from_slice(&6u16.to_be_bytes());
        out[16..20].copy_from_slice(&(sub13_off as u32).to_be_bytes());
        out.extend_from_slice(&sub12);
        out.extend_from_slice(&sub13);

        let cmap = CmapTable::parse(&out).unwrap();
        // Must resolve through format 12, NOT format 13.
        assert_eq!(cmap.lookup(0x0041), Some(100));
    }

    /// Sanity: a font that only ships a format-13 subtable still picks
    /// it. (This is the "true last-resort font" scenario.) The rank
    /// score for (format 13, platform 0, encoding 6) is 50 + 30 = 80,
    /// which beats the i32::MIN initial sentinel; the picker selects it.
    #[test]
    fn format13_only_is_pickable_as_last_resort() {
        let sub = build_format13(&[(0x0000, 0x10FFFF, 1)]);
        let cmap_bytes = build_cmap_with_subtable(13, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        assert_eq!(cmap.lookup(0x0041), Some(1));
        assert_eq!(cmap.lookup(0x1F600), Some(1));
        assert_eq!(cmap.lookup(0x10FFFF), Some(1));
    }

    // --- Format 2 ----------------------------------------------------------

    /// Build a format-2 subtable manually.
    ///
    /// `sub_header_keys` is a 256-entry array of u16 values; each value
    /// is the SubHeader index already multiplied by 8 (the spec's
    /// pre-baked offset). `sub_headers` is a list of
    /// `(firstCode, entryCount, idDelta, idRangeOffset)` SubHeader
    /// records. `glyph_id_array` is the trailing flat list of u16
    /// glyph IDs that the SubHeaders' `idRangeOffset` fields point
    /// into; this helper does NOT compute idRangeOffset for you —
    /// callers hand-craft it so the test exercises the spec formula.
    fn build_format2(
        sub_header_keys: &[u16; 256],
        sub_headers: &[(u16, u16, i16, u16)],
        glyph_id_array: &[u16],
    ) -> Vec<u8> {
        let header_len = 6;
        let keys_len = 512;
        let sub_headers_len = sub_headers.len() * 8;
        let glyph_array_len = glyph_id_array.len() * 2;
        let total = header_len + keys_len + sub_headers_len + glyph_array_len;
        let mut sub = vec![0u8; total];
        sub[0..2].copy_from_slice(&2u16.to_be_bytes()); // format
        sub[2..4].copy_from_slice(&(total as u16).to_be_bytes()); // length
                                                                  // language at 4..6 stays zero
        for (i, k) in sub_header_keys.iter().enumerate() {
            let off = 6 + i * 2;
            sub[off..off + 2].copy_from_slice(&k.to_be_bytes());
        }
        let sub_headers_off = header_len + keys_len;
        for (i, &(first_code, entry_count, id_delta, id_range_offset)) in
            sub_headers.iter().enumerate()
        {
            let off = sub_headers_off + i * 8;
            sub[off..off + 2].copy_from_slice(&first_code.to_be_bytes());
            sub[off + 2..off + 4].copy_from_slice(&entry_count.to_be_bytes());
            sub[off + 4..off + 6].copy_from_slice(&(id_delta as u16).to_be_bytes());
            sub[off + 6..off + 8].copy_from_slice(&id_range_offset.to_be_bytes());
        }
        let glyph_array_off = sub_headers_off + sub_headers_len;
        for (i, &g) in glyph_id_array.iter().enumerate() {
            let off = glyph_array_off + i * 2;
            sub[off..off + 2].copy_from_slice(&g.to_be_bytes());
        }
        sub
    }

    /// Smallest format-2 case: SubHeader 0 maps the seven ASCII digits
    /// '0'..'9' to glyphs 100..109 via a direct `idRangeOffset`. There
    /// are no lead bytes — every key entry is 0, so every byte input
    /// falls through SubHeader 0.
    #[test]
    fn format2_subheader_zero_maps_single_byte() {
        // sub_header_keys: all zero (every high byte → SubHeader 0).
        let keys = [0u16; 256];
        // SubHeader 0: firstCode = 0x30, entryCount = 10, idDelta = 0,
        //   idRangeOffset points from this SubHeader's idRangeOffset
        //   field to the start of glyph_id_array.
        // Address of SubHeader[0].idRangeOffset = 6 + 512 + 6 = 524.
        // Glyph-id array starts at byte 6 + 512 + 8 = 526.
        // idRangeOffset = 526 - 524 = 2.
        let sub_headers = [(0x30u16, 10u16, 0i16, 2u16)];
        let glyph_ids: Vec<u16> = (100..110).collect();
        let sub = build_format2(&keys, &sub_headers, &glyph_ids);
        let cmap_bytes = build_cmap_with_subtable(2, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        for (i, cp) in (0x30u32..0x3A).enumerate() {
            assert_eq!(cmap.lookup(cp), Some(100 + i as u16), "cp {cp:#x}");
        }
        // Codes outside [firstCode, firstCode + entryCount) miss.
        assert_eq!(cmap.lookup(0x2F), None);
        assert_eq!(cmap.lookup(0x3A), None);
    }

    /// id_delta is applied to a non-zero glyph-array entry. Spec:
    /// "Finally, if the value obtained from the sub-array is not 0 …
    /// you should add idDelta to it in order to get the glyphIndex."
    /// A zero entry stays a miss regardless of idDelta.
    #[test]
    fn format2_id_delta_offsets_nonzero_entries() {
        let keys = [0u16; 256];
        // Same SubHeader as above but with idDelta = +1000. The three
        // glyph-id-array slots are {200, 0, 300}: the middle 0 should
        // still resolve to None even though delta would carry it to 1000.
        let sub_headers = [(0x30u16, 3u16, 1000i16, 2u16)];
        let glyph_ids = [200u16, 0u16, 300u16];
        let sub = build_format2(&keys, &sub_headers, &glyph_ids);
        let cmap_bytes = build_cmap_with_subtable(2, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        assert_eq!(cmap.lookup(0x30), Some(1200));
        assert_eq!(cmap.lookup(0x31), None);
        assert_eq!(cmap.lookup(0x32), Some(1300));
    }

    /// idDelta modulo-65536 arithmetic. The spec is explicit: "The
    /// idDelta arithmetic is modulo 65536." A delta of -1 applied to
    /// glyph 5 yields glyph 4; a delta of -10 applied to glyph 5
    /// would naively be -5 but the spec mandates wraparound. We pick
    /// numbers that wrap cleanly: raw = 5, idDelta = -6, expected = 65535.
    #[test]
    fn format2_id_delta_wraps_modulo_65536() {
        let keys = [0u16; 256];
        let sub_headers = [(0x30u16, 1u16, -6i16, 2u16)];
        let glyph_ids = [5u16];
        let sub = build_format2(&keys, &sub_headers, &glyph_ids);
        let cmap_bytes = build_cmap_with_subtable(2, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        // (5 + (-6)) mod 65536 = 65535. Spec: "If the result … is less
        // than zero, add 65536 to obtain a valid glyph ID."
        assert_eq!(cmap.lookup(0x30), Some(0xFFFF));
    }

    /// Two-byte lookup through a non-zero SubHeader.
    ///
    /// Encoding: high byte 0x81 is a lead byte; the spec stores its
    /// SubHeader-index-times-8 at `subHeaderKeys[0x81]`. SubHeader 0
    /// is single-byte (covers 0x20..0x7F). SubHeader 1 is for high
    /// byte 0x81 and covers low byte 0x40..0x42.
    #[test]
    fn format2_lead_byte_routes_to_subheader() {
        let mut keys = [0u16; 256];
        // SubHeader 1 → byte offset 8 in the SubHeaders region.
        keys[0x81] = 8;

        // SubHeader 0 (single-byte fallback): firstCode = 0x20,
        //   entryCount = 1, idDelta = 0.
        //   Its idRangeOffset field sits at byte 6 + 512 + 0 + 6 = 524.
        //   Glyph-id array starts at byte 6 + 512 + 16 = 534 (two
        //   SubHeaders × 8 bytes).
        //   SubHeader 0's entry occupies glyph_id_array[0]; we want
        //   that pointer to land at byte 534 → idRangeOffset = 534 - 524
        //   = 10.
        // SubHeader 1 (lead-byte 0x81): firstCode = 0x40, entryCount = 3,
        //   idDelta = 0.
        //   Its idRangeOffset field sits at byte 6 + 512 + 8 + 6 = 532.
        //   We want it to point at glyph_id_array[1] = byte 536 →
        //   idRangeOffset = 536 - 532 = 4.
        let sub_headers = [
            (0x20u16, 1u16, 0i16, 10u16), // SubHeader 0
            (0x40u16, 3u16, 0i16, 4u16),  // SubHeader 1
        ];
        // glyph_id_array layout:
        //   [0] = glyph for SubHeader 0 / single-byte 0x20      → 7
        //   [1..=3] = glyphs for SubHeader 1 / 0x40 / 0x41 / 0x42
        let glyph_ids = [7u16, 1001u16, 1002u16, 1003u16];
        let sub = build_format2(&keys, &sub_headers, &glyph_ids);
        let cmap_bytes = build_cmap_with_subtable(2, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        // Single-byte ASCII space goes through SubHeader 0.
        assert_eq!(cmap.lookup(0x20), Some(7));
        // 2-byte codeunit 0x8140 / 0x8141 / 0x8142 go through SubHeader 1.
        assert_eq!(cmap.lookup(0x8140), Some(1001));
        assert_eq!(cmap.lookup(0x8141), Some(1002));
        assert_eq!(cmap.lookup(0x8142), Some(1003));
        // Out of SubHeader 1's [firstCode, firstCode + entryCount).
        assert_eq!(cmap.lookup(0x8143), None);
        assert_eq!(cmap.lookup(0x813F), None);
        // High byte 0x82 isn't a lead byte (its key is 0) and the
        // would-be low byte is 0x40 — outside SubHeader 0's range —
        // so this is rejected as an invalid encoding rather than
        // accidentally double-dispatched.
        assert_eq!(cmap.lookup(0x8240), None);
    }

    /// Two SubHeaders sharing a single sub-array. The spec calls this
    /// out as the reason idDelta exists: "The value idDelta permits
    /// the same sub-array to be used for several different
    /// subheaders." Two lead bytes (0x81 and 0x82) both use the same
    /// 3-entry sub-array but with different idDeltas, so the same
    /// underlying glyph slots produce different glyph IDs.
    #[test]
    fn format2_id_delta_lets_subheaders_share_sub_array() {
        let mut keys = [0u16; 256];
        keys[0x81] = 8; // SubHeader 1
        keys[0x82] = 16; // SubHeader 2

        // Three SubHeaders × 8 bytes = 24 bytes of SubHeader region.
        //   SubHeader 0 (single-byte fallback): no usable glyphs;
        //     firstCode = 0, entryCount = 0 disables it.
        //   SubHeader 1 (lead 0x81): idRangeOffset field at byte
        //     6 + 512 + 8 + 6 = 532. Points at glyph_id_array[0] @ byte
        //     6 + 512 + 24 = 542; idRangeOffset = 542 - 532 = 10.
        //     idDelta = +100.
        //   SubHeader 2 (lead 0x82): idRangeOffset field at byte
        //     6 + 512 + 16 + 6 = 540. Points at SAME glyph_id_array[0]
        //     @ byte 542; idRangeOffset = 542 - 540 = 2.
        //     idDelta = +200.
        let sub_headers = [
            (0x00u16, 0u16, 0i16, 0u16),
            (0x40u16, 3u16, 100i16, 10u16),
            (0x40u16, 3u16, 200i16, 2u16),
        ];
        let glyph_ids = [1u16, 2u16, 3u16];
        let sub = build_format2(&keys, &sub_headers, &glyph_ids);
        let cmap_bytes = build_cmap_with_subtable(2, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        // Through SubHeader 1: (1, 2, 3) + 100.
        assert_eq!(cmap.lookup(0x8140), Some(101));
        assert_eq!(cmap.lookup(0x8141), Some(102));
        assert_eq!(cmap.lookup(0x8142), Some(103));
        // Through SubHeader 2: (1, 2, 3) + 200, same underlying slots.
        assert_eq!(cmap.lookup(0x8240), Some(201));
        assert_eq!(cmap.lookup(0x8241), Some(202));
        assert_eq!(cmap.lookup(0x8242), Some(203));
    }

    /// A SubHeader's raw glyph-array entry of 0 stays "missing glyph"
    /// even when idDelta would push it to a non-zero value. This is
    /// the spec's "not 0" guard before the addition.
    #[test]
    fn format2_zero_glyph_array_entry_is_missing_glyph() {
        let mut keys = [0u16; 256];
        keys[0x81] = 8;
        let sub_headers = [
            (0x00u16, 0u16, 0i16, 0u16),
            // SubHeader 1: idRangeOffset field at byte 532; glyph-array
            // at byte 6 + 512 + 16 = 534. idRangeOffset = 534 - 532 = 2.
            (0x40u16, 2u16, 500i16, 2u16),
        ];
        let glyph_ids = [0u16, 42u16];
        let sub = build_format2(&keys, &sub_headers, &glyph_ids);
        let cmap_bytes = build_cmap_with_subtable(2, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        // 0x8140 → raw 0 → None, despite idDelta = +500.
        assert_eq!(cmap.lookup(0x8140), None);
        // 0x8141 → raw 42 → 542.
        assert_eq!(cmap.lookup(0x8141), Some(542));
    }

    /// A 2-byte codeunit whose high byte routes through SubHeader 0 is
    /// rejected. The spec considers SubHeader 0 the "single-byte
    /// character" SubHeader; a high byte that is NOT a registered lead
    /// byte but is also non-zero is a malformed input rather than a
    /// silent fall-through.
    #[test]
    fn format2_high_byte_through_subheader_zero_is_rejected() {
        let keys = [0u16; 256]; // every high byte → SubHeader 0
        let sub_headers = [(0x20u16, 0xE0u16, 0i16, 2u16)];
        let glyph_ids: Vec<u16> = (1u16..(1 + 0xE0)).collect();
        let sub = build_format2(&keys, &sub_headers, &glyph_ids);
        let cmap_bytes = build_cmap_with_subtable(2, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        // Single-byte 'A' → fine.
        assert_eq!(cmap.lookup(0x41), Some(1 + (0x41 - 0x20) as u16));
        // 2-byte 0x4141 has high byte 0x41 routed through SubHeader 0;
        // we reject this rather than treating it as a 1-byte 0x41.
        assert_eq!(cmap.lookup(0x4141), None);
    }

    /// A real-coverage subtable (format 4 / 12) must always outrank a
    /// format-2 sidecar. Real-world: a CJK font that ships both a
    /// modern format-4 Unicode subtable AND a legacy format-2 sidecar
    /// for old non-Unicode renderers — we want the Unicode map active.
    #[test]
    fn format2_does_not_displace_format12() {
        // -- format-12 subtable: U+0041 ('A') → glyph 7, distinct.
        let sub12_len = 16 + 12;
        let mut sub12 = vec![0u8; sub12_len];
        sub12[0..2].copy_from_slice(&12u16.to_be_bytes());
        sub12[4..8].copy_from_slice(&(sub12_len as u32).to_be_bytes());
        sub12[12..16].copy_from_slice(&1u32.to_be_bytes());
        sub12[16..20].copy_from_slice(&0x0041u32.to_be_bytes());
        sub12[20..24].copy_from_slice(&0x0041u32.to_be_bytes());
        sub12[24..28].copy_from_slice(&7u32.to_be_bytes());

        // -- format-2 subtable: byte 0x41 → glyph 99 (would-be hijack).
        let keys = [0u16; 256];
        let sub_headers = [(0x41u16, 1u16, 0i16, 2u16)];
        let glyph_ids = [99u16];
        let sub2 = build_format2(&keys, &sub_headers, &glyph_ids);

        // Hand-roll the cmap header: 2 encoding records.
        let header_len = 4 + 2 * 8;
        let sub12_off = header_len;
        let sub2_off = sub12_off + sub12.len();
        let mut out = vec![0u8; header_len];
        out[0..2].copy_from_slice(&0u16.to_be_bytes());
        out[2..4].copy_from_slice(&2u16.to_be_bytes());
        // record 0: (3, 10) format-12 — rank 425.
        out[4..6].copy_from_slice(&3u16.to_be_bytes());
        out[6..8].copy_from_slice(&10u16.to_be_bytes());
        out[8..12].copy_from_slice(&(sub12_off as u32).to_be_bytes());
        // record 1: (1, 1) format-2 — rank 60 + 5 = 65.
        out[12..14].copy_from_slice(&1u16.to_be_bytes());
        out[14..16].copy_from_slice(&1u16.to_be_bytes());
        out[16..20].copy_from_slice(&(sub2_off as u32).to_be_bytes());
        out.extend_from_slice(&sub12);
        out.extend_from_slice(&sub2);

        let cmap = CmapTable::parse(&out).unwrap();
        // Must resolve through format 12, NOT format 2.
        assert_eq!(cmap.lookup(0x0041), Some(7));
    }

    /// A font that ships ONLY a format-2 subtable still parses and
    /// looks up. (Legacy pre-Unicode CJK font scenario.)
    #[test]
    fn format2_only_is_pickable() {
        let keys = [0u16; 256];
        let sub_headers = [(0x30u16, 10u16, 0i16, 2u16)];
        let glyph_ids: Vec<u16> = (1u16..11).collect();
        let sub = build_format2(&keys, &sub_headers, &glyph_ids);
        let cmap_bytes = build_cmap_with_subtable(2, &sub);
        let cmap = CmapTable::parse(&cmap_bytes).unwrap();
        for (i, cp) in (0x30u32..0x3A).enumerate() {
            assert_eq!(cmap.lookup(cp), Some(1 + i as u16));
        }
    }
}
