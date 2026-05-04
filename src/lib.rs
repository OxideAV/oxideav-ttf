//! Pure-Rust TrueType / OpenType font parser.
//!
//! Round-1 scope:
//! - sfnt + table directory walker (`parser`).
//! - Core OpenType tables: `head`, `hhea`, `maxp`, `cmap` (base formats
//!   0/4/6/12 + format 14 Unicode Variation Sequences as a sidecar),
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

pub mod collection;
pub mod outline;
pub mod parser;
pub mod tables;

pub use collection::{is_collection, CollectionHeader, TTC_MAGIC};

use crate::parser::TableDirectory;
use crate::tables::{
    cbdt::CbdtTable, cblc::CblcTable, cmap::CmapTable, colr::ColrTable, cpal::CpalTable,
    gdef::GdefTable, glyf::GlyfTable, gpos::GposTable, gsub::GsubTable, head::HeadTable,
    hhea::HheaTable, hmtx::HmtxTable, kern::KernTable, loca::LocaTable, maxp::MaxpTable,
    name::NameTable, os2::Os2Table, post::PostTable, sbix::SbixTable,
};

pub use outline::{BBox, Contour, Point, TtOutline};
pub use tables::cbdt::ColorBitmap;
pub use tables::cblc::{BigGlyphMetrics, SmallGlyphMetrics};
pub use tables::colr::ColorLayer;
pub use tables::sbix::SbixGlyph;

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
    /// A `from_collection_bytes` call asked for a subfont index that
    /// the TTC header does not contain. Carries the requested index.
    SubfontOutOfRange(u32),
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
            Self::SubfontOutOfRange(i) => write!(f, "subfont index {i} not in collection"),
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
    /// Glyph-location offsets into `glyf`. Optional because CBDT/CBLC-only
    /// colour-emoji fonts (e.g. NotoColorEmoji.ttf) ship without `loca`
    /// and `glyf` — every glyph is a colour bitmap and there are no
    /// outlines to address.
    loca: Option<LocaTable<'a>>,
    glyf: Option<GlyfTable<'a>>,
    post: Option<PostTable>,
    kern: Option<KernTable<'a>>,
    gsub: Option<GsubTable<'a>>,
    gpos: Option<GposTable<'a>>,
    gdef: Option<GdefTable<'a>>,
    cblc: Option<CblcTable<'a>>,
    cbdt: Option<CbdtTable<'a>>,
    colr: Option<ColrTable<'a>>,
    cpal: Option<CpalTable<'a>>,
    sbix: Option<SbixTable<'a>>,
}

impl<'a> Font<'a> {
    /// Parse the `index`-th subfont out of a TrueType Collection (`.ttc` /
    /// `'ttcf'`) byte slice.
    ///
    /// TTC files start with a `'ttcf'` magic followed by a list of byte
    /// offsets pointing at per-subfont sfnt headers. This entry point
    /// reads the TTC header, then runs the regular sfnt parse path
    /// against the slice rooted at the chosen subfont. The returned
    /// `Font<'a>` borrows from the original `bytes` (sub-slicing is
    /// done internally; the lifetime stays tied to the input).
    ///
    /// Returns:
    /// - `Error::BadMagic` if `bytes` is not a TTC.
    /// - `Error::SubfontOutOfRange(index)` if the chosen index exceeds
    ///   `numFonts`.
    /// - Whatever the underlying sfnt path emits otherwise (typically
    ///   `MissingTable` / `BadOffset` for a malformed subfont).
    ///
    /// Spec: Microsoft OpenType §"Font Collections", Apple TrueType
    /// Reference / "TrueType Collections".
    pub fn from_collection_bytes(bytes: &'a [u8], index: u32) -> Result<Self, Error> {
        let header = CollectionHeader::parse(bytes)?;
        let offset = header
            .font_offset(index)
            .ok_or(Error::SubfontOutOfRange(index))? as usize;
        // The TTC spec requires the subfont's table directory offsets to
        // be FILE-relative (not subfont-relative), so we hand
        // `from_bytes_at` the full file slice and the subfont header
        // offset rather than slicing the file from `offset` onwards.
        Self::from_bytes_at(bytes, offset)
    }

    /// Parse a font from a borrowed byte slice.
    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self, Error> {
        Self::from_bytes_at(bytes, 0)
    }

    /// Parse a font whose sfnt header sits at `header_offset` inside
    /// `bytes`. Used by `from_collection_bytes` for TTC subfonts (whose
    /// table records carry file-relative offsets, not subfont-relative
    /// ones); equivalent to `from_bytes` when `header_offset == 0`.
    fn from_bytes_at(bytes: &'a [u8], header_offset: usize) -> Result<Self, Error> {
        let dir = TableDirectory::parse(bytes, header_offset)?;

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
        // `loca` + `glyf` are jointly optional: CBDT/CBLC-only colour-
        // emoji fonts (e.g. NotoColorEmoji.ttf) ship without either.
        // When loca is present we still require glyf (and vice versa)
        // because a half-pair would be malformed.
        let loca = match (dir.find(b"loca", bytes), dir.find(b"glyf", bytes)) {
            (Some(l), Some(_g)) => Some(LocaTable::parse(
                l,
                maxp.num_glyphs,
                head.index_to_loc_format,
            )?),
            (None, None) => None,
            _ => {
                return Err(Error::BadStructure(
                    "loca/glyf must both be present or both absent",
                ))
            }
        };
        let glyf = dir.find(b"glyf", bytes).map(GlyfTable::new);

        let os2 = dir.find(b"OS/2", bytes).map(Os2Table::parse).transpose()?;
        let post = dir.find(b"post", bytes).map(PostTable::parse).transpose()?;
        let kern = dir.find(b"kern", bytes).map(KernTable::parse).transpose()?;
        let gsub = dir.find(b"GSUB", bytes).map(GsubTable::parse).transpose()?;
        let gpos = dir.find(b"GPOS", bytes).map(GposTable::parse).transpose()?;
        let gdef = dir.find(b"GDEF", bytes).map(GdefTable::parse).transpose()?;
        let cblc = dir.find(b"CBLC", bytes).map(CblcTable::parse).transpose()?;
        let cbdt = dir.find(b"CBDT", bytes).map(CbdtTable::parse).transpose()?;
        let colr = dir.find(b"COLR", bytes).map(ColrTable::parse).transpose()?;
        let cpal = dir.find(b"CPAL", bytes).map(CpalTable::parse).transpose()?;
        let sbix = dir
            .find(b"sbix", bytes)
            .map(|s| SbixTable::parse(s, maxp.num_glyphs))
            .transpose()?;

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
            cblc,
            cbdt,
            colr,
            cpal,
            sbix,
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

    /// Look up the variant glyph for a `(codepoint, variation_selector)`
    /// pair from the cmap format-14 (Unicode Variation Sequences)
    /// subtable.
    ///
    /// Returns:
    ///
    /// - `Some(glyph)` from the **non-default** UVS table when the
    ///   variation selector overrides the base glyph (e.g. emoji
    ///   presentation `<emoji, U+FE0F>`, text presentation
    ///   `<emoji, U+FE0E>`, or registered Ideographic Variation
    ///   Sequence `<CJK, U+E0100..U+E01EF>`).
    /// - `Some(base)` when the pair is in the **default** UVS table —
    ///   semantically "render the base codepoint's default glyph; the
    ///   variation selector is just a hint". Equivalent to
    ///   [`Self::glyph_index`] for the base codepoint, returned for
    ///   API symmetry so callers don't have to special-case the
    ///   default-presentation branch.
    /// - `None` when the font has no format-14 subtable, the variation
    ///   selector isn't enumerated, or neither UVS table covers the
    ///   base codepoint.
    pub fn lookup_variation(&self, codepoint: char, variation_selector: char) -> Option<u16> {
        self.cmap
            .lookup_variation(codepoint as u32, variation_selector as u32)
    }

    /// Decode the TrueType outline for `glyph_id`. Empty / blank glyphs
    /// (e.g. the space glyph) return an outline with zero contours.
    ///
    /// Returns an empty outline when the font has no `glyf`/`loca`
    /// (CBDT/CBLC-only colour-emoji fonts). Callers that care should
    /// check [`Font::has_color_bitmaps`] first.
    pub fn glyph_outline(&self, glyph_id: u16) -> Result<TtOutline, Error> {
        if glyph_id >= self.maxp.num_glyphs {
            return Err(Error::GlyphOutOfRange(glyph_id));
        }
        let (loca, glyf) = match (self.loca.as_ref(), self.glyf.as_ref()) {
            (Some(l), Some(g)) => (l, g),
            _ => return Ok(TtOutline::default()),
        };
        let range = loca.glyph_range(glyph_id)?;
        if range.is_empty() {
            return Ok(TtOutline::default());
        }
        glyf.glyph_outline(range, loca, 0)
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
    /// Returns `None` for empty / blank glyphs and for fonts that lack
    /// a `glyf`/`loca` pair (CBDT-only colour-emoji fonts).
    pub fn glyph_bounding_box(&self, glyph_id: u16) -> Option<BBox> {
        if glyph_id >= self.maxp.num_glyphs {
            return None;
        }
        let (loca, glyf) = (self.loca.as_ref()?, self.glyf.as_ref()?);
        let range = loca.glyph_range(glyph_id).ok()?;
        if range.is_empty() {
            return None;
        }
        glyf.bbox(range)
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

    // ---- color bitmap glyphs (CBDT/CBLC) ---------------------------------

    /// `true` if this font ships a CBDT/CBLC pair — i.e. carries
    /// embedded colour bitmap glyphs (Noto Color Emoji, Apple Color
    /// Emoji's Google-format counterparts, and most Android emoji
    /// fonts). Returns `false` for plain outline-only fonts.
    pub fn has_color_bitmaps(&self) -> bool {
        self.cblc.is_some() && self.cbdt.is_some()
    }

    /// All `(ppem_x, ppem_y)` strikes the colour-bitmap tables ship.
    /// Returns an empty iterator when the font lacks CBDT/CBLC.
    /// Useful for picking a strike before calling
    /// [`Font::glyph_color_bitmap`].
    pub fn color_strike_sizes(&self) -> Vec<(u8, u8)> {
        self.cblc
            .as_ref()
            .map(|c| c.ppem_sizes().collect())
            .unwrap_or_default()
    }

    /// Resolve `glyph_id`'s colour bitmap at the strike whose `ppem_y`
    /// is closest to `target_ppem`. Returns `None` if the font has no
    /// CBDT/CBLC tables OR no strike contains `glyph_id` OR the strike's
    /// per-glyph entry is in a CBDT format we don't decode (anything
    /// other than 17/18/19 — the three PNG-payload formats).
    ///
    /// On success returns a [`ColorBitmap`] with raw `png_bytes` ready
    /// to feed into `oxideav-png` in the consumer crate. We deliberately
    /// don't decode the PNG here so this crate stays dependency-light.
    pub fn glyph_color_bitmap(&self, glyph_id: u16, target_ppem: u8) -> Option<ColorBitmap<'a>> {
        let cblc = self.cblc.as_ref()?;
        let cbdt = self.cbdt.as_ref()?;
        let entry = cblc.lookup_glyph(glyph_id, target_ppem)?;
        cbdt.lookup(&entry).ok().flatten()
    }

    // ---- color layer glyphs (COLR / CPAL) --------------------------------

    /// `true` if this font ships a `COLR` + `CPAL` pair — i.e. carries
    /// vector colour-emoji glyphs as a per-glyph layer stack
    /// (Microsoft's Segoe UI Emoji, Twemoji's Mozilla cut, FiraCode's
    /// "color" variant, and so on). Returns `false` for plain
    /// outline-only fonts and for CBDT-only colour-emoji fonts.
    ///
    /// Only **COLR version 0** (flat palette-indexed layer stack) is
    /// supported; v1 (paint graph with gradients/transforms) and v2/v3
    /// (variable-COLR) are accepted at parse time but the v0
    /// `BaseGlyphRecord` array is the only thing
    /// [`Font::color_layers`] returns. v1 paint graphs are out of
    /// scope for this crate.
    pub fn has_color_layers(&self) -> bool {
        self.colr.is_some() && self.cpal.is_some()
    }

    /// All colour layers for `glyph_id`, in back-to-front paint order.
    /// Each layer carries an outline-glyph id (whose outline you fetch
    /// via [`Font::glyph_outline`]) and a CPAL palette-entry index.
    /// The reserved palette index `0xFFFF` means "use the renderer's
    /// foreground colour" — substitute your own.
    ///
    /// Returns an empty `Vec` when the font has no `COLR` table or
    /// `glyph_id` isn't a base glyph (i.e. it's a single-colour
    /// outline glyph or a layer-only glyph used by other bases).
    pub fn color_layers(&self, glyph_id: u16) -> Vec<ColorLayer> {
        match self.colr.as_ref() {
            Some(colr) => colr.layers(glyph_id),
            None => Vec::new(),
        }
    }

    /// Resolve a single CPAL colour by `(palette_index, color_index)`.
    /// Returns `[r, g, b, a]` (the byte order swizzled out of CPAL's
    /// on-disk BGRA) or `None` when either index is out of range or the
    /// font has no `CPAL` table.
    ///
    /// Palette 0 is the spec's "default" palette. CPAL v1's palette
    /// flags (`USABLE_WITH_LIGHT_BACKGROUND`,
    /// `USABLE_WITH_DARK_BACKGROUND`) are exposed via
    /// [`Font::cpal_palette_type`] for renderers that want to pick a
    /// theme-appropriate palette.
    pub fn cpal_color(&self, palette_index: u16, color_index: u16) -> Option<[u8; 4]> {
        self.cpal.as_ref()?.color(palette_index, color_index)
    }

    /// All colours for palette `palette_index` as an `Vec<[u8; 4]>`
    /// (RGBA byte order). `None` if the font has no CPAL table or
    /// `palette_index` is out of range.
    pub fn cpal_palette(&self, palette_index: u16) -> Option<Vec<[u8; 4]>> {
        self.cpal.as_ref()?.palette(palette_index)
    }

    /// Number of CPAL palettes the font ships, or `0` if there's no
    /// `CPAL` table. Mostly useful for renderers that pick a palette
    /// based on `cpal_palette_type` flags.
    pub fn cpal_num_palettes(&self) -> u16 {
        self.cpal.as_ref().map(|c| c.num_palettes()).unwrap_or(0)
    }

    /// CPAL v1 palette-type flags for `palette_index`. Returns 0 when
    /// the font has no CPAL table, the table is v0, or the palette
    /// index is out of range.
    ///
    /// Bit 0 (`0x0001`) = USABLE_WITH_LIGHT_BACKGROUND
    /// Bit 1 (`0x0002`) = USABLE_WITH_DARK_BACKGROUND
    pub fn cpal_palette_type(&self, palette_index: u16) -> u32 {
        self.cpal
            .as_ref()
            .map(|c| c.palette_type(palette_index))
            .unwrap_or(0)
    }

    // ---- sbix bitmap glyphs (Apple Color Emoji format) -------------------

    /// `true` if this font ships an `sbix` table — Apple's PNG/JPEG/
    /// TIFF bitmap-strike container, used by Apple Color Emoji and
    /// every macOS/iOS-native colour-emoji font. Returns `false` for
    /// outline-only fonts and for CBDT/CBLC- or COLR/CPAL-flavoured
    /// colour fonts.
    pub fn has_sbix(&self) -> bool {
        self.sbix.is_some()
    }

    /// All strike ppem sizes the `sbix` table ships, sorted ascending
    /// and de-duplicated. Apple Color Emoji typically lists eight
    /// strikes in the 20-160 ppem range. Returns an empty `Vec` when
    /// the font has no `sbix` table.
    pub fn sbix_strikes(&self) -> Vec<u16> {
        self.sbix
            .as_ref()
            .map(|s| s.all_ppems_unique_sorted())
            .unwrap_or_default()
    }

    /// Resolve `glyph_id`'s sbix bitmap from the strike whose `ppem`
    /// is closest to the requested `ppem` (ties favour the larger
    /// strike, per the spec recommendation). Returns `None` if the
    /// font has no `sbix` table OR no strike contains a bitmap for
    /// `glyph_id`.
    ///
    /// `SbixGlyph::graphic_type` is one of `*b"png "`, `*b"jpg "`,
    /// `*b"tiff"`, or `*b"dupe"` — the consumer crate is expected to
    /// route the payload to the right decoder. The special `'dupe'`
    /// value indicates a 2-byte big-endian glyph id whose bitmap
    /// should be substituted; chasing the indirection is the
    /// caller's responsibility (we leave it explicit so the caller
    /// can do its own cycle detection).
    pub fn sbix_glyph(&self, glyph_id: u16, ppem: u16) -> Option<SbixGlyph<'a>> {
        self.sbix.as_ref()?.lookup_best_fit(glyph_id, ppem)
    }
}
