//! `maxp` — maximum profile (ISO/IEC 14496-22:2019 §5.2.5).
//!
//! Two versions: **v0.5** (6 bytes — `version` + `numGlyphs`, used by
//! CFF-outline OTF fonts) and **v1.0** (32 bytes — adds the TrueType
//! rasteriser-sizing maxima). The v1.0 statistics (`maxPoints`,
//! `maxContours`, composite limits, the bytecode-interpreter resource
//! caps, and `maxComponentDepth`) are surfaced as `Option`, populated only
//! for a v1.0 table. They let a rasteriser pre-size its point / contour /
//! stack buffers and validate composite nesting.

use crate::parser::{read_u16, read_u32};
use crate::Error;

/// The v1.0 TrueType maxima (offsets 6..32 of a 32-byte `maxp`).
#[derive(Debug, Clone, Copy)]
pub struct MaxpV1 {
    pub max_points: u16,
    pub max_contours: u16,
    pub max_composite_points: u16,
    pub max_composite_contours: u16,
    pub max_zones: u16,
    pub max_twilight_points: u16,
    pub max_storage: u16,
    pub max_function_defs: u16,
    pub max_instruction_defs: u16,
    pub max_stack_elements: u16,
    pub max_size_of_instructions: u16,
    pub max_component_elements: u16,
    pub max_component_depth: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct MaxpTable {
    pub num_glyphs: u16,
    /// The TrueType v1.0 maxima, present only for a 32-byte v1.0 table.
    pub v1: Option<MaxpV1>,
}

impl MaxpTable {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        // Layout differs by version:
        //   v0.5 (post-only, 6 bytes): used by OTF/CFF fonts.
        //   v1.0 (TT, 32 bytes):       used by TT-outline fonts.
        // Both start with `version (Fixed)` at offset 0 and `numGlyphs` (u16)
        // at offset 4.
        if bytes.len() < 6 {
            return Err(Error::UnexpectedEof);
        }
        let version = read_u32(bytes, 0)?;
        if version != 0x00005000 && version != 0x00010000 {
            return Err(Error::BadStructure("maxp.version not 0.5 or 1.0"));
        }
        let num_glyphs = read_u16(bytes, 4)?;
        if num_glyphs == 0 {
            return Err(Error::BadStructure("maxp.numGlyphs == 0"));
        }
        // v1.0 carries 13 u16 maxima from offset 6 (total 32 bytes). Read
        // them only when the version says v1.0 *and* the table is long
        // enough (some fonts mis-declare; clamp on length).
        let v1 = if version == 0x00010000 && bytes.len() >= 32 {
            Some(MaxpV1 {
                max_points: read_u16(bytes, 6)?,
                max_contours: read_u16(bytes, 8)?,
                max_composite_points: read_u16(bytes, 10)?,
                max_composite_contours: read_u16(bytes, 12)?,
                max_zones: read_u16(bytes, 14)?,
                max_twilight_points: read_u16(bytes, 16)?,
                max_storage: read_u16(bytes, 18)?,
                max_function_defs: read_u16(bytes, 20)?,
                max_instruction_defs: read_u16(bytes, 22)?,
                max_stack_elements: read_u16(bytes, 24)?,
                max_size_of_instructions: read_u16(bytes, 26)?,
                max_component_elements: read_u16(bytes, 28)?,
                max_component_depth: read_u16(bytes, 30)?,
            })
        } else {
            None
        };
        Ok(Self { num_glyphs, v1 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v05() {
        let mut b = vec![0u8; 6];
        b[0..4].copy_from_slice(&0x00005000u32.to_be_bytes());
        b[4..6].copy_from_slice(&(123u16).to_be_bytes());
        let m = MaxpTable::parse(&b).unwrap();
        assert_eq!(m.num_glyphs, 123);
        // A v0.5 (CFF) table carries no TrueType maxima.
        assert!(m.v1.is_none());
    }

    #[test]
    fn parses_v10() {
        let mut b = vec![0u8; 32];
        b[0..4].copy_from_slice(&0x00010000u32.to_be_bytes());
        b[4..6].copy_from_slice(&(4567u16).to_be_bytes());
        b[6..8].copy_from_slice(&(250u16).to_be_bytes()); // maxPoints
        b[8..10].copy_from_slice(&(12u16).to_be_bytes()); // maxContours
        b[24..26].copy_from_slice(&(800u16).to_be_bytes()); // maxStackElements
        b[28..30].copy_from_slice(&(5u16).to_be_bytes()); // maxComponentElements
        b[30..32].copy_from_slice(&(2u16).to_be_bytes()); // maxComponentDepth
        let m = MaxpTable::parse(&b).unwrap();
        assert_eq!(m.num_glyphs, 4567);
        let v1 = m.v1.expect("v1.0 maxima present");
        assert_eq!(v1.max_points, 250);
        assert_eq!(v1.max_contours, 12);
        assert_eq!(v1.max_stack_elements, 800);
        assert_eq!(v1.max_component_elements, 5);
        assert_eq!(v1.max_component_depth, 2);
    }

    #[test]
    fn v10_declared_but_truncated_keeps_v1_none() {
        // version says 1.0 but only the 6-byte prefix is present.
        let mut b = vec![0u8; 6];
        b[0..4].copy_from_slice(&0x00010000u32.to_be_bytes());
        b[4..6].copy_from_slice(&(10u16).to_be_bytes());
        let m = MaxpTable::parse(&b).unwrap();
        assert_eq!(m.num_glyphs, 10);
        assert!(m.v1.is_none());
    }

    #[test]
    fn rejects_bad_version() {
        let mut b = vec![0u8; 32];
        b[0..4].copy_from_slice(&0xFFFFFFFFu32.to_be_bytes());
        b[4..6].copy_from_slice(&(1u16).to_be_bytes());
        assert!(MaxpTable::parse(&b).is_err());
    }
}
