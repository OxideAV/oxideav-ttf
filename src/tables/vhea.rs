//! `vhea` — vertical header table.
//!
//! Spec: ISO/IEC 14496-22:2019 §5.7.9 ("vhea – Vertical header table").
//! Required by §5.7.10 ("OFFvertical fonts require both a vertical
//! header table ('vhea') and the vertical metrics table") whenever a
//! font supplies vertical-layout metrics.
//!
//! ## Two header versions
//!
//! The table comes in two byte-identical-on-the-wire 36-byte forms
//! that differ only in field naming:
//!
//! * **v1.0** (`version == 0x00010000`) — first three int16 fields are
//!   `ascent`, `descent`, `lineGap` (the §5.7.9 v1.0 table calls
//!   `lineGap` "Reserved; set to 0").
//! * **v1.1** (`version == 0x00011000`) — same three fields are renamed
//!   `vertTypoAscender`, `vertTypoDescender`, `vertTypoLineGap` and
//!   the v1.1 row carries the ideographic-em-box semantics quoted
//!   in §5.7.9. Decoders read identical bytes; the rename signals to
//!   layout engines that the fields carry typographic intent rather
//!   than the older "centre-line to neighbour-line" reading.
//!
//! Both versions are 36 bytes and parse with the same offset table
//! ([`Self::parse`] handles both transparently). The version field is
//! preserved in [`Self::version_raw`] so callers can introspect which
//! form they got.
//!
//! ## Field layout (§5.7.9, big-endian)
//!
//! ```text
//!   0  / 4 / version                    (Fixed; 0x00010000 or 0x00011000)
//!   4  / 2 / ascent / vertTypoAscender
//!   6  / 2 / descent / vertTypoDescender
//!   8  / 2 / lineGap / vertTypoLineGap
//!  10  / 2 / advanceHeightMax           (int16; per §5.7.9 row 4)
//!  12  / 2 / minTopSideBearing          (int16)
//!  14  / 2 / minBottomSideBearing       (int16)
//!  16  / 2 / yMaxExtent                 (int16; max(tsb + (yMax - yMin)))
//!  18  / 2 / caretSlopeRise             (int16; horizontal-caret default 0)
//!  20  / 2 / caretSlopeRun              (int16; horizontal-caret default 1)
//!  22  / 2 / caretOffset                (int16)
//!  24  / 8 / 4 × int16 reserved (set to 0)
//!  32  / 2 / metricDataFormat           (int16; set to 0)
//!  34  / 2 / numOfLongVerMetrics        (uint16)
//! ```
//!
//! Note the spec's `advanceHeightMax` is `int16`, in contrast to
//! `hhea.advanceWidthMax` which is `uint16`. This mirrors §5.7.9's
//! v1.0 / v1.1 tables verbatim — both rows have `int16` in the Type
//! column for that field. The wider extent (`yMaxExtent`) is likewise
//! `int16` because vertical fonts may place glyphs above the centre
//! baseline where the spec's coordinate convention yields a positive
//! number but does not guarantee it.
//!
//! ## MVAR coupling
//!
//! §5.7.9 ("'vhea' Table and OFF Font Variations") declares six MVAR
//! value tags that interpolate vhea fields in a variable font:
//!
//! ```text
//!   ascent         → 'vasc'
//!   descent        → 'vdsc'
//!   lineGap        → 'vlgp'
//!   caretOffset    → 'vcof'
//!   caretSlopeRise → 'vcrs'
//!   caretSlopeRun  → 'vcrn'
//! ```
//!
//! Parsing here exposes the static (default-instance) values; callers
//! reach `Font::metric_variation_delta(tag)` to fold the MVAR
//! contribution into the value before use.

use crate::parser::{read_i16, read_u16, read_u32};
use crate::Error;

/// `vhea` version 1.0 marker (Fixed `0x00010000`).
pub const VHEA_VERSION_1_0: u32 = 0x0001_0000;
/// `vhea` version 1.1 marker (Fixed `0x00011000`, per §5.7.9 v1.1
/// header row "Version number of the vertical header table; 0x00011000
/// for version 1.1").
pub const VHEA_VERSION_1_1: u32 = 0x0001_1000;

/// Parsed `vhea` table.
///
/// All field names use the v1.1 typographic naming (`vert_typo_*`);
/// for v1.0 inputs the fields carry the §5.7.9 v1.0 semantics
/// (`ascent` / `descent` / `lineGap`, where `lineGap` is "Reserved;
/// set to 0"). The [`Self::version_raw`] accessor lets a caller
/// distinguish the two.
#[derive(Debug, Clone, Copy)]
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub struct VheaTable {
    /// Raw version Fixed (`0x00010000` or `0x00011000`).
    version_raw: u32,
    /// Vertical typographic ascender (v1.1 `vertTypoAscender`; v1.0
    /// `ascent`). Distance in font design units from the centre
    /// baseline to the right of the ideographic em-box (v1.1) or the
    /// previous line's descent (v1.0).
    pub vert_typo_ascender: i16,
    /// Vertical typographic descender (v1.1 `vertTypoDescender`; v1.0
    /// `descent`).
    pub vert_typo_descender: i16,
    /// Vertical typographic line gap (v1.1 `vertTypoLineGap`; v1.0
    /// `lineGap`, §5.7.9 v1.0 row "Reserved; set to 0").
    pub vert_typo_line_gap: i16,
    /// Maximum advance height measurement in font design units found
    /// in the font. §5.7.9: "This value must be consistent with the
    /// entries in the vertical metrics table." Signed per the
    /// `int16` row in both v1.0 and v1.1 tables.
    pub advance_height_max: i16,
    /// Minimum top sidebearing measurement found in the font.
    pub min_top_side_bearing: i16,
    /// Minimum bottom sidebearing measurement found in the font.
    pub min_bottom_side_bearing: i16,
    /// `max(tsb + (yMax - yMin))` per §5.7.9 row 8.
    pub y_max_extent: i16,
    /// Caret slope rise (§5.7.9: a vertical font's "horizontal caret"
    /// default is rise = 0, run = 1).
    pub caret_slope_rise: i16,
    /// Caret slope run.
    pub caret_slope_run: i16,
    /// Caret offset.
    pub caret_offset: i16,
    /// Number of advance heights in the vertical metrics table (i.e.
    /// the count of `(advanceHeight, topSideBearing)` pairs in `vmtx`).
    pub num_long_ver_metrics: u16,
}

impl VheaTable {
    /// Parse the 36-byte `vhea` header.
    ///
    /// Accepts both `0x00010000` (v1.0) and `0x00011000` (v1.1) per
    /// §5.7.9. Returns `Error::BadStructure` for any other Fixed
    /// magic and `Error::UnexpectedEof` for a slice shorter than the
    /// 36-byte header.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 36 {
            return Err(Error::UnexpectedEof);
        }
        let version_raw = read_u32(bytes, 0)?;
        match version_raw {
            VHEA_VERSION_1_0 | VHEA_VERSION_1_1 => {}
            _ => return Err(Error::BadStructure("vhea: unrecognised version Fixed")),
        }
        let vert_typo_ascender = read_i16(bytes, 4)?;
        let vert_typo_descender = read_i16(bytes, 6)?;
        let vert_typo_line_gap = read_i16(bytes, 8)?;
        let advance_height_max = read_i16(bytes, 10)?;
        let min_top_side_bearing = read_i16(bytes, 12)?;
        let min_bottom_side_bearing = read_i16(bytes, 14)?;
        let y_max_extent = read_i16(bytes, 16)?;
        let caret_slope_rise = read_i16(bytes, 18)?;
        let caret_slope_run = read_i16(bytes, 20)?;
        let caret_offset = read_i16(bytes, 22)?;
        // Bytes 24..32 are 4 × int16 reserved fields (§5.7.9 v1.0 +
        // v1.1: "Set to 0"). We read past them without validating;
        // tolerating non-zero reserved bytes matches the surrounding
        // table parsers and avoids rejecting fonts whose tooling
        // failed to zero them.
        let num_long_ver_metrics = read_u16(bytes, 34)?;
        if num_long_ver_metrics == 0 {
            return Err(Error::BadStructure("vhea: numOfLongVerMetrics == 0"));
        }
        Ok(Self {
            version_raw,
            vert_typo_ascender,
            vert_typo_descender,
            vert_typo_line_gap,
            advance_height_max,
            min_top_side_bearing,
            min_bottom_side_bearing,
            y_max_extent,
            caret_slope_rise,
            caret_slope_run,
            caret_offset,
            num_long_ver_metrics,
        })
    }

    /// Raw `version` Fixed (`0x00010000` or `0x00011000`). Useful when
    /// a caller wants to expose the version semantically even though
    /// the byte layout is identical.
    pub fn version_raw(&self) -> u32 {
        self.version_raw
    }

    /// `true` when the `version` field is `0x00011000` (v1.1, the
    /// ideographic-em-box typographic naming).
    pub fn is_v1_1(&self) -> bool {
        self.version_raw == VHEA_VERSION_1_1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Construct a minimal 36-byte `vhea` body. Sets sensible vertical
    /// defaults (caret rise 0 / run 1 for a horizontal caret in a
    /// vertical font, per §5.7.9 row 9).
    fn make_v10(num_metrics: u16) -> Vec<u8> {
        let mut b = vec![0u8; 36];
        b[0..4].copy_from_slice(&VHEA_VERSION_1_0.to_be_bytes());
        b[4..6].copy_from_slice(&(1024i16).to_be_bytes()); // ascent
        b[6..8].copy_from_slice(&(-1024i16).to_be_bytes()); // descent
        b[8..10].copy_from_slice(&(0i16).to_be_bytes()); // lineGap (reserved 0)
        b[10..12].copy_from_slice(&(2079i16).to_be_bytes()); // advanceHeightMax
        b[12..14].copy_from_slice(&(-342i16).to_be_bytes()); // minTopSB
        b[14..16].copy_from_slice(&(-333i16).to_be_bytes()); // minBottomSB
        b[16..18].copy_from_slice(&(2036i16).to_be_bytes()); // yMaxExtent
        b[18..20].copy_from_slice(&(0i16).to_be_bytes()); // caretSlopeRise
        b[20..22].copy_from_slice(&(1i16).to_be_bytes()); // caretSlopeRun
        b[22..24].copy_from_slice(&(0i16).to_be_bytes()); // caretOffset
                                                          // 24..32: 4 reserved int16 already zero.
                                                          // 32..34: metricDataFormat already zero.
        b[34..36].copy_from_slice(&num_metrics.to_be_bytes());
        b
    }

    #[test]
    fn parses_v10_example_from_spec() {
        // §5.7.9 "Vertical Header Table Example" row values
        // (numOfLongVerMetrics = 258, advanceHeightMax = 2079, …).
        let b = make_v10(258);
        let v = VheaTable::parse(&b).unwrap();
        assert_eq!(v.version_raw(), VHEA_VERSION_1_0);
        assert!(!v.is_v1_1());
        assert_eq!(v.vert_typo_ascender, 1024);
        assert_eq!(v.vert_typo_descender, -1024);
        assert_eq!(v.vert_typo_line_gap, 0);
        assert_eq!(v.advance_height_max, 2079);
        assert_eq!(v.min_top_side_bearing, -342);
        assert_eq!(v.min_bottom_side_bearing, -333);
        assert_eq!(v.y_max_extent, 2036);
        assert_eq!(v.caret_slope_rise, 0);
        assert_eq!(v.caret_slope_run, 1);
        assert_eq!(v.caret_offset, 0);
        assert_eq!(v.num_long_ver_metrics, 258);
    }

    #[test]
    fn parses_v11_with_renamed_typographic_fields() {
        // v1.1 uses the same byte layout — only the field semantics
        // differ. We re-use the v1.0 builder and flip the version
        // marker.
        let mut b = make_v10(4);
        b[0..4].copy_from_slice(&VHEA_VERSION_1_1.to_be_bytes());
        let v = VheaTable::parse(&b).unwrap();
        assert!(v.is_v1_1());
        assert_eq!(v.version_raw(), VHEA_VERSION_1_1);
        // Field decoding is identical.
        assert_eq!(v.vert_typo_ascender, 1024);
        assert_eq!(v.num_long_ver_metrics, 4);
    }

    #[test]
    fn rejects_unrecognised_version() {
        let mut b = make_v10(1);
        b[0..4].copy_from_slice(&0x0002_0000u32.to_be_bytes());
        assert!(VheaTable::parse(&b).is_err());
    }

    #[test]
    fn rejects_zero_metrics() {
        // §5.7.10: "but that one entry is required" — a vmtx with
        // zero entries is meaningless, so vhea.numOfLongVerMetrics
        // must be at least 1.
        let b = make_v10(0);
        assert!(VheaTable::parse(&b).is_err());
    }

    #[test]
    fn rejects_short_slice() {
        let b = vec![0u8; 35];
        assert!(VheaTable::parse(&b).is_err());
    }

    #[test]
    fn tolerates_non_zero_reserved_bytes() {
        // §5.7.9 says "Set to 0" for the four int16 reserved fields
        // and the metricDataFormat; we still parse if a font tool
        // forgets, matching the surrounding table parsers'
        // permissiveness.
        let mut b = make_v10(1);
        b[24] = 0xAB;
        b[33] = 0xCD;
        let v = VheaTable::parse(&b).unwrap();
        assert_eq!(v.num_long_ver_metrics, 1);
    }
}
