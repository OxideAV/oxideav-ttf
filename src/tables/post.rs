//! `post` — PostScript info. Round 1 only needs `italicAngle` and the
//! underline metrics.

use crate::parser::{read_i16, read_i32};
use crate::Error;

#[derive(Debug, Clone, Copy)]
pub struct PostTable {
    pub italic_angle: f32,
    pub underline_position: i16,
    pub underline_thickness: i16,
    pub is_fixed_pitch: bool,
}

impl PostTable {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        // Layout (common prefix; format specifies what follows):
        //   0  / 4 / version (Fixed)
        //   4  / 4 / italicAngle (Fixed 16.16)
        //   8  / 2 / underlinePosition
        //  10  / 2 / underlineThickness
        //  12  / 4 / isFixedPitch (u32; bool)
        //  16  / 4 / minMemType42
        //  20  / 4 / maxMemType42
        //  24  / 4 / minMemType1
        //  28  / 4 / maxMemType1
        if bytes.len() < 32 {
            return Err(Error::UnexpectedEof);
        }
        let italic_raw = read_i32(bytes, 4)?;
        let italic_angle = italic_raw as f32 / 65536.0;
        let underline_position = read_i16(bytes, 8)?;
        let underline_thickness = read_i16(bytes, 10)?;
        // isFixedPitch is a u32 boolean — 0 = proportional, !=0 = monospace.
        let is_fixed_pitch = u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) != 0;
        Ok(Self {
            italic_angle,
            underline_position,
            underline_thickness,
            is_fixed_pitch,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal() {
        let mut b = vec![0u8; 32];
        b[0..4].copy_from_slice(&0x00020000u32.to_be_bytes());
        // italicAngle = -10.0 (Fixed: -10 * 65536)
        b[4..8].copy_from_slice(&((-10i32) << 16).to_be_bytes());
        b[8..10].copy_from_slice(&(-100i16).to_be_bytes());
        b[10..12].copy_from_slice(&50i16.to_be_bytes());
        b[12..16].copy_from_slice(&1u32.to_be_bytes());
        let p = PostTable::parse(&b).unwrap();
        assert!((p.italic_angle - (-10.0)).abs() < 0.001);
        assert_eq!(p.underline_position, -100);
        assert_eq!(p.underline_thickness, 50);
        assert!(p.is_fixed_pitch);
    }
}
