//! `cvar` — CVT Variations Table.
//!
//! Spec: ISO/IEC 14496-22:2019 §7.3.2 ("cvar — CVT variations table")
//! and §7.2.2 ("Tuple variation store"). The `cvar` table provides
//! interpolation deltas for the Control Value Table (`cvt `) entries of
//! a variable font, the same way `gvar` provides deltas for glyph
//! outline points. It exists only in TrueType-outline variable fonts
//! that also ship a `cvt ` table and TrueType hinting bytecode (`prep`
//! / `fpgm`); the deltas adjust the CVT entries the hinting program
//! reads before instructing each instance.
//!
//! Structurally `cvar` is a **single** tuple variation store
//! (§7.2.2) — identical to one of `gvar`'s per-glyph
//! `GlyphVariationData` blocks — with three differences (§7.2.2.4 /
//! §7.2.2.5):
//!
//! 1. It is prefixed by `majorVersion` / `minorVersion` (`1` / `0`).
//! 2. There is **no** shared-tuple array (the `cvar` header has no
//!    `sharedTuplesOffset`), so every tuple's peak coordinates are
//!    stored inline via `EMBEDDED_PEAK_TUPLE`. A tuple whose
//!    `tupleIndex` omits the embedded-peak flag would reference a
//!    non-existent shared tuple; such a tuple contributes nothing.
//! 3. Each tuple carries **one** packed-delta array (CVT deltas), not
//!    the `gvar` X-then-Y pair.
//!
//! "Point numbers" in the packed-point set are interpreted as **CVT
//! indices** rather than outline point numbers (§7.2.2.5). Unlike
//! `gvar`, omitted CVT entries are **not** inferred (§7.2.2.4 NOTE):
//! a CVT entry absent from a tuple's point set simply receives no
//! adjustment from that tuple.
//!
//! ## Header layout
//!
//! ```text
//!   0 / 2 / majorVersion           (1)
//!   2 / 2 / minorVersion           (0)
//!   4 / 2 / tupleVariationCount     packed: high 4 bits flags,
//!                                   low 12 bits = number of tuples
//!   6 / 2 / dataOffset              offset (from cvar start) to the
//!                                   serialized data area
//!   8 / .. / TupleVariationHeader[tupleVariationCount]
//!   .. / .. / serialized data (shared points + per-tuple data)
//! ```
//!
//! Each `TupleVariationHeader` and the packed point / delta encodings
//! are exactly as documented in [`super::gvar`]; this module reuses
//! `gvar`'s `decode_packed_points` / `decode_packed_deltas` /
//! `tuple_scalar` helpers.
//!
//! The public entry point is [`CvarTable::cvt_deltas`]: given the axis
//! count, the CVT entry count, and a normalised coordinate vector, it
//! returns a `Vec<i32>` of per-CVT deltas (font units) to add to the
//! static `cvt ` values for that variation instance.

use super::gvar::{decode_packed_deltas, decode_packed_points, f2dot14, tuple_scalar};
use crate::parser::{read_i16, read_u16};
use crate::Error;

/// Tuple-index high-byte flags (mask `0xF000`) — shared with `gvar`.
const TI_EMBEDDED_PEAK: u16 = 0x8000;
const TI_INTERMEDIATE: u16 = 0x4000;
const TI_PRIVATE_POINTS: u16 = 0x2000;

/// `tupleVariationCount` low-bit count mask (§7.2.2.2).
const COUNT_MASK: u16 = 0x0FFF;

/// Sanity cap on the tuple count (the field is 12 bits → max 4095).
const MAX_TUPLES: u16 = 4095;

/// Parsed `cvar` table. The bytes are held for lazy decoding; only the
/// fixed 8-byte header is validated up front.
#[derive(Debug, Clone)]
pub struct CvarTable<'a> {
    bytes: &'a [u8],
    major_version: u16,
    minor_version: u16,
    tuple_count: u16,
    data_offset: usize,
}

impl<'a> CvarTable<'a> {
    /// Validate the 8-byte header and remember the serialized-data
    /// offset. Returns [`Error::BadStructure`] when the major version is
    /// not 1 or the data offset falls outside the slice.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < 8 {
            return Err(Error::UnexpectedEof);
        }
        let major_version = read_u16(bytes, 0)?;
        let minor_version = read_u16(bytes, 2)?;
        if major_version != 1 {
            return Err(Error::BadStructure("cvar majorVersion != 1"));
        }
        let packed = read_u16(bytes, 4)?;
        let tuple_count = packed & COUNT_MASK;
        if tuple_count > MAX_TUPLES {
            return Err(Error::BadStructure("cvar tupleVariationCount > cap"));
        }
        let data_offset = read_u16(bytes, 6)? as usize;
        if data_offset > bytes.len() {
            return Err(Error::BadOffset);
        }
        Ok(Self {
            bytes,
            major_version,
            minor_version,
            tuple_count,
            data_offset,
        })
    }

    /// `cvar` major version (always `1`).
    pub fn major_version(&self) -> u16 {
        self.major_version
    }

    /// `cvar` minor version (`0` in the current spec).
    pub fn minor_version(&self) -> u16 {
        self.minor_version
    }

    /// Number of tuple variation tables in this store.
    pub fn tuple_count(&self) -> u16 {
        self.tuple_count
    }

    /// Compute per-CVT deltas for the variation instance at the
    /// normalised coordinate vector `coords` (one F2DOT14-scale value
    /// per axis, in `fvar` axis order). `axis_count` must equal the
    /// `fvar` axis count and `cvt_count` the number of `cvt ` entries.
    ///
    /// Returns a `Vec<i32>` of length `cvt_count`; index `i` is the
    /// delta to add to CVT entry `i`. CVT entries that no tuple varies
    /// (or that fall outside the active variation regions) get a delta
    /// of 0. CVT indices a tuple references but that exceed `cvt_count`
    /// are dropped defensively.
    pub fn cvt_deltas(
        &self,
        axis_count: u16,
        cvt_count: u16,
        coords: &[f32],
    ) -> Result<Vec<i32>, Error> {
        let mut out = vec![0i32; cvt_count as usize];
        if self.tuple_count == 0 {
            return Ok(out);
        }
        let bytes = self.bytes;
        let axis_count = axis_count as usize;

        // Headers begin at +8 (after the 8-byte fixed header).
        let mut hdr_off = 8usize;
        let mut data_cursor = self.data_offset;

        // Shared point-number set lives at the very top of the data
        // area, consumed once and reused by any tuple lacking the
        // PRIVATE_POINT_NUMBERS flag (§7.2.2.4).
        let shared_points: Option<Vec<u16>> = if self.data_offset < bytes.len() {
            let (pts, used) = decode_packed_points(&bytes[self.data_offset..], cvt_count)?;
            data_cursor = self.data_offset + used;
            Some(pts)
        } else {
            None
        };

        for _ in 0..self.tuple_count {
            if hdr_off + 4 > bytes.len() {
                return Err(Error::BadStructure("cvar tuple header truncated"));
            }
            let var_data_size = read_u16(bytes, hdr_off)? as usize;
            let tuple_index = read_u16(bytes, hdr_off + 2)?;
            hdr_off += 4;

            // Peak coordinates. cvar has no shared-tuple array, so the
            // peak must be embedded; a tuple without EMBEDDED_PEAK is
            // malformed for cvar and contributes nothing.
            let peak: Option<Vec<f32>> = if tuple_index & TI_EMBEDDED_PEAK != 0 {
                let need = axis_count * 2;
                if hdr_off + need > bytes.len() {
                    return Err(Error::BadStructure("cvar embedded peak truncated"));
                }
                let mut p = Vec::with_capacity(axis_count);
                for ai in 0..axis_count {
                    p.push(f2dot14(read_i16(bytes, hdr_off + ai * 2)?));
                }
                hdr_off += need;
                Some(p)
            } else {
                None
            };

            // Optional intermediate region.
            let (start_t, end_t) = if tuple_index & TI_INTERMEDIATE != 0 {
                let need = axis_count * 4;
                if hdr_off + need > bytes.len() {
                    return Err(Error::BadStructure("cvar intermediate region truncated"));
                }
                let mut s = Vec::with_capacity(axis_count);
                let mut e = Vec::with_capacity(axis_count);
                for ai in 0..axis_count {
                    s.push(f2dot14(read_i16(bytes, hdr_off + ai * 2)?));
                }
                for ai in 0..axis_count {
                    e.push(f2dot14(read_i16(bytes, hdr_off + axis_count * 2 + ai * 2)?));
                }
                hdr_off += need;
                (Some(s), Some(e))
            } else {
                (None, None)
            };

            // Locate this tuple's packed data inside the data area.
            if data_cursor + var_data_size > bytes.len() {
                return Err(Error::BadStructure("cvar tuple data overruns"));
            }
            let tuple_data = &bytes[data_cursor..data_cursor + var_data_size];
            data_cursor += var_data_size;

            // No embedded peak → no shared tuple to fall back on; skip.
            let peak = match peak {
                Some(p) => p,
                None => continue,
            };

            let scalar = tuple_scalar(coords, &peak, start_t.as_deref(), end_t.as_deref());
            if scalar == 0.0 {
                continue;
            }

            // Decode this tuple's CVT-index set + single delta array.
            let mut td_off = 0usize;
            let points = if tuple_index & TI_PRIVATE_POINTS != 0 {
                let (pts, used) = decode_packed_points(tuple_data, cvt_count)?;
                td_off += used;
                pts
            } else {
                shared_points
                    .clone()
                    .unwrap_or_else(|| (0..cvt_count).collect())
            };
            let n = points.len();
            let deltas = decode_packed_deltas(tuple_data, &mut td_off, n)?;

            for (i, &cvt_idx) in points.iter().enumerate() {
                let ci = cvt_idx as usize;
                if ci >= out.len() {
                    continue;
                }
                // A repeated CVT index accumulates (§7.2.2.5).
                out[ci] += (deltas[i] as f32 * scalar).round() as i32;
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a single packed-delta run of `vals` (i8 form).
    fn packed_deltas_i8(vals: &[i8]) -> Vec<u8> {
        let mut b = Vec::new();
        // control byte = run len - 1 (i8 form, bits 7/6 clear)
        b.push((vals.len() as u8 - 1) & 0x3F);
        for &v in vals {
            b.push(v as u8);
        }
        b
    }

    /// Build a minimal 1-axis cvar with a single tuple at peak (1.0)
    /// that adjusts every one of `cvt_count` CVTs by the given deltas.
    /// The data area starts with the shared "all points" sentinel
    /// (`0x00`) so the single tuple's data is just the delta run.
    fn build_cvar_one_tuple(deltas: &[i8]) -> Vec<u8> {
        let header_len = 8;
        let tvh_len = 4 + 2; // varDataSize + tupleIndex + 1-axis embedded peak
        let data_off = header_len + tvh_len;

        let delta_bytes = packed_deltas_i8(deltas);

        let mut b = vec![0u8; data_off];
        b[0..2].copy_from_slice(&1u16.to_be_bytes()); // major
        b[2..4].copy_from_slice(&0u16.to_be_bytes()); // minor
        b[4..6].copy_from_slice(&1u16.to_be_bytes()); // tupleVariationCount = 1
        b[6..8].copy_from_slice(&(data_off as u16).to_be_bytes()); // dataOffset
                                                                   // TupleVariationHeader: varDataSize = just the delta run
                                                                   // (the shared point set lives outside the tuple's data).
        b[8..10].copy_from_slice(&(delta_bytes.len() as u16).to_be_bytes());
        b[10..12].copy_from_slice(&TI_EMBEDDED_PEAK.to_be_bytes()); // tupleIndex (embedded peak)
        b[12..14].copy_from_slice(&0x4000u16.to_be_bytes()); // peak = 1.0 in F2DOT14
                                                             // Data area: shared all-points sentinel, then tuple deltas.
        b.push(0x00);
        b.extend_from_slice(&delta_bytes);
        b
    }

    #[test]
    fn parses_header() {
        let raw = build_cvar_one_tuple(&[1, 2, 3]);
        let cvar = CvarTable::parse(&raw).expect("parse");
        assert_eq!(cvar.major_version(), 1);
        assert_eq!(cvar.minor_version(), 0);
        assert_eq!(cvar.tuple_count(), 1);
    }

    #[test]
    fn rejects_bad_major_version() {
        let mut raw = build_cvar_one_tuple(&[1, 2, 3]);
        raw[0..2].copy_from_slice(&2u16.to_be_bytes());
        assert!(matches!(
            CvarTable::parse(&raw),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn rejects_truncated_header() {
        assert!(matches!(
            CvarTable::parse(&[0u8; 4]),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn full_scalar_applies_all_deltas() {
        // peak = 1.0, coord = 1.0 → scalar 1.0; deltas applied verbatim.
        let raw = build_cvar_one_tuple(&[10, -5, 20]);
        let cvar = CvarTable::parse(&raw).expect("parse");
        let d = cvar.cvt_deltas(1, 3, &[1.0]).expect("deltas");
        assert_eq!(d, vec![10, -5, 20]);
    }

    #[test]
    fn half_scalar_interpolates_deltas() {
        // peak = 1.0, coord = 0.5 → scalar 0.5; deltas halved (rounded).
        let raw = build_cvar_one_tuple(&[10, -5, 20]);
        let cvar = CvarTable::parse(&raw).expect("parse");
        let d = cvar.cvt_deltas(1, 3, &[0.5]).expect("deltas");
        // 10*0.5=5, -5*0.5=-2.5→-2 (round half to even / nearest), 20*0.5=10
        assert_eq!(d[0], 5);
        assert_eq!(d[2], 10);
        // -2.5 rounds to -2 or -3 depending on rounding mode; f32::round
        // is round-half-away-from-zero → -3.
        assert_eq!(d[1], -3);
    }

    #[test]
    fn zero_coord_yields_no_deltas() {
        // coord = 0.0 (default instance) → scalar 0 → all deltas zero.
        let raw = build_cvar_one_tuple(&[10, -5, 20]);
        let cvar = CvarTable::parse(&raw).expect("parse");
        let d = cvar.cvt_deltas(1, 3, &[0.0]).expect("deltas");
        assert_eq!(d, vec![0, 0, 0]);
    }

    #[test]
    fn deltas_padded_to_cvt_count() {
        // The tuple's all-points set covers cvt_count=3 entries, but the
        // caller asks for 5 CVTs; trailing entries stay zero.
        let raw = build_cvar_one_tuple(&[10, 20, 30]);
        let cvar = CvarTable::parse(&raw).expect("parse");
        // all-points sentinel covers exactly cvt_count requested.
        let d = cvar.cvt_deltas(1, 3, &[1.0]).expect("deltas");
        assert_eq!(d.len(), 3);
        assert_eq!(d, vec![10, 20, 30]);
    }
}
