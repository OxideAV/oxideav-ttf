//! `kern` — legacy kerning table (predates GPOS).
//!
//! Round 1 supports only Format 0 subtables. Microsoft uses one header
//! variant (u16 version, u16 nTables) and Apple uses another (u32
//! version, u32 nTables); we sniff the first 16 bits to tell them apart.

use crate::parser::{read_i16, read_u16, read_u32};
use crate::Error;

#[derive(Debug, Clone)]
pub struct KernTable<'a> {
    /// All format-0 pair lists collected at parse time, sorted by
    /// `(left << 16 | right)` for binary search.
    pairs: Vec<KernPair>,
    _phantom: core::marker::PhantomData<&'a ()>,
}

#[derive(Debug, Clone, Copy)]
struct KernPair {
    key: u32,
    value: i16,
}

impl<'a> KernTable<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < 4 {
            return Err(Error::UnexpectedEof);
        }
        // Sniff version. Microsoft format: u16 version (== 0), u16
        // nTables. Apple format: u32 version (== 0x00010000), u32 nTables.
        let v0 = read_u16(bytes, 0)?;
        let (mut off, n_subtables) = if v0 == 0 {
            // Microsoft.
            let n = read_u16(bytes, 2)?;
            (4usize, n as u32)
        } else {
            // Apple — version field is u32 0x00010000 → first 16 bits == 0
            // already, so we never reach this branch. Fail safe.
            return Err(Error::BadStructure("kern: unsupported header variant"));
        };

        let mut pairs = Vec::new();
        for _ in 0..n_subtables {
            // Subtable header (Microsoft format):
            //   u16 version, u16 length, u16 coverage.
            // Coverage low byte: bit 0 = horizontal, bit 1 = minimum
            // (else kerning), bit 2 = cross-stream, bit 3 = override.
            // High byte: format (0..3).
            if off + 6 > bytes.len() {
                return Err(Error::UnexpectedEof);
            }
            let _sub_version = read_u16(bytes, off)?;
            let length = read_u16(bytes, off + 2)? as usize;
            let coverage = read_u16(bytes, off + 4)?;
            let format = (coverage >> 8) & 0xFF;
            // Sanity-check sub-table length so we always advance.
            if length < 6 || off + length > bytes.len() {
                // Malformed — bail out of the loop rather than spin.
                break;
            }
            let next_off = off + length;
            // Only horizontal kerning, only format 0, skip "minimum"
            // tables (those provide a floor, not a delta).
            let horizontal = (coverage & 1) != 0;
            let is_kerning = (coverage & 2) == 0;
            if format == 0 && horizontal && is_kerning {
                parse_format0(bytes, off + 6, &mut pairs)?;
            }
            off = next_off;
        }
        pairs.sort_by_key(|p| p.key);
        Ok(Self {
            pairs,
            _phantom: core::marker::PhantomData,
        })
    }

    /// Look up the kerning between an ordered glyph pair, in font units.
    /// Returns 0 when no rule matches.
    pub fn lookup(&self, left: u16, right: u16) -> i16 {
        let key = ((left as u32) << 16) | right as u32;
        match self.pairs.binary_search_by_key(&key, |p| p.key) {
            Ok(i) => self.pairs[i].value,
            Err(_) => 0,
        }
    }
}

fn parse_format0(bytes: &[u8], start: usize, out: &mut Vec<KernPair>) -> Result<(), Error> {
    // Format-0 subtable body:
    //   u16 nPairs, u16 searchRange/entrySelector/rangeShift (3 * u16 — ignored).
    //   nPairs * (u16 left, u16 right, FWord value).
    if start + 8 > bytes.len() {
        return Err(Error::UnexpectedEof);
    }
    let n_pairs = read_u16(bytes, start)? as usize;
    let mut p = start + 8;
    for _ in 0..n_pairs {
        if p + 6 > bytes.len() {
            return Err(Error::UnexpectedEof);
        }
        let l = read_u16(bytes, p)?;
        let r = read_u16(bytes, p + 2)?;
        let v = read_i16(bytes, p + 4)?;
        out.push(KernPair {
            key: ((l as u32) << 16) | r as u32,
            value: v,
        });
        p += 6;
    }
    Ok(())
}

#[allow(dead_code)]
fn read_u32_be(bytes: &[u8], off: usize) -> Result<u32, Error> {
    read_u32(bytes, off)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_kern_with_one_pair(l: u16, r: u16, v: i16) -> Vec<u8> {
        // Microsoft header.
        let mut t = vec![0u8; 4];
        t[0..2].copy_from_slice(&0u16.to_be_bytes()); // version
        t[2..4].copy_from_slice(&1u16.to_be_bytes()); // nTables
                                                      // Subtable (header 6 + body 8 + 1*6 = 20 bytes).
        let mut sub = vec![0u8; 20];
        sub[0..2].copy_from_slice(&0u16.to_be_bytes()); // sub-version
        sub[2..4].copy_from_slice(&20u16.to_be_bytes()); // length
                                                         // coverage = 0x0001 (horizontal, format 0)
        sub[4..6].copy_from_slice(&1u16.to_be_bytes());
        // body: nPairs=1
        sub[6..8].copy_from_slice(&1u16.to_be_bytes());
        // 6 bytes searchRange/entrySelector/rangeShift skipped
        sub[14..16].copy_from_slice(&l.to_be_bytes());
        sub[16..18].copy_from_slice(&r.to_be_bytes());
        sub[18..20].copy_from_slice(&v.to_be_bytes());
        t.extend_from_slice(&sub);
        t
    }

    #[test]
    fn round_trips_one_pair() {
        let bytes = build_kern_with_one_pair(38, 57, -100);
        let k = KernTable::parse(&bytes).unwrap();
        assert_eq!(k.lookup(38, 57), -100);
        assert_eq!(k.lookup(38, 58), 0);
    }
}
