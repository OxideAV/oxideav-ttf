//! `meta` — Metadata table.
//!
//! Spec: ISO/IEC 14496-22:2019 §5.7.6 ("meta – Metadata table"). The
//! metadata table is the OpenType-level grab-bag for font-wide
//! key/value pairs whose keys are four-character ASCII tags and whose
//! values may be either UTF-8 text or opaque binary bytes. Two tags
//! are registered today — `'dlng'` (design languages) and `'slng'`
//! (supported languages) — and the spec reserves `'appl'` / `'bild'`
//! for Apple use. Any other tag is treated as either a vendor-private
//! key (uppercase + digits, per §5.7.6.2) or an as-yet-unregistered
//! public key whose semantics the caller is expected to interpret.
//!
//! ## On-disk layout (§5.7.6.1)
//!
//! ```text
//! uint32   version          // set to 1
//! uint32   flags            // currently unused; set to 0
//! uint32   reserved         // not used; set to 0 — see NOTE in §5.7.6.1
//!                            // ("originally documented in the Apple
//!                            //  TrueType spec as a data offset")
//! uint32   dataMapsCount
//! DataMap  dataMaps[dataMapsCount]
//!     Tag       tag
//!     Offset32  dataOffset  // from the start of the 'meta' table
//!     uint32    dataLength
//! ```
//!
//! The data payload referenced by each `DataMap.dataOffset` lives
//! later in the same table. The spec is explicit that "the data is
//! not required to be padded to any byte boundary" so the parser
//! treats each payload as an opaque byte slice and lets the caller
//! decide whether to validate UTF-8.
//!
//! ## Header invariants (§5.7.6.1)
//!
//! The parser enforces:
//!
//! - `version == 1` per the spec's "set to 1" mandate (a future
//!   header revision is permitted via the tag registry but a new
//!   `version` would imply a structural break, so rejection here is
//!   defensive);
//! - `flags == 0` and the reserved field is read but not validated
//!   (its prose says "currently unused" and "not used"; a font that
//!   misuses it does not break our parse);
//! - every `DataMap.dataOffset + dataLength` slice fits inside the
//!   on-wire `meta` byte range (out-of-range entries are rejected
//!   as `BadStructure`);
//! - the `tag` field passes the §5.7.6.2 tag character class
//!   (letters / digits / trailing spaces only — letters must be the
//!   first character of the tag).
//!
//! ## Tag class (§5.7.6.2)
//!
//! "Metadata tags shall begin with a letter (0x41 to 0x5A, 0x61 to
//! 0x7A) and must use only letters, digits (0x30 to 0x39) or space
//! (0x20). Space characters must only occur as trailing characters
//! in tags that have fewer than four letters or digits."
//!
//! The [`is_valid_meta_tag`] helper applies that grammar; the parser
//! invokes it on every `DataMap.tag` and rejects a malformed entry
//! with `BadStructure`. Vendor-private tags (uppercase-letter-led,
//! all-uppercase + digits per §5.7.6.2 paragraph 4) pass the same
//! grammar so the parser does not need a second pass.
//!
//! ## Registered tags (§5.7.6.2)
//!
//! Two registered tags are defined as of the 2019 edition:
//!
//! - `'dlng'` — *Design languages*. UTF-8 text restricted to Basic
//!   Latin (ASCII) characters. Comma-separated ScriptLangTags
//!   identifying the languages or scripts the font was primarily
//!   designed for. Only one record is meaningful; subsequent
//!   records are ignored per the §5.7.6.1 closing paragraph ("If
//!   only one record or value is permitted for a tag, then any
//!   instances after the first may be ignored.").
//! - `'slng'` — *Supported languages*. Same encoding as `'dlng'`;
//!   declares the languages or scripts the font can render
//!   adequately.
//!
//! Two reserved tags (`'appl'` and `'bild'`) carry Apple-private
//! semantics. The parser surfaces them unchanged.
//!
//! ## ScriptLangTag values (§5.7.6.3)
//!
//! The `dlng` / `slng` payloads are ASCII strings of the form
//! `[language "-"] script ["-" region] *("-" variant) *("-"
//! extension) ["-" privateuse]`. Multiple values are separated by
//! commas (with optional trailing spaces). The [`script_lang_tags`]
//! helper splits a `dlng` / `slng` payload into the individual
//! [`ScriptLangTag`] values, trimming whitespace and discarding
//! empty fragments per the §5.7.6.3 rule "any ScriptLangTag value
//! not conforming to these specifications is ignored."
//!
//! The split only validates the *grammar* of the value; deeper
//! validation (IANA Language Subtag Registry, ISO 15924 script
//! subtags, BCP 47 region forms) is deliberately left to the
//! caller — those registries change on a cadence independent of
//! the on-wire format and pulling them into the parser would
//! couple it to a moving target.

use crate::parser::{read_u32, read_u8};
use crate::Error;

/// On-wire version of the metadata table per §5.7.6.1. The spec
/// fixes the field at 1; any other value is rejected.
pub const META_VERSION_1: u32 = 1;

/// Length in bytes of the fixed `meta` header (§5.7.6.1 "Metadata
/// header"). 4 × `uint32` fields.
pub const META_HEADER_LEN: usize = 16;

/// Length in bytes of one `DataMap` record (§5.7.6.1). `Tag` +
/// `Offset32` + `uint32` = 12 bytes.
pub const META_DATA_MAP_LEN: usize = 12;

/// Four-byte ASCII tag identifying this table in the sfnt directory.
pub const META_TABLE_TAG: [u8; 4] = *b"meta";

/// Sanity cap on the per-table `dataMapsCount`. The on-wire field is
/// a `uint32` so the spec ceiling is 2³². A real-world font carries
/// a handful (typically 1–4); the cap here matches the directory
/// cap on sfnt-level tables (1024) so a malformed `meta` cannot
/// allocate an arbitrarily large vector.
const MAX_DATA_MAPS: u32 = 1024;

/// Registered tag `'dlng'` per §5.7.6.2 — design-language list.
pub const META_TAG_DLNG: [u8; 4] = *b"dlng";

/// Registered tag `'slng'` per §5.7.6.2 — supported-language list.
pub const META_TAG_SLNG: [u8; 4] = *b"slng";

/// Reserved tag `'appl'` per §5.7.6.2 — used by Apple.
pub const META_TAG_APPL: [u8; 4] = *b"appl";

/// Reserved tag `'bild'` per §5.7.6.2 — used by Apple.
pub const META_TAG_BILD: [u8; 4] = *b"bild";

/// One `DataMap` record from the `meta` table (§5.7.6.1).
///
/// The `tag` field has already been validated against the §5.7.6.2
/// character class at parse time. The `payload` slice points into
/// the on-wire `meta` table bytes — its length matches the on-wire
/// `dataLength` field exactly (no padding is implied by the spec).
#[derive(Debug, Clone, Copy)]
pub struct MetaRecord<'a> {
    /// The four-byte ASCII tag identifying the category of the
    /// payload. See §5.7.6.2 for the registered + reserved tags.
    pub tag: [u8; 4],
    /// Raw bytes of the payload. The spec leaves the encoding of
    /// the payload to the tag definition; `'dlng'` and `'slng'`
    /// are ASCII text, vendor-private tags are opaque, others are
    /// defined by their per-tag registration.
    pub payload: &'a [u8],
}

impl<'a> MetaRecord<'a> {
    /// Interpret the payload as a UTF-8 string. Returns `None` when
    /// the bytes are not valid UTF-8. The registered text tags
    /// (`'dlng'`, `'slng'`) are restricted to ASCII per §5.7.6.2 so
    /// this is the convenience accessor for those.
    pub fn payload_as_str(&self) -> Option<&'a str> {
        std::str::from_utf8(self.payload).ok()
    }
}

/// Parsed `meta` table — the 16-byte header plus the borrowed
/// `DataMap` records. The data payloads themselves are kept as
/// borrows into the on-wire bytes so the parser does not copy any
/// of the (potentially large) payload data.
#[derive(Debug, Clone)]
pub struct MetaTable<'a> {
    version: u32,
    flags: u32,
    reserved: u32,
    records: Vec<MetaRecord<'a>>,
}

impl<'a> MetaTable<'a> {
    /// Decode the `meta` table from the on-wire byte slice. The
    /// returned [`MetaTable`] borrows from `bytes` so its lifetime
    /// is bounded by the caller's slice.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < META_HEADER_LEN {
            return Err(Error::UnexpectedEof);
        }
        let version = read_u32(bytes, 0)?;
        if version != META_VERSION_1 {
            return Err(Error::BadStructure("meta: version != 1"));
        }
        let flags = read_u32(bytes, 4)?;
        if flags != 0 {
            return Err(Error::BadStructure("meta: flags != 0"));
        }
        let reserved = read_u32(bytes, 8)?;
        // §5.7.6.1 NOTE: the reserved field "was originally
        // documented in Apple TrueType specification as a data
        // offset. This was redundant…" — we read the value so a
        // caller can introspect a non-zero reserved field but do
        // not gate parsing on it.
        let count = read_u32(bytes, 12)?;
        if count > MAX_DATA_MAPS {
            return Err(Error::BadStructure("meta: dataMapsCount cap"));
        }
        let count_usize = count as usize;
        let body_end = META_HEADER_LEN
            .checked_add(
                count_usize
                    .checked_mul(META_DATA_MAP_LEN)
                    .ok_or(Error::BadStructure("meta: dataMaps overflow"))?,
            )
            .ok_or(Error::BadStructure("meta: dataMaps overflow"))?;
        if bytes.len() < body_end {
            return Err(Error::UnexpectedEof);
        }
        let total_len = bytes.len();
        let mut records: Vec<MetaRecord<'a>> = Vec::with_capacity(count_usize);
        for i in 0..count_usize {
            let off = META_HEADER_LEN + i * META_DATA_MAP_LEN;
            let tag = [
                read_u8(bytes, off)?,
                read_u8(bytes, off + 1)?,
                read_u8(bytes, off + 2)?,
                read_u8(bytes, off + 3)?,
            ];
            if !is_valid_meta_tag(&tag) {
                return Err(Error::BadStructure("meta: tag not §5.7.6.2-conformant"));
            }
            let data_offset = read_u32(bytes, off + 4)? as usize;
            let data_length = read_u32(bytes, off + 8)? as usize;
            // The §5.7.6.1 DataMap record names dataOffset as
            // "Offset in bytes from the beginning of the metadata
            // table" — i.e. relative to `bytes`, not to the
            // dataMaps array.
            let data_end = data_offset
                .checked_add(data_length)
                .ok_or(Error::BadStructure(
                    "meta: dataOffset + dataLength overflow",
                ))?;
            if data_end > total_len {
                return Err(Error::BadStructure(
                    "meta: DataMap payload past end of table",
                ));
            }
            let payload = &bytes[data_offset..data_end];
            records.push(MetaRecord { tag, payload });
        }
        Ok(Self {
            version,
            flags,
            reserved,
            records,
        })
    }

    /// `version` field from the header (always 1 per §5.7.6.1).
    pub fn version(&self) -> u32 {
        self.version
    }

    /// `flags` field from the header (always 0 per §5.7.6.1).
    pub fn flags(&self) -> u32 {
        self.flags
    }

    /// `reserved` field from the header. §5.7.6.1 says "not used;
    /// set to 0" but legacy Apple TrueType fonts may carry a
    /// non-zero value here per the NOTE; we surface the raw value
    /// rather than discard it.
    pub fn reserved(&self) -> u32 {
        self.reserved
    }

    /// Borrow the full DataMap record array. Records appear in
    /// the order they sit on disk; §5.7.6 does not impose a sort
    /// order.
    pub fn records(&self) -> &[MetaRecord<'a>] {
        &self.records
    }

    /// Return the first `MetaRecord` whose tag equals `tag`.
    /// §5.7.6.1 closing paragraph notes that "If only one record
    /// or value is permitted for a tag, then any instances after
    /// the first may be ignored" — the registered `'dlng'` and
    /// `'slng'` tags both fall into that single-record category,
    /// so this accessor returns the first match.
    pub fn record(&self, tag: &[u8; 4]) -> Option<MetaRecord<'a>> {
        self.records.iter().copied().find(|r| &r.tag == tag)
    }

    /// Convenience: return the `'dlng'` (design languages) payload
    /// as a UTF-8 string, if present and well-formed.
    pub fn design_languages(&self) -> Option<&'a str> {
        self.record(&META_TAG_DLNG)?.payload_as_str()
    }

    /// Convenience: return the `'slng'` (supported languages)
    /// payload as a UTF-8 string, if present and well-formed.
    pub fn supported_languages(&self) -> Option<&'a str> {
        self.record(&META_TAG_SLNG)?.payload_as_str()
    }
}

/// `[language "-"] script ["-" region] *("-" variant) *("-"
/// extension) ["-" privateuse]` per §5.7.6.3, kept as the raw
/// ASCII slice. The splitter ([`script_lang_tags`]) only enforces
/// the surface grammar (non-empty, ASCII, hyphen-separated
/// subtags); deeper validation against the IANA Language Subtag
/// Registry and ISO 15924 is left to the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptLangTag<'a> {
    /// Raw ASCII bytes of the tag, hyphens included, trimmed of
    /// surrounding whitespace.
    pub raw: &'a str,
}

impl<'a> ScriptLangTag<'a> {
    /// Subtags split on the `-` separator. Per §5.7.6.3 the script
    /// subtag is mandatory; the parser does not assume a position,
    /// so this is the raw split.
    pub fn subtags(&self) -> impl Iterator<Item = &'a str> {
        self.raw.split('-')
    }

    /// Number of subtags in the tag (hyphen-separated).
    pub fn subtag_count(&self) -> usize {
        self.subtags().count()
    }
}

/// Split a `'dlng'` / `'slng'` payload into [`ScriptLangTag`]
/// values per §5.7.6.3 ("a series of comma-separated
/// ScriptLangTags … Spaces may follow the comma delimiters and
/// are ignored.").
///
/// Returns an empty iterator for a non-UTF-8 payload. Per the
/// §5.7.6.3 directive "Any ScriptLangTag value not conforming to
/// these specifications is ignored", individual fragments that
/// are empty or contain non-ASCII bytes are skipped silently;
/// well-formed fragments are returned in document order.
pub fn script_lang_tags(payload: &str) -> impl Iterator<Item = ScriptLangTag<'_>> {
    payload.split(',').filter_map(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        if !trimmed.is_ascii() {
            return None;
        }
        // Hyphen at either end or a doubled hyphen would produce
        // an empty subtag — both invalid per §5.7.6.3's BNF.
        if trimmed.starts_with('-') || trimmed.ends_with('-') || trimmed.contains("--") {
            return None;
        }
        Some(ScriptLangTag { raw: trimmed })
    })
}

/// §5.7.6.2 tag-character class:
///
/// > Metadata tags shall begin with a letter (0x41 to 0x5A, 0x61 to
/// > 0x7A) and must use only letters, digits (0x30 to 0x39) or space
/// > (0x20). Space characters must only occur as trailing characters
/// > in tags that have fewer than four letters or digits.
pub fn is_valid_meta_tag(tag: &[u8; 4]) -> bool {
    if !is_meta_tag_letter(tag[0]) {
        return false;
    }
    let mut seen_space = false;
    for &b in tag {
        if b == b' ' {
            seen_space = true;
            continue;
        }
        // Once we have seen a space, the rest must also be spaces
        // ("only occur as trailing characters").
        if seen_space {
            return false;
        }
        if !(is_meta_tag_letter(b) || b.is_ascii_digit()) {
            return false;
        }
    }
    true
}

#[inline]
fn is_meta_tag_letter(b: u8) -> bool {
    matches!(b, 0x41..=0x5A | 0x61..=0x7A)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic `meta` table whose layout matches §5.7.6.1
    /// exactly: header (16 B), DataMap array (12 B / entry),
    /// then the data payloads packed in record order.
    fn build(records: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&META_VERSION_1.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes()); // flags
        b.extend_from_slice(&0u32.to_be_bytes()); // reserved
        b.extend_from_slice(&(records.len() as u32).to_be_bytes());
        // Pre-compute data offsets: each payload sits after the
        // DataMap array.
        let payload_base = META_HEADER_LEN + records.len() * META_DATA_MAP_LEN;
        let mut cur = payload_base;
        for (tag, payload) in records {
            b.extend_from_slice(*tag);
            b.extend_from_slice(&(cur as u32).to_be_bytes());
            b.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            cur += payload.len();
        }
        for (_, payload) in records {
            b.extend_from_slice(payload);
        }
        b
    }

    #[test]
    fn parses_minimal_empty_table() {
        // §5.7.6.1 permits dataMapsCount = 0 implicitly: the
        // table is just its 16-byte header.
        let bytes = build(&[]);
        assert_eq!(bytes.len(), META_HEADER_LEN);
        let meta = MetaTable::parse(&bytes).expect("parse");
        assert_eq!(meta.version(), META_VERSION_1);
        assert_eq!(meta.flags(), 0);
        assert_eq!(meta.reserved(), 0);
        assert_eq!(meta.records().len(), 0);
        assert!(meta.design_languages().is_none());
        assert!(meta.supported_languages().is_none());
    }

    #[test]
    fn parses_dlng_and_slng_records() {
        // §5.7.6.2 worked example: dlng = "Latn" (designed for
        // Latin script), slng = "Latn, Cyrl, Grek".
        let dlng = b"Latn";
        let slng = b"Latn, Cyrl, Grek";
        let bytes = build(&[(b"dlng", dlng), (b"slng", slng)]);
        let meta = MetaTable::parse(&bytes).expect("parse");
        assert_eq!(meta.records().len(), 2);
        assert_eq!(meta.design_languages(), Some("Latn"));
        assert_eq!(meta.supported_languages(), Some("Latn, Cyrl, Grek"));
        // Tag lookup matches both registered tags.
        assert!(meta.record(&META_TAG_DLNG).is_some());
        assert!(meta.record(&META_TAG_SLNG).is_some());
        assert!(meta.record(&META_TAG_APPL).is_none());
    }

    #[test]
    fn reserved_tags_appl_and_bild_pass_the_tag_grammar() {
        // §5.7.6.2 lists 'appl' and 'bild' as reserved — both must
        // pass the tag character class.
        assert!(is_valid_meta_tag(&META_TAG_APPL));
        assert!(is_valid_meta_tag(&META_TAG_BILD));
        assert!(is_valid_meta_tag(&META_TAG_DLNG));
        assert!(is_valid_meta_tag(&META_TAG_SLNG));
    }

    #[test]
    fn rejects_short_header() {
        let b = vec![0u8; META_HEADER_LEN - 1];
        assert!(matches!(MetaTable::parse(&b), Err(Error::UnexpectedEof)));
    }

    #[test]
    fn rejects_wrong_version() {
        let mut b = build(&[]);
        b[0..4].copy_from_slice(&2u32.to_be_bytes());
        assert!(matches!(MetaTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_nonzero_flags() {
        let mut b = build(&[]);
        b[4..8].copy_from_slice(&1u32.to_be_bytes());
        assert!(matches!(MetaTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn tolerates_nonzero_reserved_field() {
        // §5.7.6.1 NOTE: the reserved field was historically used
        // by Apple as a data offset. We surface a non-zero value
        // through `reserved()` rather than reject it.
        let mut b = build(&[]);
        b[8..12].copy_from_slice(&42u32.to_be_bytes());
        let meta = MetaTable::parse(&b).expect("parse");
        assert_eq!(meta.reserved(), 42);
    }

    #[test]
    fn rejects_truncated_data_maps_array() {
        let mut b = build(&[(b"dlng", b"Latn")]);
        // Claim 2 records but only ship the bytes for 1.
        b[12..16].copy_from_slice(&2u32.to_be_bytes());
        assert!(matches!(MetaTable::parse(&b), Err(Error::UnexpectedEof)));
    }

    #[test]
    fn rejects_data_payload_past_table_end() {
        let mut b = build(&[(b"dlng", b"Latn")]);
        // First DataMap record sits at byte 16; dataOffset is at
        // byte 20, dataLength at byte 24.
        let map_off = META_HEADER_LEN;
        let bogus_offset = (b.len() + 10) as u32;
        b[map_off + 4..map_off + 8].copy_from_slice(&bogus_offset.to_be_bytes());
        assert!(matches!(MetaTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_data_offset_plus_length_overflow() {
        let mut b = build(&[(b"dlng", b"Latn")]);
        let map_off = META_HEADER_LEN;
        b[map_off + 4..map_off + 8].copy_from_slice(&u32::MAX.to_be_bytes());
        b[map_off + 8..map_off + 12].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(matches!(MetaTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_data_maps_count_cap() {
        // 4-byte header reads fine, dataMapsCount > MAX_DATA_MAPS.
        let mut b = vec![0u8; META_HEADER_LEN];
        b[0..4].copy_from_slice(&META_VERSION_1.to_be_bytes());
        b[12..16].copy_from_slice(&(MAX_DATA_MAPS + 1).to_be_bytes());
        assert!(matches!(MetaTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_tag_starting_with_digit() {
        // §5.7.6.2: "tags shall begin with a letter".
        let b = build(&[(b"1lng", b"Latn")]);
        assert!(matches!(MetaTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_tag_with_inner_space() {
        // §5.7.6.2: spaces must only be trailing.
        let b = build(&[(b"d ng", b"Latn")]);
        assert!(matches!(MetaTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_tag_with_non_alphanumeric() {
        let b = build(&[(b"dl-g", b"Latn")]);
        assert!(matches!(MetaTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn accepts_short_tag_padded_with_trailing_space() {
        // §5.7.6.2: "tags that have fewer than four letters or
        // digits" carry trailing spaces. The whole-spec-defined
        // short tag we care about is 'CFF ' style for sfnt tables;
        // for the meta tag registry no current short tag is
        // defined, but the grammar allows it.
        assert!(is_valid_meta_tag(b"ab  "));
        assert!(is_valid_meta_tag(b"a   "));
    }

    #[test]
    fn rejects_all_space_tag() {
        // First byte must be a letter, not a space.
        assert!(!is_valid_meta_tag(b"    "));
    }

    #[test]
    fn parses_vendor_private_tag() {
        // §5.7.6.2 paragraph 4: vendor-private tags use uppercase
        // letters + digits. Our parser does not distinguish
        // private from registered tags — both flow through.
        let b = build(&[(b"XYZ9", b"private blob")]);
        let meta = MetaTable::parse(&b).expect("parse");
        let rec = meta.record(b"XYZ9").expect("vendor tag visible");
        assert_eq!(rec.payload, b"private blob");
        // payload_as_str round-trips the bytes when they are valid
        // UTF-8.
        assert_eq!(rec.payload_as_str(), Some("private blob"));
    }

    #[test]
    fn payload_as_str_returns_none_for_non_utf8_bytes() {
        // §5.7.6.2 permits binary-typed payloads (e.g. for
        // unregistered tags). `payload_as_str` is the convenience
        // accessor for the text branch — it must reject binary.
        let bytes = build(&[(b"BINS", &[0xFF, 0xFE, 0xFD])]);
        let meta = MetaTable::parse(&bytes).expect("parse");
        let rec = meta.record(b"BINS").expect("record");
        assert!(rec.payload_as_str().is_none());
    }

    #[test]
    fn script_lang_tag_splitter_handles_single_value() {
        let tags: Vec<_> = script_lang_tags("Latn").map(|t| t.raw).collect();
        assert_eq!(tags, vec!["Latn"]);
    }

    #[test]
    fn script_lang_tag_splitter_handles_multiple_values() {
        // §5.7.6.3 worked example pattern: comma-separated with
        // optional trailing space.
        let tags: Vec<_> = script_lang_tags("Latn, Cyrl, Grek")
            .map(|t| t.raw)
            .collect();
        assert_eq!(tags, vec!["Latn", "Cyrl", "Grek"]);
    }

    #[test]
    fn script_lang_tag_splitter_handles_extended_subtags() {
        // §5.7.6.3 example: 'sr-Cyrl', 'en-Dsrt', 'Hant-HK'.
        let tags: Vec<_> = script_lang_tags("sr-Cyrl, en-Dsrt, Hant-HK")
            .map(|t| (t.raw, t.subtag_count()))
            .collect();
        assert_eq!(tags, vec![("sr-Cyrl", 2), ("en-Dsrt", 2), ("Hant-HK", 2)]);
    }

    #[test]
    fn script_lang_tag_splitter_discards_empty_fragments() {
        // §5.7.6.3: "Any ScriptLangTag value not conforming to
        // these specifications is ignored."
        let tags: Vec<_> = script_lang_tags("Latn, , Cyrl, ").map(|t| t.raw).collect();
        assert_eq!(tags, vec!["Latn", "Cyrl"]);
    }

    #[test]
    fn script_lang_tag_splitter_rejects_leading_or_trailing_hyphen() {
        // §5.7.6.3 BNF: every subtag is a non-empty token between
        // hyphens; a leading / trailing / doubled hyphen would
        // produce an empty subtag and the value is rejected.
        let tags: Vec<_> = script_lang_tags("-Latn, Cyrl-, ja--Jpan, ok-Latn")
            .map(|t| t.raw)
            .collect();
        assert_eq!(tags, vec!["ok-Latn"]);
    }

    #[test]
    fn script_lang_tag_splitter_rejects_non_ascii_fragment() {
        let tags: Vec<_> = script_lang_tags("Latn, Lаtn") // second has a Cyrillic 'а'
            .map(|t| t.raw)
            .collect();
        assert_eq!(tags, vec!["Latn"]);
    }

    #[test]
    fn shared_data_payload_between_two_records_round_trips() {
        // Two DataMap records pointing at the same payload bytes
        // is permitted by §5.7.6.1 — it just means two tags share
        // a value. Build by hand to confirm aliasing works.
        let mut b = Vec::new();
        b.extend_from_slice(&META_VERSION_1.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&2u32.to_be_bytes()); // dataMapsCount
        let payload_off = META_HEADER_LEN + 2 * META_DATA_MAP_LEN;
        let payload = b"shared";
        // dlng -> shared
        b.extend_from_slice(b"dlng");
        b.extend_from_slice(&(payload_off as u32).to_be_bytes());
        b.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        // slng -> shared (same offset)
        b.extend_from_slice(b"slng");
        b.extend_from_slice(&(payload_off as u32).to_be_bytes());
        b.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        b.extend_from_slice(payload);
        let meta = MetaTable::parse(&b).expect("parse");
        assert_eq!(meta.design_languages(), Some("shared"));
        assert_eq!(meta.supported_languages(), Some("shared"));
    }

    #[test]
    fn records_accessor_preserves_document_order() {
        // §5.7.6 does not require records to be sorted; the parser
        // must surface them in on-wire order.
        let b = build(&[
            (b"slng", b"Latn, Cyrl"),
            (b"dlng", b"Latn"),
            (b"XYZ1", b"blob"),
        ]);
        let meta = MetaTable::parse(&b).expect("parse");
        let tags: Vec<_> = meta.records().iter().map(|r| r.tag).collect();
        assert_eq!(tags, vec![*b"slng", *b"dlng", *b"XYZ1"]);
    }
}
