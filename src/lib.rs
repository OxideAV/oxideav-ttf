//! Pure-Rust TrueType / OpenType font parser.
//!
//! Round-1 scope:
//! - sfnt + table directory walker (`parser`).
//! - Core OpenType tables: `head`, `hhea`, `maxp`, `cmap` (base formats
//!   0/4/6/12 + format 14 Unicode Variation Sequences as a sidecar),
//!   `name`, `OS/2`, `hmtx`, `loca`, `glyf` (simple + composite), `post`.
//! - Legacy `kern` table (format 0 subtable).
//! - `GSUB` LookupType 1 (single substitution: positional forms,
//!   small-caps, vertical alternates), LookupType 2 (multiple
//!   substitution — split one input glyph into N), LookupType 3
//!   (alternate substitution — `aalt` / `salt` per-coverage
//!   alternates), LookupType 4 (ligature substitution — both walker
//!   and lookup-index-specific entry points), LookupType 5
//!   (contextual substitution — formats 1 / 2 / 3), LookupType 6
//!   (chained contexts substitution — formats 1 / 2 / 3, with
//!   recursive sub-lookup dispatch), and LookupType 8 (reverse
//!   chained context single substitution), discoverable via the
//!   ScriptList / FeatureList / LookupList common-table walk.
//! - `GPOS` LookupType 2 (pair-adjustment / kerning),
//!   LookupType 4 (mark-to-base attachment for diacritics), and
//!   LookupType 6 (mark-to-mark attachment for stacked diacritics).
//! - `GDEF` (glyph class definitions).
//! - Adobe Glyph List (AGL) glyph-name → Unicode resolution:
//!   [`glyph_name_to_codepoints`] / [`glyph_name_to_char`] (direct
//!   table lookup against the staged AGL data).
//!
//! The crate is read-only (parsing-only) and dependency-light: only
//! `oxideav-core` for shared types. CFF/Type 2 charstrings live in the
//! sibling `oxideav-otf` crate. TrueType hinting, bidi, and complex
//! shaping are deferred to later rounds.
//!
//! Variable fonts (`fvar`/`avar`/`gvar`) are supported as of round
//! 4: see [`Font::variation_axes`], [`Font::named_instances`],
//! [`Font::set_variation_coords`], and [`Font::glyph_outline`] (which
//! applies gvar deltas via the current axis-coord vector when set).
//!
//! See `README.md` for the public API tour.

#![deny(missing_debug_implementations)]
#![warn(rust_2018_idioms)]

pub mod agl;
pub mod collection;
pub mod outline;
pub mod parser;
pub mod tables;

pub use agl::{glyph_name_to_char, glyph_name_to_codepoints};
pub use collection::{is_collection, CollectionHeader, TTC_MAGIC};

use crate::parser::TableDirectory;
use crate::tables::{
    avar::AvarTable, cbdt::CbdtTable, cblc::CblcTable, cmap::CmapTable, colr::ColrTable,
    cpal::CpalTable, fvar::FvarTable, gdef::GdefTable, glyf::GlyfTable, gpos::GposTable,
    gsub::GsubTable, gvar::GvarTable, head::HeadTable, hhea::HheaTable, hmtx::HmtxTable,
    kern::KernTable, loca::LocaTable, maxp::MaxpTable, mvar::MvarTable, name::NameTable,
    os2::Os2Table, post::PostTable, sbix::SbixTable,
};

pub use outline::{BBox, Contour, Point, TtOutline};
pub use tables::cbdt::ColorBitmap;
pub use tables::cblc::{BigGlyphMetrics, SmallGlyphMetrics};
pub use tables::colr::ColorLayer;
pub use tables::fvar::{NamedInstance, VariationAxis};
pub use tables::gpos::{CursiveAttachment, PosRecord, PosValue};
pub use tables::gsub::GsubFeature;
pub use tables::kern::HeaderVariant as KernHeaderVariant;
pub use tables::mvar::ItemVariationStore;
pub use tables::name::{name_id, platform, NameRecord};
pub use tables::sbix::{SbixGlyph, MAX_DUPE_DEPTH as SBIX_MAX_DUPE_DEPTH};

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
    /// Variable-font axes header (`fvar`). Absent for static fonts.
    fvar: Option<FvarTable>,
    /// Per-axis non-linear remap (`avar`). Absent unless the font
    /// publishes one (most variable fonts do, identity for axes that
    /// don't need bending).
    avar: Option<AvarTable>,
    /// Per-glyph TupleVariationStore (`gvar`). Required when `fvar`
    /// is present and the outline kind is TrueType; not populated for
    /// CFF2 (which uses `cvar` instead — out of scope here).
    gvar: Option<GvarTable<'a>>,
    /// Font-wide metrics-variation table (`MVAR`). Present in many
    /// variable fonts; carries per-instance adjustments for `OS/2`,
    /// `hhea`, `vhea`, `post`, `gasp` metric fields keyed by the
    /// §7.3.6.3 value-tag registry.
    mvar: Option<MvarTable>,
    /// Current user-space coordinate vector, one per axis (defaults
    /// to each axis's `default` value when `fvar` is present, empty
    /// vec otherwise). `set_variation_coords` updates this; the
    /// outline accessor consults [`Self::normalised_coords`] to
    /// derive the per-axis weight applied to gvar deltas.
    var_coords: Vec<f32>,
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

        // Variable-font tables. `fvar` is the gate: if it's absent the
        // font is static and we skip the rest. If it's present we still
        // try to load `gvar` (TrueType deltas) and `avar` (axis remap)
        // but a missing `gvar` is acceptable for non-outline (CBDT-only)
        // variable fonts.
        let fvar = dir.find(b"fvar", bytes).map(FvarTable::parse).transpose()?;
        let avar = dir.find(b"avar", bytes).map(AvarTable::parse).transpose()?;
        let gvar = dir.find(b"gvar", bytes).map(GvarTable::parse).transpose()?;
        let mvar = dir.find(b"MVAR", bytes).map(MvarTable::parse).transpose()?;
        let var_coords = match fvar.as_ref() {
            Some(f) => f.axes().iter().map(|a| a.default).collect(),
            None => Vec::new(),
        };

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
            fvar,
            avar,
            gvar,
            mvar,
            var_coords,
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

    /// Subfamily (style) name from the `name` table — e.g. "Bold",
    /// "Italic", "Regular". `nameID` 2 (Adobe TN5149 §1.4).
    pub fn subfamily_name(&self) -> Option<&str> {
        self.name.find(name_id::SUBFAMILY)
    }

    /// Typographic (preferred) family name — `nameID` 16 — falling back to
    /// the standard family name (`nameID` 1) when the font omits it.
    /// Adobe TN5149 §1.4: when `nameID` 16 equals `nameID` 1 it may be
    /// omitted, so the fallback reconstructs the intended value.
    pub fn typographic_family_name(&self) -> Option<&str> {
        self.name
            .find(name_id::TYPOGRAPHIC_FAMILY)
            .or_else(|| self.name.find(name_id::FAMILY))
    }

    /// Typographic (preferred) subfamily name — `nameID` 17 — falling back
    /// to the standard subfamily name (`nameID` 2). Same omission rule as
    /// [`Self::typographic_family_name`] (TN5149 §1.4).
    pub fn typographic_subfamily_name(&self) -> Option<&str> {
        self.name
            .find(name_id::TYPOGRAPHIC_SUBFAMILY)
            .or_else(|| self.name.find(name_id::SUBFAMILY))
    }

    /// PostScript name — `nameID` 6 (TN5149 §1.5). The unique name a
    /// PostScript interpreter uses to select the font.
    pub fn postscript_name(&self) -> Option<&str> {
        self.name.find(name_id::POSTSCRIPT)
    }

    /// Version string — `nameID` 5 (TN5149 §1.9), e.g. "Version 1.000".
    pub fn version_string(&self) -> Option<&str> {
        self.name.find(name_id::VERSION)
    }

    /// Copyright notice — `nameID` 0 (TN5149 §1.3).
    pub fn copyright(&self) -> Option<&str> {
        self.name.find(name_id::COPYRIGHT)
    }

    /// Trademark — `nameID` 7 (TN5149 §1.10).
    pub fn trademark(&self) -> Option<&str> {
        self.name.find(name_id::TRADEMARK)
    }

    /// Manufacturer name — `nameID` 8 (TN5149 §1.10).
    pub fn manufacturer(&self) -> Option<&str> {
        self.name.find(name_id::MANUFACTURER)
    }

    /// Designer name — `nameID` 9 (TN5149 §1.10).
    pub fn designer(&self) -> Option<&str> {
        self.name.find(name_id::DESIGNER)
    }

    /// Description — `nameID` 10 (TN5149 §1.10).
    pub fn description(&self) -> Option<&str> {
        self.name.find(name_id::DESCRIPTION)
    }

    /// Font vendor URL — `nameID` 11 (TN5149 §1.10).
    pub fn vendor_url(&self) -> Option<&str> {
        self.name.find(name_id::VENDOR_URL)
    }

    /// Font designer URL — `nameID` 12 (TN5149 §1.10).
    pub fn designer_url(&self) -> Option<&str> {
        self.name.find(name_id::DESIGNER_URL)
    }

    /// Licence description — `nameID` 13 (TN5149 §1.10).
    pub fn license_description(&self) -> Option<&str> {
        self.name.find(name_id::LICENSE)
    }

    /// Licence URL — `nameID` 14 (TN5149 §1.10).
    pub fn license_url(&self) -> Option<&str> {
        self.name.find(name_id::LICENSE_URL)
    }

    /// Arbitrary `name`-table string by `nameID`, picking the best-ranked
    /// locale (Windows English first). The well-known IDs are exported as
    /// [`name_id`] constants. Use [`Self::name_string_for`] to target a
    /// specific platform + language.
    pub fn name_string(&self, name_id: u16) -> Option<&str> {
        self.name.find(name_id)
    }

    /// A specific `(nameID, platformID, languageID)` string — no ranking,
    /// the exact locale you name (e.g. `(name_id::FAMILY,
    /// platform::WINDOWS, 0x0411)` for the Japanese family name). Returns
    /// an owned `String` because non-ASCII records are decoded into a new
    /// buffer. `None` when no record matches or its encoding is one we
    /// cannot decode without an unstaged legacy codepage table (Macintosh
    /// non-Roman scripts — TN5149 §1.2).
    pub fn name_string_for(
        &self,
        name_id: u16,
        platform_id: u16,
        language_id: u16,
    ) -> Option<String> {
        self.name.find_for(name_id, platform_id, language_id)
    }

    /// Every `name`-table record, decoded where possible (see
    /// [`NameRecord`]). The locator tuple `(platformID, encodingID,
    /// languageID, nameID)` is always present; `string` is `None` for
    /// encodings we cannot decode in-crate.
    pub fn name_records(&self) -> Vec<NameRecord> {
        self.name.records()
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
    ///
    /// **Variable fonts:** if the font ships `fvar`/`gvar` and the
    /// caller has set non-default coordinates via
    /// [`Font::set_variation_coords`], the static outline returned
    /// here has gvar deltas applied (with avar remap on the input
    /// coords first). Composite glyphs do not currently propagate
    /// per-component variation deltas — only simple glyphs are
    /// retargeted; this is sufficient for nearly all Latin/Cyrillic/
    /// Greek glyphs, which are simple, and degrades gracefully on the
    /// composite-heavy CJK case (the static outline is still returned).
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
        let mut out = glyf.glyph_outline(range, loca, 0)?;
        if let Some(gvar) = self.gvar.as_ref() {
            if !self.var_coords.is_empty() && self.coords_differ_from_default() {
                let n_pts: usize = out.contours.iter().map(|c| c.points.len()).sum();
                if n_pts > 0 && n_pts <= u16::MAX as usize {
                    let normalised = self.normalised_coords();
                    if let Ok(deltas) = gvar.glyph_deltas(glyph_id, n_pts as u16, &normalised) {
                        let mut idx = 0usize;
                        for c in out.contours.iter_mut() {
                            for p in c.points.iter_mut() {
                                let (dx, dy) = deltas[idx];
                                let nx = p.x as i32 + dx;
                                let ny = p.y as i32 + dy;
                                p.x = clamp_i16_for_outline(nx);
                                p.y = clamp_i16_for_outline(ny);
                                idx += 1;
                            }
                        }
                        // Re-derive bounds after delta application.
                        out.bounds = outline::derive_bbox(&out.contours);
                    }
                }
            }
        }
        Ok(out)
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

    /// Resolve every GSUB feature active for `script_tag` under
    /// `lang_tag` to a list of `GsubFeature { tag, lookup_indices }`.
    ///
    /// `lang_tag = None` selects the script's `DefaultLangSys`. If
    /// `lang_tag` is supplied but isn't enumerated for the script, the
    /// lookup falls back to `DefaultLangSys` (matching the spec's
    /// "language system not present in script → use default" rule).
    ///
    /// The resulting `Vec` is empty when the font has no GSUB table or
    /// the script tag isn't in the ScriptList. Order matches the
    /// LangSys's `featureIndices` field, so a shaper can apply features
    /// in declaration order. The required feature (when present) is
    /// emitted first.
    ///
    /// Used by the consumer crate's Arabic shaper to discover which
    /// lookup indices implement `init` / `medi` / `fina` / `isol` for
    /// the current script — modern Arabic fonts (Noto Sans Arabic UI,
    /// most Indic fonts) ship positional forms via GSUB rather than
    /// the legacy Presentation Forms-B Unicode block.
    pub fn gsub_features_for_script(
        &self,
        script_tag: [u8; 4],
        lang_tag: Option<[u8; 4]>,
    ) -> Vec<GsubFeature> {
        match self.gsub.as_ref() {
            Some(g) => g.features_for_script(script_tag, lang_tag),
            None => Vec::new(),
        }
    }

    /// Apply GSUB LookupType 1 (Single Substitution) lookup
    /// `lookup_index` to a single input glyph `gid`.
    ///
    /// Returns `Some(replacement_gid)` when the lookup's coverage
    /// covers `gid`, or `None` when no substitution applies (caller
    /// keeps the input glyph unchanged). `None` is also returned when
    /// the font has no GSUB, the lookup index is out of range, or the
    /// referenced lookup isn't a single-substitution lookup (e.g. a
    /// ligature lookup is silently skipped here — call
    /// [`Self::lookup_ligature`] for those).
    ///
    /// Format 1 (delta) and Format 2 (substitute-array) sub-tables are
    /// both supported; ExtensionSubst (LookupType 7) wrappers are
    /// unwrapped transparently.
    pub fn gsub_apply_lookup_type_1(&self, lookup_index: u16, gid: u16) -> Option<u16> {
        self.gsub.as_ref()?.apply_lookup_type_1(lookup_index, gid)
    }

    /// Apply GSUB LookupType 4 (Ligature Substitution) lookup
    /// `lookup_index` to a prefix of `gids`.
    ///
    /// Returns `Some((replacement_gid, consumed))` when a sub-table in
    /// the named lookup matches a prefix of `gids` of length `consumed`
    /// (typically `>= 2` for real ligatures). Returns `None` when no
    /// rule applies, the lookup index is out of range, the referenced
    /// lookup is not a ligature lookup, or the font has no GSUB table.
    /// ExtensionSubst (LookupType 7) wrappers are unwrapped
    /// transparently.
    ///
    /// This is the lookup-index-specific counterpart of
    /// [`Self::lookup_ligature`] (which walks every lookup) and is the
    /// API a feature-driven shaper uses after resolving the `liga` /
    /// `rlig` / `dlig` feature for the active script via
    /// [`Self::gsub_features_for_script`].
    pub fn gsub_apply_lookup_type_4(
        &self,
        lookup_index: u16,
        gids: &[u16],
    ) -> Option<(u16, usize)> {
        self.gsub.as_ref()?.apply_lookup_type_4(lookup_index, gids)
    }

    /// Apply GSUB LookupType 6 (Chained Contexts Substitution) lookup
    /// `lookup_index` to the glyph run starting at `pos`.
    ///
    /// Returns `Some(rewritten_run)` — a fresh `Vec<u16>` of the full
    /// run with any sub-lookups dispatched at the matched
    /// `(backtrack, input, lookahead)` window — when one of the
    /// lookup's sub-tables (Format 1 / 2 / 3) matches around `pos`.
    /// Returns `None` when no chained-context rule applies, the lookup
    /// index is out of range, the referenced lookup is not a
    /// chain-context lookup, or the font has no GSUB table.
    ///
    /// Each `SubstLookupRecord { sequenceIndex, lookupListIndex }`
    /// inside the matched rule is recursively dispatched: LookupType 1
    /// substitutes the single glyph at the relative `sequenceIndex`,
    /// LookupType 4 substitutes `componentCount` glyphs starting there.
    /// Nested LookupType 6 references are also handled (bounded depth).
    /// ExtensionSubst (LookupType 7) is unwrapped transparently.
    ///
    /// This is the biggest GSUB unlock for complex scripts: Arabic
    /// shaping cascades, Indic reordering, and most ligature-with-
    /// context rules (e.g. Latin `ct` only between word boundaries)
    /// all run through chained-context lookups.
    pub fn gsub_apply_lookup_type_6(
        &self,
        lookup_index: u16,
        gids: &[u16],
        pos: usize,
    ) -> Option<Vec<u16>> {
        self.gsub
            .as_ref()?
            .apply_lookup_type_6(lookup_index, gids, pos)
    }

    /// Apply GSUB LookupType 2 (Multiple Substitution) lookup
    /// `lookup_index` to a single input glyph `gid`.
    ///
    /// Returns `Some(substitute_sequence)` — a `Vec<u16>` of the
    /// expanded glyph sequence — when the lookup's coverage covers
    /// `gid`. Returns `None` when no rule applies, the lookup index is
    /// out of range, the referenced lookup is not a multiple
    /// substitution, or the font has no GSUB table. ExtensionSubst
    /// (LookupType 7) wrappers are unwrapped transparently. The spec
    /// permits `glyphCount = 0` (deletion); such hits surface as
    /// `Some(Vec::new())`.
    pub fn gsub_apply_lookup_type_2(&self, lookup_index: u16, gid: u16) -> Option<Vec<u16>> {
        self.gsub.as_ref()?.apply_lookup_type_2(lookup_index, gid)
    }

    /// Apply GSUB LookupType 3 (Alternate Substitution) lookup
    /// `lookup_index` to `gid`, picking `alternate_index` from the
    /// resolved `AlternateSet`.
    ///
    /// Returns `Some(replacement_gid)` when the lookup covers `gid`
    /// AND `alternate_index` is in range for that coverage's
    /// `AlternateSet`. Returns `None` on coverage miss, out-of-range
    /// alternate index, non-alternate-substitution referenced lookup,
    /// or a font without GSUB. Default callers should pass
    /// `alternate_index = 0` — the spec doesn't register a
    /// per-feature variant index. ExtensionSubst (LookupType 7) is
    /// unwrapped transparently.
    pub fn gsub_apply_lookup_type_3(
        &self,
        lookup_index: u16,
        gid: u16,
        alternate_index: u16,
    ) -> Option<u16> {
        self.gsub
            .as_ref()?
            .apply_lookup_type_3(lookup_index, gid, alternate_index)
    }

    /// Apply GSUB LookupType 5 (Contextual Substitution) lookup
    /// `lookup_index` to the glyph run starting at `pos`.
    ///
    /// LookupType 5 mirrors LookupType 6 minus backtrack and
    /// lookahead — the input window is the only context. Returns
    /// `Some(rewritten_run)` — a fresh `Vec<u16>` with any sub-lookups
    /// dispatched at the matched input window — when one of the
    /// lookup's sub-tables (Format 1 / 2 / 3) matches around `pos`.
    /// Returns `None` when no contextual rule applies, the lookup
    /// index is out of range, the referenced lookup is not a
    /// contextual lookup, or the font has no GSUB.
    /// ExtensionSubst (LookupType 7) is unwrapped transparently.
    /// Recursive sub-lookup expansion is bounded.
    pub fn gsub_apply_lookup_type_5(
        &self,
        lookup_index: u16,
        gids: &[u16],
        pos: usize,
    ) -> Option<Vec<u16>> {
        self.gsub
            .as_ref()?
            .apply_lookup_type_5(lookup_index, gids, pos)
    }

    /// Apply GSUB LookupType 8 (Reverse Chained Context Substitution)
    /// lookup `lookup_index` to the glyph at `gids[pos]`.
    ///
    /// Returns `Some(replacement_gid)` when the input coverage covers
    /// `gids[pos]` AND every backtrack / lookahead coverage matches
    /// the surrounding glyphs. Returns `None` otherwise (no rule, out
    /// of range, wrong lookup type, no GSUB). ExtensionSubst
    /// (LookupType 7) is unwrapped transparently.
    ///
    /// The spec mandates reverse-text processing of the input run
    /// (essential for Arabic isolated forms in some fonts) — a higher-
    /// level shaper is what walks `pos` from right to left; this
    /// per-position entry point answers "does the rule fire here?".
    pub fn gsub_apply_lookup_type_8(
        &self,
        lookup_index: u16,
        gids: &[u16],
        pos: usize,
    ) -> Option<u16> {
        self.gsub
            .as_ref()?
            .apply_lookup_type_8(lookup_index, gids, pos)
    }

    /// On-disk header variant of the legacy `kern` table, if present.
    ///
    /// Two header layouts coexist: Microsoft-format `kern` (every
    /// Windows-authored / most Adobe / Google TTF — `u16 version,
    /// u16 nTables`) and Apple-format `kern` (macOS-bundled TTFs —
    /// `u32 version = 0x00010000, u32 nTables`, with different
    /// per-subtable header bytes). This crate decodes Microsoft-format
    /// Format-0 horizontal kerning subtables; Apple-format tables
    /// parse cleanly but their subtable bodies surface as zero pairs
    /// (see [`KernHeaderVariant::Apple`]).
    ///
    /// Returns `None` for fonts that don't ship a `kern` table at all
    /// (modern OpenType fonts use GPOS LookupType 2 instead).
    pub fn kern_header_variant(&self) -> Option<KernHeaderVariant> {
        self.kern.as_ref().map(|k| k.header_variant())
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

    /// Apply GPOS LookupType 1 (Single Adjustment Positioning) to
    /// `gid` via the lookup at `lookup_index`.
    ///
    /// Returns `Some(PosValue)` with the four geometric adjustments
    /// (`xPlacement`, `yPlacement`, `xAdvance`, `yAdvance`) when the
    /// lookup's coverage covers `gid`, or `None` when no rule applies
    /// (or the font has no GPOS). Both SinglePosFormat 1 (one shared
    /// ValueRecord) and Format 2 (per-glyph ValueRecord) are
    /// supported; ExtensionPos (LookupType 9) wrappers are unwrapped
    /// transparently.
    ///
    /// Use this for features that don't need pair context — e.g. the
    /// `cpsp` (capital spacing) feature applies a SinglePos to every
    /// uppercase glyph to add side bearing.
    pub fn gpos_apply_lookup_type_1(&self, lookup_index: u16, gid: u16) -> Option<PosValue> {
        self.gpos.as_ref()?.apply_lookup_type_1(lookup_index, gid)
    }

    /// Apply GPOS LookupType 3 (Cursive Attachment) to `gid` via the
    /// lookup at `lookup_index`.
    ///
    /// Returns `Some(CursiveAttachment { entry, exit })` when the
    /// lookup's coverage covers `gid`. Either anchor may be `None`
    /// (the spec allows one-sided cursive glyphs at cluster
    /// boundaries). Returns `None` when no rule applies, the lookup
    /// index is out of range, the referenced lookup is not a cursive
    /// lookup, or the font has no GPOS. ExtensionPos (LookupType 9)
    /// wrappers are unwrapped transparently.
    ///
    /// Cursive attachment chains glyph N+1 onto glyph N: the shaper
    /// translates glyph N+1's pen origin so its `entry` anchor lands
    /// on glyph N's `exit` anchor — i.e. the per-glyph delta is
    /// `prev.exit - this.entry` in (x, y) font units.
    pub fn gpos_apply_lookup_type_3(
        &self,
        lookup_index: u16,
        gid: u16,
    ) -> Option<CursiveAttachment> {
        self.gpos.as_ref()?.apply_lookup_type_3(lookup_index, gid)
    }

    /// Walk every GPOS LookupType-3 (Cursive Attachment) lookup
    /// looking for `gid`'s entry/exit anchor pair. Convenience wrapper
    /// around [`Self::gpos_apply_lookup_type_3`] for fonts that ship a
    /// single `curs` lookup (the common Arabic Nastaliq case). Returns
    /// the first hit in lookup order.
    pub fn lookup_cursive_attachment(&self, gid: u16) -> Option<CursiveAttachment> {
        self.gpos.as_ref()?.lookup_cursive_attachment(gid)
    }

    /// Apply GPOS LookupType 5 (Mark-to-Ligature Attachment) to the
    /// `(ligature, ligature_component, mark)` triple via the lookup
    /// at `lookup_index`.
    ///
    /// Returns `Some((dx, dy))` (font units, TT Y-up) — the offset to
    /// add to the mark's pen origin so its class anchor lands on the
    /// selected component's anchor. `ligature_component` is 0-indexed
    /// (component 0 = first component, e.g. `f` in `fi`). Returns
    /// `None` when no rule covers both glyphs, when the component
    /// index is out of range, or when no anchor exists for the mark's
    /// class on the requested component. ExtensionPos (LookupType 9)
    /// wrappers are unwrapped transparently.
    ///
    /// Closes the "fi + dot-above" gap: a mark following the second
    /// codepoint of a 2-component ligature attaches to component 1.
    pub fn gpos_apply_lookup_type_5(
        &self,
        lookup_index: u16,
        ligature: u16,
        ligature_component: u16,
        mark: u16,
    ) -> Option<(i16, i16)> {
        self.gpos
            .as_ref()?
            .apply_lookup_type_5(lookup_index, ligature, ligature_component, mark)
    }

    /// Walk every GPOS LookupType-5 (Mark-to-Ligature) lookup looking
    /// for the `(ligature, ligature_component, mark)` triple.
    /// Convenience wrapper around [`Self::gpos_apply_lookup_type_5`]
    /// that scans the LookupList rather than a specific index.
    pub fn lookup_mark_to_ligature(
        &self,
        ligature: u16,
        ligature_component: u16,
        mark: u16,
    ) -> Option<(i16, i16)> {
        self.gpos
            .as_ref()?
            .lookup_mark_to_ligature(ligature, ligature_component, mark)
    }

    /// Apply GPOS LookupType 8 (Chained Contexts Positioning) to the
    /// glyph run starting at `pos` via the lookup at `lookup_index`.
    ///
    /// Returns `Some(records)` — a `Vec<PosRecord>` listing every
    /// per-glyph adjustment the matched chain rule emits — when one
    /// of the lookup's sub-tables matches the
    /// `(backtrack, input, lookahead)` window around `pos`. Each
    /// `PosRecord.glyph_index` is an absolute offset into `gids`.
    ///
    /// All three sub-table formats (1 glyph-sequence, 2 class-based,
    /// 3 coverage-based) are supported. ExtensionPos (LookupType 9)
    /// wrappers are unwrapped transparently. Nested
    /// `PosLookupRecord` references into LookupType 1 / 2 / 4 / 6 / 8
    /// dispatch through the same machinery; recursion is bounded.
    pub fn gpos_apply_lookup_type_8(
        &self,
        lookup_index: u16,
        gids: &[u16],
        pos: usize,
    ) -> Option<Vec<PosRecord>> {
        self.gpos
            .as_ref()?
            .apply_lookup_type_8(lookup_index, gids, pos)
    }

    /// Enumerate every GPOS lookup as `(lookup_index, lookup_type,
    /// subtable_count)`.
    ///
    /// The reported `lookup_type` is the **effective** type after
    /// unwrapping any LookupType-9 ExtensionPos wrapper. Returns an
    /// empty iterator when the font has no GPOS table.
    ///
    /// Use this to find every chained-context positioning lookup, or
    /// every mark-to-ligature lookup, etc., without probing each
    /// index in turn — for example,
    /// `font.gpos_lookup_list().filter(|(_, t, _)| *t == 8)` enumerates
    /// the chained-context-positioning lookups.
    pub fn gpos_lookup_list(&self) -> Vec<(u16, u16, u16)> {
        match self.gpos.as_ref() {
            Some(g) => g.lookup_list().collect(),
            None => Vec::new(),
        }
    }

    /// Enumerate every GSUB lookup as `(lookup_index, lookup_type,
    /// subtable_count)`. Same shape as [`Self::gpos_lookup_list`] —
    /// the reported `lookup_type` is post-unwrap of any
    /// LookupType-7 ExtensionSubst wrapper.
    pub fn gsub_lookup_list(&self) -> Vec<(u16, u16, u16)> {
        match self.gsub.as_ref() {
            Some(g) => g.lookup_list().collect(),
            None => Vec::new(),
        }
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
    /// should be substituted; this method surfaces the indirection
    /// sentinel as-is for byte-level introspection. Use
    /// [`Self::sbix_glyph_resolved`] when the caller wants the
    /// indirection chased for them.
    pub fn sbix_glyph(&self, glyph_id: u16, ppem: u16) -> Option<SbixGlyph<'a>> {
        self.sbix.as_ref()?.lookup_best_fit(glyph_id, ppem)
    }

    /// Like [`Self::sbix_glyph`], but chases `'dupe'` indirections
    /// within the chosen strike — up to [`SBIX_MAX_DUPE_DEPTH`] hops
    /// — with explicit cycle detection. Returns the first reachable
    /// non-`'dupe'` entry, or `None` if the chain cycles, exceeds the
    /// hop cap, or hits a malformed / out-of-range target. Callers
    /// that need to introspect the raw `'dupe'` sentinel keep using
    /// [`Self::sbix_glyph`].
    pub fn sbix_glyph_resolved(&self, glyph_id: u16, ppem: u16) -> Option<SbixGlyph<'a>> {
        self.sbix.as_ref()?.lookup_best_fit_resolved(glyph_id, ppem)
    }

    // ---- variable fonts (fvar / avar / gvar) -----------------------------

    /// `true` if the font ships an `fvar` table — i.e. it exposes one
    /// or more variation axes. Returns `false` for static fonts.
    pub fn is_variable(&self) -> bool {
        self.fvar.is_some()
    }

    /// All variation axes the font publishes (`fvar`), in declaration
    /// order. Returns an empty slice for static fonts.
    pub fn variation_axes(&self) -> &[VariationAxis] {
        self.fvar.as_ref().map(|f| f.axes()).unwrap_or(&[])
    }

    /// All named instances the font ships (`fvar`), in declaration
    /// order. Each carries a coordinate vector matching
    /// [`Self::variation_axes`] (one f32 per axis) plus a `name`
    /// table id for the human-readable subfamily label.
    pub fn named_instances(&self) -> &[NamedInstance] {
        self.fvar.as_ref().map(|f| f.instances()).unwrap_or(&[])
    }

    /// Current user-space variation coordinates (one entry per axis,
    /// in `fvar` declaration order). Empty slice for static fonts.
    /// Defaults to each axis's `default` value at parse time;
    /// updated by [`Self::set_variation_coords`].
    pub fn variation_coords(&self) -> &[f32] {
        &self.var_coords
    }

    /// Replace the current variation coordinates. Each entry is in
    /// **user-space** units (e.g. `wght` is 100..900). The vector
    /// must be the same length as [`Self::variation_axes`]; shorter
    /// vectors leave the trailing axes at their previous value, longer
    /// vectors are truncated. Out-of-range values are clamped to each
    /// axis's `[min, max]`.
    ///
    /// No-op when the font is static (`is_variable() == false`).
    pub fn set_variation_coords(&mut self, coords: &[f32]) {
        let axes = match self.fvar.as_ref() {
            Some(f) => f.axes(),
            None => return,
        };
        for (i, &v) in coords.iter().enumerate() {
            if i >= self.var_coords.len() {
                break;
            }
            let a = &axes[i];
            self.var_coords[i] = v.clamp(a.min, a.max);
        }
    }

    /// Compute the normalised coordinate vector (each entry in
    /// `[-1, +1]`) by mapping each user-space value through the
    /// `fvar` axis triple, then through the `avar` per-axis remap.
    /// Returns an empty vec for static fonts.
    pub fn normalised_coords(&self) -> Vec<f32> {
        let axes = match self.fvar.as_ref() {
            Some(f) => f.axes(),
            None => return Vec::new(),
        };
        let mut out = Vec::with_capacity(axes.len());
        for (i, axis) in axes.iter().enumerate() {
            let v = self.var_coords.get(i).copied().unwrap_or(axis.default);
            let n = if (v - axis.default).abs() < f32::EPSILON {
                0.0
            } else if v < axis.default {
                if (axis.default - axis.min).abs() < f32::EPSILON {
                    0.0
                } else {
                    ((v - axis.default) / (axis.default - axis.min)).clamp(-1.0, 0.0)
                }
            } else if (axis.max - axis.default).abs() < f32::EPSILON {
                0.0
            } else {
                ((v - axis.default) / (axis.max - axis.default)).clamp(0.0, 1.0)
            };
            let n = match self.avar.as_ref() {
                Some(a) => a.remap_normalised(i, n),
                None => n,
            };
            out.push(n);
        }
        out
    }

    /// Borrow the parsed `MVAR` table, when present. Static fonts and
    /// variable fonts that omit MVAR return `None`.
    pub fn mvar_table(&self) -> Option<&MvarTable> {
        self.mvar.as_ref()
    }

    /// Interpolated `MVAR` adjustment for a four-byte metric tag (e.g.
    /// `*b"xhgt"`, `*b"cpht"`, `*b"hasc"`) at the current variation
    /// coordinates.
    ///
    /// Per ISO/IEC 14496-22:2019 §7.3.6.2, the adjustment is computed
    /// against the current **normalised** coordinate vector (i.e.
    /// after the `avar` remap, see [`Self::normalised_coords`]). The
    /// returned value is a delta to be **added** to the corresponding
    /// field in `OS/2` / `hhea` / `vhea` / `post` / `gasp`.
    ///
    /// Returns `None` when:
    /// * the font lacks an `MVAR` table, or
    /// * the requested `tag` is not present in MVAR's value-record
    ///   array (the spec's "if the tag does not occur, the item is
    ///   constant across the variation space" rule).
    ///
    /// Returns `Some(0.0)` when the variation evaluates to zero at the
    /// current instance (e.g. at the axis defaults).
    pub fn metric_variation_delta(&self, tag: &[u8; 4]) -> Option<f32> {
        let m = self.mvar.as_ref()?;
        let coords = self.normalised_coords();
        m.delta_for_tag(tag, &coords)
    }

    /// `true` if any current coordinate diverges from its axis default.
    fn coords_differ_from_default(&self) -> bool {
        let axes = match self.fvar.as_ref() {
            Some(f) => f.axes(),
            None => return false,
        };
        for (i, axis) in axes.iter().enumerate() {
            if let Some(v) = self.var_coords.get(i) {
                if (v - axis.default).abs() > f32::EPSILON {
                    return true;
                }
            }
        }
        false
    }
}

#[inline]
fn clamp_i16_for_outline(v: i32) -> i16 {
    if v < i16::MIN as i32 {
        i16::MIN
    } else if v > i16::MAX as i32 {
        i16::MAX
    } else {
        v as i16
    }
}
