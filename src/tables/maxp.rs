//! `maxp` — maximum profile. Round 1 only needs `numGlyphs`.

use crate::parser::{read_u16, read_u32};
use crate::Error;

#[derive(Debug, Clone, Copy)]
pub struct MaxpTable {
    pub num_glyphs: u16,
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
        Ok(Self { num_glyphs })
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
        assert_eq!(MaxpTable::parse(&b).unwrap().num_glyphs, 123);
    }

    #[test]
    fn parses_v10() {
        let mut b = vec![0u8; 32];
        b[0..4].copy_from_slice(&0x00010000u32.to_be_bytes());
        b[4..6].copy_from_slice(&(4567u16).to_be_bytes());
        assert_eq!(MaxpTable::parse(&b).unwrap().num_glyphs, 4567);
    }

    #[test]
    fn rejects_bad_version() {
        let mut b = vec![0u8; 32];
        b[0..4].copy_from_slice(&0xFFFFFFFFu32.to_be_bytes());
        b[4..6].copy_from_slice(&(1u16).to_be_bytes());
        assert!(MaxpTable::parse(&b).is_err());
    }
}
