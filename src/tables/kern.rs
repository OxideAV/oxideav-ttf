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
//! rejected outright.
//!
//! For the Microsoft variant this crate decodes both subtable formats the
//! OFF spec defines (§5.7.3): **Format 0** (a sorted list of explicit
//! `(left, right) → value` kerning pairs) and **Format 2** (a class-based
//! two-dimensional array, where left and right glyphs map to classes and
//! the value is the array cell at `(leftClass, rightClass)`). Formats 1
//! and 3..255 are reserved by the spec and skipped. Horizontal kerning
//! subtables are honoured; "minimum" subtables (a floor rather than a
//! delta) and non-horizontal / cross-stream subtables are skipped.
//! Kerning subtables are additive, so [`KernTable::lookup`] sums every
//! matching subtable's contribution.

use crate::parser::{read_i16, read_u16, read_u32};
use crate::Error;

#[derive(Debug, Clone)]
pub struct KernTable<'a> {
    /// All format-0 pair lists collected at parse time, sorted by
    /// `(left << 16 | right)` for binary search.
    pairs: Vec<KernPair>,
    /// All format-2 (class-based two-dimensional array) horizontal
    /// kerning subtables collected at parse time. The spec (§5.7.3) makes
    /// kerning subtables additive, so a lookup sums every matching
    /// subtable; in practice a font ships either format 0 or format 2.
    format2: Vec<Format2Subtable>,
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

/// One decoded format-2 (class-based two-dimensional array) horizontal
/// kerning subtable (ISO/IEC 14496-22:2019 §5.7.3 "Format 2").
///
/// Glyphs are mapped to left- and right-hand classes; the kerning value
/// for a pair is the array cell at `(leftClass, rightClass)`. The spec
/// pre-multiplies the stored class values — left-class values by
/// `rowWidth` (bytes per row) and right-class values by the kerning-value
/// size (2) — so a cell address is `array + leftClassValue +
/// rightClassValue`. We store the pre-multiplied class values verbatim and
/// reproduce that addressing, validating every resulting cell offset lands
/// inside the subtable.
#[derive(Debug, Clone)]
struct Format2Subtable {
    /// `firstGlyph` / pre-multiplied class values for the left-hand class
    /// table. A glyph outside `[first, first + values.len())` uses class 0
    /// (the "does not kern" row, all zeros per spec).
    left: ClassTable,
    /// Same for the right-hand class table (column index).
    right: ClassTable,
    /// The flattened kerning array: `array.len()` FWord cells, row-major.
    array: Vec<i16>,
    /// Width of one row in bytes (the `rowWidth` header field); used to
    /// validate the pre-multiplied left-class addressing.
    row_width: usize,
}

/// A kern format-2 class table: a glyph-id range mapped to pre-multiplied
/// class values.
#[derive(Debug, Clone)]
struct ClassTable {
    first_glyph: u16,
    /// Pre-multiplied class value per glyph in the range. `values[g -
    /// first_glyph]` is the byte offset contribution for glyph `g`.
    values: Vec<u16>,
}

impl ClassTable {
    /// The pre-multiplied class value for `glyph`, or `0` (class 0, "does
    /// not kern") when the glyph is outside the table's range.
    fn value_for(&self, glyph: u16) -> u16 {
        if glyph < self.first_glyph {
            return 0;
        }
        let idx = (glyph - self.first_glyph) as usize;
        self.values.get(idx).copied().unwrap_or(0)
    }
}

impl Format2Subtable {
    /// Look up the additive kerning contribution for an ordered glyph pair.
    /// Returns 0 when either glyph is unmapped (class 0) or the addressed
    /// cell does not fall on a valid array index.
    fn lookup(&self, left: u16, right: u16) -> i16 {
        // Stored class values are pre-multiplied: left by rowWidth (bytes),
        // right by the 2-byte kerning-value size. The cell byte offset from
        // the array start is therefore left_value + right_value; dividing
        // by 2 yields the FWord index.
        let lo = self.left.value_for(left) as usize;
        let ro = self.right.value_for(right) as usize;
        // Class 0 on either axis means "does not kern".
        if lo == 0 || ro == 0 {
            return 0;
        }
        let byte_off = lo + ro;
        // The left value is a multiple of rowWidth and the right value a
        // multiple of 2, so the sum is even; guard anyway.
        if self.row_width == 0 || byte_off % 2 != 0 {
            return 0;
        }
        let idx = byte_off / 2;
        self.array.get(idx).copied().unwrap_or(0)
    }
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
                format2: Vec::new(),
                variant,
                _phantom: core::marker::PhantomData,
            });
        }
        let mut format2 = Vec::new();
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
            if horizontal && is_kerning {
                match format {
                    0 => parse_format0(bytes, off + 6, &mut pairs)?,
                    2 => {
                        // The format-2 body begins right after the 6-byte
                        // subtable header; its internal offsets are measured
                        // from the *subtable* start (`off`), per §5.7.3.
                        if let Some(sub) = parse_format2(bytes, off, length)? {
                            format2.push(sub);
                        }
                    }
                    // Formats 1 and 3..255 are reserved per §5.7.3; skip.
                    _ => {}
                }
            }
            off = next_off;
        }
        pairs.sort_by_key(|p| p.key);
        Ok(Self {
            pairs,
            format2,
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

    /// Number of decoded format-2 (class-based two-dimensional array)
    /// horizontal kerning subtables. A font usually ships either format 0
    /// or format 2, not both; this is `0` for the common format-0 case.
    pub fn format2_subtable_count(&self) -> usize {
        self.format2.len()
    }

    /// Look up the kerning between an ordered glyph pair, in font units.
    /// Returns 0 when no rule matches.
    ///
    /// Per §5.7.3 kerning subtables are *additive*, so the result is the
    /// sum of the matching format-0 pair (if any) and every format-2
    /// class-array cell the pair addresses. In practice a font ships one
    /// form, so the sum reduces to a single contribution.
    pub fn lookup(&self, left: u16, right: u16) -> i16 {
        let key = ((left as u32) << 16) | right as u32;
        let mut value: i32 = match self.pairs.binary_search_by_key(&key, |p| p.key) {
            Ok(i) => self.pairs[i].value as i32,
            Err(_) => 0,
        };
        for sub in &self.format2 {
            value += sub.lookup(left, right) as i32;
        }
        value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
    }
}

/// Parse a format-2 (class-based two-dimensional array) kerning subtable.
/// `sub_off` is the byte offset of the *subtable* (its 6-byte header), and
/// `length` is the subtable length from that header; the format-2 internal
/// offsets are measured from `sub_off`. Returns `Ok(None)` for a subtable
/// whose offsets or array do not fit inside the declared length (a
/// malformed subtable is skipped, not fatal).
fn parse_format2(
    bytes: &[u8],
    sub_off: usize,
    length: usize,
) -> Result<Option<Format2Subtable>, Error> {
    // Body header (after the 6-byte shared subtable header):
    //   u16 rowWidth, Offset16 leftClassTable, Offset16 rightClassTable,
    //   Offset16 array. All offsets are from the subtable start.
    let body = sub_off + 6;
    if body + 8 > bytes.len() || sub_off + length > bytes.len() {
        return Ok(None);
    }
    let row_width = read_u16(bytes, body)? as usize;
    let left_off = read_u16(bytes, body + 2)? as usize;
    let right_off = read_u16(bytes, body + 4)? as usize;
    let array_off = read_u16(bytes, body + 6)? as usize;
    // Bound every offset to within the subtable.
    let sub_end = sub_off + length;
    let left = match parse_class_table(bytes, sub_off, left_off, sub_end)? {
        Some(t) => t,
        None => return Ok(None),
    };
    let right = match parse_class_table(bytes, sub_off, right_off, sub_end)? {
        Some(t) => t,
        None => return Ok(None),
    };
    let array_start = sub_off + array_off;
    if array_off == 0 || array_start > sub_end {
        return Ok(None);
    }
    // The array runs from array_start to the subtable end; decode all whole
    // FWord cells that fit. Pre-multiplied class addressing indexes into
    // this flat array, so we keep every cell the subtable carries.
    let array_bytes = sub_end - array_start;
    let cell_count = array_bytes / 2;
    let mut array = Vec::with_capacity(cell_count);
    for i in 0..cell_count {
        array.push(read_i16(bytes, array_start + i * 2)?);
    }
    Ok(Some(Format2Subtable {
        left,
        right,
        array,
        row_width,
    }))
}

/// Parse one kern format-2 class table at `sub_off + rel_off`:
///   u16 firstGlyph, u16 nGlyphs, u16 classValues[nGlyphs].
/// Returns `Ok(None)` when the table runs past `sub_end`.
fn parse_class_table(
    bytes: &[u8],
    sub_off: usize,
    rel_off: usize,
    sub_end: usize,
) -> Result<Option<ClassTable>, Error> {
    if rel_off == 0 {
        return Ok(None);
    }
    let start = sub_off + rel_off;
    if start + 4 > sub_end {
        return Ok(None);
    }
    let first_glyph = read_u16(bytes, start)?;
    let n_glyphs = read_u16(bytes, start + 2)? as usize;
    let arr = start + 4;
    if arr + n_glyphs * 2 > sub_end {
        return Ok(None);
    }
    let mut values = Vec::with_capacity(n_glyphs);
    for i in 0..n_glyphs {
        values.push(read_u16(bytes, arr + i * 2)?);
    }
    Ok(Some(ClassTable {
        first_glyph,
        values,
    }))
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

    /// Build a Microsoft-headered `kern` carrying one format-2 (class-based
    /// 2D array) horizontal subtable with 2 left classes and 2 right
    /// classes. Glyph 10 → left class 1, glyph 20 → right class 1; the
    /// `(class1, class1)` cell holds `cell_value`. Everything else maps to
    /// class 0 ("does not kern").
    ///
    /// Layout (subtable starts at file offset 4):
    /// ```text
    ///  [4..10)   subtable header: version(2) length(2) coverage(2)=0x0201
    ///  [10..18)  body header: rowWidth(2) leftOff(2) rightOff(2) arrayOff(2)
    ///  [18..26)  left class table:  firstGlyph=10 nGlyphs=2 values=[4,0]
    ///  [26..34)  right class table: firstGlyph=20 nGlyphs=2 values=[2,0]
    ///  [34..42)  array: 4 FWord cells [r0c0,r0c1,r1c0,r1c1]
    /// ```
    /// rowWidth = 2 right-classes × 2-byte value = 4. Left class 1 is
    /// pre-multiplied by rowWidth → 4; right class 1 by value size 2 → 2.
    /// Cell byte offset = 4 + 2 = 6 → FWord index 3 (= r1c1).
    fn build_kern_format2(cell_value: i16) -> Vec<u8> {
        let mut t = vec![0u8; 4];
        t[0..2].copy_from_slice(&0u16.to_be_bytes()); // version
        t[2..4].copy_from_slice(&1u16.to_be_bytes()); // nTables
        let mut sub = vec![0u8; 38];
        sub[0..2].copy_from_slice(&0u16.to_be_bytes()); // sub version
        sub[2..4].copy_from_slice(&38u16.to_be_bytes()); // length
        sub[4..6].copy_from_slice(&0x0201u16.to_be_bytes()); // format 2, horizontal
                                                             // body header (offsets are from the subtable start)
        sub[6..8].copy_from_slice(&4u16.to_be_bytes()); // rowWidth
        sub[8..10].copy_from_slice(&14u16.to_be_bytes()); // leftClassTable offset
        sub[10..12].copy_from_slice(&22u16.to_be_bytes()); // rightClassTable offset
        sub[12..14].copy_from_slice(&30u16.to_be_bytes()); // array offset
                                                           // left class table at sub+14
        sub[14..16].copy_from_slice(&10u16.to_be_bytes()); // firstGlyph
        sub[16..18].copy_from_slice(&2u16.to_be_bytes()); // nGlyphs
        sub[18..20].copy_from_slice(&4u16.to_be_bytes()); // glyph10 -> class1*rowWidth
        sub[20..22].copy_from_slice(&0u16.to_be_bytes()); // glyph11 -> class0
                                                          // right class table at sub+22
        sub[22..24].copy_from_slice(&20u16.to_be_bytes()); // firstGlyph
        sub[24..26].copy_from_slice(&2u16.to_be_bytes()); // nGlyphs
        sub[26..28].copy_from_slice(&2u16.to_be_bytes()); // glyph20 -> class1*2
        sub[28..30].copy_from_slice(&0u16.to_be_bytes()); // glyph21 -> class0
                                                          // array at sub+30: 4 cells, r1c1 = index 3
        sub[36..38].copy_from_slice(&cell_value.to_be_bytes());
        t.extend_from_slice(&sub);
        t
    }

    #[test]
    fn format2_class_array_lookup() {
        let bytes = build_kern_format2(-50);
        let k = KernTable::parse(&bytes).unwrap();
        assert_eq!(k.header_variant(), HeaderVariant::Microsoft);
        assert_eq!(k.pair_count(), 0);
        assert_eq!(k.format2_subtable_count(), 1);
        // glyph 10 (left class 1) before glyph 20 (right class 1) -> r1c1.
        assert_eq!(k.lookup(10, 20), -50);
        // glyph 11 maps to left class 0 ("does not kern") -> 0.
        assert_eq!(k.lookup(11, 20), 0);
        // glyph 21 maps to right class 0 -> 0.
        assert_eq!(k.lookup(10, 21), 0);
        // glyphs outside either class table -> class 0 -> 0.
        assert_eq!(k.lookup(99, 99), 0);
    }

    #[test]
    fn format2_minimum_subtable_skipped() {
        // A format-2 subtable with the "minimum" coverage bit set provides
        // a floor, not a kerning delta; it must be skipped like format 0's
        // minimum tables.
        let mut bytes = build_kern_format2(-50);
        // coverage byte is at file offset 8..10 (subtable starts at 4,
        // coverage at +4). Set bit 1 (minimum) -> 0x0203.
        bytes[8..10].copy_from_slice(&0x0203u16.to_be_bytes());
        let k = KernTable::parse(&bytes).unwrap();
        assert_eq!(k.format2_subtable_count(), 0);
        assert_eq!(k.lookup(10, 20), 0);
    }

    #[test]
    fn format2_and_format0_are_additive() {
        // Two subtables: a format-0 pair (10,20)=-30 and the format-2 table
        // with (10,20)=-50. §5.7.3 makes them additive -> -80.
        let mut t = vec![0u8; 4];
        t[0..2].copy_from_slice(&0u16.to_be_bytes());
        t[2..4].copy_from_slice(&2u16.to_be_bytes()); // nTables = 2
                                                      // format-0 subtable (length 20): pair (10,20) = -30.
        let mut f0 = vec![0u8; 20];
        f0[2..4].copy_from_slice(&20u16.to_be_bytes());
        f0[4..6].copy_from_slice(&1u16.to_be_bytes()); // coverage: format 0, horizontal
        f0[6..8].copy_from_slice(&1u16.to_be_bytes()); // nPairs
        f0[14..16].copy_from_slice(&10u16.to_be_bytes());
        f0[16..18].copy_from_slice(&20u16.to_be_bytes());
        f0[18..20].copy_from_slice(&(-30i16).to_be_bytes());
        t.extend_from_slice(&f0);
        // append the format-2 subtable (skip its own 4-byte kern header).
        let f2 = build_kern_format2(-50);
        t.extend_from_slice(&f2[4..]);
        let k = KernTable::parse(&t).unwrap();
        assert_eq!(k.pair_count(), 1);
        assert_eq!(k.format2_subtable_count(), 1);
        assert_eq!(k.lookup(10, 20), -80);
    }

    #[test]
    fn format2_malformed_offsets_skipped() {
        // An array offset past the subtable end is skipped, not fatal.
        let mut bytes = build_kern_format2(-50);
        // array offset field at file offset 4+12=16.
        bytes[16..18].copy_from_slice(&9999u16.to_be_bytes());
        let k = KernTable::parse(&bytes).unwrap();
        assert_eq!(k.format2_subtable_count(), 0);
    }
}
