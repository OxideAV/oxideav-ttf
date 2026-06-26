//! `BASE` — Baseline Table.
//!
//! Spec: ISO/IEC 14496-22:2019 §6.3.1 ("BASE Baseline table").
//!
//! BASE supplies the baseline coordinates and per-script / per-language
//! / per-feature min and max glyph extents used to align glyphs of
//! different scripts and point sizes on a single line of text. The
//! data is structured around two layout-direction Axis tables: the
//! HorizAxis table (Y coordinates, horizontal text) and the VertAxis
//! table (X coordinates, vertical text). Either axis offset may be
//! NULL.
//!
//! Each Axis table references:
//!  - a BaseTagList enumerating the baseline identification tags
//!    (`romn`, `ideo`, `hang`, `icfb`, …) used in the direction, in
//!    alphabetical order, and
//!  - a BaseScriptList enumerating the scripts in the direction, each
//!    pointing at a BaseScript table that carries the BaseValues
//!    (default baseline + per-tag BaseCoord positions) and an optional
//!    MinMax table for default min/max extents plus an array of
//!    BaseLangSysRecord overrides for language-specific extents.
//!
//! The BaseValues table identifies one default baseline (by index into
//! the BaseTagList) and ships one BaseCoord table per baseline tag.
//! Each BaseCoord exists in one of three formats; format 1 stores the
//! design-unit coordinate alone, format 2 augments it with a reference
//! glyph + contour-point pair for hinted adjustment, and format 3
//! adds a Device or VariationIndex offset for size- or
//! variation-instance-dependent adjustment.
//!
//! The MinMax table carries minimum / maximum BaseCoord offsets plus
//! an array of FeatMinMaxRecord entries for feature-specific extents
//! (e.g. superscripts). The same format is reused for each
//! BaseLangSysRecord entry: a 4-byte language-system tag plus an
//! offset to a MinMax table that overrides the script defaults.
//!
//! Version 1.1 of the BASE header (introduced in OFF 2016 amendment)
//! adds a trailing `Offset32 itemVarStoreOffset` pointing at an
//! ItemVariationStore used by `BaseCoordFormat3` VariationIndex
//! references inside variable fonts (§6.3.1.1). The shared
//! [`ItemVariationStore`] decoder lives in `tables::mvar`; this
//! module records the offset and validates that the trailer fits
//! inside the slice, leaving the deferred decode to the variable-font
//! layer that consumes it.
//!
//! ## Header layout
//!
//! ```text
//!   0  / 2 / majorVersion         (== 1)
//!   2  / 2 / minorVersion         (0 or 1)
//!   4  / 2 / horizAxisOffset      (Offset16, may be NULL)
//!   6  / 2 / vertAxisOffset       (Offset16, may be NULL)
//!   8  / 4 / itemVarStoreOffset   (Offset32, v1.1 only, may be NULL)
//! ```
//!
//! ## Axis table (§6.3.1.3 "Axis tables: HorizAxis and VertAxis")
//!
//! ```text
//!   0 / 2 / baseTagListOffset    (Offset16 from start of Axis, may be NULL)
//!   2 / 2 / baseScriptListOffset (Offset16 from start of Axis)
//! ```
//!
//! ## BaseTagList table
//!
//! ```text
//!   0 / 2 / baseTagCount
//!   2 / 4 / baselineTags[baseTagCount]  (each a Tag, alphabetical order)
//! ```
//!
//! ## BaseScriptList table
//!
//! ```text
//!   0 / 2 / baseScriptCount
//!   2 / 6 / baseScriptRecords[baseScriptCount]
//!             { Tag baseScriptTag; Offset16 baseScriptOffset; }
//! ```
//!
//! ## BaseScript table
//!
//! ```text
//!   0 / 2 / baseValuesOffset       (may be NULL)
//!   2 / 2 / defaultMinMaxOffset    (may be NULL)
//!   4 / 2 / baseLangSysCount
//!   6 / 6 / baseLangSysRecords[baseLangSysCount]
//!             { Tag baseLangSysTag; Offset16 minMaxOffset; }
//! ```
//!
//! ## BaseValues table
//!
//! ```text
//!   0 / 2 / defaultBaselineIndex
//!   2 / 2 / baseCoordCount
//!   4 / 2 / baseCoords[baseCoordCount]   (each Offset16 from BaseValues start)
//! ```
//!
//! ## MinMax table
//!
//! ```text
//!   0 / 2 / minCoord            (Offset16, may be NULL)
//!   2 / 2 / maxCoord            (Offset16, may be NULL)
//!   4 / 2 / featMinMaxCount
//!   6 / 8 / featMinMaxRecords[featMinMaxCount]
//!             { Tag featureTableTag; Offset16 minCoord; Offset16 maxCoord; }
//! ```
//!
//! ## BaseCoord (three formats)
//!
//! ```text
//! Format 1 (4 bytes):
//!   0 / 2 / baseCoordFormat   (== 1)
//!   2 / 2 / coordinate        (int16, design units)
//!
//! Format 2 (8 bytes):
//!   0 / 2 / baseCoordFormat   (== 2)
//!   2 / 2 / coordinate        (int16)
//!   4 / 2 / referenceGlyph    (uint16 glyph ID)
//!   6 / 2 / baseCoordPoint    (uint16 contour-point index)
//!
//! Format 3 (6 bytes):
//!   0 / 2 / baseCoordFormat   (== 3)
//!   2 / 2 / coordinate        (int16)
//!   4 / 2 / deviceTable       (Offset16, may be NULL — Device or VariationIndex)
//! ```
//!
//! ## Scope of this module
//!
//! The decode is eager and complete for the BASE structural layout:
//! every BaseScript / BaseValues / MinMax / BaseCoord / FeatMinMax /
//! BaseLangSys structure is parsed into a typed view at
//! [`BaseTable::parse`] time. The deferred surfaces are:
//!
//! - **Device tables** referenced from `BaseCoordFormat3` in
//!   non-variable fonts — surfaced as a relative offset on the
//!   variant so callers can do the §6.2.8 (Device) decode themselves
//!   when sizing for a specific ppem.
//! - **ItemVariationStore** (BASE v1.1) — the offset is validated to
//!   land inside the slice and surfaced on the table; the shared
//!   [`crate::tables::mvar::ItemVariationStore`] decoder consumes it.
//! - **VariationIndex** tables (the BaseCoordFormat3 device-table
//!   payload in a variable font) — the offset is surfaced; the
//!   inner-/outer-index decode is application work because the
//!   referencing convention is identical to the GPOS / GDEF /
//!   MVAR / HVAR / VVAR path that already exists in the crate.

use crate::parser::{read_i16, read_u16, read_u32};
use crate::tables::mvar::ItemVariationStore;
use crate::Error;

/// Documented header version per §6.3.1.3 ("Set to 1").
pub const BASE_MAJOR_VERSION: u16 = 1;
/// `minorVersion` value for the BASE header without the
/// ItemVariationStore offset trailer.
pub const BASE_MINOR_VERSION_0: u16 = 0;
/// `minorVersion` value for the v1.1 header that adds a trailing
/// `Offset32 itemVarStoreOffset`. Introduced for variable-font support
/// per §6.3.1.1.
pub const BASE_MINOR_VERSION_1: u16 = 1;

/// Sanity cap on any count field. The on-disk count fields are all
/// `uint16`, so 65535 is the spec maximum; we accept that exactly and
/// reject obviously corrupt totals during offset bounds checks rather
/// than via a separate guard.
const MAX_COUNT: usize = u16::MAX as usize;

/// One BaseCoord value — single X or Y coordinate in design units, in
/// one of the three formats defined in §6.3.1.3.
///
/// Per the §6.3.1.3 axis convention, a BaseCoord inside the HorizAxis
/// is a Y value and one inside the VertAxis is an X value; this enum
/// stores the magnitude only and the axis disambiguates direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseCoord {
    /// Format 1 — design units only.
    Format1 {
        /// `coordinate` (int16, design units).
        coordinate: i16,
    },
    /// Format 2 — design units plus a reference glyph + contour-point
    /// pair used during hinting to derive the final value at the
    /// rendered size.
    Format2 {
        /// `coordinate` (int16, design units).
        coordinate: i16,
        /// `referenceGlyph` glyph ID.
        reference_glyph: u16,
        /// `baseCoordPoint` index on the reference glyph's outline.
        base_coord_point: u16,
    },
    /// Format 3 — design units plus a Device-table (non-variable
    /// font) or VariationIndex (variable font) offset for size- or
    /// instance-dependent adjustment.
    Format3 {
        /// `coordinate` (int16, design units).
        coordinate: i16,
        /// Device-table / VariationIndex offset, relative to the
        /// containing BaseCoord table. `None` when the on-disk offset
        /// is `0` (the spec NULL marker).
        device_offset: Option<u16>,
        /// Absolute offset of the Device / VariationIndex table within
        /// the parent BASE table (`base_coord_abs + device_offset`),
        /// retained so a VariationIndex-aware accessor can resolve it
        /// against the BASE `ItemVariationStore` without re-walking the
        /// axis tree. `None` mirrors a NULL `device_offset`.
        device_abs_offset: Option<u32>,
    },
}

impl BaseCoord {
    /// The §6.3.1.3 design-unit coordinate, shared across all three
    /// formats. Convenience for callers that don't care about the
    /// hint / variation adjustment.
    pub fn coordinate(&self) -> i16 {
        match self {
            Self::Format1 { coordinate }
            | Self::Format2 { coordinate, .. }
            | Self::Format3 { coordinate, .. } => *coordinate,
        }
    }

    /// Numeric format identifier (1, 2, or 3) for the variant.
    pub fn format(&self) -> u16 {
        match self {
            Self::Format1 { .. } => 1,
            Self::Format2 { .. } => 2,
            Self::Format3 { .. } => 3,
        }
    }

    /// Parse a BaseCoord starting at `bytes`. The slice must extend
    /// from the start of the BaseCoord table to the end of the
    /// containing BASE table, since format-3 device offsets may point
    /// past the BaseCoord but inside the parent BASE bytes.
    ///
    /// `abs_off` is the absolute offset of this BaseCoord table within
    /// the BASE table; it is added to a format-3 `deviceOffset` to
    /// record the device table's absolute position for later
    /// VariationIndex resolution.
    fn parse_at(bytes: &[u8], abs_off: usize) -> Result<Self, Error> {
        if bytes.len() < 4 {
            return Err(Error::UnexpectedEof);
        }
        let format = read_u16(bytes, 0)?;
        let coordinate = read_i16(bytes, 2)?;
        match format {
            1 => Ok(Self::Format1 { coordinate }),
            2 => {
                if bytes.len() < 8 {
                    return Err(Error::UnexpectedEof);
                }
                let reference_glyph = read_u16(bytes, 4)?;
                let base_coord_point = read_u16(bytes, 6)?;
                Ok(Self::Format2 {
                    coordinate,
                    reference_glyph,
                    base_coord_point,
                })
            }
            3 => {
                if bytes.len() < 6 {
                    return Err(Error::UnexpectedEof);
                }
                let raw = read_u16(bytes, 4)?;
                let device_offset = if raw == 0 { None } else { Some(raw) };
                let device_abs_offset = device_offset.map(|o| abs_off as u32 + o as u32);
                Ok(Self::Format3 {
                    coordinate,
                    device_offset,
                    device_abs_offset,
                })
            }
            _ => Err(Error::BadStructure("BASE: unknown BaseCoord format")),
        }
    }

    /// Resolve this BaseCoord to a font-unit coordinate at the
    /// variation instance given by `normalised_coords`, using the
    /// parent BASE table bytes (`base_bytes`) and its
    /// `ItemVariationStore`.
    ///
    /// Format 1 / 2 return their static coordinate. Format 3 folds in
    /// the VariationIndex delta resolved from the BASE IVS (the device
    /// table's absolute position was recorded at parse time); a classic
    /// Device table contributes nothing at the font-unit layer, and a
    /// NULL device offset leaves the coordinate unchanged.
    pub fn resolve(
        &self,
        base_bytes: &[u8],
        ivs: Option<&ItemVariationStore>,
        normalised_coords: &[f32],
    ) -> i16 {
        match self {
            Self::Format1 { coordinate } | Self::Format2 { coordinate, .. } => *coordinate,
            Self::Format3 {
                coordinate,
                device_abs_offset,
                ..
            } => {
                let Some(abs) = device_abs_offset else {
                    return *coordinate;
                };
                let abs = *abs as usize;
                let Some(dev_bytes) = base_bytes.get(abs..) else {
                    return *coordinate;
                };
                // The recorded offset is already absolute, so the
                // device table sits at the start of `dev_bytes`.
                let delta = crate::tables::device::DeviceOrVariationIndex::parse(dev_bytes)
                    .ok()
                    .and_then(|d| d.font_unit_delta(ivs, normalised_coords))
                    .unwrap_or(0.0);
                let rounded = delta.round() as i32;
                (*coordinate as i32 + rounded).clamp(i16::MIN as i32, i16::MAX as i32) as i16
            }
        }
    }
}

/// One feature-specific min/max extent override entry inside a MinMax
/// table (§6.3.1.3 "FeatMinMaxRecord"). Both BaseCoord values are
/// optional per the spec — the underlying offsets are documented as
/// "may be NULL".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatMinMaxRecord {
    /// `featureTableTag` — the 4-byte feature tag (matches the
    /// `FeatureTag` used in the GSUB / GPOS FeatureList).
    pub feature_tag: [u8; 4],
    /// Minimum extent BaseCoord, `None` when the underlying offset is
    /// zero.
    pub min_coord: Option<BaseCoord>,
    /// Maximum extent BaseCoord, `None` when the underlying offset is
    /// zero.
    pub max_coord: Option<BaseCoord>,
}

/// One MinMax table (§6.3.1.3) — the default min/max extents for a
/// script (when referenced from a BaseScript table) or a language
/// system (when referenced from a BaseLangSysRecord), plus an array
/// of feature-specific overrides.
#[derive(Debug, Clone)]
pub struct MinMaxTable {
    /// Script- or language-default minimum extent, `None` when the
    /// on-disk offset is zero.
    pub min_coord: Option<BaseCoord>,
    /// Script- or language-default maximum extent, `None` when the
    /// on-disk offset is zero.
    pub max_coord: Option<BaseCoord>,
    /// `featMinMaxRecords`, in `featureTag` alphabetical order per the
    /// §6.3.1.3 MinMax table layout.
    pub feat_min_max_records: Vec<FeatMinMaxRecord>,
}

/// One BaseLangSysRecord (§6.3.1.3) — a language-system tag plus the
/// MinMax table that overrides the default-script extents when
/// rendering that language.
#[derive(Debug, Clone)]
pub struct BaseLangSysRecord {
    /// `baseLangSysTag` — the 4-byte language-system tag (matches the
    /// `LangSysTag` used in GSUB / GPOS).
    pub lang_sys_tag: [u8; 4],
    /// `minMax` table for the language system.
    pub min_max: MinMaxTable,
}

/// One BaseValues table (§6.3.1.3) — the per-script baseline data.
#[derive(Debug, Clone)]
pub struct BaseValuesTable {
    /// `defaultBaselineIndex` — index into the containing Axis's
    /// `BaseTagList::baseline_tags` array identifying the script's
    /// default baseline.
    pub default_baseline_index: u16,
    /// `baseCoords` — one BaseCoord per baseline tag in the Axis's
    /// BaseTagList, in the same order.
    pub base_coords: Vec<BaseCoord>,
}

/// One BaseScript table (§6.3.1.3) — the layout data for a single
/// script.
#[derive(Debug, Clone)]
pub struct BaseScriptTable {
    /// `BaseValues` table — coordinates for all baselines defined in
    /// the Axis's BaseTagList plus the script's default baseline index.
    /// `None` when the on-disk offset is zero (script has no baseline
    /// data; see §6.3.1.3 NOTE — typically because the corresponding
    /// BaseTagList is also NULL).
    pub base_values: Option<BaseValuesTable>,
    /// Default `MinMax` table — script-wide min/max extents and any
    /// feature overrides that apply to the entire script. `None` when
    /// the on-disk offset is zero.
    pub default_min_max: Option<MinMaxTable>,
    /// `baseLangSysRecords`, in `baseLangSysTag` alphabetical order per
    /// §6.3.1.3.
    pub base_lang_sys_records: Vec<BaseLangSysRecord>,
}

impl BaseScriptTable {
    /// MinMax override for the language system identified by
    /// `lang_sys_tag`, if any. Returns `None` when the script has no
    /// matching `BaseLangSysRecord` — callers should fall back to
    /// `default_min_max` per §6.3.1.3 ("If no extent values are
    /// defined for a language system or for language-specific
    /// features, use the default min/max extent values for the
    /// script").
    pub fn min_max_for_lang_sys(&self, lang_sys_tag: [u8; 4]) -> Option<&MinMaxTable> {
        self.base_lang_sys_records
            .iter()
            .find(|r| r.lang_sys_tag == lang_sys_tag)
            .map(|r| &r.min_max)
    }
}

/// One BaseScriptRecord entry inside the Axis's BaseScriptList — a
/// 4-byte script tag plus the BaseScript table it maps to.
#[derive(Debug, Clone)]
pub struct BaseScriptRecord {
    /// `baseScriptTag` — the 4-byte script tag (matches the
    /// `ScriptTag` used in GSUB / GPOS).
    pub script_tag: [u8; 4],
    /// `BaseScript` table for the script.
    pub base_script: BaseScriptTable,
}

/// One layout-direction Axis table (HorizAxis or VertAxis) — the
/// per-script baseline + min/max data for one text direction.
///
/// Per §6.3.1.3, the HorizAxis carries Y coordinates (horizontal text;
/// baselines are positioned vertically) and the VertAxis carries X
/// coordinates (vertical text; baselines are positioned horizontally).
#[derive(Debug, Clone)]
pub struct AxisTable {
    /// `baselineTags` from the Axis's BaseTagList, in the alphabetical
    /// order mandated by §6.3.1.3 ("must be in alphabetical order").
    /// `None` when the on-disk offset is zero — §6.3.1.3 notes that
    /// "if no baseline data is available for a text direction, the
    /// offset to the corresponding BaseTagList may be set to NULL".
    pub baseline_tags: Option<Vec<[u8; 4]>>,
    /// `baseScriptRecords`, in `baseScriptTag` alphabetical order per
    /// §6.3.1.3.
    pub base_scripts: Vec<BaseScriptRecord>,
}

impl AxisTable {
    /// Look up the BaseScript entry for `script_tag`. Returns `None`
    /// when the script is not listed in the Axis's BaseScriptList —
    /// §6.3.1.3 notes that "If a script is not listed here, then the
    /// text-processing client will render the script using the layout
    /// information specified for the entire font."
    pub fn base_script_for_tag(&self, script_tag: [u8; 4]) -> Option<&BaseScriptTable> {
        self.base_scripts
            .iter()
            .find(|r| r.script_tag == script_tag)
            .map(|r| &r.base_script)
    }

    /// Index of `baseline_tag` inside the Axis's `baseline_tags`
    /// array, if present. Used to walk the parallel
    /// `BaseValues::base_coords` array for a specific baseline.
    pub fn baseline_index_for_tag(&self, baseline_tag: [u8; 4]) -> Option<usize> {
        self.baseline_tags
            .as_ref()?
            .iter()
            .position(|t| t == &baseline_tag)
    }
}

/// Parsed `BASE` table — both Axis tables plus the v1.1
/// ItemVariationStore offset and the §6.3.1.3 ItemVariationStore raw
/// bytes (when present).
#[derive(Debug, Clone)]
pub struct BaseTable {
    /// `majorVersion`. Always 1.
    pub major_version: u16,
    /// `minorVersion`. 0 (no IVS trailer) or 1 (variable-font
    /// `itemVarStoreOffset` follows).
    pub minor_version: u16,
    /// HorizAxis table (Y coordinates, horizontal text). `None` when
    /// the on-disk offset is zero.
    pub horiz_axis: Option<AxisTable>,
    /// VertAxis table (X coordinates, vertical text). `None` when the
    /// on-disk offset is zero.
    pub vert_axis: Option<AxisTable>,
    /// ItemVariationStore offset, relative to the start of the BASE
    /// table. `None` when the table is version 1.0 or the v1.1 offset
    /// is zero.
    pub item_var_store_offset: Option<u32>,
    /// ItemVariationStore raw bytes. The shared
    /// [`crate::tables::mvar::ItemVariationStore`] decoder consumes
    /// these on demand inside the variable-font layer.
    item_var_store_bytes: Option<Vec<u8>>,
    /// The full BASE table bytes, retained so a `BaseCoordFormat3`
    /// VariationIndex device offset (recorded as an absolute position at
    /// parse time) can be resolved against the IVS without re-walking
    /// the axis tree.
    raw_bytes: Vec<u8>,
}

impl BaseTable {
    /// Parse a `BASE` byte slice. Validates the header version,
    /// resolves the HorizAxis / VertAxis offsets, and eagerly decodes
    /// every sub-table. The v1.1 ItemVariationStore is bounds-checked
    /// and the bytes are retained so a downstream variable-font path
    /// can decode them with the shared
    /// [`crate::tables::mvar::ItemVariationStore`].
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 8 {
            return Err(Error::UnexpectedEof);
        }
        let major_version = read_u16(bytes, 0)?;
        let minor_version = read_u16(bytes, 2)?;
        if major_version != BASE_MAJOR_VERSION {
            return Err(Error::BadStructure("BASE: majorVersion != 1"));
        }
        if minor_version != BASE_MINOR_VERSION_0 && minor_version != BASE_MINOR_VERSION_1 {
            return Err(Error::BadStructure("BASE: minorVersion neither 0 nor 1"));
        }
        let horiz_axis_offset = read_u16(bytes, 4)? as usize;
        let vert_axis_offset = read_u16(bytes, 6)? as usize;

        let (item_var_store_offset, item_var_store_bytes) = if minor_version == BASE_MINOR_VERSION_1
        {
            if bytes.len() < 12 {
                return Err(Error::UnexpectedEof);
            }
            let raw = read_u32(bytes, 8)?;
            if raw == 0 {
                (None, None)
            } else {
                let off = raw as usize;
                if off >= bytes.len() {
                    return Err(Error::BadStructure(
                        "BASE: itemVarStoreOffset past end of table",
                    ));
                }
                // Retain a copy so the table can be carried lifetime-
                // unbounded alongside the rest of the structured view.
                let ivs_bytes = bytes[off..].to_vec();
                (Some(raw), Some(ivs_bytes))
            }
        } else {
            (None, None)
        };

        let horiz_axis = if horiz_axis_offset == 0 {
            None
        } else {
            Some(parse_axis(bytes, horiz_axis_offset)?)
        };
        let vert_axis = if vert_axis_offset == 0 {
            None
        } else {
            Some(parse_axis(bytes, vert_axis_offset)?)
        };

        Ok(Self {
            major_version,
            minor_version,
            horiz_axis,
            vert_axis,
            item_var_store_offset,
            item_var_store_bytes,
            raw_bytes: bytes.to_vec(),
        })
    }

    /// Decode the BASE `ItemVariationStore` (v1.1), if present.
    fn item_variation_store(&self) -> Option<ItemVariationStore> {
        ItemVariationStore::parse(self.item_var_store_bytes.as_deref()?).ok()
    }

    /// Resolve a HorizAxis (Y) baseline coordinate for `(script_tag,
    /// baseline_tag)` at the variation instance `normalised_coords`,
    /// folding in a `BaseCoordFormat3` VariationIndex delta from the
    /// BASE `ItemVariationStore`. Returns the static coordinate for
    /// format-1/2 coords or a font without a BASE IVS.
    pub fn horiz_baseline_y_resolved(
        &self,
        script_tag: [u8; 4],
        baseline_tag: [u8; 4],
        normalised_coords: &[f32],
    ) -> Option<i16> {
        let h = self.horiz_axis.as_ref()?;
        let idx = h.baseline_index_for_tag(baseline_tag)?;
        let bs = h.base_script_for_tag(script_tag)?;
        let bv = bs.base_values.as_ref()?;
        let coord = bv.base_coords.get(idx)?;
        let ivs = self.item_variation_store();
        Some(coord.resolve(&self.raw_bytes, ivs.as_ref(), normalised_coords))
    }

    /// VertAxis (X) mirror of [`Self::horiz_baseline_y_resolved`].
    pub fn vert_baseline_x_resolved(
        &self,
        script_tag: [u8; 4],
        baseline_tag: [u8; 4],
        normalised_coords: &[f32],
    ) -> Option<i16> {
        let v = self.vert_axis.as_ref()?;
        let idx = v.baseline_index_for_tag(baseline_tag)?;
        let bs = v.base_script_for_tag(script_tag)?;
        let bv = bs.base_values.as_ref()?;
        let coord = bv.base_coords.get(idx)?;
        let ivs = self.item_variation_store();
        Some(coord.resolve(&self.raw_bytes, ivs.as_ref(), normalised_coords))
    }

    /// Borrow the ItemVariationStore raw bytes when the v1.1 trailer
    /// supplies them. The shared
    /// [`crate::tables::mvar::ItemVariationStore::parse`] decoder
    /// consumes the slice; callers in the variable-font layer should
    /// match `BaseCoordFormat3::device_offset` deltas through the
    /// resulting store.
    pub fn item_var_store_bytes(&self) -> Option<&[u8]> {
        self.item_var_store_bytes.as_deref()
    }
}

fn parse_axis(base_bytes: &[u8], axis_off: usize) -> Result<AxisTable, Error> {
    if axis_off
        .checked_add(4)
        .ok_or(Error::BadStructure("BASE: Axis offset overflow"))?
        > base_bytes.len()
    {
        return Err(Error::UnexpectedEof);
    }
    let axis_bytes = &base_bytes[axis_off..];
    let base_tag_list_off = read_u16(axis_bytes, 0)? as usize;
    let base_script_list_off = read_u16(axis_bytes, 2)? as usize;

    let baseline_tags = if base_tag_list_off == 0 {
        None
    } else {
        let abs = axis_off
            .checked_add(base_tag_list_off)
            .ok_or(Error::BadStructure("BASE: BaseTagList offset overflow"))?;
        Some(parse_base_tag_list(base_bytes, abs)?)
    };

    if base_script_list_off == 0 {
        return Err(Error::BadStructure(
            "BASE: Axis missing baseScriptListOffset",
        ));
    }
    let bsl_abs = axis_off
        .checked_add(base_script_list_off)
        .ok_or(Error::BadStructure("BASE: BaseScriptList offset overflow"))?;
    let base_scripts = parse_base_script_list(base_bytes, bsl_abs)?;
    Ok(AxisTable {
        baseline_tags,
        base_scripts,
    })
}

fn parse_base_tag_list(base_bytes: &[u8], off: usize) -> Result<Vec<[u8; 4]>, Error> {
    if off
        .checked_add(2)
        .ok_or(Error::BadStructure("BASE: BaseTagList header overflow"))?
        > base_bytes.len()
    {
        return Err(Error::UnexpectedEof);
    }
    let count = read_u16(base_bytes, off)? as usize;
    if count > MAX_COUNT {
        return Err(Error::BadStructure("BASE: baseTagCount cap"));
    }
    let body_start = off
        .checked_add(2)
        .ok_or(Error::BadStructure("BASE: BaseTagList body overflow"))?;
    let body_end = body_start
        .checked_add(
            count
                .checked_mul(4)
                .ok_or(Error::BadStructure("BASE: baseTagCount * 4 overflow"))?,
        )
        .ok_or(Error::BadStructure("BASE: BaseTagList body overflow"))?;
    if body_end > base_bytes.len() {
        return Err(Error::UnexpectedEof);
    }
    let mut tags = Vec::with_capacity(count);
    for i in 0..count {
        let p = body_start + i * 4;
        tags.push([
            base_bytes[p],
            base_bytes[p + 1],
            base_bytes[p + 2],
            base_bytes[p + 3],
        ]);
    }
    Ok(tags)
}

fn parse_base_script_list(base_bytes: &[u8], off: usize) -> Result<Vec<BaseScriptRecord>, Error> {
    if off
        .checked_add(2)
        .ok_or(Error::BadStructure("BASE: BaseScriptList header overflow"))?
        > base_bytes.len()
    {
        return Err(Error::UnexpectedEof);
    }
    let count = read_u16(base_bytes, off)? as usize;
    if count > MAX_COUNT {
        return Err(Error::BadStructure("BASE: baseScriptCount cap"));
    }
    let body_start = off
        .checked_add(2)
        .ok_or(Error::BadStructure("BASE: BaseScriptList body overflow"))?;
    let body_end = body_start
        .checked_add(
            count
                .checked_mul(6)
                .ok_or(Error::BadStructure("BASE: baseScriptCount * 6 overflow"))?,
        )
        .ok_or(Error::BadStructure("BASE: BaseScriptList body overflow"))?;
    if body_end > base_bytes.len() {
        return Err(Error::UnexpectedEof);
    }
    let mut records = Vec::with_capacity(count);
    for i in 0..count {
        let p = body_start + i * 6;
        let script_tag = [
            base_bytes[p],
            base_bytes[p + 1],
            base_bytes[p + 2],
            base_bytes[p + 3],
        ];
        let bs_off_rel = read_u16(base_bytes, p + 4)? as usize;
        let bs_abs = off
            .checked_add(bs_off_rel)
            .ok_or(Error::BadStructure("BASE: BaseScript offset overflow"))?;
        let base_script = parse_base_script(base_bytes, bs_abs)?;
        records.push(BaseScriptRecord {
            script_tag,
            base_script,
        });
    }
    Ok(records)
}

fn parse_base_script(base_bytes: &[u8], off: usize) -> Result<BaseScriptTable, Error> {
    if off
        .checked_add(6)
        .ok_or(Error::BadStructure("BASE: BaseScript header overflow"))?
        > base_bytes.len()
    {
        return Err(Error::UnexpectedEof);
    }
    let bs_bytes = &base_bytes[off..];
    let base_values_off = read_u16(bs_bytes, 0)? as usize;
    let default_min_max_off = read_u16(bs_bytes, 2)? as usize;
    let lang_sys_count = read_u16(bs_bytes, 4)? as usize;

    let body_start = off
        .checked_add(6)
        .ok_or(Error::BadStructure("BASE: BaseScript body overflow"))?;
    let body_end = body_start
        .checked_add(
            lang_sys_count
                .checked_mul(6)
                .ok_or(Error::BadStructure("BASE: baseLangSysCount * 6 overflow"))?,
        )
        .ok_or(Error::BadStructure("BASE: BaseScript body overflow"))?;
    if body_end > base_bytes.len() {
        return Err(Error::UnexpectedEof);
    }

    let base_values = if base_values_off == 0 {
        None
    } else {
        let abs = off
            .checked_add(base_values_off)
            .ok_or(Error::BadStructure("BASE: BaseValues offset overflow"))?;
        Some(parse_base_values(base_bytes, abs)?)
    };

    let default_min_max = if default_min_max_off == 0 {
        None
    } else {
        let abs = off
            .checked_add(default_min_max_off)
            .ok_or(Error::BadStructure("BASE: defaultMinMax offset overflow"))?;
        Some(parse_min_max(base_bytes, abs)?)
    };

    let mut base_lang_sys_records = Vec::with_capacity(lang_sys_count);
    for i in 0..lang_sys_count {
        let p = body_start + i * 6;
        let lang_sys_tag = [
            base_bytes[p],
            base_bytes[p + 1],
            base_bytes[p + 2],
            base_bytes[p + 3],
        ];
        let mm_off_rel = read_u16(base_bytes, p + 4)? as usize;
        if mm_off_rel == 0 {
            return Err(Error::BadStructure(
                "BASE: BaseLangSysRecord minMaxOffset must not be NULL",
            ));
        }
        let mm_abs = off.checked_add(mm_off_rel).ok_or(Error::BadStructure(
            "BASE: BaseLangSys MinMax offset overflow",
        ))?;
        let min_max = parse_min_max(base_bytes, mm_abs)?;
        base_lang_sys_records.push(BaseLangSysRecord {
            lang_sys_tag,
            min_max,
        });
    }
    Ok(BaseScriptTable {
        base_values,
        default_min_max,
        base_lang_sys_records,
    })
}

fn parse_base_values(base_bytes: &[u8], off: usize) -> Result<BaseValuesTable, Error> {
    if off
        .checked_add(4)
        .ok_or(Error::BadStructure("BASE: BaseValues header overflow"))?
        > base_bytes.len()
    {
        return Err(Error::UnexpectedEof);
    }
    let bv_bytes = &base_bytes[off..];
    let default_baseline_index = read_u16(bv_bytes, 0)?;
    let count = read_u16(bv_bytes, 2)? as usize;
    if count > MAX_COUNT {
        return Err(Error::BadStructure("BASE: baseCoordCount cap"));
    }
    let body_start = off
        .checked_add(4)
        .ok_or(Error::BadStructure("BASE: BaseValues body overflow"))?;
    let body_end = body_start
        .checked_add(
            count
                .checked_mul(2)
                .ok_or(Error::BadStructure("BASE: baseCoordCount * 2 overflow"))?,
        )
        .ok_or(Error::BadStructure("BASE: BaseValues body overflow"))?;
    if body_end > base_bytes.len() {
        return Err(Error::UnexpectedEof);
    }
    let mut base_coords = Vec::with_capacity(count);
    for i in 0..count {
        let p = body_start + i * 2;
        let bc_off_rel = read_u16(base_bytes, p)? as usize;
        if bc_off_rel == 0 {
            return Err(Error::BadStructure(
                "BASE: BaseValues baseCoord offset must not be NULL",
            ));
        }
        let bc_abs = off
            .checked_add(bc_off_rel)
            .ok_or(Error::BadStructure("BASE: BaseCoord offset overflow"))?;
        if bc_abs >= base_bytes.len() {
            return Err(Error::UnexpectedEof);
        }
        base_coords.push(BaseCoord::parse_at(&base_bytes[bc_abs..], bc_abs)?);
    }
    Ok(BaseValuesTable {
        default_baseline_index,
        base_coords,
    })
}

fn parse_min_max(base_bytes: &[u8], off: usize) -> Result<MinMaxTable, Error> {
    if off
        .checked_add(6)
        .ok_or(Error::BadStructure("BASE: MinMax header overflow"))?
        > base_bytes.len()
    {
        return Err(Error::UnexpectedEof);
    }
    let mm_bytes = &base_bytes[off..];
    let min_off_rel = read_u16(mm_bytes, 0)? as usize;
    let max_off_rel = read_u16(mm_bytes, 2)? as usize;
    let feat_count = read_u16(mm_bytes, 4)? as usize;
    if feat_count > MAX_COUNT {
        return Err(Error::BadStructure("BASE: featMinMaxCount cap"));
    }
    let min_coord = if min_off_rel == 0 {
        None
    } else {
        let abs = off
            .checked_add(min_off_rel)
            .ok_or(Error::BadStructure("BASE: MinMax minCoord offset overflow"))?;
        if abs >= base_bytes.len() {
            return Err(Error::UnexpectedEof);
        }
        Some(BaseCoord::parse_at(&base_bytes[abs..], abs)?)
    };
    let max_coord = if max_off_rel == 0 {
        None
    } else {
        let abs = off
            .checked_add(max_off_rel)
            .ok_or(Error::BadStructure("BASE: MinMax maxCoord offset overflow"))?;
        if abs >= base_bytes.len() {
            return Err(Error::UnexpectedEof);
        }
        Some(BaseCoord::parse_at(&base_bytes[abs..], abs)?)
    };
    let body_start = off
        .checked_add(6)
        .ok_or(Error::BadStructure("BASE: MinMax body overflow"))?;
    let body_end = body_start
        .checked_add(
            feat_count
                .checked_mul(8)
                .ok_or(Error::BadStructure("BASE: featMinMaxCount * 8 overflow"))?,
        )
        .ok_or(Error::BadStructure("BASE: MinMax body overflow"))?;
    if body_end > base_bytes.len() {
        return Err(Error::UnexpectedEof);
    }
    let mut feat_min_max_records = Vec::with_capacity(feat_count);
    for i in 0..feat_count {
        let p = body_start + i * 8;
        let feature_tag = [
            base_bytes[p],
            base_bytes[p + 1],
            base_bytes[p + 2],
            base_bytes[p + 3],
        ];
        let f_min_off = read_u16(base_bytes, p + 4)? as usize;
        let f_max_off = read_u16(base_bytes, p + 6)? as usize;
        let feat_min = if f_min_off == 0 {
            None
        } else {
            let abs = off.checked_add(f_min_off).ok_or(Error::BadStructure(
                "BASE: FeatMinMax minCoord offset overflow",
            ))?;
            if abs >= base_bytes.len() {
                return Err(Error::UnexpectedEof);
            }
            Some(BaseCoord::parse_at(&base_bytes[abs..], abs)?)
        };
        let feat_max = if f_max_off == 0 {
            None
        } else {
            let abs = off.checked_add(f_max_off).ok_or(Error::BadStructure(
                "BASE: FeatMinMax maxCoord offset overflow",
            ))?;
            if abs >= base_bytes.len() {
                return Err(Error::UnexpectedEof);
            }
            Some(BaseCoord::parse_at(&base_bytes[abs..], abs)?)
        };
        feat_min_max_records.push(FeatMinMaxRecord {
            feature_tag,
            min_coord: feat_min,
            max_coord: feat_max,
        });
    }
    Ok(MinMaxTable {
        min_coord,
        max_coord,
        feat_min_max_records,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convenience: append a BaseCoordFormat1 record to `buf` and
    /// return its starting offset (relative to `buf` start).
    fn push_coord_f1(buf: &mut Vec<u8>, coord: i16) -> usize {
        let off = buf.len();
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.extend_from_slice(&coord.to_be_bytes());
        off
    }

    fn push_coord_f2(buf: &mut Vec<u8>, coord: i16, ref_glyph: u16, contour_pt: u16) -> usize {
        let off = buf.len();
        buf.extend_from_slice(&2u16.to_be_bytes());
        buf.extend_from_slice(&coord.to_be_bytes());
        buf.extend_from_slice(&ref_glyph.to_be_bytes());
        buf.extend_from_slice(&contour_pt.to_be_bytes());
        off
    }

    fn push_coord_f3(buf: &mut Vec<u8>, coord: i16, device_off: u16) -> usize {
        let off = buf.len();
        buf.extend_from_slice(&3u16.to_be_bytes());
        buf.extend_from_slice(&coord.to_be_bytes());
        buf.extend_from_slice(&device_off.to_be_bytes());
        off
    }

    /// Construct the §6.3.1.4 Example-1-shaped BASE table: a single
    /// HorizAxis with the alphabetical baseline tags `ideo`, `romn`
    /// and one script (`latn`) whose BaseValues references both
    /// baselines with the §6.3.1.3 worked-example coordinates
    /// (roman = 0, ideographic = -120 — typical English-on-Japanese
    /// dominant-run setup) plus a default MinMax with min/max extents
    /// 1750 / -432.
    ///
    /// Returned bytes are the full BASE table; offsets are wired
    /// dynamically so the layout reflects what an actual font would
    /// ship (no constant-offset shortcuts).
    fn build_horiz_only_example() -> Vec<u8> {
        // We build BASE as a single byte vector, then patch the
        // Offset fields after each child's address is known.
        let mut buf: Vec<u8> = Vec::new();

        // BASE header (v1.0, no IVS trailer).
        buf.extend_from_slice(&BASE_MAJOR_VERSION.to_be_bytes());
        buf.extend_from_slice(&BASE_MINOR_VERSION_0.to_be_bytes());
        let horiz_axis_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes()); // horizAxisOffset (patch)
        buf.extend_from_slice(&0u16.to_be_bytes()); // vertAxisOffset = NULL

        // ---- HorizAxis ----
        let horiz_axis_start = buf.len();
        // Patch BASE header.
        let off_he = (horiz_axis_start as u16).to_be_bytes();
        buf[horiz_axis_off_pos..horiz_axis_off_pos + 2].copy_from_slice(&off_he);
        let tag_list_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes()); // baseTagListOffset (patch)
        let script_list_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes()); // baseScriptListOffset (patch)

        // BaseTagList: 2 tags, alphabetical -> `ideo`, `romn`.
        let tag_list_start = buf.len();
        buf[tag_list_off_pos..tag_list_off_pos + 2]
            .copy_from_slice(&((tag_list_start - horiz_axis_start) as u16).to_be_bytes());
        buf.extend_from_slice(&2u16.to_be_bytes()); // baseTagCount
        buf.extend_from_slice(b"ideo");
        buf.extend_from_slice(b"romn");

        // BaseScriptList: 1 script (`latn`).
        let script_list_start = buf.len();
        buf[script_list_off_pos..script_list_off_pos + 2]
            .copy_from_slice(&((script_list_start - horiz_axis_start) as u16).to_be_bytes());
        buf.extend_from_slice(&1u16.to_be_bytes()); // baseScriptCount
        buf.extend_from_slice(b"latn");
        let latn_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes()); // baseScriptOffset (patch)

        // ---- BaseScript (latn) ----
        let latn_start = buf.len();
        buf[latn_off_pos..latn_off_pos + 2]
            .copy_from_slice(&((latn_start - script_list_start) as u16).to_be_bytes());
        let bv_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes()); // baseValuesOffset (patch)
        let dmm_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes()); // defaultMinMaxOffset (patch)
        buf.extend_from_slice(&0u16.to_be_bytes()); // baseLangSysCount = 0

        // ---- BaseValues ----
        // defaultBaselineIndex = 1 (romn — second tag in BaseTagList)
        // baseCoordCount = 2 (parallel to BaseTagList)
        let bv_start = buf.len();
        buf[bv_off_pos..bv_off_pos + 2]
            .copy_from_slice(&((bv_start - latn_start) as u16).to_be_bytes());
        buf.extend_from_slice(&1u16.to_be_bytes()); // defaultBaselineIndex = romn
        buf.extend_from_slice(&2u16.to_be_bytes()); // baseCoordCount
        let bc0_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes()); // baseCoords[0] (ideo) patch
        let bc1_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes()); // baseCoords[1] (romn) patch

        // BaseCoord for ideo (Format 1, coordinate = -120).
        let bc0_start = buf.len();
        buf[bc0_off_pos..bc0_off_pos + 2]
            .copy_from_slice(&((bc0_start - bv_start) as u16).to_be_bytes());
        let _ = push_coord_f1(&mut buf, -120);
        // BaseCoord for romn (Format 1, coordinate = 0).
        let bc1_start = buf.len();
        buf[bc1_off_pos..bc1_off_pos + 2]
            .copy_from_slice(&((bc1_start - bv_start) as u16).to_be_bytes());
        let _ = push_coord_f1(&mut buf, 0);

        // ---- default MinMax ----
        let dmm_start = buf.len();
        buf[dmm_off_pos..dmm_off_pos + 2]
            .copy_from_slice(&((dmm_start - latn_start) as u16).to_be_bytes());
        let dmm_min_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes()); // minCoord (patch)
        let dmm_max_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes()); // maxCoord (patch)
        buf.extend_from_slice(&0u16.to_be_bytes()); // featMinMaxCount = 0

        // MinMax minCoord (Format 1, -432).
        let dmin_start = buf.len();
        buf[dmm_min_off_pos..dmm_min_off_pos + 2]
            .copy_from_slice(&((dmin_start - dmm_start) as u16).to_be_bytes());
        let _ = push_coord_f1(&mut buf, -432);
        // MinMax maxCoord (Format 1, 1750).
        let dmax_start = buf.len();
        buf[dmm_max_off_pos..dmm_max_off_pos + 2]
            .copy_from_slice(&((dmax_start - dmm_start) as u16).to_be_bytes());
        let _ = push_coord_f1(&mut buf, 1750);

        buf
    }

    #[test]
    fn parses_horiz_only_worked_example() {
        let bytes = build_horiz_only_example();
        let base = BaseTable::parse(&bytes).expect("parse");
        assert_eq!(base.major_version, BASE_MAJOR_VERSION);
        assert_eq!(base.minor_version, BASE_MINOR_VERSION_0);
        assert!(base.vert_axis.is_none());
        let h = base.horiz_axis.as_ref().expect("HorizAxis present");
        let tags = h.baseline_tags.as_ref().expect("BaseTagList present");
        assert_eq!(tags, &vec![*b"ideo", *b"romn"]);
        assert_eq!(h.base_scripts.len(), 1);
        let latn = &h.base_scripts[0];
        assert_eq!(latn.script_tag, *b"latn");
        let bv = latn.base_script.base_values.as_ref().expect("BaseValues");
        assert_eq!(bv.default_baseline_index, 1);
        assert_eq!(bv.base_coords.len(), 2);
        // baseCoords[0] is the `ideo` coordinate (-120); baseCoords[1]
        // is the `romn` coordinate (0). The §6.3.1.3 ordering
        // mandates that the array index lines up with the BaseTagList
        // index, so the same lookup walks the same coords.
        assert_eq!(bv.base_coords[0].coordinate(), -120);
        assert_eq!(bv.base_coords[1].coordinate(), 0);
        let dmm = latn
            .base_script
            .default_min_max
            .as_ref()
            .expect("default MinMax");
        assert_eq!(dmm.min_coord.unwrap().coordinate(), -432);
        assert_eq!(dmm.max_coord.unwrap().coordinate(), 1750);
        assert_eq!(dmm.feat_min_max_records.len(), 0);
        assert_eq!(latn.base_script.base_lang_sys_records.len(), 0);

        // Accessor helpers.
        assert_eq!(h.baseline_index_for_tag(*b"ideo"), Some(0));
        assert_eq!(h.baseline_index_for_tag(*b"romn"), Some(1));
        assert_eq!(h.baseline_index_for_tag(*b"hang"), None);
        assert!(h.base_script_for_tag(*b"latn").is_some());
        assert!(h.base_script_for_tag(*b"arab").is_none());
    }

    #[test]
    fn rejects_short_header() {
        assert!(matches!(
            BaseTable::parse(&[0u8; 7]),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_wrong_major_version() {
        let mut b = build_horiz_only_example();
        b[0..2].copy_from_slice(&2u16.to_be_bytes());
        assert!(matches!(BaseTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_unknown_minor_version() {
        let mut b = build_horiz_only_example();
        // Walk it to v1.2 — we accept only 0 / 1.
        b[2..4].copy_from_slice(&2u16.to_be_bytes());
        assert!(matches!(BaseTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn parses_format2_and_format3_base_coords() {
        // Build a minimal HorizAxis with one script + one baseline +
        // BaseCoord formats 2 and 3 exercised via the default MinMax
        // (minCoord = Format2, maxCoord = Format3 with non-zero
        // device offset).
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&BASE_MAJOR_VERSION.to_be_bytes());
        buf.extend_from_slice(&BASE_MINOR_VERSION_0.to_be_bytes());
        let horiz_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());

        // HorizAxis (single baseline + single script, no values).
        let horiz_start = buf.len();
        buf[horiz_off_pos..horiz_off_pos + 2].copy_from_slice(&(horiz_start as u16).to_be_bytes());
        let tag_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes());
        let script_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes());

        // BaseTagList: 1 tag (`romn`).
        let tag_start = buf.len();
        buf[tag_off_pos..tag_off_pos + 2]
            .copy_from_slice(&((tag_start - horiz_start) as u16).to_be_bytes());
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.extend_from_slice(b"romn");

        // BaseScriptList: 1 entry (`latn`) -> BaseScript with only a
        // default MinMax (no BaseValues, no BaseLangSysRecord).
        let script_start = buf.len();
        buf[script_off_pos..script_off_pos + 2]
            .copy_from_slice(&((script_start - horiz_start) as u16).to_be_bytes());
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.extend_from_slice(b"latn");
        let latn_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes());

        // BaseScript: BaseValues = NULL, defaultMinMax present.
        let latn_start = buf.len();
        buf[latn_off_pos..latn_off_pos + 2]
            .copy_from_slice(&((latn_start - script_start) as u16).to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes()); // baseValuesOffset = NULL
        let dmm_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes()); // defaultMinMaxOffset (patch)
        buf.extend_from_slice(&0u16.to_be_bytes()); // baseLangSysCount = 0

        // MinMax: minCoord = Format2, maxCoord = Format3 with a
        // non-zero device offset that lands inside the table.
        let dmm_start = buf.len();
        buf[dmm_off_pos..dmm_off_pos + 2]
            .copy_from_slice(&((dmm_start - latn_start) as u16).to_be_bytes());
        let dmm_min_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes());
        let dmm_max_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes()); // featMinMaxCount = 0

        // BaseCoord Format2 at min (coord = -500, refGlyph = 42, point = 7).
        let f2_start = buf.len();
        buf[dmm_min_pos..dmm_min_pos + 2]
            .copy_from_slice(&((f2_start - dmm_start) as u16).to_be_bytes());
        let _ = push_coord_f2(&mut buf, -500, 42, 7);

        // BaseCoord Format3 at max (coord = 1200, device offset
        // relative to BaseCoord start = 10 — well past the BaseCoord's
        // own 6 bytes but inside the BASE table; we won't decode the
        // device payload here, only the device_offset survives).
        let f3_start = buf.len();
        buf[dmm_max_pos..dmm_max_pos + 2]
            .copy_from_slice(&((f3_start - dmm_start) as u16).to_be_bytes());
        let _ = push_coord_f3(&mut buf, 1200, 10);
        // Pad with the bytes the device offset would address — the
        // parser only validates the BaseCoord itself, but for realism
        // the spare bytes are appended.
        buf.extend_from_slice(&[0u8; 16]);

        let base = BaseTable::parse(&buf).expect("parse");
        let h = base.horiz_axis.unwrap();
        let latn = &h.base_scripts[0].base_script;
        let dmm = latn.default_min_max.as_ref().unwrap();
        match dmm.min_coord.unwrap() {
            BaseCoord::Format2 {
                coordinate,
                reference_glyph,
                base_coord_point,
            } => {
                assert_eq!(coordinate, -500);
                assert_eq!(reference_glyph, 42);
                assert_eq!(base_coord_point, 7);
            }
            other => panic!("expected Format2, got {other:?}"),
        }
        match dmm.max_coord.unwrap() {
            BaseCoord::Format3 {
                coordinate,
                device_offset,
                ..
            } => {
                assert_eq!(coordinate, 1200);
                assert_eq!(device_offset, Some(10));
            }
            other => panic!("expected Format3, got {other:?}"),
        }
        assert_eq!(dmm.min_coord.unwrap().format(), 2);
        assert_eq!(dmm.max_coord.unwrap().format(), 3);
    }

    #[test]
    fn parses_lang_sys_and_feat_min_max() {
        // Build a HorizAxis with one script (`latn`), no BaseValues,
        // and one BaseLangSysRecord (`URD `) whose MinMax overrides
        // the defaults and lists one FeatMinMaxRecord (`sups`).
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&BASE_MAJOR_VERSION.to_be_bytes());
        buf.extend_from_slice(&BASE_MINOR_VERSION_0.to_be_bytes());
        let horiz_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());

        let horiz_start = buf.len();
        buf[horiz_off_pos..horiz_off_pos + 2].copy_from_slice(&(horiz_start as u16).to_be_bytes());
        let tag_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes());
        let script_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes());

        let tag_start = buf.len();
        buf[tag_off_pos..tag_off_pos + 2]
            .copy_from_slice(&((tag_start - horiz_start) as u16).to_be_bytes());
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.extend_from_slice(b"romn");

        let script_start = buf.len();
        buf[script_off_pos..script_off_pos + 2]
            .copy_from_slice(&((script_start - horiz_start) as u16).to_be_bytes());
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.extend_from_slice(b"latn");
        let latn_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes());

        // BaseScript with baseLangSysCount = 1 and no BaseValues /
        // defaultMinMax. The single BaseLangSysRecord wires URD ->
        // MinMax.
        let latn_start = buf.len();
        buf[latn_off_pos..latn_off_pos + 2]
            .copy_from_slice(&((latn_start - script_start) as u16).to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes()); // baseValuesOffset = NULL
        buf.extend_from_slice(&0u16.to_be_bytes()); // defaultMinMaxOffset = NULL
        buf.extend_from_slice(&1u16.to_be_bytes()); // baseLangSysCount = 1
        buf.extend_from_slice(b"URD "); // lang_sys_tag
        let urd_mm_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes());

        // MinMax for URD: minCoord = NULL, maxCoord = Format1, one
        // FeatMinMaxRecord for `sups` (superscript) with both
        // coordinates present.
        let urd_mm_start = buf.len();
        buf[urd_mm_off_pos..urd_mm_off_pos + 2]
            .copy_from_slice(&((urd_mm_start - latn_start) as u16).to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes()); // minCoord = NULL
        let urd_max_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes()); // maxCoord (patch)
        buf.extend_from_slice(&1u16.to_be_bytes()); // featMinMaxCount = 1
        buf.extend_from_slice(b"sups"); // featureTag
        let sups_min_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes()); // sups minCoord (patch)
        let sups_max_off_pos = buf.len();
        buf.extend_from_slice(&0u16.to_be_bytes()); // sups maxCoord (patch)

        // Coords (all Format1 for brevity).
        let urd_max_start = buf.len();
        buf[urd_max_off_pos..urd_max_off_pos + 2]
            .copy_from_slice(&((urd_max_start - urd_mm_start) as u16).to_be_bytes());
        let _ = push_coord_f1(&mut buf, 1800);
        let sups_min_start = buf.len();
        buf[sups_min_off_pos..sups_min_off_pos + 2]
            .copy_from_slice(&((sups_min_start - urd_mm_start) as u16).to_be_bytes());
        let _ = push_coord_f1(&mut buf, 600);
        let sups_max_start = buf.len();
        buf[sups_max_off_pos..sups_max_off_pos + 2]
            .copy_from_slice(&((sups_max_start - urd_mm_start) as u16).to_be_bytes());
        let _ = push_coord_f1(&mut buf, 2100);

        let base = BaseTable::parse(&buf).expect("parse");
        let h = base.horiz_axis.unwrap();
        let latn = &h.base_scripts[0].base_script;
        assert!(latn.default_min_max.is_none());
        assert_eq!(latn.base_lang_sys_records.len(), 1);
        let lsr = &latn.base_lang_sys_records[0];
        assert_eq!(lsr.lang_sys_tag, *b"URD ");
        assert!(lsr.min_max.min_coord.is_none());
        assert_eq!(lsr.min_max.max_coord.unwrap().coordinate(), 1800);
        assert_eq!(lsr.min_max.feat_min_max_records.len(), 1);
        let feat = &lsr.min_max.feat_min_max_records[0];
        assert_eq!(feat.feature_tag, *b"sups");
        assert_eq!(feat.min_coord.unwrap().coordinate(), 600);
        assert_eq!(feat.max_coord.unwrap().coordinate(), 2100);

        assert!(latn.min_max_for_lang_sys(*b"URD ").is_some());
        assert!(latn.min_max_for_lang_sys(*b"DEU ").is_none());
    }

    #[test]
    fn rejects_axis_missing_base_script_list() {
        // BASE header with horiz axis -> axis with baseScriptListOffset = 0.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&BASE_MAJOR_VERSION.to_be_bytes());
        buf.extend_from_slice(&BASE_MINOR_VERSION_0.to_be_bytes());
        buf.extend_from_slice(&8u16.to_be_bytes()); // horizAxisOffset
        buf.extend_from_slice(&0u16.to_be_bytes()); // vertAxisOffset = NULL
                                                    // Axis at +8: baseTagListOffset = 0, baseScriptListOffset = 0
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        let res = BaseTable::parse(&buf);
        assert!(matches!(res, Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_base_tag_list_running_past_end() {
        // BASE v1.0 with horizAxis at +8; axis BaseTagList at +12
        // claiming 5 tags but only 4 bytes follow.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&BASE_MAJOR_VERSION.to_be_bytes());
        buf.extend_from_slice(&BASE_MINOR_VERSION_0.to_be_bytes());
        buf.extend_from_slice(&8u16.to_be_bytes()); // horizAxisOffset
        buf.extend_from_slice(&0u16.to_be_bytes()); // vertAxisOffset = NULL
                                                    // Axis at +8.
        buf.extend_from_slice(&4u16.to_be_bytes()); // baseTagListOffset = +4 (rel)
        buf.extend_from_slice(&8u16.to_be_bytes()); // baseScriptListOffset = +8 (rel)
                                                    // BaseTagList at +12: count = 5, but only 4 bytes follow.
        buf.extend_from_slice(&5u16.to_be_bytes());
        buf.extend_from_slice(b"romn");
        let res = BaseTable::parse(&buf);
        assert!(matches!(res, Err(Error::UnexpectedEof)));
    }

    #[test]
    fn parses_v1_1_with_item_var_store() {
        // BASE header v1.1 with HorizAxis (NULL) + VertAxis (NULL)
        // and itemVarStoreOffset = 12 pointing at four bytes of
        // dummy IVS-shaped payload. The parser bounds-checks the
        // offset against the slice and surfaces the raw bytes;
        // ItemVariationStore decoding is the consumer's job.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&BASE_MAJOR_VERSION.to_be_bytes());
        buf.extend_from_slice(&BASE_MINOR_VERSION_1.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes()); // horizAxisOffset = NULL
        buf.extend_from_slice(&0u16.to_be_bytes()); // vertAxisOffset = NULL
        buf.extend_from_slice(&12u32.to_be_bytes()); // itemVarStoreOffset = 12
                                                     // Dummy IVS payload (the actual bytes are not parsed here).
        buf.extend_from_slice(&[0xAB, 0xCD, 0xEF, 0x01]);
        let base = BaseTable::parse(&buf).expect("parse");
        assert_eq!(base.major_version, BASE_MAJOR_VERSION);
        assert_eq!(base.minor_version, BASE_MINOR_VERSION_1);
        assert!(base.horiz_axis.is_none());
        assert!(base.vert_axis.is_none());
        assert_eq!(base.item_var_store_offset, Some(12));
        assert_eq!(
            base.item_var_store_bytes(),
            Some([0xAB, 0xCD, 0xEF, 0x01].as_slice())
        );
    }

    #[test]
    fn rejects_v1_1_item_var_store_offset_past_end() {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&BASE_MAJOR_VERSION.to_be_bytes());
        buf.extend_from_slice(&BASE_MINOR_VERSION_1.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        // itemVarStoreOffset = 999 — past the slice end (12 bytes).
        buf.extend_from_slice(&999u32.to_be_bytes());
        assert!(matches!(
            BaseTable::parse(&buf),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn v1_0_table_has_no_item_var_store() {
        let bytes = build_horiz_only_example();
        let base = BaseTable::parse(&bytes).expect("parse");
        assert!(base.item_var_store_offset.is_none());
        assert!(base.item_var_store_bytes().is_none());
    }

    #[test]
    fn base_coord_format1_rejects_unknown_format() {
        // Coordinate-format = 7 is undefined.
        let bytes = [0x00, 0x07, 0x00, 0x00];
        assert!(matches!(
            BaseCoord::parse_at(&bytes, 0),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn base_coord_format2_rejects_short_slice() {
        // Format 2 needs 8 bytes; provide 6.
        let bytes = [0x00, 0x02, 0x00, 0x00, 0x00, 0x00];
        assert!(matches!(
            BaseCoord::parse_at(&bytes, 0),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn base_coord_format3_rejects_short_slice() {
        // Format 3 needs 6 bytes; provide 5.
        let bytes = [0x00, 0x03, 0x00, 0x00, 0x00];
        assert!(matches!(
            BaseCoord::parse_at(&bytes, 0),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn base_coord_format3_records_absolute_device_offset() {
        // Format 3, coord = 50, deviceOffset = 4 (BaseCoord-relative).
        // Parsed at abs_off = 100 → device_abs_offset = 104.
        let bytes = [0x00, 0x03, 0x00, 0x32, 0x00, 0x04];
        match BaseCoord::parse_at(&bytes, 100).unwrap() {
            BaseCoord::Format3 {
                coordinate,
                device_offset,
                device_abs_offset,
            } => {
                assert_eq!(coordinate, 50);
                assert_eq!(device_offset, Some(4));
                assert_eq!(device_abs_offset, Some(104));
            }
            other => panic!("expected Format3, got {other:?}"),
        }
        // NULL device offset → no absolute offset recorded.
        let null_bytes = [0x00, 0x03, 0x00, 0x32, 0x00, 0x00];
        match BaseCoord::parse_at(&null_bytes, 100).unwrap() {
            BaseCoord::Format3 {
                device_offset,
                device_abs_offset,
                ..
            } => {
                assert_eq!(device_offset, None);
                assert_eq!(device_abs_offset, None);
            }
            other => panic!("expected Format3, got {other:?}"),
        }
    }

    #[test]
    fn base_coord_format3_resolves_variation_index() {
        // Lay out a synthetic "BASE" byte buffer: a Format-3 BaseCoord
        // at offset 0 (coord 50, deviceOffset = 6 → a VariationIndex
        // at absolute offset 6), followed by the VariationIndex and an
        // IVS the resolver reads.
        let mut base_bytes = Vec::new();
        // [0..6) BaseCoord Format 3.
        base_bytes.extend_from_slice(&3u16.to_be_bytes()); // format
        base_bytes.extend_from_slice(&50i16.to_be_bytes()); // coord
        base_bytes.extend_from_slice(&6u16.to_be_bytes()); // deviceOffset
                                                           // [6..12) VariationIndex { outer=0, inner=0, fmt=0x8000 }.
        base_bytes.extend_from_slice(&0u16.to_be_bytes());
        base_bytes.extend_from_slice(&0u16.to_be_bytes());
        base_bytes.extend_from_slice(&0x8000u16.to_be_bytes());

        let coord = BaseCoord::parse_at(&base_bytes[0..6], 0).unwrap();

        // Build a single-region IVS with delta +80.
        let mut ivs_b = vec![0u8; 32];
        ivs_b[0..2].copy_from_slice(&1u16.to_be_bytes());
        ivs_b[2..6].copy_from_slice(&12u32.to_be_bytes());
        ivs_b[6..8].copy_from_slice(&1u16.to_be_bytes());
        ivs_b[8..12].copy_from_slice(&22u32.to_be_bytes());
        ivs_b[12..14].copy_from_slice(&1u16.to_be_bytes());
        ivs_b[14..16].copy_from_slice(&1u16.to_be_bytes());
        ivs_b[16..18].copy_from_slice(&0i16.to_be_bytes());
        ivs_b[18..20].copy_from_slice(&16384i16.to_be_bytes());
        ivs_b[20..22].copy_from_slice(&16384i16.to_be_bytes());
        ivs_b[22..24].copy_from_slice(&1u16.to_be_bytes());
        ivs_b[24..26].copy_from_slice(&1u16.to_be_bytes());
        ivs_b[26..28].copy_from_slice(&1u16.to_be_bytes());
        ivs_b[28..30].copy_from_slice(&0u16.to_be_bytes());
        ivs_b[30..32].copy_from_slice(&80i16.to_be_bytes());
        let ivs = ItemVariationStore::parse(&ivs_b).unwrap();

        // Default instance → static 50.
        assert_eq!(coord.resolve(&base_bytes, Some(&ivs), &[0.0]), 50);
        // Max instance → 50 + 80 = 130.
        assert_eq!(coord.resolve(&base_bytes, Some(&ivs), &[1.0]), 130);
        // Half → 50 + 40 = 90.
        assert_eq!(coord.resolve(&base_bytes, Some(&ivs), &[0.5]), 90);
        // No IVS → static.
        assert_eq!(coord.resolve(&base_bytes, None, &[1.0]), 50);
        // coordinate() still surfaces the unresolved value.
        assert_eq!(coord.coordinate(), 50);
    }

    #[test]
    fn rejects_zero_min_max_offset_in_base_lang_sys_record() {
        // Construct a BaseScript whose single BaseLangSysRecord has
        // minMaxOffset = 0; per §6.3.1.3 the field "Offset to MinMax
        // table" carries no "may be NULL" note (unlike the
        // defaultMinMaxOffset on the parent), so the parser treats
        // zero as a structural violation.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&BASE_MAJOR_VERSION.to_be_bytes());
        buf.extend_from_slice(&BASE_MINOR_VERSION_0.to_be_bytes());
        buf.extend_from_slice(&8u16.to_be_bytes()); // horizAxisOffset
        buf.extend_from_slice(&0u16.to_be_bytes());
        // Axis at +8.
        buf.extend_from_slice(&0u16.to_be_bytes()); // baseTagListOffset = NULL
        buf.extend_from_slice(&4u16.to_be_bytes()); // baseScriptListOffset = +4 (rel)
                                                    // BaseScriptList at +12.
        buf.extend_from_slice(&1u16.to_be_bytes()); // baseScriptCount
        buf.extend_from_slice(b"latn");
        buf.extend_from_slice(&8u16.to_be_bytes()); // baseScriptOffset = +8 (rel)
                                                    // BaseScript at +20.
        buf.extend_from_slice(&0u16.to_be_bytes()); // baseValuesOffset = NULL
        buf.extend_from_slice(&0u16.to_be_bytes()); // defaultMinMaxOffset = NULL
        buf.extend_from_slice(&1u16.to_be_bytes()); // baseLangSysCount = 1
        buf.extend_from_slice(b"URD ");
        buf.extend_from_slice(&0u16.to_be_bytes()); // minMaxOffset = 0 (illegal)
        let res = BaseTable::parse(&buf);
        assert!(matches!(res, Err(Error::BadStructure(_))));
    }
}
