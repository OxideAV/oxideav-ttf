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
//! vsindex) and the outline of each glyph **at an arbitrary variation
//! instance**: the charstring interpreter (shared with `CFF `) evaluates
//! every `blend` as `default + Σ scalarᵣ · deltaᵣ`, where the per-region
//! scalars come from the embedded VariationStore at the caller's
//! normalised coordinates. [`Cff2Table::glyph_outline`] renders the
//! default instance (all scalars zero, blends collapse to their
//! defaults); [`Cff2Table::glyph_outline_at`] renders any instance.
//! Region counts per `vsindex` are read from the VariationStore.

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
    /// The embedded VariationStore, when the font carries variations. Used
    /// to compute per-`vsindex` region scalars for the `blend` operator
    /// at a target instance. `None` for a non-variable CFF2.
    ivs: Option<ItemVariationStore>,
    /// Number of `vsindex` slots (= VariationStore subtable count), cached
    /// so callers can size their per-vsindex scalar vectors.
    vsindex_count: usize,
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

        // --- VariationStore ------------------------------------------
        let ivs = match top.first_int(op::VSTORE) {
            Some(off) if off > 0 => {
                let off = off as usize;
                // The vstore is a uint16 length prefix followed by the
                // ItemVariationStore. Skip the length word.
                let ivs_at = off + 2;
                Some(ItemVariationStore::parse(
                    data.get(ivs_at..).ok_or(Error::UnexpectedEof)?,
                )?)
            }
            _ => None,
        };
        let vsindex_count = ivs.as_ref().map(|s| s.subtable_count()).unwrap_or(0);

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
            ivs,
            vsindex_count,
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

    /// Number of `vsindex` slots (= VariationStore subtable count); 0 for
    /// a non-variable CFF2. Mostly useful for diagnostics / tests.
    pub fn vsindex_count(&self) -> usize {
        self.vsindex_count
    }

    /// Number of regions referenced by `vsindex`, or 0 when out of range.
    pub fn region_count(&self, vsindex: usize) -> usize {
        self.ivs
            .as_ref()
            .and_then(|s| s.region_index_count(vsindex))
            .unwrap_or(0)
    }

    /// Reconstruct the **default-instance** outline of glyph `gid`.
    /// `None` when `gid` is out of range.
    pub fn glyph_outline(&self, gid: u16) -> Option<TtOutline> {
        self.glyph_outline_at(gid, &[])
    }

    /// Reconstruct the outline of glyph `gid` at the variation instance
    /// given by `normalised_coords` (one normalised value per font axis,
    /// already avar-bent). Passing an empty slice — or a font with no
    /// VariationStore — yields the default-instance outline. `None` when
    /// `gid` is out of range.
    pub fn glyph_outline_at(&self, gid: u16, normalised_coords: &[f32]) -> Option<TtOutline> {
        let cs = self.char_strings.get(gid as usize)?;
        let fd = self.fd_for_gid(gid);
        let locals = self
            .fd_locals
            .get(fd)
            .copied()
            .unwrap_or_else(Index::empty_pub);
        // Per-vsindex region scalars at this instance. The interpreter
        // starts at vsindex 0; the FontDICT's default vsindex is folded
        // into slot 0 so a charstring that never issues an explicit
        // `vsindex` still uses the right region set.
        let mut scalars: Vec<Vec<f32>> = (0..self.vsindex_count)
            .map(|i| match &self.ivs {
                Some(s) => s.region_scalars(i, normalised_coords),
                None => Vec::new(),
            })
            .collect();
        let default_vs = *self.fd_vsindex.get(fd).unwrap_or(&0) as usize;
        if default_vs != 0 && default_vs < scalars.len() {
            scalars.swap(0, default_vs);
        }
        let mut interp = Interp::new_cff2(self.global_subrs, locals, scalars);
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
        assert_eq!(cff2.vsindex_count(), 0);

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

    /// Build a single-region ItemVariationStore (no length prefix —
    /// the caller prepends the uint16 vstore length): one axis, one
    /// region (rising edge peaking at +1), one IVD subtable carrying a
    /// single delta row `[delta]`. vsindex 0 → this subtable.
    fn build_single_region_ivs(delta: i16) -> Vec<u8> {
        let mut b = vec![0u8; 32];
        b[0..2].copy_from_slice(&1u16.to_be_bytes()); // format
        b[2..6].copy_from_slice(&12u32.to_be_bytes()); // regionListOffset
        b[6..8].copy_from_slice(&1u16.to_be_bytes()); // ivdCount
        b[8..12].copy_from_slice(&22u32.to_be_bytes()); // ivdOffsets[0]
        b[12..14].copy_from_slice(&1u16.to_be_bytes()); // axisCount
        b[14..16].copy_from_slice(&1u16.to_be_bytes()); // regionCount
        b[16..18].copy_from_slice(&0i16.to_be_bytes()); // start
        b[18..20].copy_from_slice(&16384i16.to_be_bytes()); // peak +1
        b[20..22].copy_from_slice(&16384i16.to_be_bytes()); // end +1
        b[22..24].copy_from_slice(&1u16.to_be_bytes()); // itemCount
        b[24..26].copy_from_slice(&1u16.to_be_bytes()); // shortDeltaCount
        b[26..28].copy_from_slice(&1u16.to_be_bytes()); // regionIndexCount
        b[28..30].copy_from_slice(&0u16.to_be_bytes()); // regionIndexes[0]
        b[30..32].copy_from_slice(&delta.to_be_bytes()); // delta row 0
        b
    }

    /// Build a *variable* CFF2 with a VariationStore (1 region) and one
    /// glyph (GID1) whose first move's x-coordinate is `blend`-ed:
    /// `x = 100 + scalar·delta_x`. At the default instance x = 100; at
    /// the axis extreme x = 100 + `delta_x`.
    fn build_variable_cff2(delta_x: i32, region_delta: i16) -> Vec<u8> {
        // GID1 charstring:
        //   100 <delta_x> 1 blend   -> x (blended)
        //   100                     -> y
        //   rmoveto
        //   500 0 rlineto 0 500 rlineto -500 0 rlineto
        let i100 = [239u8]; // 100
        let i0 = [139u8]; // 0
        let i500 = [248u8, 136]; // 500
        let im500 = [252u8, 136]; // -500
                                  // Type2 charstrings encode a 16-bit integer as [28, hi, lo]
                                  // (NOT the DICT 5-byte form `enc5`, which uses operator 29).
        let cs_int16 = |v: i32| -> Vec<u8> {
            let mut b = vec![28u8];
            b.extend_from_slice(&(v as i16).to_be_bytes());
            b
        };
        let cs0: Vec<u8> = Vec::new();
        let mut cs1 = Vec::new();
        cs1.extend_from_slice(&i100); // default x
        cs1.extend_from_slice(&cs_int16(delta_x)); // region-0 delta for x
        cs1.extend_from_slice(&[139 + 1]); // n = 1 (operand count to blend)
        cs1.push(16); // blend → leaves blended x on stack
        cs1.extend_from_slice(&i100); // y
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

        let fd_dict: Vec<u8> = Vec::new();
        let fd_array = build_index(&[&fd_dict]);
        let gsubrs = build_index(&[]);

        // vstore = uint16 length + ItemVariationStore.
        let ivs = build_single_region_ivs(region_delta);
        let mut vstore = Vec::new();
        vstore.extend_from_slice(&(ivs.len() as u16).to_be_bytes());
        vstore.extend_from_slice(&ivs);

        // Top DICT: CharStrings(17), vstore(24), FDArray(12 36) — all
        // 5-byte ints so the size is stable across the two passes.
        let make_top = |cs_off: i32, vs_off: i32, fd_off: i32| -> Vec<u8> {
            let mut d = Vec::new();
            d.extend_from_slice(&enc5(cs_off));
            d.push(17);
            d.extend_from_slice(&enc5(vs_off));
            d.push(24);
            d.extend_from_slice(&enc5(fd_off));
            d.extend_from_slice(&[12, 36]);
            d
        };
        let top_placeholder = make_top(0, 0, 0);
        let header_len = 5;
        let top_dict_size = top_placeholder.len();
        let top_end = header_len + top_dict_size;
        let gsub_start = top_end;
        let gsub_end = gsub_start + gsubrs.len();
        let cs_off = gsub_end;
        let vs_off = cs_off + charstrings.len();
        let fd_off = vs_off + vstore.len();
        let top = make_top(cs_off as i32, vs_off as i32, fd_off as i32);
        assert_eq!(top.len(), top_dict_size);

        let mut out = Vec::new();
        out.push(2);
        out.push(0);
        out.push(5);
        out.extend_from_slice(&(top_dict_size as u16).to_be_bytes());
        out.extend_from_slice(&top);
        out.extend_from_slice(&gsubrs);
        out.extend_from_slice(&charstrings);
        out.extend_from_slice(&vstore);
        out.extend_from_slice(&fd_array);
        out
    }

    #[test]
    fn variable_cff2_blends_x_at_instance() {
        // x = 100 + scalar·(+400); region scalar is 0 at default, 1 at
        // the axis extreme, 0.5 halfway.
        let data = build_variable_cff2(400, 0);
        let cff2 = Cff2Table::parse(&data).expect("parse variable cff2");
        assert_eq!(cff2.vsindex_count(), 1);
        assert_eq!(cff2.region_count(0), 1);

        // Default instance: x = 100.
        let g_def = cff2.glyph_outline_at(1, &[0.0]).expect("gid1 default");
        assert_eq!(g_def.contours[0].points[0].x, 100);

        // In CFF2 the per-region deltas are *charstring* operands and
        // the VariationStore supplies the per-region *scalars*: the
        // blended value is `default + Σ scalar_r · delta_r`. Here the
        // single region's scalar rises 0→1 across the axis, so the
        // charstring delta (+400) is applied proportionally.
        let g_max = cff2.glyph_outline_at(1, &[1.0]).expect("gid1 max");
        assert_eq!(g_max.contours[0].points[0].x, 500); // 100 + 1·400
        let g_half = cff2.glyph_outline_at(1, &[0.5]).expect("gid1 half");
        assert_eq!(g_half.contours[0].points[0].x, 300); // 100 + 0.5·400
                                                         // y is unaffected by the blend.
        assert_eq!(g_max.contours[0].points[0].y, 100);
    }

    #[test]
    fn rejects_wrong_version() {
        let mut data = vec![0u8; 8];
        data[0] = 1; // major = 1 (that's CFF, not CFF2)
        assert!(Cff2Table::parse(&data).is_err());
    }
}
