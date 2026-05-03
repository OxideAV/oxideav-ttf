//! `loca` — glyph-data offsets into `glyf`.
//!
//! Two encodings:
//!  - Short (`head.indexToLocFormat == 0`): `(numGlyphs + 1) * u16`,
//!    each value × 2 yielding a glyf byte offset.
//!  - Long  (`head.indexToLocFormat == 1`): `(numGlyphs + 1) * u32`.
//!
//! For glyph `g`, the data spans bytes `[loca[g] .. loca[g+1])` within
//! `glyf`. An empty range (`loca[g] == loca[g+1]`) means an empty glyph.

use core::ops::Range;

use crate::parser::{read_u16, read_u32};
use crate::Error;

#[derive(Debug, Clone)]
pub struct LocaTable<'a> {
    bytes: &'a [u8],
    num_glyphs: u16,
    long: bool,
}

impl<'a> LocaTable<'a> {
    pub fn parse(
        bytes: &'a [u8],
        num_glyphs: u16,
        index_to_loc_format: i16,
    ) -> Result<Self, Error> {
        let long = index_to_loc_format == 1;
        let entry_size = if long { 4 } else { 2 };
        let needed = (num_glyphs as usize + 1) * entry_size;
        if bytes.len() < needed {
            return Err(Error::UnexpectedEof);
        }
        Ok(Self {
            bytes,
            num_glyphs,
            long,
        })
    }

    /// Byte range in `glyf` for `glyph_id`. Empty range = blank glyph.
    pub fn glyph_range(&self, glyph_id: u16) -> Result<Range<usize>, Error> {
        if glyph_id >= self.num_glyphs {
            return Err(Error::GlyphOutOfRange(glyph_id));
        }
        let (a, b) = if self.long {
            (
                read_u32(self.bytes, glyph_id as usize * 4)? as usize,
                read_u32(self.bytes, (glyph_id as usize + 1) * 4)? as usize,
            )
        } else {
            (
                read_u16(self.bytes, glyph_id as usize * 2)? as usize * 2,
                read_u16(self.bytes, (glyph_id as usize + 1) * 2)? as usize * 2,
            )
        };
        if b < a {
            return Err(Error::BadLocaOffset);
        }
        Ok(a..b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_format_round_trip() {
        // 3 glyphs ⇒ 4 short entries; values are halved offsets.
        // glyph 0 = empty (0..0), glyph 1 = [0..20), glyph 2 = [20..30).
        let mut b = Vec::new();
        for v in [0u16, 0, 10, 15] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        let l = LocaTable::parse(&b, 3, 0).unwrap();
        assert_eq!(l.glyph_range(0).unwrap(), 0..0);
        assert_eq!(l.glyph_range(1).unwrap(), 0..20);
        assert_eq!(l.glyph_range(2).unwrap(), 20..30);
        assert!(l.glyph_range(3).is_err());
    }

    #[test]
    fn long_format_round_trip() {
        let mut b = Vec::new();
        for v in [0u32, 5, 25, 30] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        let l = LocaTable::parse(&b, 3, 1).unwrap();
        assert_eq!(l.glyph_range(0).unwrap(), 0..5);
        assert_eq!(l.glyph_range(1).unwrap(), 5..25);
        assert_eq!(l.glyph_range(2).unwrap(), 25..30);
    }
}
