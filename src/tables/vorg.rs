//! `VORG` — Vertical Origin Table.
//!
//! Spec: ISO/IEC 14496-22:2019 §5.4.4 ("VORG – Vertical origin table").
//! Optional table that records, per glyph, the Y coordinate in font
//! design units of the glyph's vertical origin. Used by vertical text
//! layout to position glyphs whose origin does not coincide with the
//! derived `topSideBearing + glyph_bounding_box.y_max` value computed
//! from `vmtx` + `glyf`.
//!
//! ## Scope per §5.4.4
//!
//! The spec restricts the table's use to CFF-flavoured sfnts: "This
//! table may be optionally present only in CFF OFF fonts. If present
//! in TrueType OFF fonts it must be ignored by font clients, just as
//! any other unrecognized table would be." A `glyf`-bearing sfnt that
//! ships a `VORG` is malformed per the spec but appears occasionally
//! in the wild; the parser reads it regardless and leaves the
//! ignore-on-TrueType policy to the [`crate::Font`] layer (whose
//! `vert_origin_y_from_vorg` accessor consults the table only when no
//! `glyf` table is present, mirroring the §5.4.4 restriction).
//!
//! ## On-disk layout
//!
//! ```text
//!   0  / 2  / majorVersion             (must be 1)
//!   2  / 2  / minorVersion             (must be 0)
//!   4  / 2  / defaultVertOriginY       (int16)
//!   6  / 2  / numVertOriginYMetrics
//!   8  / .. / vertOriginYMetrics[numVertOriginYMetrics]
//!              each entry: uint16 glyphIndex, int16 vertOriginY
//! ```
//!
//! Per §5.4.4 the metrics array must be:
//!  - sorted by increasing `glyphIndex`,
//!  - free of duplicate `glyphIndex` values, and
//!  - omitting glyphs whose `vertOriginY` equals `defaultVertOriginY`
//!    (the "size-optimized" form). A font with every glyph at the
//!    default ships `numVertOriginYMetrics == 0` and the entire table
//!    fits in 8 bytes.
//!
//! The parser validates the sort + de-duplication invariants up-front
//! so per-glyph lookup can binary-search the array without revalidating.

use crate::parser::{read_i16, read_u16};
use crate::Error;

/// Documented header version per §5.4.4 ("Set to 1").
pub const VORG_MAJOR_VERSION: u16 = 1;
/// Documented header minor version per §5.4.4 ("Set to 0").
pub const VORG_MINOR_VERSION: u16 = 0;

/// Sanity cap on the metrics-array entry count. The on-disk field is a
/// `uint16`, so the spec ceiling is 65535; that matches the cap below.
const MAX_METRICS_ENTRIES: usize = u16::MAX as usize;

/// Per-glyph vertical-origin override entry from the metrics array
/// (§5.4.4 second sub-table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertOriginEntry {
    /// Glyph index this entry applies to.
    pub glyph_index: u16,
    /// Y coordinate of the glyph's vertical origin in font design units.
    pub vert_origin_y: i16,
}

/// Parsed `VORG` table — header fields plus the sorted overrides array.
///
/// The metrics array is owned by the table so per-glyph queries are a
/// binary search rather than a re-scan of the original byte slice. The
/// table is small (8 bytes + 4 per override; for the common all-glyphs
/// -default case `metrics` is empty), so the allocation is negligible.
#[derive(Debug, Clone)]
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub struct VorgTable {
    /// `majorVersion` field — `VORG_MAJOR_VERSION` (= 1) per spec.
    pub major_version: u16,
    /// `minorVersion` field — `VORG_MINOR_VERSION` (= 0) per spec.
    pub minor_version: u16,
    /// `defaultVertOriginY` — fallback used for glyphs absent from the
    /// metrics array.
    pub default_vert_origin_y: i16,
    /// Per-glyph overrides, sorted by `glyph_index`.
    metrics: Vec<VertOriginEntry>,
}

impl VorgTable {
    /// Parse a `VORG` byte slice into the structured view.
    ///
    /// Validates the §5.4.4 invariants:
    ///  - `majorVersion == 1` and `minorVersion == 0` (the spec mandates
    ///    both verbatim);
    ///  - the metrics array fits inside the slice;
    ///  - the array is strictly sorted by `glyphIndex` with no
    ///    duplicates ("must be sorted by increasing glyphIndex, and
    ///    must not have more than one element with the same
    ///    glyphIndex").
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 8 {
            return Err(Error::UnexpectedEof);
        }
        let major_version = read_u16(bytes, 0)?;
        let minor_version = read_u16(bytes, 2)?;
        if major_version != VORG_MAJOR_VERSION {
            return Err(Error::BadStructure("VORG: majorVersion != 1"));
        }
        if minor_version != VORG_MINOR_VERSION {
            return Err(Error::BadStructure("VORG: minorVersion != 0"));
        }
        let default_vert_origin_y = read_i16(bytes, 4)?;
        let count = read_u16(bytes, 6)? as usize;
        if count > MAX_METRICS_ENTRIES {
            return Err(Error::BadStructure("VORG: numVertOriginYMetrics cap"));
        }
        let body_end = 8usize
            .checked_add(
                count
                    .checked_mul(4)
                    .ok_or(Error::BadStructure("VORG: numVertOriginYMetrics overflow"))?,
            )
            .ok_or(Error::BadStructure("VORG: numVertOriginYMetrics overflow"))?;
        if bytes.len() < body_end {
            return Err(Error::UnexpectedEof);
        }
        let mut metrics: Vec<VertOriginEntry> = Vec::with_capacity(count);
        let mut last: Option<u16> = None;
        for i in 0..count {
            let off = 8 + i * 4;
            let glyph_index = read_u16(bytes, off)?;
            let vert_origin_y = read_i16(bytes, off + 2)?;
            if let Some(prev) = last {
                if glyph_index <= prev {
                    return Err(Error::BadStructure(
                        "VORG: vertOriginYMetrics not strictly increasing",
                    ));
                }
            }
            last = Some(glyph_index);
            metrics.push(VertOriginEntry {
                glyph_index,
                vert_origin_y,
            });
        }
        Ok(Self {
            major_version,
            minor_version,
            default_vert_origin_y,
            metrics,
        })
    }

    /// Number of per-glyph override entries (= `numVertOriginYMetrics`).
    pub fn metrics_len(&self) -> usize {
        self.metrics.len()
    }

    /// Borrow the full metrics array. Sorted by `glyph_index`.
    pub fn metrics(&self) -> &[VertOriginEntry] {
        &self.metrics
    }

    /// Y coordinate of the vertical origin for `glyph_id`, in font
    /// design units. Returns the per-glyph override when one is
    /// present; otherwise the §5.4.4 `defaultVertOriginY`.
    pub fn vert_origin_y(&self, glyph_id: u16) -> i16 {
        match self
            .metrics
            .binary_search_by_key(&glyph_id, |e| e.glyph_index)
        {
            Ok(idx) => self.metrics[idx].vert_origin_y,
            Err(_) => self.default_vert_origin_y,
        }
    }

    /// Per-glyph override entry for `glyph_id`, if one is present in the
    /// metrics array. `None` indicates the glyph inherits
    /// `defaultVertOriginY` per §5.4.4.
    pub fn vert_origin_y_override(&self, glyph_id: u16) -> Option<i16> {
        self.metrics
            .binary_search_by_key(&glyph_id, |e| e.glyph_index)
            .ok()
            .map(|i| self.metrics[i].vert_origin_y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(default_y: i16, entries: &[(u16, i16)]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&VORG_MAJOR_VERSION.to_be_bytes());
        b.extend_from_slice(&VORG_MINOR_VERSION.to_be_bytes());
        b.extend_from_slice(&default_y.to_be_bytes());
        b.extend_from_slice(&(entries.len() as u16).to_be_bytes());
        for (gid, y) in entries {
            b.extend_from_slice(&gid.to_be_bytes());
            b.extend_from_slice(&y.to_be_bytes());
        }
        b
    }

    #[test]
    fn empty_metrics_table_uses_default_for_all_glyphs() {
        // §5.4.4: "If all glyphs in a font share the same
        // defaultVertOriginY value, the length of the 'VORG' table will
        // be 8 bytes in a size-optimized implementation, since the
        // vertOriginYMetrics array will be absent."
        let bytes = build(880, &[]);
        assert_eq!(bytes.len(), 8);
        let vorg = VorgTable::parse(&bytes).expect("parse");
        assert_eq!(vorg.default_vert_origin_y, 880);
        assert_eq!(vorg.metrics_len(), 0);
        assert_eq!(vorg.vert_origin_y(0), 880);
        assert_eq!(vorg.vert_origin_y(12345), 880);
        assert!(vorg.vert_origin_y_override(0).is_none());
    }

    #[test]
    fn worked_example_from_spec() {
        // §5.4.4 "complete VORG table for a 1000-unit-em font" — every
        // glyph defaults to 880 except gid 10, 12, 13.
        let bytes = build(880, &[(10, 889), (12, 861), (13, 849)]);
        let vorg = VorgTable::parse(&bytes).expect("parse");
        assert_eq!(vorg.default_vert_origin_y, 880);
        assert_eq!(vorg.metrics_len(), 3);
        // Glyphs without an entry inherit the default.
        assert_eq!(vorg.vert_origin_y(0), 880);
        assert_eq!(vorg.vert_origin_y(9), 880);
        assert_eq!(vorg.vert_origin_y(11), 880);
        assert_eq!(vorg.vert_origin_y(14), 880);
        // Per-glyph overrides hit the metrics array.
        assert_eq!(vorg.vert_origin_y(10), 889);
        assert_eq!(vorg.vert_origin_y(12), 861);
        assert_eq!(vorg.vert_origin_y(13), 849);
        assert_eq!(vorg.vert_origin_y_override(10), Some(889));
        assert_eq!(vorg.vert_origin_y_override(11), None);
    }

    #[test]
    fn rejects_short_header() {
        let b = vec![0u8; 7];
        assert!(matches!(VorgTable::parse(&b), Err(Error::UnexpectedEof)));
    }

    #[test]
    fn rejects_wrong_major_version() {
        let mut b = build(0, &[]);
        b[0..2].copy_from_slice(&2u16.to_be_bytes());
        assert!(matches!(VorgTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_nonzero_minor_version() {
        let mut b = build(0, &[]);
        b[2..4].copy_from_slice(&1u16.to_be_bytes());
        assert!(matches!(VorgTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_truncated_metrics_array() {
        // Claim 2 entries but only deliver the bytes for 1.
        let mut b = build(100, &[(5, 200)]);
        // Overwrite the count field so it claims 2 entries.
        b[6..8].copy_from_slice(&2u16.to_be_bytes());
        assert!(matches!(VorgTable::parse(&b), Err(Error::UnexpectedEof)));
    }

    #[test]
    fn rejects_unsorted_metrics_array() {
        // (10, ..), (8, ..) — not strictly increasing.
        let b = build(0, &[(10, 100), (8, 200)]);
        assert!(matches!(VorgTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_duplicate_glyph_index() {
        // (10, ..), (10, ..) — duplicate is forbidden per §5.4.4.
        let b = build(0, &[(10, 100), (10, 200)]);
        assert!(matches!(VorgTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn binary_search_handles_large_arrays() {
        let entries: Vec<(u16, i16)> = (0..512u16).map(|i| (i * 2, i as i16)).collect();
        let bytes = build(-1, &entries);
        let vorg = VorgTable::parse(&bytes).expect("parse");
        // Even-indexed glyphs hit overrides; odd-indexed inherit default.
        for i in 0..256u16 {
            assert_eq!(vorg.vert_origin_y(i * 2), i as i16);
            assert_eq!(vorg.vert_origin_y(i * 2 + 1), -1);
        }
        // Past the end of the array.
        assert_eq!(vorg.vert_origin_y(60_000), -1);
    }

    #[test]
    fn metrics_accessor_returns_sorted_array_in_full() {
        let bytes = build(0, &[(1, 10), (2, 20), (3, 30)]);
        let vorg = VorgTable::parse(&bytes).expect("parse");
        let arr = vorg.metrics();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].glyph_index, 1);
        assert_eq!(arr[0].vert_origin_y, 10);
        assert_eq!(arr[2].vert_origin_y, 30);
    }
}
