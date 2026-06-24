//! `MATH` — the mathematical typesetting table (ISO/IEC 14496-22:2019
//! §6.3.6).
//!
//! The MATH table carries the font-specific parameters a math-layout
//! engine needs to position fractions, radicals, scripts, accents, large
//! operators, and stretchy/assembled glyphs. It is **not** a layout
//! algorithm — it is the data those algorithms consume.
//!
//! ## Structure (§6.3.6.2)
//!
//! ```text
//!   MATH header  ──> MathConstants   (§6.3.6.2.3)  — ~57 font-wide values
//!                ──> MathGlyphInfo   (§6.3.6.2.4)  — per-glyph data
//!                │     ├─ MathItalicsCorrectionInfo
//!                │     ├─ MathTopAccentAttachment
//!                │     ├─ ExtendedShapeCoverage
//!                │     └─ MathKernInfo (four per-corner MathKern tables)
//!                ──> MathVariants    (§6.3.6.2.10) — stretchy variants +
//!                      glyph assemblies for growing parens/radicals/etc.
//! ```
//!
//! Many values are [`MathValueRecord`]s: a design-unit `int16` plus an
//! optional device-table offset (device corrections are ignored here —
//! we expose the design-unit value, which is what a high-resolution
//! engine uses). Coverage tables reuse the common-layout Coverage parser.
//!
//! This module decodes the whole table structurally and exposes typed
//! accessors. Each accessor borrows the parent table slice, so the
//! parsed [`MathTable`] is a cheap set of validated offsets.

use crate::parser::{read_i16, read_u16};
use crate::tables::gdef::coverage_lookup;
use crate::Error;

/// The 4-byte table tag.
pub const MATH_TABLE_TAG: [u8; 4] = *b"MATH";

/// A `MathValueRecord` (§6.3.6.2.1): a design-unit value plus an optional
/// device-table offset (the device correction is not applied here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MathValueRecord {
    /// The X or Y value in font design units.
    pub value: i16,
    /// Offset to a device table from the start of the *parent* table, or
    /// 0 for none. Exposed for completeness; corrections are not applied.
    pub device_offset: u16,
}

impl MathValueRecord {
    const LEN: usize = 4;

    fn read(bytes: &[u8], at: usize) -> Result<Self, Error> {
        Ok(Self {
            value: read_i16(bytes, at)?,
            device_offset: read_u16(bytes, at + 2)?,
        })
    }
}

/// Parsed `MATH` table — a set of validated sub-table offsets into the
/// borrowed table slice.
#[derive(Debug, Clone)]
pub struct MathTable<'a> {
    data: &'a [u8],
    constants_off: usize,
    glyph_info_off: usize,
    variants_off: usize,
}

impl<'a> MathTable<'a> {
    /// Parse the MATH header (§6.3.6.2.2) and validate its three offsets.
    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
        // majorVersion(=1) minorVersion(=0) + three Offset16.
        let major = read_u16(data, 0)?;
        if major != 1 {
            return Err(Error::BadStructure("MATH major version not 1"));
        }
        let constants_off = read_u16(data, 4)? as usize;
        let glyph_info_off = read_u16(data, 6)? as usize;
        let variants_off = read_u16(data, 8)? as usize;
        // Offsets are from the start of the MATH table; a zero offset
        // means the sub-table is absent. We validate non-zero offsets as
        // in-bounds.
        for &o in &[constants_off, glyph_info_off, variants_off] {
            if o != 0 && o >= data.len() {
                return Err(Error::BadStructure("MATH sub-table offset OOB"));
            }
        }
        Ok(Self {
            data,
            constants_off,
            glyph_info_off,
            variants_off,
        })
    }

    /// Borrow the MathConstants accessor, when the table publishes one.
    pub fn constants(&self) -> Option<MathConstants<'a>> {
        if self.constants_off == 0 {
            return None;
        }
        Some(MathConstants {
            data: self.data,
            base: self.constants_off,
        })
    }

    /// Borrow the MathGlyphInfo accessor, when present.
    pub fn glyph_info(&self) -> Option<MathGlyphInfo<'a>> {
        if self.glyph_info_off == 0 {
            return None;
        }
        let base = self.glyph_info_off;
        Some(MathGlyphInfo {
            data: self.data,
            base,
        })
    }

    /// Borrow the MathVariants accessor, when present.
    pub fn variants(&self) -> Option<MathVariants<'a>> {
        if self.variants_off == 0 {
            return None;
        }
        Some(MathVariants {
            data: self.data,
            base: self.variants_off,
        })
    }
}

// --- MathConstants (§6.3.6.2.3) --------------------------------------

/// Field layout of the MathConstants table. The leading two `int16`
/// fields and the next two `uint16` fields are plain values; every
/// remaining field is a 4-byte `MathValueRecord`. We encode each field's
/// byte offset rather than copying ~57 values eagerly.
///
/// Offsets (in bytes from the start of MathConstants):
///
/// ```text
///   +0   int16  scriptPercentScaleDown
///   +2   int16  scriptScriptPercentScaleDown
///   +4   uint16 delimitedSubFormulaMinHeight
///   +6   uint16 displayOperatorMinHeight
///   +8   MathValueRecord  mathLeading
///   ...  (the remaining 51 MathValueRecord fields, 4 bytes each)
/// ```
#[derive(Debug, Clone, Copy)]
pub struct MathConstants<'a> {
    data: &'a [u8],
    base: usize,
}

/// Indices into the MathConstants `MathValueRecord` array (the records
/// that follow the four scalar fields, in spec order §6.3.6.2.3).
///
/// Record `i` lives at MathConstants byte offset `8 + i * 4`.
pub mod constant {
    /// Position of each MathValueRecord field within the record array,
    /// in the spec's declaration order.
    pub const MATH_LEADING: usize = 0;
    pub const AXIS_HEIGHT: usize = 1;
    pub const ACCENT_BASE_HEIGHT: usize = 2;
    pub const FLATTENED_ACCENT_BASE_HEIGHT: usize = 3;
    pub const SUBSCRIPT_SHIFT_DOWN: usize = 4;
    pub const SUBSCRIPT_TOP_MAX: usize = 5;
    pub const SUBSCRIPT_BASELINE_DROP_MIN: usize = 6;
    pub const SUPERSCRIPT_SHIFT_UP: usize = 7;
    pub const SUPERSCRIPT_SHIFT_UP_CRAMPED: usize = 8;
    pub const SUPERSCRIPT_BOTTOM_MIN: usize = 9;
    pub const SUPERSCRIPT_BASELINE_DROP_MAX: usize = 10;
    pub const SUB_SUPERSCRIPT_GAP_MIN: usize = 11;
    pub const SUPERSCRIPT_BOTTOM_MAX_WITH_SUBSCRIPT: usize = 12;
    pub const SPACE_AFTER_SCRIPT: usize = 13;
    pub const UPPER_LIMIT_GAP_MIN: usize = 14;
    pub const UPPER_LIMIT_BASELINE_RISE_MIN: usize = 15;
    pub const LOWER_LIMIT_GAP_MIN: usize = 16;
    pub const LOWER_LIMIT_BASELINE_DROP_MIN: usize = 17;
    pub const STACK_TOP_SHIFT_UP: usize = 18;
    pub const STACK_TOP_DISPLAY_STYLE_SHIFT_UP: usize = 19;
    pub const STACK_BOTTOM_SHIFT_DOWN: usize = 20;
    pub const STACK_BOTTOM_DISPLAY_STYLE_SHIFT_DOWN: usize = 21;
    pub const STACK_GAP_MIN: usize = 22;
    pub const STACK_DISPLAY_STYLE_GAP_MIN: usize = 23;
    pub const STRETCH_STACK_TOP_SHIFT_UP: usize = 24;
    pub const STRETCH_STACK_BOTTOM_SHIFT_DOWN: usize = 25;
    pub const STRETCH_STACK_GAP_ABOVE_MIN: usize = 26;
    pub const STRETCH_STACK_GAP_BELOW_MIN: usize = 27;
    pub const FRACTION_NUMERATOR_SHIFT_UP: usize = 28;
    pub const FRACTION_NUMERATOR_DISPLAY_STYLE_SHIFT_UP: usize = 29;
    pub const FRACTION_DENOMINATOR_SHIFT_DOWN: usize = 30;
    pub const FRACTION_DENOMINATOR_DISPLAY_STYLE_SHIFT_DOWN: usize = 31;
    pub const FRACTION_NUMERATOR_GAP_MIN: usize = 32;
    pub const FRACTION_NUM_DISPLAY_STYLE_GAP_MIN: usize = 33;
    pub const FRACTION_RULE_THICKNESS: usize = 34;
    pub const FRACTION_DENOMINATOR_GAP_MIN: usize = 35;
    pub const FRACTION_DENOM_DISPLAY_STYLE_GAP_MIN: usize = 36;
    pub const SKEWED_FRACTION_HORIZONTAL_GAP: usize = 37;
    pub const SKEWED_FRACTION_VERTICAL_GAP: usize = 38;
    pub const OVERBAR_VERTICAL_GAP: usize = 39;
    pub const OVERBAR_RULE_THICKNESS: usize = 40;
    pub const OVERBAR_EXTRA_ASCENDER: usize = 41;
    pub const UNDERBAR_VERTICAL_GAP: usize = 42;
    pub const UNDERBAR_RULE_THICKNESS: usize = 43;
    pub const UNDERBAR_EXTRA_DESCENDER: usize = 44;
    pub const RADICAL_VERTICAL_GAP: usize = 45;
    pub const RADICAL_DISPLAY_STYLE_VERTICAL_GAP: usize = 46;
    pub const RADICAL_RULE_THICKNESS: usize = 47;
    pub const RADICAL_EXTRA_ASCENDER: usize = 48;
    pub const RADICAL_KERN_BEFORE_DEGREE: usize = 49;
    pub const RADICAL_KERN_AFTER_DEGREE: usize = 50;
}

impl<'a> MathConstants<'a> {
    /// `scriptPercentScaleDown` — percentage to scale level-1 scripts.
    pub fn script_percent_scale_down(&self) -> i16 {
        read_i16(self.data, self.base).unwrap_or(0)
    }

    /// `scriptScriptPercentScaleDown` — percentage to scale level-2
    /// scripts.
    pub fn script_script_percent_scale_down(&self) -> i16 {
        read_i16(self.data, self.base + 2).unwrap_or(0)
    }

    /// `delimitedSubFormulaMinHeight`.
    pub fn delimited_sub_formula_min_height(&self) -> u16 {
        read_u16(self.data, self.base + 4).unwrap_or(0)
    }

    /// `displayOperatorMinHeight`.
    pub fn display_operator_min_height(&self) -> u16 {
        read_u16(self.data, self.base + 6).unwrap_or(0)
    }

    /// `radicalDegreeBottomRaisePercent` — the trailing `int16` field
    /// that follows the MathValueRecord array (record 50 is the last
    /// MathValueRecord; this `int16` sits right after it).
    pub fn radical_degree_bottom_raise_percent(&self) -> i16 {
        let at = self.base + 8 + 51 * MathValueRecord::LEN;
        read_i16(self.data, at).unwrap_or(0)
    }

    /// One of the MathValueRecord constants (`constant::*`), or `None`
    /// when the index is out of range or the record is truncated.
    pub fn value(&self, index: usize) -> Option<MathValueRecord> {
        if index > constant::RADICAL_KERN_AFTER_DEGREE {
            return None;
        }
        let at = self.base + 8 + index * MathValueRecord::LEN;
        MathValueRecord::read(self.data, at).ok()
    }

    /// Convenience: the design-unit value of MathValueRecord `index`, or
    /// `0` when absent.
    pub fn value_i16(&self, index: usize) -> i16 {
        self.value(index).map(|r| r.value).unwrap_or(0)
    }
}

// --- MathGlyphInfo (§6.3.6.2.4) --------------------------------------

/// Per-glyph math positioning data.
#[derive(Debug, Clone, Copy)]
pub struct MathGlyphInfo<'a> {
    data: &'a [u8],
    base: usize,
}

impl<'a> MathGlyphInfo<'a> {
    fn sub_off(&self, idx: usize) -> Option<usize> {
        let o = read_u16(self.data, self.base + idx * 2).ok()? as usize;
        if o == 0 {
            None
        } else {
            Some(self.base + o)
        }
    }

    /// Italics-correction value for `gid` (§6.3.6.2.5), or `None` when the
    /// glyph isn't covered (treated as zero by layout).
    pub fn italics_correction(&self, gid: u16) -> Option<i16> {
        let base = self.sub_off(0)?; // mathItalicsCorrectionInfoOffset
        let cov_off = read_u16(self.data, base).ok()? as usize;
        if cov_off == 0 {
            return None;
        }
        let idx = coverage_lookup(self.data.get(base + cov_off..)?, gid)? as usize;
        let count = read_u16(self.data, base + 2).ok()? as usize;
        if idx >= count {
            return None;
        }
        let at = base + 4 + idx * MathValueRecord::LEN;
        Some(MathValueRecord::read(self.data, at).ok()?.value)
    }

    /// Top-accent horizontal attachment point for `gid` (§6.3.6.2.6), or
    /// `None` when uncovered (use the glyph's geometric centre instead).
    pub fn top_accent_attachment(&self, gid: u16) -> Option<i16> {
        let base = self.sub_off(1)?; // mathTopAccentAttachmentOffset
        let cov_off = read_u16(self.data, base).ok()? as usize;
        if cov_off == 0 {
            return None;
        }
        let idx = coverage_lookup(self.data.get(base + cov_off..)?, gid)? as usize;
        let count = read_u16(self.data, base + 2).ok()? as usize;
        if idx >= count {
            return None;
        }
        let at = base + 4 + idx * MathValueRecord::LEN;
        Some(MathValueRecord::read(self.data, at).ok()?.value)
    }

    /// Whether `gid` is flagged as an extended shape (§6.3.6.2.7).
    pub fn is_extended_shape(&self, gid: u16) -> bool {
        // extendedShapeCoverageOffset is the third Offset16.
        match self.sub_off(2) {
            Some(cov) => self
                .data
                .get(cov..)
                .and_then(|b| coverage_lookup(b, gid))
                .is_some(),
            None => false,
        }
    }

    /// Math-kern value for `gid` at one corner and a given correction
    /// height, in design units (§6.3.6.2.8/.9). `corner` selects the
    /// per-corner MathKern table; absent corners kern by zero.
    pub fn math_kern(&self, gid: u16, corner: MathKernCorner, height: i16) -> Option<i16> {
        let base = self.sub_off(3)?; // mathKernInfoOffset
        let cov_off = read_u16(self.data, base).ok()? as usize;
        if cov_off == 0 {
            return None;
        }
        let idx = coverage_lookup(self.data.get(base + cov_off..)?, gid)? as usize;
        let count = read_u16(self.data, base + 2).ok()? as usize;
        if idx >= count {
            return None;
        }
        // MathKernInfoRecord: four Offset16 per covered glyph.
        let rec_at = base + 4 + idx * 8;
        let kern_off = read_u16(self.data, rec_at + corner as usize * 2).ok()? as usize;
        if kern_off == 0 {
            return None;
        }
        math_kern_value(self.data, base + kern_off, height)
    }
}

/// The four corners a `MathKern` table can apply to (§6.3.6.2.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathKernCorner {
    TopRight = 0,
    TopLeft = 1,
    BottomRight = 2,
    BottomLeft = 3,
}

/// Look up a MathKern value (§6.3.6.2.9) at `height` from a MathKern
/// table located at `base`. `heightCount` correction heights partition
/// the vertical extent; `heightCount + 1` kern values cover the ranges.
fn math_kern_value(data: &[u8], base: usize, height: i16) -> Option<i16> {
    let n = read_u16(data, base).ok()? as usize;
    // correctionHeight[n] then kernValue[n+1], each a MathValueRecord.
    let heights_at = base + 2;
    let kerns_at = heights_at + n * MathValueRecord::LEN;
    // Find the first correction height strictly greater than `height`;
    // the index of that boundary selects the kern range.
    let mut sel = n; // default: past the last boundary -> last kern.
    for i in 0..n {
        let h = MathValueRecord::read(data, heights_at + i * MathValueRecord::LEN)
            .ok()?
            .value;
        if height < h {
            sel = i;
            break;
        }
    }
    let at = kerns_at + sel * MathValueRecord::LEN;
    Some(MathValueRecord::read(data, at).ok()?.value)
}

// --- MathVariants (§6.3.6.2.10) --------------------------------------

/// Growth direction for stretchy / assembled glyph variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrowDirection {
    Vertical,
    Horizontal,
}

/// One ready-made stretchy variant (§6.3.6.2.11 MathGlyphVariantRecord).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphVariant {
    /// Glyph ID of the variant.
    pub glyph: u16,
    /// Advance (width or height) of the variant in the growth direction.
    pub advance: u16,
}

/// One part of an assembled stretchy glyph (§6.3.6.2.12 GlyphPartRecord).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphPart {
    pub glyph: u16,
    pub start_connector_length: u16,
    pub end_connector_length: u16,
    pub full_advance: u16,
    /// Part qualifiers; bit 0 (`0x0001`) marks an extender part.
    pub part_flags: u16,
}

impl GlyphPart {
    /// Whether this part is an extender (repeatable/skippable, §6.3.6.2.12).
    pub fn is_extender(&self) -> bool {
        self.part_flags & 0x0001 != 0
    }
}

/// Stretchy / assembled glyph variants.
#[derive(Debug, Clone, Copy)]
pub struct MathVariants<'a> {
    data: &'a [u8],
    base: usize,
}

impl<'a> MathVariants<'a> {
    /// Minimum overlap of connecting glyph parts during assembly.
    pub fn min_connector_overlap(&self) -> u16 {
        read_u16(self.data, self.base).unwrap_or(0)
    }

    fn coverage_off(&self, dir: GrowDirection) -> Option<usize> {
        // vertGlyphCoverageOffset @ +2, horizGlyphCoverageOffset @ +4.
        let field = match dir {
            GrowDirection::Vertical => 2,
            GrowDirection::Horizontal => 4,
        };
        let o = read_u16(self.data, self.base + field).ok()? as usize;
        if o == 0 {
            None
        } else {
            Some(self.base + o)
        }
    }

    fn glyph_count(&self, dir: GrowDirection) -> u16 {
        // vertGlyphCount @ +6, horizGlyphCount @ +8.
        let field = match dir {
            GrowDirection::Vertical => 6,
            GrowDirection::Horizontal => 8,
        };
        read_u16(self.data, self.base + field).unwrap_or(0)
    }

    /// Offset (from the MathVariants table) to the MathGlyphConstruction
    /// table for `gid` growing in `dir`, or `None` when `gid` has no
    /// construction in that direction.
    fn construction_off(&self, gid: u16, dir: GrowDirection) -> Option<usize> {
        let cov = self.coverage_off(dir)?;
        let idx = coverage_lookup(self.data.get(cov..)?, gid)? as usize;
        let count = self.glyph_count(dir) as usize;
        if idx >= count {
            return None;
        }
        // Construction offset arrays: vert @ +10, horiz @ +10 + vertCount*2.
        let vert_count = self.glyph_count(GrowDirection::Vertical) as usize;
        let array_base = match dir {
            GrowDirection::Vertical => self.base + 10,
            GrowDirection::Horizontal => self.base + 10 + vert_count * 2,
        };
        let o = read_u16(self.data, array_base + idx * 2).ok()? as usize;
        if o == 0 {
            None
        } else {
            Some(self.base + o)
        }
    }

    /// Ready-made stretchy variants for `gid` growing in `dir`, ordered
    /// by increasing size (§6.3.6.2.11).
    pub fn variants(&self, gid: u16, dir: GrowDirection) -> Vec<GlyphVariant> {
        let mut out = Vec::new();
        let Some(ctor) = self.construction_off(gid, dir) else {
            return out;
        };
        // MathGlyphConstruction: Offset16 glyphAssemblyOffset, uint16
        // variantCount, then variantCount MathGlyphVariantRecords.
        let Ok(count) = read_u16(self.data, ctor + 2) else {
            return out;
        };
        for i in 0..count as usize {
            let at = ctor + 4 + i * 4;
            let (Ok(glyph), Ok(advance)) = (read_u16(self.data, at), read_u16(self.data, at + 2))
            else {
                break;
            };
            out.push(GlyphVariant { glyph, advance });
        }
        out
    }

    /// The glyph-assembly parts for `gid` growing in `dir`, when the font
    /// supplies a general assembly mechanism (§6.3.6.2.12). Returns the
    /// `(italics_correction, parts)` pair, or `None` when no assembly is
    /// defined.
    pub fn assembly(&self, gid: u16, dir: GrowDirection) -> Option<(i16, Vec<GlyphPart>)> {
        let ctor = self.construction_off(gid, dir)?;
        let asm_off = read_u16(self.data, ctor).ok()? as usize;
        if asm_off == 0 {
            return None;
        }
        let asm = ctor + asm_off;
        // GlyphAssembly: MathValueRecord italicsCorrection, uint16
        // partCount, GlyphPartRecord[partCount].
        let italics = MathValueRecord::read(self.data, asm).ok()?.value;
        let part_count = read_u16(self.data, asm + MathValueRecord::LEN).ok()? as usize;
        let parts_at = asm + MathValueRecord::LEN + 2;
        let mut parts = Vec::with_capacity(part_count);
        for i in 0..part_count {
            let at = parts_at + i * 10; // GlyphPartRecord is 5 * u16.
            parts.push(GlyphPart {
                glyph: read_u16(self.data, at).ok()?,
                start_connector_length: read_u16(self.data, at + 2).ok()?,
                end_connector_length: read_u16(self.data, at + 4).ok()?,
                full_advance: read_u16(self.data, at + 6).ok()?,
                part_flags: read_u16(self.data, at + 8).ok()?,
            });
        }
        Some((italics, parts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a MATH table with a MathConstants table only, exercising the
    /// scalar fields + a couple of MathValueRecords + the trailing int16.
    fn build_math_constants_only() -> Vec<u8> {
        let mut data = Vec::new();
        // Header: major=1 minor=0, constantsOff, glyphInfoOff=0, variantsOff=0
        data.extend_from_slice(&1u16.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
        let const_off_pos = data.len();
        data.extend_from_slice(&0u16.to_be_bytes()); // patched
        data.extend_from_slice(&0u16.to_be_bytes()); // glyphInfo
        data.extend_from_slice(&0u16.to_be_bytes()); // variants

        let const_off = data.len();
        // scalar fields
        data.extend_from_slice(&80i16.to_be_bytes()); // scriptPercentScaleDown
        data.extend_from_slice(&60i16.to_be_bytes()); // scriptScriptPercentScaleDown
        data.extend_from_slice(&300u16.to_be_bytes()); // delimitedSubFormulaMinHeight
        data.extend_from_slice(&1500u16.to_be_bytes()); // displayOperatorMinHeight
                                                        // 51 MathValueRecords; set axisHeight (index 1) = 250, rest 0.
        for i in 0..51 {
            let v: i16 = if i == constant::AXIS_HEIGHT as i32 as usize {
                250
            } else if i == constant::FRACTION_RULE_THICKNESS {
                40
            } else {
                0
            };
            data.extend_from_slice(&v.to_be_bytes());
            data.extend_from_slice(&0u16.to_be_bytes()); // device offset
        }
        // trailing int16 radicalDegreeBottomRaisePercent
        data.extend_from_slice(&60i16.to_be_bytes());

        // patch the constants offset
        let off = const_off as u16;
        data[const_off_pos..const_off_pos + 2].copy_from_slice(&off.to_be_bytes());
        data
    }

    #[test]
    fn math_constants_scalars_and_records() {
        let data = build_math_constants_only();
        let m = MathTable::parse(&data).expect("parse");
        let c = m.constants().expect("constants");
        assert_eq!(c.script_percent_scale_down(), 80);
        assert_eq!(c.script_script_percent_scale_down(), 60);
        assert_eq!(c.delimited_sub_formula_min_height(), 300);
        assert_eq!(c.display_operator_min_height(), 1500);
        assert_eq!(c.value_i16(constant::AXIS_HEIGHT), 250);
        assert_eq!(c.value_i16(constant::FRACTION_RULE_THICKNESS), 40);
        assert_eq!(c.value_i16(constant::MATH_LEADING), 0);
        assert_eq!(c.radical_degree_bottom_raise_percent(), 60);
        // Out-of-range record index.
        assert!(c.value(100).is_none());
        assert!(m.glyph_info().is_none());
        assert!(m.variants().is_none());
    }

    #[test]
    fn rejects_wrong_version() {
        let mut data = vec![0u8; 10];
        data[1] = 2; // major = 2
        assert!(MathTable::parse(&data).is_err());
    }

    /// Build a MATH table with a MathVariants table carrying one vertical
    /// stretchy glyph (gid 5) with two variants and a 3-part assembly.
    fn build_math_variants() -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&1u16.to_be_bytes()); // major
        data.extend_from_slice(&0u16.to_be_bytes()); // minor
        data.extend_from_slice(&0u16.to_be_bytes()); // constants off = none
        data.extend_from_slice(&0u16.to_be_bytes()); // glyphInfo = none
        let var_off_pos = data.len();
        data.extend_from_slice(&0u16.to_be_bytes()); // variants (patched)

        let var_base = data.len();
        // MathVariants header.
        data.extend_from_slice(&20u16.to_be_bytes()); // minConnectorOverlap
        let vcov_pos = data.len();
        data.extend_from_slice(&0u16.to_be_bytes()); // vertGlyphCoverageOffset (patched)
        data.extend_from_slice(&0u16.to_be_bytes()); // horizGlyphCoverageOffset = none
        data.extend_from_slice(&1u16.to_be_bytes()); // vertGlyphCount
        data.extend_from_slice(&0u16.to_be_bytes()); // horizGlyphCount
        let vctor_pos = data.len();
        data.extend_from_slice(&0u16.to_be_bytes()); // vertGlyphConstructionOffsets[0] (patched)

        // Coverage (format 1, one glyph = 5).
        let cov_at = data.len();
        data.extend_from_slice(&1u16.to_be_bytes()); // format
        data.extend_from_slice(&1u16.to_be_bytes()); // glyphCount
        data.extend_from_slice(&5u16.to_be_bytes()); // glyph 5
        data[vcov_pos..vcov_pos + 2].copy_from_slice(&((cov_at - var_base) as u16).to_be_bytes());

        // MathGlyphConstruction for gid 5.
        let ctor_at = data.len();
        let asm_off_pos = data.len();
        data.extend_from_slice(&0u16.to_be_bytes()); // glyphAssemblyOffset (patched)
        data.extend_from_slice(&2u16.to_be_bytes()); // variantCount
                                                     // variant records: (glyph, advance)
        data.extend_from_slice(&10u16.to_be_bytes());
        data.extend_from_slice(&1000u16.to_be_bytes());
        data.extend_from_slice(&11u16.to_be_bytes());
        data.extend_from_slice(&2000u16.to_be_bytes());
        data[vctor_pos..vctor_pos + 2]
            .copy_from_slice(&((ctor_at - var_base) as u16).to_be_bytes());

        // GlyphAssembly: italics=0, partCount=2, two parts (one extender).
        let asm_at = data.len();
        data.extend_from_slice(&0i16.to_be_bytes()); // italics value
        data.extend_from_slice(&0u16.to_be_bytes()); // italics device
        data.extend_from_slice(&2u16.to_be_bytes()); // partCount
                                                     // part 0: top, not extender
        data.extend_from_slice(&20u16.to_be_bytes()); // glyph
        data.extend_from_slice(&0u16.to_be_bytes()); // startConn
        data.extend_from_slice(&50u16.to_be_bytes()); // endConn
        data.extend_from_slice(&300u16.to_be_bytes()); // fullAdvance
        data.extend_from_slice(&0u16.to_be_bytes()); // flags
                                                     // part 1: extender
        data.extend_from_slice(&21u16.to_be_bytes());
        data.extend_from_slice(&50u16.to_be_bytes());
        data.extend_from_slice(&50u16.to_be_bytes());
        data.extend_from_slice(&200u16.to_be_bytes());
        data.extend_from_slice(&1u16.to_be_bytes()); // extender flag
        data[asm_off_pos..asm_off_pos + 2]
            .copy_from_slice(&((asm_at - ctor_at) as u16).to_be_bytes());

        data[var_off_pos..var_off_pos + 2].copy_from_slice(&(var_base as u16).to_be_bytes());
        data
    }

    #[test]
    fn math_variants_and_assembly() {
        let data = build_math_variants();
        let m = MathTable::parse(&data).expect("parse");
        let v = m.variants().expect("variants");
        assert_eq!(v.min_connector_overlap(), 20);

        let vars = v.variants(5, GrowDirection::Vertical);
        assert_eq!(vars.len(), 2);
        assert_eq!(
            vars[0],
            GlyphVariant {
                glyph: 10,
                advance: 1000
            }
        );
        assert_eq!(
            vars[1],
            GlyphVariant {
                glyph: 11,
                advance: 2000
            }
        );
        // Uncovered glyph -> no variants.
        assert!(v.variants(99, GrowDirection::Vertical).is_empty());
        // No horizontal coverage.
        assert!(v.variants(5, GrowDirection::Horizontal).is_empty());

        let (italics, parts) = v.assembly(5, GrowDirection::Vertical).expect("assembly");
        assert_eq!(italics, 0);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].glyph, 20);
        assert!(!parts[0].is_extender());
        assert_eq!(parts[1].glyph, 21);
        assert!(parts[1].is_extender());
        assert_eq!(parts[1].full_advance, 200);
    }
}
