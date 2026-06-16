//! `EBSC` — Embedded Bitmap Scaling table (ISO/IEC 14496-22:2019 §5.6.4).
//!
//! `EBSC` lets a font declare a bitmap strike that does not exist as real
//! pixel data, but is instead produced by *scaling* a strike that DOES
//! exist in `EBLC`/`EBDT`. The spec motivates this with small Kanji sizes,
//! where scaling an authored bitmap reads better than scan-converting an
//! outline at the same ppem. It carries no glyph imagery itself; it is a
//! redirection layer on top of the embedded-bitmap pair.
//!
//! The on-wire layout we walk:
//!
//! ```text
//! EbscHeader {
//!     u16 majorVersion;   // = 2
//!     u16 minorVersion;   // = 0
//!     u32 numSizes;
//!     BitmapScale bitmapScales[numSizes];
//! }
//! BitmapScale {
//!     SbitLineMetrics hori;          // 12 bytes
//!     SbitLineMetrics vert;          // 12 bytes
//!     u8  ppemX;                     // target horizontal ppem
//!     u8  ppemY;                     // target vertical ppem
//!     u8  substitutePpemX;           // source (existing) horizontal ppem
//!     u8  substitutePpemY;           // source (existing) vertical ppem
//! }
//! SbitLineMetrics {                  // §5.6.3.2, shared with EBLC
//!     i8 ascender;
//!     i8 descender;
//!     u8 widthMax;
//!     i8 caretSlopeNumerator;
//!     i8 caretSlopeDenominator;
//!     i8 caretOffset;
//!     i8 minOriginSB;
//!     i8 minAdvanceSB;
//!     i8 maxBeforeBL;
//!     i8 minAfterBL;
//!     i8 pad1;
//!     i8 pad2;
//! }
//! ```
//!
//! Per §5.6.4 each `BitmapScale` describes the strike *after* scaling:
//! the `ppemX`/`ppemY` give the synthesised size and the line metrics
//! refer to that scaled, font-wide geometry. `substitutePpemX`/
//! `substitutePpemY` name the real strike (an sbit in `EBLC`/`EBDT`) to
//! scale up or down. The spec notes the x and y scale factors are
//! independent — a square strike may be redirected to a non-square one —
//! and that "Glyph metrics are scaled by the same factor as the pixels
//! per Em (in the appropriate direction), and are rounded to the nearest
//! integer pixel."

use crate::parser::{read_i8, read_u16, read_u32, read_u8};
use crate::Error;

/// Major version of an `EBSC` table per §5.6.4.
pub const EBSC_MAJOR_VERSION: u16 = 2;
/// Minor version of an `EBSC` table per §5.6.4.
pub const EBSC_MINOR_VERSION: u16 = 0;

const SBIT_LINE_METRICS_LEN: usize = 12;
/// 12 (hori) + 12 (vert) + ppemX + ppemY + substitutePpemX + substitutePpemY.
const BITMAP_SCALE_LEN: usize = SBIT_LINE_METRICS_LEN * 2 + 4; // 28

/// Per-strike font-wide line metrics (§5.6.3.2). Twelve signed/unsigned
/// bytes shared between `EBLC`'s `BitmapSize` and `EBSC`'s `BitmapScale`.
/// "The line metrics are not used directly by the rasterizer, but are
/// available to clients who want to parse the table."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SbitLineMetrics {
    pub ascender: i8,
    pub descender: i8,
    pub width_max: u8,
    pub caret_slope_numerator: i8,
    pub caret_slope_denominator: i8,
    pub caret_offset: i8,
    pub min_origin_sb: i8,
    pub min_advance_sb: i8,
    pub max_before_bl: i8,
    pub min_after_bl: i8,
    pub pad1: i8,
    pub pad2: i8,
}

impl SbitLineMetrics {
    fn parse(bytes: &[u8], off: usize) -> Result<Self, Error> {
        Ok(Self {
            ascender: read_i8(bytes, off)?,
            descender: read_i8(bytes, off + 1)?,
            width_max: read_u8(bytes, off + 2)?,
            caret_slope_numerator: read_i8(bytes, off + 3)?,
            caret_slope_denominator: read_i8(bytes, off + 4)?,
            caret_offset: read_i8(bytes, off + 5)?,
            min_origin_sb: read_i8(bytes, off + 6)?,
            min_advance_sb: read_i8(bytes, off + 7)?,
            max_before_bl: read_i8(bytes, off + 8)?,
            min_after_bl: read_i8(bytes, off + 9)?,
            pad1: read_i8(bytes, off + 10)?,
            pad2: read_i8(bytes, off + 11)?,
        })
    }
}

/// One `BitmapScale` record (§5.6.4) — a single synthesised strike defined
/// as a scaled copy of an existing `EBLC`/`EBDT` strike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitmapScale {
    /// Font-wide horizontal line metrics for the scaled strike.
    pub hori: SbitLineMetrics,
    /// Font-wide vertical line metrics for the scaled strike.
    pub vert: SbitLineMetrics,
    /// Target (synthesised) horizontal pixels-per-em.
    pub ppem_x: u8,
    /// Target (synthesised) vertical pixels-per-em.
    pub ppem_y: u8,
    /// Horizontal ppem of the real strike to scale from.
    pub substitute_ppem_x: u8,
    /// Vertical ppem of the real strike to scale from.
    pub substitute_ppem_y: u8,
}

impl BitmapScale {
    fn parse(bytes: &[u8], off: usize) -> Result<Self, Error> {
        let hori = SbitLineMetrics::parse(bytes, off)?;
        let vert = SbitLineMetrics::parse(bytes, off + SBIT_LINE_METRICS_LEN)?;
        let base = off + 2 * SBIT_LINE_METRICS_LEN;
        Ok(Self {
            hori,
            vert,
            ppem_x: read_u8(bytes, base)?,
            ppem_y: read_u8(bytes, base + 1)?,
            substitute_ppem_x: read_u8(bytes, base + 2)?,
            substitute_ppem_y: read_u8(bytes, base + 3)?,
        })
    }
}

/// Parsed `EBSC` table — the version header plus the array of
/// `BitmapScale` redirection records.
#[derive(Debug, Clone)]
pub struct EbscTable {
    minor_version: u16,
    scales: Vec<BitmapScale>,
}

impl EbscTable {
    /// Parse the `EBSC` header + `BitmapScale` array. Rejects a
    /// non-`2.x` major version per §5.6.4 ("Major version of the EBSC
    /// table, = 2"); the minor version is surfaced rather than fixed so a
    /// future `2.x` revision still decodes. `numSizes` is capped to bound
    /// allocation against a truncated / malformed input.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 8 {
            return Err(Error::UnexpectedEof);
        }
        let major = read_u16(bytes, 0)?;
        if major != EBSC_MAJOR_VERSION {
            return Err(Error::BadStructure("EBSC: unknown major version"));
        }
        let minor_version = read_u16(bytes, 2)?;
        let num_sizes = read_u32(bytes, 4)?;
        // Real fonts carry only a handful of scaled strikes; the cap
        // mirrors the EBLC/CBLC walker's defence against bogus counts.
        if num_sizes > 256 {
            return Err(Error::BadStructure("EBSC: implausible numSizes"));
        }
        let needed = 8usize
            .checked_add(num_sizes as usize * BITMAP_SCALE_LEN)
            .ok_or(Error::BadStructure("EBSC: numSizes overflow"))?;
        if bytes.len() < needed {
            return Err(Error::UnexpectedEof);
        }
        let mut scales = Vec::with_capacity(num_sizes as usize);
        for i in 0..num_sizes as usize {
            scales.push(BitmapScale::parse(bytes, 8 + i * BITMAP_SCALE_LEN)?);
        }
        Ok(Self {
            minor_version,
            scales,
        })
    }

    /// Minor version from the header (`0` for the current revision).
    pub fn minor_version(&self) -> u16 {
        self.minor_version
    }

    /// Every `BitmapScale` record in declaration order.
    pub fn scales(&self) -> &[BitmapScale] {
        &self.scales
    }

    /// Number of synthesised (scaled) strikes the table declares.
    pub fn num_scales(&self) -> usize {
        self.scales.len()
    }

    /// All target `(ppemX, ppemY)` sizes this table synthesises, in
    /// declaration order. These are the sizes a client could request and
    /// have satisfied by scaling, *without* a real strike existing at
    /// that ppem.
    pub fn target_ppem_sizes(&self) -> impl Iterator<Item = (u8, u8)> + '_ {
        self.scales.iter().map(|s| (s.ppem_x, s.ppem_y))
    }

    /// The `BitmapScale` whose target `ppemY` is `target_ppem`, if any.
    /// Used to discover whether a requested rasterisation size is served
    /// by a scaled strike, and which real strike (`substitute_ppem_y`) to
    /// pull bitmaps from. When several records share a target ppemY the
    /// first in declaration order wins.
    pub fn scale_for_target_ppem(&self, target_ppem: u8) -> Option<&BitmapScale> {
        self.scales.iter().find(|s| s.ppem_y == target_ppem)
    }
}

/// Scale a metric value (advance, bearing, width, …) by the
/// `target / substitute` ppem ratio, rounding to the nearest integer
/// pixel per §5.6.4 ("Glyph metrics are scaled by the same factor as the
/// pixels per Em … and are rounded to the nearest integer pixel"). The
/// arithmetic is done in `i32` so an `i8` bearing scaled by a large
/// factor cannot overflow mid-computation; callers clamp back into range.
pub(crate) fn scale_metric(value: i32, target_ppem: u8, substitute_ppem: u8) -> i32 {
    if substitute_ppem == 0 {
        return value;
    }
    let num = value * target_ppem as i32;
    let den = substitute_ppem as i32;
    // Round half away from zero (nearest integer) so a 1.5-pixel metric
    // lands on 2 and a -1.5-pixel one on -2, symmetric about the origin.
    if num >= 0 {
        (num + den / 2) / den
    } else {
        -((-num + den / 2) / den)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 12-byte SbitLineMetrics blob from ascender/descender, the
    /// rest zeroed.
    fn line_metrics(asc: i8, desc: i8) -> [u8; 12] {
        let mut b = [0u8; 12];
        b[0] = asc as u8;
        b[1] = desc as u8;
        b
    }

    fn build_ebsc(scales: &[(u8, u8, u8, u8)]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&EBSC_MAJOR_VERSION.to_be_bytes());
        v.extend_from_slice(&EBSC_MINOR_VERSION.to_be_bytes());
        v.extend_from_slice(&(scales.len() as u32).to_be_bytes());
        for &(ppx, ppy, spx, spy) in scales {
            v.extend_from_slice(&line_metrics(ppy as i8, -(ppy as i8) / 4));
            v.extend_from_slice(&line_metrics(ppx as i8, -(ppx as i8) / 4));
            v.push(ppx);
            v.push(ppy);
            v.push(spx);
            v.push(spy);
        }
        v
    }

    #[test]
    fn parses_header_and_records() {
        let bytes = build_ebsc(&[(20, 20, 16, 16), (24, 24, 16, 16)]);
        let t = EbscTable::parse(&bytes).unwrap();
        assert_eq!(t.minor_version(), 0);
        assert_eq!(t.num_scales(), 2);
        let sizes: Vec<_> = t.target_ppem_sizes().collect();
        assert_eq!(sizes, vec![(20, 20), (24, 24)]);
        let s0 = &t.scales()[0];
        assert_eq!(s0.ppem_x, 20);
        assert_eq!(s0.substitute_ppem_x, 16);
        assert_eq!(s0.hori.ascender, 20);
    }

    #[test]
    fn line_metrics_round_trip() {
        let bytes = build_ebsc(&[(20, 20, 16, 16)]);
        let t = EbscTable::parse(&bytes).unwrap();
        let s = &t.scales()[0];
        // hori built from ppemY=20, vert from ppemX=20.
        assert_eq!(s.hori.ascender, 20);
        assert_eq!(s.hori.descender, -5);
        assert_eq!(s.vert.ascender, 20);
    }

    #[test]
    fn lookup_by_target_ppem() {
        let bytes = build_ebsc(&[(20, 20, 16, 16), (24, 24, 16, 16)]);
        let t = EbscTable::parse(&bytes).unwrap();
        let s = t.scale_for_target_ppem(24).unwrap();
        assert_eq!(s.substitute_ppem_y, 16);
        assert!(t.scale_for_target_ppem(99).is_none());
    }

    #[test]
    fn rejects_wrong_major_version() {
        let mut bytes = build_ebsc(&[(20, 20, 16, 16)]);
        bytes[1] = 3; // major = 3
        assert!(matches!(
            EbscTable::parse(&bytes),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn rejects_truncated_record_array() {
        let mut bytes = build_ebsc(&[(20, 20, 16, 16)]);
        bytes.truncate(bytes.len() - 4); // chop a record's tail
        assert!(matches!(
            EbscTable::parse(&bytes),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_implausible_num_sizes() {
        let mut bytes = vec![0, 2, 0, 0]; // major=2, minor=0
        bytes.extend_from_slice(&1000u32.to_be_bytes());
        assert!(matches!(
            EbscTable::parse(&bytes),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn zero_scales_is_valid() {
        let bytes = build_ebsc(&[]);
        let t = EbscTable::parse(&bytes).unwrap();
        assert_eq!(t.num_scales(), 0);
        assert!(t.scale_for_target_ppem(20).is_none());
    }

    #[test]
    fn scale_metric_nearest_integer() {
        // 16-px advance scaled 20/16 = 20 exactly.
        assert_eq!(scale_metric(16, 20, 16), 20);
        // 10 * 24 / 16 = 15 exactly.
        assert_eq!(scale_metric(10, 24, 16), 15);
        // 7 * 20 / 16 = 8.75 -> 9 (round half away handled by +den/2).
        assert_eq!(scale_metric(7, 20, 16), 9);
        // Down-scale: 20 * 16 / 20 = 16.
        assert_eq!(scale_metric(20, 16, 20), 16);
        // Negative bearing scales symmetrically: -7 * 20 / 16 -> -9.
        assert_eq!(scale_metric(-7, 20, 16), -9);
        // Substitute ppem 0 is a guard, returns value unchanged.
        assert_eq!(scale_metric(5, 20, 0), 5);
    }
}
