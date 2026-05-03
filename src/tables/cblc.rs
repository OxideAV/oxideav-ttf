//! `CBLC` — Color Bitmap Location Table (and the EBLC layout it inherits).
//!
//! Per Microsoft OpenType spec (CBLC + EBLC; CBLC is a downward-compatible
//! superset of EBLC), this table indexes the per-glyph byte offsets of
//! the actual bitmap data living in the `CBDT` table. We parse only the
//! pieces that are needed to support Format 17/18/19 (PNG-encoded color
//! glyph) entries — that's enough for Noto Color Emoji and every other
//! Google "embedded PNG" colour-emoji font.
//!
//! The table layout we walk:
//!
//! ```text
//! CblcHeader {
//!     u16 majorVersion;     // = 3 for CBLC, = 2 for EBLC
//!     u16 minorVersion;     // = 0
//!     u32 numSizes;
//!     BitmapSize bitmapSizes[numSizes];
//! }
//! BitmapSize {
//!     u32 indexSubtableListOffset;   // from start of CBLC
//!     u32 indexSubtableListSize;
//!     u32 numberOfIndexSubtables;
//!     u32 colorRef;                  // ignored
//!     SbitLineMetrics hori;          // 12 bytes
//!     SbitLineMetrics vert;          // 12 bytes
//!     u16 startGlyphIndex;
//!     u16 endGlyphIndex;
//!     u8  ppemX;
//!     u8  ppemY;
//!     u8  bitDepth;                  // 1/2/4/8 grayscale or 32 BGRA
//!     i8  flags;
//! }
//! IndexSubtableRecord {
//!     u16 firstGlyphIndex;
//!     u16 lastGlyphIndex;
//!     u32 indexSubtableOffset;       // from start of IndexSubtableList
//! }
//! IndexSubHeader {
//!     u16 indexFormat;               // 1..5
//!     u16 imageFormat;               // CBDT format: 17/18/19 = PNG
//!     u32 imageDataOffset;           // base offset into CBDT
//! }
//! ```
//!
//! Format 1: u32 sbitOffsets[lastGid - firstGid + 2]. Per-glyph offset
//!           stored in 32-bit; lengths derived from the next entry.
//! Format 2: constant-metrics. u32 imageSize + BigGlyphMetrics. Every
//!           glyph in the range has the same blob size.
//! Format 3: like format 1 but u16 offsets (saves 2 bytes per entry).
//! Format 4: sparse glyph IDs. u32 numGlyphs + (u16 gid, u16 offset)
//!           pairs (numGlyphs + 1 entries; final pair holds the trailing
//!           offset for length).
//! Format 5: sparse + constant-metrics. u32 imageSize + BigGlyphMetrics
//!           + u32 numGlyphs + u16 glyphIdArray[numGlyphs].
//!
//! We resolve glyph → `(image_format, image_data_offset, image_data_len,
//! ppem_x, ppem_y, metrics_kind)` and let the consumer crate pull the
//! actual bytes out of the CBDT slice. `metrics_kind` distinguishes
//! "small metrics live in CBDT next to the data" (formats 17 + 19; the
//! 19 case has them in CBLC's BigGlyphMetrics) from "big metrics live in
//! CBDT" (format 18) so the caller can decode the right header. We do
//! NOT decode the PNG ourselves — that's the consumer crate's job via
//! `oxideav-png`.

use crate::parser::{read_i8, read_u16, read_u32, read_u8};
use crate::Error;

/// Per-glyph entry resolved out of CBLC. The caller indexes into the
/// CBDT slice using `image_data_offset .. image_data_offset + data_len`
/// to pull the per-glyph entry, then decodes that entry per `image_format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CblcEntry {
    /// `imageFormat` from the IndexSubHeader — 17, 18 or 19 for PNG.
    pub image_format: u16,
    /// Byte offset into the CBDT table for the start of this glyph's
    /// per-glyph entry (NOT the PNG data itself; the PNG starts after
    /// the in-CBDT metrics + dataLen header).
    pub image_data_offset: u32,
    /// Length of this glyph's CBDT entry in bytes (including the
    /// per-CBDT-format metrics header).
    pub data_len: u32,
    /// Strike pixels-per-em on the X axis.
    pub ppem_x: u8,
    /// Strike pixels-per-em on the Y axis.
    pub ppem_y: u8,
    /// `bitDepth` from the BitmapSize record. 32 for color (BGRA / PNG).
    pub bit_depth: u8,
    /// Constant-metrics shortcut for Format 2 / 5 — when the IndexSubtable
    /// declares a per-strike fixed metrics block, we lift it here so the
    /// CBDT consumer doesn't need to re-walk CBLC for it.
    pub fixed_metrics: Option<BigGlyphMetrics>,
}

/// `BigGlyphMetrics` (8 bytes) — used by CBDT format 18 inline + by CBLC
/// IndexSubtable formats 2 / 5 when the strike has constant metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BigGlyphMetrics {
    pub height: u8,
    pub width: u8,
    pub hori_bearing_x: i8,
    pub hori_bearing_y: i8,
    pub hori_advance: u8,
    pub vert_bearing_x: i8,
    pub vert_bearing_y: i8,
    pub vert_advance: u8,
}

impl BigGlyphMetrics {
    pub(crate) fn parse(bytes: &[u8], off: usize) -> Result<Self, Error> {
        Ok(Self {
            height: read_u8(bytes, off)?,
            width: read_u8(bytes, off + 1)?,
            hori_bearing_x: read_i8(bytes, off + 2)?,
            hori_bearing_y: read_i8(bytes, off + 3)?,
            hori_advance: read_u8(bytes, off + 4)?,
            vert_bearing_x: read_i8(bytes, off + 5)?,
            vert_bearing_y: read_i8(bytes, off + 6)?,
            vert_advance: read_u8(bytes, off + 7)?,
        })
    }
}

/// `SmallGlyphMetrics` (5 bytes) — used by CBDT format 17 inline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SmallGlyphMetrics {
    pub height: u8,
    pub width: u8,
    pub bearing_x: i8,
    pub bearing_y: i8,
    pub advance: u8,
}

impl SmallGlyphMetrics {
    pub fn parse(bytes: &[u8], off: usize) -> Result<Self, Error> {
        Ok(Self {
            height: read_u8(bytes, off)?,
            width: read_u8(bytes, off + 1)?,
            bearing_x: read_i8(bytes, off + 2)?,
            bearing_y: read_i8(bytes, off + 3)?,
            advance: read_u8(bytes, off + 4)?,
        })
    }
}

/// One strike (ppemX, ppemY) parsed out of CBLC. Carries enough info to
/// resolve any glyph in this strike.
#[derive(Debug, Clone, Copy)]
struct StrikeRecord {
    index_subtable_list_offset: u32,
    /// Cached for diagnostic / future use; not read by `lookup_in_strike`.
    #[allow(dead_code)]
    index_subtable_list_size: u32,
    number_of_index_subtables: u32,
    start_glyph_index: u16,
    end_glyph_index: u16,
    ppem_x: u8,
    ppem_y: u8,
    bit_depth: u8,
}

const BITMAP_SIZE_RECORD_LEN: usize = 4 + 4 + 4 + 4 + 12 + 12 + 2 + 2 + 1 + 1 + 1 + 1; // 48
const SBIT_LINE_METRICS_LEN: usize = 12;

/// Parsed CBLC table. Stores the original byte slice + the strike records;
/// per-glyph resolution walks the IndexSubtableList lazily.
#[derive(Debug, Clone)]
pub struct CblcTable<'a> {
    bytes: &'a [u8],
    strikes: Vec<StrikeRecord>,
}

impl<'a> CblcTable<'a> {
    /// Parse the CBLC header + BitmapSize array. Per-strike IndexSubtable
    /// walking happens lazily on `lookup_glyph` to avoid pre-decoding
    /// every entry up-front (a colour-emoji font ships thousands).
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < 8 {
            return Err(Error::UnexpectedEof);
        }
        let major = read_u16(bytes, 0)?;
        let _minor = read_u16(bytes, 2)?;
        // CBLC = 3, EBLC = 2. Accept both since the layout is identical
        // for everything we touch (BitmapSize + IndexSubtable formats).
        if major != 2 && major != 3 {
            return Err(Error::BadStructure("CBLC: unknown major version"));
        }
        let num_sizes = read_u32(bytes, 4)?;
        // Sanity cap. Real fonts have <50 strikes; the cap exists for
        // truncated / malformed inputs.
        if num_sizes == 0 || num_sizes > 256 {
            return Err(Error::BadStructure("CBLC: implausible numSizes"));
        }
        let needed = 8usize
            .checked_add(num_sizes as usize * BITMAP_SIZE_RECORD_LEN)
            .ok_or(Error::BadStructure("CBLC: numSizes overflow"))?;
        if bytes.len() < needed {
            return Err(Error::UnexpectedEof);
        }
        let mut strikes = Vec::with_capacity(num_sizes as usize);
        for i in 0..num_sizes as usize {
            let base = 8 + i * BITMAP_SIZE_RECORD_LEN;
            let index_subtable_list_offset = read_u32(bytes, base)?;
            let index_subtable_list_size = read_u32(bytes, base + 4)?;
            let number_of_index_subtables = read_u32(bytes, base + 8)?;
            // colorRef at base+12 ignored.
            // hori at base+16 (12 bytes) ignored.
            // vert at base+28 (12 bytes) ignored.
            let after_metrics = base + 16 + 2 * SBIT_LINE_METRICS_LEN;
            let start_glyph_index = read_u16(bytes, after_metrics)?;
            let end_glyph_index = read_u16(bytes, after_metrics + 2)?;
            let ppem_x = read_u8(bytes, after_metrics + 4)?;
            let ppem_y = read_u8(bytes, after_metrics + 5)?;
            let bit_depth = read_u8(bytes, after_metrics + 6)?;
            // flags at +7 ignored.
            // The IndexSubtableList must lie inside the table.
            if (index_subtable_list_offset as u64)
                .checked_add(index_subtable_list_size as u64)
                .map(|end| end > bytes.len() as u64)
                .unwrap_or(true)
            {
                return Err(Error::BadOffset);
            }
            strikes.push(StrikeRecord {
                index_subtable_list_offset,
                index_subtable_list_size,
                number_of_index_subtables,
                start_glyph_index,
                end_glyph_index,
                ppem_x,
                ppem_y,
                bit_depth,
            });
        }
        Ok(Self { bytes, strikes })
    }

    /// All ppem sizes carried by this CBLC, in declaration order. Useful
    /// for picking the strike closest to a target rasterisation size.
    pub fn ppem_sizes(&self) -> impl Iterator<Item = (u8, u8)> + '_ {
        self.strikes.iter().map(|s| (s.ppem_x, s.ppem_y))
    }

    /// Number of strikes in the table.
    pub fn num_strikes(&self) -> usize {
        self.strikes.len()
    }

    /// Find the strike whose `ppem_y` is closest to `target_ppem` and try
    /// to resolve `glyph_id`. If that strike doesn't carry the glyph, the
    /// next-closest strike is tried, and so on. Returns `None` only if no
    /// strike contains the glyph at all.
    ///
    /// The "closest" criterion picks the strike with the smallest absolute
    /// difference; in case of a tie the strike with the larger ppem wins
    /// (better quality after downscale than after upscale, per the CBDT
    /// spec's "Scaling behavior" recommendation).
    pub fn lookup_glyph(&self, glyph_id: u16, target_ppem: u8) -> Option<CblcEntry> {
        // Build a strike preference order: closest ppem first.
        let mut order: Vec<usize> = (0..self.strikes.len()).collect();
        order.sort_by_key(|&i| {
            let s = &self.strikes[i];
            let diff = (s.ppem_y as i32 - target_ppem as i32).abs();
            // Tie-break: prefer LARGER ppem (cheaper to downscale than
            // upscale). We negate ppem so that a smaller key is "better"
            // — the sort is stable + ascending so this keeps the larger-
            // ppem strike first when diff is equal.
            (diff, -(s.ppem_y as i32))
        });
        for i in order {
            if let Some(entry) = self.lookup_in_strike(i, glyph_id).ok().flatten() {
                return Some(entry);
            }
        }
        None
    }

    /// Resolve `glyph_id` against a specific strike. Returns `Ok(None)` if
    /// the strike doesn't carry the glyph; `Err(_)` only on structural
    /// damage (the rest of the lookup chain treats `Err` as "skip strike").
    fn lookup_in_strike(
        &self,
        strike_idx: usize,
        glyph_id: u16,
    ) -> Result<Option<CblcEntry>, Error> {
        let strike = &self.strikes[strike_idx];
        if glyph_id < strike.start_glyph_index || glyph_id > strike.end_glyph_index {
            return Ok(None);
        }
        let list_off = strike.index_subtable_list_offset as usize;
        // Walk the IndexSubtableRecord array (8 bytes each).
        let n = strike.number_of_index_subtables as usize;
        for i in 0..n {
            let rec_off = list_off + i * 8;
            let first = read_u16(self.bytes, rec_off)?;
            let last = read_u16(self.bytes, rec_off + 2)?;
            let sub_rel = read_u32(self.bytes, rec_off + 4)? as usize;
            if glyph_id < first || glyph_id > last {
                continue;
            }
            let sub_off = list_off + sub_rel;
            return self
                .resolve_in_subtable(strike, sub_off, first, last, glyph_id)
                .map(Some);
        }
        Ok(None)
    }

    fn resolve_in_subtable(
        &self,
        strike: &StrikeRecord,
        sub_off: usize,
        first_glyph: u16,
        _last_glyph: u16,
        glyph_id: u16,
    ) -> Result<CblcEntry, Error> {
        let index_format = read_u16(self.bytes, sub_off)?;
        let image_format = read_u16(self.bytes, sub_off + 2)?;
        let image_data_offset = read_u32(self.bytes, sub_off + 4)?;
        let header_size = 8usize;
        match index_format {
            1 => {
                // u32 offsets, contiguous range
                let local = (glyph_id - first_glyph) as usize;
                let off_a = read_u32(self.bytes, sub_off + header_size + local * 4)? as u64;
                let off_b = read_u32(self.bytes, sub_off + header_size + (local + 1) * 4)? as u64;
                let len = off_b
                    .checked_sub(off_a)
                    .ok_or(Error::BadStructure("CBLC: format 1 offsets reversed"))?;
                Ok(CblcEntry {
                    image_format,
                    image_data_offset: image_data_offset
                        .checked_add(off_a as u32)
                        .ok_or(Error::BadStructure("CBLC: format 1 offset overflow"))?,
                    data_len: len as u32,
                    ppem_x: strike.ppem_x,
                    ppem_y: strike.ppem_y,
                    bit_depth: strike.bit_depth,
                    fixed_metrics: None,
                })
            }
            2 => {
                // constant size + BigGlyphMetrics
                let image_size = read_u32(self.bytes, sub_off + header_size)?;
                let metrics = BigGlyphMetrics::parse(self.bytes, sub_off + header_size + 4)?;
                let local = (glyph_id - first_glyph) as u32;
                Ok(CblcEntry {
                    image_format,
                    image_data_offset: image_data_offset
                        .checked_add(local.saturating_mul(image_size))
                        .ok_or(Error::BadStructure("CBLC: format 2 offset overflow"))?,
                    data_len: image_size,
                    ppem_x: strike.ppem_x,
                    ppem_y: strike.ppem_y,
                    bit_depth: strike.bit_depth,
                    fixed_metrics: Some(metrics),
                })
            }
            3 => {
                // u16 offsets, contiguous range
                let local = (glyph_id - first_glyph) as usize;
                let off_a = read_u16(self.bytes, sub_off + header_size + local * 2)? as u32;
                let off_b = read_u16(self.bytes, sub_off + header_size + (local + 1) * 2)? as u32;
                let len = off_b
                    .checked_sub(off_a)
                    .ok_or(Error::BadStructure("CBLC: format 3 offsets reversed"))?;
                Ok(CblcEntry {
                    image_format,
                    image_data_offset: image_data_offset
                        .checked_add(off_a)
                        .ok_or(Error::BadStructure("CBLC: format 3 offset overflow"))?,
                    data_len: len,
                    ppem_x: strike.ppem_x,
                    ppem_y: strike.ppem_y,
                    bit_depth: strike.bit_depth,
                    fixed_metrics: None,
                })
            }
            4 => {
                // sparse: u32 numGlyphs + (u16 gid, u16 sbitOffset) pairs.
                let num_glyphs = read_u32(self.bytes, sub_off + header_size)?;
                let pairs_off = sub_off + header_size + 4;
                // Linear scan — the spec doesn't require sorted-by-gid, so
                // we play it safe. Real fonts are sorted; ~100s of entries
                // worst case, fine for occasional emoji lookups.
                let mut hit: Option<(u32, u32)> = None;
                let mut next_off: Option<u32> = None;
                for i in 0..num_glyphs as usize {
                    let pair_off = pairs_off + i * 4;
                    let gid = read_u16(self.bytes, pair_off)?;
                    let off = read_u16(self.bytes, pair_off + 2)? as u32;
                    if gid == glyph_id {
                        let next_pair = pairs_off + (i + 1) * 4;
                        let n_off = read_u16(self.bytes, next_pair + 2)? as u32;
                        hit = Some((off, n_off));
                        next_off = Some(n_off);
                        break;
                    }
                }
                let (off_a, off_b) = match (hit, next_off) {
                    (Some((a, b)), _) => (a, b),
                    _ => {
                        return Err(Error::BadStructure(
                            "CBLC: format 4 lookup missed (range covered but glyph absent)",
                        ))
                    }
                };
                let len = off_b
                    .checked_sub(off_a)
                    .ok_or(Error::BadStructure("CBLC: format 4 offsets reversed"))?;
                Ok(CblcEntry {
                    image_format,
                    image_data_offset: image_data_offset
                        .checked_add(off_a)
                        .ok_or(Error::BadStructure("CBLC: format 4 offset overflow"))?,
                    data_len: len,
                    ppem_x: strike.ppem_x,
                    ppem_y: strike.ppem_y,
                    bit_depth: strike.bit_depth,
                    fixed_metrics: None,
                })
            }
            5 => {
                // sparse + constant size: u32 imageSize + BigGlyphMetrics +
                // u32 numGlyphs + u16 glyphIdArray[numGlyphs].
                let image_size = read_u32(self.bytes, sub_off + header_size)?;
                let metrics = BigGlyphMetrics::parse(self.bytes, sub_off + header_size + 4)?;
                let num_glyphs = read_u32(self.bytes, sub_off + header_size + 12)?;
                let arr_off = sub_off + header_size + 16;
                for i in 0..num_glyphs as usize {
                    let gid = read_u16(self.bytes, arr_off + i * 2)?;
                    if gid == glyph_id {
                        let local = i as u32;
                        return Ok(CblcEntry {
                            image_format,
                            image_data_offset: image_data_offset
                                .checked_add(local.saturating_mul(image_size))
                                .ok_or(Error::BadStructure("CBLC: format 5 offset overflow"))?,
                            data_len: image_size,
                            ppem_x: strike.ppem_x,
                            ppem_y: strike.ppem_y,
                            bit_depth: strike.bit_depth,
                            fixed_metrics: Some(metrics),
                        });
                    }
                }
                Err(Error::BadStructure(
                    "CBLC: format 5 lookup missed (range covered but glyph absent)",
                ))
            }
            _ => Err(Error::BadStructure("CBLC: unsupported indexFormat")),
        }
        // covered first/last-glyph guarded the range upstream
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_short_header() {
        assert!(matches!(
            CblcTable::parse(&[0u8; 4]),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_unknown_major_version() {
        let mut bytes = vec![0u8; 8 + BITMAP_SIZE_RECORD_LEN];
        bytes[0..2].copy_from_slice(&7u16.to_be_bytes()); // major = 7
        bytes[4..8].copy_from_slice(&1u32.to_be_bytes()); // numSizes
        assert!(matches!(
            CblcTable::parse(&bytes),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn parses_minimal_cblc_with_format2_strike() {
        // Build:
        //   header (major=3, minor=0, numSizes=1)
        //   one BitmapSize at offset 8 (48 bytes), pointing the
        //   IndexSubtableList at offset 56.
        //   IndexSubtableList: one 8-byte IndexSubtableRecord (gid 5..7,
        //   subtableOffset = 8 — i.e. the IndexSubtable starts right
        //   after the record, at file offset 64).
        //   IndexSubtable Format 2: indexFormat=2 imageFormat=18 imageDataOffset=200,
        //   imageSize=120, BigGlyphMetrics(0..7).
        let strike_off = 56u32;
        let mut bytes = vec![0u8; 256];
        // Header.
        bytes[0..2].copy_from_slice(&3u16.to_be_bytes()); // major
        bytes[2..4].copy_from_slice(&0u16.to_be_bytes()); // minor
        bytes[4..8].copy_from_slice(&1u32.to_be_bytes()); // numSizes
                                                          // BitmapSize record at +8.
        let base = 8;
        bytes[base..base + 4].copy_from_slice(&strike_off.to_be_bytes()); // listOff
        bytes[base + 4..base + 8].copy_from_slice(&((8 + 8 + 16) as u32).to_be_bytes()); // listSize
        bytes[base + 8..base + 12].copy_from_slice(&1u32.to_be_bytes()); // numIndexSubtables
        bytes[base + 12..base + 16].copy_from_slice(&0u32.to_be_bytes()); // colorRef
                                                                          // skip 12+12 metrics zeroed.
        let after = base + 16 + 24;
        bytes[after..after + 2].copy_from_slice(&5u16.to_be_bytes()); // startGid
        bytes[after + 2..after + 4].copy_from_slice(&7u16.to_be_bytes()); // endGid
        bytes[after + 4] = 96; // ppemX
        bytes[after + 5] = 96; // ppemY
        bytes[after + 6] = 32; // bitDepth (color)
        bytes[after + 7] = 0x01; // flags

        // IndexSubtableList at offset 56.
        let list_off = strike_off as usize;
        bytes[list_off..list_off + 2].copy_from_slice(&5u16.to_be_bytes()); // first
        bytes[list_off + 2..list_off + 4].copy_from_slice(&7u16.to_be_bytes()); // last
        bytes[list_off + 4..list_off + 8].copy_from_slice(&8u32.to_be_bytes()); // subtable rel off

        // IndexSubtable Format 2 at offset 56 + 8 = 64.
        let sub_off = list_off + 8;
        bytes[sub_off..sub_off + 2].copy_from_slice(&2u16.to_be_bytes()); // indexFormat
        bytes[sub_off + 2..sub_off + 4].copy_from_slice(&18u16.to_be_bytes()); // imageFormat
        bytes[sub_off + 4..sub_off + 8].copy_from_slice(&200u32.to_be_bytes()); // imageDataOffset
        bytes[sub_off + 8..sub_off + 12].copy_from_slice(&120u32.to_be_bytes()); // imageSize
        for i in 0..8u8 {
            bytes[sub_off + 12 + i as usize] = i;
        }

        let cblc = CblcTable::parse(&bytes).expect("parse");
        assert_eq!(cblc.num_strikes(), 1);
        let entry = cblc.lookup_glyph(5, 96).expect("entry");
        assert_eq!(entry.image_format, 18);
        assert_eq!(entry.image_data_offset, 200);
        assert_eq!(entry.data_len, 120);
        assert_eq!(entry.ppem_x, 96);
        assert_eq!(entry.ppem_y, 96);
        let m = entry.fixed_metrics.expect("constant metrics");
        assert_eq!(m.height, 0);
        assert_eq!(m.width, 1);

        // Glyph 6 — second slot, same strike.
        let entry6 = cblc.lookup_glyph(6, 96).expect("entry6");
        assert_eq!(entry6.image_data_offset, 200 + 120);
        assert_eq!(entry6.data_len, 120);

        // Out-of-range glyph (gid 8) → None.
        assert!(cblc.lookup_glyph(8, 96).is_none());
    }
}
