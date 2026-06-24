//! `CFF2` — Compact Font Format version 2 (variable PostScript outlines).
//!
//! CFF2 is the variable-font evolution of the `CFF ` table (OpenType CFF2
//! chapter). It drops the per-font name/string machinery and the
//! glyph-width prefix (advances come from `hmtx`/`HVAR`) and adds a
//! VariationStore plus the `blend` / `vsindex` charstring operators so a
//! single charstring can describe a glyph across the whole design space.
//!
//! ## Differences from `CFF ` (relevant to outline decode)
//!
//! - **Header** is fixed-size: `majorVersion(2) minorVersion(0)
//!   headerSize(5) topDictSize(uint16)`. The Top DICT immediately follows
//!   at offset 5; the Global Subr INDEX immediately follows the Top DICT.
//! - **No** Name INDEX, String INDEX, charset, or Encoding.
//! - The Top DICT **always** has an FDArray (Font DICT INDEX); FDSelect is
//!   optional (absent ⇒ every glyph uses Font DICT 0). Each Font DICT's
//!   Private DICT may set a default `vsindex` and point at local subrs.
//! - CharStrings are Type 2, but carry **no width** and **no `endchar`**
//!   (a charstring ends at its data boundary), and may use `blend` (16)
//!   and `vsindex` (15).
//!
//! ## What this module decodes
//!
//! The container walk (header → Top DICT → Global Subrs → CharStrings →
//! FDArray/FDSelect → per-FD Private DICT + local subrs + default
//! vsindex) and the **default-instance** outline of each glyph: the
//! charstring interpreter (shared with `CFF `) collapses every `blend` to
//! its default values, so the rendered outline matches the font at its
//! default variation coordinates. Region counts per `vsindex` are read
//! from the embedded VariationStore.

use super::cff::charstring::Interp;
use super::cff::{Dict, Index};
use super::mvar::ItemVariationStore;
use crate::outline::TtOutline;
use crate::parser::{read_u16, read_u8};
use crate::Error;

/// The 4-byte table tag.
pub const CFF2_TABLE_TAG: [u8; 4] = *b"CFF2";

/// Top DICT operator keys used by CFF2 (OpenType CFF2 chapter).
mod op {
    pub const CHAR_STRINGS: u16 = 17;
    pub const VSTORE: u16 = 24;
    pub const FD_ARRAY: u16 = 1236; // 12 36
    pub const FD_SELECT: u16 = 1237; // 12 37
                                     // Private DICT.
    pub const SUBRS: u16 = 19;
    pub const VS_INDEX: u16 = 22; // default vsindex for the Private DICT
}

/// Parsed `CFF2` table.
#[derive(Debug, Clone)]
pub struct Cff2Table<'a> {
    data: &'a [u8],
    char_strings: Index<'a>,
    global_subrs: Index<'a>,
    /// Per-Font-DICT local subr INDEX.
    fd_locals: Vec<Index<'a>>,
    /// Per-Font-DICT default vsindex.
    fd_vsindex: Vec<u16>,
    /// GID → Font DICT index (None ⇒ all glyphs use FD 0).
    fd_select: Option<FdSelect>,
    /// Region count `k` per `vsindex` (= per VariationStore subtable).
    /// Empty when the font has no VariationStore.
    vs_region_counts: Vec<usize>,
}

impl<'a> Cff2Table<'a> {
    /// Parse the CFF2 table from its raw slice.
    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
        // --- Header (fixed 5 bytes + topDictSize) --------------------
        let major = read_u8(data, 0)?;
        if major != 2 {
            return Err(Error::BadStructure("CFF2 major version not 2"));
        }
        let hdr_size = read_u8(data, 2)? as usize;
        let top_dict_size = read_u16(data, 3)? as usize;
        if hdr_size < 5 {
            return Err(Error::BadStructure("CFF2 header size too small"));
        }
        let top_start = hdr_size;
        let top_end = top_start
            .checked_add(top_dict_size)
            .ok_or(Error::BadStructure("CFF2 top DICT size overflow"))?;
        let top_bytes = data.get(top_start..top_end).ok_or(Error::UnexpectedEof)?;
        let top = Dict::parse(top_bytes)?;

        // --- Global Subr INDEX immediately follows the Top DICT ------
        let mut pos = top_end;
        let global_subrs = Index::parse_wide(data, &mut pos)?;

        // --- CharStrings INDEX ---------------------------------------
        let cs_off = top
            .first_int(op::CHAR_STRINGS)
            .ok_or(Error::BadStructure("CFF2 Top DICT missing CharStrings"))?
            as usize;
        let mut cs_pos = cs_off;
        let char_strings = Index::parse_wide(data, &mut cs_pos)?;
        let n_glyphs = char_strings.count();

        // --- VariationStore (region counts per vsindex) --------------
        let vs_region_counts = match top.first_int(op::VSTORE) {
            Some(off) if off > 0 => {
                let off = off as usize;
                // The vstore is a uint16 length prefix followed by the
                // ItemVariationStore. Skip the length word.
                let ivs_at = off + 2;
                let ivs =
                    ItemVariationStore::parse(data.get(ivs_at..).ok_or(Error::UnexpectedEof)?)?;
                (0..ivs.subtable_count())
                    .map(|i| ivs.region_index_count(i).unwrap_or(0))
                    .collect()
            }
            _ => Vec::new(),
        };

        // --- FDArray (always present in CFF2) ------------------------
        let fd_array_off =
            top.first_int(op::FD_ARRAY)
                .ok_or(Error::BadStructure("CFF2 Top DICT missing FDArray"))? as usize;
        let mut fd_pos = fd_array_off;
        let fd_array = Index::parse_wide(data, &mut fd_pos)?;
        let mut fd_locals = Vec::with_capacity(fd_array.count());
        let mut fd_vsindex = Vec::with_capacity(fd_array.count());
        for i in 0..fd_array.count() {
            let fd_bytes = fd_array
                .get(i)
                .ok_or(Error::BadStructure("CFF2 FDArray entry"))?;
            let fd_dict = Dict::parse(fd_bytes)?;
            let (locals, vsindex) = parse_private(data, &fd_dict)?;
            fd_locals.push(locals);
            fd_vsindex.push(vsindex);
        }
        if fd_locals.is_empty() {
            // A CFF2 font must define at least one Font DICT.
            return Err(Error::BadStructure("CFF2 FDArray empty"));
        }

        // --- FDSelect (optional) -------------------------------------
        let fd_select = match top.first_int(op::FD_SELECT) {
            Some(off) if off > 0 => Some(FdSelect::parse(data, off as usize, n_glyphs)?),
            _ => None,
        };

        Ok(Self {
            data,
            char_strings,
            global_subrs,
            fd_locals,
            fd_vsindex,
            fd_select,
            vs_region_counts,
        })
    }

    /// Number of glyphs (CharStrings INDEX count).
    pub fn glyph_count(&self) -> u16 {
        self.char_strings.count().min(u16::MAX as usize) as u16
    }

    /// The raw table slice (mostly for tests / diagnostics).
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Number of variation regions per `vsindex` (empty for a
    /// non-variable CFF2). Mostly useful for diagnostics / tests.
    pub fn region_counts(&self) -> &[usize] {
        &self.vs_region_counts
    }

    /// Reconstruct the **default-instance** outline of glyph `gid`.
    /// `None` when `gid` is out of range.
    pub fn glyph_outline(&self, gid: u16) -> Option<TtOutline> {
        let cs = self.char_strings.get(gid as usize)?;
        let fd = self.fd_for_gid(gid);
        let locals = self
            .fd_locals
            .get(fd)
            .copied()
            .unwrap_or_else(Index::empty_pub);
        // Re-order region counts so index 0 is the FontDICT's default
        // vsindex (the interpreter starts with active vsindex 0). We keep
        // the full table but bias the interpreter's starting `active_k`
        // by passing the region counts and letting an explicit `vsindex`
        // in the charstring override; the default vsindex is folded by
        // putting its count first.
        let mut region_counts = self.vs_region_counts.clone();
        let default_vs = *self.fd_vsindex.get(fd).unwrap_or(&0) as usize;
        if default_vs != 0 && default_vs < region_counts.len() {
            region_counts.swap(0, default_vs);
        }
        let mut interp = Interp::new_cff2(self.global_subrs, locals, region_counts);
        interp.run(cs).ok()?;
        Some(interp.into_outline())
    }

    fn fd_for_gid(&self, gid: u16) -> usize {
        match &self.fd_select {
            Some(s) => s.fd_for_gid(gid) as usize,
            None => 0,
        }
    }
}

/// Parse a CFF2 Private DICT referenced by a Font DICT, returning
/// `(local_subrs, default_vsindex)`.
fn parse_private<'a>(data: &'a [u8], fd: &Dict) -> Result<(Index<'a>, u16), Error> {
    // Private DICT operand: size + offset (same shape as CFF).
    let priv_ops = match fd.operands(18) {
        Some(v) if v.len() >= 2 => v,
        _ => return Ok((Index::empty_pub(), 0)),
    };
    let size = priv_ops[0] as usize;
    let off = priv_ops[1] as usize;
    if size == 0 {
        return Ok((Index::empty_pub(), 0));
    }
    let pd = data.get(off..off + size).ok_or(Error::UnexpectedEof)?;
    let priv_dict = Dict::parse(pd)?;
    let vsindex = priv_dict.first_int(op::VS_INDEX).unwrap_or(0).max(0) as u16;
    let locals = match priv_dict.first_int(op::SUBRS) {
        Some(subr_off) => {
            let mut p = off + subr_off as usize;
            Index::parse_wide(data, &mut p)?
        }
        None => Index::empty_pub(),
    };
    Ok((locals, vsindex))
}

/// FDSelect (formats 0, 3, 4 per CFF2). We support formats 0 and 3 (the
/// common cases); format 4 (32-bit ranges) is rare and rejected.
#[derive(Debug, Clone)]
enum FdSelect {
    Format0(Vec<u8>),
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
            _ => Err(Error::BadStructure("CFF2 FDSelect format unsupported")),
        }
    }

    fn fd_for_gid(&self, gid: u16) -> u8 {
        match self {
            FdSelect::Format0(arr) => arr.get(gid as usize).copied().unwrap_or(0),
            FdSelect::Format3 { ranges, sentinel } => {
                if gid >= *sentinel {
                    return 0;
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a CFF2 (Card32-count) INDEX from a list of objects.
    fn build_index(objs: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(objs.len() as u32).to_be_bytes());
        if objs.is_empty() {
            return out;
        }
        out.push(1);
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

    fn enc5(v: i32) -> Vec<u8> {
        let mut b = vec![29u8];
        b.extend_from_slice(&v.to_be_bytes());
        b
    }

    /// Build a non-variable CFF2 with one Font DICT and two glyphs:
    /// GID0 empty, GID1 a square drawn with rmoveto + rlineto (no width,
    /// no endchar — CFF2 charstrings end at their data boundary).
    fn build_minimal_cff2() -> Vec<u8> {
        // Charstrings. GID0: empty. GID1: 100 100 rmoveto 500 0 rlineto
        // 0 500 rlineto -500 0 rlineto  (ends at data boundary).
        let i100 = [239u8];
        let i0 = [139u8];
        let i500 = [248u8, 136];
        let im500 = [252u8, 136];
        let cs0: Vec<u8> = Vec::new();
        let mut cs1 = Vec::new();
        cs1.extend_from_slice(&i100);
        cs1.extend_from_slice(&i100);
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
        let charstrings = build_index(&[&cs0, &cs1]);

        // Font DICT (no Private) and FDArray.
        let fd_dict: Vec<u8> = Vec::new(); // empty Private => default subrs
        let fd_array = build_index(&[&fd_dict]);

        let gsubrs = build_index(&[]);

        // Top DICT with CharStrings(17) + FDArray(12 36), 5-byte ints so
        // its length is fixed.
        let make_top = |cs_off: i32, fd_off: i32| -> Vec<u8> {
            let mut d = Vec::new();
            d.extend_from_slice(&enc5(cs_off));
            d.push(17); // CharStrings
            d.extend_from_slice(&enc5(fd_off));
            d.extend_from_slice(&[12, 36]); // FDArray
            d
        };
        let top_placeholder = make_top(0, 0);

        // Header: major=2 minor=0 headerSize=5 topDictSize.
        let header_len = 5;
        let top_dict_size = top_placeholder.len();
        let top_start = header_len;
        let top_end = top_start + top_dict_size;
        // Global subrs after top dict.
        let gsub_start = top_end;
        let gsub_end = gsub_start + gsubrs.len();
        // CharStrings then FDArray.
        let cs_off = gsub_end;
        let fd_off = cs_off + charstrings.len();

        let top = make_top(cs_off as i32, fd_off as i32);
        assert_eq!(top.len(), top_dict_size);

        let mut out = Vec::new();
        out.push(2); // major
        out.push(0); // minor
        out.push(5); // headerSize
        out.extend_from_slice(&(top_dict_size as u16).to_be_bytes());
        out.extend_from_slice(&top);
        out.extend_from_slice(&gsubrs);
        out.extend_from_slice(&charstrings);
        out.extend_from_slice(&fd_array);
        out
    }

    #[test]
    fn minimal_cff2_outline() {
        let data = build_minimal_cff2();
        let cff2 = Cff2Table::parse(&data).expect("parse cff2");
        assert_eq!(cff2.glyph_count(), 2);
        assert!(cff2.region_counts().is_empty());

        let g0 = cff2.glyph_outline(0).expect("gid0");
        assert!(g0.is_empty());

        let g1 = cff2.glyph_outline(1).expect("gid1");
        assert_eq!(g1.contours.len(), 1);
        let pts = &g1.contours[0].points;
        assert_eq!(pts.len(), 4);
        assert_eq!((pts[0].x, pts[0].y), (100, 100));
        assert_eq!((pts[1].x, pts[1].y), (600, 100));
        assert_eq!((pts[2].x, pts[2].y), (600, 600));
        assert_eq!((pts[3].x, pts[3].y), (100, 600));
    }

    #[test]
    fn rejects_wrong_version() {
        let mut data = vec![0u8; 8];
        data[0] = 1; // major = 1 (that's CFF, not CFF2)
        assert!(Cff2Table::parse(&data).is_err());
    }
}
