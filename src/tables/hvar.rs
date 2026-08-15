//! `HVAR` — Horizontal metrics variations table.
//!
//! Spec: ISO/IEC 14496-22:2019 §7.3.5 ("HVAR – Horizontal metrics
//! variations table"). The shared `ItemVariationStore` substructure
//! (§7.2.3) is parsed by [`crate::tables::mvar::ItemVariationStore`]
//! and re-used here unchanged.
//!
//! HVAR carries per-glyph adjustments for the values found in `hmtx`
//! (advance width and left side bearing) plus the right side bearing
//! derived from glyph bounding box. The table has three optional
//! `DeltaSetIndexMap` sub-tables — one each for advance width, LSB,
//! and RSB. Whichever maps are present provide a per-glyph
//! `(outer_index, inner_index)` pair into the embedded IVS; whichever
//! are absent fall back to "glyph ID is the inner index, outer index
//! is zero" per §7.3.5.3.
//!
//! ## Header layout (§7.3.5.2)
//!
//! ```text
//!   0  / 2 / majorVersion                 (== 1)
//!   2  / 2 / minorVersion                 (== 0)
//!   4  / 4 / itemVariationStoreOffset     (relative to HVAR start)
//!   8  / 4 / advanceWidthMappingOffset    (relative to HVAR start, or 0)
//!  12  / 4 / lsbMappingOffset             (relative to HVAR start, or 0)
//!  16  / 4 / rsbMappingOffset             (relative to HVAR start, or 0)
//! ```
//!
//! ## DeltaSetIndexMap layout (staged OFF common-formats chapter)
//!
//! Two formats are defined — format 0 with a 16-bit `mapCount` and
//! format 1 with a 32-bit `mapCount`:
//!
//! ```text
//!   0 / 1 / format             (0 or 1)
//!   1 / 1 / entryFormat        (packed: 4 bits inner-bit-count-minus-1,
//!                                       2 bits entry-size-in-bytes-minus-1,
//!                                       2 bits reserved)
//!   2 / 2 / mapCount           (format 0)   — or —
//!   2 / 4 / mapCount           (format 1)
//!   . / N / mapData[mapCount * entrySize]
//! ```
//!
//! The chapter's compatibility note records that earlier revisions
//! (including the ISO/IEC 14496-22:2019 §7.3.5.2 layout) defined a
//! single 16-bit `entryFormat` field whose reserved high-order byte
//! was zero — byte-identical to format 0 above, so both decode
//! through one parser.
//!
//! Each packed entry, decoded as a big-endian integer of `entrySize`
//! bytes (1..=4), splits into `(outerIndex, innerIndex)`:
//!
//! ```text
//!   innerBits  = (entryFormat & 0x0F) + 1
//!   entrySize  = ((entryFormat & 0x30) >> 4) + 1
//!   outerIndex = entry >> innerBits
//!   innerIndex = entry & ((1 << innerBits) - 1)
//! ```
//!
//! If a glyph ID is greater than `mapCount - 1`, the spec mandates
//! using the last entry (§7.3.5.2 sentence under "DeltaSetIndexMap
//! table").
//!
//! ## Implicit (no-map) case (§7.3.5.3)
//!
//! When `advanceWidthMappingOffset == 0`, glyph IDs implicitly
//! provide indices: outer = 0, inner = gid. The spec only allows the
//! implicit form for advance widths; LSB / RSB lookups always require
//! a mapping table (otherwise the table cannot disambiguate which
//! glyphs carry side-bearing variations).

use crate::parser::{read_u16, read_u32, read_u8};
use crate::tables::mvar::ItemVariationStore;
use crate::Error;

/// Reserved bits in the (8-bit) `entryFormat` field. Per the staged
/// common-formats chapter bits 0xC0 are reserved and must be zero; a
/// non-zero value flags a malformed or future-revision map.
const ENTRY_FORMAT_RESERVED_MASK: u8 = 0xC0;
/// `INNER_INDEX_BIT_COUNT_MASK` — count of inner-index bits minus 1.
const ENTRY_FORMAT_INNER_BITS_MASK: u8 = 0x0F;
/// `MAP_ENTRY_SIZE_MASK` — entry size in bytes minus 1.
const ENTRY_FORMAT_SIZE_MASK: u8 = 0x30;

/// Parsed HVAR table.
#[derive(Debug, Clone)]
pub struct HvarTable {
    /// Item variation store referenced by all three optional mappings.
    ivs: ItemVariationStore,
    /// Optional advance-width mapping. When absent, the spec mandates
    /// the implicit (outer=0, inner=gid) form.
    advance_map: Option<DeltaSetIndexMap>,
    /// Optional left-side-bearing mapping.
    lsb_map: Option<DeltaSetIndexMap>,
    /// Optional right-side-bearing mapping.
    rsb_map: Option<DeltaSetIndexMap>,
}

/// One `DeltaSetIndexMap` substructure. Stores the decoded packed
/// entries as `(outer, inner)` pairs of `u16` for efficient lookup.
#[derive(Debug, Clone)]
pub struct DeltaSetIndexMap {
    /// The wire format (0 = 16-bit `mapCount`, 1 = 32-bit `mapCount`).
    format: u8,
    entries: Vec<(u16, u16)>,
}

impl HvarTable {
    /// Parse the HVAR table from `bytes` (the slice starts at the
    /// HVAR table header, end at the table's payload end).
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 20 {
            return Err(Error::UnexpectedEof);
        }
        let major = read_u16(bytes, 0)?;
        let _minor = read_u16(bytes, 2)?;
        let ivs_off = read_u32(bytes, 4)? as usize;
        let awm_off = read_u32(bytes, 8)? as usize;
        let lsb_off = read_u32(bytes, 12)? as usize;
        let rsb_off = read_u32(bytes, 16)? as usize;

        if major != 1 {
            return Err(Error::BadStructure("HVAR majorVersion != 1"));
        }
        if ivs_off == 0 || ivs_off > bytes.len() {
            return Err(Error::BadOffset);
        }
        let ivs = ItemVariationStore::parse(&bytes[ivs_off..])?;

        let advance_map = if awm_off != 0 {
            if awm_off > bytes.len() {
                return Err(Error::BadOffset);
            }
            Some(DeltaSetIndexMap::parse(&bytes[awm_off..])?)
        } else {
            None
        };
        let lsb_map = if lsb_off != 0 {
            if lsb_off > bytes.len() {
                return Err(Error::BadOffset);
            }
            Some(DeltaSetIndexMap::parse(&bytes[lsb_off..])?)
        } else {
            None
        };
        let rsb_map = if rsb_off != 0 {
            if rsb_off > bytes.len() {
                return Err(Error::BadOffset);
            }
            Some(DeltaSetIndexMap::parse(&bytes[rsb_off..])?)
        } else {
            None
        };

        Ok(Self {
            ivs,
            advance_map,
            lsb_map,
            rsb_map,
        })
    }

    /// Borrow the embedded `ItemVariationStore`.
    pub fn item_variation_store(&self) -> &ItemVariationStore {
        &self.ivs
    }

    /// `true` if an advance-width delta-set index map is present.
    pub fn has_advance_width_map(&self) -> bool {
        self.advance_map.is_some()
    }

    /// `true` if a left-side-bearing delta-set index map is present.
    pub fn has_lsb_map(&self) -> bool {
        self.lsb_map.is_some()
    }

    /// `true` if a right-side-bearing delta-set index map is present.
    pub fn has_rsb_map(&self) -> bool {
        self.rsb_map.is_some()
    }

    /// Interpolated advance-width adjustment for `glyph_id` against
    /// `normalised_coords`.
    ///
    /// Per §7.3.5.3, when no advance-width mapping is published the
    /// glyph ID itself acts as the inner index and the outer index is
    /// zero. With a mapping, the packed entry at index `glyph_id`
    /// (clamped at `mapCount - 1` per §7.3.5.2) is consulted.
    ///
    /// Returns `None` only when the resulting `(outer, inner)` pair
    /// is out of range for the embedded IVS.
    pub fn advance_width_delta(&self, glyph_id: u16, normalised_coords: &[f32]) -> Option<f32> {
        let (outer, inner) = self.resolve_indices(glyph_id, self.advance_map.as_ref());
        self.ivs.delta(outer, inner, normalised_coords)
    }

    /// Interpolated left-side-bearing adjustment for `glyph_id`.
    ///
    /// Per §7.3.5.2 / §7.3.5.3, LSB variations require an explicit
    /// mapping table; with no LSB map present this returns `None`.
    pub fn lsb_delta(&self, glyph_id: u16, normalised_coords: &[f32]) -> Option<f32> {
        let map = self.lsb_map.as_ref()?;
        let (outer, inner) = resolve_with_map(glyph_id, map);
        self.ivs.delta(outer, inner, normalised_coords)
    }

    /// Interpolated right-side-bearing adjustment for `glyph_id`.
    ///
    /// As with LSB, RSB variations require an explicit mapping table
    /// (§7.3.5.2); returns `None` when absent.
    pub fn rsb_delta(&self, glyph_id: u16, normalised_coords: &[f32]) -> Option<f32> {
        let map = self.rsb_map.as_ref()?;
        let (outer, inner) = resolve_with_map(glyph_id, map);
        self.ivs.delta(outer, inner, normalised_coords)
    }

    /// Resolve the `(outer, inner)` IVS index pair for `glyph_id`
    /// given the (possibly-absent) advance-width map. Used by the
    /// advance-width path; LSB / RSB use `resolve_with_map` directly
    /// because their maps are mandatory.
    fn resolve_indices(&self, glyph_id: u16, map: Option<&DeltaSetIndexMap>) -> (u16, u16) {
        match map {
            Some(m) => resolve_with_map(glyph_id, m),
            // §7.3.5.3 implicit form: outer = 0, inner = glyph_id.
            None => (0, glyph_id),
        }
    }
}

fn resolve_with_map(glyph_id: u16, map: &DeltaSetIndexMap) -> (u16, u16) {
    // §7.3.5.2: "If a given glyph ID is greater than mapCount - 1,
    // then the last entry is used." For an empty map (mapCount == 0)
    // we fall back to (0, glyph_id) — same shape as the implicit form.
    if map.entries.is_empty() {
        return (0, glyph_id);
    }
    let idx = (glyph_id as usize).min(map.entries.len() - 1);
    map.entries[idx]
}

impl DeltaSetIndexMap {
    /// Parse a `DeltaSetIndexMap` from `bytes` (offset 0 at the
    /// leading `format` byte). Both defined formats decode: format 0
    /// (16-bit `mapCount` — byte-identical to the pre-subdivision
    /// single-`uint16`-`entryFormat` layout of ISO/IEC 14496-22:2019
    /// §7.3.5.2, whose reserved high byte was zero) and format 1
    /// (32-bit `mapCount`). Validates the `format` byte and the
    /// entryFormat reserved bits, and bounds-checks the trailing
    /// `mapData` array.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 4 {
            return Err(Error::UnexpectedEof);
        }
        let format = read_u8(bytes, 0)?;
        let entry_format = read_u8(bytes, 1)?;
        let (map_count, header_len) = match format {
            0 => (read_u16(bytes, 2)? as usize, 4usize),
            1 => (read_u32(bytes, 2)? as usize, 6usize),
            _ => {
                return Err(Error::BadStructure("DeltaSetIndexMap: unrecognised format"));
            }
        };

        if entry_format & ENTRY_FORMAT_RESERVED_MASK != 0 {
            return Err(Error::BadStructure(
                "DeltaSetIndexMap entryFormat reserved bits set",
            ));
        }
        let inner_bits = u32::from(entry_format & ENTRY_FORMAT_INNER_BITS_MASK) + 1;
        let entry_size = (((entry_format & ENTRY_FORMAT_SIZE_MASK) >> 4) + 1) as usize;
        debug_assert!((1..=4).contains(&entry_size));

        let need = header_len
            .checked_add(map_count.checked_mul(entry_size).ok_or(Error::BadOffset)?)
            .ok_or(Error::BadOffset)?;
        if need > bytes.len() {
            return Err(Error::UnexpectedEof);
        }

        let inner_mask: u32 = (1u32 << inner_bits) - 1;
        let mut entries = Vec::with_capacity(map_count);
        for i in 0..map_count {
            let off = header_len + i * entry_size;
            let mut raw: u32 = 0;
            for b in &bytes[off..off + entry_size] {
                raw = (raw << 8) | *b as u32;
            }
            let outer = (raw >> inner_bits) as u16;
            let inner = (raw & inner_mask) as u16;
            entries.push((outer, inner));
        }
        Ok(Self { format, entries })
    }

    /// The map's wire format: 0 (16-bit `mapCount`) or 1 (32-bit
    /// `mapCount`).
    pub fn format(&self) -> u8 {
        self.format
    }

    /// Number of glyph-ID entries in the map.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` if the map has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Borrow the decoded `(outer, inner)` entries in glyph-ID order.
    pub fn entries(&self) -> &[(u16, u16)] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an HVAR with one IVS row (single i16 delta = -50 against
    /// a single rising-edge region peaking at +1) and no advance-width
    /// map (so the implicit "outer=0, inner=gid" path is exercised).
    fn build_minimal_hvar_no_map() -> Vec<u8> {
        // Layout:
        //   [0..20)   HVAR header
        //   [20..)    IVS (parsed verbatim by mvar::ItemVariationStore)
        // IVS layout (re-using the §7.2.3 shape used in mvar.rs):
        //   [0..2)   format = 1
        //   [2..6)   variationRegionListOffset = 12
        //   [6..8)   itemVariationDataCount = 1
        //   [8..12)  itemVariationDataOffsets[0] = 22 (IVS-relative)
        //   [12..22) region list (1 axis, 1 region of 6 B)
        //   [22..32) IVD subtable (6 B header + 2 B region index + 2 B delta)
        let mut b = vec![0u8; 20 + 32];
        // HVAR header
        b[0..2].copy_from_slice(&1u16.to_be_bytes()); // major
                                                      // minor = 0
        b[4..8].copy_from_slice(&20u32.to_be_bytes()); // ivsOffset
                                                       // awmOff / lsbOff / rsbOff = 0
        let ivs = 20usize;
        b[ivs..ivs + 2].copy_from_slice(&1u16.to_be_bytes());
        b[ivs + 2..ivs + 6].copy_from_slice(&12u32.to_be_bytes());
        b[ivs + 6..ivs + 8].copy_from_slice(&1u16.to_be_bytes());
        b[ivs + 8..ivs + 12].copy_from_slice(&22u32.to_be_bytes());
        // region list
        let rl = ivs + 12;
        b[rl..rl + 2].copy_from_slice(&1u16.to_be_bytes()); // axisCount
        b[rl + 2..rl + 4].copy_from_slice(&1u16.to_be_bytes()); // regionCount
                                                                // axis 0 of region 0: start=0 / peak=+1 / end=+1
        b[rl + 4..rl + 6].copy_from_slice(&0i16.to_be_bytes());
        b[rl + 6..rl + 8].copy_from_slice(&16384i16.to_be_bytes());
        b[rl + 8..rl + 10].copy_from_slice(&16384i16.to_be_bytes());
        // IVD
        let ivd = ivs + 22;
        b[ivd..ivd + 2].copy_from_slice(&1u16.to_be_bytes()); // itemCount
        b[ivd + 2..ivd + 4].copy_from_slice(&1u16.to_be_bytes()); // shortDeltaCount
        b[ivd + 4..ivd + 6].copy_from_slice(&1u16.to_be_bytes()); // regionIndexCount
        b[ivd + 6..ivd + 8].copy_from_slice(&0u16.to_be_bytes()); // regionIndexes[0]
        b[ivd + 8..ivd + 10].copy_from_slice(&(-50i16).to_be_bytes());
        b
    }

    #[test]
    fn parses_header_without_maps() {
        let raw = build_minimal_hvar_no_map();
        let h = HvarTable::parse(&raw).expect("parse");
        assert!(!h.has_advance_width_map());
        assert!(!h.has_lsb_map());
        assert!(!h.has_rsb_map());
    }

    #[test]
    fn implicit_advance_width_uses_gid_as_inner_index() {
        // With no advance-width map, glyph 0 ⇒ outer=0, inner=0 ⇒ the
        // single delta of -50 against the (+1)-peaked region.
        let raw = build_minimal_hvar_no_map();
        let h = HvarTable::parse(&raw).unwrap();
        // At the axis default (normalised 0) the region scalar is 0 ⇒ 0.
        let d = h.advance_width_delta(0, &[0.0]).expect("gid 0 in range");
        assert!(d.abs() < 1e-5, "got {d}");
        // At the peak (normalised +1) the scalar is 1 ⇒ delta = -50.
        let d = h.advance_width_delta(0, &[1.0]).expect("gid 0 in range");
        assert!((d - (-50.0)).abs() < 1e-5, "got {d}");
    }

    #[test]
    fn implicit_advance_width_out_of_range_gid_returns_none() {
        // The IVS only has one inner index (itemCount=1); gid 5 ⇒
        // inner=5 ⇒ out of range ⇒ None.
        let raw = build_minimal_hvar_no_map();
        let h = HvarTable::parse(&raw).unwrap();
        assert!(h.advance_width_delta(5, &[1.0]).is_none());
    }

    #[test]
    fn lsb_rsb_without_maps_returns_none() {
        let raw = build_minimal_hvar_no_map();
        let h = HvarTable::parse(&raw).unwrap();
        assert!(h.lsb_delta(0, &[1.0]).is_none());
        assert!(h.rsb_delta(0, &[1.0]).is_none());
    }

    /// Build an HVAR that places an advance-width map between the
    /// header and the IVS. The map has 3 entries (mapCount=3) packed
    /// in 2-byte entries with 4 inner-index bits (so outer occupies
    /// the high 12 bits, inner the low 4 — entryFormat = 0x0013):
    ///   gid 0 ⇒ (0, 0)
    ///   gid 1 ⇒ (0, 0)
    ///   gid 2 ⇒ (0, 0)
    /// All three resolve to the only delta in the IVS.
    fn build_hvar_with_advance_map() -> Vec<u8> {
        // Header:        20 B
        // AdvanceMap:    4 B header + 3 * 2 B = 10 B  (located at offset 20)
        // IVS:           starts at offset 30, same shape as no-map build
        let mut b = vec![0u8; 20 + 10 + 32];
        // Header
        b[0..2].copy_from_slice(&1u16.to_be_bytes());
        let ivs_off: u32 = 30;
        b[4..8].copy_from_slice(&ivs_off.to_be_bytes());
        let awm_off: u32 = 20;
        b[8..12].copy_from_slice(&awm_off.to_be_bytes());
        // lsb / rsb off = 0
        // AdvanceMap @ 20: entryFormat = 0x0013 (entry_size = 2, innerBits = 4),
        // mapCount = 3, three 2-byte zero entries (outer=0, inner=0).
        b[20..22].copy_from_slice(&0x0013u16.to_be_bytes());
        b[22..24].copy_from_slice(&3u16.to_be_bytes());
        // entries already zero
        // IVS @ 30
        let ivs = 30usize;
        b[ivs..ivs + 2].copy_from_slice(&1u16.to_be_bytes());
        b[ivs + 2..ivs + 6].copy_from_slice(&12u32.to_be_bytes());
        b[ivs + 6..ivs + 8].copy_from_slice(&1u16.to_be_bytes());
        b[ivs + 8..ivs + 12].copy_from_slice(&22u32.to_be_bytes());
        let rl = ivs + 12;
        b[rl..rl + 2].copy_from_slice(&1u16.to_be_bytes());
        b[rl + 2..rl + 4].copy_from_slice(&1u16.to_be_bytes());
        b[rl + 6..rl + 8].copy_from_slice(&16384i16.to_be_bytes());
        b[rl + 8..rl + 10].copy_from_slice(&16384i16.to_be_bytes());
        let ivd = ivs + 22;
        b[ivd..ivd + 2].copy_from_slice(&1u16.to_be_bytes());
        b[ivd + 2..ivd + 4].copy_from_slice(&1u16.to_be_bytes());
        b[ivd + 4..ivd + 6].copy_from_slice(&1u16.to_be_bytes());
        b[ivd + 6..ivd + 8].copy_from_slice(&0u16.to_be_bytes());
        b[ivd + 8..ivd + 10].copy_from_slice(&(-30i16).to_be_bytes());
        b
    }

    #[test]
    fn advance_map_routes_all_glyphs_to_same_delta() {
        let raw = build_hvar_with_advance_map();
        let h = HvarTable::parse(&raw).expect("parse");
        assert!(h.has_advance_width_map());
        // At the peak, all three mapped glyphs produce the same -30.
        for gid in 0..3 {
            let d = h.advance_width_delta(gid, &[1.0]).expect("in range");
            assert!((d - (-30.0)).abs() < 1e-5, "gid {gid} got {d}");
        }
    }

    #[test]
    fn advance_map_oob_glyph_clamps_to_last_entry() {
        let raw = build_hvar_with_advance_map();
        let h = HvarTable::parse(&raw).unwrap();
        // gid 99 > mapCount-1 (=2) ⇒ §7.3.5.2 says use the last entry,
        // which is the same (0,0) ⇒ -30.
        let d = h.advance_width_delta(99, &[1.0]).expect("clamped");
        assert!((d - (-30.0)).abs() < 1e-5);
    }

    #[test]
    fn entry_format_reserved_bits_rejected() {
        // Build a tiny format-0 map and set a reserved entryFormat
        // bit (0xC0 mask).
        let mut data = vec![0u8; 4 + 2];
        data[0] = 0x00; // format 0
        data[1] = 0x40 | 0x13; // reserved bit + 2-byte entries
        data[2..4].copy_from_slice(&1u16.to_be_bytes());
        // Trailing entry data = 2 bytes, fine.
        assert!(matches!(
            DeltaSetIndexMap::parse(&data),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn unrecognised_map_format_rejected() {
        // A future format byte (2) must be rejected, not misread.
        let mut data = vec![0u8; 4 + 2];
        data[0] = 0x02;
        data[1] = 0x13;
        data[2..4].copy_from_slice(&1u16.to_be_bytes());
        assert!(matches!(
            DeltaSetIndexMap::parse(&data),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn format1_map_decodes_with_u32_count() {
        // Format 1: uint8 format, uint8 entryFormat, uint32 mapCount.
        // entryFormat 0x13 → 2-byte entries, 4 inner bits.
        let mut data = Vec::new();
        data.push(0x01);
        data.push(0x13);
        data.extend_from_slice(&3u32.to_be_bytes());
        // Entries: raw >> 4 = outer, raw & 0xF = inner.
        for raw in [0x0012u16, 0x0034, 0x00A7] {
            data.extend_from_slice(&raw.to_be_bytes());
        }
        let m = DeltaSetIndexMap::parse(&data).expect("parse");
        assert_eq!(m.format(), 1);
        assert_eq!(m.len(), 3);
        assert_eq!(m.entries()[0], (0x1, 0x2));
        assert_eq!(m.entries()[1], (0x3, 0x4));
        assert_eq!(m.entries()[2], (0xA, 0x7));
    }

    #[test]
    fn format1_map_truncated_rejected() {
        // Format-1 header claims more entries than the data supplies.
        let mut data = Vec::new();
        data.push(0x01);
        data.push(0x13);
        data.extend_from_slice(&4u32.to_be_bytes());
        data.extend_from_slice(&[0u8; 6]); // only 3 of 4 entries
        assert!(matches!(
            DeltaSetIndexMap::parse(&data),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn format0_map_reports_format_zero() {
        let mut data = vec![0u8; 4 + 2];
        data[1] = 0x13;
        data[2..4].copy_from_slice(&1u16.to_be_bytes());
        let m = DeltaSetIndexMap::parse(&data).expect("parse");
        assert_eq!(m.format(), 0);
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn entry_format_widths() {
        // 1-byte entry, 4-bit inner: entryFormat = 0x0003.
        let mut data = vec![0u8; 4 + 2];
        data[0..2].copy_from_slice(&0x0003u16.to_be_bytes());
        data[2..4].copy_from_slice(&2u16.to_be_bytes());
        // Two 1-byte entries: gid 0 ⇒ raw 0b00010010 ⇒ outer=1, inner=2
        //                     gid 1 ⇒ raw 0b00110100 ⇒ outer=3, inner=4
        data[4] = 0b0001_0010;
        data[5] = 0b0011_0100;
        let m = DeltaSetIndexMap::parse(&data).expect("parse");
        assert_eq!(m.len(), 2);
        assert_eq!(m.entries()[0], (1, 2));
        assert_eq!(m.entries()[1], (3, 4));
    }

    #[test]
    fn entry_format_three_byte_entry_decodes_correctly() {
        // 3-byte entry (entrySize = 3 ⇒ size bits = 0b10), inner bits = 6
        // (innerBitsCount = 5 in low nibble). entryFormat = 0x0025.
        let mut data = vec![0u8; 4 + 3];
        data[0..2].copy_from_slice(&0x0025u16.to_be_bytes());
        data[2..4].copy_from_slice(&1u16.to_be_bytes());
        // One 3-byte entry: 0x12_34_56 ⇒ outer = 0x12_34_56 >> 6,
        // inner = 0x12_34_56 & 0x3F = 0x16.
        data[4] = 0x12;
        data[5] = 0x34;
        data[6] = 0x56;
        let m = DeltaSetIndexMap::parse(&data).expect("parse");
        let raw = 0x123456u32;
        let inner = raw & ((1u32 << 6) - 1);
        let outer = raw >> 6;
        assert_eq!(m.entries()[0], (outer as u16, inner as u16));
    }

    #[test]
    fn rejects_truncated_map() {
        // entryFormat says 2-byte entries, mapCount = 3, but we only
        // supply enough trailing bytes for 1 entry.
        let mut data = vec![0u8; 4 + 2];
        data[0..2].copy_from_slice(&0x0013u16.to_be_bytes());
        data[2..4].copy_from_slice(&3u16.to_be_bytes());
        // Only 2 bytes of map data follow (need 6).
        assert!(matches!(
            DeltaSetIndexMap::parse(&data),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_major_version_other_than_1() {
        let mut raw = build_minimal_hvar_no_map();
        raw[0..2].copy_from_slice(&2u16.to_be_bytes());
        assert!(matches!(
            HvarTable::parse(&raw),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn rejects_null_ivs_offset() {
        let mut raw = build_minimal_hvar_no_map();
        raw[4..8].copy_from_slice(&0u32.to_be_bytes());
        assert!(matches!(HvarTable::parse(&raw), Err(Error::BadOffset)));
    }

    #[test]
    fn rejects_truncated_header() {
        let raw = vec![0u8; 12];
        assert!(matches!(HvarTable::parse(&raw), Err(Error::UnexpectedEof)));
    }
}
