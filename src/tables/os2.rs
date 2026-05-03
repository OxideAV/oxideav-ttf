//! `OS/2` — Windows-specific metrics + classification.
//!
//! Multiple versions exist (0..5). All start with a 78-byte common prefix
//! that contains everything round 1 needs except the typo metrics, which
//! land at offset 68/70/72 in version >= 0.

use crate::parser::{read_i16, read_u16};
use crate::Error;

#[derive(Debug, Clone, Copy)]
pub struct Os2Table {
    pub us_weight_class: u16,
    pub fs_selection: u16,
    pub s_typo_ascender: Option<i16>,
    pub s_typo_descender: Option<i16>,
    pub s_typo_line_gap: Option<i16>,
    pub sx_height: Option<i16>,
    pub s_cap_height: Option<i16>,
}

impl Os2Table {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 4 {
            return Err(Error::UnexpectedEof);
        }
        let version = read_u16(bytes, 0)?;
        // Common-prefix offsets:
        //   0  / 2 / version
        //   2  / 2 / xAvgCharWidth
        //   4  / 2 / usWeightClass
        //   6  / 2 / usWidthClass
        //   8  / 2 / fsType
        //  10  / 10 * 2 / subscript/superscript/strikeout fields
        //  30  / 2 / sFamilyClass
        //  32  / 10 / panose
        //  42  / 16 / ulUnicodeRange (4 * u32)
        //  58  / 4 / achVendID
        //  62  / 2 / fsSelection
        //  64  / 2 / usFirstCharIndex
        //  66  / 2 / usLastCharIndex
        //  68  / 2 / sTypoAscender
        //  70  / 2 / sTypoDescender
        //  72  / 2 / sTypoLineGap
        //  74  / 2 / usWinAscent
        //  76  / 2 / usWinDescent
        // Version 1 adds ulCodePageRange1/2 (8 bytes) starting at 78.
        // Version 2..5 adds sxHeight, sCapHeight at 86, 88.
        if bytes.len() < 78 {
            return Err(Error::UnexpectedEof);
        }
        let us_weight_class = read_u16(bytes, 4)?;
        let fs_selection = read_u16(bytes, 62)?;
        let s_typo_ascender = Some(read_i16(bytes, 68)?);
        let s_typo_descender = Some(read_i16(bytes, 70)?);
        let s_typo_line_gap = Some(read_i16(bytes, 72)?);

        let mut sx_height = None;
        let mut s_cap_height = None;
        if version >= 2 && bytes.len() >= 90 {
            sx_height = Some(read_i16(bytes, 86)?);
            s_cap_height = Some(read_i16(bytes, 88)?);
        }
        Ok(Self {
            us_weight_class,
            fs_selection,
            s_typo_ascender,
            s_typo_descender,
            s_typo_line_gap,
            sx_height,
            s_cap_height,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v0_minimum() {
        let mut b = vec![0u8; 78];
        b[0..2].copy_from_slice(&0u16.to_be_bytes()); // version
        b[4..6].copy_from_slice(&500u16.to_be_bytes()); // weight
        b[62..64].copy_from_slice(&64u16.to_be_bytes()); // fsSelection
        b[68..70].copy_from_slice(&(1900i16).to_be_bytes());
        b[70..72].copy_from_slice(&(-500i16).to_be_bytes());
        b[72..74].copy_from_slice(&(0i16).to_be_bytes());
        let t = Os2Table::parse(&b).unwrap();
        assert_eq!(t.us_weight_class, 500);
        assert_eq!(t.s_typo_ascender, Some(1900));
        assert!(t.sx_height.is_none());
    }
}
