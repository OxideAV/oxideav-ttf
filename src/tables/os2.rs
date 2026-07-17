//! `OS/2` — OS/2 and Windows metrics table (ISO/IEC 14496-22:2019 §5.2.3).
//!
//! Versions 0..5 all share an 78-byte common prefix and grow a versioned
//! tail. This decoder reads every field the spec defines, surfacing the
//! later-version fields as `Option` (present only when the table's version
//! and length cover them):
//!
//! ```text
//!   0   uint16 version
//!   2   int16  xAvgCharWidth
//!   4   uint16 usWeightClass
//!   6   uint16 usWidthClass
//!   8   uint16 fsType                  (embedding permissions)
//!  10   int16  ySubscriptXSize .. yStrikeoutPosition  (10 fields)
//!  30   int16  sFamilyClass
//!  32   uint8  panose[10]
//!  42   uint32 ulUnicodeRange1..4      (16 bytes)
//!  58   Tag    achVendID[4]
//!  62   uint16 fsSelection
//!  64   uint16 usFirstCharIndex
//!  66   uint16 usLastCharIndex
//!  68   int16  sTypoAscender
//!  70   int16  sTypoDescender
//!  72   int16  sTypoLineGap
//!  74   uint16 usWinAscent
//!  76   uint16 usWinDescent
//!  -- version >= 1 --
//!  78   uint32 ulCodePageRange1
//!  82   uint32 ulCodePageRange2
//!  -- version >= 2 --
//!  86   int16  sxHeight
//!  88   int16  sCapHeight
//!  90   uint16 usDefaultChar
//!  92   uint16 usBreakChar
//!  94   uint16 usMaxContext
//!  -- version >= 5 --
//!  96   uint16 usLowerOpticalPointSize
//!  98   uint16 usUpperOpticalPointSize
//! ```

use crate::parser::{read_i16, read_u16, read_u32};
use crate::Error;

/// `fsType` embedding-permission bits (§5.2.3 `fsType`). Bit 0 is reserved
/// (must be 0); the low 4 bits 1..3 are the mutually-exclusive licensing
/// level, and bits 8/9 are independent flags.
pub const FSTYPE_RESTRICTED_LICENSE: u16 = 0x0002;
pub const FSTYPE_PREVIEW_PRINT: u16 = 0x0004;
pub const FSTYPE_EDITABLE: u16 = 0x0008;
pub const FSTYPE_NO_SUBSETTING: u16 = 0x0100;
pub const FSTYPE_BITMAP_ONLY: u16 = 0x0200;

/// `fsSelection` bits (§5.2.3 `fsSelection`).
pub const FSSELECTION_ITALIC: u16 = 0x0001;
pub const FSSELECTION_UNDERSCORE: u16 = 0x0002;
pub const FSSELECTION_NEGATIVE: u16 = 0x0004;
pub const FSSELECTION_OUTLINED: u16 = 0x0008;
pub const FSSELECTION_STRIKEOUT: u16 = 0x0010;
pub const FSSELECTION_BOLD: u16 = 0x0020;
pub const FSSELECTION_REGULAR: u16 = 0x0040;
pub const FSSELECTION_USE_TYPO_METRICS: u16 = 0x0080;
pub const FSSELECTION_WWS: u16 = 0x0100;
pub const FSSELECTION_OBLIQUE: u16 = 0x0200;

#[derive(Debug, Clone, Copy)]
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub struct Os2Table {
    pub version: u16,
    pub x_avg_char_width: i16,
    pub us_weight_class: u16,
    pub us_width_class: u16,
    pub fs_type: u16,
    pub y_subscript_x_size: i16,
    pub y_subscript_y_size: i16,
    pub y_subscript_x_offset: i16,
    pub y_subscript_y_offset: i16,
    pub y_superscript_x_size: i16,
    pub y_superscript_y_size: i16,
    pub y_superscript_x_offset: i16,
    pub y_superscript_y_offset: i16,
    pub y_strikeout_size: i16,
    pub y_strikeout_position: i16,
    pub s_family_class: i16,
    pub panose: [u8; 10],
    pub ul_unicode_range1: u32,
    pub ul_unicode_range2: u32,
    pub ul_unicode_range3: u32,
    pub ul_unicode_range4: u32,
    pub ach_vend_id: [u8; 4],
    pub fs_selection: u16,
    pub us_first_char_index: u16,
    pub us_last_char_index: u16,
    pub s_typo_ascender: Option<i16>,
    pub s_typo_descender: Option<i16>,
    pub s_typo_line_gap: Option<i16>,
    pub us_win_ascent: Option<u16>,
    pub us_win_descent: Option<u16>,
    // version >= 1
    pub ul_code_page_range1: Option<u32>,
    pub ul_code_page_range2: Option<u32>,
    // version >= 2
    pub sx_height: Option<i16>,
    pub s_cap_height: Option<i16>,
    pub us_default_char: Option<u16>,
    pub us_break_char: Option<u16>,
    pub us_max_context: Option<u16>,
    // version >= 5
    pub us_lower_optical_point_size: Option<u16>,
    pub us_upper_optical_point_size: Option<u16>,
}

impl Os2Table {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 78 {
            return Err(Error::UnexpectedEof);
        }
        let version = read_u16(bytes, 0)?;
        let mut panose = [0u8; 10];
        panose.copy_from_slice(&bytes[32..42]);
        let mut ach_vend_id = [0u8; 4];
        ach_vend_id.copy_from_slice(&bytes[58..62]);

        // sTypoAscender/Descender/LineGap and usWinAscent/Descent are at
        // 68..78, inside the 78-byte common prefix.
        let s_typo_ascender = Some(read_i16(bytes, 68)?);
        let s_typo_descender = Some(read_i16(bytes, 70)?);
        let s_typo_line_gap = Some(read_i16(bytes, 72)?);
        let us_win_ascent = Some(read_u16(bytes, 74)?);
        let us_win_descent = Some(read_u16(bytes, 76)?);

        // Versioned tail — only read fields the version *and* the on-wire
        // length cover (some real fonts ship a shorter table than their
        // declared version's full size; clamp on length too).
        let has_v1 = version >= 1 && bytes.len() >= 86;
        let (ul_code_page_range1, ul_code_page_range2) = if has_v1 {
            (Some(read_u32(bytes, 78)?), Some(read_u32(bytes, 82)?))
        } else {
            (None, None)
        };

        let has_v2 = version >= 2 && bytes.len() >= 96;
        let (sx_height, s_cap_height, us_default_char, us_break_char, us_max_context) = if has_v2 {
            (
                Some(read_i16(bytes, 86)?),
                Some(read_i16(bytes, 88)?),
                Some(read_u16(bytes, 90)?),
                Some(read_u16(bytes, 92)?),
                Some(read_u16(bytes, 94)?),
            )
        } else {
            (None, None, None, None, None)
        };

        let has_v5 = version >= 5 && bytes.len() >= 100;
        let (us_lower_optical_point_size, us_upper_optical_point_size) = if has_v5 {
            (Some(read_u16(bytes, 96)?), Some(read_u16(bytes, 98)?))
        } else {
            (None, None)
        };

        Ok(Self {
            version,
            x_avg_char_width: read_i16(bytes, 2)?,
            us_weight_class: read_u16(bytes, 4)?,
            us_width_class: read_u16(bytes, 6)?,
            fs_type: read_u16(bytes, 8)?,
            y_subscript_x_size: read_i16(bytes, 10)?,
            y_subscript_y_size: read_i16(bytes, 12)?,
            y_subscript_x_offset: read_i16(bytes, 14)?,
            y_subscript_y_offset: read_i16(bytes, 16)?,
            y_superscript_x_size: read_i16(bytes, 18)?,
            y_superscript_y_size: read_i16(bytes, 20)?,
            y_superscript_x_offset: read_i16(bytes, 22)?,
            y_superscript_y_offset: read_i16(bytes, 24)?,
            y_strikeout_size: read_i16(bytes, 26)?,
            y_strikeout_position: read_i16(bytes, 28)?,
            s_family_class: read_i16(bytes, 30)?,
            panose,
            ul_unicode_range1: read_u32(bytes, 42)?,
            ul_unicode_range2: read_u32(bytes, 46)?,
            ul_unicode_range3: read_u32(bytes, 50)?,
            ul_unicode_range4: read_u32(bytes, 54)?,
            ach_vend_id,
            fs_selection: read_u16(bytes, 62)?,
            us_first_char_index: read_u16(bytes, 64)?,
            us_last_char_index: read_u16(bytes, 66)?,
            s_typo_ascender,
            s_typo_descender,
            s_typo_line_gap,
            us_win_ascent,
            us_win_descent,
            ul_code_page_range1,
            ul_code_page_range2,
            sx_height,
            s_cap_height,
            us_default_char,
            us_break_char,
            us_max_context,
            us_lower_optical_point_size,
            us_upper_optical_point_size,
        })
    }

    /// `achVendID` as a trimmed ASCII string (the 4-byte registered vendor
    /// tag, e.g. `"GOOG"`). Trailing spaces / NULs are stripped; returns
    /// `None` if the bytes are not valid ASCII.
    pub fn vendor_id(&self) -> Option<&str> {
        let s = core::str::from_utf8(&self.ach_vend_id).ok()?;
        Some(s.trim_end_matches([' ', '\0']))
    }

    /// `true` if the font is marked **installable embedding** — no `fsType`
    /// licence-restriction bit is set (the low 4 bits 1..3 are all clear).
    /// This is the most permissive embedding state (§5.2.3 `fsType`).
    pub fn embedding_installable(&self) -> bool {
        self.fs_type & 0x000E == 0
    }

    /// `true` if `fsType` sets the **restricted licence embedding** bit —
    /// the font must not be embedded or exchanged without permission.
    pub fn embedding_restricted(&self) -> bool {
        self.fs_type & FSTYPE_RESTRICTED_LICENSE != 0
    }

    /// `true` if `fsType` permits **preview & print** embedding only.
    pub fn embedding_preview_print(&self) -> bool {
        self.fs_type & FSTYPE_PREVIEW_PRINT != 0
    }

    /// `true` if `fsType` permits **editable** embedding.
    pub fn embedding_editable(&self) -> bool {
        self.fs_type & FSTYPE_EDITABLE != 0
    }

    /// `true` if `fsType` forbids subsetting the embedded font.
    pub fn embedding_no_subsetting(&self) -> bool {
        self.fs_type & FSTYPE_NO_SUBSETTING != 0
    }

    /// `true` if `fsType` restricts embedding to bitmap data only.
    pub fn embedding_bitmap_only(&self) -> bool {
        self.fs_type & FSTYPE_BITMAP_ONLY != 0
    }

    /// `true` if the `fsSelection` ITALIC bit is set.
    pub fn is_italic(&self) -> bool {
        self.fs_selection & FSSELECTION_ITALIC != 0
    }

    /// `true` if the `fsSelection` BOLD bit is set.
    pub fn is_bold(&self) -> bool {
        self.fs_selection & FSSELECTION_BOLD != 0
    }

    /// `true` if the `fsSelection` REGULAR bit is set.
    pub fn is_regular(&self) -> bool {
        self.fs_selection & FSSELECTION_REGULAR != 0
    }

    /// `true` if the `fsSelection` USE_TYPO_METRICS bit is set — clients
    /// should use the `sTypoAscender` / `sTypoDescender` / `sTypoLineGap`
    /// values as the default line metrics.
    pub fn use_typo_metrics(&self) -> bool {
        self.fs_selection & FSSELECTION_USE_TYPO_METRICS != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_v0() -> Vec<u8> {
        let mut b = vec![0u8; 78];
        b[0..2].copy_from_slice(&0u16.to_be_bytes()); // version
        b[2..4].copy_from_slice(&(1234i16).to_be_bytes()); // xAvgCharWidth
        b[4..6].copy_from_slice(&500u16.to_be_bytes()); // weight
        b[6..8].copy_from_slice(&5u16.to_be_bytes()); // width class
        b[8..10].copy_from_slice(&FSTYPE_EDITABLE.to_be_bytes()); // fsType
        b[32..42].copy_from_slice(&[2, 0, 5, 3, 0, 0, 0, 0, 0, 0]); // panose
        b[42..46].copy_from_slice(&0x0000_0001u32.to_be_bytes()); // uniRange1
        b[58..62].copy_from_slice(b"GOOG"); // achVendID
        b[62..64].copy_from_slice(&(FSSELECTION_BOLD | FSSELECTION_ITALIC).to_be_bytes());
        b[64..66].copy_from_slice(&0x0020u16.to_be_bytes()); // firstCharIndex
        b[66..68].copy_from_slice(&0xFFFFu16.to_be_bytes()); // lastCharIndex
        b[68..70].copy_from_slice(&(1900i16).to_be_bytes());
        b[70..72].copy_from_slice(&(-500i16).to_be_bytes());
        b[72..74].copy_from_slice(&(0i16).to_be_bytes());
        b[74..76].copy_from_slice(&1800u16.to_be_bytes()); // winAscent
        b[76..78].copy_from_slice(&400u16.to_be_bytes()); // winDescent
        b
    }

    #[test]
    fn parses_v0_full_prefix() {
        let t = Os2Table::parse(&build_v0()).unwrap();
        assert_eq!(t.version, 0);
        assert_eq!(t.x_avg_char_width, 1234);
        assert_eq!(t.us_weight_class, 500);
        assert_eq!(t.us_width_class, 5);
        assert_eq!(t.panose, [2, 0, 5, 3, 0, 0, 0, 0, 0, 0]);
        assert_eq!(t.ul_unicode_range1, 1);
        assert_eq!(t.vendor_id(), Some("GOOG"));
        assert_eq!(t.us_first_char_index, 0x0020);
        assert_eq!(t.us_last_char_index, 0xFFFF);
        assert_eq!(t.us_win_ascent, Some(1800));
        assert_eq!(t.us_win_descent, Some(400));
        assert!(t.is_bold());
        assert!(t.is_italic());
        assert!(!t.is_regular());
        // fsType EDITABLE -> not installable, editable true.
        assert!(!t.embedding_installable());
        assert!(t.embedding_editable());
        assert!(!t.embedding_restricted());
        // v0 has no codepage / xHeight / optical-size fields.
        assert!(t.ul_code_page_range1.is_none());
        assert!(t.sx_height.is_none());
        assert!(t.us_lower_optical_point_size.is_none());
    }

    #[test]
    fn parses_v1_codepage_range() {
        let mut b = build_v0();
        b[0..2].copy_from_slice(&1u16.to_be_bytes());
        b.resize(86, 0);
        b[78..82].copy_from_slice(&0x0000_0003u32.to_be_bytes()); // cp range1
        b[82..86].copy_from_slice(&0x0000_0001u32.to_be_bytes()); // cp range2
        let t = Os2Table::parse(&b).unwrap();
        assert_eq!(t.version, 1);
        assert_eq!(t.ul_code_page_range1, Some(3));
        assert_eq!(t.ul_code_page_range2, Some(1));
        assert!(t.sx_height.is_none());
    }

    #[test]
    fn parses_v2_extra_fields() {
        let mut b = build_v0();
        b[0..2].copy_from_slice(&2u16.to_be_bytes());
        b.resize(96, 0);
        b[86..88].copy_from_slice(&(540i16).to_be_bytes()); // sxHeight
        b[88..90].copy_from_slice(&(720i16).to_be_bytes()); // sCapHeight
        b[90..92].copy_from_slice(&0u16.to_be_bytes()); // defaultChar
        b[92..94].copy_from_slice(&0x0020u16.to_be_bytes()); // breakChar
        b[94..96].copy_from_slice(&3u16.to_be_bytes()); // maxContext
        let t = Os2Table::parse(&b).unwrap();
        assert_eq!(t.sx_height, Some(540));
        assert_eq!(t.s_cap_height, Some(720));
        assert_eq!(t.us_break_char, Some(0x0020));
        assert_eq!(t.us_max_context, Some(3));
        assert!(t.us_lower_optical_point_size.is_none());
    }

    #[test]
    fn parses_v5_optical_sizes() {
        let mut b = build_v0();
        b[0..2].copy_from_slice(&5u16.to_be_bytes());
        b.resize(100, 0);
        b[96..98].copy_from_slice(&80u16.to_be_bytes()); // lower (8.0pt * 10)
        b[98..100].copy_from_slice(&240u16.to_be_bytes()); // upper (24.0pt * 10)
        let t = Os2Table::parse(&b).unwrap();
        assert_eq!(t.us_lower_optical_point_size, Some(80));
        assert_eq!(t.us_upper_optical_point_size, Some(240));
    }

    #[test]
    fn version_declared_but_table_truncated() {
        // version says 2 but only the 78-byte prefix is present: the
        // tail fields must stay None rather than over-read.
        let mut b = build_v0();
        b[0..2].copy_from_slice(&2u16.to_be_bytes());
        let t = Os2Table::parse(&b).unwrap();
        assert_eq!(t.version, 2);
        assert!(t.sx_height.is_none());
        assert!(t.ul_code_page_range1.is_none());
    }

    #[test]
    fn rejects_short_table() {
        assert!(matches!(
            Os2Table::parse(&[0u8; 40]),
            Err(Error::UnexpectedEof)
        ));
    }
}
