//! `CBDT` — Color Bitmap Data Table.
//!
//! The CBDT table holds the actual per-glyph image data; the `CBLC`
//! sibling table maps `(glyph_id, ppem)` → byte range here. Per
//! Microsoft OpenType spec, the table starts with a 4-byte header
//! (`majorVersion = 3`, `minorVersion = 0`) followed by an opaque blob
//! of glyph entries whose layout depends on the per-strike `imageFormat`
//! recorded in CBLC.
//!
//! We support the three PNG-based entry formats Noto Color Emoji and
//! every other Google "embedded PNG" colour-emoji font use:
//!
//! ```text
//! Format 17:  SmallGlyphMetrics (5 B) + u32 dataLen + PNG[dataLen]
//! Format 18:  BigGlyphMetrics   (8 B) + u32 dataLen + PNG[dataLen]
//! Format 19:  u32 dataLen + PNG[dataLen]
//!             (metrics live in CBLC IndexSubtable's BigGlyphMetrics)
//! ```
//!
//! Format 19's metrics are recovered via `CblcEntry::fixed_metrics`
//! which the CBLC walker populates from the IndexSubtable Format 2 / 5
//! BigGlyphMetrics field.
//!
//! Other CBDT formats (1/2/5/6/7/8/9 inherited from EBDT — monochrome
//! / grayscale; 32 BGRA uncompressed) are not supported in this round.
//! They're rare in the wild compared to PNG and the consumer crate
//! doesn't have a native BGRA path yet.

use crate::parser::read_u32;
use crate::tables::cblc::{BigGlyphMetrics, CblcEntry, SmallGlyphMetrics};
use crate::Error;

/// One glyph's worth of data resolved out of CBDT, ready for PNG decode.
#[derive(Debug, Clone, Copy)]
pub struct ColorBitmap<'a> {
    /// Width of the bitmap in pixels (from the per-glyph metrics).
    pub width: u8,
    /// Height in pixels.
    pub height: u8,
    /// Distance from the horizontal pen origin to the LEFT edge of the
    /// bitmap, in pixels.
    pub bearing_x: i8,
    /// Distance from the horizontal pen origin to the TOP edge of the
    /// bitmap, in pixels.
    pub bearing_y: i8,
    /// Horizontal advance in pixels.
    pub advance: u8,
    /// Strike pixels-per-em (the size at which this glyph was authored).
    pub ppem: u8,
    /// Raw PNG byte stream — pass to `oxideav_png::decode_png_to_frame`
    /// in the consumer crate. Borrowed from the parent CBDT slice.
    pub png_bytes: &'a [u8],
}

/// Parsed CBDT table.
#[derive(Debug, Clone)]
pub struct CbdtTable<'a> {
    bytes: &'a [u8],
}

impl<'a> CbdtTable<'a> {
    /// Wrap a CBDT byte slice. Validates the header version only — per-
    /// glyph entry layout is parsed lazily via `lookup`.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < 4 {
            return Err(Error::UnexpectedEof);
        }
        let major = u16::from_be_bytes([bytes[0], bytes[1]]);
        // CBDT = 3, EBDT = 2. We only emit `ColorBitmap` entries
        // (Formats 17-19 are CBDT-only) but we accept either header
        // since EBDT's bytes are layout-compatible for everything we
        // touch (zero, in practice).
        if major != 2 && major != 3 {
            return Err(Error::BadStructure("CBDT: unknown major version"));
        }
        Ok(Self { bytes })
    }

    /// Decode a per-glyph entry given the CBLC-resolved descriptor.
    ///
    /// Returns `None` if `entry.image_format` is not one of 17/18/19
    /// (we don't support uncompressed BGRA or monochrome formats yet).
    /// Returns `Err(_)` only on structural damage (truncated PNG range).
    pub fn lookup(&self, entry: &CblcEntry) -> Result<Option<ColorBitmap<'a>>, Error> {
        let off = entry.image_data_offset as usize;
        let end = off
            .checked_add(entry.data_len as usize)
            .ok_or(Error::BadStructure("CBDT: entry overflow"))?;
        if end > self.bytes.len() {
            return Err(Error::BadOffset);
        }
        let blob = &self.bytes[off..end];
        match entry.image_format {
            17 => {
                // SmallGlyphMetrics (5) + u32 dataLen + PNG.
                let metrics = SmallGlyphMetrics::parse(blob, 0)?;
                if blob.len() < 5 + 4 {
                    return Err(Error::UnexpectedEof);
                }
                let data_len = read_u32(blob, 5)? as usize;
                let png = blob.get(9..9 + data_len).ok_or(Error::BadOffset)?;
                Ok(Some(ColorBitmap {
                    width: metrics.width,
                    height: metrics.height,
                    bearing_x: metrics.bearing_x,
                    bearing_y: metrics.bearing_y,
                    advance: metrics.advance,
                    ppem: entry.ppem_y,
                    png_bytes: png,
                }))
            }
            18 => {
                // BigGlyphMetrics (8) + u32 dataLen + PNG.
                let metrics = BigGlyphMetrics::parse(blob, 0)?;
                if blob.len() < 8 + 4 {
                    return Err(Error::UnexpectedEof);
                }
                let data_len = read_u32(blob, 8)? as usize;
                let png = blob.get(12..12 + data_len).ok_or(Error::BadOffset)?;
                Ok(Some(ColorBitmap {
                    width: metrics.width,
                    height: metrics.height,
                    bearing_x: metrics.hori_bearing_x,
                    bearing_y: metrics.hori_bearing_y,
                    advance: metrics.hori_advance,
                    ppem: entry.ppem_y,
                    png_bytes: png,
                }))
            }
            19 => {
                // u32 dataLen + PNG. Metrics come from CBLC.
                let metrics = entry.fixed_metrics.ok_or(Error::BadStructure(
                    "CBDT format 19 needs CBLC fixed metrics",
                ))?;
                if blob.len() < 4 {
                    return Err(Error::UnexpectedEof);
                }
                let data_len = read_u32(blob, 0)? as usize;
                let png = blob.get(4..4 + data_len).ok_or(Error::BadOffset)?;
                Ok(Some(ColorBitmap {
                    width: metrics.width,
                    height: metrics.height,
                    bearing_x: metrics.hori_bearing_x,
                    bearing_y: metrics.hori_bearing_y,
                    advance: metrics.hori_advance,
                    ppem: entry.ppem_y,
                    png_bytes: png,
                }))
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_format17_entry() {
        // Build a CBDT byte slice with a single Format 17 entry at
        // offset 16:
        //   [00..04] header (major=3, minor=0)
        //   [04..16] padding zeros
        //   [16..21] SmallGlyphMetrics (h=10, w=12, bx=-1, by=8, adv=14)
        //   [21..25] dataLen = 5
        //   [25..30] PNG bytes (fake) [0x89, 'P', 'N', 'G', 0x0D]
        let mut bytes = vec![0u8; 64];
        bytes[0..2].copy_from_slice(&3u16.to_be_bytes());
        bytes[16] = 10; // height
        bytes[17] = 12; // width
        bytes[18] = (-1i8) as u8; // bearingX
        bytes[19] = 8; // bearingY
        bytes[20] = 14; // advance
        bytes[21..25].copy_from_slice(&5u32.to_be_bytes()); // dataLen
        bytes[25..30].copy_from_slice(&[0x89, b'P', b'N', b'G', 0x0D]);
        let cbdt = CbdtTable::parse(&bytes).expect("parse");
        let entry = CblcEntry {
            image_format: 17,
            image_data_offset: 16,
            data_len: 30 - 16,
            ppem_x: 96,
            ppem_y: 96,
            bit_depth: 32,
            fixed_metrics: None,
        };
        let cb = cbdt.lookup(&entry).expect("lookup ok").expect("entry");
        assert_eq!(cb.width, 12);
        assert_eq!(cb.height, 10);
        assert_eq!(cb.bearing_x, -1);
        assert_eq!(cb.bearing_y, 8);
        assert_eq!(cb.advance, 14);
        assert_eq!(cb.png_bytes.len(), 5);
        assert_eq!(cb.png_bytes[0], 0x89);
    }

    #[test]
    fn parses_format18_entry() {
        // BigGlyphMetrics is 8 bytes: h, w, hbx, hby, hadv, vbx, vby, vadv.
        let mut bytes = vec![0u8; 64];
        bytes[0..2].copy_from_slice(&3u16.to_be_bytes());
        // Entry at offset 12.
        bytes[12] = 20; // height
        bytes[13] = 24; // width
        bytes[14] = 2; // hori_bearing_x
        bytes[15] = 18; // hori_bearing_y
        bytes[16] = 26; // hori_advance
        bytes[17..20].copy_from_slice(&[0; 3]); // vert metrics ignored
        bytes[20..24].copy_from_slice(&3u32.to_be_bytes()); // dataLen
        bytes[24..27].copy_from_slice(&[0xA1, 0xB2, 0xC3]);
        let cbdt = CbdtTable::parse(&bytes).expect("parse");
        let entry = CblcEntry {
            image_format: 18,
            image_data_offset: 12,
            data_len: 27 - 12,
            ppem_x: 109,
            ppem_y: 109,
            bit_depth: 32,
            fixed_metrics: None,
        };
        let cb = cbdt.lookup(&entry).expect("lookup ok").expect("entry");
        assert_eq!(cb.width, 24);
        assert_eq!(cb.height, 20);
        assert_eq!(cb.bearing_x, 2);
        assert_eq!(cb.bearing_y, 18);
        assert_eq!(cb.advance, 26);
        assert_eq!(cb.ppem, 109);
        assert_eq!(cb.png_bytes, &[0xA1, 0xB2, 0xC3]);
    }

    #[test]
    fn returns_none_for_unsupported_format() {
        let mut bytes = vec![0u8; 16];
        bytes[0..2].copy_from_slice(&3u16.to_be_bytes());
        let cbdt = CbdtTable::parse(&bytes).expect("parse");
        let entry = CblcEntry {
            image_format: 1, // monochrome — not supported
            image_data_offset: 4,
            data_len: 4,
            ppem_x: 32,
            ppem_y: 32,
            bit_depth: 1,
            fixed_metrics: None,
        };
        assert!(cbdt.lookup(&entry).expect("lookup ok").is_none());
    }

    #[test]
    fn parses_format19_with_cblc_metrics() {
        let mut bytes = vec![0u8; 32];
        bytes[0..2].copy_from_slice(&3u16.to_be_bytes());
        // Entry at offset 8: just dataLen + PNG.
        bytes[8..12].copy_from_slice(&4u32.to_be_bytes());
        bytes[12..16].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let cbdt = CbdtTable::parse(&bytes).expect("parse");
        let entry = CblcEntry {
            image_format: 19,
            image_data_offset: 8,
            data_len: 16 - 8,
            ppem_x: 64,
            ppem_y: 64,
            bit_depth: 32,
            fixed_metrics: Some(BigGlyphMetrics {
                height: 7,
                width: 9,
                hori_bearing_x: 3,
                hori_bearing_y: 11,
                hori_advance: 13,
                vert_bearing_x: 0,
                vert_bearing_y: 0,
                vert_advance: 0,
            }),
        };
        let cb = cbdt.lookup(&entry).expect("lookup ok").expect("entry");
        assert_eq!(cb.width, 9);
        assert_eq!(cb.height, 7);
        assert_eq!(cb.bearing_x, 3);
        assert_eq!(cb.bearing_y, 11);
        assert_eq!(cb.advance, 13);
        assert_eq!(cb.png_bytes, &[0xDE, 0xAD, 0xBE, 0xEF]);
    }
}
