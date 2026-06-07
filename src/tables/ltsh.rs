//! `LTSH` — linear threshold table.
//!
//! Spec: ISO/IEC 14496-22:2019 §5.7.4 ("LTSH – Linear threshold"). This
//! optional table addresses a TrueType-specific quirk: instructions
//! applied to the sidebearings can make a glyph's advance width grid-fit
//! away from its design-unit linear value at small pixel sizes, forcing
//! a rasteriser to actually scan-convert the glyph before it can answer
//! "what advance width does this glyph use at ppem N?" §5.7.4 lets the
//! font publish, on a per-glyph basis, the lowest ppem at which the
//! grid-fitted (instructed) advance width has converged on the rounded
//! linear advance — i.e. the threshold at which the rasteriser can
//! short-circuit the scan-convert and just round the design-unit advance
//! to integer pixels.
//!
//! ## Layout (§5.7.4)
//!
//! ```text
//! uint16 version    // starts at 0
//! uint16 numGlyphs  // matches numGlyphs in 'maxp'
//! uint8  yPels[numGlyphs]
//! ```
//!
//! Total on-wire length is therefore `4 + numGlyphs` bytes. §5.7.4 notes
//! that "glyphs which do not have instructions on their sidebearings
//! have yPels = 1; i.e., always scales linearly", so a font that publishes
//! a hinted glyph set will typically carry an array dominated by `1`
//! values, with a handful of glyphs flagged at the actual threshold
//! ppem at which their instructed advance converges with the linear
//! advance.
//!
//! ## Convergence criterion (§5.7.4)
//!
//! The "linearly scaled" criterion the table records is one of:
//!
//! a) `ppem ≥ 50` *and* `|rounded_linear − rounded_instructed| ≤ 2 % ·
//!    rounded_linear`, **or**
//! b) `linear_width == instructed_width`.
//!
//! §5.7.4 also states the spec invariant a parser does not check but a
//! shaper depends on: once a glyph hits its threshold ppem the rasteriser
//! remains linear at every larger ppem (no record of, e.g., 55 ppem
//! becoming non-linear again at 90 ppem).
//!
//! ## Eligibility (§5.7.4 + §5.2.3 head.flags bit 4)
//!
//! §5.7.4 directs that `LTSH` "should *not* be included unless bit 4 of
//! the 'flags' field in the 'head' table is set." Bit 4 means
//! "instructions may depend on point size." We parse the table whenever
//! it is present — the parser's job is to surface the bytes the font
//! ships, not to second-guess the font author about the head-flags
//! pairing. A caller that wants to honour the §5.7.4 recommendation
//! can cross-check head.flags before consulting `yPels()`.
//!
//! ## VDMX / hdmx neighbours (§5.7.4 introduction)
//!
//! §5.7.4 calls out the `hdmx` and `vdmx` tables as the alternative
//! solutions to the same speed problem. `hdmx` publishes precomputed
//! grid-fitted advance widths at selected ppem sizes; `vdmx` does the
//! same for vertical advance. `LTSH` is the third, complementary
//! method — instead of a precomputed advance, it records the ppem at
//! which the grid-fit advance can be trusted to round to the linear
//! advance, and the rasteriser computes the advance arithmetically.

use crate::parser::{read_u16, read_u8};
use crate::Error;

/// On-wire table tag (`b"LTSH"`, big-endian Fixed `0x4C545348`). Exposed
/// for callers that walk the table directory directly.
pub const LTSH_TABLE_TAG: u32 = 0x4C54_5348;

/// The §5.7.4 "always scales linearly" sentinel (`yPels = 1`). Per the
/// spec note "Glyphs which do not have instructions on their
/// sidebearings have yPels = 1." Use [`LtshTable::is_always_linear`] for
/// the typed question.
pub const LTSH_ALWAYS_LINEAR: u8 = 1;

/// The only currently-defined `LTSH` version (`0`). §5.7.4 reserves the
/// `uint16 version` field for future expansion.
pub const LTSH_VERSION_0: u16 = 0;

/// Parsed `LTSH` table.
///
/// Storage is the on-wire `yPels` array as-is. A `numGlyphs` mismatch
/// against the parent `maxp` is detected at parse time when the caller
/// passes `expected_num_glyphs`, otherwise the parser accepts whatever
/// length the header declares (subject to the slice carrying enough
/// bytes).
#[derive(Debug, Clone)]
pub struct LtshTable {
    version: u16,
    y_pels: Vec<u8>,
}

impl LtshTable {
    /// Parse an `LTSH` table from its raw slice without cross-checking
    /// against `maxp.numGlyphs`. The on-wire `numGlyphs` field is honoured
    /// for sizing.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        Self::parse_inner(bytes, None)
    }

    /// Parse and cross-check that the on-wire `numGlyphs` matches the
    /// font's `maxp.numGlyphs`. §5.7.4 says the field "should be the
    /// same as the numGlyphs field in the 'maxp' table"; a mismatch
    /// would either truncate or over-read the parent's glyph table.
    pub fn parse_with_glyph_count(bytes: &[u8], expected_num_glyphs: u16) -> Result<Self, Error> {
        Self::parse_inner(bytes, Some(expected_num_glyphs))
    }

    fn parse_inner(bytes: &[u8], expected: Option<u16>) -> Result<Self, Error> {
        if bytes.len() < 4 {
            return Err(Error::UnexpectedEof);
        }
        let version = read_u16(bytes, 0)?;
        if version != LTSH_VERSION_0 {
            return Err(Error::BadStructure("LTSH: unrecognised version"));
        }
        let num_glyphs = read_u16(bytes, 2)?;
        if let Some(exp) = expected {
            if num_glyphs != exp {
                return Err(Error::BadStructure(
                    "LTSH: numGlyphs disagrees with maxp.numGlyphs",
                ));
            }
        }
        // yPels[numGlyphs] is uint8 per §5.7.4 so the body length is
        // exactly `numGlyphs` bytes — the table is `4 + numGlyphs` bytes
        // total. A short slice is `UnexpectedEof`; trailing padding past
        // the declared length is accepted (sfnt-conformant fonts often
        // pad table records to 4-byte boundaries).
        let body_off = 4usize;
        let body_end = body_off
            .checked_add(num_glyphs as usize)
            .ok_or(Error::BadStructure("LTSH: numGlyphs overflow"))?;
        if bytes.len() < body_end {
            return Err(Error::UnexpectedEof);
        }
        let mut y_pels = Vec::with_capacity(num_glyphs as usize);
        for i in 0..num_glyphs as usize {
            y_pels.push(read_u8(bytes, body_off + i)?);
        }
        Ok(Self { version, y_pels })
    }

    /// Raw `version` field. Always `0` for the only spec-defined
    /// version (`LTSH_VERSION_0`).
    pub fn version_raw(&self) -> u16 {
        self.version
    }

    /// Glyph count declared in the table header — equal to `yPels.len()`
    /// after a successful parse.
    pub fn num_glyphs(&self) -> u16 {
        // y_pels.len() fits in u16 because parse() walked it up from a
        // u16 numGlyphs.
        self.y_pels.len() as u16
    }

    /// The full `yPels[]` array, in glyph-index order.
    pub fn y_pels(&self) -> &[u8] {
        &self.y_pels
    }

    /// Lowest ppem at which `glyph_id`'s grid-fitted advance has converged
    /// on its rounded linear advance per §5.7.4 (i.e. at every ppem at
    /// least this value the rasteriser may round the linear advance
    /// without scan-converting). Returns `None` for `glyph_id` outside
    /// the array.
    pub fn linear_threshold(&self, glyph_id: u16) -> Option<u8> {
        self.y_pels.get(glyph_id as usize).copied()
    }

    /// `true` when `glyph_id` is flagged as "always scales linearly" —
    /// i.e. `yPels = 1`, the §5.7.4 sentinel for glyphs without
    /// instructions on their sidebearings. Returns `false` for
    /// out-of-range indices.
    pub fn is_always_linear(&self, glyph_id: u16) -> bool {
        self.linear_threshold(glyph_id) == Some(LTSH_ALWAYS_LINEAR)
    }

    /// `true` when the grid-fitted advance for `glyph_id` is safe to
    /// approximate as the rounded linear advance at the requested
    /// `ppem` size per §5.7.4 (i.e. `ppem >= yPels[glyph_id]`). Returns
    /// `false` when the glyph is below threshold or when `glyph_id`
    /// is out of range (a caller faced with `false` should grid-fit
    /// the glyph rather than trust a linear approximation).
    pub fn linearly_scales_at_ppem(&self, glyph_id: u16, ppem: u16) -> bool {
        match self.linear_threshold(glyph_id) {
            Some(threshold) => ppem >= threshold as u16,
            None => false,
        }
    }

    /// `true` when every glyph in the table is flagged `yPels = 1` —
    /// the table publishes no thresholds beyond the §5.7.4 sentinel,
    /// equivalent to saying "advance always scales linearly for every
    /// glyph at every ppem." A consumer may short-circuit further
    /// per-glyph lookups in this case.
    pub fn all_always_linear(&self) -> bool {
        !self.y_pels.is_empty() && self.y_pels.iter().all(|&y| y == LTSH_ALWAYS_LINEAR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a wire-format `LTSH` table from a `yPels[]` array.
    fn make_ltsh(version: u16, y_pels: &[u8]) -> Vec<u8> {
        let mut b = Vec::with_capacity(4 + y_pels.len());
        b.extend_from_slice(&version.to_be_bytes());
        b.extend_from_slice(&(y_pels.len() as u16).to_be_bytes());
        b.extend_from_slice(y_pels);
        b
    }

    #[test]
    fn parses_minimal_three_glyph_table() {
        // Hypothetical "tiny" font with 3 glyphs: .notdef + two glyphs;
        // the second glyph has instructed sidebearings that converge at
        // 12 ppem.
        let bytes = make_ltsh(LTSH_VERSION_0, &[1, 1, 12]);
        let t = LtshTable::parse(&bytes).expect("parse");
        assert_eq!(t.version_raw(), 0);
        assert_eq!(t.num_glyphs(), 3);
        assert_eq!(t.y_pels(), &[1, 1, 12]);
        assert_eq!(t.linear_threshold(0), Some(1));
        assert_eq!(t.linear_threshold(2), Some(12));
        assert_eq!(t.linear_threshold(3), None);
    }

    #[test]
    fn rejects_short_header() {
        // Anything shorter than the 4-byte header is UnexpectedEof.
        let bytes = vec![0u8; 3];
        assert!(matches!(
            LtshTable::parse(&bytes),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_truncated_body() {
        // numGlyphs = 5, but only 2 bytes of yPels.
        let mut bytes = Vec::with_capacity(6);
        bytes.extend_from_slice(&0u16.to_be_bytes());
        bytes.extend_from_slice(&5u16.to_be_bytes());
        bytes.extend_from_slice(&[1u8, 1u8]);
        assert!(matches!(
            LtshTable::parse(&bytes),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_unknown_version() {
        let bytes = make_ltsh(2, &[1, 1, 1]);
        assert!(matches!(
            LtshTable::parse(&bytes),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn rejects_maxp_glyph_count_mismatch() {
        let bytes = make_ltsh(LTSH_VERSION_0, &[1, 1, 1]);
        // Font claims 4 glyphs, LTSH claims 3 — mismatch.
        assert!(matches!(
            LtshTable::parse_with_glyph_count(&bytes, 4),
            Err(Error::BadStructure(_))
        ));
        // Matching count parses.
        let t = LtshTable::parse_with_glyph_count(&bytes, 3).expect("parse");
        assert_eq!(t.num_glyphs(), 3);
    }

    #[test]
    fn always_linear_sentinel_detected() {
        let bytes = make_ltsh(LTSH_VERSION_0, &[1, 1, 1, 1]);
        let t = LtshTable::parse(&bytes).expect("parse");
        assert!(t.all_always_linear());
        for gid in 0..4 {
            assert!(t.is_always_linear(gid));
        }
        // Out-of-range glyph is not "always linear" — it's not in the
        // table at all.
        assert!(!t.is_always_linear(4));
    }

    #[test]
    fn mixed_threshold_table_not_all_linear() {
        let bytes = make_ltsh(LTSH_VERSION_0, &[1, 1, 8, 1]);
        let t = LtshTable::parse(&bytes).expect("parse");
        assert!(!t.all_always_linear());
        assert!(t.is_always_linear(0));
        assert!(!t.is_always_linear(2));
        assert_eq!(t.linear_threshold(2), Some(8));
    }

    #[test]
    fn linearly_scales_at_ppem_threshold() {
        // §5.7.4 criterion: at ppem >= yPels[gid] the advance is safe to
        // linearly scale. Below that, the rasteriser should grid-fit.
        let bytes = make_ltsh(LTSH_VERSION_0, &[1, 1, 24, 50]);
        let t = LtshTable::parse(&bytes).expect("parse");
        // Always-linear sentinel: linear at ppem >= 1 (i.e. any size).
        assert!(t.linearly_scales_at_ppem(0, 8));
        // Threshold 24: ppem 23 is below, 24 / 25 are above.
        assert!(!t.linearly_scales_at_ppem(2, 23));
        assert!(t.linearly_scales_at_ppem(2, 24));
        assert!(t.linearly_scales_at_ppem(2, 25));
        // Threshold 50 matches §5.7.4 criterion a) "ppem size ≥ 50".
        assert!(!t.linearly_scales_at_ppem(3, 49));
        assert!(t.linearly_scales_at_ppem(3, 50));
        // Out-of-range glyph stays false.
        assert!(!t.linearly_scales_at_ppem(4, 100));
    }

    #[test]
    fn empty_table_round_trips_through_parser() {
        // numGlyphs == 0 is a degenerate but spec-valid table; the body
        // is the bare 4-byte header.
        let bytes = make_ltsh(LTSH_VERSION_0, &[]);
        let t = LtshTable::parse(&bytes).expect("parse");
        assert_eq!(t.num_glyphs(), 0);
        assert!(t.y_pels().is_empty());
        // No glyphs => `all_always_linear` is `false` (vacuous-truth-as-
        // useful-default: a caller asking the question expects at least
        // one glyph to be reported).
        assert!(!t.all_always_linear());
    }

    #[test]
    fn parser_accepts_trailing_pad_bytes() {
        // sfnt aligns each table record to a 4-byte boundary; a
        // 3-glyph LTSH is 7 bytes on the wire and one pad byte may
        // follow. The parser ignores the pad.
        let mut bytes = make_ltsh(LTSH_VERSION_0, &[1, 1, 8]);
        bytes.push(0u8);
        let t = LtshTable::parse(&bytes).expect("parse");
        assert_eq!(t.num_glyphs(), 3);
        assert_eq!(t.y_pels(), &[1, 1, 8]);
    }

    #[test]
    fn tag_bytes_match_constant() {
        assert_eq!(
            u32::from_be_bytes(*b"LTSH"),
            LTSH_TABLE_TAG,
            "LTSH_TABLE_TAG = 0x{:08X}",
            LTSH_TABLE_TAG
        );
    }
}
