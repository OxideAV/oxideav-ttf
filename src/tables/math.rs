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
//! optional device / VariationIndex offset (§6.3.6.2.1). The plain
//! accessors expose the design-unit value; the `*_resolved` accessors fold
//! in the variable-font delta at a given instance — a VariationIndex
//! offset is evaluated against the GDEF `ItemVariationStore`, while a
//! classic ppem-indexed Device table (a render-time concern) contributes
//! no font-unit adjustment. Coverage tables reuse the common-layout
//! Coverage parser.
//!
//! This module decodes the whole table structurally and exposes typed
//! accessors. Each accessor borrows the parent table slice, so the
//! parsed [`MathTable`] is a cheap set of validated offsets.

use crate::parser::{read_i16, read_u16};
use crate::tables::device::resolve_device_delta;
use crate::tables::gdef::coverage_lookup;
use crate::tables::mvar::ItemVariationStore;
use crate::Error;

/// The 4-byte table tag.
pub const MATH_TABLE_TAG: [u8; 4] = *b"MATH";

/// A `MathValueRecord` (§6.3.6.2.1): a design-unit value plus an optional
/// device / VariationIndex table offset.
///
/// Per §6.3.6.2.1 the `deviceTableOffset` is measured **from the beginning
/// of the parent table** that contains the record (the MathConstants
/// table, a per-glyph value sub-table, a MathKern table, or a
/// GlyphAssembly table — never the MATH-table root). In a variable font
/// the referenced table is a VariationIndex table (§6.2, `deltaFormat`
/// `0x8000`) whose `(outer, inner)` delta-set index is evaluated against
/// the font-wide GDEF `ItemVariationStore`; in a non-variable font it is a
/// classic ppem-indexed Device table whose pixel correction is a
/// render-time concern and contributes no font-unit adjustment here.
///
/// The plain `value` field is always the unmodified design-unit value; use
/// [`MathValueRecord::resolved_value`] to fold in the variable-font delta
/// at a given instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MathValueRecord {
    /// The X or Y value in font design units.
    pub value: i16,
    /// Offset to a device / VariationIndex table from the start of the
    /// *parent* table, or 0 for none.
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

    /// The design-unit value adjusted for the current variation instance.
    ///
    /// `parent_bytes` is the slice the record's `device_offset` is relative
    /// to (the parent sub-table base, per §6.3.6.2.1), `ivs` the GDEF
    /// `ItemVariationStore` (pass `None` for a non-variable font), and
    /// `coords` the normalised axis coordinates. A NULL `device_offset`, a
    /// classic Device table, or a missing/out-of-range VariationIndex all
    /// fold to a zero adjustment, so the return collapses to `value` for a
    /// static instance.
    ///
    /// The result is `value as f32` plus the resolved font-unit delta; a
    /// caller wanting an integer can round it.
    pub fn resolved_value(
        &self,
        parent_bytes: &[u8],
        ivs: Option<&ItemVariationStore>,
        coords: &[f32],
    ) -> f32 {
        let delta = resolve_device_delta(parent_bytes, self.device_offset, ivs, coords);
        self.value as f32 + delta
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

    /// MathValueRecord `index` resolved for the current variation instance.
    ///
    /// Folds in the record's device / VariationIndex correction per
    /// §6.3.6.2.1: in a variable font the `(outer, inner)` delta-set index
    /// is evaluated against the GDEF `ItemVariationStore` `ivs` at `coords`;
    /// in a static font (or for an absent record) the result is the plain
    /// design-unit value. The device offset is relative to the start of the
    /// MathConstants table, so the parent slice is `self.data[self.base..]`.
    pub fn value_resolved(
        &self,
        index: usize,
        ivs: Option<&ItemVariationStore>,
        coords: &[f32],
    ) -> f32 {
        match self.value(index) {
            Some(r) => r.resolved_value(&self.data[self.base..], ivs, coords),
            None => 0.0,
        }
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

    /// Look up the per-glyph MathValueRecord at `value_sub`
    /// (`mathItalicsCorrectionInfo` index 0 / `mathTopAccentAttachment`
    /// index 1) for `gid`, returning the *record* (value + parent-relative
    /// device offset) and the parent-table base needed to resolve that
    /// offset per §6.3.6.2.1.
    fn value_record_for(&self, value_sub: usize, gid: u16) -> Option<(MathValueRecord, usize)> {
        let base = self.sub_off(value_sub)?;
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
        Some((MathValueRecord::read(self.data, at).ok()?, base))
    }

    /// Italics-correction for `gid` resolved at the current variation
    /// instance (§6.3.6.2.5 + §6.3.6.2.1). Folds in a VariationIndex delta
    /// against the GDEF `ItemVariationStore` `ivs` at `coords`; `None` when
    /// uncovered (layout treats that as zero).
    pub fn italics_correction_resolved(
        &self,
        gid: u16,
        ivs: Option<&ItemVariationStore>,
        coords: &[f32],
    ) -> Option<f32> {
        let (rec, base) = self.value_record_for(0, gid)?;
        Some(rec.resolved_value(&self.data[base..], ivs, coords))
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

    /// Top-accent attachment for `gid` resolved at the current variation
    /// instance (§6.3.6.2.6 + §6.3.6.2.1).
    pub fn top_accent_attachment_resolved(
        &self,
        gid: u16,
        ivs: Option<&ItemVariationStore>,
        coords: &[f32],
    ) -> Option<f32> {
        let (rec, base) = self.value_record_for(1, gid)?;
        Some(rec.resolved_value(&self.data[base..], ivs, coords))
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
        math_kern_value(self.data, base + kern_off, height).map(|r| r.value)
    }

    /// Math-kern value for `gid` at one corner and correction `height`,
    /// resolved at the current variation instance (§6.3.6.2.8/.9 +
    /// §6.3.6.2.1). The selected kern value's device offset is parent-
    /// relative to the MathKern table, so a VariationIndex delta is
    /// evaluated against `ivs` at `coords`.
    pub fn math_kern_resolved(
        &self,
        gid: u16,
        corner: MathKernCorner,
        height: i16,
        ivs: Option<&ItemVariationStore>,
        coords: &[f32],
    ) -> Option<f32> {
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
        let rec_at = base + 4 + idx * 8;
        let kern_off = read_u16(self.data, rec_at + corner as usize * 2).ok()? as usize;
        if kern_off == 0 {
            return None;
        }
        let kern_base = base + kern_off;
        let rec = math_kern_value(self.data, kern_base, height)?;
        Some(rec.resolved_value(&self.data[kern_base..], ivs, coords))
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
fn math_kern_value(data: &[u8], base: usize, height: i16) -> Option<MathValueRecord> {
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
    MathValueRecord::read(data, at).ok()
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

    /// The glyph-assembly italics correction for `gid` growing in `dir`,
    /// resolved at the current variation instance (§6.3.6.2.12 +
    /// §6.3.6.2.1). The italicsCorrection record's device offset is
    /// relative to the GlyphAssembly table, so a VariationIndex delta is
    /// evaluated against `ivs` at `coords`. `None` when no assembly is
    /// defined for `gid` in `dir`.
    pub fn assembly_italics_correction_resolved(
        &self,
        gid: u16,
        dir: GrowDirection,
        ivs: Option<&ItemVariationStore>,
        coords: &[f32],
    ) -> Option<f32> {
        let ctor = self.construction_off(gid, dir)?;
        let asm_off = read_u16(self.data, ctor).ok()? as usize;
        if asm_off == 0 {
            return None;
        }
        let asm = ctor + asm_off;
        let rec = MathValueRecord::read(self.data, asm).ok()?;
        Some(rec.resolved_value(&self.data[asm..], ivs, coords))
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

    /// A single-axis, single-region ItemVariationStore that contributes
    /// `delta` font units at the +1 end of the axis. Mirrors the GDEF /
    /// GPOS test stores so MATH VariationIndex resolution exercises the
    /// same shared decoder.
    fn build_single_region_ivs(delta: i16) -> Vec<u8> {
        let mut b = vec![0u8; 32];
        b[0..2].copy_from_slice(&1u16.to_be_bytes()); // format 1
        b[2..6].copy_from_slice(&12u32.to_be_bytes()); // regionListOffset
        b[6..8].copy_from_slice(&1u16.to_be_bytes()); // itemVariationDataCount
        b[8..12].copy_from_slice(&22u32.to_be_bytes()); // IVD[0] offset
        b[12..14].copy_from_slice(&1u16.to_be_bytes()); // axisCount
        b[14..16].copy_from_slice(&1u16.to_be_bytes()); // regionCount
        b[16..18].copy_from_slice(&0i16.to_be_bytes()); // startCoord
        b[18..20].copy_from_slice(&16384i16.to_be_bytes()); // peakCoord = 1.0
        b[20..22].copy_from_slice(&16384i16.to_be_bytes()); // endCoord = 1.0
        b[22..24].copy_from_slice(&1u16.to_be_bytes()); // itemCount
        b[24..26].copy_from_slice(&1u16.to_be_bytes()); // shortDeltaCount
        b[26..28].copy_from_slice(&1u16.to_be_bytes()); // regionIndexCount
        b[28..30].copy_from_slice(&0u16.to_be_bytes()); // regionIndexes[0]
        b[30..32].copy_from_slice(&delta.to_be_bytes()); // deltaSets[0]
        b
    }

    /// Build a MATH table whose MathConstants `axisHeight` record carries a
    /// VariationIndex device offset (outer 0, inner 0) pointing at a
    /// VariationIndex sub-table appended after the constants table. The
    /// device offset is measured from the start of the MathConstants table
    /// per §6.3.6.2.1.
    fn build_math_constants_with_var_axis_height(base: i16) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&1u16.to_be_bytes()); // major
        data.extend_from_slice(&0u16.to_be_bytes()); // minor
        let const_off_pos = data.len();
        data.extend_from_slice(&0u16.to_be_bytes()); // constants (patched)
        data.extend_from_slice(&0u16.to_be_bytes()); // glyphInfo
        data.extend_from_slice(&0u16.to_be_bytes()); // variants

        let const_off = data.len();
        data.extend_from_slice(&80i16.to_be_bytes()); // scriptPercentScaleDown
        data.extend_from_slice(&60i16.to_be_bytes()); // scriptScriptPercentScaleDown
        data.extend_from_slice(&300u16.to_be_bytes()); // delimitedSubFormulaMinHeight
        data.extend_from_slice(&1500u16.to_be_bytes()); // displayOperatorMinHeight
        let records_at = data.len();
        for i in 0..51usize {
            if i == constant::AXIS_HEIGHT {
                data.extend_from_slice(&base.to_be_bytes()); // value
                data.extend_from_slice(&0u16.to_be_bytes()); // device offset (patched)
            } else {
                data.extend_from_slice(&0i16.to_be_bytes());
                data.extend_from_slice(&0u16.to_be_bytes());
            }
        }
        data.extend_from_slice(&60i16.to_be_bytes()); // radicalDegreeBottomRaisePercent

        // VariationIndex sub-table (outer 0, inner 0, fmt 0x8000), appended
        // right after the constants table; its offset is relative to the
        // MathConstants table start.
        let var_idx_at = data.len();
        data.extend_from_slice(&0u16.to_be_bytes()); // outer
        data.extend_from_slice(&0u16.to_be_bytes()); // inner
        data.extend_from_slice(&0x8000u16.to_be_bytes()); // deltaFormat

        // Patch the axisHeight record's device offset (parent-relative).
        let dev_off_pos = records_at + constant::AXIS_HEIGHT * MathValueRecord::LEN + 2;
        let dev_off = (var_idx_at - const_off) as u16;
        data[dev_off_pos..dev_off_pos + 2].copy_from_slice(&dev_off.to_be_bytes());
        // Patch the constants offset.
        data[const_off_pos..const_off_pos + 2].copy_from_slice(&(const_off as u16).to_be_bytes());
        data
    }

    #[test]
    fn math_constants_value_resolves_variation_index_delta() {
        let data = build_math_constants_with_var_axis_height(250);
        let m = MathTable::parse(&data).expect("parse");
        let c = m.constants().expect("constants");
        let ivs_bytes = build_single_region_ivs(-40);
        let ivs = ItemVariationStore::parse(&ivs_bytes).expect("ivs");

        // Plain value ignores the device offset entirely.
        assert_eq!(c.value_i16(constant::AXIS_HEIGHT), 250);

        // No IVS → static value (device contributes nothing).
        assert_eq!(c.value_resolved(constant::AXIS_HEIGHT, None, &[0.0]), 250.0);
        // Default instance (coord 0): region scalar 0 → no delta.
        assert_eq!(
            c.value_resolved(constant::AXIS_HEIGHT, Some(&ivs), &[0.0]),
            250.0
        );
        // Max instance (coord +1): 250 + (-40) = 210.
        assert_eq!(
            c.value_resolved(constant::AXIS_HEIGHT, Some(&ivs), &[1.0]),
            210.0
        );
        // Half: 250 + (-20) = 230.
        assert_eq!(
            c.value_resolved(constant::AXIS_HEIGHT, Some(&ivs), &[0.5]),
            230.0
        );

        // A record with no device offset folds to its plain value.
        assert_eq!(
            c.value_resolved(constant::MATH_LEADING, Some(&ivs), &[1.0]),
            0.0
        );
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

    /// Build a MATH table with a MathGlyphInfo carrying, for gid 7:
    ///   * an italicsCorrectionInfo entry (value `ic`, VariationIndex dev),
    ///   * a topAccentAttachment entry (value `tac`, no device),
    ///   * a MathKernInfo with a single TopRight kern (one height boundary,
    ///     two kern values; the upper kern carries a VariationIndex dev).
    ///
    /// All device offsets are parent-relative per §6.3.6.2.1.
    fn build_math_glyph_info(ic: i16, tac: i16, kern_hi: i16) -> Vec<u8> {
        // We build the MathGlyphInfo body self-contained, then splice it
        // into a MATH header at glyphInfoOffset.
        // ---- italicsCorrectionInfo (gi-relative offsets) ----
        // header: coverageOffset(=8), italicsCorrectionCount(=1),
        //         MathValueRecord[1] = { ic, devOff }
        // then Coverage at +8, then a VariationIndex at +(after coverage).
        let mut gi = Vec::new();
        // We assemble four sub-tables back to back, recording their
        // gi-relative starts so the four leading Offset16 fields can point
        // at them. Layout: [4 Offset16 header][ic][tac][esc=0][kern].
        let header_len = 8usize; // four Offset16

        // -- italicsCorrectionInfo sub-table --
        let mut ic_sub = Vec::new();
        ic_sub.extend_from_slice(&8u16.to_be_bytes()); // coverageOffset
        ic_sub.extend_from_slice(&1u16.to_be_bytes()); // count
        ic_sub.extend_from_slice(&ic.to_be_bytes()); // value
        let ic_dev_pos = ic_sub.len();
        ic_sub.extend_from_slice(&0u16.to_be_bytes()); // device (patched)
                                                       // Coverage @ +8: format 1, [7]
        ic_sub.extend_from_slice(&1u16.to_be_bytes());
        ic_sub.extend_from_slice(&1u16.to_be_bytes());
        ic_sub.extend_from_slice(&7u16.to_be_bytes());
        // VariationIndex (outer 0, inner 0) right after coverage.
        let ic_var_at = ic_sub.len();
        ic_sub.extend_from_slice(&0u16.to_be_bytes());
        ic_sub.extend_from_slice(&0u16.to_be_bytes());
        ic_sub.extend_from_slice(&0x8000u16.to_be_bytes());
        ic_sub[ic_dev_pos..ic_dev_pos + 2].copy_from_slice(&(ic_var_at as u16).to_be_bytes());

        // -- topAccentAttachment sub-table (no device) --
        let mut tac_sub = Vec::new();
        tac_sub.extend_from_slice(&8u16.to_be_bytes()); // coverageOffset
        tac_sub.extend_from_slice(&1u16.to_be_bytes()); // count
        tac_sub.extend_from_slice(&tac.to_be_bytes());
        tac_sub.extend_from_slice(&0u16.to_be_bytes()); // no device
        tac_sub.extend_from_slice(&1u16.to_be_bytes()); // cov fmt
        tac_sub.extend_from_slice(&1u16.to_be_bytes());
        tac_sub.extend_from_slice(&7u16.to_be_bytes());

        // -- MathKernInfo sub-table --
        // header: coverageOffset, mathKernCount(=1),
        //         MathKernInfoRecord[1] = four Offset16 (TR,TL,BR,BL).
        // Only TopRight is non-zero → points at a MathKern table.
        let mut kern_sub = Vec::new();
        let kcov_pos = kern_sub.len();
        kern_sub.extend_from_slice(&0u16.to_be_bytes()); // coverageOffset (patched)
        kern_sub.extend_from_slice(&1u16.to_be_bytes()); // mathKernCount
        let krec_pos = kern_sub.len();
        kern_sub.extend_from_slice(&0u16.to_be_bytes()); // TR (patched)
        kern_sub.extend_from_slice(&0u16.to_be_bytes()); // TL
        kern_sub.extend_from_slice(&0u16.to_be_bytes()); // BR
        kern_sub.extend_from_slice(&0u16.to_be_bytes()); // BL
                                                         // Coverage [7].
        let kcov_at = kern_sub.len();
        kern_sub.extend_from_slice(&1u16.to_be_bytes());
        kern_sub.extend_from_slice(&1u16.to_be_bytes());
        kern_sub.extend_from_slice(&7u16.to_be_bytes());
        kern_sub[kcov_pos..kcov_pos + 2].copy_from_slice(&(kcov_at as u16).to_be_bytes());
        // MathKern table: heightCount=1, correctionHeight[0]=100 (no dev),
        //   kernValue[0]=10 (no dev), kernValue[1]=kern_hi (+ VariationIndex).
        let mkern_at = kern_sub.len();
        kern_sub.extend_from_slice(&1u16.to_be_bytes()); // heightCount
        kern_sub.extend_from_slice(&100i16.to_be_bytes()); // height[0] value
        kern_sub.extend_from_slice(&0u16.to_be_bytes()); // height[0] dev
        kern_sub.extend_from_slice(&10i16.to_be_bytes()); // kern[0] value
        kern_sub.extend_from_slice(&0u16.to_be_bytes()); // kern[0] dev
        kern_sub.extend_from_slice(&kern_hi.to_be_bytes()); // kern[1] value
        let khi_dev_pos = kern_sub.len();
        kern_sub.extend_from_slice(&0u16.to_be_bytes()); // kern[1] dev (patched)
        let kvar_at = kern_sub.len();
        kern_sub.extend_from_slice(&0u16.to_be_bytes()); // outer
        kern_sub.extend_from_slice(&0u16.to_be_bytes()); // inner
        kern_sub.extend_from_slice(&0x8000u16.to_be_bytes()); // fmt
                                                              // kern[1] device offset is relative to the MathKern table start.
        kern_sub[khi_dev_pos..khi_dev_pos + 2]
            .copy_from_slice(&((kvar_at - mkern_at) as u16).to_be_bytes());
        kern_sub[krec_pos..krec_pos + 2].copy_from_slice(&(mkern_at as u16).to_be_bytes());

        // Assemble the MathGlyphInfo: header (four Offset16) + sub-tables.
        let ic_at = header_len;
        let tac_at = ic_at + ic_sub.len();
        let kern_at = tac_at + tac_sub.len();
        gi.extend_from_slice(&(ic_at as u16).to_be_bytes()); // italicsCorrectionInfoOffset
        gi.extend_from_slice(&(tac_at as u16).to_be_bytes()); // topAccentAttachmentOffset
        gi.extend_from_slice(&0u16.to_be_bytes()); // extendedShapeCoverageOffset = none
        gi.extend_from_slice(&(kern_at as u16).to_be_bytes()); // mathKernInfoOffset
        gi.extend_from_slice(&ic_sub);
        gi.extend_from_slice(&tac_sub);
        gi.extend_from_slice(&kern_sub);

        // MATH header: glyphInfo only.
        let mut data = Vec::new();
        data.extend_from_slice(&1u16.to_be_bytes()); // major
        data.extend_from_slice(&0u16.to_be_bytes()); // minor
        data.extend_from_slice(&0u16.to_be_bytes()); // constants = none
        let gi_off_pos = data.len();
        data.extend_from_slice(&0u16.to_be_bytes()); // glyphInfo (patched)
        data.extend_from_slice(&0u16.to_be_bytes()); // variants = none
        let gi_off = data.len();
        data.extend_from_slice(&gi);
        data[gi_off_pos..gi_off_pos + 2].copy_from_slice(&(gi_off as u16).to_be_bytes());
        data
    }

    #[test]
    fn glyph_info_values_resolve_variation_deltas() {
        let data = build_math_glyph_info(120, 300, 25);
        let m = MathTable::parse(&data).expect("parse");
        let gi = m.glyph_info().expect("glyph info");
        let ivs_bytes = build_single_region_ivs(-15);
        let ivs = ItemVariationStore::parse(&ivs_bytes).expect("ivs");

        // Plain accessors ignore device offsets.
        assert_eq!(gi.italics_correction(7), Some(120));
        assert_eq!(gi.top_accent_attachment(7), Some(300));
        // Below the single height boundary (100) → first kern value (10).
        assert_eq!(gi.math_kern(7, MathKernCorner::TopRight, 50), Some(10));
        // At/above the boundary → second kern value (25).
        assert_eq!(gi.math_kern(7, MathKernCorner::TopRight, 150), Some(25));

        // Resolved italics correction tracks the instance.
        assert_eq!(
            gi.italics_correction_resolved(7, Some(&ivs), &[0.0]),
            Some(120.0)
        );
        assert_eq!(
            gi.italics_correction_resolved(7, Some(&ivs), &[1.0]),
            Some(105.0)
        ); // 120 + (-15)
           // Top-accent has no device → unchanged.
        assert_eq!(
            gi.top_accent_attachment_resolved(7, Some(&ivs), &[1.0]),
            Some(300.0)
        );
        // The lower kern range has no device → static.
        assert_eq!(
            gi.math_kern_resolved(7, MathKernCorner::TopRight, 50, Some(&ivs), &[1.0]),
            Some(10.0)
        );
        // The upper kern range carries the VariationIndex.
        assert_eq!(
            gi.math_kern_resolved(7, MathKernCorner::TopRight, 150, Some(&ivs), &[0.0]),
            Some(25.0)
        );
        assert_eq!(
            gi.math_kern_resolved(7, MathKernCorner::TopRight, 150, Some(&ivs), &[1.0]),
            Some(10.0)
        ); // 25 + (-15)

        // Uncovered glyph → None on every accessor.
        assert!(gi
            .italics_correction_resolved(99, Some(&ivs), &[1.0])
            .is_none());
        assert!(gi
            .math_kern_resolved(99, MathKernCorner::TopRight, 0, Some(&ivs), &[1.0])
            .is_none());
    }
}
