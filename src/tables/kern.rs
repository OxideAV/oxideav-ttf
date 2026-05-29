//! `kern` — legacy kerning table (predates GPOS).
//!
//! Two on-disk header variants coexist:
//!
//! - **Microsoft / OpenType `kern`** (used by every Windows-authored
//!   TTF and most Adobe / Google fonts): `u16 version` followed by
//!   `u16 nTables`. The `version` field is `0`, so the first 16 bits
//!   of the table read as zero.
//! - **Apple `kern`** (used by macOS-bundled TTFs and most Apple-
//!   authored fonts): `u32 version` followed by `u32 nTables`. The
//!   `version` field is `0x00010000`, so the first 16 bits read as
//!   `0x0001` (NOT zero) — this is what distinguishes the two
//!   variants at parse time.
//!
//! Per-subtable layouts differ between the two variants. The
//! Microsoft per-subtable header is `u16 version, u16 length, u16
//! coverage` (coverage's high byte carries the format, low byte the
//! flags). Apple's per-subtable header is `u32 length, u16 coverage,
//! u16 tupleIndex` and its coverage byte order is mirrored (format in
//! the low byte, flags in the high byte) — the byte-level details
//! aren't fully covered by the staged spec docs, so this parser
//! accepts the Apple header at the table level but does not decode
//! the Apple subtable bodies; an Apple-headered `kern` parses as a
//! valid table with zero pairs (lookup → 0) rather than being
//! rejected outright. Round 1 + this round therefore expose
//! Microsoft-format Format-0 kerning only.

use crate::parser::{read_i16, read_u16, read_u32};
use crate::Error;

#[derive(Debug, Clone)]
pub struct KernTable<'a> {
    /// All format-0 pair lists collected at parse time, sorted by
    /// `(left << 16 | right)` for binary search.
    pairs: Vec<KernPair>,
    /// Which on-disk header variant the input used. Distinguishing
    /// the two at parse time matters because subtable layouts differ;
    /// the field is also surfaced via [`KernTable::header_variant`]
    /// for callers that want to know whether the source font ships an
    /// Apple-format table whose per-subtable bodies this crate does
    /// not decode.
    variant: HeaderVariant,
    _phantom: core::marker::PhantomData<&'a ()>,
}

/// Which `kern` header layout the input table uses. Exposed so callers
/// can tell apart Microsoft-format fonts (whose Format-0 subtables this
/// crate decodes) from Apple-format fonts (whose subtable bodies are
/// currently surfaced as "no kerning pairs available" rather than
/// rejected at parse time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderVariant {
    /// Microsoft / OpenType layout: `u16 version` (= 0), `u16 nTables`,
    /// then `nTables` subtables. Per-subtable header is `u16 version,
    /// u16 length, u16 coverage`. This crate decodes Format-0
    /// horizontal kerning subtables.
    Microsoft,
    /// Apple layout: `u32 version` (= 0x00010000), `u32 nTables`, then
    /// `nTables` subtables with a different per-subtable header
    /// layout. The subtable bodies are not decoded by this crate;
    /// callers that need Apple-kern data should hold the fixed Apple
    /// `kerx` clean-room reference and submit a follow-up.
    Apple,
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
        // Sniff version. Microsoft format: `u16 version` (= 0) — first
        // 16 bits read as 0. Apple format: `u32 version` (= 0x00010000,
        // big-endian → bytes 00 01 00 00) — first 16 bits read as
        // 0x0001 (NOT zero). The two are mutually exclusive at the
        // first u16: any other value is malformed.
        let v0 = read_u16(bytes, 0)?;
        let (mut off, n_subtables, variant) = match v0 {
            0 => {
                // Microsoft layout: u16 version, u16 nTables.
                let n = read_u16(bytes, 2)?;
                (4usize, n as u32, HeaderVariant::Microsoft)
            }
            1 => {
                // Apple layout: u32 version (= 0x00010000), u32 nTables.
                // Confirm the low half of the version u32 is also zero
                // to defuse fonts that mis-encode the field.
                if bytes.len() < 8 {
                    return Err(Error::UnexpectedEof);
                }
                let v_lo = read_u16(bytes, 2)?;
                if v_lo != 0 {
                    return Err(Error::BadStructure("kern: bad version"));
                }
                let n = read_u32(bytes, 4)?;
                (8usize, n, HeaderVariant::Apple)
            }
            _ => return Err(Error::BadStructure("kern: bad version")),
        };

        let mut pairs = Vec::new();
        if matches!(variant, HeaderVariant::Apple) {
            // Apple per-subtable layout is not covered by the spec docs
            // staged under `docs/text/opentype/`. Accept the table
            // structurally (so the host font still parses) but do not
            // walk the subtable list — the `length` field placement
            // differs from the Microsoft variant and a mis-parsed walk
            // would either fabricate bogus pairs or panic.
            let _ = n_subtables;
            let _ = off;
            return Ok(Self {
                pairs,
                variant,
                _phantom: core::marker::PhantomData,
            });
        }
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
            variant,
            _phantom: core::marker::PhantomData,
        })
    }

    /// Which on-disk header layout the input table used. Useful for
    /// callers that want to report "this font ships an Apple-format
    /// `kern` whose subtable bodies are not decoded".
    pub fn header_variant(&self) -> HeaderVariant {
        self.variant
    }

    /// Number of decoded kerning pairs available for [`Self::lookup`].
    /// Returns `0` for Apple-headered tables (whose subtable bodies
    /// this crate does not decode) and for Microsoft-headered tables
    /// that ship only non-horizontal / non-Format-0 subtables.
    pub fn pair_count(&self) -> usize {
        self.pairs.len()
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
        assert_eq!(k.header_variant(), HeaderVariant::Microsoft);
        assert_eq!(k.pair_count(), 1);
    }

    /// Apple-format `kern` (the layout shipped by every macOS-bundled
    /// `.ttf` — Helvetica, Lucida, Times, etc.). The previous version
    /// of the header sniffer matched both Microsoft and Apple on
    /// `first u16 == 0` and dispatched both into the Microsoft body
    /// walker; Apple's u32-wide `version` field has high u16 = `0x0001`
    /// (NOT zero), so the correct dispatch picks it up here, accepts
    /// the table without rejecting the host font, and exposes zero
    /// kerning pairs (the subtable body layout differs from the
    /// Microsoft variant and isn't decoded by this crate yet).
    #[test]
    fn apple_header_parses_as_empty_table() {
        let mut bytes = vec![0u8; 8];
        // u32 version = 0x00010000 (big-endian bytes 00 01 00 00).
        bytes[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        // u32 nTables = 0.
        bytes[4..8].copy_from_slice(&0u32.to_be_bytes());
        let k = KernTable::parse(&bytes).unwrap();
        assert_eq!(k.header_variant(), HeaderVariant::Apple);
        assert_eq!(k.pair_count(), 0);
        // Any lookup returns the no-data sentinel (0), so consumer-
        // crate shapers degrade to "no legacy kerning" rather than
        // panicking on an out-of-bounds slice into a misparsed body.
        assert_eq!(k.lookup(38, 57), 0);
        assert_eq!(k.lookup(0, 0), 0);
    }

    /// An Apple-headered table that claims a non-zero subtable count
    /// also parses cleanly: this crate doesn't walk the Apple subtable
    /// list so the bogus nTables field is harmless. The point of the
    /// test is to prove the header sniff doesn't crash on the field —
    /// real-world Apple `kern` tables routinely list 2-3 subtables.
    #[test]
    fn apple_header_with_nonzero_n_tables_parses() {
        let mut bytes = vec![0u8; 8];
        bytes[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        bytes[4..8].copy_from_slice(&3u32.to_be_bytes());
        let k = KernTable::parse(&bytes).unwrap();
        assert_eq!(k.header_variant(), HeaderVariant::Apple);
        assert_eq!(k.pair_count(), 0);
    }

    /// Truncated Apple header — version reads as 0x0001 but the table
    /// ends before the u32 nTables field. The parser must surface
    /// `UnexpectedEof` instead of indexing out of bounds.
    #[test]
    fn apple_header_truncated_returns_eof() {
        // Only 4 bytes — high half of the version is there (forcing
        // the Apple branch), but nTables and the rest are missing.
        let mut bytes = vec![0u8; 4];
        bytes[0..2].copy_from_slice(&0x0001u16.to_be_bytes());
        bytes[2..4].copy_from_slice(&0u16.to_be_bytes()); // version low half
        assert!(matches!(
            KernTable::parse(&bytes),
            Err(Error::UnexpectedEof)
        ));
    }

    /// A first-u16 sentinel that's neither 0 (Microsoft) nor 0x0001
    /// (Apple's version high-half) is malformed. Reject with a typed
    /// `BadStructure` rather than mis-dispatching into one of the two
    /// walkers and corrupting state.
    #[test]
    fn unknown_version_rejected() {
        let mut bytes = vec![0u8; 8];
        bytes[0..2].copy_from_slice(&0x1234u16.to_be_bytes());
        let r = KernTable::parse(&bytes);
        assert!(matches!(r, Err(Error::BadStructure(_))));
    }

    /// Apple version high-half matches (0x0001) but the low half of
    /// the u32 version is non-zero — i.e. the value on disk is some
    /// 0x0001XXXX where XXXX != 0. The real Apple `kern` table version
    /// is exactly 0x00010000, so anything else is malformed and we
    /// reject it as a structural error rather than dispatching into
    /// the Apple body path.
    #[test]
    fn apple_header_with_dirty_low_half_rejected() {
        let mut bytes = vec![0u8; 8];
        bytes[0..2].copy_from_slice(&0x0001u16.to_be_bytes());
        bytes[2..4].copy_from_slice(&0xBEEFu16.to_be_bytes()); // dirty low half
        bytes[4..8].copy_from_slice(&0u32.to_be_bytes());
        assert!(matches!(
            KernTable::parse(&bytes),
            Err(Error::BadStructure(_))
        ));
    }
}
