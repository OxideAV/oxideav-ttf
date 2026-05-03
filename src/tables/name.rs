//! `name` — name records.
//!
//! The `name` table holds many records; we just want the family + full
//! names. Selection priority: Windows / Unicode BMP English (3,1,0x409),
//! then Mac Roman English (1,0,0).

use crate::parser::read_u16;
use crate::Error;

#[derive(Debug, Clone)]
pub struct NameTable<'a> {
    bytes: &'a [u8],
    /// `count` and `stringOffset` for the format-0/1 record table.
    count: u16,
    string_offset: u16,
    /// Optional storage of decoded UTF-8 strings, keyed by `name_id`.
    /// Populated lazily — but for the round-1 use case we only ever
    /// look up two ids, so we just decode on demand in `find`.
    _phantom: core::marker::PhantomData<&'a ()>,
}

impl<'a> NameTable<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        // Header:
        //   0 / format (2; 0 or 1)
        //   2 / count  (2)
        //   4 / stringOffset (2)
        if bytes.len() < 6 {
            return Err(Error::UnexpectedEof);
        }
        let format = read_u16(bytes, 0)?;
        if format > 1 {
            return Err(Error::BadStructure("name.format > 1"));
        }
        let count = read_u16(bytes, 2)?;
        let string_offset = read_u16(bytes, 4)?;
        // Each record is 12 bytes: platformID, encodingID, languageID,
        // nameID, length, offset.
        let table_end = 6usize + count as usize * 12;
        if bytes.len() < table_end {
            return Err(Error::UnexpectedEof);
        }
        if (string_offset as usize) > bytes.len() {
            return Err(Error::BadOffset);
        }
        Ok(Self {
            bytes,
            count,
            string_offset,
            _phantom: core::marker::PhantomData,
        })
    }

    /// Find the value of a name record by its `name_id`. Selects the
    /// best-ranked encoding (Windows/Unicode/English first).
    pub fn find(&self, name_id: u16) -> Option<&'a str> {
        // We don't return owned strings: instead we look for a record
        // whose payload is already valid UTF-8 (or transcodable to it via
        // the trivial UTF-16-BE path). We return the *highest-ranked*
        // record we can decode.
        let mut best: Option<(i32, std::borrow::Cow<'a, str>)> = None;

        for i in 0..self.count as usize {
            let off = 6 + i * 12;
            let platform = read_u16(self.bytes, off).ok()?;
            let encoding = read_u16(self.bytes, off + 2).ok()?;
            let language = read_u16(self.bytes, off + 4).ok()?;
            let nid = read_u16(self.bytes, off + 6).ok()?;
            if nid != name_id {
                continue;
            }
            let length = read_u16(self.bytes, off + 8).ok()? as usize;
            let str_off = read_u16(self.bytes, off + 10).ok()? as usize;
            let start = self.string_offset as usize + str_off;
            let end = start.checked_add(length)?;
            let raw = self.bytes.get(start..end)?;
            let rank = rank_record(platform, encoding, language);
            let decoded = decode(platform, encoding, raw)?;
            match &best {
                Some((br, _)) if *br >= rank => {}
                _ => best = Some((rank, decoded)),
            }
        }
        // Leak the decoded Cow into a 'a str: only safe for the borrowed
        // case. For owned strings (re-encoded UTF-16) we Box::leak so the
        // returned str outlives the call. Names are tiny (< 100 bytes
        // typically); leak cost is negligible per font load.
        let (_, c) = best?;
        Some(match c {
            std::borrow::Cow::Borrowed(s) => s,
            std::borrow::Cow::Owned(s) => Box::leak(s.into_boxed_str()),
        })
    }
}

fn rank_record(platform: u16, encoding: u16, language: u16) -> i32 {
    // Higher = preferred. Windows English first (most common in modern
    // fonts), then Mac Roman English, then anything Unicode-y, then the
    // rest.
    match (platform, encoding, language) {
        (3, 1, 0x0409) => 100,            // Windows Unicode English (US)
        (3, 1, l) if l & 0xFF == 9 => 90, // Any Windows English
        (3, 1, _) => 80,
        (3, 10, _) => 75, // Windows UCS-4
        (1, 0, 0) => 70,  // Mac Roman English
        (0, _, _) => 60,  // Unicode platform
        _ => 10,
    }
}

fn decode<'a>(platform: u16, encoding: u16, raw: &'a [u8]) -> Option<std::borrow::Cow<'a, str>> {
    match (platform, encoding) {
        // UTF-16 BE: Unicode platform (0,*), Windows Unicode (3,1) and
        // (3,10).
        (0, _) | (3, 1) | (3, 10) => {
            if raw.len() % 2 != 0 {
                return None;
            }
            let mut s = String::with_capacity(raw.len() / 2);
            let mut i = 0;
            while i + 1 < raw.len() {
                let u = u16::from_be_bytes([raw[i], raw[i + 1]]);
                i += 2;
                if (0xD800..=0xDBFF).contains(&u) {
                    // High surrogate — pair with the next code unit.
                    if i + 1 >= raw.len() {
                        return None;
                    }
                    let lo = u16::from_be_bytes([raw[i], raw[i + 1]]);
                    if !(0xDC00..=0xDFFF).contains(&lo) {
                        return None;
                    }
                    i += 2;
                    let cp = 0x10000 + (((u - 0xD800) as u32) << 10) + (lo - 0xDC00) as u32;
                    s.push(char::from_u32(cp)?);
                } else {
                    s.push(char::from_u32(u as u32)?);
                }
            }
            Some(std::borrow::Cow::Owned(s))
        }
        // Mac Roman is a 1-byte encoding; the lower 7 bits are ASCII so
        // everything we need (font-name-wise) decodes as raw ASCII.
        (1, 0) => {
            // Try ASCII fast path, fall back to lossy.
            if raw.iter().all(|&b| b < 0x80) {
                std::str::from_utf8(raw)
                    .ok()
                    .map(std::borrow::Cow::Borrowed)
            } else {
                Some(std::borrow::Cow::Owned(
                    raw.iter()
                        .map(|&b| if b < 0x80 { b as char } else { '?' })
                        .collect(),
                ))
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a one-record name table (Windows Unicode English) holding
    /// "Hi" as name id 1.
    fn build_minimal() -> Vec<u8> {
        let utf16: Vec<u8> = "Hi".encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        let length = utf16.len() as u16;
        let header_size = 6 + 12;
        let mut out = vec![0u8; header_size];
        out[0..2].copy_from_slice(&0u16.to_be_bytes()); // format
        out[2..4].copy_from_slice(&1u16.to_be_bytes()); // count
        out[4..6].copy_from_slice(&(header_size as u16).to_be_bytes()); // stringOffset
                                                                        // Record:
        out[6..8].copy_from_slice(&3u16.to_be_bytes()); // platform = Windows
        out[8..10].copy_from_slice(&1u16.to_be_bytes()); // encoding = Unicode BMP
        out[10..12].copy_from_slice(&0x0409u16.to_be_bytes()); // language = English
        out[12..14].copy_from_slice(&1u16.to_be_bytes()); // name id
        out[14..16].copy_from_slice(&length.to_be_bytes()); // length
        out[16..18].copy_from_slice(&0u16.to_be_bytes()); // offset
        out.extend_from_slice(&utf16);
        out
    }

    #[test]
    fn decodes_utf16_be() {
        let bytes = build_minimal();
        let n = NameTable::parse(&bytes).unwrap();
        assert_eq!(n.find(1), Some("Hi"));
        assert_eq!(n.find(99), None);
    }
}
