//! `EBDT` — Embedded Bitmap Data Table (ISO/IEC 14496-22:2019 §5.6.2).
//!
//! The `EBDT` table holds the actual per-glyph monochrome / grayscale
//! bitmap data; the [`EBLC`](crate::tables::cblc) sibling table (which is
//! the *same on-wire layout* as `CBLC` — `CBLC`/`CBDT` are the colour
//! superset of `EBLC`/`EBDT`, §5.6.5) maps `(glyph_id, ppem)` → byte
//! range here. The shared [`CblcTable`](crate::tables::cblc::CblcTable)
//! parser already accepts the `majorVersion == 2` `EBLC` header, so this
//! module only has to decode the §5.6.2.2 *image-data* formats that
//! `CBDT` does not cover — the bit-packed grayscale ones.
//!
//! The §5.6.2.2 glyph-bitmap-data formats and our coverage:
//!
//! ```text
//! Format 1:  SmallGlyphMetrics (5 B) + byte-aligned image data   ✓ decoded
//! Format 2:  SmallGlyphMetrics (5 B) + bit-aligned image data    ✓ decoded
//! Format 3:  obsolete — "not supported in OFF"                   n/a
//! Format 4:  metrics in EBLC, compressed (Mac East-Asian)        ✗ None
//! Format 5:  bit-aligned image data only (metrics in EBLC)       ✓ decoded
//! Format 6:  BigGlyphMetrics (8 B) + byte-aligned image data     ✓ decoded
//! Format 7:  BigGlyphMetrics (8 B) + bit-aligned image data      ✓ decoded
//! Format 8:  small metrics + component records (composite)       ✗ None
//! Format 9:  big metrics + component records (composite)         ✗ None
//! ```
//!
//! §5.6.2.2 prescribes the pixel layout: "the bitmap data begins with the
//! most significant bit of the first byte corresponding to the top-left
//! pixel of the bounding box, proceeding through succeeding bits moving
//! left to right". When `bitDepth > 1` "all of a pixel's bits are stored
//! consecutively in memory, and all of a row's pixels are stored
//! consecutively". For the *byte-aligned* formats (1 and 6) "the data for
//! each row is padded to a byte boundary, so the next row begins with the
//! most significant bit of a new byte". For the *bit-aligned* formats (2,
//! 5, 7) "the data for a new row will begin with the bit immediately
//! following the last bit of the previous row" — only the *glyph* is
//! byte-aligned, not each row.
//!
//! We unpack into a `width × height` grid of one byte per pixel, scaling
//! the `bitDepth`-bit sample up to the full 0..=255 range (a `bitDepth=1`
//! "1 bit = black" sample becomes `0xFF`, "0 = white" becomes `0x00`,
//! matching the §5.6.2.2.1 "1 bits correspond to black, and 0 bits to
//! white" convention but flipped to alpha-coverage so the caller can blit
//! the byte as an alpha mask). `bitDepth` of 1 / 2 / 4 / 8 is honoured per
//! the §5.6.3.1 "Bit Depth" table; `bitDepth == 32` (BGRA colour) is the
//! `CBDT` path and is rejected here as out of scope.

use crate::tables::cblc::{BigGlyphMetrics, CblcEntry, SmallGlyphMetrics};
use crate::Error;

/// One glyph's worth of unpacked grayscale-coverage bitmap data resolved
/// out of `EBDT`.
///
/// `pixels` is `width * height` bytes, row-major, top row first, each byte
/// the glyph's *alpha coverage* at that pixel (`0x00` = transparent/white,
/// `0xFF` = opaque/black). The caller composites it at `(bearing_x,
/// bearing_y)` relative to the pen origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrayBitmap {
    /// Width of the bitmap in pixels (from the per-glyph metrics).
    pub width: u8,
    /// Height in pixels.
    pub height: u8,
    /// Distance from the horizontal pen origin to the LEFT edge of the
    /// bitmap, in pixels (`horiBearingX` / `bearingX`).
    pub bearing_x: i8,
    /// Distance from the horizontal pen origin to the TOP edge of the
    /// bitmap, in pixels (`horiBearingY` / `bearingY`).
    pub bearing_y: i8,
    /// Horizontal advance in pixels (`horiAdvance` / `advance`).
    pub advance: u8,
    /// Strike pixels-per-em on the Y axis (the size this glyph was
    /// authored at).
    pub ppem: u8,
    /// Source bit depth (1 / 2 / 4 / 8) before expansion to one byte per
    /// pixel. Surfaced for callers that want to know the original
    /// gray-level granularity.
    pub bit_depth: u8,
    /// `width * height` bytes of alpha coverage, row-major, top-left
    /// first.
    pub pixels: Vec<u8>,
}

/// One `EbdtComponent` record (§5.6.2.2 "EbdtComponent Record") — a
/// reference, by glyph ID, to another glyph whose bitmap is copied into a
/// composite (format 8 / 9) glyph at `(x_offset, y_offset)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EbdtComponent {
    /// Component glyph ID. Its bitmap is looked up in the *same* `EBLC`
    /// strike as the composite. Nested composites are allowed (a component
    /// may itself be a format-8/9 glyph).
    pub glyph_id: u16,
    /// Position of the component's left edge, in pixels, relative to the
    /// composite's bitmap origin (§5.6.2.2: "Position of component left").
    pub x_offset: i8,
    /// Position of the component's top edge, in pixels (§5.6.2.2:
    /// "Position of component top").
    pub y_offset: i8,
}

/// The decoded *descriptor* of a composite (format 8 / 9) `EBDT` glyph —
/// the composite's own bounding metrics plus the list of component glyphs
/// to assemble. It carries no pixels itself; the [`Font`] layer resolves
/// each [`EbdtComponent`]'s `GrayBitmap` and blits them onto a canvas.
///
/// [`Font`]: crate::Font
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositeBitmap {
    /// Composite bounding-box width in pixels (from the composite's own
    /// Small/BigGlyphMetrics).
    pub width: u8,
    /// Composite bounding-box height in pixels.
    pub height: u8,
    /// Horizontal pen origin → left edge of the composite bitmap, pixels.
    pub bearing_x: i8,
    /// Horizontal pen origin → top edge of the composite bitmap, pixels.
    pub bearing_y: i8,
    /// Horizontal advance of the composite, pixels.
    pub advance: u8,
    /// Strike ppem on the Y axis.
    pub ppem: u8,
    /// Bit depth of the strike (1 / 2 / 4 / 8); shared by every component.
    pub bit_depth: u8,
    /// The component glyphs to assemble, in declaration order. The spec
    /// does not prescribe a paint order; later components in the array are
    /// drawn over earlier ones (back-to-front) so the array order is
    /// preserved.
    pub components: Vec<EbdtComponent>,
}

/// Parsed `EBDT` table.
#[derive(Debug, Clone)]
pub struct EbdtTable<'a> {
    bytes: &'a [u8],
}

impl<'a> EbdtTable<'a> {
    /// Wrap an `EBDT` byte slice. Validates the header version only — the
    /// per-glyph entry layout is parsed lazily via [`EbdtTable::lookup`].
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < 4 {
            return Err(Error::UnexpectedEof);
        }
        let major = u16::from_be_bytes([bytes[0], bytes[1]]);
        // EBDT = 2 per §5.6.2.1. We also accept 3 (CBDT) since the
        // grayscale image-data formats are byte-identical — a CBDT may
        // ship a monochrome strike alongside its colour ones.
        if major != 2 && major != 3 {
            return Err(Error::BadStructure("EBDT: unknown major version"));
        }
        Ok(Self { bytes })
    }

    /// Decode a per-glyph monochrome / grayscale entry given the
    /// EBLC-resolved descriptor and the strike's `bitDepth`
    /// ([`CblcEntry::bit_depth`]).
    ///
    /// Returns `Ok(None)` when `entry.image_format` is one we do not
    /// decode (4 compressed, 8 / 9 composite) or when `bit_depth == 32`
    /// (that is the colour `CBDT` path). Returns `Err(_)` only on
    /// structural damage (truncated image-data range).
    pub fn lookup(&self, entry: &CblcEntry) -> Result<Option<GrayBitmap>, Error> {
        // bitDepth 32 = BGRA colour → not our table.
        if entry.bit_depth == 32 {
            return Ok(None);
        }
        if !matches!(entry.bit_depth, 1 | 2 | 4 | 8) {
            return Err(Error::BadStructure("EBDT: unsupported bitDepth"));
        }
        let off = entry.image_data_offset as usize;
        let end = off
            .checked_add(entry.data_len as usize)
            .ok_or(Error::BadStructure("EBDT: entry overflow"))?;
        if end > self.bytes.len() {
            return Err(Error::BadOffset);
        }
        let blob = &self.bytes[off..end];
        match entry.image_format {
            1 => self.decode_small(blob, entry, /* byte_aligned */ true),
            2 => self.decode_small(blob, entry, /* byte_aligned */ false),
            5 => self.decode_format5(blob, entry),
            6 => self.decode_big(blob, entry, /* byte_aligned */ true),
            7 => self.decode_big(blob, entry, /* byte_aligned */ false),
            // 4 = compressed, 8 / 9 = composite: not decoded here.
            _ => Ok(None),
        }
    }

    /// Formats 1 (byte-aligned) and 2 (bit-aligned): SmallGlyphMetrics
    /// followed by image data.
    fn decode_small(
        &self,
        blob: &[u8],
        entry: &CblcEntry,
        byte_aligned: bool,
    ) -> Result<Option<GrayBitmap>, Error> {
        let m = SmallGlyphMetrics::parse(blob, 0)?;
        let pixels = unpack_image(
            blob.get(5..).ok_or(Error::UnexpectedEof)?,
            m.width,
            m.height,
            entry.bit_depth,
            byte_aligned,
        )?;
        Ok(Some(GrayBitmap {
            width: m.width,
            height: m.height,
            bearing_x: m.bearing_x,
            bearing_y: m.bearing_y,
            advance: m.advance,
            ppem: entry.ppem_y,
            bit_depth: entry.bit_depth,
            pixels,
        }))
    }

    /// Formats 6 (byte-aligned) and 7 (bit-aligned): BigGlyphMetrics
    /// followed by image data.
    fn decode_big(
        &self,
        blob: &[u8],
        entry: &CblcEntry,
        byte_aligned: bool,
    ) -> Result<Option<GrayBitmap>, Error> {
        let m = BigGlyphMetrics::parse(blob, 0)?;
        let pixels = unpack_image(
            blob.get(8..).ok_or(Error::UnexpectedEof)?,
            m.width,
            m.height,
            entry.bit_depth,
            byte_aligned,
        )?;
        Ok(Some(GrayBitmap {
            width: m.width,
            height: m.height,
            bearing_x: m.hori_bearing_x,
            bearing_y: m.hori_bearing_y,
            advance: m.hori_advance,
            ppem: entry.ppem_y,
            bit_depth: entry.bit_depth,
            pixels,
        }))
    }

    /// Decode a composite (component-data) entry — §5.6.2.2.8 format 8
    /// (small metrics) and §5.6.2.2.9 format 9 (big metrics). Unlike the
    /// pixel formats these carry **no** imagery of their own: the glyph is
    /// assembled from copies of other glyphs' bitmaps positioned at
    /// per-component `(xOffset, yOffset)` offsets. This method only decodes
    /// the *descriptor* (composite metrics + the [`EbdtComponent`] array);
    /// resolving each component's pixels and compositing them onto a canvas
    /// needs the `EBLC` strike, so that recursion lives at the [`Font`]
    /// layer (`crate::Font::glyph_gray_bitmap`).
    ///
    /// Returns `Ok(None)` for non-composite formats (so a caller can probe
    /// every entry uniformly); `Err(_)` only on structural damage.
    ///
    /// [`Font`]: crate::Font
    pub fn lookup_composite(&self, entry: &CblcEntry) -> Result<Option<CompositeBitmap>, Error> {
        let off = entry.image_data_offset as usize;
        let end = off
            .checked_add(entry.data_len as usize)
            .ok_or(Error::BadStructure("EBDT: composite entry overflow"))?;
        if end > self.bytes.len() {
            return Err(Error::BadOffset);
        }
        let blob = &self.bytes[off..end];
        match entry.image_format {
            8 => self.decode_composite(blob, entry, /* big_metrics */ false),
            9 => self.decode_composite(blob, entry, /* big_metrics */ true),
            _ => Ok(None),
        }
    }

    /// Shared body for formats 8 (small metrics + 1-byte pad) and 9 (big
    /// metrics, no pad). Layout per §5.6.2.2.8 / §5.6.2.2.9:
    ///
    /// ```text
    /// Format 8: SmallGlyphMetrics(5) | uint8 pad | uint16 numComponents | EbdtComponent[n]
    /// Format 9: BigGlyphMetrics(8)              | uint16 numComponents | EbdtComponent[n]
    /// ```
    ///
    /// Each `EbdtComponent` is `uint16 glyphID | int8 xOffset | int8 yOffset`
    /// (4 bytes), `xOffset`/`yOffset` giving the top-left placement of the
    /// component in the composite.
    fn decode_composite(
        &self,
        blob: &[u8],
        entry: &CblcEntry,
        big_metrics: bool,
    ) -> Result<Option<CompositeBitmap>, Error> {
        // Decode the composite's own bounding metrics, and find where the
        // numComponents field begins.
        let (width, height, bearing_x, bearing_y, advance, n_off) = if big_metrics {
            let m = BigGlyphMetrics::parse(blob, 0)?;
            (
                m.width,
                m.height,
                m.hori_bearing_x,
                m.hori_bearing_y,
                m.hori_advance,
                8usize,
            )
        } else {
            let m = SmallGlyphMetrics::parse(blob, 0)?;
            // Format 8 has a 1-byte pad "to short boundary" before the count.
            (
                m.width,
                m.height,
                m.bearing_x,
                m.bearing_y,
                m.advance,
                6usize,
            )
        };
        let num_components = blob
            .get(n_off..n_off + 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
            .ok_or(Error::UnexpectedEof)? as usize;
        let arr_off = n_off + 2;
        // 4 bytes per EbdtComponent record.
        let needed = arr_off
            .checked_add(num_components.checked_mul(4).ok_or(Error::BadStructure(
                "EBDT composite: component count overflow",
            ))?)
            .ok_or(Error::BadStructure("EBDT composite: array overflow"))?;
        if blob.len() < needed {
            return Err(Error::UnexpectedEof);
        }
        let mut components = Vec::with_capacity(num_components);
        for i in 0..num_components {
            let base = arr_off + i * 4;
            components.push(EbdtComponent {
                glyph_id: u16::from_be_bytes([blob[base], blob[base + 1]]),
                x_offset: blob[base + 2] as i8,
                y_offset: blob[base + 3] as i8,
            });
        }
        Ok(Some(CompositeBitmap {
            width,
            height,
            bearing_x,
            bearing_y,
            advance,
            ppem: entry.ppem_y,
            bit_depth: entry.bit_depth,
            components,
        }))
    }

    /// Format 5: bit-aligned image data only, no inline metrics. §5.6.2.2.5
    /// says this is for use with EBLC IndexSubTable format 2 or 5, which
    /// hold the constant metrics for the whole range — surfaced here via
    /// [`CblcEntry::fixed_metrics`].
    fn decode_format5(&self, blob: &[u8], entry: &CblcEntry) -> Result<Option<GrayBitmap>, Error> {
        let m = entry.fixed_metrics.ok_or(Error::BadStructure(
            "EBDT format 5 needs EBLC fixed metrics (IndexSubTable 2/5)",
        ))?;
        let pixels = unpack_image(
            blob,
            m.width,
            m.height,
            entry.bit_depth,
            /* byte_aligned */ false,
        )?;
        Ok(Some(GrayBitmap {
            width: m.width,
            height: m.height,
            bearing_x: m.hori_bearing_x,
            bearing_y: m.hori_bearing_y,
            advance: m.hori_advance,
            ppem: entry.ppem_y,
            bit_depth: entry.bit_depth,
            pixels,
        }))
    }
}

/// Unpack `width × height` pixels of `bit_depth`-bit samples (MSB first,
/// left-to-right, top-to-bottom) into a `width * height`-byte grid of
/// alpha coverage scaled to the full 0..=255 range.
///
/// `byte_aligned == true`: each row starts on a fresh byte (formats 1 / 6).
/// `byte_aligned == false`: rows pack contiguously bit-for-bit, only the
/// glyph as a whole is byte-aligned (formats 2 / 5 / 7).
fn unpack_image(
    data: &[u8],
    width: u8,
    height: u8,
    bit_depth: u8,
    byte_aligned: bool,
) -> Result<Vec<u8>, Error> {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 {
        return Ok(Vec::new());
    }
    let depth = bit_depth as usize;
    // Size validation: how many input bytes does this glyph need?
    let row_bits = w * depth;
    let needed = if byte_aligned {
        // Each row padded up to a byte boundary.
        row_bits.div_ceil(8) * h
    } else {
        // Whole glyph bit-packed, then padded to a byte at the very end.
        (row_bits * h).div_ceil(8)
    };
    if data.len() < needed {
        return Err(Error::UnexpectedEof);
    }
    let mut out = Vec::with_capacity(w * h);
    // Maximum sample value for this bit depth (1 → 1, 2 → 3, 4 → 15,
    // 8 → 255), used to scale the sample up to 0..=255.
    let max_sample = (1u32 << depth) - 1;
    let mut bit_cursor = 0usize; // absolute bit position (bit-aligned mode)
    for _row in 0..h {
        if byte_aligned {
            // Restart on a byte boundary at the top of each row.
            bit_cursor = bit_cursor.div_ceil(8) * 8;
        }
        for _col in 0..w {
            let sample = read_bits(data, bit_cursor, depth);
            bit_cursor += depth;
            // Scale sample to 0..=255: alpha = sample * 255 / max_sample.
            // For depth 8 this is the identity; for depth 1 it maps
            // {0,1} → {0,255}.
            let alpha = (sample * 255 / max_sample) as u8;
            out.push(alpha);
        }
    }
    Ok(out)
}

/// Read `count` (≤ 8) bits starting at absolute bit offset `bit_off`,
/// MSB-first within each byte, returning them right-aligned in a `u32`.
/// The caller guarantees the range is in bounds (validated in
/// [`unpack_image`]).
fn read_bits(data: &[u8], bit_off: usize, count: usize) -> u32 {
    let mut value = 0u32;
    for i in 0..count {
        let abs = bit_off + i;
        let byte = data[abs / 8];
        let bit = (byte >> (7 - (abs % 8))) & 1;
        value = (value << 1) | bit as u32;
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_entry(format: u16, off: u32, len: u32, bit_depth: u8) -> CblcEntry {
        CblcEntry {
            image_format: format,
            image_data_offset: off,
            data_len: len,
            ppem_x: 16,
            ppem_y: 16,
            bit_depth,
            fixed_metrics: None,
        }
    }

    #[test]
    fn rejects_short_header() {
        assert!(matches!(
            EbdtTable::parse(&[0u8; 2]),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_unknown_major() {
        let mut b = vec![0u8; 4];
        b[0..2].copy_from_slice(&9u16.to_be_bytes());
        assert!(matches!(EbdtTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn read_bits_msb_first() {
        // 0b1010_0110 = 0xA6
        let data = [0xA6u8];
        assert_eq!(read_bits(&data, 0, 1), 1);
        assert_eq!(read_bits(&data, 1, 1), 0);
        assert_eq!(read_bits(&data, 0, 4), 0b1010);
        assert_eq!(read_bits(&data, 4, 4), 0b0110);
        assert_eq!(read_bits(&data, 0, 8), 0xA6);
    }

    #[test]
    fn format1_monochrome_byte_aligned() {
        // 3×2, bitDepth 1, byte-aligned. Each row = 3 bits padded to 1
        // byte. Row 0 = 101 -> 0b1010_0000 = 0xA0. Row 1 = 011 ->
        // 0b0110_0000 = 0x60.
        let mut bytes = vec![0u8; 4]; // header
        bytes[0..2].copy_from_slice(&2u16.to_be_bytes());
        // SmallGlyphMetrics: h=2, w=3, bx=1, by=2, adv=4.
        let entry_off = bytes.len();
        bytes.extend_from_slice(&[2, 3, 1, 2, 4]);
        bytes.push(0xA0); // row 0
        bytes.push(0x60); // row 1
        let total = (bytes.len() - entry_off) as u32;
        let t = EbdtTable::parse(&bytes).unwrap();
        let e = small_entry(1, entry_off as u32, total, 1);
        let g = t.lookup(&e).unwrap().unwrap();
        assert_eq!((g.width, g.height), (3, 2));
        assert_eq!((g.bearing_x, g.bearing_y, g.advance), (1, 2, 4));
        // Row 0 = 1,0,1 -> 255,0,255. Row 1 = 0,1,1 -> 0,255,255.
        assert_eq!(g.pixels, vec![255, 0, 255, 0, 255, 255]);
    }

    #[test]
    fn format2_monochrome_bit_aligned() {
        // 3×2, bitDepth 1, bit-aligned. 6 bits total = 101011 ->
        // 0b1010_1100 = 0xAC (padded with two zero bits to a byte).
        let mut bytes = vec![0u8; 4];
        bytes[0..2].copy_from_slice(&2u16.to_be_bytes());
        let entry_off = bytes.len();
        bytes.extend_from_slice(&[2, 3, 0, 0, 0]); // metrics h=2,w=3
        bytes.push(0xAC);
        let total = (bytes.len() - entry_off) as u32;
        let t = EbdtTable::parse(&bytes).unwrap();
        let e = small_entry(2, entry_off as u32, total, 1);
        let g = t.lookup(&e).unwrap().unwrap();
        // Row 0 = 1,0,1; Row 1 = 0,1,1 (bits flow continuously).
        assert_eq!(g.pixels, vec![255, 0, 255, 0, 255, 255]);
    }

    #[test]
    fn format1_vs_format2_differ_on_padding() {
        // 5×2 bitDepth 1. Byte-aligned needs 2 bytes (1 per row);
        // bit-aligned needs 10 bits = 2 bytes but rows flow.
        // Pattern row0 = 11111, row1 = 00000.
        // Byte-aligned: 0xF8, 0x00.
        let mut b1 = vec![0u8; 4];
        b1[0..2].copy_from_slice(&2u16.to_be_bytes());
        let off = b1.len();
        b1.extend_from_slice(&[2, 5, 0, 0, 0]);
        b1.extend_from_slice(&[0xF8, 0x00]);
        let t1 = EbdtTable::parse(&b1).unwrap();
        let g1 = t1
            .lookup(&small_entry(1, off as u32, (b1.len() - off) as u32, 1))
            .unwrap()
            .unwrap();
        assert_eq!(g1.pixels, vec![255, 255, 255, 255, 255, 0, 0, 0, 0, 0]);
        // Bit-aligned: 11111 00000 = 0b1111_1000 0b00.. = 0xF8, 0x00.
        let mut b2 = vec![0u8; 4];
        b2[0..2].copy_from_slice(&2u16.to_be_bytes());
        let off2 = b2.len();
        b2.extend_from_slice(&[2, 5, 0, 0, 0]);
        b2.extend_from_slice(&[0xF8, 0x00]);
        let t2 = EbdtTable::parse(&b2).unwrap();
        let g2 = t2
            .lookup(&small_entry(2, off2 as u32, (b2.len() - off2) as u32, 1))
            .unwrap()
            .unwrap();
        assert_eq!(g2.pixels, g1.pixels);
    }

    #[test]
    fn format6_big_metrics_grayscale_4bit() {
        // 2×1, bitDepth 4, byte-aligned. Row = two 4-bit samples
        // 0xF (=15 -> 255) and 0x8 (=8 -> 8*255/15 = 136). Byte = 0xF8.
        let mut bytes = vec![0u8; 4];
        bytes[0..2].copy_from_slice(&2u16.to_be_bytes());
        let off = bytes.len();
        // BigGlyphMetrics: h=1,w=2,hbx=3,hby=4,hadv=5,vbx,vby,vadv.
        bytes.extend_from_slice(&[1, 2, 3, 4, 5, 0, 0, 0]);
        bytes.push(0xF8);
        let t = EbdtTable::parse(&bytes).unwrap();
        let g = t
            .lookup(&small_entry(6, off as u32, (bytes.len() - off) as u32, 4))
            .unwrap()
            .unwrap();
        assert_eq!((g.width, g.height, g.bit_depth), (2, 1, 4));
        assert_eq!((g.bearing_x, g.bearing_y, g.advance), (3, 4, 5));
        assert_eq!(g.pixels, vec![255, (8u32 * 255 / 15) as u8]);
    }

    #[test]
    fn format5_uses_eblc_fixed_metrics() {
        // No inline metrics — comes from CblcEntry.fixed_metrics.
        let mut bytes = vec![0u8; 4];
        bytes[0..2].copy_from_slice(&2u16.to_be_bytes());
        let off = bytes.len();
        // 4×1 bitDepth 1 = 4 bits = 1011 -> 0b1011_0000 = 0xB0.
        bytes.push(0xB0);
        let mut e = small_entry(5, off as u32, 1, 1);
        e.fixed_metrics = Some(BigGlyphMetrics {
            height: 1,
            width: 4,
            hori_bearing_x: -2,
            hori_bearing_y: 6,
            hori_advance: 7,
            vert_bearing_x: 0,
            vert_bearing_y: 0,
            vert_advance: 0,
        });
        let t = EbdtTable::parse(&bytes).unwrap();
        let g = t.lookup(&e).unwrap().unwrap();
        assert_eq!((g.width, g.height), (4, 1));
        assert_eq!((g.bearing_x, g.bearing_y, g.advance), (-2, 6, 7));
        assert_eq!(g.pixels, vec![255, 0, 255, 255]);
    }

    #[test]
    fn format5_without_fixed_metrics_errors() {
        let mut bytes = vec![0u8; 4];
        bytes[0..2].copy_from_slice(&2u16.to_be_bytes());
        bytes.push(0x00);
        let t = EbdtTable::parse(&bytes).unwrap();
        let e = small_entry(5, 4, 1, 1);
        assert!(matches!(t.lookup(&e), Err(Error::BadStructure(_))));
    }

    #[test]
    fn color_bitdepth_returns_none() {
        let mut bytes = vec![0u8; 8];
        bytes[0..2].copy_from_slice(&3u16.to_be_bytes());
        let t = EbdtTable::parse(&bytes).unwrap();
        let e = small_entry(1, 4, 1, 32); // bitDepth 32 = BGRA colour
        assert!(t.lookup(&e).unwrap().is_none());
    }

    #[test]
    fn unsupported_format_returns_none() {
        let mut bytes = vec![0u8; 8];
        bytes[0..2].copy_from_slice(&2u16.to_be_bytes());
        let t = EbdtTable::parse(&bytes).unwrap();
        for fmt in [4u16, 8, 9] {
            let e = small_entry(fmt, 4, 1, 1);
            assert!(t.lookup(&e).unwrap().is_none(), "format {fmt}");
        }
    }

    #[test]
    fn truncated_image_data_errors() {
        let mut bytes = vec![0u8; 4];
        bytes[0..2].copy_from_slice(&2u16.to_be_bytes());
        let off = bytes.len();
        // Claim 4×4 (needs 4 bytes byte-aligned) but supply only metrics
        // + 1 data byte.
        bytes.extend_from_slice(&[4, 4, 0, 0, 0]);
        bytes.push(0x00);
        let t = EbdtTable::parse(&bytes).unwrap();
        let e = small_entry(1, off as u32, (bytes.len() - off) as u32, 1);
        assert!(matches!(t.lookup(&e), Err(Error::UnexpectedEof)));
    }

    #[test]
    fn out_of_range_offset_errors() {
        let mut bytes = vec![0u8; 8];
        bytes[0..2].copy_from_slice(&2u16.to_be_bytes());
        let t = EbdtTable::parse(&bytes).unwrap();
        let e = small_entry(1, 100, 10, 1);
        assert!(matches!(t.lookup(&e), Err(Error::BadOffset)));
    }

    #[test]
    fn composite_format8_descriptor() {
        // Format 8: SmallGlyphMetrics(5) | pad(1) | numComponents(2) |
        // EbdtComponent[2] (4 bytes each).
        let mut bytes = vec![0u8; 4];
        bytes[0..2].copy_from_slice(&2u16.to_be_bytes());
        let off = bytes.len();
        // metrics h=10, w=8, bx=1, by=9, adv=8.
        bytes.extend_from_slice(&[10, 8, 1, 9, 8]);
        bytes.push(0x00); // pad
        bytes.extend_from_slice(&2u16.to_be_bytes()); // numComponents
                                                      // component 0: glyph 5 at (0, 0)
        bytes.extend_from_slice(&5u16.to_be_bytes());
        bytes.push(0);
        bytes.push(0);
        // component 1: glyph 6 at (3, 255==-1)
        bytes.extend_from_slice(&6u16.to_be_bytes());
        bytes.push(3);
        bytes.push(0xFF);
        let total = (bytes.len() - off) as u32;
        let t = EbdtTable::parse(&bytes).unwrap();
        let e = small_entry(8, off as u32, total, 1);
        // The pixel lookup yields None (no imagery) ...
        assert!(t.lookup(&e).unwrap().is_none());
        // ... but the composite descriptor decodes.
        let c = t.lookup_composite(&e).unwrap().unwrap();
        assert_eq!((c.width, c.height), (8, 10));
        assert_eq!((c.bearing_x, c.bearing_y, c.advance), (1, 9, 8));
        assert_eq!(c.components.len(), 2);
        assert_eq!(
            c.components[0],
            EbdtComponent {
                glyph_id: 5,
                x_offset: 0,
                y_offset: 0
            }
        );
        assert_eq!(
            c.components[1],
            EbdtComponent {
                glyph_id: 6,
                x_offset: 3,
                y_offset: -1
            }
        );
    }

    #[test]
    fn composite_format9_big_metrics() {
        // Format 9: BigGlyphMetrics(8) | numComponents(2) | EbdtComponent[1]
        let mut bytes = vec![0u8; 4];
        bytes[0..2].copy_from_slice(&2u16.to_be_bytes());
        let off = bytes.len();
        // BigGlyphMetrics: h=4, w=4, hbx=0, hby=4, hadv=4, vbx, vby, vadv.
        bytes.extend_from_slice(&[4, 4, 0, 4, 4, 0, 0, 4]);
        bytes.extend_from_slice(&1u16.to_be_bytes()); // numComponents
        bytes.extend_from_slice(&7u16.to_be_bytes());
        bytes.push(2);
        bytes.push(2);
        let total = (bytes.len() - off) as u32;
        let t = EbdtTable::parse(&bytes).unwrap();
        let e = small_entry(9, off as u32, total, 4);
        let c = t.lookup_composite(&e).unwrap().unwrap();
        assert_eq!((c.width, c.height), (4, 4));
        assert_eq!(c.bit_depth, 4);
        assert_eq!(c.components.len(), 1);
        assert_eq!(
            c.components[0],
            EbdtComponent {
                glyph_id: 7,
                x_offset: 2,
                y_offset: 2
            }
        );
    }

    #[test]
    fn composite_truncated_array_errors() {
        let mut bytes = vec![0u8; 4];
        bytes[0..2].copy_from_slice(&2u16.to_be_bytes());
        let off = bytes.len();
        bytes.extend_from_slice(&[10, 8, 1, 9, 8]); // small metrics
        bytes.push(0x00); // pad
        bytes.extend_from_slice(&3u16.to_be_bytes()); // claims 3 components
                                                      // ... but only supply one 4-byte record.
        bytes.extend_from_slice(&[0, 5, 0, 0]);
        let total = (bytes.len() - off) as u32;
        let t = EbdtTable::parse(&bytes).unwrap();
        let e = small_entry(8, off as u32, total, 1);
        assert!(matches!(t.lookup_composite(&e), Err(Error::UnexpectedEof)));
    }

    #[test]
    fn lookup_composite_none_for_pixel_format() {
        let mut bytes = vec![0u8; 8];
        bytes[0..2].copy_from_slice(&2u16.to_be_bytes());
        let t = EbdtTable::parse(&bytes).unwrap();
        // A format-1 (pixel) entry has no composite descriptor.
        let e = small_entry(1, 4, 1, 1);
        assert!(t.lookup_composite(&e).unwrap().is_none());
    }
}
