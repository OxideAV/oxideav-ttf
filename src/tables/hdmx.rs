//! `hdmx` — horizontal device metrics.
//!
//! Spec: ISO/IEC 14496-22:2019 §5.7.2 ("hdmx – Horizontal device
//! metrics"). This optional table is the precomputed-advance counterpart
//! to [`LTSH`](crate::tables::ltsh): instead of recording the threshold
//! ppem at which the grid-fitted advance converges with the rounded
//! linear advance (the §5.7.4 method), `hdmx` stores the actual
//! integer-pixel advance width of every glyph at a handful of
//! selected ppem sizes. A consumer rendering at one of the recorded
//! sizes can pick the matching device record out of the table and
//! avoid scan-converting the glyph just to learn its grid-fitted
//! advance — at the cost of a non-trivial number of bytes in the font
//! file (§5.7.2 estimates ~9,600 bytes for the recommended ppem set
//! in §7.2).
//!
//! ## Layout (§5.7.2)
//!
//! ```text
//! hdmx Header
//! uint16 version          // 0
//! int16  numRecords       // number of device records
//! int32  sizeDeviceRecord // size of one record, long-aligned
//! DeviceRecord records[numRecords]
//!
//! DeviceRecord (format 0)
//! uint8  pixelSize        // ppem; record is sorted by this field
//! uint8  maxWidth         // maximum width in this record
//! uint8  widths[numGlyphs] // advance widths in pixels at pixelSize ppem
//! // padded with zeros to make sizeDeviceRecord a multiple of 4 bytes
//! ```
//!
//! §5.7.2 fixes the on-wire stride at `sizeDeviceRecord` (one int32 in
//! the header), not at `2 + numGlyphs`. The header byte plus the
//! per-glyph width array fill `2 + numGlyphs` bytes; any trailing
//! bytes up to `sizeDeviceRecord` are spec-prescribed long-word
//! alignment padding ("Each DeviceRecord is padded with 0's to make
//! it long word aligned"). We honour `sizeDeviceRecord` as the
//! record stride so a minor-version growth of the record layout
//! still walks the array correctly with unknown trailing bytes
//! ignored.
//!
//! ## Sort + uniqueness (§5.7.2)
//!
//! "This table is sorted by pixel size." We enforce strictly-increasing
//! `pixelSize` across the record array at parse time so a corrupted
//! table cannot shadow later records under per-ppem lookup. (The spec
//! does not call out a duplicate-record bail explicitly, but two
//! records at the same `pixelSize` could only contradict each other;
//! the strict inequality covers both cases.)
//!
//! ## Eligibility (§5.7.2 + §5.2.3 head.flags bit 4)
//!
//! §5.7.2 specifies that `hdmx` "is not necessary and should not be
//! built" when `head.flags` bit 4 (= "instructions may depend on point
//! size") is unset. §5.7.2 §"Recommendations" (Clause 7) also
//! recommends excluding `hdmx` when only bit 2 ("force ppem to integer
//! values for all internal scaler math") is set. Following the same
//! policy as our `LTSH` parser, we accept any well-formed `hdmx`
//! regardless of `head.flags` — the parser's job is to surface what
//! the font ships, not to second-guess the font author. A caller
//! that wants to honour the §5.7.2 recommendation can cross-check
//! `head.flags` before consulting [`HdmxTable::advance_pixels`].
//!
//! ## Use sites (§5.7.4 + §7.3.5)
//!
//! §5.7.4 names `hdmx` and `vdmx` as the alternative solutions to the
//! same speed problem that `LTSH` addresses by threshold. `LTSH`
//! records "at which ppem does the linear advance round to the
//! grid-fit advance"; `hdmx` records "at this exact ppem, the
//! grid-fit advance in integer pixels". §7.3.5 (Metrics Variations)
//! also calls out that "The 'hdmx' table is not used in variable
//! fonts" — variable fonts encode the equivalent per-axis
//! interpolation through `HVAR` + region scalars instead. We parse
//! the table whenever it is present; a caller that wants to honour
//! the §7.3.5 rule can cross-check `fvar.is_some()` before consulting
//! `hdmx`.

use crate::parser::{read_i16, read_i32, read_u16, read_u8};
use crate::Error;

/// On-wire table tag (`b"hdmx"`, big-endian Fixed `0x68646D78`). Exposed
/// for callers that walk the table directory directly.
pub const HDMX_TABLE_TAG: u32 = 0x6864_6D78;

/// The only currently-defined `hdmx` version (`0`). §5.7.2 reserves the
/// `uint16 version` field for future expansion.
pub const HDMX_VERSION_0: u16 = 0;

/// Header byte count: 2 (`pixelSize`) + 2 (`maxWidth`) is wrong — the
/// header itself is just `version` (2) + `numRecords` (2) +
/// `sizeDeviceRecord` (4) = 8 bytes per §5.7.2.
pub const HDMX_HEADER_LEN: usize = 8;

/// Per-record header: `pixelSize` (1 byte) + `maxWidth` (1 byte). The
/// remaining `numGlyphs` bytes of the record are the per-glyph
/// `widths[]` array, followed by long-alignment padding up to
/// `sizeDeviceRecord`.
pub const HDMX_RECORD_HEADER_LEN: usize = 2;

/// One device record from the `hdmx` array — a single ppem snapshot of
/// every glyph's grid-fitted advance width.
///
/// Storage is the raw `widths[numGlyphs]` slice (the on-wire `widths`
/// array), kept owned so the table outlives the borrowed font bytes.
/// Both `pixelSize` and `maxWidth` come straight from the on-wire
/// per-record header; the §5.7.2 invariant `maxWidth == max(widths)`
/// is informational (a rasteriser uses `maxWidth` to allocate a
/// per-glyph cache without scanning the array) and we do not enforce
/// it at parse time — surfacing the on-wire value lets a caller spot
/// a malformed font cheaply.
#[derive(Debug, Clone)]
pub struct HdmxRecord {
    pixel_size: u8,
    max_width: u8,
    widths: Vec<u8>,
}

impl HdmxRecord {
    /// Pixel size (ppem) at which the per-glyph advances in [`Self::widths`]
    /// are measured. §5.7.2: "ppem sizes are measured along the y axis."
    pub fn pixel_size(&self) -> u8 {
        self.pixel_size
    }

    /// On-wire `maxWidth` field per §5.7.2 — the maximum advance width
    /// across every glyph in this record, in integer pixels. We surface
    /// the byte the font ships; we do not cross-check it against the
    /// array.
    pub fn max_width(&self) -> u8 {
        self.max_width
    }

    /// Per-glyph grid-fitted advance widths at this record's `pixelSize`
    /// ppem, in integer pixels. `widths.len() == maxp.num_glyphs`.
    pub fn widths(&self) -> &[u8] {
        &self.widths
    }

    /// Advance width of `glyph_id` at this record's ppem. Returns `None`
    /// when `glyph_id` is outside the array (a caller that received
    /// a record for ppem N must still range-check the glyph id).
    pub fn advance_pixels(&self, glyph_id: u16) -> Option<u8> {
        self.widths.get(glyph_id as usize).copied()
    }
}

/// Parsed `hdmx` table.
///
/// Records are stored in document order; §5.7.2 mandates strictly-
/// increasing `pixelSize` and we enforce that at parse time so a
/// lookup by ppem either binary-searches the array (when ppem hits a
/// recorded size) or returns `None` (when the requested ppem is not
/// in the table — there is no §5.7.2 "round down" rule).
#[derive(Debug, Clone)]
pub struct HdmxTable {
    version: u16,
    size_device_record: i32,
    records: Vec<HdmxRecord>,
}

impl HdmxTable {
    /// Parse an `hdmx` table from its raw slice and cross-check the
    /// per-record `widths[]` length against `maxp.numGlyphs` per
    /// §5.7.2's "numGlyphs is from the 'maxp' table". A mismatch
    /// (insufficient bytes per record) is rejected as `BadStructure`
    /// instead of silently truncating per-glyph lookups.
    pub fn parse(bytes: &[u8], expected_num_glyphs: u16) -> Result<Self, Error> {
        if bytes.len() < HDMX_HEADER_LEN {
            return Err(Error::UnexpectedEof);
        }
        let version = read_u16(bytes, 0)?;
        if version != HDMX_VERSION_0 {
            return Err(Error::BadStructure("hdmx: unrecognised version"));
        }
        let num_records = read_i16(bytes, 2)?;
        if num_records < 0 {
            return Err(Error::BadStructure("hdmx: negative numRecords"));
        }
        let size_device_record = read_i32(bytes, 4)?;
        let num_glyphs = expected_num_glyphs as usize;
        // The on-wire DeviceRecord is at least `2 + numGlyphs` bytes
        // (per-record header + widths[]). §5.7.2's "Each DeviceRecord
        // is padded with 0's to make it long word aligned" means
        // sizeDeviceRecord >= (2 + numGlyphs) and is a multiple of 4.
        // We enforce the lower bound. The multiple-of-4 expectation is
        // a SHOULD; a malformed font might break the alignment without
        // affecting the per-glyph lookup, so we accept any stride that
        // exceeds the minimum.
        let min_stride = HDMX_RECORD_HEADER_LEN
            .checked_add(num_glyphs)
            .ok_or(Error::BadStructure("hdmx: numGlyphs overflow"))?;
        if size_device_record < 0 || (size_device_record as usize) < min_stride {
            return Err(Error::BadStructure("hdmx: sizeDeviceRecord too small"));
        }
        let stride = size_device_record as usize;
        let total = stride
            .checked_mul(num_records as usize)
            .and_then(|n| n.checked_add(HDMX_HEADER_LEN))
            .ok_or(Error::BadStructure("hdmx: record array overflow"))?;
        if bytes.len() < total {
            return Err(Error::UnexpectedEof);
        }
        let mut records = Vec::with_capacity(num_records as usize);
        let mut prev_ppem: Option<u8> = None;
        for i in 0..num_records as usize {
            let off = HDMX_HEADER_LEN + i * stride;
            let pixel_size = read_u8(bytes, off)?;
            let max_width = read_u8(bytes, off + 1)?;
            // §5.7.2: "This table is sorted by pixel size." Enforce
            // strict monotonic increase so duplicates / out-of-order
            // entries do not silently shadow each other.
            if let Some(prev) = prev_ppem {
                if pixel_size <= prev {
                    return Err(Error::BadStructure(
                        "hdmx: pixelSize not strictly increasing",
                    ));
                }
            }
            prev_ppem = Some(pixel_size);
            let widths_off = off + HDMX_RECORD_HEADER_LEN;
            let widths = bytes[widths_off..widths_off + num_glyphs].to_vec();
            records.push(HdmxRecord {
                pixel_size,
                max_width,
                widths,
            });
        }
        Ok(Self {
            version,
            size_device_record,
            records,
        })
    }

    /// Raw `version` field. Always `0` for the only spec-defined
    /// version (`HDMX_VERSION_0`).
    pub fn version_raw(&self) -> u16 {
        self.version
    }

    /// On-wire `numRecords` field — equal to `records().len()` after a
    /// successful parse.
    pub fn num_records(&self) -> u16 {
        // records.len() fits in u16 because parse() walked up from a
        // non-negative i16 numRecords.
        self.records.len() as u16
    }

    /// On-wire `sizeDeviceRecord` field — the per-record stride. Always
    /// at least `2 + maxp.numGlyphs` per §5.7.2's record layout; the
    /// spec recommends a multiple of 4 for long-word alignment but we
    /// surface the raw int32 either way.
    pub fn size_device_record(&self) -> i32 {
        self.size_device_record
    }

    /// The full record array, in document order. §5.7.2 mandates
    /// strictly-increasing `pixelSize`; we enforce that at parse time,
    /// so iterating this slice walks ppem values in ascending order.
    pub fn records(&self) -> &[HdmxRecord] {
        &self.records
    }

    /// Pick the device record whose `pixelSize` equals `ppem`, or
    /// `None` when the table does not record that ppem. §5.7.2 has no
    /// "nearest neighbour" / "round down" rule — a rasteriser at an
    /// unrecorded ppem must fall back to grid-fitting the glyph.
    pub fn record_for_ppem(&self, ppem: u8) -> Option<&HdmxRecord> {
        // Strict-increasing invariant from parse() means a binary
        // search is correct without further duplicate handling.
        match self
            .records
            .binary_search_by_key(&ppem, HdmxRecord::pixel_size)
        {
            Ok(i) => self.records.get(i),
            Err(_) => None,
        }
    }

    /// Grid-fitted advance width of `glyph_id` at `ppem`, in integer
    /// pixels, per §5.7.2. Returns `None` when the table does not
    /// record that ppem, or when `glyph_id` is outside the per-record
    /// `widths[]` array.
    pub fn advance_pixels(&self, glyph_id: u16, ppem: u8) -> Option<u8> {
        self.record_for_ppem(ppem)?.advance_pixels(glyph_id)
    }

    /// The set of ppem sizes the table publishes, in ascending order.
    /// Convenience for callers that want to find the nearest recorded
    /// ppem to a target without iterating the full record array.
    pub fn recorded_ppem_sizes(&self) -> Vec<u8> {
        self.records.iter().map(HdmxRecord::pixel_size).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a wire-format `hdmx` table from a list of `(pixelSize,
    /// maxWidth, widths)` records, computing `sizeDeviceRecord` as the
    /// minimum spec-conformant value rounded up to a multiple of 4
    /// (the §5.7.2 long-alignment rule).
    fn make_hdmx(version: u16, num_glyphs: usize, recs: &[(u8, u8, &[u8])]) -> Vec<u8> {
        let min_stride = HDMX_RECORD_HEADER_LEN + num_glyphs;
        let stride = (min_stride + 3) & !3;
        let mut out = Vec::with_capacity(HDMX_HEADER_LEN + stride * recs.len());
        out.extend_from_slice(&version.to_be_bytes());
        out.extend_from_slice(&(recs.len() as i16).to_be_bytes());
        out.extend_from_slice(&(stride as i32).to_be_bytes());
        for &(ppem, max_w, widths) in recs {
            assert_eq!(widths.len(), num_glyphs);
            out.push(ppem);
            out.push(max_w);
            out.extend_from_slice(widths);
            // Padding to long-align this record.
            let pad = stride - HDMX_RECORD_HEADER_LEN - num_glyphs;
            out.resize(out.len() + pad, 0);
        }
        out
    }

    #[test]
    fn parses_two_record_table() {
        // 3-glyph font; records at 12 and 16 ppem. At 12 ppem all
        // glyphs are narrow; at 16 ppem the second glyph widens.
        let bytes = make_hdmx(
            HDMX_VERSION_0,
            3,
            &[(12, 7, &[0, 6, 7]), (16, 9, &[0, 8, 9])],
        );
        let t = HdmxTable::parse(&bytes, 3).expect("parse");
        assert_eq!(t.version_raw(), 0);
        assert_eq!(t.num_records(), 2);
        // Stride = 2 + 3 rounded up to 4 = 8.
        assert_eq!(t.size_device_record(), 8);
        assert_eq!(t.recorded_ppem_sizes(), vec![12, 16]);

        // Per-ppem lookup.
        assert_eq!(t.advance_pixels(0, 12), Some(0));
        assert_eq!(t.advance_pixels(2, 16), Some(9));
        assert_eq!(t.advance_pixels(2, 14), None); // ppem not in table
        assert_eq!(t.advance_pixels(3, 12), None); // glyph out of range

        // Per-record access.
        let r12 = t.record_for_ppem(12).expect("ppem 12 record");
        assert_eq!(r12.max_width(), 7);
        assert_eq!(r12.widths(), &[0, 6, 7]);
        assert_eq!(r12.advance_pixels(1), Some(6));
    }

    #[test]
    fn rejects_short_header() {
        // Anything shorter than the 8-byte header is UnexpectedEof.
        let bytes = vec![0u8; 7];
        assert!(matches!(
            HdmxTable::parse(&bytes, 0),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_unknown_version() {
        let mut bytes = vec![0u8; HDMX_HEADER_LEN];
        bytes[0..2].copy_from_slice(&1u16.to_be_bytes());
        assert!(matches!(
            HdmxTable::parse(&bytes, 0),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn rejects_negative_num_records() {
        let mut bytes = vec![0u8; HDMX_HEADER_LEN];
        bytes[2..4].copy_from_slice(&(-1i16).to_be_bytes());
        // sizeDeviceRecord doesn't matter; numRecords < 0 trips first.
        assert!(matches!(
            HdmxTable::parse(&bytes, 0),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn rejects_too_small_stride() {
        // numGlyphs = 4 so the stride must be at least 2 + 4 = 6 bytes;
        // we set it to 5 to trip the lower-bound check.
        let mut bytes = vec![0u8; HDMX_HEADER_LEN];
        bytes[2..4].copy_from_slice(&1i16.to_be_bytes());
        bytes[4..8].copy_from_slice(&5i32.to_be_bytes());
        assert!(matches!(
            HdmxTable::parse(&bytes, 4),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn rejects_truncated_body() {
        // numRecords = 2 but only one record's worth of bytes follow.
        let bytes = make_hdmx(HDMX_VERSION_0, 3, &[(12, 7, &[0, 6, 7])]);
        let mut bytes2 = bytes.clone();
        // Patch numRecords to 2 without growing the body.
        bytes2[2..4].copy_from_slice(&2i16.to_be_bytes());
        assert!(matches!(
            HdmxTable::parse(&bytes2, 3),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_non_monotonic_pixel_size() {
        // Two records with the same pixelSize trips the §5.7.2 sort
        // invariant.
        let bytes = make_hdmx(HDMX_VERSION_0, 2, &[(12, 5, &[0, 5]), (12, 6, &[0, 6])]);
        assert!(matches!(
            HdmxTable::parse(&bytes, 2),
            Err(Error::BadStructure(_))
        ));
        // Out-of-order records (descending ppem) also bail.
        let bytes2 = make_hdmx(HDMX_VERSION_0, 2, &[(16, 6, &[0, 6]), (12, 5, &[0, 5])]);
        assert!(matches!(
            HdmxTable::parse(&bytes2, 2),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn binary_search_picks_exact_ppem_only() {
        let bytes = make_hdmx(
            HDMX_VERSION_0,
            2,
            &[(10, 4, &[0, 4]), (14, 6, &[0, 6]), (20, 9, &[0, 9])],
        );
        let t = HdmxTable::parse(&bytes, 2).expect("parse");
        // Exact matches resolve.
        assert!(t.record_for_ppem(10).is_some());
        assert!(t.record_for_ppem(14).is_some());
        assert!(t.record_for_ppem(20).is_some());
        // Misses do not round to a neighbour — §5.7.2 has no fallback.
        assert!(t.record_for_ppem(11).is_none());
        assert!(t.record_for_ppem(16).is_none());
        assert!(t.record_for_ppem(255).is_none());
    }

    #[test]
    fn tolerates_extra_padding_in_stride() {
        // Force a larger sizeDeviceRecord (12 instead of the minimum 6
        // for numGlyphs = 4) — emulates a font built by a writer that
        // long-aligns aggressively. The parser honours the stride and
        // walks the records correctly.
        let num_glyphs = 4usize;
        let stride = 12usize;
        let mut bytes = Vec::with_capacity(HDMX_HEADER_LEN + stride * 2);
        bytes.extend_from_slice(&HDMX_VERSION_0.to_be_bytes());
        bytes.extend_from_slice(&2i16.to_be_bytes());
        bytes.extend_from_slice(&(stride as i32).to_be_bytes());
        // Record 1: ppem 12, widths [1,2,3,4], 6 bytes padding.
        bytes.push(12);
        bytes.push(4);
        bytes.extend_from_slice(&[1, 2, 3, 4]);
        bytes.extend_from_slice(&[0u8; 6]);
        // Record 2: ppem 16, widths [2,3,4,5], 6 bytes padding.
        bytes.push(16);
        bytes.push(5);
        bytes.extend_from_slice(&[2, 3, 4, 5]);
        bytes.extend_from_slice(&[0u8; 6]);

        let t = HdmxTable::parse(&bytes, num_glyphs as u16).expect("parse");
        assert_eq!(t.size_device_record(), 12);
        assert_eq!(t.advance_pixels(0, 12), Some(1));
        assert_eq!(t.advance_pixels(3, 16), Some(5));
    }

    #[test]
    fn empty_table_round_trips() {
        // num_records = 0: header-only table, no records to walk.
        let mut bytes = Vec::with_capacity(HDMX_HEADER_LEN);
        bytes.extend_from_slice(&HDMX_VERSION_0.to_be_bytes());
        bytes.extend_from_slice(&0i16.to_be_bytes());
        bytes.extend_from_slice(&8i32.to_be_bytes());
        // numGlyphs irrelevant when num_records == 0.
        let t = HdmxTable::parse(&bytes, 5).expect("parse");
        assert_eq!(t.num_records(), 0);
        assert!(t.records().is_empty());
        assert!(t.record_for_ppem(12).is_none());
        assert!(t.advance_pixels(0, 12).is_none());
    }
}
