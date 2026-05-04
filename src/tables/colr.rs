//! `COLR` — Color Table (version 0).
//!
//! The COLR table maps a "base glyph" to an ordered stack of "layer
//! glyphs", each tagged with a CPAL palette-entry index. To render a
//! coloured glyph, the consumer takes each layer in order (back-to-front),
//! resolves its palette index against the CPAL palette to get an RGBA
//! colour, then paints the layer glyph's outline at the same pen origin
//! filled with that colour. The base glyph's own outline is **not**
//! drawn — it's a stand-in glyph id that the cmap maps to (so e.g. the
//! emoji code-point routes through the layer stack instead of the
//! single-colour outline).
//!
//! Spec: Microsoft OpenType §"COLR — Color Table". This implementation
//! only parses **version 0** (flat layer stack, palette-indexed colours).
//! Version 1 (paint graph with gradients, transforms and composites)
//! and version 2/3 (variation deltas) are out of scope for this round —
//! when the version field is not 0 we still allow the table to parse
//! (the v0 fields are always present at the same offsets per the spec)
//! but only the v0 base/layer arrays are exposed.
//!
//! ## Header layout (v0, 14 bytes)
//!
//! ```text
//! Offset  Field                    Type      Notes
//! ------  ----------------------   --------  -------------------------------
//!  +0     version                  uint16    0 (or 1+ for newer tables)
//!  +2     numBaseGlyphRecords      uint16    BaseGlyph record count
//!  +4     baseGlyphRecordsOffset   Offset32  from start of COLR
//!  +8     layerRecordsOffset       Offset32  from start of COLR
//! +12     numLayerRecords          uint16    Layer record count
//! ```
//!
//! ## BaseGlyphRecord (6 bytes, sorted ascending by glyphID)
//!
//! ```text
//! +0     glyphID                  uint16    base glyph id
//! +2     firstLayerIndex          uint16    base index into layer array
//! +4     numLayers                uint16    contiguous layer count
//! ```
//!
//! ## LayerRecord (4 bytes)
//!
//! ```text
//! +0     glyphID                  uint16    layer glyph id (TT outline)
//! +2     paletteIndex             uint16    CPAL entry index;
//!                                          0xFFFF = "use foreground"
//! ```

use crate::parser::{read_u16, read_u32};
use crate::Error;

/// One layer of a colour glyph: an outline-glyph id plus a CPAL
/// palette-entry index. `palette_index == 0xFFFF` is the spec's
/// "use the text foreground colour" sentinel; the consumer renderer is
/// expected to substitute its own foreground in that case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorLayer {
    /// Glyph id of the outline that paints this layer (TT/CFF/CFF2).
    pub layer_glyph_id: u16,
    /// CPAL palette-entry index; `0xFFFF` = foreground colour.
    pub palette_index: u16,
}

/// Parsed COLR table (v0 walker).
#[derive(Debug, Clone)]
pub struct ColrTable<'a> {
    bytes: &'a [u8],
    /// Number of `BaseGlyphRecord`s (always v0-array-shaped).
    num_base_records: u16,
    /// Byte offset (from start of COLR) of the base record array.
    base_records_offset: u32,
    /// Number of `LayerRecord`s.
    num_layer_records: u16,
    /// Byte offset of the layer record array.
    layer_records_offset: u32,
}

impl<'a> ColrTable<'a> {
    /// Validate the header and remember the v0 array offsets. Higher
    /// versions are accepted (the v0 fields at offsets 0..14 are
    /// always present per the spec); v1+ paint-graph extensions are
    /// just ignored.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < 14 {
            return Err(Error::UnexpectedEof);
        }
        // version at +0; we don't reject non-zero values since v0
        // fields keep their meaning in v1/v2/v3 (the extensions are
        // additive trailing fields).
        let _version = read_u16(bytes, 0)?;
        let num_base_records = read_u16(bytes, 2)?;
        let base_records_offset = read_u32(bytes, 4)?;
        let layer_records_offset = read_u32(bytes, 8)?;
        let num_layer_records = read_u16(bytes, 12)?;

        // Range-check the two arrays against the COLR slice. We allow
        // an empty array (numFoo == 0) regardless of offset.
        if num_base_records > 0 {
            let end = (base_records_offset as u64)
                .checked_add(num_base_records as u64 * 6)
                .ok_or(Error::BadOffset)?;
            if end > bytes.len() as u64 {
                return Err(Error::BadOffset);
            }
        }
        if num_layer_records > 0 {
            let end = (layer_records_offset as u64)
                .checked_add(num_layer_records as u64 * 4)
                .ok_or(Error::BadOffset)?;
            if end > bytes.len() as u64 {
                return Err(Error::BadOffset);
            }
        }

        Ok(Self {
            bytes,
            num_base_records,
            base_records_offset,
            num_layer_records,
            layer_records_offset,
        })
    }

    /// Locate `glyph_id`'s BaseGlyphRecord by binary search and decode
    /// its `(first_layer_index, num_layers)` pair. Returns `None` when
    /// the glyph isn't a base — i.e. it's a single-colour outline glyph
    /// or a layer-only glyph.
    fn find_base_record(&self, glyph_id: u16) -> Option<(u16, u16)> {
        // Records are required to be sorted ascending by glyphID.
        let base = self.base_records_offset as usize;
        let mut lo = 0i32;
        let mut hi = self.num_base_records as i32 - 1;
        while lo <= hi {
            let mid = ((lo + hi) >> 1) as usize;
            let off = base + mid * 6;
            let gid = read_u16(self.bytes, off).ok()?;
            match gid.cmp(&glyph_id) {
                std::cmp::Ordering::Less => lo = mid as i32 + 1,
                std::cmp::Ordering::Greater => hi = mid as i32 - 1,
                std::cmp::Ordering::Equal => {
                    let first = read_u16(self.bytes, off + 2).ok()?;
                    let count = read_u16(self.bytes, off + 4).ok()?;
                    return Some((first, count));
                }
            }
        }
        None
    }

    /// All colour layers for `glyph_id`, in back-to-front paint order
    /// (= the order the layer records appear in the table). Returns an
    /// empty `Vec` when the glyph isn't a colour glyph or the COLR
    /// table is empty.
    pub fn layers(&self, glyph_id: u16) -> Vec<ColorLayer> {
        let (first, count) = match self.find_base_record(glyph_id) {
            Some(p) => p,
            None => return Vec::new(),
        };
        let mut out = Vec::with_capacity(count as usize);
        let layer_base = self.layer_records_offset as usize;
        for i in 0..count {
            let idx = first as usize + i as usize;
            // Spec says firstLayerIndex+numLayers must be <=
            // numLayerRecords; we range-check defensively anyway.
            if idx >= self.num_layer_records as usize {
                break;
            }
            let off = layer_base + idx * 4;
            let layer_glyph_id = match read_u16(self.bytes, off) {
                Ok(v) => v,
                Err(_) => break,
            };
            let palette_index = match read_u16(self.bytes, off + 2) {
                Ok(v) => v,
                Err(_) => break,
            };
            out.push(ColorLayer {
                layer_glyph_id,
                palette_index,
            });
        }
        out
    }

    /// Number of `BaseGlyphRecord`s the table ships. Mostly useful for
    /// tests / debug printing; consumers should call `layers` directly.
    pub fn num_base_records(&self) -> u16 {
        self.num_base_records
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-build a 4-byte-aligned COLR v0 fragment with one base
    /// glyph (gid 65) that points at three layers.
    fn synth_colr_one_base_three_layers() -> Vec<u8> {
        // Header (14 B) + 1 BaseGlyphRecord (6 B) + 3 LayerRecord (12 B) = 32 B
        let mut bytes = vec![0u8; 32];
        // version = 0
        bytes[0..2].copy_from_slice(&0u16.to_be_bytes());
        // numBaseGlyphRecords = 1
        bytes[2..4].copy_from_slice(&1u16.to_be_bytes());
        // baseGlyphRecordsOffset = 14
        bytes[4..8].copy_from_slice(&14u32.to_be_bytes());
        // layerRecordsOffset = 20
        bytes[8..12].copy_from_slice(&20u32.to_be_bytes());
        // numLayerRecords = 3
        bytes[12..14].copy_from_slice(&3u16.to_be_bytes());

        // BaseGlyphRecord at +14: glyphID=65, firstLayerIndex=0, numLayers=3
        bytes[14..16].copy_from_slice(&65u16.to_be_bytes());
        bytes[16..18].copy_from_slice(&0u16.to_be_bytes());
        bytes[18..20].copy_from_slice(&3u16.to_be_bytes());

        // LayerRecord[0..3] at +20
        // Layer 0: glyphID=100, paletteIndex=2
        bytes[20..22].copy_from_slice(&100u16.to_be_bytes());
        bytes[22..24].copy_from_slice(&2u16.to_be_bytes());
        // Layer 1: glyphID=101, paletteIndex=5
        bytes[24..26].copy_from_slice(&101u16.to_be_bytes());
        bytes[26..28].copy_from_slice(&5u16.to_be_bytes());
        // Layer 2: glyphID=102, paletteIndex=0xFFFF (foreground)
        bytes[28..30].copy_from_slice(&102u16.to_be_bytes());
        bytes[30..32].copy_from_slice(&0xFFFFu16.to_be_bytes());
        bytes
    }

    #[test]
    fn parses_v0_header() {
        let bytes = synth_colr_one_base_three_layers();
        let colr = ColrTable::parse(&bytes).expect("parse");
        assert_eq!(colr.num_base_records(), 1);
    }

    #[test]
    fn layers_for_known_base_glyph() {
        let bytes = synth_colr_one_base_three_layers();
        let colr = ColrTable::parse(&bytes).expect("parse");
        let layers = colr.layers(65);
        assert_eq!(
            layers,
            vec![
                ColorLayer {
                    layer_glyph_id: 100,
                    palette_index: 2
                },
                ColorLayer {
                    layer_glyph_id: 101,
                    palette_index: 5
                },
                ColorLayer {
                    layer_glyph_id: 102,
                    palette_index: 0xFFFF
                },
            ]
        );
    }

    #[test]
    fn layers_for_non_base_glyph_is_empty() {
        let bytes = synth_colr_one_base_three_layers();
        let colr = ColrTable::parse(&bytes).expect("parse");
        assert!(colr.layers(0).is_empty());
        assert!(colr.layers(64).is_empty());
        assert!(colr.layers(66).is_empty());
        assert!(colr.layers(0xFFFF).is_empty());
    }

    #[test]
    fn rejects_truncated_header() {
        assert!(matches!(
            ColrTable::parse(&[0u8; 10]),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_array_past_end() {
        let mut bytes = vec![0u8; 14];
        // numBaseGlyphRecords = 1, baseRecordsOffset = 14 (but no data after).
        bytes[2..4].copy_from_slice(&1u16.to_be_bytes());
        bytes[4..8].copy_from_slice(&14u32.to_be_bytes());
        assert!(matches!(ColrTable::parse(&bytes), Err(Error::BadOffset)));
    }

    /// Three base glyphs with random-but-sorted gids: verify binary
    /// search lands on the correct middle / left / right elements.
    #[test]
    fn binary_search_three_records() {
        let mut bytes = vec![0u8; 14 + 18 + 12];
        bytes[0..2].copy_from_slice(&0u16.to_be_bytes());
        bytes[2..4].copy_from_slice(&3u16.to_be_bytes());
        bytes[4..8].copy_from_slice(&14u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&32u32.to_be_bytes());
        bytes[12..14].copy_from_slice(&3u16.to_be_bytes());

        // Records (sorted by gid): 10/0/1, 50/1/1, 200/2/1
        let recs: [(u16, u16, u16); 3] = [(10, 0, 1), (50, 1, 1), (200, 2, 1)];
        for (i, (g, first, count)) in recs.iter().enumerate() {
            let off = 14 + i * 6;
            bytes[off..off + 2].copy_from_slice(&g.to_be_bytes());
            bytes[off + 2..off + 4].copy_from_slice(&first.to_be_bytes());
            bytes[off + 4..off + 6].copy_from_slice(&count.to_be_bytes());
        }
        // Layers: gid 1000+i / palette i
        for i in 0..3 {
            let off = 32 + i * 4;
            bytes[off..off + 2].copy_from_slice(&(1000 + i as u16).to_be_bytes());
            bytes[off + 2..off + 4].copy_from_slice(&(i as u16).to_be_bytes());
        }

        let colr = ColrTable::parse(&bytes).expect("parse");
        // Hits
        for (gid, _first, _count) in &recs {
            let layers = colr.layers(*gid);
            assert_eq!(layers.len(), 1, "gid {gid}");
        }
        // Misses
        assert!(colr.layers(0).is_empty());
        assert!(colr.layers(11).is_empty());
        assert!(colr.layers(199).is_empty());
        assert!(colr.layers(201).is_empty());
    }
}
