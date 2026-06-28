//! `hhea` — horizontal header (ISO/IEC 14496-22:2019 §5.2.4).
//!
//! Decodes the full table: the typographic ascent / descent / line gap,
//! `advanceWidthMax`, the min left / right side-bearing extremes and
//! `xMaxExtent`, the caret-slope rise / run / offset (the angle at which a
//! text cursor is drawn — non-vertical for italic / oblique faces), the
//! `metricDataFormat`, and `numberOfHMetrics` (the count of long entries in
//! `hmtx`).

use crate::parser::{read_i16, read_u16};
use crate::Error;

#[derive(Debug, Clone, Copy)]
pub struct HheaTable {
    pub ascent: i16,
    pub descent: i16,
    pub line_gap: i16,
    pub advance_width_max: u16,
    pub min_left_side_bearing: i16,
    pub min_right_side_bearing: i16,
    pub x_max_extent: i16,
    /// Caret-slope rise (1 + the vertical run for an upright caret). With
    /// `caret_slope_run`, defines the slope of the text cursor.
    pub caret_slope_rise: i16,
    /// Caret-slope run (0 for an upright caret; non-zero for italic).
    pub caret_slope_run: i16,
    /// Caret offset — the amount, in font units, by which the highlight on
    /// a slanted glyph is shifted (0 for non-slanted fonts).
    pub caret_offset: i16,
    /// `metricDataFormat` (0 = current format).
    pub metric_data_format: i16,
    pub num_long_hor_metrics: u16,
}

impl HheaTable {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        // Spec layout, big-endian:
        //   0  / 4 / version (Fixed; should be 1.0)
        //   4  / 2 / Ascender
        //   6  / 2 / Descender
        //   8  / 2 / LineGap
        //  10  / 2 / advanceWidthMax (UFWord)
        //  12  / 2 / minLeftSideBearing
        //  14  / 2 / minRightSideBearing
        //  16  / 2 / xMaxExtent
        //  18  / 2 / caretSlopeRise
        //  20  / 2 / caretSlopeRun
        //  22  / 2 / caretOffset
        //  24  / 8 / reserved (4 * i16)
        //  32  / 2 / metricDataFormat
        //  34  / 2 / numberOfHMetrics
        if bytes.len() < 36 {
            return Err(Error::UnexpectedEof);
        }
        let num_long_hor_metrics = read_u16(bytes, 34)?;
        if num_long_hor_metrics == 0 {
            return Err(Error::BadStructure("hhea.numberOfHMetrics == 0"));
        }
        Ok(Self {
            ascent: read_i16(bytes, 4)?,
            descent: read_i16(bytes, 6)?,
            line_gap: read_i16(bytes, 8)?,
            advance_width_max: read_u16(bytes, 10)?,
            min_left_side_bearing: read_i16(bytes, 12)?,
            min_right_side_bearing: read_i16(bytes, 14)?,
            x_max_extent: read_i16(bytes, 16)?,
            caret_slope_rise: read_i16(bytes, 18)?,
            caret_slope_run: read_i16(bytes, 20)?,
            caret_offset: read_i16(bytes, 22)?,
            metric_data_format: read_i16(bytes, 32)?,
            num_long_hor_metrics,
        })
    }

    /// `true` when the caret slope describes a vertical (upright) cursor
    /// (`caretSlopeRun == 0`). Italic / oblique faces report a non-zero run.
    pub fn caret_is_vertical(&self) -> bool {
        self.caret_slope_run == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal() {
        let mut b = vec![0u8; 36];
        b[0..4].copy_from_slice(&0x00010000u32.to_be_bytes());
        b[4..6].copy_from_slice(&(1900i16).to_be_bytes());
        b[6..8].copy_from_slice(&(-500i16).to_be_bytes());
        b[8..10].copy_from_slice(&(0i16).to_be_bytes());
        b[10..12].copy_from_slice(&(2048u16).to_be_bytes());
        b[34..36].copy_from_slice(&(1u16).to_be_bytes());
        let h = HheaTable::parse(&b).unwrap();
        assert_eq!(h.ascent, 1900);
        assert_eq!(h.descent, -500);
        assert_eq!(h.advance_width_max, 2048);
        assert_eq!(h.num_long_hor_metrics, 1);
    }

    #[test]
    fn parses_full_fields_and_caret_slope() {
        let mut b = vec![0u8; 36];
        b[0..4].copy_from_slice(&0x00010000u32.to_be_bytes());
        b[4..6].copy_from_slice(&(1900i16).to_be_bytes());
        b[6..8].copy_from_slice(&(-500i16).to_be_bytes());
        b[10..12].copy_from_slice(&(2048u16).to_be_bytes());
        b[12..14].copy_from_slice(&(-64i16).to_be_bytes()); // minLeftSideBearing
        b[14..16].copy_from_slice(&(-32i16).to_be_bytes()); // minRightSideBearing
        b[16..18].copy_from_slice(&(2100i16).to_be_bytes()); // xMaxExtent
        b[18..20].copy_from_slice(&(1i16).to_be_bytes()); // caretSlopeRise
        b[20..22].copy_from_slice(&(0i16).to_be_bytes()); // caretSlopeRun (upright)
        b[22..24].copy_from_slice(&(0i16).to_be_bytes()); // caretOffset
        b[32..34].copy_from_slice(&(0i16).to_be_bytes()); // metricDataFormat
        b[34..36].copy_from_slice(&(3u16).to_be_bytes());
        let h = HheaTable::parse(&b).unwrap();
        assert_eq!(h.min_left_side_bearing, -64);
        assert_eq!(h.min_right_side_bearing, -32);
        assert_eq!(h.x_max_extent, 2100);
        assert_eq!(h.caret_slope_rise, 1);
        assert_eq!(h.caret_slope_run, 0);
        assert!(h.caret_is_vertical());
        assert_eq!(h.metric_data_format, 0);
        assert_eq!(h.num_long_hor_metrics, 3);
    }

    #[test]
    fn italic_caret_slope_is_not_vertical() {
        let mut b = vec![0u8; 36];
        b[0..4].copy_from_slice(&0x00010000u32.to_be_bytes());
        b[18..20].copy_from_slice(&(20i16).to_be_bytes()); // rise
        b[20..22].copy_from_slice(&(7i16).to_be_bytes()); // run -> slanted
        b[34..36].copy_from_slice(&(1u16).to_be_bytes());
        let h = HheaTable::parse(&b).unwrap();
        assert!(!h.caret_is_vertical());
    }

    #[test]
    fn rejects_zero_metrics() {
        let mut b = vec![0u8; 36];
        b[0..4].copy_from_slice(&0x00010000u32.to_be_bytes());
        // num_long_hor_metrics stays 0
        assert!(HheaTable::parse(&b).is_err());
    }
}
