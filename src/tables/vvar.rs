//! `VVAR` — Vertical metrics variations table.
//!
//! Spec: ISO/IEC 14496-22:2019 §7.3.8 ("VVAR – Vertical metrics
//! variations table"). The shared `ItemVariationStore` substructure
//! (§7.2.3) and the `DeltaSetIndexMap` substructure (§7.3.5.2 — VVAR
//! reuses HVAR's mapping format verbatim per §7.3.8.2 "See the
//! horizontal metrics variations ('HVAR') table description for
//! remaining details") are parsed by
//! [`crate::tables::mvar::ItemVariationStore`] and
//! [`crate::tables::hvar::DeltaSetIndexMap`] respectively.
//!
//! VVAR carries per-glyph adjustments for the values found in `vmtx`
//! (advance height and top side bearing) plus the bottom side bearing
//! derived from the glyph bounding box, and — in CFF2 variable fonts
//! that publish a `VORG` table — the Y coordinate of each glyph's
//! vertical origin.
//!
//! The table has four optional `DeltaSetIndexMap` sub-tables — one
//! each for advance height, TSB, BSB, and vertical-origin Y. Whichever
//! maps are present provide a per-glyph `(outer_index, inner_index)`
//! pair into the embedded IVS; whichever are absent fall back, for the
//! advance-height path only, to "glyph ID is the inner index, outer
//! index is zero" per the §7.3.8.2 cross-reference back to §7.3.5.3.
//!
//! ## Header layout (§7.3.8.2)
//!
//! ```text
//!   0  / 2 / majorVersion                   (== 1)
//!   2  / 2 / minorVersion                   (== 0)
//!   4  / 4 / itemVariationStoreOffset       (relative to VVAR start)
//!   8  / 4 / advanceHeightMappingOffset     (relative, 0 = absent)
//!  12  / 4 / tsbMappingOffset               (relative, 0 = absent)
//!  16  / 4 / bsbMappingOffset               (relative, 0 = absent)
//!  20  / 4 / vOrgMappingOffset              (relative, 0 = absent)
//! ```
//!
//! ## Implicit (no-map) case (§7.3.5.3 via §7.3.8.2)
//!
//! When `advanceHeightMappingOffset == 0`, glyph IDs implicitly
//! provide indices: outer = 0, inner = gid. The spec only allows the
//! implicit form for advance heights (analogous to HVAR's advance
//! widths); TSB / BSB / vOrg lookups always require a mapping table
//! (otherwise the table cannot disambiguate which glyphs carry side-
//! bearing or vertical-origin variations).

use crate::parser::{read_u16, read_u32};
use crate::tables::hvar::DeltaSetIndexMap;
use crate::tables::mvar::ItemVariationStore;
use crate::Error;

/// Parsed VVAR table.
#[derive(Debug, Clone)]
pub struct VvarTable {
    /// Item variation store referenced by all four optional mappings.
    ivs: ItemVariationStore,
    /// Optional advance-height mapping. When absent, the spec mandates
    /// the implicit (outer=0, inner=gid) form per §7.3.5.3.
    advance_map: Option<DeltaSetIndexMap>,
    /// Optional top-side-bearing mapping.
    tsb_map: Option<DeltaSetIndexMap>,
    /// Optional bottom-side-bearing mapping.
    bsb_map: Option<DeltaSetIndexMap>,
    /// Optional vertical-origin-Y mapping. Only meaningful for CFF2
    /// variable fonts that publish a `VORG` table (§7.3.8.2 final
    /// paragraph: "Mappings and variation data for vertical origins
    /// are not used in fonts with TrueType outlines"). The parser
    /// still decodes the field when present so a future CFF2 path can
    /// consume it.
    vorg_map: Option<DeltaSetIndexMap>,
}

impl VvarTable {
    /// Parse the VVAR table from `bytes` (the slice starts at the
    /// VVAR table header, ends at the table's payload end).
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        // Header is 24 bytes: 2 (major) + 2 (minor) + 5 * 4 (offsets).
        if bytes.len() < 24 {
            return Err(Error::UnexpectedEof);
        }
        let major = read_u16(bytes, 0)?;
        let _minor = read_u16(bytes, 2)?;
        let ivs_off = read_u32(bytes, 4)? as usize;
        let ahm_off = read_u32(bytes, 8)? as usize;
        let tsb_off = read_u32(bytes, 12)? as usize;
        let bsb_off = read_u32(bytes, 16)? as usize;
        let vorg_off = read_u32(bytes, 20)? as usize;

        if major != 1 {
            return Err(Error::BadStructure("VVAR majorVersion != 1"));
        }
        if ivs_off == 0 || ivs_off > bytes.len() {
            return Err(Error::BadOffset);
        }
        let ivs = ItemVariationStore::parse(&bytes[ivs_off..])?;

        let advance_map = parse_optional_map(bytes, ahm_off)?;
        let tsb_map = parse_optional_map(bytes, tsb_off)?;
        let bsb_map = parse_optional_map(bytes, bsb_off)?;
        let vorg_map = parse_optional_map(bytes, vorg_off)?;

        Ok(Self {
            ivs,
            advance_map,
            tsb_map,
            bsb_map,
            vorg_map,
        })
    }

    /// Borrow the embedded `ItemVariationStore`.
    pub fn item_variation_store(&self) -> &ItemVariationStore {
        &self.ivs
    }

    /// `true` if an advance-height delta-set index map is present.
    pub fn has_advance_height_map(&self) -> bool {
        self.advance_map.is_some()
    }

    /// `true` if a top-side-bearing delta-set index map is present.
    pub fn has_tsb_map(&self) -> bool {
        self.tsb_map.is_some()
    }

    /// `true` if a bottom-side-bearing delta-set index map is present.
    pub fn has_bsb_map(&self) -> bool {
        self.bsb_map.is_some()
    }

    /// `true` if a vertical-origin-Y delta-set index map is present.
    pub fn has_vorg_map(&self) -> bool {
        self.vorg_map.is_some()
    }

    /// Interpolated advance-height adjustment for `glyph_id` against
    /// `normalised_coords`.
    ///
    /// Per §7.3.8.2 (cross-reference to §7.3.5.3), when no advance-
    /// height mapping is published the glyph ID itself acts as the
    /// inner index and the outer index is zero. With a mapping, the
    /// packed entry at index `glyph_id` (clamped at `mapCount - 1` per
    /// §7.3.5.2) is consulted.
    ///
    /// Returns `None` only when the resulting `(outer, inner)` pair
    /// is out of range for the embedded IVS.
    pub fn advance_height_delta(&self, glyph_id: u16, normalised_coords: &[f32]) -> Option<f32> {
        let (outer, inner) = resolve_indices(glyph_id, self.advance_map.as_ref());
        self.ivs.delta(outer, inner, normalised_coords)
    }

    /// Interpolated top-side-bearing adjustment for `glyph_id`.
    ///
    /// Per §7.3.8.2 / §7.3.5.3, TSB variations require an explicit
    /// mapping table; with no TSB map present this returns `None`.
    pub fn tsb_delta(&self, glyph_id: u16, normalised_coords: &[f32]) -> Option<f32> {
        let map = self.tsb_map.as_ref()?;
        let (outer, inner) = resolve_with_map(glyph_id, map);
        self.ivs.delta(outer, inner, normalised_coords)
    }

    /// Interpolated bottom-side-bearing adjustment for `glyph_id`.
    ///
    /// As with TSB, BSB variations require an explicit mapping table
    /// (§7.3.8.2); returns `None` when absent.
    pub fn bsb_delta(&self, glyph_id: u16, normalised_coords: &[f32]) -> Option<f32> {
        let map = self.bsb_map.as_ref()?;
        let (outer, inner) = resolve_with_map(glyph_id, map);
        self.ivs.delta(outer, inner, normalised_coords)
    }

    /// Interpolated vertical-origin-Y adjustment for `glyph_id`.
    ///
    /// §7.3.8.2 final paragraph: a mapping table is required for
    /// vertical-origin variation data. Returns `None` when the map is
    /// absent. The result is intended to be added to the corresponding
    /// `VORG` entry by a CFF2 rasterizer; TrueType fonts do not
    /// publish a vOrg map.
    pub fn vorg_delta(&self, glyph_id: u16, normalised_coords: &[f32]) -> Option<f32> {
        let map = self.vorg_map.as_ref()?;
        let (outer, inner) = resolve_with_map(glyph_id, map);
        self.ivs.delta(outer, inner, normalised_coords)
    }
}

/// Decode an optional `DeltaSetIndexMap` whose offset is given relative
/// to the VVAR table start. `offset == 0` means "no map published".
fn parse_optional_map(bytes: &[u8], offset: usize) -> Result<Option<DeltaSetIndexMap>, Error> {
    if offset == 0 {
        return Ok(None);
    }
    if offset > bytes.len() {
        return Err(Error::BadOffset);
    }
    Ok(Some(DeltaSetIndexMap::parse(&bytes[offset..])?))
}

/// Resolve the `(outer, inner)` IVS index pair for `glyph_id` given
/// the (possibly-absent) advance-height map. Used by the advance-
/// height path; TSB / BSB / vOrg use `resolve_with_map` directly
/// because their maps are mandatory.
fn resolve_indices(glyph_id: u16, map: Option<&DeltaSetIndexMap>) -> (u16, u16) {
    match map {
        Some(m) => resolve_with_map(glyph_id, m),
        // §7.3.8.2 → §7.3.5.3 implicit form: outer = 0, inner = gid.
        None => (0, glyph_id),
    }
}

fn resolve_with_map(glyph_id: u16, map: &DeltaSetIndexMap) -> (u16, u16) {
    // §7.3.5.2 (inherited by §7.3.8.2): "If a given glyph ID is greater
    // than mapCount - 1, then the last entry is used." For an empty
    // map (mapCount == 0) fall back to (0, glyph_id) — same shape as
    // the implicit form.
    let entries = map.entries();
    if entries.is_empty() {
        return (0, glyph_id);
    }
    let idx = (glyph_id as usize).min(entries.len() - 1);
    entries[idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal VVAR with one IVS row (single i16 delta = -50
    /// against a single rising-edge region peaking at +1) and no
    /// mapping tables, so the implicit "outer=0, inner=gid" path is
    /// exercised for advance-height lookups.
    fn build_minimal_vvar_no_maps() -> Vec<u8> {
        // Layout:
        //   [0..24)   VVAR header (5 u32 offsets after major/minor)
        //   [24..)    IVS (parsed by mvar::ItemVariationStore)
        // IVS layout (re-using the §7.2.3 shape used in mvar.rs):
        //   [0..2)   format = 1
        //   [2..6)   variationRegionListOffset = 12
        //   [6..8)   itemVariationDataCount = 1
        //   [8..12)  itemVariationDataOffsets[0] = 22 (IVS-relative)
        //   [12..22) region list (1 axis, 1 region of 6 B)
        //   [22..32) IVD subtable (6 B header + 2 B region index + 2 B delta)
        let mut b = vec![0u8; 24 + 32];
        // Header
        b[0..2].copy_from_slice(&1u16.to_be_bytes()); // major
                                                      // minor = 0
        b[4..8].copy_from_slice(&24u32.to_be_bytes()); // ivsOffset
                                                       // ahm / tsb / bsb / vorg = 0
        let ivs = 24usize;
        b[ivs..ivs + 2].copy_from_slice(&1u16.to_be_bytes());
        b[ivs + 2..ivs + 6].copy_from_slice(&12u32.to_be_bytes());
        b[ivs + 6..ivs + 8].copy_from_slice(&1u16.to_be_bytes());
        b[ivs + 8..ivs + 12].copy_from_slice(&22u32.to_be_bytes());
        let rl = ivs + 12;
        b[rl..rl + 2].copy_from_slice(&1u16.to_be_bytes()); // axisCount
        b[rl + 2..rl + 4].copy_from_slice(&1u16.to_be_bytes()); // regionCount
                                                                // axis 0 of region 0: start=0 / peak=+1 / end=+1
        b[rl + 4..rl + 6].copy_from_slice(&0i16.to_be_bytes());
        b[rl + 6..rl + 8].copy_from_slice(&16384i16.to_be_bytes());
        b[rl + 8..rl + 10].copy_from_slice(&16384i16.to_be_bytes());
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
        let raw = build_minimal_vvar_no_maps();
        let v = VvarTable::parse(&raw).expect("parse");
        assert!(!v.has_advance_height_map());
        assert!(!v.has_tsb_map());
        assert!(!v.has_bsb_map());
        assert!(!v.has_vorg_map());
    }

    #[test]
    fn implicit_advance_height_uses_gid_as_inner_index() {
        // With no advance-height map, glyph 0 ⇒ outer=0, inner=0 ⇒ the
        // single delta of -50 against the (+1)-peaked region.
        let raw = build_minimal_vvar_no_maps();
        let v = VvarTable::parse(&raw).unwrap();
        // At the axis default (normalised 0) the region scalar is 0 ⇒ 0.
        let d = v.advance_height_delta(0, &[0.0]).expect("gid 0 in range");
        assert!(d.abs() < 1e-5, "got {d}");
        // At the peak (normalised +1) the scalar is 1 ⇒ delta = -50.
        let d = v.advance_height_delta(0, &[1.0]).expect("gid 0 in range");
        assert!((d - (-50.0)).abs() < 1e-5, "got {d}");
    }

    #[test]
    fn implicit_advance_height_out_of_range_gid_returns_none() {
        // The IVS only has one inner index (itemCount=1); gid 5 ⇒
        // inner=5 ⇒ out of range ⇒ None.
        let raw = build_minimal_vvar_no_maps();
        let v = VvarTable::parse(&raw).unwrap();
        assert!(v.advance_height_delta(5, &[1.0]).is_none());
    }

    #[test]
    fn tsb_bsb_vorg_without_maps_return_none() {
        let raw = build_minimal_vvar_no_maps();
        let v = VvarTable::parse(&raw).unwrap();
        assert!(v.tsb_delta(0, &[1.0]).is_none());
        assert!(v.bsb_delta(0, &[1.0]).is_none());
        assert!(v.vorg_delta(0, &[1.0]).is_none());
    }

    /// Build a VVAR that places an advance-height map between the
    /// header and the IVS. The map has 3 entries (mapCount=3) packed
    /// in 2-byte entries with 4 inner-index bits (entryFormat 0x0013):
    /// all three glyphs route to (outer=0, inner=0) — the single IVS
    /// row carrying delta = -30 against the (+1)-peaked region.
    fn build_vvar_with_advance_map() -> Vec<u8> {
        // Header:         24 B
        // AdvanceMap:     4 B header + 3 * 2 B = 10 B (located at 24)
        // IVS:            starts at offset 34, same shape as no-map build
        let mut b = vec![0u8; 24 + 10 + 32];
        b[0..2].copy_from_slice(&1u16.to_be_bytes());
        let ivs_off: u32 = 34;
        b[4..8].copy_from_slice(&ivs_off.to_be_bytes());
        let ahm_off: u32 = 24;
        b[8..12].copy_from_slice(&ahm_off.to_be_bytes());
        // tsb / bsb / vorg = 0
        // AdvanceMap @ 24
        b[24..26].copy_from_slice(&0x0013u16.to_be_bytes());
        b[26..28].copy_from_slice(&3u16.to_be_bytes());
        // entries already zero (outer=0, inner=0 in 2-byte raw form)
        // IVS @ 34
        let ivs = 34usize;
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
        let raw = build_vvar_with_advance_map();
        let v = VvarTable::parse(&raw).expect("parse");
        assert!(v.has_advance_height_map());
        for gid in 0..3 {
            let d = v.advance_height_delta(gid, &[1.0]).expect("in range");
            assert!((d - (-30.0)).abs() < 1e-5, "gid {gid} got {d}");
        }
    }

    #[test]
    fn advance_map_oob_glyph_clamps_to_last_entry() {
        let raw = build_vvar_with_advance_map();
        let v = VvarTable::parse(&raw).unwrap();
        // gid 99 > mapCount-1 (=2) ⇒ §7.3.5.2 says use the last entry,
        // which is (0,0) ⇒ -30.
        let d = v.advance_height_delta(99, &[1.0]).expect("clamped");
        assert!((d - (-30.0)).abs() < 1e-5);
    }

    /// Build a VVAR that publishes the optional vertical-origin-Y
    /// mapping. CFF2 fonts use this to vary the per-glyph vOrg-Y in
    /// the `VORG` table; we just verify the offset is decoded and the
    /// map round-trips through the IVS.
    fn build_vvar_with_vorg_map() -> Vec<u8> {
        // Header:    24 B
        // vOrgMap:   4 B header + 2 * 2 B = 8 B (located at 24)
        // IVS:       starts at offset 32 (same shape as no-map build)
        let mut b = vec![0u8; 24 + 8 + 32];
        b[0..2].copy_from_slice(&1u16.to_be_bytes());
        let ivs_off: u32 = 32;
        b[4..8].copy_from_slice(&ivs_off.to_be_bytes());
        // ahm / tsb / bsb = 0
        let vorg_off: u32 = 24;
        b[20..24].copy_from_slice(&vorg_off.to_be_bytes());
        // vOrgMap @ 24: entryFormat 0x0013, mapCount 2.
        b[24..26].copy_from_slice(&0x0013u16.to_be_bytes());
        b[26..28].copy_from_slice(&2u16.to_be_bytes());
        // Two 2-byte entries: both (outer=0, inner=0).
        // IVS @ 32
        let ivs = 32usize;
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
        b[ivd + 8..ivd + 10].copy_from_slice(&17i16.to_be_bytes());
        b
    }

    #[test]
    fn vorg_map_decoded_and_queried() {
        let raw = build_vvar_with_vorg_map();
        let v = VvarTable::parse(&raw).expect("parse");
        assert!(v.has_vorg_map());
        // At the peak both mapped glyphs yield the +17 delta.
        let d = v.vorg_delta(0, &[1.0]).expect("gid 0");
        assert!((d - 17.0).abs() < 1e-5, "gid 0 got {d}");
        let d = v.vorg_delta(1, &[1.0]).expect("gid 1");
        assert!((d - 17.0).abs() < 1e-5);
        // The advance-height map is absent, so the implicit form picks
        // inner=gid 0 ⇒ the same (and only) IVS row.
        let d = v.advance_height_delta(0, &[1.0]).expect("implicit");
        assert!((d - 17.0).abs() < 1e-5);
    }

    #[test]
    fn rejects_major_version_other_than_1() {
        let mut raw = build_minimal_vvar_no_maps();
        raw[0..2].copy_from_slice(&2u16.to_be_bytes());
        assert!(matches!(
            VvarTable::parse(&raw),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn rejects_null_ivs_offset() {
        let mut raw = build_minimal_vvar_no_maps();
        raw[4..8].copy_from_slice(&0u32.to_be_bytes());
        assert!(matches!(VvarTable::parse(&raw), Err(Error::BadOffset)));
    }

    #[test]
    fn rejects_truncated_header() {
        let raw = vec![0u8; 20];
        assert!(matches!(VvarTable::parse(&raw), Err(Error::UnexpectedEof)));
    }

    #[test]
    fn rejects_ivs_offset_past_end_of_table() {
        let mut raw = build_minimal_vvar_no_maps();
        let too_far: u32 = (raw.len() + 1) as u32;
        raw[4..8].copy_from_slice(&too_far.to_be_bytes());
        assert!(matches!(VvarTable::parse(&raw), Err(Error::BadOffset)));
    }

    #[test]
    fn rejects_map_offset_past_end_of_table() {
        let mut raw = build_minimal_vvar_no_maps();
        let too_far: u32 = (raw.len() + 1) as u32;
        // Plant an out-of-range tsb mapping offset.
        raw[12..16].copy_from_slice(&too_far.to_be_bytes());
        assert!(matches!(VvarTable::parse(&raw), Err(Error::BadOffset)));
    }
}
