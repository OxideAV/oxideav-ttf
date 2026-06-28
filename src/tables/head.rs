//! `head` — font header (ISO/IEC 14496-22:2019 §5.2.1).
//!
//! Decodes the full table: version / revision, the 16-bit `flags` word
//! (baseline-at-y0, lsb-at-x0, instructions-depend-on-ppem,
//! force-integer-ppem, instructions-alter-advance, lossless-transform,
//! converted, ClearType-optimised, last-resort), `unitsPerEm`, the
//! created / modified timestamps, the glyph-extent bbox, the `macStyle`
//! word, `lowestRecPPEM`, `fontDirectionHint`, `indexToLocFormat` (which
//! controls `loca` width), and `glyphDataFormat`.

use crate::parser::{read_i16, read_u16};
use crate::Error;

/// `head.flags` bits (§5.2.1).
pub const HEAD_FLAG_BASELINE_AT_Y0: u16 = 0x0001;
pub const HEAD_FLAG_LSB_AT_X0: u16 = 0x0002;
pub const HEAD_FLAG_INSTRUCTIONS_DEPEND_ON_PPEM: u16 = 0x0004;
pub const HEAD_FLAG_FORCE_INTEGER_PPEM: u16 = 0x0008;
pub const HEAD_FLAG_INSTRUCTIONS_ALTER_ADVANCE: u16 = 0x0010;
pub const HEAD_FLAG_LOSSLESS: u16 = 0x0800;
pub const HEAD_FLAG_CONVERTED: u16 = 0x1000;
pub const HEAD_FLAG_CLEARTYPE_OPTIMIZED: u16 = 0x2000;
pub const HEAD_FLAG_LAST_RESORT: u16 = 0x4000;

/// `head.macStyle` bits (§5.2.1).
pub const MAC_STYLE_BOLD: u16 = 0x0001;
pub const MAC_STYLE_ITALIC: u16 = 0x0002;
pub const MAC_STYLE_UNDERLINE: u16 = 0x0004;
pub const MAC_STYLE_OUTLINE: u16 = 0x0008;
pub const MAC_STYLE_SHADOW: u16 = 0x0010;
pub const MAC_STYLE_CONDENSED: u16 = 0x0020;
pub const MAC_STYLE_EXTENDED: u16 = 0x0040;

#[derive(Debug, Clone, Copy)]
pub struct HeadTable {
    /// `fontRevision` as a 16.16 fixed-point value (the table's `Fixed`
    /// version field for the font designer's revision number).
    pub font_revision: f32,
    /// The 16-bit `flags` word; decode through the `flag_*` predicates.
    pub flags: u16,
    pub units_per_em: u16,
    /// `created` timestamp — seconds since 1904-01-01 00:00 UTC.
    pub created: i64,
    /// `modified` timestamp — seconds since 1904-01-01 00:00 UTC.
    pub modified: i64,
    pub x_min: i16,
    pub y_min: i16,
    pub x_max: i16,
    pub y_max: i16,
    pub mac_style: u16,
    /// Smallest readable size in pixels (`lowestRecPPEM`).
    pub lowest_rec_ppem: u16,
    /// Deprecated directionality hint (set to 2 in modern fonts).
    pub font_direction_hint: i16,
    /// 0 = short (u16 offsets / 2), 1 = long (u32 offsets).
    pub index_to_loc_format: i16,
    /// `glyphDataFormat` (0 = current format).
    pub glyph_data_format: i16,
}

impl HeadTable {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        // Layout (offset / size / field):
        //   0  / 4 / version (Fixed, expect 1.0)
        //   4  / 4 / fontRevision (Fixed)
        //   8  / 4 / checkSumAdjustment
        //  12  / 4 / magicNumber (0x5F0F3CF5)
        //  16  / 2 / flags
        //  18  / 2 / unitsPerEm
        //  20  / 8 / created (LONGDATETIME)
        //  28  / 8 / modified
        //  36  / 2 / xMin
        //  38  / 2 / yMin
        //  40  / 2 / xMax
        //  42  / 2 / yMax
        //  44  / 2 / macStyle
        //  46  / 2 / lowestRecPPEM
        //  48  / 2 / fontDirectionHint
        //  50  / 2 / indexToLocFormat
        //  52  / 2 / glyphDataFormat
        if bytes.len() < 54 {
            return Err(Error::UnexpectedEof);
        }
        let units_per_em = read_u16(bytes, 18)?;
        if units_per_em == 0 {
            return Err(Error::BadStructure("head.unitsPerEm == 0"));
        }
        // fontRevision is a signed 16.16 Fixed.
        let rev_raw = i32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let font_revision = rev_raw as f32 / 65536.0;
        let flags = read_u16(bytes, 16)?;
        let created = i64::from_be_bytes([
            bytes[20], bytes[21], bytes[22], bytes[23], bytes[24], bytes[25], bytes[26], bytes[27],
        ]);
        let modified = i64::from_be_bytes([
            bytes[28], bytes[29], bytes[30], bytes[31], bytes[32], bytes[33], bytes[34], bytes[35],
        ]);
        let x_min = read_i16(bytes, 36)?;
        let y_min = read_i16(bytes, 38)?;
        let x_max = read_i16(bytes, 40)?;
        let y_max = read_i16(bytes, 42)?;
        let mac_style = read_u16(bytes, 44)?;
        let lowest_rec_ppem = read_u16(bytes, 46)?;
        let font_direction_hint = read_i16(bytes, 48)?;
        let index_to_loc_format = read_i16(bytes, 50)?;
        if index_to_loc_format != 0 && index_to_loc_format != 1 {
            return Err(Error::BadStructure("head.indexToLocFormat not 0/1"));
        }
        let glyph_data_format = read_i16(bytes, 52)?;
        Ok(Self {
            font_revision,
            flags,
            units_per_em,
            created,
            modified,
            x_min,
            y_min,
            x_max,
            y_max,
            mac_style,
            lowest_rec_ppem,
            font_direction_hint,
            index_to_loc_format,
            glyph_data_format,
        })
    }

    /// `flags` bit 0: the font baseline is at y = 0.
    pub fn flag_baseline_at_y0(&self) -> bool {
        self.flags & HEAD_FLAG_BASELINE_AT_Y0 != 0
    }

    /// `flags` bit 1: the left side-bearing point is at x = 0 (TrueType).
    pub fn flag_lsb_at_x0(&self) -> bool {
        self.flags & HEAD_FLAG_LSB_AT_X0 != 0
    }

    /// `flags` bit 4: instructions may alter advance width — advance widths
    /// might not scale linearly (drives `LTSH` / `hdmx` precomputation).
    pub fn flag_instructions_alter_advance(&self) -> bool {
        self.flags & HEAD_FLAG_INSTRUCTIONS_ALTER_ADVANCE != 0
    }

    /// `flags` bit 11: the font has been losslessly transformed /
    /// compressed (WOFF2, MicroType Express, …) so binary compatibility
    /// with the original is not guaranteed and `DSIG` may be invalidated.
    pub fn flag_lossless(&self) -> bool {
        self.flags & HEAD_FLAG_LOSSLESS != 0
    }

    /// `flags` bit 14: a Last-Resort font whose cmap glyphs are generic
    /// code-point-range symbols rather than true glyph support.
    pub fn flag_last_resort(&self) -> bool {
        self.flags & HEAD_FLAG_LAST_RESORT != 0
    }

    /// `macStyle` bit 0: Bold.
    pub fn mac_style_bold(&self) -> bool {
        self.mac_style & MAC_STYLE_BOLD != 0
    }

    /// `macStyle` bit 1: Italic.
    pub fn mac_style_italic(&self) -> bool {
        self.mac_style & MAC_STYLE_ITALIC != 0
    }

    /// `macStyle` bit 5: Condensed.
    pub fn mac_style_condensed(&self) -> bool {
        self.mac_style & MAC_STYLE_CONDENSED != 0
    }

    /// `macStyle` bit 6: Extended.
    pub fn mac_style_extended(&self) -> bool {
        self.mac_style & MAC_STYLE_EXTENDED != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_head(units: u16, loc_fmt: i16) -> Vec<u8> {
        let mut b = vec![0u8; 54];
        // version 1.0 + fontRevision (1.5) + checksum + magic.
        b[0..4].copy_from_slice(&0x00010000u32.to_be_bytes());
        b[4..8].copy_from_slice(&((1.5f32 * 65536.0) as i32).to_be_bytes()); // fontRevision
        b[12..16].copy_from_slice(&0x5F0F3CF5u32.to_be_bytes());
        b[16..18].copy_from_slice(&(HEAD_FLAG_BASELINE_AT_Y0 | HEAD_FLAG_LSB_AT_X0).to_be_bytes());
        b[18..20].copy_from_slice(&units.to_be_bytes());
        b[20..28].copy_from_slice(&3_000_000_000i64.to_be_bytes()); // created
        b[28..36].copy_from_slice(&3_100_000_000i64.to_be_bytes()); // modified
        b[36..38].copy_from_slice(&(-100i16).to_be_bytes());
        b[38..40].copy_from_slice(&(-200i16).to_be_bytes());
        b[40..42].copy_from_slice(&(1500i16).to_be_bytes());
        b[42..44].copy_from_slice(&(2000i16).to_be_bytes());
        b[44..46].copy_from_slice(&(MAC_STYLE_BOLD | MAC_STYLE_CONDENSED).to_be_bytes());
        b[46..48].copy_from_slice(&8u16.to_be_bytes()); // lowestRecPPEM
        b[48..50].copy_from_slice(&2i16.to_be_bytes()); // fontDirectionHint
        b[50..52].copy_from_slice(&loc_fmt.to_be_bytes());
        b[52..54].copy_from_slice(&0i16.to_be_bytes()); // glyphDataFormat
        b
    }

    #[test]
    fn parses_short_loca_format() {
        let h = HeadTable::parse(&build_head(1024, 0)).unwrap();
        assert_eq!(h.units_per_em, 1024);
        assert_eq!(h.index_to_loc_format, 0);
        assert_eq!(h.x_min, -100);
        assert_eq!(h.y_max, 2000);
    }

    #[test]
    fn parses_full_field_set() {
        let h = HeadTable::parse(&build_head(2048, 1)).unwrap();
        assert!((h.font_revision - 1.5).abs() < 1e-4);
        assert!(h.flag_baseline_at_y0());
        assert!(h.flag_lsb_at_x0());
        assert!(!h.flag_lossless());
        assert!(!h.flag_last_resort());
        assert_eq!(h.created, 3_000_000_000);
        assert_eq!(h.modified, 3_100_000_000);
        assert_eq!(h.lowest_rec_ppem, 8);
        assert_eq!(h.font_direction_hint, 2);
        assert_eq!(h.glyph_data_format, 0);
        assert!(h.mac_style_bold());
        assert!(h.mac_style_condensed());
        assert!(!h.mac_style_italic());
        assert!(!h.mac_style_extended());
    }

    #[test]
    fn parses_long_loca_format() {
        let h = HeadTable::parse(&build_head(2048, 1)).unwrap();
        assert_eq!(h.units_per_em, 2048);
        assert_eq!(h.index_to_loc_format, 1);
    }

    #[test]
    fn rejects_zero_upem() {
        assert!(HeadTable::parse(&build_head(0, 0)).is_err());
    }

    #[test]
    fn rejects_bad_loc_format() {
        assert!(HeadTable::parse(&build_head(1024, 2)).is_err());
    }
}
