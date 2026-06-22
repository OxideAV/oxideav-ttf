//! `CPAL` — Color Palette Table (versions 0 and 1).
//!
//! CPAL ships one or more colour palettes. Each palette has the same
//! number of entries (`numPaletteEntries`); a palette entry is a single
//! sRGB BGRA byte tuple. The COLR table addresses palette entries by
//! `(palette_index, color_index)` — typically the renderer picks
//! palette 0 ("default"), and COLR layers carry the per-layer colour
//! index. The reserved colour index `0xFFFF` (handled in `colr.rs`,
//! never appears in this table) means "use foreground colour".
//!
//! Spec: ISO/IEC 14496-22:2019 §5.7.11 (Microsoft OpenType §"CPAL —
//! Color Palette Table"). This crate implements the v0 walker (palette
//! plus colour record array) and the full v1 sidecar: palette types,
//! palette labels, and palette-entry labels. Consumers that care can
//! read the palette type for light/dark-background hints, the palette
//! label for the per-palette UI name-table ID, and the palette-entry
//! label for the per-entry UI name-table ID such as Outline or Fill.
//! Variable-CPAL deltas are out of scope.
//!
//! ## Header layout (v0, fixed 12 bytes + 2*numPalettes index array)
//!
//! ```text
//! Offset  Field                        Type          Notes
//! ------  --------------------------   ------------  -------------------
//!  +0     version                      uint16        0 or 1
//!  +2     numPaletteEntries            uint16        entries per palette
//!  +4     numPalettes                  uint16        palette count
//!  +6     numColorRecords              uint16        total ColorRecords
//!  +8     colorRecordsArrayOffset      Offset32      from start of CPAL
//! +12     colorRecordIndices[N]        uint16[N]     per-palette base index
//! ```
//!
//! ## v1 trailer (3*Offset32 immediately after `colorRecordIndices`)
//!
//! ```text
//! +12+2*N  paletteTypesArrayOffset       Offset32   0 = absent
//! +16+2*N  paletteLabelsArrayOffset      Offset32   0 = absent
//! +20+2*N  paletteEntryLabelsArrayOffset Offset32   0 = absent
//! ```
//!
//! ## ColorRecord (4 bytes, sRGB)
//!
//! ```text
//! +0  blue   uint8
//! +1  green  uint8
//! +2  red    uint8
//! +3  alpha  uint8   (0 = transparent, 255 = opaque, NOT pre-multiplied)
//! ```

use crate::parser::{read_u16, read_u32};
use crate::Error;

/// Decoded CPAL v0/v1 header. Colour records are decoded lazily by
/// [`CpalTable::color`] / [`CpalTable::palette`] — we only validate
/// the fixed-size header + the palette index array up front.
#[derive(Debug, Clone)]
pub struct CpalTable<'a> {
    bytes: &'a [u8],
    /// `0` or `1` per the spec.
    version: u16,
    /// Entries per palette (== count returned by `palette()`).
    num_palette_entries: u16,
    /// Palette count.
    num_palettes: u16,
    /// Total ColorRecord count (palettes may overlap and share records).
    num_color_records: u16,
    /// Offset (from start of CPAL) of the first ColorRecord.
    color_records_array_offset: u32,
    /// Where the v0 `colorRecordIndices` array starts (always +12).
    color_record_indices_offset: usize,
    /// v1 `paletteTypesArrayOffset` (0 or table-relative).
    palette_types_array_offset: u32,
    /// v1 `paletteLabelsArrayOffset` (0 or table-relative). Each entry
    /// is a `uint16` name-table ID (or `0xFFFF` = no label).
    palette_labels_array_offset: u32,
    /// v1 `paletteEntryLabelsArrayOffset` (0 or table-relative). Each
    /// entry is a `uint16` name-table ID (or `0xFFFF` = no label).
    palette_entry_labels_array_offset: u32,
}

/// Sentinel `uint16` value meaning "no name-table ID was provided for
/// this palette / palette entry" in the CPAL v1 label arrays.
/// (ISO/IEC 14496-22:2019 §5.7.11 — "Use 0xFFFF if no name ID is
/// provided".)
pub const NO_NAME_ID: u16 = 0xFFFF;

impl<'a> CpalTable<'a> {
    /// Parse the v0/v1 header and validate index arrays + colour records
    /// against the slice. Higher major versions are accepted (we read
    /// only the v0/v1 prefix); unknown trailing fields are ignored.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < 12 {
            return Err(Error::UnexpectedEof);
        }
        let version = read_u16(bytes, 0)?;
        let num_palette_entries = read_u16(bytes, 2)?;
        let num_palettes = read_u16(bytes, 4)?;
        let num_color_records = read_u16(bytes, 6)?;
        let color_records_array_offset = read_u32(bytes, 8)?;

        // colorRecordIndices array: numPalettes * uint16 immediately
        // after the fixed header.
        let indices_off = 12usize;
        let indices_end = indices_off
            .checked_add(num_palettes as usize * 2)
            .ok_or(Error::BadOffset)?;
        if bytes.len() < indices_end {
            return Err(Error::UnexpectedEof);
        }

        // Validate that the colour records array fits inside the slice.
        let color_records_end = (color_records_array_offset as u64)
            .checked_add(num_color_records as u64 * 4)
            .ok_or(Error::BadOffset)?;
        if color_records_end > bytes.len() as u64 {
            return Err(Error::BadOffset);
        }

        // v1: three trailing Offset32 fields immediately after
        // colorRecordIndices — offsetPaletteTypeArray,
        // offsetPaletteLabelArray, offsetPaletteEntryLabelArray.
        // Some real-world fonts ship a v1 header that we truncate
        // gracefully — a missing trailer is treated as "no extras"
        // (all three offsets 0).
        let (
            palette_types_array_offset,
            palette_labels_array_offset,
            palette_entry_labels_array_offset,
        ) = if version >= 1 && bytes.len() >= indices_end + 12 {
            let trailer = indices_end;
            (
                read_u32(bytes, trailer)?,
                read_u32(bytes, trailer + 4)?,
                read_u32(bytes, trailer + 8)?,
            )
        } else {
            (0, 0, 0)
        };

        Ok(Self {
            bytes,
            version,
            num_palette_entries,
            num_palettes,
            num_color_records,
            color_records_array_offset,
            color_record_indices_offset: indices_off,
            palette_types_array_offset,
            palette_labels_array_offset,
            palette_entry_labels_array_offset,
        })
    }

    /// CPAL header version (`0` or `1`).
    pub fn version(&self) -> u16 {
        self.version
    }

    /// Entries per palette. (Every palette in a CPAL table has the same
    /// length per the spec.)
    pub fn num_palette_entries(&self) -> u16 {
        self.num_palette_entries
    }

    /// Number of palettes available. Palette 0 is "default".
    pub fn num_palettes(&self) -> u16 {
        self.num_palettes
    }

    /// First-colour-record index for palette `palette_index`, or `None`
    /// if `palette_index >= num_palettes`.
    fn first_record(&self, palette_index: u16) -> Option<u16> {
        if palette_index >= self.num_palettes {
            return None;
        }
        let off = self.color_record_indices_offset + (palette_index as usize) * 2;
        read_u16(self.bytes, off).ok()
    }

    /// Resolve a single colour by `(palette_index, color_index)`.
    /// Returns `[r, g, b, a]` (the byte order swizzled out of CPAL's
    /// on-disk BGRA). `None` when either index is out of range.
    pub fn color(&self, palette_index: u16, color_index: u16) -> Option<[u8; 4]> {
        let first = self.first_record(palette_index)?;
        if color_index >= self.num_palette_entries {
            return None;
        }
        // Combined index into the colour record array.
        let abs = first as u32 + color_index as u32;
        if abs >= self.num_color_records as u32 {
            return None;
        }
        let off = self.color_records_array_offset as usize + (abs as usize) * 4;
        // BGRA on disk -> RGBA in our public API.
        let b = self.bytes.get(off)?;
        let g = self.bytes.get(off + 1)?;
        let r = self.bytes.get(off + 2)?;
        let a = self.bytes.get(off + 3)?;
        Some([*r, *g, *b, *a])
    }

    /// Read the entire palette as an owned `Vec<[u8; 4]>` (RGBA byte
    /// order). Returns `None` if `palette_index` is out of range.
    pub fn palette(&self, palette_index: u16) -> Option<Vec<[u8; 4]>> {
        let n = self.num_palette_entries;
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            out.push(self.color(palette_index, i)?);
        }
        Some(out)
    }

    /// v1 palette-type flags for palette `palette_index`. Returns 0
    /// when the table is v0, the trailer is absent, or
    /// `palette_index` is out of range.
    ///
    /// Bit 0 (`0x0001`) = USABLE_WITH_LIGHT_BACKGROUND
    /// Bit 1 (`0x0002`) = USABLE_WITH_DARK_BACKGROUND
    pub fn palette_type(&self, palette_index: u16) -> u32 {
        if palette_index >= self.num_palettes
            || self.palette_types_array_offset == 0
            || self.version < 1
        {
            return 0;
        }
        let off = self.palette_types_array_offset as usize + palette_index as usize * 4;
        // Defensive bounds check — the v1 trailer offsets are sometimes
        // sloppy in real fonts.
        if off + 4 > self.bytes.len() {
            return 0;
        }
        read_u32(self.bytes, off).unwrap_or(0)
    }

    /// v1 palette **label** for palette `palette_index`: the `name`
    /// table ID of a user-interface string describing this palette
    /// (e.g. "Regular", "High Contrast"). Returns `None` when the table
    /// is v0, the `paletteLabelArray` is absent, `palette_index` is out
    /// of range, or the slot holds the `0xFFFF` "no label" sentinel.
    ///
    /// Per ISO/IEC 14496-22:2019 §5.7.11 the label array has one
    /// `uint16` name ID per palette (`paletteLabels[numPalettes]`).
    pub fn palette_label(&self, palette_index: u16) -> Option<u16> {
        if palette_index >= self.num_palettes
            || self.palette_labels_array_offset == 0
            || self.version < 1
        {
            return None;
        }
        let off = self.palette_labels_array_offset as usize + palette_index as usize * 2;
        if off + 2 > self.bytes.len() {
            return None;
        }
        match read_u16(self.bytes, off).ok()? {
            NO_NAME_ID => None,
            id => Some(id),
        }
    }

    /// v1 palette **entry** label for entry `entry_index`: the `name`
    /// table ID of a user-interface string describing this palette
    /// entry across *all* palettes (e.g. "Outline", "Fill"). Returns
    /// `None` when the table is v0, the `paletteEntryLabelArray` is
    /// absent, `entry_index` is out of range, or the slot holds the
    /// `0xFFFF` "no label" sentinel.
    ///
    /// Per ISO/IEC 14496-22:2019 §5.7.11 the entry-label array has one
    /// `uint16` name ID per palette **entry**
    /// (`paletteEntryLabels[numPaletteEntries]`) and applies uniformly
    /// to every palette in the font.
    pub fn palette_entry_label(&self, entry_index: u16) -> Option<u16> {
        if entry_index >= self.num_palette_entries
            || self.palette_entry_labels_array_offset == 0
            || self.version < 1
        {
            return None;
        }
        let off = self.palette_entry_labels_array_offset as usize + entry_index as usize * 2;
        if off + 2 > self.bytes.len() {
            return None;
        }
        match read_u16(self.bytes, off).ok()? {
            NO_NAME_ID => None,
            id => Some(id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal CPAL v0 with 2 palettes of 3 entries each.
    fn synth_cpal_v0_two_palettes() -> Vec<u8> {
        // Header (12) + colorRecordIndices (2*2=4) + 6 ColorRecords (24)
        let header_end = 12 + 4;
        let records_off = header_end;
        let total = records_off + 6 * 4;
        let mut bytes = vec![0u8; total];
        bytes[0..2].copy_from_slice(&0u16.to_be_bytes()); // version 0
        bytes[2..4].copy_from_slice(&3u16.to_be_bytes()); // numPaletteEntries
        bytes[4..6].copy_from_slice(&2u16.to_be_bytes()); // numPalettes
        bytes[6..8].copy_from_slice(&6u16.to_be_bytes()); // numColorRecords
        bytes[8..12].copy_from_slice(&(records_off as u32).to_be_bytes());
        // colorRecordIndices: palette 0 -> 0, palette 1 -> 3
        bytes[12..14].copy_from_slice(&0u16.to_be_bytes());
        bytes[14..16].copy_from_slice(&3u16.to_be_bytes());
        // Records: palette 0 = red, green, blue
        // BGRA on disk
        let recs = [
            (0xFF, 0x00, 0x00, 0xFF), // red   = (R=FF,G=00,B=00,A=FF) -> BGRA(00,00,FF,FF)
            (0x00, 0xFF, 0x00, 0xFF), // green
            (0x00, 0x00, 0xFF, 0xFF), // blue
            (0x80, 0x80, 0x80, 0x80), // gray, half alpha
            (0x00, 0x00, 0x00, 0xFF), // black
            (0xFF, 0xFF, 0xFF, 0xFF), // white
        ];
        for (i, (r, g, b, a)) in recs.iter().enumerate() {
            let off = records_off + i * 4;
            bytes[off] = *b;
            bytes[off + 1] = *g;
            bytes[off + 2] = *r;
            bytes[off + 3] = *a;
        }
        bytes
    }

    #[test]
    fn parses_v0_header() {
        let bytes = synth_cpal_v0_two_palettes();
        let cpal = CpalTable::parse(&bytes).expect("parse");
        assert_eq!(cpal.version(), 0);
        assert_eq!(cpal.num_palettes(), 2);
        assert_eq!(cpal.num_palette_entries(), 3);
    }

    #[test]
    fn color_lookup_palette0() {
        let bytes = synth_cpal_v0_two_palettes();
        let cpal = CpalTable::parse(&bytes).expect("parse");
        assert_eq!(cpal.color(0, 0), Some([0xFF, 0x00, 0x00, 0xFF])); // red
        assert_eq!(cpal.color(0, 1), Some([0x00, 0xFF, 0x00, 0xFF])); // green
        assert_eq!(cpal.color(0, 2), Some([0x00, 0x00, 0xFF, 0xFF])); // blue
    }

    #[test]
    fn color_lookup_palette1() {
        let bytes = synth_cpal_v0_two_palettes();
        let cpal = CpalTable::parse(&bytes).expect("parse");
        assert_eq!(cpal.color(1, 0), Some([0x80, 0x80, 0x80, 0x80])); // gray, half alpha
        assert_eq!(cpal.color(1, 1), Some([0x00, 0x00, 0x00, 0xFF])); // black
        assert_eq!(cpal.color(1, 2), Some([0xFF, 0xFF, 0xFF, 0xFF])); // white
    }

    #[test]
    fn out_of_range_lookup_returns_none() {
        let bytes = synth_cpal_v0_two_palettes();
        let cpal = CpalTable::parse(&bytes).expect("parse");
        assert!(cpal.color(2, 0).is_none()); // palette out of range
        assert!(cpal.color(0, 3).is_none()); // entry out of range
        assert!(cpal.color(99, 99).is_none());
    }

    #[test]
    fn palette_returns_full_vec() {
        let bytes = synth_cpal_v0_two_palettes();
        let cpal = CpalTable::parse(&bytes).expect("parse");
        let p1 = cpal.palette(1).expect("palette 1");
        assert_eq!(p1.len(), 3);
        assert_eq!(p1[0], [0x80, 0x80, 0x80, 0x80]);
        assert_eq!(p1[1], [0x00, 0x00, 0x00, 0xFF]);
        assert_eq!(p1[2], [0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(cpal.palette(2).is_none());
    }

    /// Synthesise a v1 header with paletteTypes: palette 0 = light,
    /// palette 1 = dark.
    fn synth_cpal_v1_with_types() -> Vec<u8> {
        // v1 trailer adds 3 * Offset32 = 12 bytes after the indices array.
        // We only emit paletteTypesArrayOffset; labels & entry labels = 0.
        // Header (12) + indices (4) + trailer (12) + records (24) + types (8)
        let header_fixed = 12;
        let indices = 4; // 2 palettes
        let trailer = 12;
        let records_off = header_fixed + indices + trailer;
        let records_len = 6 * 4;
        let types_off = records_off + records_len;
        let total = types_off + 4 * 2; // 2 palettes * uint32

        let mut bytes = vec![0u8; total];
        bytes[0..2].copy_from_slice(&1u16.to_be_bytes()); // version 1
        bytes[2..4].copy_from_slice(&3u16.to_be_bytes());
        bytes[4..6].copy_from_slice(&2u16.to_be_bytes());
        bytes[6..8].copy_from_slice(&6u16.to_be_bytes());
        bytes[8..12].copy_from_slice(&(records_off as u32).to_be_bytes());
        // colorRecordIndices
        bytes[12..14].copy_from_slice(&0u16.to_be_bytes());
        bytes[14..16].copy_from_slice(&3u16.to_be_bytes());
        // v1 trailer
        bytes[16..20].copy_from_slice(&(types_off as u32).to_be_bytes()); // paletteTypesOffset
        bytes[20..24].copy_from_slice(&0u32.to_be_bytes()); // labels = absent
        bytes[24..28].copy_from_slice(&0u32.to_be_bytes()); // entry labels = absent
                                                            // colour records — leave zeroed (we don't read them in this test)
                                                            // paletteTypes
        bytes[types_off..types_off + 4].copy_from_slice(&0x0001u32.to_be_bytes()); // light
        bytes[types_off + 4..types_off + 8].copy_from_slice(&0x0002u32.to_be_bytes()); // dark
        bytes
    }

    /// Synthesise a fully-populated v1 header: 2 palettes of 3 entries,
    /// with paletteTypes + paletteLabels + paletteEntryLabels arrays all
    /// present. Palette labels: [256, 0xFFFF]; entry labels: [300, 0xFFFF,
    /// 302].
    fn synth_cpal_v1_full() -> Vec<u8> {
        let header_fixed = 12;
        let indices = 4; // 2 palettes * uint16
        let trailer = 12; // 3 * Offset32
        let records_off = header_fixed + indices + trailer;
        let records_len = 6 * 4; // 6 records
        let types_off = records_off + records_len;
        let types_len = 2 * 4; // 2 palettes * uint32
        let labels_off = types_off + types_len;
        let labels_len = 2 * 2; // 2 palettes * uint16
        let entry_labels_off = labels_off + labels_len;
        let entry_labels_len = 3 * 2; // 3 entries * uint16
        let total = entry_labels_off + entry_labels_len;

        let mut bytes = vec![0u8; total];
        bytes[0..2].copy_from_slice(&1u16.to_be_bytes()); // version 1
        bytes[2..4].copy_from_slice(&3u16.to_be_bytes()); // numPaletteEntries
        bytes[4..6].copy_from_slice(&2u16.to_be_bytes()); // numPalettes
        bytes[6..8].copy_from_slice(&6u16.to_be_bytes()); // numColorRecords
        bytes[8..12].copy_from_slice(&(records_off as u32).to_be_bytes());
        bytes[12..14].copy_from_slice(&0u16.to_be_bytes()); // palette 0 base
        bytes[14..16].copy_from_slice(&3u16.to_be_bytes()); // palette 1 base
        bytes[16..20].copy_from_slice(&(types_off as u32).to_be_bytes());
        bytes[20..24].copy_from_slice(&(labels_off as u32).to_be_bytes());
        bytes[24..28].copy_from_slice(&(entry_labels_off as u32).to_be_bytes());
        // paletteTypes
        bytes[types_off..types_off + 4].copy_from_slice(&0x0001u32.to_be_bytes());
        bytes[types_off + 4..types_off + 8].copy_from_slice(&0x0002u32.to_be_bytes());
        // paletteLabels: [256, 0xFFFF]
        bytes[labels_off..labels_off + 2].copy_from_slice(&256u16.to_be_bytes());
        bytes[labels_off + 2..labels_off + 4].copy_from_slice(&0xFFFFu16.to_be_bytes());
        // paletteEntryLabels: [300, 0xFFFF, 302]
        bytes[entry_labels_off..entry_labels_off + 2].copy_from_slice(&300u16.to_be_bytes());
        bytes[entry_labels_off + 2..entry_labels_off + 4].copy_from_slice(&0xFFFFu16.to_be_bytes());
        bytes[entry_labels_off + 4..entry_labels_off + 6].copy_from_slice(&302u16.to_be_bytes());
        bytes
    }

    #[test]
    fn v1_palette_types() {
        let bytes = synth_cpal_v1_with_types();
        let cpal = CpalTable::parse(&bytes).expect("parse");
        assert_eq!(cpal.version(), 1);
        assert_eq!(cpal.palette_type(0), 0x0001);
        assert_eq!(cpal.palette_type(1), 0x0002);
        assert_eq!(cpal.palette_type(2), 0); // out of range
    }

    #[test]
    fn v1_types_only_has_no_labels() {
        // The types-only fixture sets label offsets to 0 → no labels.
        let bytes = synth_cpal_v1_with_types();
        let cpal = CpalTable::parse(&bytes).expect("parse");
        assert_eq!(cpal.palette_label(0), None);
        assert_eq!(cpal.palette_entry_label(0), None);
    }

    #[test]
    fn v1_palette_labels() {
        let bytes = synth_cpal_v1_full();
        let cpal = CpalTable::parse(&bytes).expect("parse");
        assert_eq!(cpal.palette_label(0), Some(256));
        assert_eq!(cpal.palette_label(1), None); // 0xFFFF sentinel
        assert_eq!(cpal.palette_label(2), None); // out of range
    }

    #[test]
    fn v1_palette_entry_labels() {
        let bytes = synth_cpal_v1_full();
        let cpal = CpalTable::parse(&bytes).expect("parse");
        assert_eq!(cpal.palette_entry_label(0), Some(300));
        assert_eq!(cpal.palette_entry_label(1), None); // 0xFFFF sentinel
        assert_eq!(cpal.palette_entry_label(2), Some(302));
        assert_eq!(cpal.palette_entry_label(3), None); // out of range
    }

    #[test]
    fn v0_has_no_labels() {
        let bytes = synth_cpal_v0_two_palettes();
        let cpal = CpalTable::parse(&bytes).expect("parse");
        assert_eq!(cpal.palette_label(0), None);
        assert_eq!(cpal.palette_entry_label(0), None);
    }

    #[test]
    fn rejects_truncated_header() {
        assert!(matches!(
            CpalTable::parse(&[0u8; 8]),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_records_past_end() {
        // v0 header claims 100 colour records but the slice is too short.
        let mut bytes = vec![0u8; 12 + 4]; // header + indices for 2 palettes
        bytes[2..4].copy_from_slice(&3u16.to_be_bytes());
        bytes[4..6].copy_from_slice(&2u16.to_be_bytes());
        bytes[6..8].copy_from_slice(&100u16.to_be_bytes());
        bytes[8..12].copy_from_slice(&100u32.to_be_bytes());
        assert!(matches!(CpalTable::parse(&bytes), Err(Error::BadOffset)));
    }
}
