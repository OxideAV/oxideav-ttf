//! `SVG ` — The SVG (Scalable Vector Graphics) table.
//!
//! Spec: ISO/IEC 14496-22:2019/Amd.1:2020 §5.5.1 ("SVG — The SVG (Scalable
//! Vector Graphics) Table"). The table carries vector colour-glyph
//! descriptions as SVG 1.1 documents — an alternative to the flat
//! `COLR`/`CPAL` layer stack and the bitmap `CBDT`/`sbix` strikes. Each
//! document covers a contiguous range of glyph IDs and may be shared by
//! more than one range record.
//!
//! ## On-disk layout (§5.5.1)
//!
//! SVG table header (10 bytes, all offsets from the start of the table):
//!
//! ```text
//! uint16    version                  // table version; set to 0
//! Offset32  offsetToSVGDocumentList  // from start of SVG table; non-zero
//! uint32    reserved                 // set to 0
//! ```
//!
//! NOTE on field order: the spec's header table lists `version`,
//! `reserved`, then `offsetToSVGDocumentList` in the prose column, but
//! the on-wire order fixed by every shipping font and the surrounding
//! `Type` column is `version` (uint16), `offsetToSVGDocumentList`
//! (Offset32), `reserved` (uint32) — the 32-bit document-list offset
//! immediately follows the 16-bit version so the structure stays
//! naturally aligned. We decode in that on-wire order.
//!
//! SVG Document List (at `offsetToSVGDocumentList`):
//!
//! ```text
//! uint16              numEntries            // non-zero
//! SVGDocumentRecord   documentRecords[numEntries]
//! ```
//!
//! SVGDocumentRecord (12 bytes):
//!
//! ```text
//! uint16    startGlyphID    // first glyph ID of the range
//! uint16    endGlyphID      // last glyph ID of the range (inclusive)
//! Offset32  svgDocOffset    // from the start of the SVGDocumentList; non-zero
//! uint32    svgDocLength    // length of the on-wire (possibly gzip) document; non-zero
//! ```
//!
//! ## Invariants (§5.5.1)
//!
//! The parser enforces:
//!
//! - `version == 0` (the spec's "set to 0" mandate; a future structural
//!   revision would carry a new version, so rejection is defensive);
//! - `offsetToSVGDocumentList != 0` ("must be non-zero") and in bounds;
//! - `numEntries != 0` ("must be non-zero");
//! - per record, `startGlyphID <= endGlyphID`, `svgDocOffset != 0`, and
//!   `svgDocLength != 0` ("must be non-zero");
//! - the §5.5.1 ordering rule: "Records must be sorted in order of
//!   increasing startGlyphID. For any given record, the startGlyphID
//!   must be less than or equal to the endGlyphID of that record, and
//!   also must be greater than the endGlyphID of any previous record."
//!   The parser checks `startGlyphID[i] > endGlyphID[i-1]` so the
//!   ranges are strictly disjoint and ascending; a record that violates
//!   it is rejected as `BadStructure`;
//! - every `svgDocOffset + svgDocLength` slice (relative to the
//!   SVGDocumentList) lies inside the on-wire SVG table bytes.
//!
//! ## Document encoding (§5.5.2)
//!
//! "SVG documents within an OFF SVG table may either be plain text or
//! gzip-encoded … the first three bytes of the gzip-encoded document
//! header must be 0x1F, 0x8B, 0x08." The `svgDocLength` field always
//! gives the length of the *encoded* data, not the decoded document.
//! This parser surfaces the raw on-wire document bytes plus an
//! [`SvgDocument::is_gzip_encoded`] predicate that tests the 3-byte gzip
//! magic; actual gzip inflation + XML parsing of the (UTF-8) SVG 1.1
//! markup is left to the consumer renderer (`oxideav-scribe`), matching
//! the raw-payload policy already used for `sbix` PNG/JPEG/TIFF blobs
//! and `CBDT` PNG strikes. The spec's SVG capability restrictions
//! (no `<text>`, `<script>`, `<a>`, relative `em`/`ex` units, …) are
//! likewise a renderer concern, not a table-decode concern.

use crate::parser::{read_u16, read_u32};
use crate::Error;

/// Four-byte ASCII tag identifying this table in the sfnt directory.
/// Note the trailing space — the tag is exactly `'SVG '`.
pub const SVG_TABLE_TAG: [u8; 4] = *b"SVG ";

/// On-wire version of the SVG table per §5.5.1. The spec fixes the
/// field at 0; any other value is rejected.
pub const SVG_VERSION_0: u16 = 0;

/// Length in bytes of the fixed SVG table header (§5.5.1): `uint16`
/// version + `Offset32` documentListOffset + `uint32` reserved.
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub const SVG_HEADER_LEN: usize = 10;

/// Length in bytes of one `SVGDocumentRecord` (§5.5.1): two `uint16`
/// glyph IDs + `Offset32` + `uint32` = 12 bytes.
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub const SVG_DOCUMENT_RECORD_LEN: usize = 12;

/// The three-byte gzip member header that opens a gzip-encoded SVG
/// document per §5.5.2 (RFC 1952 magic `0x1F 0x8B` + deflate method
/// `0x08`).
pub const SVG_GZIP_MAGIC: [u8; 3] = [0x1F, 0x8B, 0x08];

/// Sanity cap on `numEntries`. The on-wire field is a `uint16` so the
/// spec ceiling is 65535; real fonts ship one record per contiguous
/// colour-glyph range. We cap at the `uint16` max so a malformed
/// header cannot over-allocate but every conformant table still parses.
const MAX_DOCUMENT_RECORDS: usize = u16::MAX as usize;

/// One SVG document and the glyph-ID range it covers (§5.5.1
/// `SVGDocumentRecord`). The `data` slice borrows the on-wire bytes of
/// the document exactly as stored (plain UTF-8 text *or* gzip-encoded);
/// its length matches the on-wire `svgDocLength` field.
#[derive(Debug, Clone, Copy)]
pub struct SvgDocument<'a> {
    /// First glyph ID covered by this document (inclusive).
    pub start_glyph_id: u16,
    /// Last glyph ID covered by this document (inclusive).
    pub end_glyph_id: u16,
    /// Raw on-wire document bytes: plain UTF-8 SVG 1.1 markup, or a
    /// gzip-encoded stream (test with [`Self::is_gzip_encoded`]). The
    /// length is the on-wire `svgDocLength`, i.e. the *encoded* size.
    pub data: &'a [u8],
}

impl<'a> SvgDocument<'a> {
    /// `true` when the document opens with the §5.5.2 gzip magic
    /// (`0x1F 0x8B 0x08`). When this returns `true` the consumer must
    /// inflate the deflate stream (RFC 1951/1952) before parsing the
    /// SVG markup; when `false` the bytes are already plain UTF-8 SVG.
    pub fn is_gzip_encoded(&self) -> bool {
        self.data.len() >= SVG_GZIP_MAGIC.len()
            && self.data[..SVG_GZIP_MAGIC.len()] == SVG_GZIP_MAGIC
    }

    /// `true` when `gid` falls inside this document's glyph-ID range
    /// (`start_glyph_id <= gid <= end_glyph_id`).
    pub fn covers(&self, gid: u16) -> bool {
        gid >= self.start_glyph_id && gid <= self.end_glyph_id
    }
}

/// Parsed `SVG ` table — the header plus the borrowed document records.
/// Document payloads are kept as borrows into the on-wire bytes so the
/// parser copies none of the (potentially large) SVG markup.
#[derive(Debug, Clone)]
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub struct SvgTable<'a> {
    version: u16,
    documents: Vec<SvgDocument<'a>>,
}

impl<'a> SvgTable<'a> {
    /// Decode the `SVG ` table from its on-wire byte slice. The returned
    /// [`SvgTable`] borrows from `bytes` so its lifetime is bounded by
    /// the caller's slice.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < SVG_HEADER_LEN {
            return Err(Error::UnexpectedEof);
        }
        let version = read_u16(bytes, 0)?;
        if version != SVG_VERSION_0 {
            return Err(Error::BadStructure("SVG: version != 0"));
        }
        // On-wire order (see module NOTE): Offset32 doc-list offset at
        // +2, uint32 reserved at +6. `reserved` is read past but not
        // validated — the spec says "set to 0" yet a non-zero value
        // does not affect the decode.
        let doc_list_offset = read_u32(bytes, 2)? as usize;
        if doc_list_offset == 0 {
            return Err(Error::BadStructure("SVG: offsetToSVGDocumentList == 0"));
        }
        if doc_list_offset + 2 > bytes.len() {
            return Err(Error::BadStructure("SVG: document list past end of table"));
        }

        // The SVGDocumentList: uint16 numEntries then the records.
        let num_entries = read_u16(bytes, doc_list_offset)? as usize;
        if num_entries == 0 {
            return Err(Error::BadStructure("SVG: numEntries == 0"));
        }
        if num_entries > MAX_DOCUMENT_RECORDS {
            return Err(Error::BadStructure("SVG: numEntries cap"));
        }
        let records_base = doc_list_offset + 2;
        let records_end = records_base
            .checked_add(
                num_entries
                    .checked_mul(SVG_DOCUMENT_RECORD_LEN)
                    .ok_or(Error::BadStructure("SVG: document records overflow"))?,
            )
            .ok_or(Error::BadStructure("SVG: document records overflow"))?;
        if records_end > bytes.len() {
            return Err(Error::UnexpectedEof);
        }

        let total_len = bytes.len();
        let mut documents: Vec<SvgDocument<'a>> = Vec::with_capacity(num_entries);
        let mut prev_end: Option<u16> = None;
        for i in 0..num_entries {
            let off = records_base + i * SVG_DOCUMENT_RECORD_LEN;
            let start_glyph_id = read_u16(bytes, off)?;
            let end_glyph_id = read_u16(bytes, off + 2)?;
            // §5.5.1: startGlyphID must be <= endGlyphID for the record.
            if start_glyph_id > end_glyph_id {
                return Err(Error::BadStructure("SVG: startGlyphID > endGlyphID"));
            }
            // §5.5.1: records sorted by increasing startGlyphID, and
            // each startGlyphID > the endGlyphID of any previous
            // record — so the ranges are strictly disjoint + ascending.
            if let Some(prev) = prev_end {
                if start_glyph_id <= prev {
                    return Err(Error::BadStructure(
                        "SVG: record range not strictly after previous",
                    ));
                }
            }
            prev_end = Some(end_glyph_id);

            let svg_doc_offset = read_u32(bytes, off + 4)? as usize;
            let svg_doc_length = read_u32(bytes, off + 8)? as usize;
            // §5.5.1: svgDocOffset and svgDocLength "must be non-zero".
            if svg_doc_offset == 0 {
                return Err(Error::BadStructure("SVG: svgDocOffset == 0"));
            }
            if svg_doc_length == 0 {
                return Err(Error::BadStructure("SVG: svgDocLength == 0"));
            }
            // svgDocOffset is measured "from the beginning of the
            // SVGDocumentList" per §5.5.1, NOT from the start of the
            // SVG table.
            let doc_start = doc_list_offset
                .checked_add(svg_doc_offset)
                .ok_or(Error::BadStructure("SVG: svgDocOffset overflow"))?;
            let doc_end = doc_start
                .checked_add(svg_doc_length)
                .ok_or(Error::BadStructure("SVG: svgDocOffset + length overflow"))?;
            if doc_end > total_len {
                return Err(Error::BadStructure("SVG: document past end of table"));
            }
            documents.push(SvgDocument {
                start_glyph_id,
                end_glyph_id,
                data: &bytes[doc_start..doc_end],
            });
        }

        Ok(Self { version, documents })
    }

    /// `version` field from the header (always 0 per §5.5.1).
    pub fn version(&self) -> u16 {
        self.version
    }

    /// Borrow the full document-record array, in on-wire order (which
    /// §5.5.1 mandates be sorted by ascending `startGlyphID`).
    pub fn documents(&self) -> &[SvgDocument<'a>] {
        &self.documents
    }

    /// Resolve the SVG document covering `gid`, or `None` when no range
    /// record covers it. Because §5.5.1 guarantees the records are
    /// sorted by ascending, strictly-disjoint `startGlyphID`, this is a
    /// binary search over the range starts.
    pub fn document_for_glyph(&self, gid: u16) -> Option<&SvgDocument<'a>> {
        // partition_point finds the first record whose start > gid; the
        // candidate is the record just before it.
        let idx = self.documents.partition_point(|d| d.start_glyph_id <= gid);
        if idx == 0 {
            return None;
        }
        let candidate = &self.documents[idx - 1];
        if candidate.covers(gid) {
            Some(candidate)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic `SVG ` table whose layout matches §5.5.1
    /// exactly: 10-byte header, then the SVGDocumentList (uint16 count +
    /// 12-byte records), then the document payloads packed in record
    /// order. `records` is `(startGID, endGID, payload)`.
    fn build(records: &[(u16, u16, &[u8])]) -> Vec<u8> {
        let mut b = Vec::new();
        // Header: version=0, offsetToSVGDocumentList, reserved=0.
        b.extend_from_slice(&SVG_VERSION_0.to_be_bytes());
        let doc_list_offset = SVG_HEADER_LEN as u32;
        b.extend_from_slice(&doc_list_offset.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes()); // reserved
        debug_assert_eq!(b.len(), SVG_HEADER_LEN);

        // SVGDocumentList: numEntries then the records.
        b.extend_from_slice(&(records.len() as u16).to_be_bytes());
        let records_base = SVG_HEADER_LEN + 2;
        let payload_base = records_base + records.len() * SVG_DOCUMENT_RECORD_LEN;
        // svgDocOffset is relative to the SVGDocumentList start (=
        // doc_list_offset).
        let mut cur = payload_base - SVG_HEADER_LEN; // offset from list start
        for (start, end, payload) in records {
            b.extend_from_slice(&start.to_be_bytes());
            b.extend_from_slice(&end.to_be_bytes());
            b.extend_from_slice(&(cur as u32).to_be_bytes());
            b.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            cur += payload.len();
        }
        for (_, _, payload) in records {
            b.extend_from_slice(payload);
        }
        b
    }

    #[test]
    fn parses_single_document() {
        let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        let bytes = build(&[(3, 5, svg)]);
        let table = SvgTable::parse(&bytes).expect("parse");
        assert_eq!(table.version(), 0);
        assert_eq!(table.documents().len(), 1);
        let doc = &table.documents()[0];
        assert_eq!(doc.start_glyph_id, 3);
        assert_eq!(doc.end_glyph_id, 5);
        assert_eq!(doc.data, svg);
        assert!(!doc.is_gzip_encoded());
    }

    #[test]
    fn document_for_glyph_resolves_inside_and_outside_ranges() {
        let a = b"<svg>A</svg>";
        let b = b"<svg>B</svg>";
        let bytes = build(&[(10, 12, a), (20, 20, b)]);
        let table = SvgTable::parse(&bytes).expect("parse");
        // Inside the first range.
        assert_eq!(table.document_for_glyph(10).unwrap().data, a);
        assert_eq!(table.document_for_glyph(11).unwrap().data, a);
        assert_eq!(table.document_for_glyph(12).unwrap().data, a);
        // The single-glyph second range.
        assert_eq!(table.document_for_glyph(20).unwrap().data, b);
        // Gaps: below the first range, in the hole between, above the last.
        assert!(table.document_for_glyph(9).is_none());
        assert!(table.document_for_glyph(13).is_none());
        assert!(table.document_for_glyph(19).is_none());
        assert!(table.document_for_glyph(21).is_none());
    }

    #[test]
    fn detects_gzip_encoded_document() {
        // §5.5.2: gzip header opens with 0x1F 0x8B 0x08.
        let mut gz = vec![0x1F, 0x8B, 0x08];
        gz.extend_from_slice(&[0x00; 16]); // dummy deflate body
        let bytes = build(&[(1, 1, &gz)]);
        let table = SvgTable::parse(&bytes).expect("parse");
        let doc = &table.documents()[0];
        assert!(doc.is_gzip_encoded());
        // The surfaced length is the encoded length, not a decoded size.
        assert_eq!(doc.data.len(), gz.len());
    }

    #[test]
    fn shared_document_across_two_records_round_trips() {
        // §5.5.1 NOTE: two records may point at the same SVG document so
        // a single document covers discontinuous glyph-ID ranges. Build
        // by hand so both records carry the same svgDocOffset.
        let payload = b"<svg>shared</svg>";
        let mut b = Vec::new();
        b.extend_from_slice(&SVG_VERSION_0.to_be_bytes());
        let doc_list_offset = SVG_HEADER_LEN as u32;
        b.extend_from_slice(&doc_list_offset.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes()); // reserved
        b.extend_from_slice(&2u16.to_be_bytes()); // numEntries
        let records_base = SVG_HEADER_LEN + 2;
        let payload_base = records_base + 2 * SVG_DOCUMENT_RECORD_LEN;
        let shared_off = (payload_base - SVG_HEADER_LEN) as u32; // from list start
                                                                 // Record 1: gids 5..=6.
        b.extend_from_slice(&5u16.to_be_bytes());
        b.extend_from_slice(&6u16.to_be_bytes());
        b.extend_from_slice(&shared_off.to_be_bytes());
        b.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        // Record 2: gids 9..=9, SAME offset.
        b.extend_from_slice(&9u16.to_be_bytes());
        b.extend_from_slice(&9u16.to_be_bytes());
        b.extend_from_slice(&shared_off.to_be_bytes());
        b.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        b.extend_from_slice(payload);

        let table = SvgTable::parse(&b).expect("parse");
        assert_eq!(table.document_for_glyph(5).unwrap().data, payload);
        assert_eq!(table.document_for_glyph(9).unwrap().data, payload);
        // The hole between the two ranges resolves to None.
        assert!(table.document_for_glyph(7).is_none());
    }

    #[test]
    fn rejects_short_header() {
        let b = vec![0u8; SVG_HEADER_LEN - 1];
        assert!(matches!(SvgTable::parse(&b), Err(Error::UnexpectedEof)));
    }

    #[test]
    fn rejects_nonzero_version() {
        let mut b = build(&[(1, 1, b"<svg/>")]);
        b[0..2].copy_from_slice(&1u16.to_be_bytes());
        assert!(matches!(SvgTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_zero_document_list_offset() {
        let mut b = build(&[(1, 1, b"<svg/>")]);
        b[2..6].copy_from_slice(&0u32.to_be_bytes());
        assert!(matches!(SvgTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_zero_num_entries() {
        // Header is valid but the SVGDocumentList claims 0 records.
        let mut b = Vec::new();
        b.extend_from_slice(&SVG_VERSION_0.to_be_bytes());
        b.extend_from_slice(&(SVG_HEADER_LEN as u32).to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes()); // numEntries == 0
        assert!(matches!(SvgTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_start_greater_than_end() {
        let mut b = build(&[(5, 5, b"<svg/>")]);
        // Overwrite the first record's startGlyphID to 6 (> endGID 5).
        let rec_off = SVG_HEADER_LEN + 2;
        b[rec_off..rec_off + 2].copy_from_slice(&6u16.to_be_bytes());
        assert!(matches!(SvgTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_records_out_of_order() {
        // §5.5.1: each startGlyphID must be > the previous endGlyphID.
        // Two records (1..=5) then (3..=8) overlap → rejected.
        let bytes = build(&[(1, 5, b"<svg>A</svg>"), (3, 8, b"<svg>B</svg>")]);
        assert!(matches!(
            SvgTable::parse(&bytes),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn rejects_adjacent_touching_ranges() {
        // (1..=5) then (5..=8): startGlyphID 5 is NOT > prev endGlyphID
        // 5, so the strict-disjoint rule rejects it.
        let bytes = build(&[(1, 5, b"<svg>A</svg>"), (5, 8, b"<svg>B</svg>")]);
        assert!(matches!(
            SvgTable::parse(&bytes),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn rejects_zero_svg_doc_offset() {
        let mut b = build(&[(1, 1, b"<svg/>")]);
        let rec_off = SVG_HEADER_LEN + 2;
        b[rec_off + 4..rec_off + 8].copy_from_slice(&0u32.to_be_bytes());
        assert!(matches!(SvgTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_zero_svg_doc_length() {
        let mut b = build(&[(1, 1, b"<svg/>")]);
        let rec_off = SVG_HEADER_LEN + 2;
        b[rec_off + 8..rec_off + 12].copy_from_slice(&0u32.to_be_bytes());
        assert!(matches!(SvgTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_document_past_end_of_table() {
        let mut b = build(&[(1, 1, b"<svg/>")]);
        let rec_off = SVG_HEADER_LEN + 2;
        // Push svgDocOffset far past the table end.
        let bogus = (b.len() as u32) + 100;
        b[rec_off + 4..rec_off + 8].copy_from_slice(&bogus.to_be_bytes());
        assert!(matches!(SvgTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_truncated_records_array() {
        let mut b = build(&[(1, 1, b"<svg/>")]);
        // Claim 4 records but only ship the bytes for 1.
        let list_off = SVG_HEADER_LEN;
        b[list_off..list_off + 2].copy_from_slice(&4u16.to_be_bytes());
        assert!(matches!(SvgTable::parse(&b), Err(Error::UnexpectedEof)));
    }

    #[test]
    fn document_for_glyph_below_first_range_is_none() {
        // partition_point edge: gid strictly below the very first start.
        let bytes = build(&[(100, 110, b"<svg/>")]);
        let table = SvgTable::parse(&bytes).expect("parse");
        assert!(table.document_for_glyph(0).is_none());
        assert!(table.document_for_glyph(99).is_none());
        assert!(table.document_for_glyph(100).is_some());
    }
}
