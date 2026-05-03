//! `cmap` — character → glyph map.
//!
//! We pick a single subtable at parse time (preferred order: 32-bit
//! formats first, BMP formats second, legacy single-byte last) and run
//! all `lookup` calls through it. Round-1 supports formats 0, 4, 6, 12.

use crate::parser::{read_u16, read_u32};
use crate::Error;

/// Decoded cmap subtable, preselected from the candidate list.
#[derive(Debug, Clone)]
pub struct CmapTable<'a> {
    subtable: Subtable<'a>,
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

        // We want the *richest* subtable: prefer Unicode 32-bit (format
        // 12), then any BMP format-4, then format-6, then format-0.
        // Walk all encoding records and collect candidates.
        let mut best: Option<Subtable<'_>> = None;
        let mut best_rank = i32::MIN;

        for i in 0..num_tables as usize {
            let off = 4 + i * 8;
            let platform_id = read_u16(bytes, off)?;
            let encoding_id = read_u16(bytes, off + 2)?;
            let sub_off = read_u32(bytes, off + 4)? as usize;
            if sub_off + 2 > bytes.len() {
                return Err(Error::BadOffset);
            }
            let format = read_u16(bytes, sub_off)?;
            let length = subtable_length(bytes, sub_off, format)?;
            let sub = bytes
                .get(sub_off..sub_off + length)
                .ok_or(Error::BadOffset)?;

            let candidate = match format {
                0 => Some(Subtable::Format0(sub)),
                4 => Some(Subtable::Format4(sub)),
                6 => Some(Subtable::Format6(sub)),
                12 => Some(Subtable::Format12(sub)),
                _ => None, // formats 2/8/10/13/14 ignored in round 1
            };
            if let Some(c) = candidate {
                let rank = subtable_rank(format, platform_id, encoding_id);
                if rank > best_rank {
                    best_rank = rank;
                    best = Some(c);
                }
            }
        }

        Ok(Self {
            subtable: best.ok_or(Error::UnsupportedCmapFormat(0xFFFF))?,
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
}

fn subtable_length(bytes: &[u8], off: usize, format: u16) -> Result<usize, Error> {
    // Formats 0/4/6 have a u16 length at offset+2. Formats 8/10/12/13
    // have a u32 length at offset+4.
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
