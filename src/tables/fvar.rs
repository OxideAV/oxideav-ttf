//! `fvar` — Font Variations Header.
//!
//! Spec: Microsoft OpenType §"fvar — Font Variations Table" / OpenType
//! 1.9. Apple TrueType Reference §"fvar".
//!
//! The table publishes the font's *design space*: a list of variation
//! axes (e.g. `wght`, `wdth`, `slnt`, `opsz`, plus any custom 4-byte
//! tag) each carrying a `(min, default, max)` triple in user-space
//! units (Fixed 16.16). It also publishes a list of **named instances**
//! (e.g. "Light", "Regular", "Bold") that pin the design space to a
//! single coordinate vector and reference a `name` table id for the
//! human-readable label.
//!
//! Header layout (all fields big-endian):
//!
//! ```text
//!   0  / 2  / majorVersion             (1)
//!   2  / 2  / minorVersion             (0)
//!   4  / 2  / axesArrayOffset          (relative to fvar start)
//!   6  / 2  / (reserved, must be 2)
//!   8  / 2  / axisCount
//!  10  / 2  / axisSize                 (== 20 for v1.0)
//!  12  / 2  / instanceCount
//!  14  / 2  / instanceSize             (== 4 + 4*axisCount, optionally
//!                                       + 2 for postScriptNameID)
//! ```
//!
//! Each axis record (`axisSize` bytes):
//!
//! ```text
//!   0 / 4 / axisTag                   (4 ASCII bytes)
//!   4 / 4 / minValue                  (Fixed 16.16)
//!   8 / 4 / defaultValue              (Fixed 16.16)
//!  12 / 4 / maxValue                  (Fixed 16.16)
//!  16 / 2 / flags                     (bit 0 = HIDDEN_AXIS)
//!  18 / 2 / axisNameID                (`name` table id)
//! ```
//!
//! Each instance record (`instanceSize` bytes):
//!
//! ```text
//!   0 / 2 / subfamilyNameID           (`name` table id)
//!   2 / 2 / flags
//!   4 / 4*axisCount / coordinates      (Fixed 16.16 each)
//!   ? / 2 / postScriptNameID          (optional — only when
//!                                      instanceSize == 6 + 4*axisCount)
//! ```

use crate::parser::{read_i32, read_u16};
use crate::Error;

/// Minimum legal `axisSize` per the spec (one axis record).
const MIN_AXIS_SIZE: u16 = 20;
/// Sanity cap. Real fonts publish at most a handful of axes; the cap
/// keeps a malformed header from making us allocate wildly.
const MAX_AXES: u16 = 64;
/// Sanity cap on named-instance count.
const MAX_INSTANCES: u16 = 4096;
/// "HIDDEN_AXIS" bit on `axis.flags` — the axis exists in the design
/// space but UI pickers should not surface it. We keep parsing it
/// (callers may need it for shaping) but expose the bit so consumers
/// can filter.
pub const AXIS_FLAG_HIDDEN: u16 = 0x0001;

/// One variation axis as published in the font's `fvar` table. All
/// values are in user-space units (Fixed 16.16 scaled to f32 here).
#[derive(Debug, Clone, PartialEq)]
pub struct VariationAxis {
    pub tag: [u8; 4],
    pub min: f32,
    pub default: f32,
    pub max: f32,
    pub flags: u16,
    /// `name` table id for the human-readable axis label.
    pub name_id: u16,
}

impl VariationAxis {
    /// `true` if the axis carries the `HIDDEN_AXIS` flag — UI pickers
    /// should skip it but shapers should still honour any coordinate
    /// pinned by the caller.
    pub fn is_hidden(&self) -> bool {
        self.flags & AXIS_FLAG_HIDDEN != 0
    }
}

/// One named instance (a pre-defined coordinate vector).
#[derive(Debug, Clone, PartialEq)]
pub struct NamedInstance {
    /// `name` table id for the subfamily label ("Light", "Bold" …).
    pub subfamily_name_id: u16,
    pub flags: u16,
    /// One coordinate per axis, in axis-declaration order.
    pub coords: Vec<f32>,
    /// Optional `name` table id for the PostScript name; `None` when
    /// the instance record is the short variant (no trailing
    /// `postScriptNameID`).
    pub post_script_name_id: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct FvarTable {
    axes: Vec<VariationAxis>,
    instances: Vec<NamedInstance>,
}

impl FvarTable {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 16 {
            return Err(Error::UnexpectedEof);
        }
        let major = read_u16(bytes, 0)?;
        let minor = read_u16(bytes, 2)?;
        if major != 1 || minor != 0 {
            return Err(Error::BadStructure("fvar version not 1.0"));
        }
        let axes_array_offset = read_u16(bytes, 4)? as usize;
        // bytes[6..8] is reserved (== 2 in valid fonts) — we don't
        // enforce so we accept the rare font that emits 0 here.
        let axis_count = read_u16(bytes, 8)?;
        let axis_size = read_u16(bytes, 10)?;
        let instance_count = read_u16(bytes, 12)?;
        let instance_size = read_u16(bytes, 14)?;

        if axis_count > MAX_AXES {
            return Err(Error::BadStructure("fvar axisCount exceeds sanity cap"));
        }
        if instance_count > MAX_INSTANCES {
            return Err(Error::BadStructure("fvar instanceCount exceeds sanity cap"));
        }
        if axis_size < MIN_AXIS_SIZE {
            return Err(Error::BadStructure("fvar axisSize < 20"));
        }
        // Per spec the *minimum* instance record size is
        // `4 + 4 * axisCount`; the optional `postScriptNameID` adds 2.
        let min_instance_size = 4u16
            .checked_add(
                axis_count
                    .checked_mul(4)
                    .ok_or(Error::BadStructure("fvar axisCount * 4 overflow"))?,
            )
            .ok_or(Error::BadStructure("fvar instanceSize overflow"))?;
        if instance_size != min_instance_size && instance_size != min_instance_size + 2 {
            return Err(Error::BadStructure("fvar instanceSize unexpected"));
        }
        let has_psname = instance_size == min_instance_size + 2;

        // Parse axes.
        let mut axes = Vec::with_capacity(axis_count as usize);
        for i in 0..axis_count as usize {
            let off = axes_array_offset
                .checked_add(i.checked_mul(axis_size as usize).ok_or(Error::BadOffset)?)
                .ok_or(Error::BadOffset)?;
            if off + axis_size as usize > bytes.len() {
                return Err(Error::UnexpectedEof);
            }
            let rec = &bytes[off..off + axis_size as usize];
            let mut tag = [0u8; 4];
            tag.copy_from_slice(&rec[0..4]);
            let min = fixed_to_f32(read_i32(rec, 4)?);
            let default = fixed_to_f32(read_i32(rec, 8)?);
            let max = fixed_to_f32(read_i32(rec, 12)?);
            let flags = read_u16(rec, 16)?;
            let name_id = read_u16(rec, 18)?;
            if !(min <= default && default <= max) {
                return Err(Error::BadStructure("fvar axis min/default/max disorder"));
            }
            axes.push(VariationAxis {
                tag,
                min,
                default,
                max,
                flags,
                name_id,
            });
        }

        // Parse instances.
        let inst_array_offset = axes_array_offset
            .checked_add(
                (axis_count as usize)
                    .checked_mul(axis_size as usize)
                    .ok_or(Error::BadOffset)?,
            )
            .ok_or(Error::BadOffset)?;
        let mut instances = Vec::with_capacity(instance_count as usize);
        for i in 0..instance_count as usize {
            let off = inst_array_offset
                .checked_add(
                    i.checked_mul(instance_size as usize)
                        .ok_or(Error::BadOffset)?,
                )
                .ok_or(Error::BadOffset)?;
            if off + instance_size as usize > bytes.len() {
                return Err(Error::UnexpectedEof);
            }
            let rec = &bytes[off..off + instance_size as usize];
            let subfamily_name_id = read_u16(rec, 0)?;
            let flags = read_u16(rec, 2)?;
            let mut coords = Vec::with_capacity(axis_count as usize);
            for ai in 0..axis_count as usize {
                coords.push(fixed_to_f32(read_i32(rec, 4 + ai * 4)?));
            }
            let post_script_name_id = if has_psname {
                Some(read_u16(rec, 4 + axis_count as usize * 4)?)
            } else {
                None
            };
            instances.push(NamedInstance {
                subfamily_name_id,
                flags,
                coords,
                post_script_name_id,
            });
        }

        Ok(Self { axes, instances })
    }

    pub fn axes(&self) -> &[VariationAxis] {
        &self.axes
    }

    pub fn instances(&self) -> &[NamedInstance] {
        &self.instances
    }

    pub fn axis_count(&self) -> usize {
        self.axes.len()
    }
}

#[inline]
fn fixed_to_f32(raw: i32) -> f32 {
    raw as f32 / 65536.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic fvar with one axis (`wght`, 100..400..900) and
    /// no instances.
    fn build_one_axis(min: f32, def: f32, max: f32) -> Vec<u8> {
        let mut b = vec![0u8; 16 + 20];
        b[0..2].copy_from_slice(&1u16.to_be_bytes()); // major
        b[2..4].copy_from_slice(&0u16.to_be_bytes()); // minor
        b[4..6].copy_from_slice(&16u16.to_be_bytes()); // axesArrayOffset
        b[6..8].copy_from_slice(&2u16.to_be_bytes()); // reserved
        b[8..10].copy_from_slice(&1u16.to_be_bytes()); // axisCount
        b[10..12].copy_from_slice(&20u16.to_be_bytes()); // axisSize
        b[12..14].copy_from_slice(&0u16.to_be_bytes()); // instanceCount
        b[14..16].copy_from_slice(&8u16.to_be_bytes()); // instanceSize (4 + 4*1)
        let rec = &mut b[16..36];
        rec[0..4].copy_from_slice(b"wght");
        rec[4..8].copy_from_slice(&((min * 65536.0) as i32).to_be_bytes());
        rec[8..12].copy_from_slice(&((def * 65536.0) as i32).to_be_bytes());
        rec[12..16].copy_from_slice(&((max * 65536.0) as i32).to_be_bytes());
        rec[16..18].copy_from_slice(&0u16.to_be_bytes()); // flags
        rec[18..20].copy_from_slice(&256u16.to_be_bytes()); // nameID
        b
    }

    #[test]
    fn fvar_parses_wght_axis_min_default_max() {
        let raw = build_one_axis(100.0, 400.0, 900.0);
        let f = FvarTable::parse(&raw).expect("parse fvar");
        assert_eq!(f.axes().len(), 1);
        let a = &f.axes()[0];
        assert_eq!(&a.tag, b"wght");
        assert_eq!(a.min, 100.0);
        assert_eq!(a.default, 400.0);
        assert_eq!(a.max, 900.0);
        assert_eq!(a.name_id, 256);
        assert!(!a.is_hidden());
        assert!(f.instances().is_empty());
    }

    #[test]
    fn fvar_rejects_disordered_min_default_max() {
        let raw = build_one_axis(900.0, 400.0, 100.0);
        assert!(matches!(
            FvarTable::parse(&raw),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn fvar_parses_named_instance() {
        // One axis (wght 100..400..900), one instance pinning wght=700,
        // sub-family nameID 257, no postScriptNameID.
        let mut b = vec![0u8; 16 + 20 + 12];
        b[0..2].copy_from_slice(&1u16.to_be_bytes());
        b[4..6].copy_from_slice(&16u16.to_be_bytes());
        b[6..8].copy_from_slice(&2u16.to_be_bytes());
        b[8..10].copy_from_slice(&1u16.to_be_bytes());
        b[10..12].copy_from_slice(&20u16.to_be_bytes());
        b[12..14].copy_from_slice(&1u16.to_be_bytes());
        b[14..16].copy_from_slice(&8u16.to_be_bytes()); // 4 + 4*1
        let rec = &mut b[16..36];
        rec[0..4].copy_from_slice(b"wght");
        rec[4..8].copy_from_slice(&(100i32 << 16).to_be_bytes());
        rec[8..12].copy_from_slice(&(400i32 << 16).to_be_bytes());
        rec[12..16].copy_from_slice(&(900i32 << 16).to_be_bytes());
        rec[18..20].copy_from_slice(&256u16.to_be_bytes());
        let inst = &mut b[36..44];
        inst[0..2].copy_from_slice(&257u16.to_be_bytes()); // subfamilyNameID
        inst[2..4].copy_from_slice(&0u16.to_be_bytes());
        inst[4..8].copy_from_slice(&(700i32 << 16).to_be_bytes());

        let f = FvarTable::parse(&b).expect("parse");
        assert_eq!(f.instances().len(), 1);
        let i = &f.instances()[0];
        assert_eq!(i.subfamily_name_id, 257);
        assert_eq!(i.coords, vec![700.0]);
        assert!(i.post_script_name_id.is_none());
    }

    #[test]
    fn fvar_rejects_short_header() {
        let b = vec![0u8; 8];
        assert!(matches!(FvarTable::parse(&b), Err(Error::UnexpectedEof)));
    }
}
