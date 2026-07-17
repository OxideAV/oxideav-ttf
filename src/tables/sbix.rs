//! `sbix` — Standard Bitmap Graphics Table.
//!
//! Apple's container for per-glyph PNG/JPEG/TIFF bitmap strikes — the
//! original colour-emoji format, used by Apple Color Emoji and most
//! macOS / iOS emoji fonts. Functionally similar to CBDT/CBLC but
//! self-contained: every strike is a single contiguous block with a
//! header + an `(numGlyphs+1)`-element offset array + the per-glyph
//! bitmap blobs (header + payload).
//!
//! Spec: Microsoft OpenType §"sbix — Standard Bitmap Graphics Table"
//! / Apple TrueType Reference §"sbix". The 'sbix' table version is
//! always 1.
//!
//! ## Header layout
//!
//! ```text
//! Offset  Field                    Type             Notes
//! ------  ----------------------   ---------------  --------------------
//!  +0     version                  uint16           = 1
//!  +2     flags                    uint16           bit 0 always set,
//!                                                   bit 1 = "draw outlines"
//!  +4     numStrikes               uint32           strike count
//!  +8     strikeOffsets[N]         Offset32[N]      from start of sbix
//! ```
//!
//! ## Strike layout (each strike at `bytes[strikeOffsets[i]..]`)
//!
//! ```text
//! +0     ppem                     uint16           target ppem
//! +2     ppi                      uint16           device PPI
//! +4     glyphDataOffsets         Offset32[numGlyphs+1]
//!                                                   offsets RELATIVE TO
//!                                                   the strike header
//!                                                   start (NOT sbix start)
//! ```
//!
//! ## Per-glyph blob layout (at `strike + glyphDataOffsets[gid]`)
//!
//! ```text
//! +0     originOffsetX            int16
//! +2     originOffsetY            int16
//! +4     graphicType              Tag (4 bytes)    'png ', 'jpg ',
//!                                                   'tiff', or 'dupe'
//! +8     data                     uint8[]          length =
//!                                                   glyphDataOffsets[gid+1]
//!                                                   - glyphDataOffsets[gid]
//!                                                   - 8
//! ```
//!
//! When the per-glyph length is 0 there's no bitmap for that glyph in
//! this strike (consumers should fall through to the next strike).
//! Special graphicType `'dupe'` means "use the bitmap of glyph N
//! instead", where N is a big-endian u16 in the 2-byte data payload.
//! This crate exposes the raw `'dupe'` entry as-is via
//! [`SbixTable::glyph`] (so byte-level consumers can introspect the
//! indirection target without follow-up work) *and* provides
//! [`SbixTable::resolve_dupe_chain`] /
//! [`SbixTable::lookup_best_fit_resolved`] that follow the
//! indirection within the same strike up to [`MAX_DUPE_DEPTH`] hops
//! with explicit cycle detection — every visited glyph id is recorded
//! and revisiting one bails to `None` rather than recursing forever.

use crate::parser::{read_i16, read_u16, read_u32};
use crate::Error;

/// Maximum number of `'dupe'` hops chased by
/// [`SbixTable::resolve_dupe_chain`] / [`SbixTable::lookup_best_fit_resolved`]
/// before the resolver gives up. Real-world fonts use a single hop (or
/// none); the cap exists purely to defuse pathological / hostile
/// indirection chains that don't form a strict cycle (e.g. A→B→C→…).
pub const MAX_DUPE_DEPTH: usize = 8;

/// One per-glyph bitmap entry resolved out of a strike.
#[derive(Debug, Clone, Copy)]
pub struct SbixGlyph<'a> {
    /// 4-byte format tag, e.g. `*b"png "`, `*b"jpg "`, `*b"tiff"`,
    /// or `*b"dupe"` (the indirection sentinel — `bytes` is then a
    /// 2-byte big-endian glyph id).
    pub graphic_type: [u8; 4],
    /// Raw graphic blob (PNG / JPEG / TIFF / 2-byte u16 for `dupe`).
    /// Borrows from the parent sbix slice.
    pub bytes: &'a [u8],
    /// Horizontal pen-origin → bitmap-left-edge in font units.
    pub origin_x: i16,
    /// Pen-origin → bitmap-bottom-edge in font units (Y up).
    pub origin_y: i16,
}

impl<'a> SbixGlyph<'a> {
    /// `true` when this entry is a `'dupe'` indirection sentinel
    /// pointing at another glyph id within the same strike. The
    /// indirect glyph id is in [`Self::dupe_target`].
    pub fn is_dupe(&self) -> bool {
        &self.graphic_type == b"dupe"
    }

    /// Decode the `'dupe'` payload as a big-endian u16 glyph id and
    /// return it; returns `None` when this entry isn't a `'dupe'`
    /// or its payload is shorter than 2 bytes (malformed).
    pub fn dupe_target(&self) -> Option<u16> {
        if !self.is_dupe() {
            return None;
        }
        if self.bytes.len() < 2 {
            return None;
        }
        Some(u16::from_be_bytes([self.bytes[0], self.bytes[1]]))
    }
}

/// Parsed sbix table walker.
#[derive(Debug, Clone)]
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub struct SbixTable<'a> {
    bytes: &'a [u8],
    /// Number of strikes (deduplicated against `numGlyphs` for the
    /// per-strike offset array length).
    num_strikes: u32,
    /// Per-strike file-relative offsets into `bytes`.
    strike_offsets: Vec<u32>,
    /// Cached `maxp.numGlyphs` so we know how big each strike's
    /// `glyphDataOffsets` array is.
    num_glyphs: u16,
}

impl<'a> SbixTable<'a> {
    /// Validate the header + the per-strike offset array. The
    /// per-strike `glyphDataOffsets` arrays are walked lazily by
    /// [`Self::strike_ppem`] / [`Self::glyph`].
    ///
    /// `num_glyphs` is `maxp.numGlyphs` from the parent font — every
    /// strike's offset array is `numGlyphs + 1` entries long per the
    /// spec.
    pub fn parse(bytes: &'a [u8], num_glyphs: u16) -> Result<Self, Error> {
        if bytes.len() < 8 {
            return Err(Error::UnexpectedEof);
        }
        let version = read_u16(bytes, 0)?;
        if version != 1 {
            return Err(Error::BadStructure("sbix: version must be 1"));
        }
        // flags at +2; we don't act on them.
        let _flags = read_u16(bytes, 2)?;
        let num_strikes = read_u32(bytes, 4)?;
        // Sanity cap — real fonts ship at most a couple of dozen
        // strikes; anything > 4096 is almost certainly garbage.
        if num_strikes > 4096 {
            return Err(Error::BadStructure("sbix: numStrikes implausibly large"));
        }
        let table_end = 8u64 + num_strikes as u64 * 4;
        if table_end > bytes.len() as u64 {
            return Err(Error::UnexpectedEof);
        }
        let mut strike_offsets = Vec::with_capacity(num_strikes as usize);
        let strike_data_min = 4u64 + (num_glyphs as u64 + 1) * 4;
        for i in 0..num_strikes as usize {
            let off = read_u32(bytes, 8 + i * 4)?;
            // The strike must fit the fixed-size header (4 B) + the
            // (numGlyphs+1)-entry offset array.
            let end = (off as u64)
                .checked_add(strike_data_min)
                .ok_or(Error::BadOffset)?;
            if end > bytes.len() as u64 {
                return Err(Error::BadOffset);
            }
            strike_offsets.push(off);
        }

        Ok(Self {
            bytes,
            num_strikes,
            strike_offsets,
            num_glyphs,
        })
    }

    /// Number of strikes the table ships.
    pub fn num_strikes(&self) -> u32 {
        self.num_strikes
    }

    /// `ppem` of strike `strike_index`. Returns `None` when out of
    /// range.
    pub fn strike_ppem(&self, strike_index: u32) -> Option<u16> {
        let off = *self.strike_offsets.get(strike_index as usize)? as usize;
        read_u16(self.bytes, off).ok()
    }

    /// `(ppem, ppi)` of strike `strike_index`.
    pub fn strike_size(&self, strike_index: u32) -> Option<(u16, u16)> {
        let off = *self.strike_offsets.get(strike_index as usize)? as usize;
        let ppem = read_u16(self.bytes, off).ok()?;
        let ppi = read_u16(self.bytes, off + 2).ok()?;
        Some((ppem, ppi))
    }

    /// All strike ppem values, in declaration order. Some fonts ship
    /// duplicate ppems (for different PPI targets); de-duping is the
    /// caller's responsibility. (Most callers use
    /// [`Self::all_ppems_unique_sorted`] instead.)
    pub fn all_ppems(&self) -> Vec<u16> {
        (0..self.num_strikes)
            .filter_map(|i| self.strike_ppem(i))
            .collect()
    }

    /// All strike ppem values de-duped and sorted ascending. This is
    /// the canonical "what strike sizes does this font ship?" view.
    pub fn all_ppems_unique_sorted(&self) -> Vec<u16> {
        let mut v = self.all_ppems();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Resolve `glyph_id`'s bitmap entry inside strike `strike_index`.
    /// Returns `None` when:
    /// - `strike_index >= numStrikes`,
    /// - `glyph_id >= numGlyphs` (cached at parse time),
    /// - this strike has no bitmap for the glyph (the
    ///   `glyphDataOffsets[gid] == glyphDataOffsets[gid+1]` zero-
    ///   length case).
    pub fn glyph(&self, strike_index: u32, glyph_id: u16) -> Option<SbixGlyph<'a>> {
        if glyph_id >= self.num_glyphs {
            return None;
        }
        let strike_off = *self.strike_offsets.get(strike_index as usize)? as usize;
        // glyphDataOffsets array is at strike_off + 4.
        let g = glyph_id as usize;
        let off_lo = read_u32(self.bytes, strike_off + 4 + g * 4).ok()?;
        let off_hi = read_u32(self.bytes, strike_off + 4 + (g + 1) * 4).ok()?;
        if off_lo == off_hi {
            // Zero-length: no bitmap for this glyph in this strike.
            return None;
        }
        // The two offsets are RELATIVE TO THE STRIKE HEADER START
        // (not the sbix table start).
        let abs_lo = strike_off.checked_add(off_lo as usize)?;
        let abs_hi = strike_off.checked_add(off_hi as usize)?;
        if abs_hi > self.bytes.len() || abs_hi < abs_lo + 8 {
            return None;
        }
        let blob = &self.bytes[abs_lo..abs_hi];
        let origin_x = read_i16(blob, 0).ok()?;
        let origin_y = read_i16(blob, 2).ok()?;
        let mut graphic_type = [0u8; 4];
        graphic_type.copy_from_slice(&blob[4..8]);
        let data = &blob[8..];
        Some(SbixGlyph {
            graphic_type,
            bytes: data,
            origin_x,
            origin_y,
        })
    }

    /// Follow a `'dupe'` indirection chain inside a single strike,
    /// returning the first non-`'dupe'` entry reachable from
    /// `glyph_id`. Returns `None` when:
    /// - `glyph_id` has no entry in this strike (zero-length / OOB),
    /// - the chain length exceeds [`MAX_DUPE_DEPTH`],
    /// - the chain hits a cycle (a previously-visited glyph id is
    ///   revisited),
    /// - a `'dupe'` payload is malformed (< 2 bytes), or
    /// - a `'dupe'` target glyph id is out of range or itself absent
    ///   from the strike.
    ///
    /// A non-`'dupe'` entry is returned untouched on the first hop.
    pub fn resolve_dupe_chain(&self, strike_index: u32, glyph_id: u16) -> Option<SbixGlyph<'a>> {
        // Tiny stack-backed visited set — MAX_DUPE_DEPTH is small enough
        // that a linear scan beats hashing.
        let mut visited: [u16; MAX_DUPE_DEPTH] = [0; MAX_DUPE_DEPTH];
        let mut visited_len = 0usize;
        let mut cur = glyph_id;
        for _ in 0..MAX_DUPE_DEPTH {
            // Cycle check before stepping.
            if visited[..visited_len].contains(&cur) {
                return None;
            }
            if visited_len < MAX_DUPE_DEPTH {
                visited[visited_len] = cur;
                visited_len += 1;
            }
            let entry = self.glyph(strike_index, cur)?;
            if !entry.is_dupe() {
                return Some(entry);
            }
            cur = entry.dupe_target()?;
        }
        None
    }

    /// Best-fit lookup that also resolves `'dupe'` indirection within
    /// the chosen strike. Picks the closest-ppem strike that has *any*
    /// entry for `glyph_id` (per [`Self::lookup_best_fit`]), then
    /// follows [`Self::resolve_dupe_chain`] inside that strike.
    ///
    /// Returns `None` under the same conditions as
    /// [`Self::lookup_best_fit`] *plus* the chain-resolution failure
    /// cases enumerated on [`Self::resolve_dupe_chain`].
    pub fn lookup_best_fit_resolved(
        &self,
        glyph_id: u16,
        target_ppem: u16,
    ) -> Option<SbixGlyph<'a>> {
        // Walk strikes ourselves so we know the picked strike index for
        // the dupe chase. Same closest-ppem-larger-tiebreak policy as
        // `lookup_best_fit`.
        let mut best: Option<(u32, u32)> = None; // (strike_idx, |ppem-target|)
        for i in 0..self.num_strikes {
            let ppem = match self.strike_ppem(i) {
                Some(p) => p,
                None => continue,
            };
            if self.glyph(i, glyph_id).is_none() {
                continue;
            }
            let dist = (ppem as i32 - target_ppem as i32).unsigned_abs();
            match best {
                None => best = Some((i, dist)),
                Some((bi, bd)) => {
                    if dist < bd
                        || (dist == bd
                            && self.strike_ppem(i).unwrap_or(0) > self.strike_ppem(bi).unwrap_or(0))
                    {
                        best = Some((i, dist));
                    }
                }
            }
        }
        let (strike_idx, _) = best?;
        self.resolve_dupe_chain(strike_idx, glyph_id)
    }

    /// Best-fit lookup: return the bitmap for `glyph_id` from the
    /// strike whose ppem is closest to `target_ppem`, scanning all
    /// strikes. When two strikes are equidistant we pick the larger
    /// (matches the spec recommendation: "implementations may choose
    /// a bitmap based on the closest available larger size"). Returns
    /// `None` if no strike covers the glyph.
    pub fn lookup_best_fit(&self, glyph_id: u16, target_ppem: u16) -> Option<SbixGlyph<'a>> {
        let mut best: Option<(u32, u32)> = None; // (strike_idx, |ppem-target|)
        for i in 0..self.num_strikes {
            let ppem = match self.strike_ppem(i) {
                Some(p) => p,
                None => continue,
            };
            // Skip strikes that don't cover this glyph.
            if self.glyph(i, glyph_id).is_none() {
                continue;
            }
            let dist = (ppem as i32 - target_ppem as i32).unsigned_abs();
            match best {
                None => best = Some((i, dist)),
                Some((bi, bd)) => {
                    if dist < bd
                        || (dist == bd
                            && self.strike_ppem(i).unwrap_or(0) > self.strike_ppem(bi).unwrap_or(0))
                    {
                        best = Some((i, dist));
                    }
                }
            }
        }
        best.and_then(|(i, _)| self.glyph(i, glyph_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-build a minimal sbix table with 2 strikes (32ppem,
    /// 64ppem), each covering 3 glyphs.
    fn synth_sbix(num_glyphs: u16) -> Vec<u8> {
        // Layout:
        //   sbix header  : 8 + 2*4   = 16 B
        //   strike #0    : 4 + (n+1)*4 + glyph blobs
        //   strike #1    : same
        //
        // For 3 glyphs each with a 5-byte payload (8-byte glyph header
        // + 5 = 13 bytes per glyph blob), strike data area = 3*13 =
        // 39 B. Strike header (with offsets) is 4 + 4*4 = 20 B. Strike
        // total = 59 B.
        assert_eq!(num_glyphs, 3);
        let strike_header_len = 4 + (num_glyphs as usize + 1) * 4; // 20
        let glyph_payload = 5usize;
        let glyph_blob = 8 + glyph_payload; // 13
        let strike_data = num_glyphs as usize * glyph_blob; // 39
        let strike_total = strike_header_len + strike_data; // 59

        let header_len = 8 + 2 * 4; // 16
        let strike0 = header_len; // 16
        let strike1 = strike0 + strike_total; // 75
        let total = strike1 + strike_total; // 134

        let mut bytes = vec![0u8; total];

        // Header
        bytes[0..2].copy_from_slice(&1u16.to_be_bytes()); // version
        bytes[2..4].copy_from_slice(&1u16.to_be_bytes()); // flags (bit 0 always set)
        bytes[4..8].copy_from_slice(&2u32.to_be_bytes()); // numStrikes
        bytes[8..12].copy_from_slice(&(strike0 as u32).to_be_bytes());
        bytes[12..16].copy_from_slice(&(strike1 as u32).to_be_bytes());

        // Strike 0: 32ppem, 96ppi
        bytes[strike0..strike0 + 2].copy_from_slice(&32u16.to_be_bytes());
        bytes[strike0 + 2..strike0 + 4].copy_from_slice(&96u16.to_be_bytes());
        // glyphDataOffsets[0..=3] all relative to strike start.
        // First glyph blob lives at strike_header_len = 20.
        for i in 0..=num_glyphs as usize {
            let off = strike_header_len + i * glyph_blob;
            let dst = strike0 + 4 + i * 4;
            bytes[dst..dst + 4].copy_from_slice(&(off as u32).to_be_bytes());
        }
        // Glyph blobs
        for g in 0..num_glyphs as usize {
            let blob_off = strike0 + strike_header_len + g * glyph_blob;
            // originOffsetX = g+1, originOffsetY = -(g+10)
            bytes[blob_off..blob_off + 2].copy_from_slice(&((g as i16 + 1) as u16).to_be_bytes());
            bytes[blob_off + 2..blob_off + 4]
                .copy_from_slice(&(-(g as i16 + 10) as u16).to_be_bytes());
            bytes[blob_off + 4..blob_off + 8].copy_from_slice(b"png ");
            // payload: 5 bytes, gid-tagged
            bytes[blob_off + 8..blob_off + 13].copy_from_slice(&[g as u8, 0xAA, 0xBB, 0xCC, 0xDD]);
        }

        // Strike 1: 64ppem, 192ppi — same shape, different payload.
        bytes[strike1..strike1 + 2].copy_from_slice(&64u16.to_be_bytes());
        bytes[strike1 + 2..strike1 + 4].copy_from_slice(&192u16.to_be_bytes());
        for i in 0..=num_glyphs as usize {
            let off = strike_header_len + i * glyph_blob;
            let dst = strike1 + 4 + i * 4;
            bytes[dst..dst + 4].copy_from_slice(&(off as u32).to_be_bytes());
        }
        for g in 0..num_glyphs as usize {
            let blob_off = strike1 + strike_header_len + g * glyph_blob;
            bytes[blob_off..blob_off + 2].copy_from_slice(&((g as i16 + 100) as u16).to_be_bytes());
            bytes[blob_off + 2..blob_off + 4]
                .copy_from_slice(&(-(g as i16 + 50) as u16).to_be_bytes());
            bytes[blob_off + 4..blob_off + 8].copy_from_slice(b"png ");
            bytes[blob_off + 8..blob_off + 13].copy_from_slice(&[
                g as u8 ^ 0xFF,
                0x11,
                0x22,
                0x33,
                0x44,
            ]);
        }

        bytes
    }

    #[test]
    fn parses_header_and_strikes() {
        let bytes = synth_sbix(3);
        let sbix = SbixTable::parse(&bytes, 3).expect("parse");
        assert_eq!(sbix.num_strikes(), 2);
        assert_eq!(sbix.strike_ppem(0), Some(32));
        assert_eq!(sbix.strike_ppem(1), Some(64));
        assert_eq!(sbix.strike_ppem(2), None);
        assert_eq!(sbix.strike_size(0), Some((32, 96)));
        assert_eq!(sbix.strike_size(1), Some((64, 192)));
    }

    #[test]
    fn glyph_lookup_strike0() {
        let bytes = synth_sbix(3);
        let sbix = SbixTable::parse(&bytes, 3).expect("parse");
        for g in 0u16..3 {
            let entry = sbix.glyph(0, g).expect("entry");
            assert_eq!(entry.graphic_type, *b"png ");
            assert_eq!(entry.origin_x, g as i16 + 1);
            assert_eq!(entry.origin_y, -(g as i16 + 10));
            assert_eq!(entry.bytes.len(), 5);
            assert_eq!(entry.bytes[0], g as u8);
        }
    }

    #[test]
    fn glyph_lookup_strike1() {
        let bytes = synth_sbix(3);
        let sbix = SbixTable::parse(&bytes, 3).expect("parse");
        let entry = sbix.glyph(1, 2).expect("entry");
        assert_eq!(entry.origin_x, 102);
        assert_eq!(entry.origin_y, -52);
        assert_eq!(entry.bytes[0], 2u8 ^ 0xFF);
    }

    #[test]
    fn out_of_range_returns_none() {
        let bytes = synth_sbix(3);
        let sbix = SbixTable::parse(&bytes, 3).expect("parse");
        assert!(sbix.glyph(0, 99).is_none()); // gid out of range
        assert!(sbix.glyph(99, 0).is_none()); // strike out of range
    }

    #[test]
    fn rejects_bad_version() {
        let mut bytes = vec![0u8; 16];
        bytes[0..2].copy_from_slice(&2u16.to_be_bytes()); // version 2 not allowed
        bytes[4..8].copy_from_slice(&0u32.to_be_bytes());
        assert!(matches!(
            SbixTable::parse(&bytes, 0),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn ppems_unique_sorted() {
        let bytes = synth_sbix(3);
        let sbix = SbixTable::parse(&bytes, 3).expect("parse");
        assert_eq!(sbix.all_ppems_unique_sorted(), vec![32u16, 64u16]);
    }

    #[test]
    fn best_fit_picks_closest_strike() {
        let bytes = synth_sbix(3);
        let sbix = SbixTable::parse(&bytes, 3).expect("parse");
        // target 30 -> strike 0 (32ppem, dist 2 vs strike 1's 34)
        let e = sbix.lookup_best_fit(0, 30).expect("entry");
        assert_eq!(e.bytes[0], 0u8);
        // target 70 -> strike 1 (64ppem)
        let e = sbix.lookup_best_fit(0, 70).expect("entry");
        assert_eq!(e.bytes[0], 0xFFu8);
        // target 48 -> equidistant; spec-recommended choice is the
        // larger size, so strike 1.
        let e = sbix.lookup_best_fit(1, 48).expect("entry");
        assert_eq!(e.bytes[0], 1u8 ^ 0xFF);
    }

    /// A strike where glyph 1 has a zero-length entry (no bitmap).
    #[test]
    fn zero_length_entry_returns_none() {
        let bytes = {
            let mut b = synth_sbix(3);
            // Tweak strike 0's offset[2] = offset[1] to make glyph 1
            // zero-length, then bump the rest by -13 to keep the
            // table consistent. Instead, simpler: just collapse
            // offsets[1] and offsets[2] to the same value.
            // Strike 0 at offset 16; offsets array starts at 16+4=20.
            let strike0 = 16usize;
            // Get offsets[2] (current = 20 + 2*13 = 46? wait, the
            // strike-relative value, not absolute) — read what we
            // just wrote.
            let off1 = read_u32(&b, strike0 + 4 + 4).unwrap();
            // Set offsets[2] := off1 (zero-length glyph 1).
            b[strike0 + 4 + 2 * 4..strike0 + 4 + 2 * 4 + 4].copy_from_slice(&off1.to_be_bytes());
            b
        };
        let sbix = SbixTable::parse(&bytes, 3).expect("parse");
        assert!(sbix.glyph(0, 1).is_none());
        // Other glyphs in same strike still resolve.
        assert!(sbix.glyph(0, 0).is_some());
    }

    // ---- 'dupe' indirection ---------------------------------------------

    /// Build a single-strike sbix where every glyph's blob is laid out
    /// at offset = `strike_header_len + g * blob_len`. Each entry's
    /// `graphic_type` and 2-byte payload are filled from `entries[g]`,
    /// where the first 4 bytes of the tuple slice are the graphic tag
    /// and the next 2 bytes are the payload (`'dupe'` => big-endian
    /// target glyph id; everything else => raw bitmap payload).
    fn synth_sbix_with_entries(entries: &[([u8; 4], [u8; 2])]) -> Vec<u8> {
        let num_glyphs = entries.len() as u16;
        let strike_header_len = 4 + (num_glyphs as usize + 1) * 4;
        let payload_len = 2usize;
        let blob_len = 8 + payload_len;
        let strike_data = num_glyphs as usize * blob_len;
        let strike_total = strike_header_len + strike_data;

        let header_len = 8 + 4; // 1 strike → 1 offset
        let strike0 = header_len;
        let total = strike0 + strike_total;

        let mut bytes = vec![0u8; total];
        bytes[0..2].copy_from_slice(&1u16.to_be_bytes()); // version
        bytes[2..4].copy_from_slice(&1u16.to_be_bytes()); // flags
        bytes[4..8].copy_from_slice(&1u32.to_be_bytes()); // numStrikes
        bytes[8..12].copy_from_slice(&(strike0 as u32).to_be_bytes());

        // Strike 0: 32ppem / 96ppi.
        bytes[strike0..strike0 + 2].copy_from_slice(&32u16.to_be_bytes());
        bytes[strike0 + 2..strike0 + 4].copy_from_slice(&96u16.to_be_bytes());
        for i in 0..=num_glyphs as usize {
            let off = strike_header_len + i * blob_len;
            let dst = strike0 + 4 + i * 4;
            bytes[dst..dst + 4].copy_from_slice(&(off as u32).to_be_bytes());
        }
        for (g, (tag, payload)) in entries.iter().enumerate() {
            let blob_off = strike0 + strike_header_len + g * blob_len;
            // origin_x / origin_y = 0 here; the dupe tests don't care.
            bytes[blob_off + 4..blob_off + 8].copy_from_slice(tag);
            bytes[blob_off + 8..blob_off + 10].copy_from_slice(payload);
        }
        bytes
    }

    #[test]
    fn dupe_predicates_decode_target_glyph_id() {
        // glyph 0: real png, glyph 1: dupe -> 0
        let bytes = synth_sbix_with_entries(&[(*b"png ", [0x12, 0x34]), (*b"dupe", [0x00, 0x00])]);
        let sbix = SbixTable::parse(&bytes, 2).expect("parse");
        let g0 = sbix.glyph(0, 0).unwrap();
        let g1 = sbix.glyph(0, 1).unwrap();
        assert!(!g0.is_dupe());
        assert_eq!(g0.dupe_target(), None);
        assert!(g1.is_dupe());
        assert_eq!(g1.dupe_target(), Some(0));
    }

    #[test]
    fn resolve_dupe_chain_follows_indirection_to_real_entry() {
        // 0:png, 1:dupe->0, 2:dupe->1 (chain of two hops resolves to 0).
        let bytes = synth_sbix_with_entries(&[
            (*b"png ", [0xAA, 0xBB]),
            (*b"dupe", [0x00, 0x00]),
            (*b"dupe", [0x00, 0x01]),
        ]);
        let sbix = SbixTable::parse(&bytes, 3).expect("parse");
        // Direct entry resolves to itself.
        let r0 = sbix.resolve_dupe_chain(0, 0).expect("entry");
        assert_eq!(&r0.graphic_type, b"png ");
        // One-hop chain.
        let r1 = sbix.resolve_dupe_chain(0, 1).expect("entry");
        assert_eq!(&r1.graphic_type, b"png ");
        assert_eq!(r1.bytes[0..2], [0xAA, 0xBB]);
        // Two-hop chain.
        let r2 = sbix.resolve_dupe_chain(0, 2).expect("entry");
        assert_eq!(&r2.graphic_type, b"png ");
    }

    #[test]
    fn resolve_dupe_chain_detects_two_glyph_cycle() {
        // 0:dupe->1, 1:dupe->0 — A↔B cycle.
        let bytes = synth_sbix_with_entries(&[(*b"dupe", [0x00, 0x01]), (*b"dupe", [0x00, 0x00])]);
        let sbix = SbixTable::parse(&bytes, 2).expect("parse");
        assert!(sbix.resolve_dupe_chain(0, 0).is_none());
        assert!(sbix.resolve_dupe_chain(0, 1).is_none());
    }

    #[test]
    fn resolve_dupe_chain_detects_self_cycle() {
        // 0:dupe->0 — self-loop.
        let bytes = synth_sbix_with_entries(&[(*b"dupe", [0x00, 0x00])]);
        let sbix = SbixTable::parse(&bytes, 1).expect("parse");
        assert!(sbix.resolve_dupe_chain(0, 0).is_none());
    }

    #[test]
    fn resolve_dupe_chain_caps_depth() {
        // Build a longest-possible non-cyclic chain that still exceeds
        // MAX_DUPE_DEPTH: 0->1->2->...->MAX_DUPE_DEPTH where the final
        // entry is itself a dupe. resolve_dupe_chain must give up
        // before reaching a real entry.
        let n = MAX_DUPE_DEPTH + 1;
        let mut entries: Vec<([u8; 4], [u8; 2])> = Vec::with_capacity(n);
        for g in 0..n - 1 {
            let next = (g + 1) as u16;
            entries.push((*b"dupe", next.to_be_bytes()));
        }
        // Last entry is also a dupe pointing forward off-chain at
        // a non-existent glyph — but resolve must time out first.
        entries.push((*b"dupe", (n as u16 + 100).to_be_bytes()));
        let bytes = synth_sbix_with_entries(&entries);
        let sbix = SbixTable::parse(&bytes, n as u16).expect("parse");
        assert!(sbix.resolve_dupe_chain(0, 0).is_none());
    }

    #[test]
    fn resolve_dupe_chain_returns_none_for_oob_dupe_target() {
        // 0:dupe->99 (99 is past num_glyphs=1).
        let bytes = synth_sbix_with_entries(&[(*b"dupe", [0x00, 0x63])]);
        let sbix = SbixTable::parse(&bytes, 1).expect("parse");
        assert!(sbix.resolve_dupe_chain(0, 0).is_none());
    }

    #[test]
    fn lookup_best_fit_resolved_walks_through_dupe() {
        // 0:png (payload AB CD), 1:dupe->0.
        let bytes = synth_sbix_with_entries(&[(*b"png ", [0xAB, 0xCD]), (*b"dupe", [0x00, 0x00])]);
        let sbix = SbixTable::parse(&bytes, 2).expect("parse");
        // Raw access returns the dupe sentinel.
        let raw = sbix.lookup_best_fit(1, 32).expect("entry");
        assert!(raw.is_dupe());
        assert_eq!(raw.dupe_target(), Some(0));
        // Resolved access returns the underlying png entry.
        let res = sbix.lookup_best_fit_resolved(1, 32).expect("entry");
        assert!(!res.is_dupe());
        assert_eq!(&res.graphic_type, b"png ");
        assert_eq!(res.bytes[0..2], [0xAB, 0xCD]);
    }

    #[test]
    fn dupe_predicates_return_none_on_short_payload() {
        // Manually build a sbix where glyph 0 has graphic_type 'dupe'
        // but a 0-byte payload (blob length = 8). dupe_target must
        // surface None rather than reading past the slice.
        let num_glyphs: u16 = 1;
        let strike_header_len = 4 + (num_glyphs as usize + 1) * 4;
        let blob_len = 8; // 8 header + 0 payload
        let strike_data = num_glyphs as usize * blob_len;
        let strike_total = strike_header_len + strike_data;
        let header_len = 8 + 4;
        let strike0 = header_len;
        let total = strike0 + strike_total;

        let mut bytes = vec![0u8; total];
        bytes[0..2].copy_from_slice(&1u16.to_be_bytes());
        bytes[2..4].copy_from_slice(&1u16.to_be_bytes());
        bytes[4..8].copy_from_slice(&1u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&(strike0 as u32).to_be_bytes());
        bytes[strike0..strike0 + 2].copy_from_slice(&32u16.to_be_bytes());
        bytes[strike0 + 2..strike0 + 4].copy_from_slice(&96u16.to_be_bytes());
        for i in 0..=num_glyphs as usize {
            let off = strike_header_len + i * blob_len;
            let dst = strike0 + 4 + i * 4;
            bytes[dst..dst + 4].copy_from_slice(&(off as u32).to_be_bytes());
        }
        let blob_off = strike0 + strike_header_len;
        bytes[blob_off + 4..blob_off + 8].copy_from_slice(b"dupe");

        let sbix = SbixTable::parse(&bytes, 1).expect("parse");
        let g = sbix.glyph(0, 0).expect("entry");
        assert!(g.is_dupe());
        assert_eq!(g.dupe_target(), None);
        assert!(sbix.resolve_dupe_chain(0, 0).is_none());
    }
}
