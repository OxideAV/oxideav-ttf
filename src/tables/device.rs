//! Device and VariationIndex tables (OpenType §"Common Table Formats"
//! → "Device and VariationIndex Tables").
//!
//! A `Device` / `VariationIndex` table is a small 6-byte-minimum
//! sub-table referenced by an `Offset16` from GPOS `ValueRecord`s,
//! GPOS `Anchor` format-3 fields, GDEF `CaretValueFormat3`, BASE
//! `BaseCoordFormat3`, the MATH table, and JSTF. All of them share the
//! same wire shape, discriminated by the third `uint16` field
//! (`deltaFormat`):
//!
//! ```text
//!   Device table (deltaFormat 0x0001 / 0x0002 / 0x0003):
//!     0 / 2 / startSize           (uint16, smallest ppem corrected)
//!     2 / 2 / endSize             (uint16, largest ppem corrected)
//!     4 / 2 / deltaFormat         (0x0001 = 2-bit, 0x0002 = 4-bit,
//!                                  0x0003 = 8-bit packed deltas)
//!     6 / .. / deltaValue[]       (uint16 array of packed pixel deltas)
//!
//!   VariationIndex table (deltaFormat 0x8000):
//!     0 / 2 / deltaSetOuterIndex  (uint16 — IVS subtable selector)
//!     2 / 2 / deltaSetInnerIndex  (uint16 — delta-set row selector)
//!     4 / 2 / deltaFormat         (= 0x8000)
//! ```
//!
//! Per the spec, an application "should begin by reading the first
//! three fields and then testing the DeltaFormat field to determine
//! the interpretation of the first two fields and whether there is
//! additional data to read".
//!
//! ## What this crate resolves
//!
//! * **VariationIndex** (`deltaFormat == 0x8000`) — the variable-font
//!   path. The `(outer, inner)` delta-set index is resolved against
//!   the host table's `ItemVariationStore` at the current normalised
//!   instance, yielding a font-unit `f32` delta. This is the missing
//!   piece for variable-font GPOS positioning: a value record / anchor
//!   whose x/y shifts with the `wght` / `wdth` / `opsz` axes.
//! * **Device** (`deltaFormat` 0x0001..=0x0003) — the classic
//!   ppem-indexed pixel-hinting path. We **decode** the packed delta
//!   array (so tooling can introspect it and so the discriminator is
//!   honoured) but the `pixel_delta(ppem)` lookup is only meaningful
//!   for a hinting rasteriser; this crate does not run the TrueType
//!   bytecode interpreter, so the shaping paths treat a classic Device
//!   table as contributing zero font-unit adjustment (pixel snapping
//!   is a render-time, not a layout-time, concern).
//!
//! Spec: Microsoft OpenType §"Common Table Formats" / ISO/IEC
//! 14496-22 §6.2 (OFF).

use crate::parser::read_u16;
use crate::tables::mvar::ItemVariationStore;
use crate::Error;

/// `deltaFormat` discriminator for a VariationIndex table.
pub const DELTA_FORMAT_VARIATION_INDEX: u16 = 0x8000;
/// Smallest classic-Device `deltaFormat`.
pub const DELTA_FORMAT_LOCAL_2_BIT: u16 = 0x0001;
/// Mid classic-Device `deltaFormat`.
pub const DELTA_FORMAT_LOCAL_4_BIT: u16 = 0x0002;
/// Largest classic-Device `deltaFormat`.
pub const DELTA_FORMAT_LOCAL_8_BIT: u16 = 0x0003;

/// Sanity cap on a classic Device table's covered ppem span. Real
/// device tables span a few dozen sizes at most; the cap bounds the
/// decoded `deltaValue` array so a malformed `(startSize, endSize)`
/// pair cannot force a huge allocation.
const MAX_DEVICE_PPEM_SPAN: usize = 4096;

/// A decoded Device or VariationIndex table.
///
/// The `parse` entry point reads the discriminating `deltaFormat`
/// field and produces the right variant. A `VariationIndex` carries
/// the `(outer, inner)` delta-set index pair ready to feed an
/// `ItemVariationStore`; a `Device` carries the unpacked per-ppem
/// pixel deltas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceOrVariationIndex {
    /// A VariationIndex table (`deltaFormat == 0x8000`): a delta-set
    /// index into the host table's `ItemVariationStore`.
    Variation {
        /// `deltaSetOuterIndex` — selects an `ItemVariationData`
        /// subtable within the store.
        outer: u16,
        /// `deltaSetInnerIndex` — selects a delta-set row within that
        /// subtable.
        inner: u16,
    },
    /// A classic Device table (`deltaFormat` 0x0001..=0x0003): the
    /// `startSize`/`endSize`-bounded array of per-ppem pixel deltas.
    Device {
        /// Smallest ppem the table corrects.
        start_size: u16,
        /// Largest ppem the table corrects.
        end_size: u16,
        /// One signed pixel delta per ppem in `start_size..=end_size`.
        deltas: Vec<i8>,
    },
}

impl DeviceOrVariationIndex {
    /// Parse a Device / VariationIndex table from `bytes` (offset 0 at
    /// the first `uint16`).
    ///
    /// Returns `Err` only for a structurally invalid table — a slice
    /// too short to hold the three header words, an unknown
    /// `deltaFormat`, or a Device whose `(startSize, endSize)` /
    /// packed-array length is inconsistent.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 6 {
            return Err(Error::UnexpectedEof);
        }
        let field0 = read_u16(bytes, 0)?;
        let field1 = read_u16(bytes, 2)?;
        let delta_format = read_u16(bytes, 4)?;

        match delta_format {
            DELTA_FORMAT_VARIATION_INDEX => Ok(Self::Variation {
                outer: field0,
                inner: field1,
            }),
            DELTA_FORMAT_LOCAL_2_BIT | DELTA_FORMAT_LOCAL_4_BIT | DELTA_FORMAT_LOCAL_8_BIT => {
                let start_size = field0;
                let end_size = field1;
                if end_size < start_size {
                    return Err(Error::BadStructure("Device endSize < startSize"));
                }
                let count = (end_size - start_size) as usize + 1;
                if count > MAX_DEVICE_PPEM_SPAN {
                    return Err(Error::BadStructure("Device ppem span exceeds cap"));
                }
                let bits_per_value = match delta_format {
                    DELTA_FORMAT_LOCAL_2_BIT => 2u32,
                    DELTA_FORMAT_LOCAL_4_BIT => 4,
                    _ => 8,
                };
                let values_per_word = 16 / bits_per_value as usize;
                // Round `count` up to a whole number of uint16 words.
                let word_count = count.div_ceil(values_per_word);
                let need = 6 + word_count * 2;
                if need > bytes.len() {
                    return Err(Error::UnexpectedEof);
                }
                let deltas = unpack_device_deltas(&bytes[6..], count, bits_per_value);
                Ok(Self::Device {
                    start_size,
                    end_size,
                    deltas,
                })
            }
            _ => Err(Error::BadStructure("Device deltaFormat reserved/unknown")),
        }
    }

    /// `true` if this is a variable-font VariationIndex table.
    pub fn is_variation_index(&self) -> bool {
        matches!(self, Self::Variation { .. })
    }

    /// The classic Device pixel delta for `ppem`, or `None` for a
    /// VariationIndex table or a ppem outside the covered span.
    ///
    /// Only meaningful for a hinting rasteriser; the shaping paths in
    /// this crate do not call it (pixel snapping is render-time).
    pub fn pixel_delta(&self, ppem: u16) -> Option<i8> {
        match self {
            Self::Device {
                start_size,
                end_size,
                deltas,
            } => {
                if ppem < *start_size || ppem > *end_size {
                    return None;
                }
                deltas.get((ppem - start_size) as usize).copied()
            }
            Self::Variation { .. } => None,
        }
    }

    /// Resolve this table to a font-unit `f32` adjustment at the
    /// current variation instance.
    ///
    /// * **VariationIndex** — the `(outer, inner)` delta-set index is
    ///   evaluated against `ivs` at `normalised_coords`. Returns `None`
    ///   when `ivs` is absent or the index pair is out of range.
    /// * **Device** — returns `Some(0.0)`. A classic Device table
    ///   contributes a ppem-dependent *pixel* adjustment that only a
    ///   hinting rasteriser applies; at the font-unit layout layer this
    ///   crate operates in, it adds nothing.
    pub fn font_unit_delta(
        &self,
        ivs: Option<&ItemVariationStore>,
        normalised_coords: &[f32],
    ) -> Option<f32> {
        match self {
            Self::Variation { outer, inner } => ivs?.delta(*outer, *inner, normalised_coords),
            Self::Device { .. } => Some(0.0),
        }
    }
}

/// Resolve a (possibly-NULL) Device/VariationIndex offset into a
/// font-unit delta against `ivs` at `normalised_coords`.
///
/// `table_bytes` is the slice the `offset` is relative to (the host
/// sub-table's base). A zero `offset` (NULL) yields `0.0`. A
/// structurally invalid sub-table also yields `0.0` — a malformed
/// device table must not abort a shaping pass, it just contributes no
/// adjustment. The return is always a concrete `f32` so call sites can
/// add it unconditionally.
pub fn resolve_device_delta(
    table_bytes: &[u8],
    offset: u16,
    ivs: Option<&ItemVariationStore>,
    normalised_coords: &[f32],
) -> f32 {
    if offset == 0 {
        return 0.0;
    }
    let off = offset as usize;
    if off >= table_bytes.len() {
        return 0.0;
    }
    DeviceOrVariationIndex::parse(&table_bytes[off..])
        .ok()
        .and_then(|d| d.font_unit_delta(ivs, normalised_coords))
        .unwrap_or(0.0)
}

/// Unpack `count` signed deltas of `bits_per_value` bits each from the
/// packed `uint16` array `bytes`, MSB-first per the §"Device table"
/// description. Missing trailing words decode as zero deltas.
fn unpack_device_deltas(bytes: &[u8], count: usize, bits_per_value: u32) -> Vec<i8> {
    let mut out = Vec::with_capacity(count);
    let mask: u16 = ((1u32 << bits_per_value) - 1) as u16;
    let sign_bit: u16 = 1 << (bits_per_value - 1);
    let values_per_word = 16 / bits_per_value as usize;
    for i in 0..count {
        let word_idx = i / values_per_word;
        let slot = i % values_per_word;
        let word_off = word_idx * 2;
        let word = read_u16(bytes, word_off).unwrap_or(0);
        // MSB-first: slot 0 occupies the most-significant `bits` of the
        // word.
        let shift = 16 - (slot as u32 + 1) * bits_per_value;
        let raw = (word >> shift) & mask;
        // Sign-extend from `bits_per_value` to i8.
        let val = if raw & sign_bit != 0 {
            (raw | !mask) as i16
        } else {
            raw as i16
        };
        out.push(val as i8);
    }
    out
}

/// Read a ValueRecord device offset at `bytes[off]`, treating an
/// out-of-range read as NULL. Small convenience for value-record
/// device-offset extraction in GPOS.
pub(crate) fn read_device_offset(bytes: &[u8], off: usize) -> u16 {
    read_u16(bytes, off).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_variation_index() {
        // outer=3, inner=7, deltaFormat=0x8000
        let mut b = vec![0u8; 6];
        b[0..2].copy_from_slice(&3u16.to_be_bytes());
        b[2..4].copy_from_slice(&7u16.to_be_bytes());
        b[4..6].copy_from_slice(&0x8000u16.to_be_bytes());
        let d = DeviceOrVariationIndex::parse(&b).expect("parse");
        assert_eq!(d, DeviceOrVariationIndex::Variation { outer: 3, inner: 7 });
        assert!(d.is_variation_index());
        assert_eq!(d.pixel_delta(12), None);
    }

    #[test]
    fn parses_device_4bit_packed() {
        // startSize=12, endSize=15 (4 deltas), deltaFormat=0x0002.
        // From the spec example {1, 2, 3, -1} packs to 0x123F.
        let mut b = vec![0u8; 8];
        b[0..2].copy_from_slice(&12u16.to_be_bytes());
        b[2..4].copy_from_slice(&15u16.to_be_bytes());
        b[4..6].copy_from_slice(&0x0002u16.to_be_bytes());
        b[6..8].copy_from_slice(&0x123Fu16.to_be_bytes());
        let d = DeviceOrVariationIndex::parse(&b).expect("parse");
        assert_eq!(
            d,
            DeviceOrVariationIndex::Device {
                start_size: 12,
                end_size: 15,
                deltas: vec![1, 2, 3, -1],
            }
        );
        assert_eq!(d.pixel_delta(12), Some(1));
        assert_eq!(d.pixel_delta(14), Some(3));
        assert_eq!(d.pixel_delta(15), Some(-1));
        assert_eq!(d.pixel_delta(11), None);
        assert_eq!(d.pixel_delta(16), None);
    }

    #[test]
    fn parses_device_2bit_packed() {
        // 2-bit deltas {1, -1, 0, 1, -2, 0, 1, -1}, MSB-first per slot:
        //   01 11 00 01 10 00 01 11  = 0b0111_0001_1000_0111 = 0x7187
        //   01=1, 11=-1, 00=0, 01=1, 10=-2, 00=0, 01=1, 11=-1
        let bits: u16 = 0x7187;
        let mut b = vec![0u8; 8];
        b[0..2].copy_from_slice(&20u16.to_be_bytes());
        b[2..4].copy_from_slice(&27u16.to_be_bytes());
        b[4..6].copy_from_slice(&0x0001u16.to_be_bytes());
        b[6..8].copy_from_slice(&bits.to_be_bytes());
        let d = DeviceOrVariationIndex::parse(&b).expect("parse");
        match d {
            DeviceOrVariationIndex::Device { deltas, .. } => {
                assert_eq!(deltas, vec![1, -1, 0, 1, -2, 0, 1, -1]);
            }
            _ => panic!("expected Device"),
        }
    }

    #[test]
    fn parses_device_8bit_packed() {
        // 8-bit deltas {5, -7, 100} -> 2 uint16 words (last half empty).
        let mut b = vec![0u8; 10];
        b[0..2].copy_from_slice(&8u16.to_be_bytes());
        b[2..4].copy_from_slice(&10u16.to_be_bytes());
        b[4..6].copy_from_slice(&0x0003u16.to_be_bytes());
        // word0: 0x05F9 (5, -7); word1: 0x6400 (100, 0)
        b[6] = 5u8;
        b[7] = (-7i8) as u8;
        b[8] = 100u8;
        b[9] = 0u8;
        let d = DeviceOrVariationIndex::parse(&b).expect("parse");
        match d {
            DeviceOrVariationIndex::Device { deltas, .. } => {
                assert_eq!(deltas, vec![5, -7, 100]);
            }
            _ => panic!("expected Device"),
        }
    }

    #[test]
    fn rejects_unknown_delta_format() {
        let mut b = vec![0u8; 6];
        b[4..6].copy_from_slice(&0x0004u16.to_be_bytes());
        assert!(DeviceOrVariationIndex::parse(&b).is_err());
    }

    #[test]
    fn rejects_end_before_start() {
        let mut b = vec![0u8; 6];
        b[0..2].copy_from_slice(&15u16.to_be_bytes());
        b[2..4].copy_from_slice(&12u16.to_be_bytes());
        b[4..6].copy_from_slice(&0x0001u16.to_be_bytes());
        assert!(DeviceOrVariationIndex::parse(&b).is_err());
    }

    #[test]
    fn rejects_short_slice() {
        assert!(DeviceOrVariationIndex::parse(&[0, 0, 0x80]).is_err());
    }

    #[test]
    fn resolve_null_offset_is_zero() {
        assert_eq!(resolve_device_delta(&[0u8; 8], 0, None, &[]), 0.0);
    }

    #[test]
    fn resolve_out_of_range_offset_is_zero() {
        assert_eq!(resolve_device_delta(&[0u8; 4], 99, None, &[]), 0.0);
    }

    #[test]
    fn resolve_device_table_contributes_zero_font_units() {
        // A classic Device table at offset 4.
        let mut b = vec![0u8; 12];
        b[4..6].copy_from_slice(&12u16.to_be_bytes());
        b[6..8].copy_from_slice(&15u16.to_be_bytes());
        b[8..10].copy_from_slice(&0x0002u16.to_be_bytes());
        b[10..12].copy_from_slice(&0x123Fu16.to_be_bytes());
        assert_eq!(resolve_device_delta(&b, 4, None, &[]), 0.0);
    }

    #[test]
    fn variation_index_needs_ivs_to_resolve() {
        let mut b = vec![0u8; 12];
        // VariationIndex at offset 4: outer=0, inner=0, fmt=0x8000.
        b[8..10].copy_from_slice(&0x8000u16.to_be_bytes());
        // No IVS -> 0.0.
        assert_eq!(resolve_device_delta(&b, 4, None, &[]), 0.0);
    }

    #[test]
    fn read_device_offset_out_of_range_is_null() {
        assert_eq!(read_device_offset(&[0, 5], 0), 5);
        assert_eq!(read_device_offset(&[0, 5], 10), 0);
    }
}
