//! `STAT` — Style Attributes Table.
//!
//! Spec: ISO/IEC 14496-22:2019 §7.3.7 ("STAT – Style attributes table").
//!
//! STAT describes which design attributes distinguish font-style
//! variants within a font family (weight, width, slant, optical size,
//! plus arbitrary custom axes) and associates each value (or
//! combination of values) on those axes with a `name` table string ID.
//! It is required in variable fonts and recommended for new
//! non-variable fonts that need to surface non-WWS attributes through
//! application UIs.
//!
//! Three logical pieces live in the table:
//!
//! 1. **Header** — major/minor version, the design-axis stride, the
//!    arrays' counts + offsets, and (from v1.1) an
//!    `elidedFallbackNameID` used when every component of a composed
//!    subfamily name is flagged elidable.
//! 2. **AxisRecord array** — one record per design axis (`axisTag`,
//!    `axisNameID`, `axisOrdering`). For a variable font §7.3.7.2
//!    requires one record per `fvar` axis using the matching name ID,
//!    plus optional extra records for static "family" axes that aren't
//!    variation axes.
//! 3. **Axis value tables** — one of four formats:
//!    - **Format 1**: one axis + one value (e.g. wght=700 ⇒ "Bold").
//!    - **Format 2**: one axis + a (`nominalValue`, `rangeMin`,
//!      `rangeMax`) triple, used for ranged attributes like optical
//!      size ("Subhead" covers 6..10 pt with a nominal of 8).
//!    - **Format 3**: format 1 + a `linkedValue` for style-linked
//!      pairs (e.g. wght=400 → linked 700 to drive the "Bold" UI
//!      affordance).
//!    - **Format 4** (added in minor version 2): one combination of
//!      values across `axisCount` different axes, used for
//!      non-analytic instance names ("Florid" implying a particular
//!      tuple of values across custom axes).
//!
//! ## Header layout
//!
//! ```text
//!   0  / 2 / majorVersion              (== 1)
//!   2  / 2 / minorVersion              (1 / 2; 0 is deprecated)
//!   4  / 2 / designAxisSize            (stride; ≥ 8 in v1.x)
//!   6  / 2 / designAxisCount
//!   8  / 4 / offsetToDesignAxes        (relative to STAT start)
//!  12  / 2 / axisValueCount
//!  14  / 4 / offsetToAxisValueOffsets  (relative to STAT start)
//!  18  / 2 / elidedFallbackNameID      (v1.1+; absent in v1.0)
//! ```
//!
//! ## AxisRecord
//!
//! ```text
//!   0 / 4 / axisTag
//!   4 / 2 / axisNameID
//!   6 / 2 / axisOrdering
//! ```
//!
//! `designAxisSize` is honoured as the stride so a minor-version bump
//! that grows the record (preserving the first 8 bytes per §7.3.7.1)
//! decodes correctly with the trailing bytes ignored.
//!
//! ## AxisValueTable
//!
//! Format 1:
//! ```text
//!   0 / 2 / format              (== 1)
//!   2 / 2 / axisIndex
//!   4 / 2 / flags
//!   6 / 2 / valueNameID
//!   8 / 4 / value               (Fixed / 16.16)
//! ```
//!
//! Format 2:
//! ```text
//!   0 / 2 / format              (== 2)
//!   2 / 2 / axisIndex
//!   4 / 2 / flags
//!   6 / 2 / valueNameID
//!   8 / 4 / nominalValue        (Fixed)
//!  12 / 4 / rangeMinValue       (Fixed; 0x80000000 ⇒ -∞)
//!  16 / 4 / rangeMaxValue       (Fixed; 0x7FFFFFFF ⇒ +∞)
//! ```
//!
//! Format 3:
//! ```text
//!   0 / 2 / format              (== 3)
//!   2 / 2 / axisIndex
//!   4 / 2 / flags
//!   6 / 2 / valueNameID
//!   8 / 4 / value               (Fixed)
//!  12 / 4 / linkedValue         (Fixed)
//! ```
//!
//! Format 4 (minor version ≥ 2):
//! ```text
//!   0 / 2 / format              (== 4)
//!   2 / 2 / axisCount
//!   4 / 2 / flags
//!   6 / 2 / valueNameID
//!   8 / 6*axisCount / axisValues  (uint16 axisIndex + Fixed value)
//! ```
//!
//! The OLDER_SIBLING_FONT_ATTRIBUTE (0x0001) and
//! ELIDABLE_AXIS_VALUE_NAME (0x0002) bits in `flags` are surfaced as
//! convenience accessors on each variant.
//!
//! ## Out of scope
//!
//! - The "two format-2 tables on one axis with overlapping ranges"
//!   tie-break (§7.3.7.3) is documented as caller policy; we keep all
//!   records in document order and expose the entire list — picking
//!   one out of an overlap is application-domain.
//! - The `linkedValue` mechanism for style-linked Bold/Italic UI is
//!   surfaced (the field is decoded and accessible) but not consumed
//!   here; consumers wire it into their text-style controls.

use crate::parser::{read_i32, read_u16, read_u32};
use crate::Error;

/// Bound on `designAxisCount`. The spec allows up to 65 535; real fonts
/// stay well under double digits (Inter ships three: opsz, wght, ital).
const MAX_AXIS_RECORDS: u16 = 1024;

/// Bound on `axisValueCount`. Real fonts ship dozens at most.
const MAX_AXIS_VALUES: u16 = 16384;

/// Bound on `axisCount` inside a format-4 axis-value record.
const MAX_FORMAT4_AXES: u16 = 1024;

/// `OLDER_SIBLING_FONT_ATTRIBUTE` (§7.3.7.3 flags). This axis value
/// table is intended to back-fill information into earlier siblings in
/// the family that did not include the axis at all.
pub const FLAG_OLDER_SIBLING_FONT_ATTRIBUTE: u16 = 0x0001;

/// `ELIDABLE_AXIS_VALUE_NAME` (§7.3.7.3 flags). The axis value
/// represents the "normal" point on the axis and the display name may
/// be omitted when composing a face name.
pub const FLAG_ELIDABLE_AXIS_VALUE_NAME: u16 = 0x0002;

/// Sentinel value for a format-2 `rangeMinValue` meaning "negative
/// infinity" (§7.3.7.3).
pub const RANGE_MIN_NEG_INFINITY: i32 = i32::MIN; // 0x80000000

/// Sentinel value for a format-2 `rangeMaxValue` meaning "positive
/// infinity" (§7.3.7.3).
pub const RANGE_MAX_POS_INFINITY: i32 = i32::MAX; // 0x7FFFFFFF

/// One axis record (§7.3.7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxisRecord {
    /// Four-byte axis tag, e.g. `*b"wght"`, `*b"wdth"`, `*b"opsz"`.
    pub axis_tag: [u8; 4],
    /// `name` table nameID for the axis display string. Must satisfy
    /// 255 < `axis_name_id` < 32 768 per §7.3.7.2.
    pub axis_name_id: u16,
    /// Application sort/ordering hint. Lower values sort first.
    pub axis_ordering: u16,
}

/// An axis value table (§7.3.7.3).
#[derive(Debug, Clone)]
pub enum AxisValue {
    /// Format 1 — single (axis, value) point.
    Format1 {
        axis_index: u16,
        flags: u16,
        value_name_id: u16,
        /// Decoded Fixed (16.16) value, e.g. 700.0 for "Bold".
        value: f32,
    },
    /// Format 2 — single axis with a (nominal, [min, max]) range.
    Format2 {
        axis_index: u16,
        flags: u16,
        value_name_id: u16,
        nominal_value: f32,
        range_min_value: f32,
        range_max_value: f32,
        /// Raw `rangeMinValue` from the file. `RANGE_MIN_NEG_INFINITY`
        /// is the negative-infinity sentinel.
        raw_range_min: i32,
        /// Raw `rangeMaxValue`. `RANGE_MAX_POS_INFINITY` is the
        /// positive-infinity sentinel.
        raw_range_max: i32,
    },
    /// Format 3 — single point with a style-linked counterpart.
    Format3 {
        axis_index: u16,
        flags: u16,
        value_name_id: u16,
        value: f32,
        linked_value: f32,
    },
    /// Format 4 — combination across multiple axes (added in v1.2).
    Format4 {
        flags: u16,
        value_name_id: u16,
        /// One (axisIndex, value) pair per contributing axis. Per
        /// §7.3.7.3 the records may be in any order but each axisIndex
        /// must be unique inside this format-4 entry.
        axis_values: Vec<(u16, f32)>,
    },
}

impl AxisValue {
    /// Flags field (§7.3.7.3). Format 4 entries surface their `flags`
    /// here too.
    pub fn flags(&self) -> u16 {
        match self {
            Self::Format1 { flags, .. }
            | Self::Format2 { flags, .. }
            | Self::Format3 { flags, .. }
            | Self::Format4 { flags, .. } => *flags,
        }
    }

    /// `valueNameID` (§7.3.7.3).
    pub fn value_name_id(&self) -> u16 {
        match self {
            Self::Format1 { value_name_id, .. }
            | Self::Format2 { value_name_id, .. }
            | Self::Format3 { value_name_id, .. }
            | Self::Format4 { value_name_id, .. } => *value_name_id,
        }
    }

    /// Per §7.3.7.3, this flag designates that the table targets
    /// earlier-released siblings and should be ignored when describing
    /// the font that contains it.
    pub fn is_older_sibling_font_attribute(&self) -> bool {
        self.flags() & FLAG_OLDER_SIBLING_FONT_ATTRIBUTE != 0
    }

    /// Per §7.3.7.3, this flag designates the "normal" axis value
    /// whose name may be elided from composed subfamily strings.
    pub fn is_elidable(&self) -> bool {
        self.flags() & FLAG_ELIDABLE_AXIS_VALUE_NAME != 0
    }
}

/// Parsed STAT table (§7.3.7).
#[derive(Debug, Clone)]
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub struct StatTable {
    major_version: u16,
    minor_version: u16,
    axes: Vec<AxisRecord>,
    values: Vec<AxisValue>,
    /// `elidedFallbackNameID`. Always present in v1.1+; for v1.0 (which
    /// the spec marks deprecated) this is set to the sentinel name ID 2
    /// ("Regular") so callers don't carry a separate "version had it"
    /// flag — a deprecated v1.0 font is treated as if it requested the
    /// historical default.
    elided_fallback_name_id: u16,
}

impl StatTable {
    /// Parse the STAT table from a `&[u8]` rooted at the table start
    /// (i.e. `bytes[0..]` is `majorVersion`).
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        // The v1.0 header is 18 bytes (no elidedFallbackNameID); v1.1+
        // is 20. We require ≥ 18 up front and re-check before reading
        // the v1.1 field.
        if bytes.len() < 18 {
            return Err(Error::UnexpectedEof);
        }
        let major_version = read_u16(bytes, 0)?;
        let minor_version = read_u16(bytes, 2)?;
        if major_version != 1 {
            return Err(Error::BadStructure("STAT majorVersion != 1"));
        }
        let design_axis_size = read_u16(bytes, 4)?;
        let design_axis_count = read_u16(bytes, 6)?;
        let offset_to_design_axes = read_u32(bytes, 8)? as usize;
        let axis_value_count = read_u16(bytes, 12)?;
        let offset_to_axis_value_offsets = read_u32(bytes, 14)? as usize;

        // v1.1+ added elidedFallbackNameID at offset 18.
        let elided_fallback_name_id = if minor_version >= 1 {
            if bytes.len() < 20 {
                return Err(Error::UnexpectedEof);
            }
            read_u16(bytes, 18)?
        } else {
            // §7.3.7.1: "Use of version 1.0 is deprecated." We still
            // parse it (some legacy fonts may exist) and default the
            // fallback to nameID 2 — the conventional "Regular".
            2
        };

        if design_axis_count > MAX_AXIS_RECORDS {
            return Err(Error::BadStructure("STAT designAxisCount exceeds cap"));
        }
        if axis_value_count > MAX_AXIS_VALUES {
            return Err(Error::BadStructure("STAT axisValueCount exceeds cap"));
        }

        // Parse axis records. The stride is `designAxisSize`, which
        // must accommodate the four required fields (4 + 2 + 2 = 8);
        // any trailing bytes (future minor-version growth) are
        // ignored per §7.3.7.1.
        if design_axis_count > 0 {
            if design_axis_size < 8 {
                return Err(Error::BadStructure("STAT designAxisSize < 8"));
            }
            if offset_to_design_axes == 0 {
                return Err(Error::BadOffset);
            }
        }
        let mut axes = Vec::with_capacity(design_axis_count as usize);
        if design_axis_count > 0 {
            let stride = design_axis_size as usize;
            let total = (design_axis_count as usize)
                .checked_mul(stride)
                .and_then(|n| n.checked_add(offset_to_design_axes))
                .ok_or(Error::BadOffset)?;
            if total > bytes.len() {
                return Err(Error::UnexpectedEof);
            }
            for i in 0..design_axis_count as usize {
                let off = offset_to_design_axes + i * stride;
                let axis_tag = [bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]];
                let axis_name_id = read_u16(bytes, off + 4)?;
                let axis_ordering = read_u16(bytes, off + 6)?;
                axes.push(AxisRecord {
                    axis_tag,
                    axis_name_id,
                    axis_ordering,
                });
            }
        }

        // Parse axis value tables. The offsets array is `axisValueCount`
        // 16-bit offsets, each relative to the *start of the offsets
        // array* (not the STAT table) per §7.3.7.1.
        let mut values = Vec::with_capacity(axis_value_count as usize);
        if axis_value_count > 0 {
            if offset_to_axis_value_offsets == 0 {
                return Err(Error::BadOffset);
            }
            let array_end = (axis_value_count as usize)
                .checked_mul(2)
                .and_then(|n| n.checked_add(offset_to_axis_value_offsets))
                .ok_or(Error::BadOffset)?;
            if array_end > bytes.len() {
                return Err(Error::UnexpectedEof);
            }
            for i in 0..axis_value_count as usize {
                let off = offset_to_axis_value_offsets + i * 2;
                let rel = read_u16(bytes, off)? as usize;
                if rel == 0 {
                    return Err(Error::BadOffset);
                }
                let table_off = offset_to_axis_value_offsets
                    .checked_add(rel)
                    .ok_or(Error::BadOffset)?;
                if table_off >= bytes.len() {
                    return Err(Error::BadOffset);
                }
                values.push(parse_axis_value(&bytes[table_off..], design_axis_count)?);
            }
        }

        Ok(Self {
            major_version,
            minor_version,
            axes,
            values,
            elided_fallback_name_id,
        })
    }

    /// `majorVersion` field — always 1 in a well-formed file.
    pub fn major_version(&self) -> u16 {
        self.major_version
    }

    /// `minorVersion` field — 0 (deprecated), 1, or 2.
    pub fn minor_version(&self) -> u16 {
        self.minor_version
    }

    /// The full design-axes array in document order.
    pub fn axes(&self) -> &[AxisRecord] {
        &self.axes
    }

    /// The full axis-value array in document order.
    pub fn axis_values(&self) -> &[AxisValue] {
        &self.values
    }

    /// `elidedFallbackNameID` (§7.3.7.1) — the name ID used when every
    /// component of a composed subfamily would otherwise be elided. For
    /// a v1.0 header (no field present), the historical default of name
    /// ID 2 ("Regular") is returned.
    pub fn elided_fallback_name_id(&self) -> u16 {
        self.elided_fallback_name_id
    }

    /// Convenience: every axis-value record that lives on the axis with
    /// tag `axis_tag`. Returns an empty iterator when the tag is absent
    /// from the design-axis array. Format-4 records are matched when at
    /// least one of their (axisIndex, value) entries refers to this
    /// axis.
    pub fn axis_values_for_tag(&self, axis_tag: [u8; 4]) -> impl Iterator<Item = &AxisValue> + '_ {
        // Resolve to the index. If absent there's nothing to yield.
        let idx = self
            .axes
            .iter()
            .position(|a| a.axis_tag == axis_tag)
            .map(|i| i as u16);
        self.values.iter().filter(move |v| match (idx, v) {
            (Some(want), AxisValue::Format1 { axis_index, .. })
            | (Some(want), AxisValue::Format2 { axis_index, .. })
            | (Some(want), AxisValue::Format3 { axis_index, .. }) => *axis_index == want,
            (Some(want), AxisValue::Format4 { axis_values, .. }) => {
                axis_values.iter().any(|(ai, _)| *ai == want)
            }
            _ => false,
        })
    }
}

fn parse_axis_value(bytes: &[u8], design_axis_count: u16) -> Result<AxisValue, Error> {
    if bytes.len() < 8 {
        return Err(Error::UnexpectedEof);
    }
    let format = read_u16(bytes, 0)?;
    match format {
        1 => {
            if bytes.len() < 12 {
                return Err(Error::UnexpectedEof);
            }
            let axis_index = read_u16(bytes, 2)?;
            if axis_index >= design_axis_count {
                return Err(Error::BadStructure("STAT format-1 axisIndex out of range"));
            }
            let flags = read_u16(bytes, 4)?;
            let value_name_id = read_u16(bytes, 6)?;
            let value = fixed_to_f32(read_i32(bytes, 8)?);
            Ok(AxisValue::Format1 {
                axis_index,
                flags,
                value_name_id,
                value,
            })
        }
        2 => {
            if bytes.len() < 20 {
                return Err(Error::UnexpectedEof);
            }
            let axis_index = read_u16(bytes, 2)?;
            if axis_index >= design_axis_count {
                return Err(Error::BadStructure("STAT format-2 axisIndex out of range"));
            }
            let flags = read_u16(bytes, 4)?;
            let value_name_id = read_u16(bytes, 6)?;
            let raw_nominal = read_i32(bytes, 8)?;
            let raw_range_min = read_i32(bytes, 12)?;
            let raw_range_max = read_i32(bytes, 16)?;
            Ok(AxisValue::Format2 {
                axis_index,
                flags,
                value_name_id,
                nominal_value: fixed_to_f32(raw_nominal),
                range_min_value: fixed_to_f32(raw_range_min),
                range_max_value: fixed_to_f32(raw_range_max),
                raw_range_min,
                raw_range_max,
            })
        }
        3 => {
            if bytes.len() < 16 {
                return Err(Error::UnexpectedEof);
            }
            let axis_index = read_u16(bytes, 2)?;
            if axis_index >= design_axis_count {
                return Err(Error::BadStructure("STAT format-3 axisIndex out of range"));
            }
            let flags = read_u16(bytes, 4)?;
            let value_name_id = read_u16(bytes, 6)?;
            let value = fixed_to_f32(read_i32(bytes, 8)?);
            let linked_value = fixed_to_f32(read_i32(bytes, 12)?);
            Ok(AxisValue::Format3 {
                axis_index,
                flags,
                value_name_id,
                value,
                linked_value,
            })
        }
        4 => {
            // header: format/axisCount/flags/valueNameID = 8 bytes,
            // then axisCount * (uint16 axisIndex + Fixed value) = 6
            // bytes apiece.
            let axis_count = read_u16(bytes, 2)?;
            if axis_count == 0 {
                return Err(Error::BadStructure("STAT format-4 axisCount == 0"));
            }
            if axis_count > MAX_FORMAT4_AXES {
                return Err(Error::BadStructure("STAT format-4 axisCount exceeds cap"));
            }
            let flags = read_u16(bytes, 4)?;
            let value_name_id = read_u16(bytes, 6)?;
            let body_len = (axis_count as usize)
                .checked_mul(6)
                .and_then(|n| n.checked_add(8))
                .ok_or(Error::BadOffset)?;
            if body_len > bytes.len() {
                return Err(Error::UnexpectedEof);
            }
            let mut axis_values = Vec::with_capacity(axis_count as usize);
            for i in 0..axis_count as usize {
                let off = 8 + i * 6;
                let axis_index = read_u16(bytes, off)?;
                if axis_index >= design_axis_count {
                    return Err(Error::BadStructure("STAT format-4 axisIndex out of range"));
                }
                // "Each AxisValue record shall have a different
                // axisIndex value." (§7.3.7.3)
                if axis_values
                    .iter()
                    .any(|(prev, _): &(u16, f32)| *prev == axis_index)
                {
                    return Err(Error::BadStructure("STAT format-4 duplicate axisIndex"));
                }
                let value = fixed_to_f32(read_i32(bytes, off + 2)?);
                axis_values.push((axis_index, value));
            }
            Ok(AxisValue::Format4 {
                flags,
                value_name_id,
                axis_values,
            })
        }
        _ => {
            // "If the format is not recognized, then the axis value
            // table can be ignored." (§7.3.7.3). We represent that as a
            // structural error so callers see the unsupported value
            // explicitly; future formats can be added without breaking
            // the existing variants.
            Err(Error::BadStructure("STAT unsupported axis value format"))
        }
    }
}

#[inline]
fn fixed_to_f32(raw: i32) -> f32 {
    raw as f32 / 65536.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_minimal_v11() -> Vec<u8> {
        // One axis (wght) and two format-1 axis-value tables: 400 and
        // 700, with 400 marked elidable.
        let mut b = vec![0u8; 0];
        // Header (20 bytes for v1.1)
        b.extend_from_slice(&1u16.to_be_bytes()); // major
        b.extend_from_slice(&1u16.to_be_bytes()); // minor
        b.extend_from_slice(&8u16.to_be_bytes()); // designAxisSize
        b.extend_from_slice(&1u16.to_be_bytes()); // designAxisCount
        b.extend_from_slice(&20u32.to_be_bytes()); // offsetToDesignAxes
        b.extend_from_slice(&2u16.to_be_bytes()); // axisValueCount
        b.extend_from_slice(&28u32.to_be_bytes()); // offsetToAxisValueOffsets
        b.extend_from_slice(&2u16.to_be_bytes()); // elidedFallbackNameID

        // AxisRecord @20
        b.extend_from_slice(b"wght");
        b.extend_from_slice(&256u16.to_be_bytes()); // axisNameID
        b.extend_from_slice(&0u16.to_be_bytes()); // axisOrdering

        // axisValueOffsets @28 (2 entries of u16, relative to 28)
        let array_base = b.len();
        assert_eq!(array_base, 28);
        // We'll fill the offsets after we know the table positions.
        b.extend_from_slice(&0u16.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes());

        // format-1 table for wght=400, ELIDABLE flag set
        let tbl1_off = b.len();
        b.extend_from_slice(&1u16.to_be_bytes()); // format
        b.extend_from_slice(&0u16.to_be_bytes()); // axisIndex
        b.extend_from_slice(&FLAG_ELIDABLE_AXIS_VALUE_NAME.to_be_bytes()); // flags
        b.extend_from_slice(&257u16.to_be_bytes()); // valueNameID
        b.extend_from_slice(&((400i32) << 16).to_be_bytes()); // value 400.0

        // format-1 table for wght=700
        let tbl2_off = b.len();
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes());
        b.extend_from_slice(&258u16.to_be_bytes());
        b.extend_from_slice(&((700i32) << 16).to_be_bytes());

        // Fill in the offsets array entries (relative to array_base).
        let off1 = (tbl1_off - array_base) as u16;
        let off2 = (tbl2_off - array_base) as u16;
        b[array_base..array_base + 2].copy_from_slice(&off1.to_be_bytes());
        b[array_base + 2..array_base + 4].copy_from_slice(&off2.to_be_bytes());

        b
    }

    #[test]
    fn parses_minimal_v11() {
        let bytes = build_minimal_v11();
        let t = StatTable::parse(&bytes).expect("parse");
        assert_eq!(t.major_version(), 1);
        assert_eq!(t.minor_version(), 1);
        assert_eq!(t.elided_fallback_name_id(), 2);
        assert_eq!(t.axes().len(), 1);
        assert_eq!(t.axes()[0].axis_tag, *b"wght");
        assert_eq!(t.axes()[0].axis_name_id, 256);
        assert_eq!(t.axis_values().len(), 2);
        match &t.axis_values()[0] {
            AxisValue::Format1 {
                axis_index,
                value,
                value_name_id,
                ..
            } => {
                assert_eq!(*axis_index, 0);
                assert!((*value - 400.0).abs() < 1e-6);
                assert_eq!(*value_name_id, 257);
            }
            _ => panic!("expected Format1"),
        }
        assert!(t.axis_values()[0].is_elidable());
        assert!(!t.axis_values()[1].is_elidable());
    }

    #[test]
    fn axis_values_for_tag_filter() {
        let bytes = build_minimal_v11();
        let t = StatTable::parse(&bytes).unwrap();
        let n = t.axis_values_for_tag(*b"wght").count();
        assert_eq!(n, 2);
        let n = t.axis_values_for_tag(*b"opsz").count();
        assert_eq!(n, 0);
    }

    #[test]
    fn rejects_truncated_header() {
        let bytes = [0u8; 10];
        assert!(matches!(
            StatTable::parse(&bytes),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_bad_major_version() {
        let mut b = vec![0u8; 20];
        b[0..2].copy_from_slice(&2u16.to_be_bytes()); // majorVersion = 2
        assert!(matches!(StatTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn parses_format2_range_with_sentinels() {
        // Single axis + a format-2 table with negative-infinity min and
        // positive-infinity max.
        let mut b = Vec::<u8>::new();
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&8u16.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&20u32.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&28u32.to_be_bytes());
        b.extend_from_slice(&2u16.to_be_bytes());
        // AxisRecord @20: opsz
        b.extend_from_slice(b"opsz");
        b.extend_from_slice(&100u16.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes());
        // axisValueOffsets @28 (one u16)
        let array_base = b.len();
        b.extend_from_slice(&0u16.to_be_bytes());
        // format-2 @30
        let tbl_off = b.len();
        b.extend_from_slice(&2u16.to_be_bytes()); // format
        b.extend_from_slice(&0u16.to_be_bytes()); // axisIndex
        b.extend_from_slice(&0u16.to_be_bytes()); // flags
        b.extend_from_slice(&200u16.to_be_bytes()); // valueNameID
        b.extend_from_slice(&((12i32) << 16).to_be_bytes()); // nominal 12.0
        b.extend_from_slice(&RANGE_MIN_NEG_INFINITY.to_be_bytes()); // -∞
        b.extend_from_slice(&RANGE_MAX_POS_INFINITY.to_be_bytes()); // +∞
        let off = (tbl_off - array_base) as u16;
        b[array_base..array_base + 2].copy_from_slice(&off.to_be_bytes());

        let t = StatTable::parse(&b).expect("parse");
        match &t.axis_values()[0] {
            AxisValue::Format2 {
                axis_index,
                nominal_value,
                raw_range_min,
                raw_range_max,
                ..
            } => {
                assert_eq!(*axis_index, 0);
                assert!((*nominal_value - 12.0).abs() < 1e-6);
                assert_eq!(*raw_range_min, RANGE_MIN_NEG_INFINITY);
                assert_eq!(*raw_range_max, RANGE_MAX_POS_INFINITY);
            }
            _ => panic!("expected Format2"),
        }
    }

    #[test]
    fn parses_format3_style_linked_pair() {
        // wght=400 linked to wght=700 ("Regular" → "Bold").
        let mut b = Vec::<u8>::new();
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&8u16.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&20u32.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&28u32.to_be_bytes());
        b.extend_from_slice(&2u16.to_be_bytes());
        b.extend_from_slice(b"wght");
        b.extend_from_slice(&256u16.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes());
        let array_base = b.len();
        b.extend_from_slice(&0u16.to_be_bytes());
        let tbl_off = b.len();
        b.extend_from_slice(&3u16.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes());
        b.extend_from_slice(&FLAG_ELIDABLE_AXIS_VALUE_NAME.to_be_bytes());
        b.extend_from_slice(&257u16.to_be_bytes());
        b.extend_from_slice(&((400i32) << 16).to_be_bytes());
        b.extend_from_slice(&((700i32) << 16).to_be_bytes());
        let off = (tbl_off - array_base) as u16;
        b[array_base..array_base + 2].copy_from_slice(&off.to_be_bytes());

        let t = StatTable::parse(&b).unwrap();
        match &t.axis_values()[0] {
            AxisValue::Format3 {
                value,
                linked_value,
                ..
            } => {
                assert!((*value - 400.0).abs() < 1e-6);
                assert!((*linked_value - 700.0).abs() < 1e-6);
            }
            _ => panic!("expected Format3"),
        }
    }

    #[test]
    fn parses_format4_multi_axis_combo() {
        // Two axes, one format-4 record combining them.
        let mut b = Vec::<u8>::new();
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&2u16.to_be_bytes()); // minor = 2 for fmt4
        b.extend_from_slice(&8u16.to_be_bytes());
        b.extend_from_slice(&2u16.to_be_bytes()); // 2 axes
        b.extend_from_slice(&20u32.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&36u32.to_be_bytes()); // 20 + 2*8 = 36
        b.extend_from_slice(&2u16.to_be_bytes());
        // Axis records
        b.extend_from_slice(b"wght");
        b.extend_from_slice(&256u16.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes());
        b.extend_from_slice(b"wdth");
        b.extend_from_slice(&257u16.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        let array_base = b.len();
        b.extend_from_slice(&0u16.to_be_bytes());
        let tbl_off = b.len();
        b.extend_from_slice(&4u16.to_be_bytes()); // format
        b.extend_from_slice(&2u16.to_be_bytes()); // axisCount
        b.extend_from_slice(&0u16.to_be_bytes()); // flags
        b.extend_from_slice(&300u16.to_be_bytes()); // valueNameID
                                                    // axisValue[0]: wght(0) = 700
        b.extend_from_slice(&0u16.to_be_bytes());
        b.extend_from_slice(&((700i32) << 16).to_be_bytes());
        // axisValue[1]: wdth(1) = 75
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&((75i32) << 16).to_be_bytes());
        let off = (tbl_off - array_base) as u16;
        b[array_base..array_base + 2].copy_from_slice(&off.to_be_bytes());

        let t = StatTable::parse(&b).unwrap();
        match &t.axis_values()[0] {
            AxisValue::Format4 {
                axis_values,
                value_name_id,
                ..
            } => {
                assert_eq!(*value_name_id, 300);
                assert_eq!(axis_values.len(), 2);
                assert_eq!(axis_values[0].0, 0);
                assert!((axis_values[0].1 - 700.0).abs() < 1e-6);
                assert_eq!(axis_values[1].0, 1);
                assert!((axis_values[1].1 - 75.0).abs() < 1e-6);
            }
            _ => panic!("expected Format4"),
        }
    }

    #[test]
    fn rejects_format4_duplicate_axis_index() {
        let mut b = Vec::<u8>::new();
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&2u16.to_be_bytes());
        b.extend_from_slice(&8u16.to_be_bytes());
        b.extend_from_slice(&2u16.to_be_bytes());
        b.extend_from_slice(&20u32.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&36u32.to_be_bytes());
        b.extend_from_slice(&2u16.to_be_bytes());
        b.extend_from_slice(b"wght");
        b.extend_from_slice(&256u16.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes());
        b.extend_from_slice(b"wdth");
        b.extend_from_slice(&257u16.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        let array_base = b.len();
        b.extend_from_slice(&0u16.to_be_bytes());
        let tbl_off = b.len();
        b.extend_from_slice(&4u16.to_be_bytes());
        b.extend_from_slice(&2u16.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes());
        b.extend_from_slice(&300u16.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes());
        b.extend_from_slice(&((700i32) << 16).to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes()); // duplicate axisIndex
        b.extend_from_slice(&((400i32) << 16).to_be_bytes());
        let off = (tbl_off - array_base) as u16;
        b[array_base..array_base + 2].copy_from_slice(&off.to_be_bytes());

        assert!(matches!(StatTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_unknown_format() {
        // Single axis + one axis value table with format = 9 (unknown).
        let mut b = Vec::<u8>::new();
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&8u16.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&20u32.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&28u32.to_be_bytes());
        b.extend_from_slice(&2u16.to_be_bytes());
        b.extend_from_slice(b"wght");
        b.extend_from_slice(&256u16.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes());
        let array_base = b.len();
        b.extend_from_slice(&0u16.to_be_bytes());
        let tbl_off = b.len();
        b.extend_from_slice(&9u16.to_be_bytes()); // unknown format
        b.extend_from_slice(&[0u8; 16]);
        let off = (tbl_off - array_base) as u16;
        b[array_base..array_base + 2].copy_from_slice(&off.to_be_bytes());

        assert!(matches!(StatTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_axis_index_out_of_range() {
        // One axis declared, but format-1 entry says axisIndex=5.
        let mut b = Vec::<u8>::new();
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&8u16.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&20u32.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&28u32.to_be_bytes());
        b.extend_from_slice(&2u16.to_be_bytes());
        b.extend_from_slice(b"wght");
        b.extend_from_slice(&256u16.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes());
        let array_base = b.len();
        b.extend_from_slice(&0u16.to_be_bytes());
        let tbl_off = b.len();
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&5u16.to_be_bytes()); // out-of-range axisIndex
        b.extend_from_slice(&0u16.to_be_bytes());
        b.extend_from_slice(&100u16.to_be_bytes());
        b.extend_from_slice(&((400i32) << 16).to_be_bytes());
        let off = (tbl_off - array_base) as u16;
        b[array_base..array_base + 2].copy_from_slice(&off.to_be_bytes());

        assert!(matches!(StatTable::parse(&b), Err(Error::BadStructure(_))));
    }
}
