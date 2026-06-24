//! `CFF ` — Compact Font Format (PostScript outlines).
//!
//! A CFF table carries one font's worth of PostScript glyph outlines in
//! the compact binary container described by Adobe Technical Note #5176
//! ("The Compact Font Format Specification"). The per-glyph charstrings
//! are Type 2 charstrings (Adobe Technical Note #5177); they are decoded
//! into cubic-Bezier outlines by [`charstring`].
//!
//! ## Container layout (TN #5176 §2, Table 1)
//!
//! ```text
//!   Header
//!   Name INDEX
//!   Top DICT INDEX
//!   String INDEX
//!   Global Subr INDEX
//!   Encodings              (offset from Top DICT)
//!   Charsets               (offset from Top DICT)
//!   FDSelect               (CIDFonts only, offset from Top DICT)
//!   CharStrings INDEX      (offset from Top DICT)
//!   Font DICT INDEX        (CIDFonts only, offset from Top DICT)
//!   Private DICT           (size + offset from Top DICT)
//!   Local Subr INDEX       (offset from the Private DICT)
//! ```
//!
//! Everything after the first five fixed structures is reached through
//! byte offsets stored as DICT operands, so the ordering of the later
//! structures is not fixed. All multi-byte integers are big-endian and
//! unaligned (TN #5176 §3).
//!
//! ## What this module decodes
//!
//! - The fixed header + Name / Top DICT / String / Global Subr INDEXes.
//! - The Top DICT operands needed to render outlines: `CharStrings`,
//!   `Private`, `charset`, `FDArray`, `FDSelect`, `ROS`, `FontMatrix`.
//! - The CharStrings INDEX (one Type 2 charstring per glyph).
//! - Local subrs (per-font, or per-Font-DICT for CIDFonts).
//! - The charset (SID/CID per glyph) in formats 0 / 1 / 2.
//! - FDSelect (formats 0 / 3) for CIDFonts.
//!
//! Outline reconstruction lives in [`charstring`]; this module owns the
//! container walk and exposes [`CffTable::glyph_outline`].

use crate::outline::TtOutline;
use crate::parser::{read_u16, read_u8};
use crate::Error;

pub mod charstring;
pub mod strings;

/// Parsed `CFF ` table: the container walk plus everything needed to
/// reconstruct a glyph outline by GID.
#[derive(Debug, Clone)]
pub struct CffTable<'a> {
    /// The whole `CFF ` table slice (all internal offsets are relative
    /// to the start of this slice per TN #5176 §3 reference point `(0)`).
    data: &'a [u8],
    /// CharStrings INDEX — one Type 2 charstring per glyph, indexed by
    /// GID. Its `count` is the glyph count.
    char_strings: Index<'a>,
    /// Global Subr INDEX (shared across the FontSet).
    global_subrs: Index<'a>,
    /// Per-font local subrs, when the font is not CID-keyed. CIDFonts
    /// carry local subrs per Font DICT instead (see `fd_select` /
    /// `fd_local_subrs`).
    local_subrs: Index<'a>,
    /// Per-font default/nominal glyph width from the Private DICT
    /// (non-CID fonts). `(defaultWidthX, nominalWidthX)`.
    width: (f32, f32),
    /// The charset: per-GID SID (non-CID) or CID (CIDFont). `charset[0]`
    /// is always GID 0 (`.notdef`, SID/CID 0). Empty when the predefined
    /// ISOAdobe charset (id 0) is in force.
    charset: Vec<u16>,
    /// CIDFont per-Font-DICT data, present iff the Top DICT had a `ROS`
    /// operator. `(fd_select, fd_locals, fd_widths)`.
    cid: Option<CidData<'a>>,
    /// Whether the charstrings are Type 2 (`CharstringType` default 2).
    /// Type 1 charstrings are out of scope; we still parse the container.
    charstring_type: i32,
    /// String INDEX — custom strings beyond the 391 standard strings.
    /// SID `s >= N_STD_STRINGS` resolves to `strings.get(s - N_STD_STRINGS)`.
    strings: Index<'a>,
}

/// CIDFont-specific decode state.
#[derive(Debug, Clone)]
struct CidData<'a> {
    /// Maps GID -> Font DICT index (FDSelect, TN #5176 §19).
    fd_select: FdSelect,
    /// Per-Font-DICT local subr INDEX.
    fd_locals: Vec<Index<'a>>,
    /// Per-Font-DICT `(defaultWidthX, nominalWidthX)`.
    fd_widths: Vec<(f32, f32)>,
}

impl<'a> CffTable<'a> {
    /// Parse the `CFF ` table from `data` (the raw table slice).
    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
        // --- Header (TN #5176 §6, Table 8) ---------------------------
        // major, minor, hdrSize, offSize — we only need hdrSize to skip
        // to the Name INDEX (the offSize here applies to absolute (0)
        // offsets, not used by the layout walk).
        let hdr_size = read_u8(data, 2)? as usize;
        if hdr_size < 4 {
            return Err(Error::BadStructure("CFF header size too small"));
        }

        // --- Name INDEX (skipped; we only render one font) -----------
        let mut pos = hdr_size;
        let _name = Index::parse(data, &mut pos)?;
        // --- Top DICT INDEX ------------------------------------------
        let top_dicts = Index::parse(data, &mut pos)?;
        // --- String INDEX (custom strings beyond the standard set) ---
        let strings = Index::parse(data, &mut pos)?;
        // --- Global Subr INDEX ---------------------------------------
        let global_subrs = Index::parse(data, &mut pos)?;

        // The FontSet's first font is the one we render.
        let top_dict_bytes = top_dicts
            .get(0)
            .ok_or(Error::BadStructure("CFF empty Top DICT INDEX"))?;
        let top = Dict::parse(top_dict_bytes)?;

        // CharStrings INDEX offset is mandatory.
        let cs_off = top
            .first_int(op::CHAR_STRINGS)
            .ok_or(Error::BadStructure("CFF Top DICT missing CharStrings"))?
            as usize;
        let mut cs_pos = cs_off;
        let char_strings = Index::parse(data, &mut cs_pos)?;

        let charstring_type = top.first_int(op::CHARSTRING_TYPE).unwrap_or(2);

        let n_glyphs = char_strings.count();

        // --- charset (per-GID SID/CID) -------------------------------
        let charset_off = top.first_int(op::CHARSET).unwrap_or(0);
        let charset = parse_charset(data, charset_off, n_glyphs)?;

        // --- CID vs. non-CID dispatch --------------------------------
        let (local_subrs, width, cid) = if top.contains(op::ROS) {
            // CIDFont: FDArray + FDSelect drive per-glyph Private DICTs.
            let cid = parse_cid(data, &top, n_glyphs)?;
            (Index::empty(), (0.0, 0.0), Some(cid))
        } else {
            // Plain font: one Private DICT for the whole font.
            let (local_subrs, width) = parse_private(data, &top)?;
            (local_subrs, width, None)
        };

        Ok(Self {
            data,
            char_strings,
            global_subrs,
            local_subrs,
            width,
            charset,
            cid,
            charstring_type,
            strings,
        })
    }

    /// Resolve a CFF string identifier (SID) to its string. SIDs below
    /// `strings::N_STD_STRINGS` index the predefined standard-strings
    /// table; higher SIDs index the font's String INDEX. `None` when the
    /// SID is out of range or the String-INDEX entry is not valid UTF-8.
    pub fn string_for_sid(&self, sid: u16) -> Option<&str> {
        if sid < strings::N_STD_STRINGS {
            strings::STANDARD_STRINGS.get(sid as usize).copied()
        } else {
            let i = (sid - strings::N_STD_STRINGS) as usize;
            std::str::from_utf8(self.strings.get(i)?).ok()
        }
    }

    /// The PostScript glyph name for `gid`, resolved through the charset
    /// (GID → SID) and [`Self::string_for_sid`]. `None` for CID-keyed
    /// fonts (whose charset names glyphs by CID, not by string), for the
    /// predefined-charset case, or when the SID is unresolvable.
    pub fn glyph_name(&self, gid: u16) -> Option<&str> {
        if self.cid.is_some() {
            return None;
        }
        // The charset is empty for a predefined charset; we can only
        // resolve names when the font ships an explicit charset.
        if self.charset.is_empty() {
            // GID 0 is always `.notdef`.
            return if gid == 0 {
                strings::STANDARD_STRINGS.first().copied()
            } else {
                None
            };
        }
        let sid = *self.charset.get(gid as usize)?;
        self.string_for_sid(sid)
    }

    /// Number of glyphs (the CharStrings INDEX count).
    pub fn glyph_count(&self) -> u16 {
        self.char_strings.count().min(u16::MAX as usize) as u16
    }

    /// Whether the font is CID-keyed (Top DICT had a `ROS` operator).
    pub fn is_cid(&self) -> bool {
        self.cid.is_some()
    }

    /// The SID (non-CID) or CID (CIDFont) assigned to `gid` by the
    /// charset, or `None` when `gid` is out of range. For the predefined
    /// ISOAdobe charset the identity-ish mapping `gid as SID` is returned.
    pub fn sid_for_gid(&self, gid: u16) -> Option<u16> {
        if self.charset.is_empty() {
            // Predefined charset id 0 (ISOAdobe): charset[gid] == gid for
            // the leading glyphs. We expose the GID itself as a best
            // effort; callers wanting exact ISOAdobe SIDs need the
            // predefined table (out of scope here).
            (gid as usize).lt(&self.char_strings.count()).then_some(gid)
        } else {
            self.charset.get(gid as usize).copied()
        }
    }

    /// Reconstruct the outline of glyph `gid` by interpreting its Type 2
    /// charstring. Returns an empty outline for a charstring with no path
    /// (e.g. a space glyph). `None` when `gid` is out of range or the
    /// font uses non-Type-2 charstrings.
    pub fn glyph_outline(&self, gid: u16) -> Option<TtOutline> {
        if self.charstring_type != 2 {
            return None;
        }
        let cs = self.char_strings.get(gid as usize)?;
        let (locals, nominal) = self.subrs_for_gid(gid);
        let mut interp = charstring::Interp::new(self.global_subrs, locals, nominal);
        interp.run(cs).ok()?;
        Some(interp.into_outline())
    }

    /// Advance width of `gid` in font units, derived from its charstring
    /// (Type 2 charstrings encode the optional width as the first stack
    /// entry, offset from `nominalWidthX`; absent means `defaultWidthX`).
    pub fn glyph_width(&self, gid: u16) -> Option<f32> {
        if self.charstring_type != 2 {
            return None;
        }
        let cs = self.char_strings.get(gid as usize)?;
        let (locals, nominal) = self.subrs_for_gid(gid);
        let default = if let Some(cid) = &self.cid {
            let fd = cid.fd_select.fd_for_gid(gid) as usize;
            cid.fd_widths.get(fd).map(|w| w.0).unwrap_or(0.0)
        } else {
            self.width.0
        };
        let mut interp = charstring::Interp::new(self.global_subrs, locals, nominal);
        interp.run(cs).ok()?;
        Some(interp.width().unwrap_or(default))
    }

    /// Pick the local subr INDEX and `nominalWidthX` that apply to `gid`,
    /// honouring FDSelect for CIDFonts.
    fn subrs_for_gid(&self, gid: u16) -> (Index<'a>, f32) {
        if let Some(cid) = &self.cid {
            let fd = cid.fd_select.fd_for_gid(gid) as usize;
            let locals = cid.fd_locals.get(fd).copied().unwrap_or_else(Index::empty);
            let nominal = cid.fd_widths.get(fd).map(|w| w.1).unwrap_or(0.0);
            (locals, nominal)
        } else {
            (self.local_subrs, self.width.1)
        }
    }

    /// The raw table slice (mostly for tests / diagnostics).
    pub fn data(&self) -> &'a [u8] {
        self.data
    }
}

// --- INDEX (TN #5176 §5, Table 7) ------------------------------------

/// A CFF INDEX: a count-prefixed array of variable-length objects, with an
/// offset array of `count + 1` entries (each `off_size` bytes) pointing
/// into the object data. Offsets are 1-based relative to the byte before
/// the object data.
///
/// We keep a handle to the full table slice `data` plus byte positions
/// within it, so `get` can return sub-slices that borrow the original
/// table for `'a`.
#[derive(Debug, Clone, Copy)]
pub struct Index<'a> {
    /// The full table slice (positions below index into this).
    data: &'a [u8],
    /// Number of objects.
    count: usize,
    /// Offset-array element width in bytes (1..=4).
    off_size: usize,
    /// Byte position (within `data`) where the offset array starts.
    offsets_at: usize,
    /// Byte position (within `data`) of the byte preceding object data,
    /// i.e. the reference point for the 1-based offsets.
    data_base: usize,
}

impl<'a> Index<'a> {
    /// An empty INDEX (count 0).
    fn empty() -> Self {
        Self {
            data: &[],
            count: 0,
            off_size: 1,
            offsets_at: 0,
            data_base: 0,
        }
    }

    /// `pub(crate)` alias of [`Self::empty`] so the CFF2 walker (a sibling
    /// module) can build empty subr INDEXes.
    pub(crate) fn empty_pub() -> Self {
        Self::empty()
    }

    /// Parse a CFF (Card16-count) INDEX at `*pos` within `data`,
    /// advancing `*pos` to the first byte past the INDEX (TN #5176 §5
    /// Note 2: the end is the offset given by the last offset-array
    /// element). `pub(crate)` so the CFF2 container walker can reuse it.
    pub(crate) fn parse(data: &'a [u8], pos: &mut usize) -> Result<Self, Error> {
        Self::parse_impl(data, pos, false)
    }

    /// Parse a CFF2 (Card32-count) INDEX. CFF2 INDEXes are identical to
    /// CFF INDEXes except the leading `count` is a 32-bit value.
    pub(crate) fn parse_wide(data: &'a [u8], pos: &mut usize) -> Result<Self, Error> {
        Self::parse_impl(data, pos, true)
    }

    fn parse_impl(data: &'a [u8], pos: &mut usize, wide_count: bool) -> Result<Self, Error> {
        let start = *pos;
        // CFF: Card16 count; CFF2: Card32 count.
        let (count, after_count) = if wide_count {
            (crate::parser::read_u32(data, start)? as usize, start + 4)
        } else {
            (read_u16(data, start)? as usize, start + 2)
        };
        if count == 0 {
            // Empty INDEX is just the count field.
            *pos = after_count;
            return Ok(Self {
                data,
                count: 0,
                off_size: 1,
                offsets_at: after_count,
                data_base: after_count,
            });
        }
        let off_size = read_u8(data, after_count)? as usize;
        if !(1..=4).contains(&off_size) {
            return Err(Error::BadStructure("CFF INDEX offSize out of range"));
        }
        let offsets_at = after_count + 1;
        // count + 1 offsets, each off_size bytes.
        let off_array_len = (count + 1) * off_size;
        let data_base = offsets_at + off_array_len - 1;
        // The last offset gives the length of the object data + 1.
        let last_off = read_offset(data, offsets_at + count * off_size, off_size)?;
        let end = data_base + last_off;
        if end > data.len() {
            return Err(Error::UnexpectedEof);
        }
        let me = Self {
            data,
            count,
            off_size,
            offsets_at,
            data_base,
        };
        *pos = end;
        Ok(me)
    }

    /// Number of objects in the INDEX.
    pub fn count(&self) -> usize {
        self.count
    }

    /// The `i`-th object's bytes, or `None` when `i` is out of range or
    /// the offset pair is malformed.
    pub fn get(&self, i: usize) -> Option<&'a [u8]> {
        if i >= self.count {
            return None;
        }
        let off_lo = read_offset_slice(self.data, self.offsets_at, self.off_size, i)?;
        let off_hi = read_offset_slice(self.data, self.offsets_at, self.off_size, i + 1)?;
        if off_hi < off_lo {
            return None;
        }
        let lo = self.data_base + off_lo;
        let hi = self.data_base + off_hi;
        self.data.get(lo..hi)
    }
}

/// Read one `off_size`-byte big-endian offset at `at` from `data`.
fn read_offset(data: &[u8], at: usize, off_size: usize) -> Result<usize, Error> {
    let s = data.get(at..at + off_size).ok_or(Error::UnexpectedEof)?;
    let mut v = 0usize;
    for &b in s {
        v = (v << 8) | b as usize;
    }
    Ok(v)
}

/// Read the `idx`-th offset from an offset array that begins at
/// `offsets_at` within `data`, each element `off_size` bytes.
fn read_offset_slice(data: &[u8], offsets_at: usize, off_size: usize, idx: usize) -> Option<usize> {
    read_offset(data, offsets_at + idx * off_size, off_size).ok()
}

// --- DICT (TN #5176 §4) ----------------------------------------------

/// A parsed CFF DICT: a map from operator key to its operand list. Keys
/// are encoded as the operator byte, or `1200 + b` for two-byte (escape
/// 12) operators.
#[derive(Debug, Clone, Default)]
pub struct Dict {
    entries: Vec<(u16, Vec<f64>)>,
}

impl Dict {
    /// Parse a DICT from its byte slice (TN #5176 §4): a sequence of
    /// operands followed by an operator.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let mut entries = Vec::new();
        let mut operands: Vec<f64> = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            let b0 = bytes[i];
            match b0 {
                // Operators: 0..=21 in CFF (12 is a two-byte escape);
                // CFF2 extends the DICT operator space with `blend` (23)
                // and `vstore` (24), so accept 0..=24. Bytes 25..=27, 31,
                // 255 remain reserved operand bytes.
                0..=24 => {
                    let key = if b0 == 12 {
                        let b1 = *bytes.get(i + 1).ok_or(Error::UnexpectedEof)?;
                        i += 2;
                        1200 + b1 as u16
                    } else {
                        i += 1;
                        b0 as u16
                    };
                    entries.push((key, std::mem::take(&mut operands)));
                }
                // 28: 3-byte short int.
                28 => {
                    let hi = *bytes.get(i + 1).ok_or(Error::UnexpectedEof)?;
                    let lo = *bytes.get(i + 2).ok_or(Error::UnexpectedEof)?;
                    operands.push((i16::from_be_bytes([hi, lo])) as f64);
                    i += 3;
                }
                // 29: 5-byte int.
                29 => {
                    let s = bytes.get(i + 1..i + 5).ok_or(Error::UnexpectedEof)?;
                    let v = i32::from_be_bytes([s[0], s[1], s[2], s[3]]);
                    operands.push(v as f64);
                    i += 5;
                }
                // 30: real number (BCD nibbles).
                30 => {
                    let (val, consumed) = parse_real(&bytes[i + 1..])?;
                    operands.push(val);
                    i += 1 + consumed;
                }
                // 32..=246: 1-byte int.
                32..=246 => {
                    operands.push((b0 as i32 - 139) as f64);
                    i += 1;
                }
                // 247..=250: 2-byte positive int.
                247..=250 => {
                    let w = *bytes.get(i + 1).ok_or(Error::UnexpectedEof)?;
                    operands.push(((b0 as i32 - 247) * 256 + w as i32 + 108) as f64);
                    i += 2;
                }
                // 251..=254: 2-byte negative int.
                251..=254 => {
                    let w = *bytes.get(i + 1).ok_or(Error::UnexpectedEof)?;
                    operands.push((-(b0 as i32 - 251) * 256 - w as i32 - 108) as f64);
                    i += 2;
                }
                // 22..=27, 31, 255 are reserved as DICT operand bytes.
                _ => return Err(Error::BadStructure("CFF DICT reserved operand byte")),
            }
        }
        Ok(Self { entries })
    }

    /// All operands for `key`, or `None` when the key is absent.
    pub fn operands(&self, key: u16) -> Option<&[f64]> {
        self.entries
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_slice())
    }

    /// Whether `key` is present.
    pub fn contains(&self, key: u16) -> bool {
        self.entries.iter().any(|(k, _)| *k == key)
    }

    /// The first operand of `key` as an integer, or `None`.
    pub fn first_int(&self, key: u16) -> Option<i32> {
        self.operands(key)
            .and_then(|v| v.first())
            .map(|&f| f as i32)
    }

    /// The first operand of `key` as a float, or `None`.
    pub fn first_float(&self, key: u16) -> Option<f64> {
        self.operands(key).and_then(|v| v.first()).copied()
    }
}

/// Parse a real-number operand body (the bytes after the leading 30),
/// returning `(value, bytes_consumed)`. TN #5176 §4 Table 5: nibble pairs
/// per byte; `0xf` terminates.
fn parse_real(bytes: &[u8]) -> Result<(f64, usize), Error> {
    let mut s = String::new();
    let mut consumed = 0;
    'outer: for &b in bytes {
        consumed += 1;
        for nib in [b >> 4, b & 0x0f] {
            match nib {
                0..=9 => s.push((b'0' + nib) as char),
                0xa => s.push('.'),
                0xb => s.push('E'),
                0xc => s.push_str("E-"),
                0xd => return Err(Error::BadStructure("CFF real reserved nibble")),
                0xe => s.push('-'),
                0xf => break 'outer,
                _ => unreachable!(),
            }
        }
    }
    let val = s.parse::<f64>().unwrap_or(0.0);
    Ok((val, consumed))
}

// --- Top / Private DICT operator keys --------------------------------

/// DICT operator key constants (TN #5176 Tables 9, 10, 23).
pub mod op {
    // Top DICT.
    pub const CHARSET: u16 = 15;
    pub const CHAR_STRINGS: u16 = 17;
    pub const PRIVATE: u16 = 18;
    pub const CHARSTRING_TYPE: u16 = 1206;
    pub const ROS: u16 = 1230;
    pub const FD_ARRAY: u16 = 1236;
    pub const FD_SELECT: u16 = 1237;

    // Private DICT.
    pub const SUBRS: u16 = 19;
    pub const DEFAULT_WIDTH_X: u16 = 20;
    pub const NOMINAL_WIDTH_X: u16 = 21;
}

// --- Private DICT + local subrs --------------------------------------

/// Parse the (non-CID) Private DICT referenced by the Top DICT and the
/// local subr INDEX it points to. Returns `(local_subrs, (default,
/// nominal) widths)`.
fn parse_private<'a>(data: &'a [u8], top: &Dict) -> Result<(Index<'a>, (f32, f32)), Error> {
    let priv_ops = match top.operands(op::PRIVATE) {
        Some(v) if v.len() >= 2 => v,
        // No Private DICT: legal (size 0). No local subrs, default widths.
        _ => return Ok((Index::empty(), (0.0, 0.0))),
    };
    let size = priv_ops[0] as usize;
    let off = priv_ops[1] as usize;
    if size == 0 {
        return Ok((Index::empty(), (0.0, 0.0)));
    }
    let pd = data.get(off..off + size).ok_or(Error::UnexpectedEof)?;
    let priv_dict = Dict::parse(pd)?;
    let default_w = priv_dict.first_float(op::DEFAULT_WIDTH_X).unwrap_or(0.0) as f32;
    let nominal_w = priv_dict.first_float(op::NOMINAL_WIDTH_X).unwrap_or(0.0) as f32;

    // Subrs offset is relative to the start of the Private DICT data.
    let locals = match priv_dict.first_int(op::SUBRS) {
        Some(subr_off) => {
            let mut p = off + subr_off as usize;
            Index::parse(data, &mut p)?
        }
        None => Index::empty(),
    };
    Ok((locals, (default_w, nominal_w)))
}

// --- CIDFont: FDArray + FDSelect -------------------------------------

/// Parse the CIDFont per-Font-DICT structures (FDArray, FDSelect) plus
/// the per-FD local subrs and widths (TN #5176 §18, §19).
fn parse_cid<'a>(data: &'a [u8], top: &Dict, n_glyphs: usize) -> Result<CidData<'a>, Error> {
    let fd_array_off = top
        .first_int(op::FD_ARRAY)
        .ok_or(Error::BadStructure("CIDFont missing FDArray"))? as usize;
    let fd_select_off = top
        .first_int(op::FD_SELECT)
        .ok_or(Error::BadStructure("CIDFont missing FDSelect"))? as usize;

    let mut p = fd_array_off;
    let fd_array = Index::parse(data, &mut p)?;
    let mut fd_locals = Vec::with_capacity(fd_array.count());
    let mut fd_widths = Vec::with_capacity(fd_array.count());
    for i in 0..fd_array.count() {
        let fd_bytes = fd_array
            .get(i)
            .ok_or(Error::BadStructure("CIDFont FDArray entry"))?;
        let fd_dict = Dict::parse(fd_bytes)?;
        let (locals, w) = parse_private(data, &fd_dict)?;
        fd_locals.push(locals);
        fd_widths.push(w);
    }

    let fd_select = FdSelect::parse(data, fd_select_off, n_glyphs)?;
    Ok(CidData {
        fd_select,
        fd_locals,
        fd_widths,
    })
}

/// FDSelect: maps each GID to a Font DICT index (TN #5176 §19).
#[derive(Debug, Clone)]
enum FdSelect {
    /// Format 0: one FD byte per glyph.
    Format0(Vec<u8>),
    /// Format 3: sorted `(first_gid, fd)` ranges with a sentinel.
    Format3 {
        ranges: Vec<(u16, u8)>,
        sentinel: u16,
    },
}

impl FdSelect {
    fn parse(data: &[u8], off: usize, n_glyphs: usize) -> Result<Self, Error> {
        let format = read_u8(data, off)?;
        match format {
            0 => {
                let arr = data
                    .get(off + 1..off + 1 + n_glyphs)
                    .ok_or(Error::UnexpectedEof)?;
                Ok(FdSelect::Format0(arr.to_vec()))
            }
            3 => {
                let n_ranges = read_u16(data, off + 1)? as usize;
                let mut ranges = Vec::with_capacity(n_ranges);
                let mut p = off + 3;
                for _ in 0..n_ranges {
                    let first = read_u16(data, p)?;
                    let fd = read_u8(data, p + 2)?;
                    ranges.push((first, fd));
                    p += 3;
                }
                let sentinel = read_u16(data, p)?;
                Ok(FdSelect::Format3 { ranges, sentinel })
            }
            _ => Err(Error::BadStructure("CFF FDSelect format unsupported")),
        }
    }

    /// The Font DICT index for `gid` (0 when out of range).
    fn fd_for_gid(&self, gid: u16) -> u8 {
        match self {
            FdSelect::Format0(arr) => arr.get(gid as usize).copied().unwrap_or(0),
            FdSelect::Format3 { ranges, sentinel } => {
                if gid >= *sentinel {
                    return 0;
                }
                // Ranges are sorted by `first`; find the last range whose
                // first <= gid.
                let mut fd = 0;
                for &(first, this_fd) in ranges {
                    if first <= gid {
                        fd = this_fd;
                    } else {
                        break;
                    }
                }
                fd
            }
        }
    }
}

// --- Charset (TN #5176 §13) ------------------------------------------

/// Parse the charset (per-GID SID for non-CID, per-GID CID for CIDFonts).
/// `charset_off` of 0/1/2 selects a predefined charset (we return an
/// empty vec for id 0 ISOAdobe and treat 1/2 as also "predefined" — only
/// id 0 is common and the predefined SID tables are out of scope). GID 0
/// (`.notdef`) always maps to SID/CID 0 and is not stored in the font.
fn parse_charset(data: &[u8], charset_off: i32, n_glyphs: usize) -> Result<Vec<u16>, Error> {
    if n_glyphs == 0 {
        return Ok(Vec::new());
    }
    // Predefined charsets (ids 0/1/2): we don't expand them.
    if (0..=2).contains(&charset_off) {
        return Ok(Vec::new());
    }
    let off = charset_off as usize;
    let format = read_u8(data, off)?;
    let mut out = Vec::with_capacity(n_glyphs);
    out.push(0); // GID 0 -> SID/CID 0 (.notdef), implicit.
    match format {
        0 => {
            // Format 0: nGlyphs-1 SIDs.
            let mut p = off + 1;
            for _ in 1..n_glyphs {
                out.push(read_u16(data, p)?);
                p += 2;
            }
        }
        1 => {
            // Format 1: Range1 { first SID, nLeft Card8 }.
            let mut p = off + 1;
            while out.len() < n_glyphs {
                let first = read_u16(data, p)?;
                let n_left = read_u8(data, p + 2)? as usize;
                p += 3;
                for k in 0..=n_left {
                    if out.len() >= n_glyphs {
                        break;
                    }
                    out.push(first.wrapping_add(k as u16));
                }
            }
        }
        2 => {
            // Format 2: Range2 { first SID, nLeft Card16 }.
            let mut p = off + 1;
            while out.len() < n_glyphs {
                let first = read_u16(data, p)?;
                let n_left = read_u16(data, p + 2)? as usize;
                p += 4;
                for k in 0..=n_left {
                    if out.len() >= n_glyphs {
                        break;
                    }
                    out.push(first.wrapping_add(k as u16));
                }
            }
        }
        _ => return Err(Error::BadStructure("CFF charset format unsupported")),
    }
    Ok(out)
}

/// Compute the Type 2 subr-number bias from a subr INDEX count
/// (TN #5176 §16 / TN #5177 §4.7).
pub(crate) fn subr_bias(n_subrs: usize) -> i32 {
    if n_subrs < 1240 {
        107
    } else if n_subrs < 33900 {
        1131
    } else {
        32768
    }
}

/// Read the 4-byte CFF table tag (`b"CFF "`, note the trailing space).
pub const CFF_TABLE_TAG: [u8; 4] = *b"CFF ";

#[cfg(test)]
mod tests {
    use super::*;

    // --- INDEX -------------------------------------------------------

    /// Build a minimal INDEX from a list of objects (off_size = 1).
    fn build_index(objs: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(objs.len() as u16).to_be_bytes());
        if objs.is_empty() {
            return out;
        }
        out.push(1); // off_size
        let mut off = 1u8;
        out.push(off);
        for o in objs {
            off += o.len() as u8;
            out.push(off);
        }
        for o in objs {
            out.extend_from_slice(o);
        }
        out
    }

    #[test]
    fn index_roundtrip() {
        let data = build_index(&[b"hello", b"hi", b"world"]);
        let mut pos = 0;
        let idx = Index::parse(&data, &mut pos).expect("parse");
        assert_eq!(idx.count(), 3);
        assert_eq!(idx.get(0), Some(&b"hello"[..]));
        assert_eq!(idx.get(1), Some(&b"hi"[..]));
        assert_eq!(idx.get(2), Some(&b"world"[..]));
        assert_eq!(idx.get(3), None);
        assert_eq!(pos, data.len());
    }

    #[test]
    fn empty_index() {
        let data = build_index(&[]);
        let mut pos = 0;
        let idx = Index::parse(&data, &mut pos).expect("parse");
        assert_eq!(idx.count(), 0);
        assert_eq!(idx.get(0), None);
        assert_eq!(pos, 2);
    }

    // --- DICT --------------------------------------------------------

    #[test]
    fn dict_integer_operands() {
        // 139 -> 0 (one-byte 0), operator 17 (CharStrings).
        // encode value 100: byte 100+139 = 239.
        let bytes = [239u8, 17];
        let d = Dict::parse(&bytes).expect("parse");
        assert_eq!(d.first_int(op::CHAR_STRINGS), Some(100));
    }

    #[test]
    fn dict_two_byte_operator() {
        // CharstringType (12 6) = 2. Encode 2 via byte 2+139=141.
        let bytes = [141u8, 12, 6];
        let d = Dict::parse(&bytes).expect("parse");
        assert_eq!(d.first_int(op::CHARSTRING_TYPE), Some(2));
    }

    #[test]
    fn dict_negative_and_large() {
        // -1000: 2-byte 251..254 form. -(b0-251)*256 - w - 108 = -1000
        // pick b0=254 -> -(3)*256 - w - 108 = -768-108-w = -876-w
        // need -1000 => w = 124. So bytes 254,124.
        // 5-byte int 100000: byte 29 + i32 be.
        let mut bytes = vec![254u8, 124]; // -1000
        bytes.push(29);
        bytes.extend_from_slice(&100_000i32.to_be_bytes());
        bytes.push(13); // UniqueID operator (key 13)
        let d = Dict::parse(&bytes).expect("parse");
        let ops = d.operands(13).expect("uniqueid");
        assert_eq!(ops[0], -1000.0);
        assert_eq!(ops[1], 100_000.0);
    }

    #[test]
    fn dict_real_operand() {
        // 0.5 encoded as 30 (real) then nibbles: '0','.','5','f'
        // 0 . 5 f -> 0x0a, 0x5f
        let bytes = [30u8, 0x0a, 0x5f, 12, 9]; // 12 9 = abs (arbitrary op key 1209)
        let d = Dict::parse(&bytes).expect("parse");
        assert_eq!(d.first_float(1209), Some(0.5));
    }

    // --- subr bias ---------------------------------------------------

    #[test]
    fn bias_values() {
        assert_eq!(subr_bias(0), 107);
        assert_eq!(subr_bias(1239), 107);
        assert_eq!(subr_bias(1240), 1131);
        assert_eq!(subr_bias(33899), 1131);
        assert_eq!(subr_bias(33900), 32768);
    }

    // --- full minimal CFF font --------------------------------------

    /// Assemble a tiny non-CID CFF with two glyphs: GID 0 = empty
    /// `.notdef` (just endchar), GID 1 = a unit square drawn with rmoveto
    /// + rlineto + endchar. Returns the CFF table bytes.
    fn build_minimal_cff() -> Vec<u8> {
        // Charstring for GID0: endchar (14).
        let cs0: Vec<u8> = vec![14];
        // GID1: 100 100 rmoveto  500 0 rlineto  0 500 rlineto
        //       -500 0 rlineto   endchar
        // encode int v as v+139 for -107..107; 100 -> 239; 500 needs
        // 2-byte: 247..250 form. 500 = (b0-247)*256 + w + 108.
        //   pick b0=247 -> 0*256 + w + 108 = w+108 = 500 => w=392 >255 no.
        //   b0=248 -> 256 + w + 108 = 364+w = 500 => w=136. bytes 248,136.
        // -500: 251..254: -(b0-251)*256 - w -108 = -500
        //   b0=252 -> -256 - w -108 = -364-w = -500 => w=136. bytes 252,136.
        let i500 = [248u8, 136];
        let im500 = [252u8, 136];
        let i100 = [239u8];
        let i0 = [139u8];
        let mut cs1: Vec<u8> = Vec::new();
        cs1.extend_from_slice(&i100); // dx
        cs1.extend_from_slice(&i100); // dy
        cs1.push(21); // rmoveto
        cs1.extend_from_slice(&i500);
        cs1.extend_from_slice(&i0);
        cs1.push(5); // rlineto
        cs1.extend_from_slice(&i0);
        cs1.extend_from_slice(&i500);
        cs1.push(5);
        cs1.extend_from_slice(&im500);
        cs1.extend_from_slice(&i0);
        cs1.push(5);
        cs1.push(14); // endchar

        let charstrings = build_index(&[&cs0, &cs1]);

        // Build Private DICT (empty here -> size 0, so omit Subrs).
        // We still emit a Private DICT operand pointing at an empty dict.
        let private_dict: Vec<u8> = Vec::new(); // no entries

        // Layout: header(4) | Name INDEX | Top DICT INDEX | String INDEX
        //         | Global Subr INDEX | <Top-DICT-referenced blocks>
        // We place CharStrings + Private after the five fixed structures.
        let name = build_index(&[b"Test"]);
        let strings = build_index(&[]);
        let gsubrs = build_index(&[]);

        // We must know absolute offsets, so compute the prefix length
        // first with a placeholder Top DICT, then patch.
        // Fixed header = 4 bytes.
        let header = vec![1u8, 0, 4, 1];

        // Compute the offset where post-fixed blocks begin. The Top DICT
        // INDEX size depends on the Top DICT contents (which carry the
        // offsets). Build the Top DICT with 5-byte int operands (op 29)
        // so its length is constant regardless of the offset value.
        fn enc5(v: i32) -> Vec<u8> {
            let mut b = vec![29u8];
            b.extend_from_slice(&v.to_be_bytes());
            b
        }
        // Top DICT: CharStrings(17) Private(18) size off.
        let make_top = |cs_off: i32, priv_size: i32, priv_off: i32| -> Vec<u8> {
            let mut d = Vec::new();
            d.extend_from_slice(&enc5(cs_off));
            d.push(17); // CharStrings
            d.extend_from_slice(&enc5(priv_size));
            d.extend_from_slice(&enc5(priv_off));
            d.push(18); // Private
            d
        };
        let top_dict_placeholder = make_top(0, 0, 0);
        let top_index_placeholder = build_index(&[&top_dict_placeholder]);

        let prefix_len =
            header.len() + name.len() + top_index_placeholder.len() + strings.len() + gsubrs.len();

        // CharStrings goes right after the prefix; Private after that.
        let cs_off = prefix_len as i32;
        let priv_off = (prefix_len + charstrings.len()) as i32;
        let priv_size = private_dict.len() as i32;

        let top_dict = make_top(cs_off, priv_size, priv_off);
        let top_index = build_index(&[&top_dict]);
        assert_eq!(top_index.len(), top_index_placeholder.len());

        let mut cff = Vec::new();
        cff.extend_from_slice(&header);
        cff.extend_from_slice(&name);
        cff.extend_from_slice(&top_index);
        cff.extend_from_slice(&strings);
        cff.extend_from_slice(&gsubrs);
        cff.extend_from_slice(&charstrings);
        cff.extend_from_slice(&private_dict);
        cff
    }

    #[test]
    fn minimal_cff_outline() {
        let data = build_minimal_cff();
        let cff = CffTable::parse(&data).expect("parse cff");
        assert_eq!(cff.glyph_count(), 2);
        assert!(!cff.is_cid());

        // GID0 .notdef: empty outline.
        let g0 = cff.glyph_outline(0).expect("gid0");
        assert!(g0.is_empty());

        // GID1: a square 100,100 -> 600,100 -> 600,600 -> 100,600.
        let g1 = cff.glyph_outline(1).expect("gid1");
        assert_eq!(g1.contours.len(), 1);
        let pts = &g1.contours[0].points;
        assert_eq!(pts.len(), 4);
        assert_eq!((pts[0].x, pts[0].y), (100, 100));
        assert_eq!((pts[1].x, pts[1].y), (600, 100));
        assert_eq!((pts[2].x, pts[2].y), (600, 600));
        assert_eq!((pts[3].x, pts[3].y), (100, 600));
        let b = g1.bounds.expect("bounds");
        assert_eq!((b.x_min, b.y_min, b.x_max, b.y_max), (100, 100, 600, 600));
    }

    #[test]
    fn standard_strings_resolve() {
        // SID 0/1/3 are predefined standard strings; spot-check a few.
        assert_eq!(strings::STANDARD_STRINGS[0], ".notdef");
        assert_eq!(strings::STANDARD_STRINGS[1], "space");
        assert_eq!(strings::STANDARD_STRINGS[3], "quotedbl");
        assert_eq!(strings::STANDARD_STRINGS[34], "A");
        assert_eq!(strings::N_STD_STRINGS, 391);
        assert_eq!(strings::STANDARD_STRINGS.len(), 391);
    }

    #[test]
    fn string_for_sid_standard_and_custom() {
        let data = build_minimal_cff();
        let cff = CffTable::parse(&data).expect("parse cff");
        // Standard SID.
        assert_eq!(cff.string_for_sid(1), Some("space"));
        assert_eq!(cff.string_for_sid(34), Some("A"));
        // The minimal font has an empty String INDEX, so a custom SID
        // (>= 391) is unresolvable.
        assert_eq!(cff.string_for_sid(391), None);
        // Out-of-range standard SID.
        assert_eq!(cff.string_for_sid(390), Some("Semibold"));
    }
}
