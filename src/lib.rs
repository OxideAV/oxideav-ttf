//! Pure-Rust TrueType / OpenType font parser.
//!
//! Round-1 scope:
//! - sfnt + table directory walker (`parser`).
//! - Core OpenType tables: `head`, `hhea`, `maxp`, `cmap` (formats 0/4/6/12),
//!   `name`, `OS/2`, `hmtx`, `loca`, `glyf` (simple + composite), `post`.
//! - Legacy `kern` table (format 0 subtable).
//! - `GSUB` LookupType 4 (ligature substitution).
//! - `GPOS` LookupType 2 (pair-adjustment / kerning),
//!   LookupType 4 (mark-to-base attachment for diacritics), and
//!   LookupType 6 (mark-to-mark attachment for stacked diacritics).
//! - `GDEF` (glyph class definitions).
//!
//! The crate is read-only (parsing-only) and dependency-light: only
//! `oxideav-core` for shared types. CFF/Type 2 charstrings, variable
//! fonts, TrueType hinting, bidi, and complex shaping are deferred to
//! later rounds and to the sibling `oxideav-otf` crate.
//!
//! See `README.md` for the public API tour.

#![deny(missing_debug_implementations)]
#![warn(rust_2018_idioms)]

pub mod outline;
pub mod parser;
pub mod tables;

use crate::parser::TableDirectory;
use crate::tables::{
    cmap::CmapTable, gdef::GdefTable, glyf::GlyfTable, gpos::GposTable, gsub::GsubTable,
    head::HeadTable, hhea::HheaTable, hmtx::HmtxTable, kern::KernTable, loca::LocaTable,
    maxp::MaxpTable, name::NameTable, os2::Os2Table, post::PostTable,
};

pub use outline::{BBox, Contour, Point, TtOutline};

/// Errors emitted during font parsing or glyph lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The input slice is too short for the requested header / structure.
    UnexpectedEof,
    /// The sfnt magic version did not match `0x00010000`, `OTTO`, or `true`.
    BadMagic,
    /// The table count in the sfnt header is implausibly large.
    BadHeader,
    /// A required table was missing from the table directory.
    MissingTable(&'static str),
    /// A length / offset field pointed outside the file.
    BadOffset,
    /// A glyph index was out of range vs. `maxp.numGlyphs`.
    GlyphOutOfRange(u16),
    /// A cmap subtable used a format we do not implement in round 1.
    UnsupportedCmapFormat(u16),
    /// A composite-glyph chain exceeded the max recursion depth (16).
    CompositeTooDeep,
    /// A loca offset pointed past the end of `glyf`.
    BadLocaOffset,
    /// A varying-length structure was malformed.
    BadStructure(&'static str),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnexpectedEof => f.write_str("unexpected end of font data"),
            Self::BadMagic => f.write_str("not a TrueType / OpenType font (bad magic)"),
            Self::BadHeader => f.write_str("malformed sfnt header"),
            Self::MissingTable(t) => write!(f, "required table missing: {t}"),
            Self::BadOffset => f.write_str("table offset out of range"),
            Self::GlyphOutOfRange(g) => write!(f, "glyph index {g} out of range"),
            Self::UnsupportedCmapFormat(fmt) => {
                write!(f, "cmap format {fmt} not implemented in round 1")
            }
            Self::CompositeTooDeep => f.write_str("composite glyph recursion too deep"),
            Self::BadLocaOffset => f.write_str("loca offset past end of glyf"),
            Self::BadStructure(s) => write!(f, "malformed structure: {s}"),
        }
    }
}

impl std::error::Error for Error {}

/// A parsed TrueType / OpenType font, lifetime-bound to the input bytes.
///
/// `Font::from_bytes` walks the sfnt header + table directory once; the
/// individual `*Table` parsers are run on first use and cached as
/// already-validated slices on the struct. Lookup methods (`glyph_index`,
/// `glyph_outline`, etc.) are O(log n) or O(n) over the raw table bytes —
/// no glyphs are pre-decoded or cached.
#[derive(Debug)]
pub struct Font<'a> {
    bytes: &'a [u8],
    head: HeadTable,
    hhea: HheaTable,
    maxp: MaxpTable,
    cmap: CmapTable<'a>,
    name: NameTable<'a>,
    os2: Option<Os2Table>,
    hmtx: HmtxTable<'a>,
    loca: LocaTable<'a>,
    glyf: GlyfTable<'a>,
    post: Option<PostTable>,
    kern: Option<KernTable<'a>>,
    gsub: Option<GsubTable<'a>>,
    gpos: Option<GposTable<'a>>,
    gdef: Option<GdefTable<'a>>,
}

impl<'a> Font<'a> {
    /// Parse a font from a borrowed byte slice.
    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self, Error> {
        let dir = TableDirectory::parse(bytes)?;

        let head = HeadTable::parse(dir.required(b"head", bytes)?)?;
        let hhea = HheaTable::parse(dir.required(b"hhea", bytes)?)?;
        let maxp = MaxpTable::parse(dir.required(b"maxp", bytes)?)?;
        let cmap = CmapTable::parse(dir.required(b"cmap", bytes)?)?;
        let name = NameTable::parse(dir.required(b"name", bytes)?)?;
        let hmtx = HmtxTable::parse(
            dir.required(b"hmtx", bytes)?,
            hhea.num_long_hor_metrics,
            maxp.num_glyphs,
        )?;
        let loca = LocaTable::parse(
            dir.required(b"loca", bytes)?,
            maxp.num_glyphs,
            head.index_to_loc_format,
        )?;
        let glyf = GlyfTable::new(dir.required(b"glyf", bytes)?);

        let os2 = dir.find(b"OS/2", bytes).map(Os2Table::parse).transpose()?;
        let post = dir.find(b"post", bytes).map(PostTable::parse).transpose()?;
        let kern = dir.find(b"kern", bytes).map(KernTable::parse).transpose()?;
        let gsub = dir.find(b"GSUB", bytes).map(GsubTable::parse).transpose()?;
        let gpos = dir.find(b"GPOS", bytes).map(GposTable::parse).transpose()?;
        let gdef = dir.find(b"GDEF", bytes).map(GdefTable::parse).transpose()?;

        Ok(Self {
            bytes,
            head,
            hhea,
            maxp,
            cmap,
            name,
            os2,
            hmtx,
            loca,
            glyf,
            post,
            kern,
            gsub,
            gpos,
            gdef,
        })
    }

    /// Raw bytes used to build this `Font`. Mostly useful for debugging.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    // ---- metadata ----------------------------------------------------------

    /// Family name from the `name` table (Windows English first, falls back
    /// to Mac Roman if that's all the font has).
    pub fn family_name(&self) -> Option<&str> {
        // 1 = Family name
        self.name.find(1)
    }

    /// Full name (typically family + style) from the `name` table.
    pub fn full_name(&self) -> Option<&str> {
        // 4 = Full name
        self.name.find(4)
    }

    /// `head.unitsPerEm`. Almost always 1024 or 2048; never zero in valid
    /// fonts.
    pub fn units_per_em(&self) -> u16 {
        self.head.units_per_em
    }

    /// Typographic ascent. We prefer `OS/2.sTypoAscender` if present
    /// (Windows-clean), falling back to `hhea.ascent`.
    pub fn ascent(&self) -> i16 {
        self.os2
            .as_ref()
            .and_then(|o| o.s_typo_ascender)
            .unwrap_or(self.hhea.ascent)
    }

    /// Typographic descent (typically negative).
    pub fn descent(&self) -> i16 {
        self.os2
            .as_ref()
            .and_then(|o| o.s_typo_descender)
            .unwrap_or(self.hhea.descent)
    }

    /// Suggested gap between lines.
    pub fn line_gap(&self) -> i16 {
        self.os2
            .as_ref()
            .and_then(|o| o.s_typo_line_gap)
            .unwrap_or(self.hhea.line_gap)
    }

    /// `maxp.numGlyphs`.
    pub fn glyph_count(&self) -> u16 {
        self.maxp.num_glyphs
    }

    /// `OS/2.usWeightClass` (100..1000), or 400 (Regular) if `OS/2` absent.
    pub fn weight_class(&self) -> u16 {
        self.os2.as_ref().map(|o| o.us_weight_class).unwrap_or(400)
    }

    /// `post.italicAngle` in degrees (negative for forward-slanted).
    pub fn italic_angle(&self) -> f32 {
        self.post.as_ref().map(|p| p.italic_angle).unwrap_or(0.0)
    }

    // ---- glyph lookup ------------------------------------------------------

    /// Map a Unicode codepoint to its glyph id.
    pub fn glyph_index(&self, codepoint: char) -> Option<u16> {
        self.cmap.lookup(codepoint as u32)
    }

    /// Decode the TrueType outline for `glyph_id`. Empty / blank glyphs
    /// (e.g. the space glyph) return an outline with zero contours.
    pub fn glyph_outline(&self, glyph_id: u16) -> Result<TtOutline, Error> {
        if glyph_id >= self.maxp.num_glyphs {
            return Err(Error::GlyphOutOfRange(glyph_id));
        }
        let range = self.loca.glyph_range(glyph_id)?;
        if range.is_empty() {
            return Ok(TtOutline::default());
        }
        self.glyf.glyph_outline(range, &self.loca, 0)
    }

    /// Per-glyph advance width in font units.
    pub fn glyph_advance(&self, glyph_id: u16) -> i16 {
        self.hmtx.advance(glyph_id) as i16
    }

    /// Per-glyph left-side bearing in font units.
    pub fn glyph_lsb(&self, glyph_id: u16) -> i16 {
        self.hmtx.lsb(glyph_id)
    }

    /// Glyph bounding box from the `glyf` header (xMin/yMin/xMax/yMax).
    /// Returns `None` for empty / blank glyphs.
    pub fn glyph_bounding_box(&self, glyph_id: u16) -> Option<BBox> {
        if glyph_id >= self.maxp.num_glyphs {
            return None;
        }
        let range = self.loca.glyph_range(glyph_id).ok()?;
        if range.is_empty() {
            return None;
        }
        self.glyf.bbox(range)
    }

    // ---- shaping support ---------------------------------------------------

    /// Look up a ligature substitution for the input glyph run.
    ///
    /// Returns `Some((replacement, consumed))` if a GSUB LookupType 4 rule
    /// matches a prefix of `glyphs` of length `consumed >= 2`. Returns
    /// `None` otherwise (no ligature, or no GSUB table).
    pub fn lookup_ligature(&self, glyphs: &[u16]) -> Option<(u16, usize)> {
        self.gsub.as_ref().and_then(|g| g.lookup_ligature(glyphs))
    }

    /// Look up the kerning between an ordered glyph pair, in font units.
    ///
    /// Tries GPOS LookupType 2 first; falls back to the legacy `kern`
    /// table (format 0). Returns 0 if neither is present or the pair has
    /// no defined kerning.
    pub fn lookup_kerning(&self, left: u16, right: u16) -> i16 {
        if let Some(gpos) = &self.gpos {
            let v = gpos.lookup_kerning(left, right, self.gdef.as_ref());
            if v != 0 {
                return v;
            }
        }
        if let Some(kern) = &self.kern {
            return kern.lookup(left, right);
        }
        0
    }

    /// Look up a mark-to-base attachment offset for a `(base, mark)`
    /// glyph pair. Returns `(dx, dy)` in font units (TT Y-up convention)
    /// to add to the mark's pen origin so its anchor lands on the
    /// base's anchor for the mark's class.
    ///
    /// Walks GPOS LookupType 4 sub-tables; returns `None` if no
    /// matching MarkBasePos rule covers both glyphs (or if the font has
    /// no GPOS table). Used by the consumer crate's shaper to position
    /// diacritics above / below their base glyph (essential for
    /// European Latin extended, Vietnamese, polytonic Greek).
    ///
    /// Whether `mark` is actually a mark glyph (per `GDEF`) is the
    /// caller's responsibility — typically the shaper checks
    /// [`Font::is_mark_glyph`] before calling this. The lookup itself
    /// works for any pair the font's MarkBasePos coverage tables
    /// list, regardless of GDEF.
    pub fn lookup_mark_to_base(&self, base: u16, mark: u16) -> Option<(i16, i16)> {
        self.gpos.as_ref()?.lookup_mark_to_base(base, mark)
    }

    /// Look up a mark-to-mark attachment offset for a `(mark1, mark2)`
    /// glyph pair, where `mark1` is the previously-positioned mark
    /// (already attached to a base via a prior mark-to-base lookup) and
    /// `mark2` is the mark we want to stack on top of (or below) it.
    /// Returns `(dx, dy)` in font units (TT Y-up convention) to add to
    /// `mark2`'s pen origin so its anchor lands on `mark1`'s anchor for
    /// `mark2`'s class.
    ///
    /// Walks GPOS LookupType 6 sub-tables; returns `None` if no
    /// matching MarkMarkPos rule covers both glyphs (or if the font
    /// has no GPOS table). Used by the consumer crate's shaper to
    /// build multi-mark stacks (e.g. polytonic Greek `α + tonos +
    /// dialytika`, Vietnamese `a + circumflex + acute`).
    pub fn lookup_mark_to_mark(&self, mark1: u16, mark2: u16) -> Option<(i16, i16)> {
        self.gpos.as_ref()?.lookup_mark_to_mark(mark1, mark2)
    }

    /// Is this glyph classified as a mark by the font's `GDEF` table?
    /// Returns `false` if the font has no GDEF or the glyph isn't
    /// enumerated. Used by the consumer crate's shaper to decide
    /// whether to attempt mark-to-base attachment for an adjacent
    /// glyph pair.
    pub fn is_mark_glyph(&self, glyph_id: u16) -> bool {
        self.gdef
            .as_ref()
            .map(|g| g.is_mark(glyph_id))
            .unwrap_or(false)
    }
}
