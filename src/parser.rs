//! sfnt header + table directory parser.
//!
//! The sfnt container starts with a 12-byte header (sfnt version + a
//! 16-bit table count + 6 bytes of binary-search hints) followed by
//! `numTables * 16` bytes of table records. Each record is
//! `(tag[4], checksum, offset, length)`; we only use `tag`, `offset`,
//! and `length` here. Spec: Microsoft OpenType, "Organization of an
//! OpenType Font" / "Table directory".

use crate::Error;

/// Maximum table count we will accept in the sfnt header. 1024 is well
/// above any real-world font (real fonts top out around 30-40 tables);
/// the cap exists purely to bound how much we read on malformed input.
const MAX_TABLES: u16 = 1024;

/// Parsed sfnt table directory: `(tag, offset, length)` entries.
#[derive(Debug, Clone)]
pub(crate) struct TableDirectory {
    entries: Vec<TableRecord>,
}

#[derive(Debug, Clone, Copy)]
struct TableRecord {
    tag: [u8; 4],
    offset: u32,
    length: u32,
}

impl TableDirectory {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 12 {
            return Err(Error::UnexpectedEof);
        }
        let version = read_u32(bytes, 0)?;
        match version {
            // 'true', 'OTTO', or sfnt version 1.0.
            0x00010000 | 0x4F54544F /* OTTO */ | 0x74727565 /* true */ => {}
            _ => return Err(Error::BadMagic),
        }
        let num_tables = read_u16(bytes, 4)?;
        if num_tables == 0 || num_tables > MAX_TABLES {
            return Err(Error::BadHeader);
        }
        // searchRange / entrySelector / rangeShift skipped.

        let dir_end = 12usize
            .checked_add(num_tables as usize * 16)
            .ok_or(Error::BadHeader)?;
        if bytes.len() < dir_end {
            return Err(Error::UnexpectedEof);
        }

        let mut entries = Vec::with_capacity(num_tables as usize);
        for i in 0..num_tables as usize {
            let off = 12 + i * 16;
            let tag = [bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]];
            // checksum at off+4, skipped.
            let offset = read_u32(bytes, off + 8)?;
            let length = read_u32(bytes, off + 12)?;
            // Validate offset + length lie inside the file.
            let end = (offset as u64)
                .checked_add(length as u64)
                .ok_or(Error::BadOffset)?;
            if end > bytes.len() as u64 {
                return Err(Error::BadOffset);
            }
            entries.push(TableRecord {
                tag,
                offset,
                length,
            });
        }
        Ok(Self { entries })
    }

    /// Return the slice for `tag`, or `None` if the table is absent.
    pub(crate) fn find<'a>(&self, tag: &[u8; 4], bytes: &'a [u8]) -> Option<&'a [u8]> {
        for rec in &self.entries {
            if rec.tag == *tag {
                let start = rec.offset as usize;
                let end = start + rec.length as usize;
                return Some(&bytes[start..end]);
            }
        }
        None
    }

    /// Like `find` but errors with `Error::MissingTable` when absent.
    pub(crate) fn required<'a>(
        &self,
        tag: &'static [u8; 4],
        bytes: &'a [u8],
    ) -> Result<&'a [u8], Error> {
        self.find(tag, bytes).ok_or_else(|| {
            // SAFETY: tag is ASCII per the OpenType spec.
            Error::MissingTable(std::str::from_utf8(tag).unwrap_or("???"))
        })
    }
}

// --- big-endian primitive readers ------------------------------------------
//
// All sfnt fields are big-endian. We hand-roll the readers to keep the
// crate dependency-free; the `read_*` helpers all bounds-check and return
// `UnexpectedEof` on truncation.

#[inline]
pub(crate) fn read_u8(bytes: &[u8], off: usize) -> Result<u8, Error> {
    bytes.get(off).copied().ok_or(Error::UnexpectedEof)
}

#[inline]
pub(crate) fn read_u16(bytes: &[u8], off: usize) -> Result<u16, Error> {
    let s = bytes.get(off..off + 2).ok_or(Error::UnexpectedEof)?;
    Ok(u16::from_be_bytes([s[0], s[1]]))
}

#[inline]
pub(crate) fn read_i16(bytes: &[u8], off: usize) -> Result<i16, Error> {
    Ok(read_u16(bytes, off)? as i16)
}

#[inline]
pub(crate) fn read_u32(bytes: &[u8], off: usize) -> Result<u32, Error> {
    let s = bytes.get(off..off + 4).ok_or(Error::UnexpectedEof)?;
    Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

#[inline]
pub(crate) fn read_i32(bytes: &[u8], off: usize) -> Result<i32, Error> {
    Ok(read_u32(bytes, off)? as i32)
}

/// Convenience: read a `u32` Fixed (16.16) and return the raw integer
/// scaled by 0x10000. We don't need fractional precision for the round-1
/// metadata fields.
#[allow(dead_code)]
#[inline]
pub(crate) fn read_fixed_int(bytes: &[u8], off: usize) -> Result<i32, Error> {
    Ok(read_i32(bytes, off)? >> 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_short_input() {
        assert!(matches!(
            TableDirectory::parse(&[0u8; 4]),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = [0u8; 12];
        bytes[0..4].copy_from_slice(&0xDEADBEEFu32.to_be_bytes());
        assert!(matches!(
            TableDirectory::parse(&bytes),
            Err(Error::BadMagic)
        ));
    }

    #[test]
    fn parses_minimal_directory() {
        // sfnt v1, 1 table, dummy 'head' record at offset 28, length 4.
        let mut bytes = vec![0u8; 32];
        bytes[0..4].copy_from_slice(&0x00010000u32.to_be_bytes()); // version
        bytes[4..6].copy_from_slice(&1u16.to_be_bytes()); // numTables
                                                          // searchRange/entrySelector/rangeShift left zero.
        bytes[12..16].copy_from_slice(b"head");
        // checksum 0
        bytes[20..24].copy_from_slice(&28u32.to_be_bytes()); // offset
        bytes[24..28].copy_from_slice(&4u32.to_be_bytes()); // length

        let dir = TableDirectory::parse(&bytes).expect("parse");
        let head = dir.find(b"head", &bytes).expect("find head");
        assert_eq!(head.len(), 4);
        assert!(dir.find(b"glyf", &bytes).is_none());
    }
}
