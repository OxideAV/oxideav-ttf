//! `name` — naming table.
//!
//! The `name` table holds human-readable strings — family / style / full
//! names, copyright, version, designer, licence, vendor URLs, and (for
//! CID-keyed CJK fonts) the PostScript and PostScript-CID findfont names.
//! Each string is keyed by a four-tuple `(platformID, encodingID,
//! languageID, nameID)`: the first three select the operating system,
//! script, and language the string is intended for; `nameID` selects the
//! string's role.
//!
//! Round-1 only exposed `find(name_id)` (family + full name, Windows
//! English preferred). This module now also exposes the full record list,
//! locale-targeted lookup, and the well-known `nameID` constants so a
//! consumer can read e.g. the licence URL, the designer, or a specific
//! locale's family name.
//!
//! Spec: Microsoft OpenType §"name — Naming Table"; Adobe Technical Note
//! #5149 "OpenType-CID/CFF CJK Fonts: 'name' Table Tutorial" §1.2
//! (Platform / Script / Language IDs) and §1.3–1.10 (per-`nameID`
//! semantics). Apple TrueType Reference §"name".

use crate::parser::read_u16;
use crate::Error;

/// Well-known `nameID` values.
///
/// The role of each string is fixed by its `nameID`. These are the IDs
/// enumerated by Adobe TN5149 §1.3–1.10 (the same registry the Microsoft
/// OpenType `name` page publishes). IDs not listed here are either
/// reserved or font-vendor private; the numeric `name_id` field is always
/// available for those.
pub mod name_id {
    /// Copyright notice. (TN5149 §1.3.1)
    pub const COPYRIGHT: u16 = 0;
    /// Font family name. (TN5149 §1.4)
    pub const FAMILY: u16 = 1;
    /// Font subfamily / style name (e.g. "Bold", "Italic"). (TN5149 §1.4)
    pub const SUBFAMILY: u16 = 2;
    /// Unique font identifier. (TN5149 §1.8)
    pub const UNIQUE_ID: u16 = 3;
    /// Full font name (family + subfamily). (TN5149 §1.4)
    pub const FULL_NAME: u16 = 4;
    /// Version string ("Version x.y…"). (TN5149 §1.9)
    pub const VERSION: u16 = 5;
    /// PostScript name. (TN5149 §1.5 / §1.7)
    pub const POSTSCRIPT: u16 = 6;
    /// Trademark. (TN5149 §1.10)
    pub const TRADEMARK: u16 = 7;
    /// Manufacturer name. (TN5149 §1.10)
    pub const MANUFACTURER: u16 = 8;
    /// Designer name. (TN5149 §1.10)
    pub const DESIGNER: u16 = 9;
    /// Description. (TN5149 §1.10)
    pub const DESCRIPTION: u16 = 10;
    /// URL of the font vendor. (TN5149 §1.10)
    pub const VENDOR_URL: u16 = 11;
    /// URL of the font designer. (TN5149 §1.10)
    pub const DESIGNER_URL: u16 = 12;
    /// Licence description. (TN5149 §1.10)
    pub const LICENSE: u16 = 13;
    /// URL where the licence can be found. (TN5149 §1.10)
    pub const LICENSE_URL: u16 = 14;
    /// Typographic (preferred) family name. (TN5149 §1.4)
    pub const TYPOGRAPHIC_FAMILY: u16 = 16;
    /// Typographic (preferred) subfamily name. (TN5149 §1.4)
    pub const TYPOGRAPHIC_SUBFAMILY: u16 = 17;
    /// Compatible full name (Macintosh only).
    pub const COMPATIBLE_FULL: u16 = 18;
    /// Sample text.
    pub const SAMPLE_TEXT: u16 = 19;
    /// PostScript CID findfont name. (TN5149 §1.7)
    pub const POSTSCRIPT_CID: u16 = 20;
}

/// Platform IDs (TN5149 §1.2; Microsoft OpenType `name` page).
pub mod platform {
    /// Unicode platform.
    pub const UNICODE: u16 = 0;
    /// Macintosh platform.
    pub const MACINTOSH: u16 = 1;
    /// Windows platform.
    pub const WINDOWS: u16 = 3;
}

/// One decoded `name` record: the locator tuple plus the decoded string.
///
/// `string` is `Some` for the encodings we can decode without an external
/// legacy codepage table — Unicode (platform 0), Windows Unicode BMP /
/// UCS-4 (platform 3, encoding 1 / 10), and Macintosh Roman ASCII
/// (platform 1, encoding 0). It is `None` for Macintosh non-Roman scripts
/// (Japanese / Chinese / Korean, TN5149 §1.2), whose legacy byte
/// encodings need codepage tables that are not staged under `docs/`; the
/// locator tuple is still surfaced so a caller can decode the raw bytes
/// itself via [`NameTable::record_bytes`].
#[derive(Debug, Clone)]
pub struct NameRecord {
    pub platform_id: u16,
    pub encoding_id: u16,
    pub language_id: u16,
    pub name_id: u16,
    /// Decoded UTF-8 string, or `None` for encodings we cannot decode
    /// without an unstaged legacy codepage table.
    pub string: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NameTable<'a> {
    bytes: &'a [u8],
    /// `count` and `stringOffset` for the format-0/1 record table.
    count: u16,
    string_offset: u16,
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
        // nameID, length, offset. (For format 1 a langTagCount +
        // langTagRecord[] array follows the name records; we don't need
        // the language-tag indirection for the strings themselves — a
        // record's languageID >= 0x8000 references it — so the record
        // table walk is identical for format 0 and 1.)
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

    /// Number of name records in the table.
    pub fn len(&self) -> usize {
        self.count as usize
    }

    /// Whether the table has zero records.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Read the raw locator fields of the `i`-th record:
    /// `(platformID, encodingID, languageID, nameID)`. Returns `None`
    /// when `i` is out of range or the record header is truncated.
    fn record_header(&self, i: usize) -> Option<(u16, u16, u16, u16, usize, usize)> {
        if i >= self.count as usize {
            return None;
        }
        let off = 6 + i * 12;
        let platform = read_u16(self.bytes, off).ok()?;
        let encoding = read_u16(self.bytes, off + 2).ok()?;
        let language = read_u16(self.bytes, off + 4).ok()?;
        let nid = read_u16(self.bytes, off + 6).ok()?;
        let length = read_u16(self.bytes, off + 8).ok()? as usize;
        let str_off = read_u16(self.bytes, off + 10).ok()? as usize;
        Some((platform, encoding, language, nid, length, str_off))
    }

    /// Raw (undecoded) string bytes of the `i`-th record. Useful for the
    /// Macintosh non-Roman scripts we don't decode in-crate.
    pub fn record_bytes(&self, i: usize) -> Option<&'a [u8]> {
        let (_, _, _, _, length, str_off) = self.record_header(i)?;
        let start = self.string_offset as usize + str_off;
        let end = start.checked_add(length)?;
        self.bytes.get(start..end)
    }

    /// All name records, decoded where possible (see [`NameRecord`]).
    pub fn records(&self) -> Vec<NameRecord> {
        let mut out = Vec::with_capacity(self.count as usize);
        for i in 0..self.count as usize {
            let Some((platform, encoding, language, name_id, _, _)) = self.record_header(i) else {
                continue;
            };
            let string = self
                .record_bytes(i)
                .and_then(|raw| decode(platform, encoding, raw).map(|c| c.into_owned()));
            out.push(NameRecord {
                platform_id: platform,
                encoding_id: encoding,
                language_id: language,
                name_id,
                string,
            });
        }
        out
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
            let (platform, encoding, language, nid, length, str_off) = match self.record_header(i) {
                Some(h) => h,
                None => continue,
            };
            if nid != name_id {
                continue;
            }
            let start = self.string_offset as usize + str_off;
            let end = start.checked_add(length)?;
            let raw = self.bytes.get(start..end)?;
            let rank = rank_record(platform, encoding, language);
            let decoded = match decode(platform, encoding, raw) {
                Some(d) => d,
                None => continue,
            };
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

    /// Find the value of a specific `(name_id, platform_id, language_id)`
    /// record. Unlike [`find`](Self::find), no ranking is applied — the
    /// caller has named the exact locale they want (e.g. the
    /// `(FAMILY, WINDOWS, 0x0411)` Japanese family name). Returns the
    /// first matching, decodable record. Returns `None` when no record
    /// matches or the matched record's encoding isn't one we decode.
    pub fn find_for(&self, name_id: u16, platform_id: u16, language_id: u16) -> Option<String> {
        for i in 0..self.count as usize {
            let (platform, encoding, language, nid, length, str_off) = self.record_header(i)?;
            if nid != name_id || platform != platform_id || language != language_id {
                continue;
            }
            let start = self.string_offset as usize + str_off;
            let end = start.checked_add(length)?;
            let raw = self.bytes.get(start..end)?;
            if let Some(decoded) = decode(platform, encoding, raw) {
                return Some(decoded.into_owned());
            }
        }
        None
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
        // Mac non-Roman scripts (TN5149 §1.2: Japanese / Chinese / Korean)
        // need legacy codepage tables we don't stage, so they return None.
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

    /// Build a name table with several records, each (platform, encoding,
    /// language, name_id, &str). UTF-16-BE for Windows/Unicode records,
    /// raw bytes for the Mac record. Records are emitted in input order
    /// and the storage area is laid out contiguously.
    fn build_multi(records: &[(u16, u16, u16, u16, &[u8])]) -> Vec<u8> {
        let header_size = 6 + records.len() * 12;
        let mut out = vec![0u8; header_size];
        out[0..2].copy_from_slice(&0u16.to_be_bytes()); // format 0
        out[2..4].copy_from_slice(&(records.len() as u16).to_be_bytes());
        out[4..6].copy_from_slice(&(header_size as u16).to_be_bytes()); // stringOffset
        let mut storage: Vec<u8> = Vec::new();
        for (i, &(p, e, l, n, raw)) in records.iter().enumerate() {
            let off = 6 + i * 12;
            out[off..off + 2].copy_from_slice(&p.to_be_bytes());
            out[off + 2..off + 4].copy_from_slice(&e.to_be_bytes());
            out[off + 4..off + 6].copy_from_slice(&l.to_be_bytes());
            out[off + 6..off + 8].copy_from_slice(&n.to_be_bytes());
            out[off + 8..off + 10].copy_from_slice(&(raw.len() as u16).to_be_bytes());
            out[off + 10..off + 12].copy_from_slice(&(storage.len() as u16).to_be_bytes());
            storage.extend_from_slice(raw);
        }
        out.extend_from_slice(&storage);
        out
    }

    fn utf16be(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
    }

    #[test]
    fn decodes_utf16_be() {
        let bytes = build_minimal();
        let n = NameTable::parse(&bytes).unwrap();
        assert_eq!(n.find(1), Some("Hi"));
        assert_eq!(n.find(99), None);
    }

    #[test]
    fn well_known_name_id_constants_match_tn5149() {
        // Spot-check the registry from TN5149 §1.3–1.10.
        assert_eq!(name_id::COPYRIGHT, 0);
        assert_eq!(name_id::FAMILY, 1);
        assert_eq!(name_id::SUBFAMILY, 2);
        assert_eq!(name_id::FULL_NAME, 4);
        assert_eq!(name_id::VERSION, 5);
        assert_eq!(name_id::POSTSCRIPT, 6);
        assert_eq!(name_id::LICENSE_URL, 14);
        assert_eq!(name_id::TYPOGRAPHIC_FAMILY, 16);
        assert_eq!(name_id::POSTSCRIPT_CID, 20);
    }

    #[test]
    fn records_enumerates_every_record_with_locator_tuple() {
        let fam = utf16be("Acme Sans");
        let ver = utf16be("Version 1.0");
        let bytes = build_multi(&[
            (3, 1, 0x0409, name_id::FAMILY, &fam),
            (3, 1, 0x0409, name_id::VERSION, &ver),
        ]);
        let n = NameTable::parse(&bytes).unwrap();
        assert_eq!(n.len(), 2);
        assert!(!n.is_empty());
        let recs = n.records();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].platform_id, platform::WINDOWS);
        assert_eq!(recs[0].encoding_id, 1);
        assert_eq!(recs[0].language_id, 0x0409);
        assert_eq!(recs[0].name_id, name_id::FAMILY);
        assert_eq!(recs[0].string.as_deref(), Some("Acme Sans"));
        assert_eq!(recs[1].name_id, name_id::VERSION);
        assert_eq!(recs[1].string.as_deref(), Some("Version 1.0"));
    }

    #[test]
    fn find_for_targets_exact_locale_without_ranking() {
        // Two family records: English (US) and Japanese. `find` returns
        // the highest-ranked (English); `find_for` returns whichever the
        // caller names.
        let en = utf16be("Acme Sans");
        let ja = utf16be("\u{30A2}\u{30AF}\u{30E1}"); // アクメ
        let bytes = build_multi(&[
            (3, 1, 0x0411, name_id::FAMILY, &ja), // Japanese first in the table
            (3, 1, 0x0409, name_id::FAMILY, &en),
        ]);
        let n = NameTable::parse(&bytes).unwrap();
        // `find` ranks English (US) above the generic-Windows Japanese.
        assert_eq!(n.find(name_id::FAMILY), Some("Acme Sans"));
        // `find_for` honours the exact request.
        assert_eq!(
            n.find_for(name_id::FAMILY, platform::WINDOWS, 0x0411)
                .as_deref(),
            Some("\u{30A2}\u{30AF}\u{30E1}")
        );
        assert_eq!(
            n.find_for(name_id::FAMILY, platform::WINDOWS, 0x0409)
                .as_deref(),
            Some("Acme Sans")
        );
        // No match -> None.
        assert_eq!(n.find_for(name_id::FAMILY, platform::WINDOWS, 0x0407), None);
        assert_eq!(
            n.find_for(name_id::VERSION, platform::WINDOWS, 0x0409),
            None
        );
    }

    #[test]
    fn mac_nonroman_record_undecodable_but_locator_and_bytes_surfaced() {
        // Mac Japanese (platform 1, script 1) — TN5149 §1.2. We don't
        // stage a Shift-JIS table, so `string` is None, but the locator
        // tuple and the raw bytes are still available.
        let mac_bytes = [0x82u8, 0xA0, 0x82, 0xA2]; // arbitrary non-ASCII
        let bytes = build_multi(&[(1, 1, 11, name_id::FAMILY, &mac_bytes)]);
        let n = NameTable::parse(&bytes).unwrap();
        let recs = n.records();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].platform_id, platform::MACINTOSH);
        assert_eq!(recs[0].encoding_id, 1); // Japanese script
        assert_eq!(recs[0].language_id, 11);
        assert!(recs[0].string.is_none());
        assert_eq!(n.record_bytes(0), Some(&mac_bytes[..]));
        assert_eq!(n.record_bytes(1), None);
    }

    #[test]
    fn mac_roman_ascii_decodes() {
        let bytes = build_multi(&[(1, 0, 0, name_id::FULL_NAME, b"Acme Sans Bold")]);
        let n = NameTable::parse(&bytes).unwrap();
        assert_eq!(
            n.find_for(name_id::FULL_NAME, platform::MACINTOSH, 0)
                .as_deref(),
            Some("Acme Sans Bold")
        );
    }
}
