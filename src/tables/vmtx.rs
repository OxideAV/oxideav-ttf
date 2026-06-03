//! `vmtx` — vertical metrics table.
//!
//! Spec: ISO/IEC 14496-22:2019 §5.7.10 ("vmtx – Vertical metric
//! table"). The structural cousin of `hmtx` for vertical fonts: the
//! per-glyph advance-height and top-side-bearing pairs that drive
//! glyph-to-glyph vertical advancement in a vertically-laid-out
//! script (CJK, Mongolian, …).
//!
//! ## Two-array layout (§5.7.10)
//!
//! Like `hmtx`, the table is two concatenated arrays with no header:
//!
//! 1. **`vMetrics`** — `numOfLongVerMetrics` entries, each
//!    `(advanceHeight: uint16, topSideBearing: int16)`. The count
//!    comes from `vhea.numOfLongVerMetrics`.
//! 2. **Optional `topSideBearing[]` tail** — `numGlyphs -
//!    numOfLongVerMetrics` bare `int16` top-side-bearings. Per
//!    §5.7.10: "This second array is optional and generally is used
//!    for a run of monospaced glyphs in the font… all the glyphs in
//!    this array shall have the same advance height as the last
//!    entry in the vMetrics array."
//!
//! For a perfectly monospaced vertical font (most CJK fonts), the
//! producer can ship a single `vMetrics` entry and a tail covering
//! the rest of the glyph count, mirroring the hmtx idiom.
//!
//! ## Field types
//!
//! `advanceHeight` is `uint16` (§5.7.10 "vMetrics array" table). This
//! matches `hmtx.advanceWidth`. `topSideBearing` is `int16` — vertical
//! glyphs can sit either side of the centre baseline so the value is
//! signed.
//!
//! ## Vertical origin (§5.7.10)
//!
//! §5.7.10's "Vertical Origin and Advance Height" paragraph states
//! the Y coordinate of a glyph's vertical origin equals
//! `topSideBearing + glyph_bounding_box.y_max`. For CFF outlines
//! lacking an explicit bbox the spec recommends the optional `VORG`
//! table. The parser here surfaces the raw `vmtx` values; the
//! origin-Y derivation lives at the [`crate::Font`] level so it can
//! consult `glyf`.

use crate::parser::{read_i16, read_u16};
use crate::Error;

/// Parsed `vmtx` table — borrowed-bytes view over the two arrays.
///
/// The reader is index-safe by construction: `numOfLongVerMetrics`
/// and `numGlyphs` are validated against the slice length at parse
/// time, so per-glyph queries cannot run off the end.
#[derive(Debug, Clone)]
pub struct VmtxTable<'a> {
    bytes: &'a [u8],
    num_long_ver_metrics: u16,
    num_glyphs: u16,
}

impl<'a> VmtxTable<'a> {
    /// Parse the `vmtx` slice given the counts published in `vhea`
    /// and `maxp`.
    ///
    /// Validates:
    /// - `num_long_ver_metrics > 0` (§5.7.10: "but that one entry is
    ///   required").
    /// - `num_long_ver_metrics <= num_glyphs`.
    /// - `bytes.len() >=
    ///   num_long_ver_metrics*4 + (num_glyphs - num_long_ver_metrics)*2`.
    pub fn parse(
        bytes: &'a [u8],
        num_long_ver_metrics: u16,
        num_glyphs: u16,
    ) -> Result<Self, Error> {
        if num_long_ver_metrics == 0 {
            return Err(Error::BadStructure("vmtx: numOfLongVerMetrics == 0"));
        }
        if num_long_ver_metrics > num_glyphs {
            return Err(Error::BadStructure("vmtx: numOfLongVerMetrics > numGlyphs"));
        }
        let expected =
            num_long_ver_metrics as usize * 4 + (num_glyphs - num_long_ver_metrics) as usize * 2;
        if bytes.len() < expected {
            return Err(Error::UnexpectedEof);
        }
        Ok(Self {
            bytes,
            num_long_ver_metrics,
            num_glyphs,
        })
    }

    /// Total glyph count this table was parsed against. Useful for
    /// callers that want to range-check against the same upper bound
    /// the parser used.
    pub fn num_glyphs(&self) -> u16 {
        self.num_glyphs
    }

    /// Number of `(advanceHeight, topSideBearing)` pairs in the first
    /// array (from `vhea.numOfLongVerMetrics`).
    pub fn num_long_ver_metrics(&self) -> u16 {
        self.num_long_ver_metrics
    }

    /// Per-glyph advance height in font design units. Returns 0 for
    /// an out-of-range `glyph_id`. Glyphs whose ID falls beyond the
    /// `vMetrics` array share the advance of the last full pair, per
    /// §5.7.10: "all the glyphs in this array shall have the same
    /// advance height as the last entry in the vMetrics array."
    pub fn advance_height(&self, glyph_id: u16) -> u16 {
        if glyph_id >= self.num_glyphs {
            return 0;
        }
        let idx = glyph_id.min(self.num_long_ver_metrics - 1) as usize;
        read_u16(self.bytes, idx * 4).unwrap_or(0)
    }

    /// Per-glyph top side bearing in font design units. Returns 0 for
    /// an out-of-range `glyph_id`. For glyphs in the first array the
    /// value sits at `offset + 2` inside the `(advanceHeight,
    /// topSideBearing)` pair; for tail glyphs it lives in the bare
    /// `topSideBearing[]` array immediately following the pairs.
    pub fn top_side_bearing(&self, glyph_id: u16) -> i16 {
        if glyph_id >= self.num_glyphs {
            return 0;
        }
        if glyph_id < self.num_long_ver_metrics {
            // (advanceHeight, topSideBearing) pair → tsb is at offset + 2.
            read_i16(self.bytes, glyph_id as usize * 4 + 2).unwrap_or(0)
        } else {
            // Bare topSideBearing in tail array.
            let tail_idx = (glyph_id - self.num_long_ver_metrics) as usize;
            let tail_off = self.num_long_ver_metrics as usize * 4 + tail_idx * 2;
            read_i16(self.bytes, tail_off).unwrap_or(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_array_then_tsb_tail() {
        // Three entries total: two long pairs, one tsb-only tail glyph.
        //   gid 0 → (1500, 100)
        //   gid 1 → (1800, -20)
        //   gid 2 → advanceHeight inherits 1800 (last pair), tsb = 77
        let mut b = Vec::new();
        b.extend_from_slice(&1500u16.to_be_bytes());
        b.extend_from_slice(&100i16.to_be_bytes());
        b.extend_from_slice(&1800u16.to_be_bytes());
        b.extend_from_slice(&(-20i16).to_be_bytes());
        b.extend_from_slice(&77i16.to_be_bytes());
        let v = VmtxTable::parse(&b, 2, 3).unwrap();
        assert_eq!(v.num_glyphs(), 3);
        assert_eq!(v.num_long_ver_metrics(), 2);
        assert_eq!(v.advance_height(0), 1500);
        assert_eq!(v.top_side_bearing(0), 100);
        assert_eq!(v.advance_height(1), 1800);
        assert_eq!(v.top_side_bearing(1), -20);
        // Tail glyph inherits last pair's advance.
        assert_eq!(v.advance_height(2), 1800);
        assert_eq!(v.top_side_bearing(2), 77);
    }

    #[test]
    fn single_metric_monospaced_layout() {
        // Monospaced CJK font: one pair covers every glyph.
        //   gid 0 → (2000, -50)
        //   gid 1..4 → advanceHeight inherits 2000, tsbs from tail
        let mut b = Vec::new();
        b.extend_from_slice(&2000u16.to_be_bytes());
        b.extend_from_slice(&(-50i16).to_be_bytes());
        // tail tsbs: 60, 70, 80
        for v in [60i16, 70, 80] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        let v = VmtxTable::parse(&b, 1, 4).unwrap();
        for gid in 0..4u16 {
            assert_eq!(v.advance_height(gid), 2000);
        }
        assert_eq!(v.top_side_bearing(0), -50);
        assert_eq!(v.top_side_bearing(1), 60);
        assert_eq!(v.top_side_bearing(2), 70);
        assert_eq!(v.top_side_bearing(3), 80);
    }

    #[test]
    fn out_of_range_returns_zero() {
        let mut b = Vec::new();
        b.extend_from_slice(&1500u16.to_be_bytes());
        b.extend_from_slice(&100i16.to_be_bytes());
        let v = VmtxTable::parse(&b, 1, 1).unwrap();
        assert_eq!(v.advance_height(99), 0);
        assert_eq!(v.top_side_bearing(99), 0);
    }

    #[test]
    fn rejects_zero_metrics() {
        let b = vec![0u8; 4];
        assert!(VmtxTable::parse(&b, 0, 1).is_err());
    }

    #[test]
    fn rejects_metrics_exceeding_glyphs() {
        let b = vec![0u8; 12];
        assert!(VmtxTable::parse(&b, 3, 2).is_err());
    }

    #[test]
    fn rejects_short_slice() {
        // num_long_ver_metrics=2, num_glyphs=3 → 2*4 + 1*2 = 10 bytes
        // expected, give 9.
        let b = vec![0u8; 9];
        assert!(VmtxTable::parse(&b, 2, 3).is_err());
    }
}
