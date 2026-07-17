//! `gasp` — grid-fitting and scan-conversion procedure table.
//!
//! Spec: ISO/IEC 14496-22:2019 §5.3.7 ("gasp – Grid-fitting and
//! scan-conversion procedure table"). The table is optional. When a
//! font omits it, rasterisers apply default policy across all ppem
//! ranges. When present, it carries a sorted list of `(ppem, flags)`
//! records that recommend grid-fitting / grayscale / ClearType
//! behaviour up to each `rangeMaxPPEM` limit. The §5.3.7 invariants:
//!
//! * `version ∈ {0, 1}` — version 0 omits the two ClearType flags
//!   (`GASP_SYMMETRIC_GRIDFIT`, `GASP_SYMMETRIC_SMOOTHING`); version 1
//!   defines them.
//! * `numRanges` records of `(uint16 rangeMaxPPEM, uint16
//!   rangeGaspBehavior)` follow the 4-byte header (4 bytes each, total
//!   `4 + 4*numRanges` bytes).
//! * Records "must be sorted in order of increasing rangeMaxPPEM
//!   value." §5.3.7 also calls out the `0xFFFF` sentinel "should" rule
//!   for the final record; we enforce the sort invariant and surface
//!   the sentinel via [`Self::covers_all_sizes`].
//! * `rangeGaspBehavior` reserved bits (`0xFFF0`) are documented "set
//!   to 0" but we tolerate non-zero reserved bits so a typo from a font
//!   tool does not lock the consumer out of a per-glyph rasterisation
//!   hint that is otherwise valid.
//!
//! ## Lookup
//!
//! [`Self::behavior_for_ppem`] picks the first record whose
//! `rangeMaxPPEM` is at least the requested ppem. This mirrors the
//! §5.3.7 description "Upper limit of range, in PPEM" — a ppem of 12
//! against records `[8, 16, 0xFFFF]` lands on the second record. A ppem
//! larger than every limit falls through to `None`, which a caller
//! should treat as "apply default rasterisation policy" (the same
//! behaviour the spec prescribes for an absent table).
//!
//! ## MVAR coupling
//!
//! §5.3.7 ("'gasp' Table and OFF Font Variations") declares ten MVAR
//! value tags `gsp0` … `gsp9` that interpolate the `rangeMaxPPEM` of
//! up to ten ranges in a variable font. Parsing here exposes the
//! static (default-instance) ranges; callers reach
//! `Font::metric_variation_delta(tag)` against the `gsp0`..`gsp9` tags
//! to fold per-instance adjustments into the upper-limit ppem value
//! before lookup. The last record's `rangeMaxPPEM` (commonly
//! `0xFFFF`) is "never adjusted" per §5.3.7.

use crate::parser::{read_u16, read_u32};
use crate::Error;

/// `gasp` table version 0 — pre-ClearType layout. Versions 0 and 1
/// share the same byte layout; the version field gates whether the
/// two `GASP_SYMMETRIC_*` flags in `rangeGaspBehavior` are meaningful.
pub const GASP_VERSION_0: u16 = 0;
/// `gasp` table version 1 — adds the two ClearType symmetric-smoothing
/// flags to `rangeGaspBehavior`.
pub const GASP_VERSION_1: u16 = 1;

/// Sentinel `rangeMaxPPEM` used in the final record to mean "all sizes
/// larger than the previous record's limit" (§5.3.7). When the only
/// record in a `gasp` table carries this value the table is a single
/// global behaviour-recommendation.
pub const GASP_PPEM_SENTINEL: u16 = 0xFFFF;

/// `GASP_GRIDFIT` — recommend grid-fitting (hinting) inside this ppem
/// range. §5.3.7 RangeGaspBehavior bit 0.
pub const GASP_GRIDFIT: u16 = 0x0001;
/// `GASP_DOGRAY` — recommend grayscale rendering inside this ppem
/// range. §5.3.7 RangeGaspBehavior bit 1.
pub const GASP_DOGRAY: u16 = 0x0002;
/// `GASP_SYMMETRIC_GRIDFIT` — recommend ClearType symmetric grid-
/// fitting inside this ppem range. §5.3.7 RangeGaspBehavior bit 2;
/// only meaningful when the table is version 1.
pub const GASP_SYMMETRIC_GRIDFIT: u16 = 0x0004;
/// `GASP_SYMMETRIC_SMOOTHING` — recommend ClearType smoothing along
/// multiple axes inside this ppem range. §5.3.7 RangeGaspBehavior bit
/// 3; only meaningful when the table is version 1.
pub const GASP_SYMMETRIC_SMOOTHING: u16 = 0x0008;

/// Reserved RangeGaspBehavior bit mask (§5.3.7 "Reserved flags – set
/// to 0"). Exposed as a constant so consumers can mask the reserved
/// bits off when comparing against the defined flag set.
pub const GASP_RESERVED_MASK: u16 = 0xFFF0;

/// One `(rangeMaxPPEM, rangeGaspBehavior)` record. `range_max_ppem` is
/// the inclusive upper ppem limit at which `flags` applies; the lower
/// limit is one greater than the preceding record's `range_max_ppem`
/// (or zero for the first record).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GaspRange {
    /// Inclusive upper limit of the ppem range, in pixels per em.
    /// The sentinel `0xFFFF` means "all sizes ≥ the previous record's
    /// limit + 1" — see [`GASP_PPEM_SENTINEL`].
    pub range_max_ppem: u16,
    /// Bit field of `GASP_*` flags. Use [`Self::has_flag`] for typed
    /// checks. Reserved bits (`0xFFF0`) are preserved verbatim from
    /// the input so a caller can introspect them if needed.
    pub flags: u16,
}

impl GaspRange {
    /// `true` when `flag` (one of the `GASP_*` masks) is set.
    pub fn has_flag(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }

    /// Recommend grid-fitting (hinting)? §5.3.7 bit 0.
    pub fn gridfit(&self) -> bool {
        self.has_flag(GASP_GRIDFIT)
    }

    /// Recommend grayscale rendering? §5.3.7 bit 1.
    pub fn dogray(&self) -> bool {
        self.has_flag(GASP_DOGRAY)
    }

    /// Recommend ClearType symmetric grid-fitting? §5.3.7 bit 2.
    /// Only meaningful in a version-1 table.
    pub fn symmetric_gridfit(&self) -> bool {
        self.has_flag(GASP_SYMMETRIC_GRIDFIT)
    }

    /// Recommend ClearType symmetric smoothing along multiple axes?
    /// §5.3.7 bit 3. Only meaningful in a version-1 table.
    pub fn symmetric_smoothing(&self) -> bool {
        self.has_flag(GASP_SYMMETRIC_SMOOTHING)
    }

    /// Bits the spec marks reserved (`0xFFF0`) that nonetheless came in
    /// non-zero. Useful when round-tripping the table or surfacing a
    /// diagnostic to the font author; the consumer should ignore these
    /// for rasterisation decisions.
    pub fn reserved_bits(&self) -> u16 {
        self.flags & GASP_RESERVED_MASK
    }
}

/// Parsed `gasp` table.
#[derive(Debug, Clone)]
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub struct GaspTable {
    version: u16,
    ranges: Vec<GaspRange>,
}

impl GaspTable {
    /// Parse a `gasp` table from its raw slice.
    ///
    /// Returns:
    /// - `Error::UnexpectedEof` if the slice is too short for the
    ///   declared `numRanges`.
    /// - `Error::BadStructure` if the version is neither 0 nor 1, or
    ///   if the `gaspRange` array is not sorted by increasing
    ///   `rangeMaxPPEM` (§5.3.7).
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 4 {
            return Err(Error::UnexpectedEof);
        }
        let version = read_u16(bytes, 0)?;
        match version {
            GASP_VERSION_0 | GASP_VERSION_1 => {}
            _ => return Err(Error::BadStructure("gasp: unrecognised version")),
        }
        let num_ranges = read_u16(bytes, 2)? as usize;
        // Each record is 4 bytes: uint16 rangeMaxPPEM + uint16
        // rangeGaspBehavior. The slice must be long enough for the
        // declared count; we reject implausible counts that overflow
        // the slice length without allocating.
        let body_len = num_ranges
            .checked_mul(4)
            .ok_or(Error::BadStructure("gasp: numRanges overflow"))?;
        let total = body_len
            .checked_add(4)
            .ok_or(Error::BadStructure("gasp: numRanges overflow"))?;
        if bytes.len() < total {
            return Err(Error::UnexpectedEof);
        }
        let mut ranges = Vec::with_capacity(num_ranges);
        let mut prev_max: Option<u16> = None;
        for i in 0..num_ranges {
            let off = 4 + i * 4;
            let range_max_ppem = read_u16(bytes, off)?;
            let flags = read_u16(bytes, off + 2)?;
            // §5.3.7: "records in the gaspRange[] array must be sorted
            // in order of increasing rangeMaxPPEM value." A duplicate
            // rangeMaxPPEM would shadow the latter record; reject.
            if let Some(prev) = prev_max {
                if range_max_ppem <= prev {
                    return Err(Error::BadStructure(
                        "gasp: gaspRange not sorted by increasing rangeMaxPPEM",
                    ));
                }
            }
            prev_max = Some(range_max_ppem);
            ranges.push(GaspRange {
                range_max_ppem,
                flags,
            });
        }
        Ok(Self { version, ranges })
    }

    /// Raw version field (0 or 1). Use [`Self::is_version_1`] for the
    /// typed question.
    pub fn version_raw(&self) -> u16 {
        self.version
    }

    /// `true` when the table is version 1 (ClearType-aware bits 2 and
    /// 3 of `rangeGaspBehavior` are meaningful). When `false`, callers
    /// should ignore `GASP_SYMMETRIC_GRIDFIT` /
    /// `GASP_SYMMETRIC_SMOOTHING` in returned records — the §5.3.7
    /// RangeGaspBehavior table marks them "only supported in version
    /// 1 of 'gasp' table."
    pub fn is_version_1(&self) -> bool {
        self.version == GASP_VERSION_1
    }

    /// Every `GaspRange` record, in on-wire order (sorted by
    /// increasing `rangeMaxPPEM` per §5.3.7).
    pub fn ranges(&self) -> &[GaspRange] {
        &self.ranges
    }

    /// `true` when the table contains a single record carrying the
    /// `0xFFFF` sentinel `rangeMaxPPEM` (§5.3.7 "If the only entry in
    /// 'gasp' is the 0xFFFF sentinel value, the behavior described
    /// will be used for all sizes."). A consumer can short-circuit
    /// per-ppem lookup in this case.
    pub fn covers_all_sizes(&self) -> bool {
        matches!(self.ranges.as_slice(),
                 [r] if r.range_max_ppem == GASP_PPEM_SENTINEL)
    }

    /// Pick the `GaspRange` that governs rasterisation at `ppem` —
    /// the first record whose `range_max_ppem` is at least `ppem`.
    /// Returns `None` when every record's upper limit is strictly less
    /// than `ppem`, in which case the consumer should fall back to
    /// the rasteriser default (§5.3.7 "may apply default rules").
    pub fn behavior_for_ppem(&self, ppem: u16) -> Option<&GaspRange> {
        self.ranges.iter().find(|r| r.range_max_ppem >= ppem)
    }
}

/// `gasp` table tag (`b"gasp"`, Fixed `0x67617370`). Exposed for
/// callers that walk the table directory directly.
pub const GASP_TABLE_TAG: u32 = 0x6761_7370;

#[allow(dead_code)] // kept for tag-arithmetic callers
fn gasp_tag_fixed() -> u32 {
    let raw = b"gasp";
    read_u32(raw, 0).expect("4-byte literal")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a wire-format `gasp` table from a list of
    /// `(rangeMaxPPEM, flags)` pairs at the given version.
    fn make_gasp(version: u16, records: &[(u16, u16)]) -> Vec<u8> {
        let mut b = Vec::with_capacity(4 + records.len() * 4);
        b.extend_from_slice(&version.to_be_bytes());
        b.extend_from_slice(&(records.len() as u16).to_be_bytes());
        for (ppem, flags) in records {
            b.extend_from_slice(&ppem.to_be_bytes());
            b.extend_from_slice(&flags.to_be_bytes());
        }
        b
    }

    #[test]
    fn parses_spec_sample_v1_four_ranges() {
        // §5.3.7 "Sample 'gasp' table": version 1, four records:
        //   (8,    0x000A) — DOGRAY | SYMMETRIC_SMOOTHING
        //   (16,   0x0005) — GRIDFIT | SYMMETRIC_GRIDFIT
        //   (19,   0x0007) — GRIDFIT | DOGRAY | SYMMETRIC_GRIDFIT
        //   (FFFF, 0x000F) — all four flags
        let bytes = make_gasp(
            GASP_VERSION_1,
            &[(8, 0x000A), (16, 0x0005), (19, 0x0007), (0xFFFF, 0x000F)],
        );
        let g = GaspTable::parse(&bytes).unwrap();
        assert!(g.is_version_1());
        assert_eq!(g.ranges().len(), 4);
        // Behaviour lookup: ppem 8 lands on the first record.
        let r = g.behavior_for_ppem(8).unwrap();
        assert!(r.dogray());
        assert!(r.symmetric_smoothing());
        assert!(!r.gridfit());
        // ppem 12 falls inside the (8, 16] range — second record.
        let r = g.behavior_for_ppem(12).unwrap();
        assert!(r.gridfit());
        assert!(r.symmetric_gridfit());
        assert!(!r.dogray());
        // ppem 19 lands on the third record (inclusive upper limit).
        let r = g.behavior_for_ppem(19).unwrap();
        assert!(r.gridfit());
        assert!(r.dogray());
        assert!(r.symmetric_gridfit());
        // Anything past 19 lands on the sentinel.
        let r = g.behavior_for_ppem(20).unwrap();
        assert_eq!(r.range_max_ppem, GASP_PPEM_SENTINEL);
        assert!(r.gridfit() && r.dogray() && r.symmetric_gridfit() && r.symmetric_smoothing());
        // A version-1 sample isn't "covers all sizes" unless it's a
        // single sentinel record.
        assert!(!g.covers_all_sizes());
    }

    #[test]
    fn single_sentinel_covers_all_sizes() {
        // §5.3.7: "If the only entry in 'gasp' is the 0xFFFF sentinel
        // value, the behavior described will be used for all sizes."
        let bytes = make_gasp(GASP_VERSION_0, &[(0xFFFF, GASP_GRIDFIT | GASP_DOGRAY)]);
        let g = GaspTable::parse(&bytes).unwrap();
        assert!(g.covers_all_sizes());
        assert_eq!(g.version_raw(), GASP_VERSION_0);
        assert!(!g.is_version_1());
        // Every ppem maps to the same record.
        for &ppem in &[1u16, 12, 96, 0xFFFE] {
            let r = g.behavior_for_ppem(ppem).unwrap();
            assert!(r.gridfit());
            assert!(r.dogray());
            // Version 0 reads the ClearType bits as semantically
            // meaningless — but the record is parsed verbatim, so the
            // bit-level accessors still return their raw state. The
            // sample sets neither.
            assert!(!r.symmetric_gridfit());
            assert!(!r.symmetric_smoothing());
        }
    }

    #[test]
    fn rejects_unsorted_records() {
        // 8 then 6 — strictly decreasing, must reject per §5.3.7.
        let bytes = make_gasp(GASP_VERSION_0, &[(8, 0x0001), (6, 0x0002)]);
        let err = GaspTable::parse(&bytes).unwrap_err();
        match err {
            Error::BadStructure(s) => assert!(s.contains("sorted")),
            other => panic!("expected BadStructure, got {other:?}"),
        }
    }

    #[test]
    fn rejects_duplicate_rangemaxppem() {
        // Same rangeMaxPPEM twice would shadow the second record's
        // behaviour and break the strict-increasing invariant.
        let bytes = make_gasp(GASP_VERSION_1, &[(16, 0x0001), (16, 0x0002)]);
        assert!(GaspTable::parse(&bytes).is_err());
    }

    #[test]
    fn rejects_unrecognised_version() {
        let bytes = make_gasp(2, &[(0xFFFF, 0x0003)]);
        assert!(GaspTable::parse(&bytes).is_err());
    }

    #[test]
    fn rejects_short_header() {
        // Three bytes — header is four.
        let bytes = vec![0u8, 0, 0];
        assert!(matches!(
            GaspTable::parse(&bytes),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_truncated_records() {
        // numRanges declares 2 records (8 bytes) but only 4 bytes of
        // record body follow the 4-byte header.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&GASP_VERSION_0.to_be_bytes());
        bytes.extend_from_slice(&2u16.to_be_bytes());
        bytes.extend_from_slice(&8u16.to_be_bytes());
        bytes.extend_from_slice(&0x0001u16.to_be_bytes());
        // Only one full record present; second one missing.
        assert!(matches!(
            GaspTable::parse(&bytes),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn reserved_bits_preserved_but_dont_block_parse() {
        // Reserved bits (0xFFF0) per §5.3.7 are "set to 0", but we
        // tolerate non-zero values so a sloppy font tool does not
        // lock the consumer out of an otherwise valid record.
        let bytes = make_gasp(GASP_VERSION_1, &[(0xFFFF, 0x8010 | GASP_GRIDFIT)]);
        let g = GaspTable::parse(&bytes).unwrap();
        let r = &g.ranges()[0];
        assert!(r.gridfit());
        assert_eq!(r.reserved_bits(), 0x8010);
    }

    #[test]
    fn behavior_for_ppem_falls_off_end() {
        // A `gasp` table without a sentinel can leave huge ppem
        // values unmatched. §5.3.7 advises a sentinel "should" be
        // present; when it isn't, lookup falls through to the
        // rasteriser default.
        let bytes = make_gasp(GASP_VERSION_0, &[(8, 0x0002), (16, 0x0003)]);
        let g = GaspTable::parse(&bytes).unwrap();
        assert!(g.behavior_for_ppem(17).is_none());
    }

    #[test]
    fn gasp_tag_is_four_byte_ascii_literal() {
        // Sanity: the documented tag constant matches the ASCII tag.
        assert_eq!(GASP_TABLE_TAG, gasp_tag_fixed());
    }
}
