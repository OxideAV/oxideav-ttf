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
//! - `GPOS` LookupType 1 (single adjustment), LookupType 2
//!   (pair-adjustment / kerning), LookupType 3 (cursive attachment),
//!   LookupType 4 (mark-to-base attachment for diacritics), LookupType 5
//!   (mark-to-ligature attachment), LookupType 6 (mark-to-mark
//!   attachment for stacked diacritics), LookupType 7 (contextual
//!   positioning — `SequenceContext` formats 1/2/3 with recursive
//!   nested-lookup dispatch), and LookupType 8 (chained contexts
//!   positioning).
//! - `GDEF` (glyph class definitions).
//! - Adobe Glyph List (AGL) glyph-name → Unicode resolution:
//!   [`glyph_name_to_codepoints`] / [`glyph_name_to_char`] (direct
//!   table lookup against the staged AGL data).
//! - `gasp` (grid-fitting and scan-conversion procedure table, ISO/IEC
//!   14496-22:2019 §5.3.7) — both version 0 and 1, per-record flag
//!   accessors, behaviour-for-ppem lookup.
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
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod parser;
pub mod shape;
pub mod tables;

pub use agl::{glyph_name_to_char, glyph_name_to_codepoints};
pub use collection::{is_collection, CollectionHeader, TTC_MAGIC};
pub use shape::ShapedGlyph;

use crate::parser::TableDirectory;
use crate::tables::{
    avar::AvarTable,
    base::BaseTable,
    cbdt::CbdtTable,
    cblc::CblcTable,
    cff::CffTable,
    cff2::Cff2Table,
    cmap::CmapTable,
    colr::ColrTable,
    cpal::CpalTable,
    cvar::CvarTable,
    dsig::DsigTable,
    ebdt::EbdtTable,
    ebsc::EbscTable,
    fvar::FvarTable,
    gasp::GaspTable,
    gdef::GdefTable,
    glyf::GlyfTable,
    gpos::GposTable,
    gsub::GsubTable,
    gvar::GvarTable,
    hdmx::HdmxTable,
    head::HeadTable,
    hhea::HheaTable,
    hmtx::HmtxTable,
    hvar::HvarTable,
    jstf::JstfTable,
    kern::KernTable,
    loca::LocaTable,
    ltsh::LtshTable,
    math::{GrowDirection, MathKernCorner, MathTable},
    maxp::MaxpTable,
    merg::MergTable,
    meta::MetaTable,
    mvar::MvarTable,
    name::NameTable,
    os2::Os2Table,
    pclt::PcltTable,
    post::PostTable,
    sbix::SbixTable,
    stat::StatTable,
    svg::SvgTable,
    vdmx::VdmxTable,
    vhea::VheaTable,
    vmtx::VmtxTable,
    vorg::VorgTable,
    vvar::VvarTable,
};

pub use outline::{BBox, Contour, Point, TtOutline};
// internal — exposed for tests/fuzz; not part of the stable API (the
// stable BASE surface is the `Font::base_*` accessor family)
#[doc(hidden)]
pub use tables::base::{
    AxisTable as BaseAxisTable, BaseCoord, BaseLangSysRecord, BaseScriptRecord, BaseScriptTable,
    BaseValuesTable, FeatMinMaxRecord, MinMaxTable as BaseMinMaxTable, BASE_MAJOR_VERSION,
    BASE_MINOR_VERSION_0, BASE_MINOR_VERSION_1,
};
pub use tables::cbdt::ColorBitmap;
pub use tables::cblc::{BigGlyphMetrics, SmallGlyphMetrics};
pub use tables::colr::{
    Affine2x3, ClipBox, ColorLayer, ColorLine, ColorStop, CompositeMode, Extend, Paint, PaintRef,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use tables::device::DeviceOrVariationIndex;
pub use tables::dsig::{Signature as DsigSignature, DSIG_BLOCK_FORMAT_PKCS7, DSIG_VERSION};
pub use tables::ebdt::{CompositeBitmap, EbdtComponent, GrayBitmap};
pub use tables::ebsc::{BitmapScale, SbitLineMetrics, EBSC_MAJOR_VERSION, EBSC_MINOR_VERSION};
pub use tables::fvar::{NamedInstance, VariationAxis};
pub use tables::gasp::{
    GaspRange, GASP_DOGRAY, GASP_GRIDFIT, GASP_PPEM_SENTINEL, GASP_RESERVED_MASK,
    GASP_SYMMETRIC_GRIDFIT, GASP_SYMMETRIC_SMOOTHING, GASP_TABLE_TAG, GASP_VERSION_0,
    GASP_VERSION_1,
};
pub use tables::gpos::{CursiveAttachment, GposFeature, PosRecord, PosValue};
pub use tables::gsub::GsubFeature;
pub use tables::hdmx::{HdmxRecord, HDMX_TABLE_TAG, HDMX_VERSION_0};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use tables::hdmx::{HDMX_HEADER_LEN, HDMX_RECORD_HEADER_LEN};
pub use tables::head::{
    HEAD_FLAG_BASELINE_AT_Y0, HEAD_FLAG_CLEARTYPE_OPTIMIZED, HEAD_FLAG_CONVERTED,
    HEAD_FLAG_INSTRUCTIONS_ALTER_ADVANCE, HEAD_FLAG_LAST_RESORT, HEAD_FLAG_LOSSLESS,
    MAC_STYLE_BOLD, MAC_STYLE_CONDENSED, MAC_STYLE_EXTENDED, MAC_STYLE_ITALIC,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use tables::hvar::DeltaSetIndexMap;
pub use tables::kern::HeaderVariant as KernHeaderVariant;
pub use tables::ltsh::{LTSH_ALWAYS_LINEAR, LTSH_TABLE_TAG, LTSH_VERSION_0};
pub use tables::merg::{
    MergeEntry, GROUP_LTR, GROUP_RTL, MERGE_LTR, MERGE_RTL, SECOND_IS_SUBORDINATE_LTR,
    SECOND_IS_SUBORDINATE_RTL,
};
pub use tables::meta::{
    is_valid_meta_tag, script_lang_tags, MetaRecord, ScriptLangTag, META_TABLE_TAG, META_TAG_APPL,
    META_TAG_BILD, META_TAG_DLNG, META_TAG_SLNG, META_VERSION_1,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use tables::meta::{META_DATA_MAP_LEN, META_HEADER_LEN};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use tables::mvar::ItemVariationStore;
pub use tables::name::{name_id, platform, NameRecord};
pub use tables::os2::{
    FSSELECTION_BOLD, FSSELECTION_ITALIC, FSSELECTION_OBLIQUE, FSSELECTION_REGULAR,
    FSSELECTION_USE_TYPO_METRICS, FSTYPE_BITMAP_ONLY, FSTYPE_EDITABLE, FSTYPE_NO_SUBSETTING,
    FSTYPE_PREVIEW_PRINT, FSTYPE_RESTRICTED_LICENSE,
};
pub use tables::pclt::{
    PCLT_MAJOR_VERSION, PCLT_STROKE_WEIGHT_RANGE, PCLT_TABLE_TAG, PCLT_WIDTH_TYPE_RANGE,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use tables::pclt::PCLT_TABLE_LEN;
pub use tables::post::{
    standard_mac_glyph_name, GlyphNameRef, PostFormat, PostV20, PostV25, POST_TABLE_TAG,
    POST_VERSION_10, POST_VERSION_20, POST_VERSION_25, POST_VERSION_30,
    RECOMMENDED_GLYPH_NAME_MAX_LEN, STANDARD_MAC_GLYPH_COUNT, STANDARD_MAC_GLYPH_NAMES,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use tables::post::POST_HEADER_LEN;
pub use tables::sbix::{SbixGlyph, MAX_DUPE_DEPTH as SBIX_MAX_DUPE_DEPTH};
pub use tables::stat::{
    AxisRecord as StatAxisRecord, AxisValue as StatAxisValue,
    FLAG_ELIDABLE_AXIS_VALUE_NAME as STAT_FLAG_ELIDABLE_AXIS_VALUE_NAME,
    FLAG_OLDER_SIBLING_FONT_ATTRIBUTE as STAT_FLAG_OLDER_SIBLING_FONT_ATTRIBUTE,
    RANGE_MAX_POS_INFINITY as STAT_RANGE_MAX_POS_INFINITY,
    RANGE_MIN_NEG_INFINITY as STAT_RANGE_MIN_NEG_INFINITY,
};
pub use tables::svg::{SvgDocument, SVG_GZIP_MAGIC, SVG_TABLE_TAG, SVG_VERSION_0};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use tables::svg::{SVG_DOCUMENT_RECORD_LEN, SVG_HEADER_LEN};
pub use tables::vdmx::{
    RatioRange as VdmxRatioRange, VdmxGroup, VdmxVTableRecord, VDMX_TABLE_TAG, VDMX_VERSION_0,
    VDMX_VERSION_1,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use tables::vdmx::{
    VDMX_GROUP_HEADER_LEN, VDMX_HEADER_LEN, VDMX_OFFSET_LEN, VDMX_RATIO_RECORD_LEN,
    VDMX_VTABLE_RECORD_LEN,
};
pub use tables::vhea::{VHEA_VERSION_1_0, VHEA_VERSION_1_1};
pub use tables::vorg::{VertOriginEntry, VORG_MAJOR_VERSION, VORG_MINOR_VERSION};

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
    /// Vertical header table (`vhea`, ISO/IEC 14496-22:2019 §5.7.9).
    /// Optional — only fonts intended for vertical layout ship one;
    /// in particular, CJK fonts and the rare Mongolian / Manchu font.
    /// When present, the companion `vmtx` table is also required per
    /// §5.7.10 ("OFFvertical fonts require both a vertical header
    /// table ('vhea') and the vertical metrics table").
    vhea: Option<VheaTable>,
    /// Vertical metrics table (`vmtx`, ISO/IEC 14496-22:2019 §5.7.10).
    /// Always paired with `vhea`; only present when the font supplies
    /// vertical layout data.
    vmtx: Option<VmtxTable<'a>>,
    /// Vertical origin table (`VORG`, ISO/IEC 14496-22:2019 §5.4.4).
    /// Optional table that records, per glyph, the Y coordinate of the
    /// glyph's vertical origin in font design units. Per §5.4.4 the
    /// table is restricted to CFF-flavoured sfnts ("If present in
    /// TrueType OFF fonts it must be ignored by font clients"); when a
    /// TrueType-flavoured sfnt nonetheless ships one we still parse it
    /// here so the bytes are available, but the
    /// [`Font::vert_origin_y_from_vorg`] accessor respects the
    /// ignore-on-TrueType policy and returns `None` once `glyf` is
    /// present.
    vorg: Option<VorgTable>,
    /// Glyph-location offsets into `glyf`. Optional because CBDT/CBLC-only
    /// colour-emoji fonts (e.g. NotoColorEmoji.ttf) ship without `loca`
    /// and `glyf` — every glyph is a colour bitmap and there are no
    /// outlines to address.
    loca: Option<LocaTable<'a>>,
    glyf: Option<GlyfTable<'a>>,
    /// `CFF ` outlines (PostScript / Type 2 charstrings). Present in
    /// OTTO-flavoured fonts; mutually exclusive with `glyf` in practice.
    cff: Option<CffTable<'a>>,
    /// `CFF2` outlines (variable PostScript charstrings). Present in
    /// CFF2-flavoured variable fonts; we render the default instance.
    cff2: Option<Cff2Table<'a>>,
    post: Option<PostTable>,
    kern: Option<KernTable<'a>>,
    gsub: Option<GsubTable<'a>>,
    gpos: Option<GposTable<'a>>,
    gdef: Option<GdefTable<'a>>,
    cblc: Option<CblcTable<'a>>,
    cbdt: Option<CbdtTable<'a>>,
    /// Embedded bitmap *location* table (`EBLC`, ISO/IEC 14496-22:2019
    /// §5.6.3). The monochrome / grayscale analog of `CBLC`; identical
    /// on-wire layout (the shared [`CblcTable`] walker accepts both),
    /// paired with [`Font::ebdt`](Self::ebdt) rather than `CBDT`.
    eblc: Option<CblcTable<'a>>,
    /// Embedded monochrome / grayscale bitmap data (`EBDT`, ISO/IEC
    /// 14496-22:2019 §5.6.2). Located through the shared `EBLC`/`CBLC`
    /// walker (the same `CblcTable` used for colour bitmaps); an `EBLC`
    /// (major == 2) strike resolves the same way a `CBLC` colour strike
    /// does. Present on legacy
    /// pixel / CJK bitmap faces.
    ebdt: Option<EbdtTable<'a>>,
    /// Embedded bitmap *scaling* table (`EBSC`, ISO/IEC 14496-22:2019
    /// §5.6.4). Declares synthesised strikes built by scaling an existing
    /// `EBLC`/`EBDT` strike up or down (small Kanji sizes are the spec's
    /// motivating case). Owns no glyph imagery; it redirects a requested
    /// ppem to a real `substitutePpem` strike. Carries no lifetime — every
    /// field copies out of the slice at parse time.
    ebsc: Option<EbscTable>,
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
    /// CFF2 (which interleaves its deltas inside the `CFF2` table).
    gvar: Option<GvarTable<'a>>,
    /// CVT-variations table (`cvar`, ISO/IEC 14496-22:2019 §7.3.2).
    /// Present in TrueType-hinted variable fonts; supplies per-instance
    /// deltas for the `cvt ` Control Value Table entries.
    cvar: Option<CvarTable<'a>>,
    /// Raw `cvt ` Control Value Table bytes (an array of big-endian
    /// `int16` FWORDs). Held so [`Font::cvt_value`] / [`Font::cvt_count`]
    /// can resolve entries, optionally with `cvar` deltas applied.
    cvt_bytes: Option<&'a [u8]>,
    /// Raw `fpgm` font-program bytes (TrueType bytecode, run once when the
    /// font is first used — ISO/IEC 14496-22:2019 §5.3.3). This crate does
    /// not execute the program; the bytes are surfaced through
    /// [`Font::fpgm_program`] for tooling that introspects or round-trips
    /// the hinting program.
    fpgm_bytes: Option<&'a [u8]>,
    /// Raw `prep` control-value-program bytes (TrueType bytecode, run
    /// whenever size / transform changes — ISO/IEC 14496-22:2019 §5.3.x).
    /// Surfaced raw through [`Font::prep_program`]; not executed.
    prep_bytes: Option<&'a [u8]>,
    /// Font-wide metrics-variation table (`MVAR`). Present in many
    /// variable fonts; carries per-instance adjustments for `OS/2`,
    /// `hhea`, `vhea`, `post`, `gasp` metric fields keyed by the
    /// §7.3.6.3 value-tag registry.
    mvar: Option<MvarTable>,
    /// Per-glyph horizontal-metrics variation table (`HVAR`,
    /// ISO/IEC 14496-22:2019 §7.3.5). Variable fonts with TrueType
    /// outlines are encouraged to ship one; CFF2 variable fonts are
    /// required to. Provides interpolated adjustments for `hmtx`
    /// advance widths plus optional left- and right-side bearings.
    hvar: Option<HvarTable>,
    /// Per-glyph vertical-metrics variation table (`VVAR`,
    /// ISO/IEC 14496-22:2019 §7.3.8). Optional in TrueType variable
    /// fonts (where `gvar` phantom points carry the same data); for
    /// CFF2 variable fonts that support vertical layout it is required
    /// (§7.3.8.1). Provides interpolated adjustments for `vmtx`
    /// advance heights plus optional top-/bottom-side bearings and —
    /// for CFF2 fonts that publish a `VORG` table — vertical-origin
    /// Y coordinates.
    vvar: Option<VvarTable>,
    /// Style attributes table (`STAT`, ISO/IEC 14496-22:2019 §7.3.7).
    /// Required in all variable fonts; optional otherwise. Carries
    /// design-axis records and per-axis-value name mappings used by
    /// font pickers to compose family / subfamily strings under the
    /// R/B/I/BI, WWS, and unrestricted naming models.
    stat: Option<StatTable>,
    /// Baseline table (`BASE`, ISO/IEC 14496-22:2019 §6.3.1). Optional
    /// table that supplies per-script baseline coordinates and
    /// per-script / per-language-system / per-feature minimum and
    /// maximum glyph extents. Carries one Axis sub-table per text
    /// direction (HorizAxis for Y baselines / horizontal text;
    /// VertAxis for X baselines / vertical text).
    base: Option<BaseTable>,
    /// Grid-fitting and scan-conversion procedure table (`gasp`,
    /// ISO/IEC 14496-22:2019 §5.3.7). Optional; carries the
    /// per-ppem-range rasterisation hints (grid-fit / grayscale /
    /// ClearType-symmetric flags) sorted by `rangeMaxPPEM`. Used by
    /// callers that drive a font rasteriser and want to pick the
    /// font-author-recommended hinting policy at a given pixel size.
    gasp: Option<GaspTable>,
    /// Linear threshold table (`LTSH`, ISO/IEC 14496-22:2019 §5.7.4).
    /// Optional; carries one byte per glyph recording the lowest ppem
    /// at which the grid-fitted advance width has converged on the
    /// rounded linear advance, so a rasteriser at or above that ppem
    /// can round the linear advance arithmetically without scan-
    /// converting the glyph. The §5.7.4 sentinel `1` means "always
    /// scales linearly" (the glyph carries no instructions on its
    /// sidebearings).
    ltsh: Option<LtshTable>,
    /// Horizontal device metrics table (`hdmx`, ISO/IEC 14496-22:2019
    /// §5.7.2). Optional; carries one device record per selected ppem,
    /// each holding the per-glyph grid-fitted advance width in integer
    /// pixels. The precomputed-advance counterpart to `LTSH`: instead
    /// of recording when the grid-fit advance converges to the linear
    /// advance, `hdmx` records the exact grid-fit advance for a fixed
    /// set of ppem sizes. §7.3.5 forbids `hdmx` in variable fonts;
    /// callers that want to honour that rule can cross-check
    /// `is_variable()` before consulting these accessors.
    hdmx: Option<HdmxTable>,
    /// Vertical device metrics table (`VDMX`, ISO/IEC 14496-22:2019
    /// §5.7.8). Optional; carries one or more groups of vTable
    /// records (`yPelHeight` → `(yMax, yMin)` pel envelope) indexed
    /// via a per-aspect-ratio RatioRange array. The precomputed-extent
    /// counterpart to `hdmx`'s per-glyph advance widths: instead of
    /// publishing each glyph's grid-fitted advance, `VDMX` publishes
    /// the font-wide vertical extent at a curated ppem set so a
    /// rasteriser can pick a render bitmap height without
    /// grid-fitting every glyph in the font. §7.3.5 forbids `VDMX`
    /// in variable fonts; callers can cross-check `is_variable()`
    /// before consulting these accessors.
    vdmx: Option<VdmxTable>,
    /// Metadata table (`meta`, ISO/IEC 14496-22:2019 §5.7.6). Optional;
    /// carries a tagged DataMap array whose payloads describe font-wide
    /// metadata in either UTF-8 text (`'dlng'`, `'slng'`) or vendor-
    /// defined binary form. Records borrow from the on-wire `meta`
    /// byte slice — the table itself does not copy the payload data.
    meta: Option<MetaTable<'a>>,
    /// PCL 5 table (`PCLT`, ISO/IEC 14496-22:2019 §5.7.7). Optional
    /// (and "strongly discouraged for OFF fonts with TrueType
    /// outlines" per the spec); carries the PCL 5 font-selection
    /// attributes — HP font number, pitch / x-height / cap-height,
    /// packed style / type-family / symbol-set words, the 16-byte
    /// typeface string, the 8-byte character-complement bitfield,
    /// the 6-byte PCL file name, and the stroke-weight / width-type
    /// / serif-style classification bytes.
    pclt: Option<PcltTable>,
    /// SVG table (`SVG `, ISO/IEC 14496-22:2019/Amd.1:2020 §5.5.1).
    /// Optional; carries per-glyph-range SVG 1.1 vector colour-glyph
    /// documents (plain UTF-8 or gzip-encoded). Records borrow from the
    /// on-wire `SVG ` byte slice — the table does not copy the markup.
    svg: Option<SvgTable<'a>>,
    /// Math typesetting table (`MATH`, ISO/IEC 14496-22:2019 §6.3.6).
    /// Present in fonts designed for mathematical layout; carries the
    /// MathConstants / MathGlyphInfo / MathVariants sub-tables that a
    /// math-layout engine consumes. Borrows from the on-wire slice.
    math: Option<MathTable<'a>>,
    /// Justification table (`JSTF`, ISO/IEC 14496-22:2019 §6.3.5).
    /// Optional; carries per-script/language justification suggestions
    /// (GSUB/GPOS lookup enable/disable lists + extender glyphs).
    jstf: Option<JstfTable<'a>>,
    /// Digital signature table (`DSIG`, ISO/IEC 14496-22:2019 §8.x).
    /// Optional; carries the font's digital signature as one or more
    /// `SignatureRecord`s pointing at PKCS#7 signature blocks. Block
    /// payloads borrow from the on-wire `DSIG` slice — this crate decodes
    /// the table structure but does not verify the signature.
    dsig: Option<DsigTable<'a>>,
    /// Merge table (`MERG`, ISO/IEC 14496-22:2019 §5.7.5). Optional;
    /// declares which glyph-class pairs a renderer should merge or group
    /// before antialias filtering. Copies its ClassDef + merge-entry bytes
    /// out at parse time, so it carries no lifetime.
    merg: Option<MergTable>,
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
        // `vhea` + `vmtx` are jointly optional: a font that lacks
        // either is treated as horizontal-only. §5.7.10 mandates that
        // a font shipping one ship both ("OFFvertical fonts require
        // both"), so a half-pair is rejected as a malformed file
        // rather than silently degraded.
        let vhea = dir.find(b"vhea", bytes).map(VheaTable::parse).transpose()?;
        let vmtx_slice = dir.find(b"vmtx", bytes);
        let vmtx = match (vhea.as_ref(), vmtx_slice) {
            (Some(vh), Some(slice)) => Some(VmtxTable::parse(
                slice,
                vh.num_long_ver_metrics,
                maxp.num_glyphs,
            )?),
            (None, None) => None,
            (Some(_), None) => {
                return Err(Error::BadStructure(
                    "vhea present but vmtx missing (§5.7.10 requires both)",
                ));
            }
            (None, Some(_)) => {
                return Err(Error::BadStructure(
                    "vmtx present but vhea missing (§5.7.10 requires both)",
                ));
            }
        };
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
        // `CFF ` carries PostScript outlines (OTTO fonts). The tag has a
        // trailing space.
        let cff = dir.find(b"CFF ", bytes).map(CffTable::parse).transpose()?;
        // `CFF2` carries variable PostScript outlines; we render the
        // default instance.
        let cff2 = dir.find(b"CFF2", bytes).map(Cff2Table::parse).transpose()?;

        let os2 = dir.find(b"OS/2", bytes).map(Os2Table::parse).transpose()?;
        let post = dir.find(b"post", bytes).map(PostTable::parse).transpose()?;
        let kern = dir.find(b"kern", bytes).map(KernTable::parse).transpose()?;
        let gsub = dir.find(b"GSUB", bytes).map(GsubTable::parse).transpose()?;
        let gpos = dir.find(b"GPOS", bytes).map(GposTable::parse).transpose()?;
        let gdef = dir.find(b"GDEF", bytes).map(GdefTable::parse).transpose()?;
        let cblc = dir.find(b"CBLC", bytes).map(CblcTable::parse).transpose()?;
        let cbdt = dir.find(b"CBDT", bytes).map(CbdtTable::parse).transpose()?;
        let eblc = dir.find(b"EBLC", bytes).map(CblcTable::parse).transpose()?;
        let ebdt = dir.find(b"EBDT", bytes).map(EbdtTable::parse).transpose()?;
        let ebsc = dir.find(b"EBSC", bytes).map(EbscTable::parse).transpose()?;
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
        let cvar = dir.find(b"cvar", bytes).map(CvarTable::parse).transpose()?;
        // `cvt ` is a plain `int16[]` Control Value Table; held raw.
        let cvt_bytes = dir.find(b"cvt ", bytes);
        // `fpgm` / `prep` are raw TrueType bytecode programs (§5.3.3 /
        // §5.3.x). Not executed by this crate — held raw for tooling.
        let fpgm_bytes = dir.find(b"fpgm", bytes);
        let prep_bytes = dir.find(b"prep", bytes);
        let mvar = dir.find(b"MVAR", bytes).map(MvarTable::parse).transpose()?;
        let hvar = dir.find(b"HVAR", bytes).map(HvarTable::parse).transpose()?;
        let vvar = dir.find(b"VVAR", bytes).map(VvarTable::parse).transpose()?;
        let stat = dir.find(b"STAT", bytes).map(StatTable::parse).transpose()?;
        let base = dir.find(b"BASE", bytes).map(BaseTable::parse).transpose()?;
        let gasp = dir.find(b"gasp", bytes).map(GaspTable::parse).transpose()?;
        let vorg = dir.find(b"VORG", bytes).map(VorgTable::parse).transpose()?;
        // §5.7.4 says `LTSH.numGlyphs` "should be the same as the
        // numGlyphs field in the 'maxp' table". A mismatch would either
        // truncate or over-read the per-glyph lookups, so cross-check
        // at parse time and reject as `BadStructure`.
        let ltsh = dir
            .find(b"LTSH", bytes)
            .map(|s| LtshTable::parse_with_glyph_count(s, maxp.num_glyphs))
            .transpose()?;
        // §5.7.2 fixes the per-record `widths[]` length at
        // `maxp.numGlyphs`. Cross-checking against `maxp.num_glyphs`
        // at parse time rejects under-sized records (`UnexpectedEof`)
        // and protects per-ppem lookups from over-reading the slice.
        let hdmx = dir
            .find(b"hdmx", bytes)
            .map(|s| HdmxTable::parse(s, maxp.num_glyphs))
            .transpose()?;
        // §5.7.8 describes a fixed-shape table: 6-byte header, then a
        // RatioRange + Offset16 pair of arrays followed by VDMX groups
        // referenced from those offsets. No per-glyph cross-check
        // against `maxp` is needed — the table publishes font-wide
        // extents indexed by ppem only, not per-glyph data. `parse`
        // enforces the §5.7.8 sort + sentinel invariants.
        let vdmx = dir.find(b"VDMX", bytes).map(VdmxTable::parse).transpose()?;
        // §5.7.6 metadata table — header + DataMap array indexed by
        // four-character ASCII tags. The data payloads sit later in
        // the same byte slice and `MetaRecord::payload` borrows from
        // there; the `'a` lifetime of `Font<'a>` therefore covers
        // every payload exposed through `meta_*` accessors.
        let meta = dir.find(b"meta", bytes).map(MetaTable::parse).transpose()?;
        // §5.7.7 PCL 5 table — fixed 54-byte struct of PCL font-
        // selection attributes. All fields copy out of the slice at
        // parse time so the parsed table carries no lifetime.
        let pclt = dir.find(b"PCLT", bytes).map(PcltTable::parse).transpose()?;
        // §5.5.1 (Amd.1:2020) SVG table — per-glyph-range SVG 1.1 vector
        // colour-glyph documents. The tag carries a trailing space
        // (`'SVG '`). Document payloads borrow from this byte slice so
        // the `'a` lifetime of `Font<'a>` covers every document exposed
        // through the `svg_*` accessors.
        let svg = dir
            .find(&SVG_TABLE_TAG, bytes)
            .map(SvgTable::parse)
            .transpose()?;
        // §6.3.6 MATH table — math-layout parameters. Borrows from the
        // on-wire slice.
        let math = dir.find(b"MATH", bytes).map(MathTable::parse).transpose()?;
        // §6.3.5 JSTF table — justification suggestions. Borrows from the
        // on-wire slice.
        let jstf = dir.find(b"JSTF", bytes).map(JstfTable::parse).transpose()?;
        // §8.x DSIG table — digital signature. Structural decode only; the
        // PKCS#7 block payloads borrow from this slice.
        let dsig = dir.find(b"DSIG", bytes).map(DsigTable::parse).transpose()?;
        // §5.7.5 MERG table — glyph-merge declarations for antialias
        // filtering. Copies its bytes out at parse time.
        let merg = dir.find(b"MERG", bytes).map(MergTable::parse).transpose()?;
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
            vhea,
            vmtx,
            vorg,
            loca,
            glyf,
            cff,
            cff2,
            post,
            kern,
            gsub,
            gpos,
            gdef,
            cblc,
            cbdt,
            eblc,
            ebdt,
            ebsc,
            colr,
            cpal,
            sbix,
            fvar,
            avar,
            gvar,
            cvar,
            cvt_bytes,
            fpgm_bytes,
            prep_bytes,
            mvar,
            hvar,
            vvar,
            stat,
            base,
            gasp,
            ltsh,
            hdmx,
            vdmx,
            meta,
            pclt,
            svg,
            math,
            jstf,
            dsig,
            merg,
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

    /// Borrow the parsed `head` table (ISO/IEC 14496-22:2019 §5.2.1),
    /// exposing `fontRevision`, the `flags` / `macStyle` words (with
    /// decoded predicates), the created / modified timestamps,
    /// `lowestRecPPEM`, `fontDirectionHint`, and `glyphDataFormat`.
    pub fn head_table(&self) -> &HeadTable {
        &self.head
    }

    /// `head.fontRevision` — the font designer's revision number as a
    /// 16.16 fixed value (e.g. `2.37`).
    pub fn font_revision(&self) -> f32 {
        self.head.font_revision
    }

    /// `head.lowestRecPPEM` — the smallest size, in pixels, at which the
    /// font is intended to remain legible.
    pub fn lowest_rec_ppem(&self) -> u16 {
        self.head.lowest_rec_ppem
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

    /// Borrow the parsed `hhea` table (ISO/IEC 14496-22:2019 §5.2.4),
    /// exposing the horizontal header in full: ascent / descent / line gap,
    /// `advanceWidthMax`, the min side-bearing extremes, `xMaxExtent`, the
    /// caret-slope rise / run / offset, and `numberOfHMetrics`.
    pub fn hhea_table(&self) -> &HheaTable {
        &self.hhea
    }

    /// Borrow the parsed `maxp` table (ISO/IEC 14496-22:2019 §5.2.5). For a
    /// v1.0 (TrueType) table the `v1` field carries the rasteriser-sizing
    /// maxima (`maxPoints`, composite limits, bytecode resource caps,
    /// `maxComponentDepth`); `v1` is `None` for a v0.5 (CFF) table.
    pub fn maxp_table(&self) -> &MaxpTable {
        &self.maxp
    }

    /// `OS/2.usWeightClass` (100..1000), or 400 (Regular) if `OS/2` absent.
    pub fn weight_class(&self) -> u16 {
        self.os2.as_ref().map(|o| o.us_weight_class).unwrap_or(400)
    }

    /// `OS/2.usWidthClass` (1..9, where 5 = Medium/Normal), or 5 if `OS/2`
    /// is absent (ISO/IEC 14496-22:2019 §5.2.3).
    pub fn width_class(&self) -> u16 {
        self.os2.as_ref().map(|o| o.us_width_class).unwrap_or(5)
    }

    /// Borrow the parsed `OS/2` table (ISO/IEC 14496-22:2019 §5.2.3), when
    /// the font publishes one. Exposes the full field set: classification
    /// (weight / width / PANOSE / family class), `fsType` embedding
    /// permissions, `fsSelection` style bits, the sub/superscript and
    /// strikeout metrics, Unicode / code-page coverage ranges, vendor id,
    /// the typographic / Windows vertical metrics, and (versioned) x-height
    /// / cap-height / optical-size range.
    pub fn os2_table(&self) -> Option<&Os2Table> {
        self.os2.as_ref()
    }

    /// The `OS/2.fsType` embedding-permission state, distilled to the
    /// single most-restrictive applicable flag, or `None` when `OS/2` is
    /// absent. `installable` (no restriction bit) is the permissive
    /// default. See [`Os2Table`]'s `embedding_*` predicates for the raw
    /// bits.
    pub fn embedding_installable(&self) -> Option<bool> {
        self.os2.as_ref().map(|o| o.embedding_installable())
    }

    /// `post.italicAngle` in degrees (negative for forward-slanted).
    pub fn italic_angle(&self) -> f32 {
        self.post.as_ref().map(|p| p.italic_angle).unwrap_or(0.0)
    }

    /// `true` when the font ships a `post` table (any version).
    pub fn has_post(&self) -> bool {
        self.post.is_some()
    }

    /// `true` when the font carries PostScript (`CFF `) outlines rather
    /// than (or in addition to) TrueType `glyf` outlines.
    pub fn has_cff_outlines(&self) -> bool {
        self.cff.is_some()
    }

    /// Borrow the parsed `CFF ` table, when the font ships one.
    pub fn cff_table(&self) -> Option<&CffTable<'a>> {
        self.cff.as_ref()
    }

    /// `true` when the font carries variable PostScript (`CFF2`) outlines.
    pub fn has_cff2_outlines(&self) -> bool {
        self.cff2.is_some()
    }

    /// Borrow the parsed `CFF2` table, when the font ships one.
    pub fn cff2_table(&self) -> Option<&Cff2Table<'a>> {
        self.cff2.as_ref()
    }

    /// `true` when the `CFF ` table is CID-keyed (Adobe TN #5176 §18).
    pub fn is_cid_keyed(&self) -> bool {
        self.cff.as_ref().is_some_and(|c| c.is_cid())
    }

    /// `true` when the font ships a `MATH` table (math typesetting data).
    pub fn has_math(&self) -> bool {
        self.math.is_some()
    }

    /// Borrow the parsed `MATH` table, when the font publishes one
    /// (ISO/IEC 14496-22:2019 §6.3.6).
    pub fn math_table(&self) -> Option<&MathTable<'a>> {
        self.math.as_ref()
    }

    /// A `MathConstants` value (one of the `tables::math::constant::*`
    /// indices) resolved at the font's current variation instance.
    ///
    /// Folds in the record's VariationIndex correction (§6.3.6.2.1)
    /// against the GDEF `ItemVariationStore` at the instance set via
    /// [`Self::set_variation_coords`]. Returns `None` when the font has no
    /// MATH table or no MathConstants sub-table; the value is in font
    /// design units (fractional after variation). For a non-variable font
    /// the result equals the plain `MathConstants` design-unit value.
    pub fn math_constant_var(&self, index: usize) -> Option<f32> {
        let c = self.math.as_ref()?.constants()?;
        let ivs = self.gdef_item_variation_store();
        let coords = self.normalised_coords();
        Some(c.value_resolved(index, ivs.as_ref(), &coords))
    }

    /// Per-glyph MATH italics correction for `gid` resolved at the current
    /// variation instance (§6.3.6.2.5 + §6.3.6.2.1). `None` when there is
    /// no MATH table, no MathGlyphInfo, or `gid` is uncovered.
    pub fn math_italics_correction_var(&self, gid: u16) -> Option<f32> {
        let gi = self.math.as_ref()?.glyph_info()?;
        let ivs = self.gdef_item_variation_store();
        let coords = self.normalised_coords();
        gi.italics_correction_resolved(gid, ivs.as_ref(), &coords)
    }

    /// Per-glyph MATH top-accent attachment point for `gid` resolved at the
    /// current variation instance (§6.3.6.2.6 + §6.3.6.2.1). `None` when
    /// uncovered (the layout engine then uses the glyph's geometric centre).
    pub fn math_top_accent_attachment_var(&self, gid: u16) -> Option<f32> {
        let gi = self.math.as_ref()?.glyph_info()?;
        let ivs = self.gdef_item_variation_store();
        let coords = self.normalised_coords();
        gi.top_accent_attachment_resolved(gid, ivs.as_ref(), &coords)
    }

    /// MATH per-corner kern value for `gid` at correction `height`,
    /// resolved at the current variation instance (§6.3.6.2.8/.9 +
    /// §6.3.6.2.1). `None` when `gid` has no kern table for `corner`.
    pub fn math_kern_var(&self, gid: u16, corner: MathKernCorner, height: i16) -> Option<f32> {
        let gi = self.math.as_ref()?.glyph_info()?;
        let ivs = self.gdef_item_variation_store();
        let coords = self.normalised_coords();
        gi.math_kern_resolved(gid, corner, height, ivs.as_ref(), &coords)
    }

    /// MATH glyph-assembly italics correction for `gid` growing in `dir`,
    /// resolved at the current variation instance (§6.3.6.2.12 +
    /// §6.3.6.2.1). `None` when `gid` has no assembly in `dir`.
    pub fn math_assembly_italics_correction_var(
        &self,
        gid: u16,
        dir: GrowDirection,
    ) -> Option<f32> {
        let v = self.math.as_ref()?.variants()?;
        let ivs = self.gdef_item_variation_store();
        let coords = self.normalised_coords();
        v.assembly_italics_correction_resolved(gid, dir, ivs.as_ref(), &coords)
    }

    /// `true` when the font ships a `JSTF` table (justification data).
    pub fn has_jstf(&self) -> bool {
        self.jstf.is_some()
    }

    /// Borrow the parsed `JSTF` table, when the font publishes one
    /// (ISO/IEC 14496-22:2019 §6.3.5).
    pub fn jstf_table(&self) -> Option<&JstfTable<'a>> {
        self.jstf.as_ref()
    }

    /// `true` when the font ships a `DSIG` table (a digital signature).
    pub fn has_dsig(&self) -> bool {
        self.dsig.is_some()
    }

    /// Borrow the parsed `DSIG` table (ISO/IEC 14496-22:2019 §8.x), when
    /// the font publishes one. The table carries one or more PKCS#7
    /// signature blocks surfaced as raw bytes; this crate decodes the table
    /// structure but does not verify the signature cryptographically.
    pub fn dsig_table(&self) -> Option<&DsigTable<'a>> {
        self.dsig.as_ref()
    }

    /// `true` when the font ships a `MERG` table (glyph-merge declarations
    /// for antialias filtering, ISO/IEC 14496-22:2019 §5.7.5).
    pub fn has_merg(&self) -> bool {
        self.merg.is_some()
    }

    /// Borrow the parsed `MERG` table, when the font publishes one. The
    /// table maps glyphs to merge classes and gives a per-class-pair
    /// merge-entry byte; the run-processing algorithm that consumes those
    /// entries is a renderer concern.
    pub fn merg_table(&self) -> Option<&MergTable> {
        self.merg.as_ref()
    }

    /// Borrow the parsed `post` table. `None` when the font does not
    /// publish one.
    pub fn post_table(&self) -> Option<&PostTable> {
        self.post.as_ref()
    }

    /// Resolve glyph `gid`'s `post`-table name reference, when the
    /// table publishes one.
    ///
    /// Returns:
    ///
    /// - `Some(GlyphNameRef::Custom(name))` — the font supplied the
    ///   glyph's name as a v2.0 Pascal string. The string is already
    ///   trimmed of its length byte.
    /// - `Some(GlyphNameRef::StandardMac { index })` — the glyph
    ///   resolves to entry `index` of the 258-name standard Macintosh
    ///   glyph table (referenced through v1.0, v2.0, or v2.5). The
    ///   258-name array is staged in `docs/text/opentype/` and exposed
    ///   as [`STANDARD_MAC_GLYPH_NAMES`]; [`Font::glyph_name`] resolves
    ///   the index into the canonical name. This lower-level accessor
    ///   surfaces the raw index so tooling can introspect the reference
    ///   without name resolution.
    /// - `None` — the font has no `post` table, the table is v3.0
    ///   (no glyph names at all), `gid` falls outside the v2.0 /
    ///   v2.5 index array, or the index references a Pascal string
    ///   the pool cannot satisfy.
    pub fn glyph_name_ref(&self, gid: u16) -> Option<GlyphNameRef<'_>> {
        self.post.as_ref()?.glyph_name_ref(gid)
    }

    /// Convenience accessor: return the glyph's PostScript name,
    /// resolving **both** `post`-name branches.
    ///
    /// A font-supplied v2.0 Pascal string is returned directly; a
    /// `StandardMac { index }` reference (from v1.0, v2.0 with
    /// `glyphNameIndex < 258`, or v2.5) is resolved through the
    /// [`STANDARD_MAC_GLYPH_NAMES`] table into its canonical standard
    /// Macintosh name.
    ///
    /// Returns `None` when no name is available — the font has no
    /// `post` table, the table is v3.0 (no names at all), or `gid`
    /// falls outside the table's index space. Use
    /// [`Font::glyph_name_ref`] to distinguish the custom and
    /// standard-Mac branches when that matters.
    pub fn glyph_name(&self, gid: u16) -> Option<&str> {
        match self.glyph_name_ref(gid) {
            Some(GlyphNameRef::Custom(s)) => Some(s),
            Some(GlyphNameRef::StandardMac { index }) => {
                crate::tables::post::standard_mac_glyph_name(index)
            }
            None => {
                // OTTO/CFF fonts commonly ship a `post` v3.0 (no names);
                // the `CFF ` charset is then the only name source.
                self.cff.as_ref().and_then(|c| c.glyph_name(gid))
            }
        }
    }

    /// Reverse lookup: the glyph id named `name` by the `post` table,
    /// inverting [`Font::glyph_name`].
    ///
    /// Resolves over every named glyph the table publishes — v2.0
    /// custom Pascal strings and standard-Macintosh names alike (from
    /// v1.0, v2.0 with `glyphNameIndex < 258`, or v2.5). The comparison
    /// is exact byte equality (PostScript glyph names are ASCII).
    ///
    /// Returns the **lowest** glyph id carrying that name, or `None`
    /// when the font has no `post` table, the table is v3.0, or no glyph
    /// is named `name`.
    pub fn gid_for_glyph_name(&self, name: &str) -> Option<u16> {
        if let Some(gid) = self.post.as_ref().and_then(|p| p.gid_for_name(name)) {
            return Some(gid);
        }
        // OTTO/CFF fonts with a `post` v3.0 (no names) resolve through the
        // CFF charset instead.
        self.cff.as_ref().and_then(|c| c.gid_for_name(name))
    }

    /// Iterate every `(glyph_id, post-table name)` pair the font
    /// publishes, in ascending glyph-id order.
    ///
    /// Standard-Macintosh references are resolved to their canonical
    /// names; v2.0 custom strings are returned directly. Glyph ids the
    /// `post` table names with an unsatisfiable reference are skipped.
    /// The iterator is empty when the font has no `post` table or the
    /// table is v3.0 (no names at all).
    pub fn iter_glyph_names(&self) -> Box<dyn Iterator<Item = (u16, &str)> + '_> {
        // Prefer `post`-table names; fall back to the CFF charset for OTTO
        // fonts whose `post` is v3.0 (no names).
        let has_post_names = self
            .post
            .as_ref()
            .is_some_and(|p| p.iter_glyph_names().next().is_some());
        if has_post_names {
            Box::new(self.post.iter().flat_map(|p| p.iter_glyph_names()))
        } else if let Some(cff) = self.cff.as_ref() {
            Box::new(cff.iter_glyph_names())
        } else {
            Box::new(std::iter::empty())
        }
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
    /// coords first).
    ///
    /// Both simple **and** composite glyphs are retargeted. For a
    /// composite glyph the gvar packed point numbers address the
    /// *components* (plus four phantom points), not flattened outline
    /// points, per ISO/IEC 14496-22:2019 §7.3.4.3 — the per-component
    /// `(dx, dy)` placement deltas are folded into each component's
    /// X/Y offset (and scaled with the offset where
    /// `SCALED_COMPONENT_OFFSET` is set) before the children are
    /// flattened. Point-matched components take no delta, and nested
    /// components inherit their own glyph's variation when decoded as
    /// top-level glyphs, matching the spec's "most deeply-nested
    /// first" processing order.
    pub fn glyph_outline(&self, glyph_id: u16) -> Result<TtOutline, Error> {
        if glyph_id >= self.maxp.num_glyphs {
            return Err(Error::GlyphOutOfRange(glyph_id));
        }
        // OTTO (PostScript-outline) fonts carry no `glyf`; reconstruct the
        // outline from the `CFF ` Type 2 charstring instead. CFF outlines
        // are not gvar-variable in this crate (CFF2 is a separate table),
        // so the variation path below never applies to them.
        if self.glyf.is_none() {
            if let Some(cff) = self.cff.as_ref() {
                return Ok(cff.glyph_outline(glyph_id).unwrap_or_default());
            }
            if let Some(cff2) = self.cff2.as_ref() {
                // CFF2 outline at the current variation instance. When the
                // caller has set non-default axis coordinates, the
                // avar-bent normalised vector drives the `blend` operator;
                // otherwise the default instance is rendered.
                let cff2_variable = self.fvar.is_some()
                    && !self.var_coords.is_empty()
                    && self.coords_differ_from_default();
                let normalised = if cff2_variable {
                    self.normalised_coords()
                } else {
                    Vec::new()
                };
                return Ok(cff2
                    .glyph_outline_at(glyph_id, &normalised)
                    .unwrap_or_default());
            }
        }
        let variable =
            self.gvar.is_some() && !self.var_coords.is_empty() && self.coords_differ_from_default();
        // Compute the avar-bent normalised coordinate vector once and
        // share it across the whole (possibly recursive) composite walk.
        let normalised = if variable {
            self.normalised_coords()
        } else {
            Vec::new()
        };
        self.glyph_outline_at_depth(glyph_id, 0, variable, &normalised)
    }

    /// Recursive outline resolver. `depth` guards composite recursion;
    /// `variable` + `normalised` carry the variation context down through
    /// the §7.3.4.3 component walk so each component glyph is resolved
    /// with its own gvar deltas applied before placement.
    fn glyph_outline_at_depth(
        &self,
        glyph_id: u16,
        depth: u8,
        variable: bool,
        normalised: &[f32],
    ) -> Result<TtOutline, Error> {
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

        // Composite-glyph variation path (§7.3.4.3): apply per-component
        // placement deltas inside the composite decode rather than to
        // flattened outline points, and resolve each component glyph's
        // own variation via a recursive child resolver.
        if variable {
            if let Ok(n_comp) = glyf.composite_component_count(range.clone()) {
                if n_comp > 0 {
                    let gvar = self.gvar.as_ref().unwrap();
                    if let Ok(deltas) = gvar.glyph_component_deltas(glyph_id, n_comp, normalised) {
                        let resolve = |child_gid: u16, child_depth: u8| {
                            self.glyph_outline_at_depth(
                                child_gid,
                                child_depth,
                                variable,
                                normalised,
                            )
                        };
                        return glyf.glyph_outline_var(range, loca, depth, &deltas, &resolve);
                    }
                }
            }
        }

        let mut out = glyf.glyph_outline(range, loca, depth)?;
        if variable {
            let gvar = self.gvar.as_ref().unwrap();
            let n_pts: usize = out.contours.iter().map(|c| c.points.len()).sum();
            if n_pts > 0 && n_pts <= u16::MAX as usize {
                // Build the static contour structure + default grid
                // coordinates so the gvar layer can infer deltas for
                // points a tuple omits (IUP, ISO/IEC 14496-22:2019
                // §7.3.4.4). The default coordinates must be the
                // pre-delta outline points, in gvar point-number order
                // (= contour-concatenated order), which is exactly the
                // order `out.contours` flattens to here.
                let contours: Vec<Vec<(i32, i32)>> = out
                    .contours
                    .iter()
                    .map(|c| c.points.iter().map(|p| (p.x as i32, p.y as i32)).collect())
                    .collect();
                let info = tables::gvar::SimpleOutlineInfo::from_contours(&contours);
                if let Ok(deltas) = gvar.glyph_deltas_iup(glyph_id, &info, normalised) {
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
        Ok(out)
    }

    /// Per-glyph advance width in font units.
    ///
    /// For a composite glyph whose components include one carrying the
    /// `USE_MY_METRICS` flag (§5.3.4), the advance is taken from that
    /// component's `hmtx` entry rather than the composite's own — the spec
    /// uses this to force a composite (e.g. `i`-circumflex) to inherit a
    /// component's (e.g. dotless-`i`) metrics. The last flagged component
    /// wins; the chase is depth-bounded.
    pub fn glyph_advance(&self, glyph_id: u16) -> i16 {
        let effective = self.metrics_source_glyph(glyph_id);
        self.hmtx.advance(effective) as i16
    }

    /// Per-glyph left-side bearing in font units. Honours `USE_MY_METRICS`
    /// the same way as [`Font::glyph_advance`] (the spec forces both `aw`
    /// and `lsb` to the flagged component's values).
    pub fn glyph_lsb(&self, glyph_id: u16) -> i16 {
        let effective = self.metrics_source_glyph(glyph_id);
        self.hmtx.lsb(effective)
    }

    /// Resolve the glyph whose `hmtx` metrics a composite should adopt,
    /// following the `USE_MY_METRICS` component chain (§5.3.4). Returns
    /// `glyph_id` itself for simple glyphs, fonts without `glyf`/`loca`, or
    /// composites where no component sets the flag. The chase is bounded by
    /// the composite-depth limit and guards against a self-reference.
    fn metrics_source_glyph(&self, glyph_id: u16) -> u16 {
        let (loca, glyf) = match (self.loca.as_ref(), self.glyf.as_ref()) {
            (Some(l), Some(g)) => (l, g),
            _ => return glyph_id,
        };
        let mut current = glyph_id;
        // Bound the chase: a USE_MY_METRICS component can itself be a
        // composite that sets the flag, so follow the chain but never more
        // than a few hops (matching the outline composite-depth guard).
        for _ in 0..8u8 {
            let range = match loca.glyph_range(current) {
                Ok(r) => r,
                Err(_) => return current,
            };
            match glyf.use_my_metrics_glyph(range) {
                Ok(Some(next)) if next != current => current = next,
                _ => return current,
            }
        }
        current
    }

    /// `true` when the font ships both a `vhea` and `vmtx` table —
    /// i.e. it supplies vertical-layout metrics for CJK / Mongolian
    /// or other top-to-bottom-written scripts.
    pub fn has_vertical_metrics(&self) -> bool {
        self.vhea.is_some() && self.vmtx.is_some()
    }

    /// Borrow the parsed `vhea` table, when present.
    /// (ISO/IEC 14496-22:2019 §5.7.9.)
    pub fn vhea_table(&self) -> Option<&VheaTable> {
        self.vhea.as_ref()
    }

    /// Vertical typographic ascender from `vhea`. For v1.1 this is
    /// `vertTypoAscender` (distance in font design units from the
    /// ideographic em-box centre baseline to the right side of the
    /// em-box, per §5.7.9 v1.1 row 2); for v1.0 the same bytes are
    /// the centre-line-relative `ascent` field. Returns `None` if the
    /// font lacks a `vhea` table.
    pub fn vertical_ascent(&self) -> Option<i16> {
        self.vhea.map(|v| v.vert_typo_ascender)
    }

    /// Vertical typographic descender from `vhea` (v1.1
    /// `vertTypoDescender`; v1.0 `descent`).
    pub fn vertical_descent(&self) -> Option<i16> {
        self.vhea.map(|v| v.vert_typo_descender)
    }

    /// Vertical typographic line gap from `vhea` (v1.1
    /// `vertTypoLineGap`; v1.0 row "Reserved; set to 0", so static
    /// v1.0 fonts will return `Some(0)` here).
    pub fn vertical_line_gap(&self) -> Option<i16> {
        self.vhea.map(|v| v.vert_typo_line_gap)
    }

    /// `vhea.advanceHeightMax` — the maximum advance height in the
    /// font, in design units. Per §5.7.9 the field is `int16`.
    pub fn advance_height_max(&self) -> Option<i16> {
        self.vhea.map(|v| v.advance_height_max)
    }

    /// Borrow the parsed `vmtx` table, when present.
    /// (ISO/IEC 14496-22:2019 §5.7.10.)
    pub fn vmtx_table(&self) -> Option<&VmtxTable<'a>> {
        self.vmtx.as_ref()
    }

    /// Per-glyph advance height in font design units. Returns `None`
    /// when the font lacks `vhea`/`vmtx`; otherwise returns the
    /// `vMetrics` advance for `glyph_id`, with the §5.7.10 "monospaced
    /// tail" rule (glyphs beyond `numOfLongVerMetrics` inherit the
    /// last pair's advance height) applied transparently.
    pub fn glyph_advance_height(&self, glyph_id: u16) -> Option<u16> {
        Some(self.vmtx.as_ref()?.advance_height(glyph_id))
    }

    /// Per-glyph top side bearing in font design units. Returns
    /// `None` when the font lacks `vmtx`.
    pub fn glyph_top_side_bearing(&self, glyph_id: u16) -> Option<i16> {
        Some(self.vmtx.as_ref()?.top_side_bearing(glyph_id))
    }

    /// Per-glyph vertical origin Y coordinate in font design units.
    /// Per §5.7.10 ("Vertical Origin and Advance Height"), this is
    /// `topSideBearing + glyph_bounding_box.y_max`. Returns `None`
    /// when the font lacks `vmtx` or when the glyph has no outline
    /// bounding box (empty glyph, blank glyph, or a CBDT-only colour-
    /// emoji font with no `glyf`/`loca`). For CFF fonts the spec
    /// recommends the optional `VORG` table instead; that path is not
    /// implemented here (TrueType outlines only).
    pub fn glyph_vertical_origin_y(&self, glyph_id: u16) -> Option<i16> {
        let tsb = self.vmtx.as_ref()?.top_side_bearing(glyph_id);
        let bbox = self.glyph_bounding_box(glyph_id)?;
        // Saturating add keeps a pathological bbox from panicking;
        // real-world fonts are nowhere near i16::MAX in this dim.
        Some(tsb.saturating_add(bbox.y_max))
    }

    /// `true` when the font ships a `VORG` table per §5.4.4. The table
    /// is optional and, per spec, restricted to CFF-flavoured sfnts;
    /// it appears occasionally in TrueType sfnts as well, in which case
    /// the parser surfaces the bytes but [`Self::vert_origin_y_from_vorg`]
    /// declines to consult it (the spec mandates "If present in
    /// TrueType OFF fonts it must be ignored by font clients").
    pub fn has_vorg(&self) -> bool {
        self.vorg.is_some()
    }

    /// Borrow the parsed `VORG` table, when present. Surfaced verbatim
    /// so callers that want to introspect the metrics array directly
    /// (e.g. font tooling) can do so without re-parsing the bytes.
    pub fn vorg_table(&self) -> Option<&VorgTable> {
        self.vorg.as_ref()
    }

    /// Default vertical-origin Y per §5.4.4, in font design units.
    /// Returns `None` when no `VORG` table is present.
    pub fn vorg_default_vert_origin_y(&self) -> Option<i16> {
        self.vorg.as_ref().map(|v| v.default_vert_origin_y)
    }

    /// Y coordinate of the vertical origin for `glyph_id` per `VORG`
    /// §5.4.4, in font design units.
    ///
    /// Returns:
    ///  - `None` when the font has no `VORG`.
    ///  - `None` when the font is TrueType-flavoured (a `glyf` table is
    ///    present). §5.4.4 mandates "If present in TrueType OFF fonts
    ///    it must be ignored by font clients, just as any other
    ///    unrecognized table would be"; we honour that rule here.
    ///    Callers that want the TrueType-derived origin should use
    ///    [`Self::glyph_vertical_origin_y`] (which derives the value
    ///    from `vmtx.topSideBearing` + `glyf` bbox per §5.7.10).
    ///  - `Some(default_vert_origin_y)` when the glyph has no per-glyph
    ///    override entry — §5.4.4 size-optimised form ("glyphs whose
    ///    vertical origin's y coordinate equals defaultVertOriginY will
    ///    not have an entry").
    ///  - `Some(vert_origin_y)` from the metrics-array override when
    ///    one is present.
    pub fn vert_origin_y_from_vorg(&self, glyph_id: u16) -> Option<i16> {
        let vorg = self.vorg.as_ref()?;
        // §5.4.4: TrueType clients must ignore the table. The presence
        // of `glyf` is the canonical sfnt signal that the outlines are
        // TrueType (a CFF font carries `CFF ` or `CFF2` instead and has
        // no `glyf`/`loca`).
        if self.glyf.is_some() {
            return None;
        }
        Some(vorg.vert_origin_y(glyph_id))
    }

    /// `true` when the font ships a `BASE` table (ISO/IEC 14496-22:2019
    /// §6.3.1). The table is optional for both TrueType and CFF sfnts
    /// and is consulted by text-layout clients when aligning glyphs
    /// from different scripts on a common baseline.
    pub fn has_base(&self) -> bool {
        self.base.is_some()
    }

    /// Borrow the parsed `BASE` table when present. Exposes the
    /// HorizAxis / VertAxis trees plus (in v1.1 tables) the
    /// ItemVariationStore offset for variable-font baseline deltas.
    pub fn base_table(&self) -> Option<&BaseTable> {
        self.base.as_ref()
    }

    /// Per-script default Y baseline (HorizAxis, §6.3.1.3) for the
    /// given script tag and baseline tag. Returns the design-unit
    /// coordinate from the BaseValues entry whose index matches
    /// `baseline_tag` inside the Axis's BaseTagList.
    ///
    /// Returns `None` when:
    ///  - the font has no `BASE` table;
    ///  - the HorizAxis is missing (typical for CJK vertical-only
    ///    fonts);
    ///  - the script tag is not listed in the Axis's BaseScriptList
    ///    (§6.3.1.3 "If a script is not listed here, then the
    ///    text-processing client will render the script using the
    ///    layout information specified for the entire font");
    ///  - the BaseTagList is NULL or `baseline_tag` is not in it;
    ///  - the BaseValues array is shorter than the BaseTagList index.
    pub fn base_horiz_y_for_script_baseline(
        &self,
        script_tag: [u8; 4],
        baseline_tag: [u8; 4],
    ) -> Option<i16> {
        let base = self.base.as_ref()?;
        let h = base.horiz_axis.as_ref()?;
        let idx = h.baseline_index_for_tag(baseline_tag)?;
        let bs = h.base_script_for_tag(script_tag)?;
        let bv = bs.base_values.as_ref()?;
        bv.base_coords.get(idx).map(|c| c.coordinate())
    }

    /// Per-script default X baseline (VertAxis, §6.3.1.3) for the given
    /// script tag and baseline tag. Mirror of
    /// [`Self::base_horiz_y_for_script_baseline`] for vertical layout.
    pub fn base_vert_x_for_script_baseline(
        &self,
        script_tag: [u8; 4],
        baseline_tag: [u8; 4],
    ) -> Option<i16> {
        let base = self.base.as_ref()?;
        let v = base.vert_axis.as_ref()?;
        let idx = v.baseline_index_for_tag(baseline_tag)?;
        let bs = v.base_script_for_tag(script_tag)?;
        let bv = bs.base_values.as_ref()?;
        bv.base_coords.get(idx).map(|c| c.coordinate())
    }

    /// Variation-aware sibling of
    /// [`Self::base_horiz_y_for_script_baseline`]: a `BaseCoordFormat3`
    /// VariationIndex device offset is resolved against the BASE
    /// `ItemVariationStore` at the font's current instance, so the
    /// baseline Y tracks the design axes.
    pub fn base_horiz_y_for_script_baseline_var(
        &self,
        script_tag: [u8; 4],
        baseline_tag: [u8; 4],
    ) -> Option<i16> {
        let coords = self.normalised_coords();
        self.base
            .as_ref()?
            .horiz_baseline_y_resolved(script_tag, baseline_tag, &coords)
    }

    /// Variation-aware sibling of
    /// [`Self::base_vert_x_for_script_baseline`].
    pub fn base_vert_x_for_script_baseline_var(
        &self,
        script_tag: [u8; 4],
        baseline_tag: [u8; 4],
    ) -> Option<i16> {
        let coords = self.normalised_coords();
        self.base
            .as_ref()?
            .vert_baseline_x_resolved(script_tag, baseline_tag, &coords)
    }

    /// `true` when the font carries a `gasp` table
    /// (ISO/IEC 14496-22:2019 §5.3.7). Absent in many fonts; the
    /// rasteriser applies its default policy when missing.
    pub fn has_gasp(&self) -> bool {
        self.gasp.is_some()
    }

    /// Borrow the parsed `gasp` table when present. Carries the
    /// per-ppem rasterisation hints (`GASP_GRIDFIT`, `GASP_DOGRAY`,
    /// `GASP_SYMMETRIC_GRIDFIT`, `GASP_SYMMETRIC_SMOOTHING`) sorted
    /// by `rangeMaxPPEM`.
    pub fn gasp_table(&self) -> Option<&GaspTable> {
        self.gasp.as_ref()
    }

    /// Pick the `gasp` record that governs rasterisation at the given
    /// pixel-per-em size — the first record whose `rangeMaxPPEM` is at
    /// least `ppem` (§5.3.7). Returns `None` when the font ships no
    /// `gasp` table or every record's upper limit is below `ppem`; in
    /// either case the caller should fall back to the rasteriser's
    /// default policy.
    pub fn gasp_behavior_for_ppem(&self, ppem: u16) -> Option<&GaspRange> {
        self.gasp.as_ref()?.behavior_for_ppem(ppem)
    }

    /// `true` when the font ships an `LTSH` table (ISO/IEC 14496-22:2019
    /// §5.7.4). Absent in most fonts; rasterisers without one always
    /// grid-fit (or consult `hdmx` / `vdmx` if those are present
    /// instead) to find each glyph's true advance width.
    pub fn has_ltsh(&self) -> bool {
        self.ltsh.is_some()
    }

    /// Borrow the parsed `LTSH` table when present. Carries the
    /// per-glyph `yPels` array recording each glyph's linear-threshold
    /// ppem per §5.7.4.
    pub fn ltsh_table(&self) -> Option<&LtshTable> {
        self.ltsh.as_ref()
    }

    /// Lowest ppem at which the grid-fitted advance for `glyph_id` has
    /// converged on the rounded linear advance per §5.7.4 — i.e. the
    /// rasteriser may round the design-unit advance to integer pixels
    /// at every ppem at least the returned value. Returns `None` when
    /// the font ships no `LTSH` table or `glyph_id` is out of range.
    pub fn ltsh_threshold(&self, glyph_id: u16) -> Option<u8> {
        self.ltsh.as_ref()?.linear_threshold(glyph_id)
    }

    /// `true` when `glyph_id` is safe to advance-scale linearly at
    /// `ppem` per §5.7.4 — i.e. `ppem >= LTSH.yPels[glyph_id]`. When
    /// the font ships no `LTSH` table, returns `false` so the caller
    /// falls back to grid-fitting (which is what §5.7.4 also prescribes
    /// for fonts without an `LTSH`). Returns `false` for out-of-range
    /// `glyph_id`.
    pub fn ltsh_linearly_scales_at_ppem(&self, glyph_id: u16, ppem: u16) -> bool {
        match self.ltsh.as_ref() {
            Some(t) => t.linearly_scales_at_ppem(glyph_id, ppem),
            None => false,
        }
    }

    /// `true` when the font ships an `hdmx` table (ISO/IEC 14496-22:2019
    /// §5.7.2). Optional table; absent in most fonts. §7.3.5 forbids
    /// `hdmx` in variable fonts — a caller that wants to validate the
    /// font shape may pair this with [`Self::is_variable`].
    pub fn has_hdmx(&self) -> bool {
        self.hdmx.is_some()
    }

    /// Borrow the parsed `hdmx` table when present. Carries the
    /// per-ppem device records mapping each glyph to its grid-fitted
    /// integer-pixel advance width at that ppem.
    pub fn hdmx_table(&self) -> Option<&HdmxTable> {
        self.hdmx.as_ref()
    }

    /// Grid-fitted advance width of `glyph_id` at the requested
    /// `ppem`, in integer pixels, per §5.7.2. Returns `None` when the
    /// font ships no `hdmx`, when the requested `ppem` is not in the
    /// table's record array (§5.7.2 has no "round down" rule — the
    /// caller falls back to scan-converting), or when `glyph_id`
    /// exceeds the recorded per-glyph array. `ppem` is `u8` because
    /// the on-wire field that drives the lookup is `uint8`; values
    /// above 255 ppem are not representable in the table.
    pub fn hdmx_advance_pixels(&self, glyph_id: u16, ppem: u8) -> Option<u8> {
        self.hdmx.as_ref()?.advance_pixels(glyph_id, ppem)
    }

    /// The set of ppem sizes the font's `hdmx` table covers, in
    /// ascending order. Returns an empty `Vec` when no `hdmx` is
    /// present.
    pub fn hdmx_recorded_ppem_sizes(&self) -> Vec<u8> {
        match self.hdmx.as_ref() {
            Some(t) => t.recorded_ppem_sizes(),
            None => Vec::new(),
        }
    }

    /// `true` when the font ships a `VDMX` table (ISO/IEC 14496-22:2019
    /// §5.7.8). Optional table; absent in most fonts. §7.3.5 forbids
    /// `VDMX` in variable fonts — pair with [`Self::is_variable`] when
    /// validating a font's shape.
    pub fn has_vdmx(&self) -> bool {
        self.vdmx.is_some()
    }

    /// Borrow the parsed `VDMX` table when present. Carries one or
    /// more VDMX groups indexed via a per-aspect-ratio RatioRange
    /// array; each group publishes per-ppem `(yMax, yMin)` envelopes
    /// for the font as a whole.
    pub fn vdmx_table(&self) -> Option<&VdmxTable> {
        self.vdmx.as_ref()
    }

    /// `(yMax, yMin)` pel envelope for `(ppem, deviceXRatio,
    /// deviceYRatio)`, per §5.7.8's first-match RatioRange search.
    /// Returns `None` when the font ships no `VDMX`, when no
    /// RatioRange matches the device pair (and there is no `(0,0,0)`
    /// sentinel), or when the matched group does not record the
    /// exact `ppem` requested (§5.7.8 "need not be continuous" — no
    /// fallback to neighbouring records).
    ///
    /// For square-pixel screens the canonical call is
    /// `vdmx_y_extent_for_device(ppem, 1, 1)`.
    pub fn vdmx_y_extent_for_device(
        &self,
        ppem: u16,
        device_x_ratio: u8,
        device_y_ratio: u8,
    ) -> Option<(i16, i16)> {
        self.vdmx
            .as_ref()?
            .y_extent_for_device(ppem, device_x_ratio, device_y_ratio)
    }

    /// Convenience for the common square-pixel case: equivalent to
    /// `vdmx_y_extent_for_device(ppem, 1, 1)`. Returns the `(yMax,
    /// yMin)` pel envelope at `ppem` under the 1:1 RatioRange
    /// (matching either the explicit `(xRatio=1, yStartRatio=1,
    /// yEndRatio=1)` entry, or the `(0,0,0)` catch-all sentinel
    /// when present), or `None` otherwise.
    pub fn vdmx_y_extent_square(&self, ppem: u16) -> Option<(i16, i16)> {
        self.vdmx_y_extent_for_device(ppem, 1, 1)
    }

    /// `true` when the font ships a `meta` (Metadata) table per
    /// ISO/IEC 14496-22:2019 §5.7.6.
    pub fn has_meta(&self) -> bool {
        self.meta.is_some()
    }

    /// Borrow the parsed `meta` table when present.
    ///
    /// The returned [`MetaTable`] carries the §5.7.6 DataMap array;
    /// per-record payloads borrow from the on-wire `meta` byte slice
    /// for the lifetime of the [`Font`].
    pub fn meta_table(&self) -> Option<&MetaTable<'a>> {
        self.meta.as_ref()
    }

    /// First `meta` DataMap record whose tag equals `tag`, or
    /// `None`. §5.7.6.1's closing paragraph permits multiple records
    /// for the same tag but specifies that "any instances after the
    /// first may be ignored" for single-record tags; this accessor
    /// honours that rule by returning the first match. Callers that
    /// want every record for a duplicated tag should iterate
    /// [`MetaTable::records`] directly.
    pub fn meta_record(&self, tag: &[u8; 4]) -> Option<MetaRecord<'_>> {
        self.meta.as_ref()?.record(tag)
    }

    /// Design-language declaration from the `meta` table's `'dlng'`
    /// record (ISO/IEC 14496-22:2019 §5.7.6.2), if present and
    /// well-formed UTF-8. The value is a comma-separated list of
    /// ScriptLangTags identifying the languages or scripts the font
    /// was primarily designed for.
    pub fn meta_design_languages(&self) -> Option<&'a str> {
        self.meta.as_ref()?.design_languages()
    }

    /// Supported-language declaration from the `meta` table's
    /// `'slng'` record (ISO/IEC 14496-22:2019 §5.7.6.2), if present
    /// and well-formed UTF-8. Used to declare languages or scripts
    /// the font is capable of supporting (a superset of
    /// [`Self::meta_design_languages`] in typical use).
    pub fn meta_supported_languages(&self) -> Option<&'a str> {
        self.meta.as_ref()?.supported_languages()
    }

    /// `true` when the font ships a `PCLT` (PCL 5) table per ISO/IEC
    /// 14496-22:2019 §5.7.7. The spec deems the table "strongly
    /// discouraged for OFF fonts with TrueType outlines", so a `true`
    /// here typically marks a legacy font.
    pub fn has_pclt(&self) -> bool {
        self.pclt.is_some()
    }

    /// Borrow the parsed `PCLT` table when present.
    ///
    /// The returned [`PcltTable`] carries the §5.7.7 PCL 5
    /// font-selection attributes: HP font number, pitch / x-height /
    /// cap-height design-unit metrics, the packed style / type-family
    /// / symbol-set words, the typeface "font print" string, the
    /// character-complement bitfield, the PCL file name, and the
    /// stroke-weight / width-type / serif-style classification bytes.
    pub fn pclt_table(&self) -> Option<&PcltTable> {
        self.pclt.as_ref()
    }

    /// `true` when the font ships an `SVG ` table per ISO/IEC
    /// 14496-22:2019/Amd.1:2020 §5.5.1 — vector colour-glyph
    /// descriptions as SVG 1.1 documents. This is one of the four
    /// colour-glyph mechanisms (`COLR`/`CPAL`, `CBDT`/`CBLC`, `sbix`,
    /// `SVG `); a font may ship more than one.
    pub fn has_svg(&self) -> bool {
        self.svg.is_some()
    }

    /// Borrow the parsed `SVG ` table when present.
    ///
    /// The returned [`SvgTable`] carries the §5.5.1 document records,
    /// each covering a contiguous glyph-ID range. Document payloads
    /// borrow from the on-wire `SVG ` byte slice and are surfaced raw
    /// (plain UTF-8 markup or gzip-encoded — test with
    /// [`SvgDocument::is_gzip_encoded`]).
    pub fn svg_table(&self) -> Option<&SvgTable<'a>> {
        self.svg.as_ref()
    }

    /// Resolve the raw SVG document covering `glyph_id`, or `None` when
    /// the font has no `SVG ` table or no document range covers the
    /// glyph. The returned [`SvgDocument`] borrows the on-wire document
    /// bytes (plain UTF-8 SVG 1.1 markup or a gzip-encoded stream per
    /// §5.5.2); inflation + XML parsing are the consumer renderer's
    /// responsibility, matching the raw-payload policy used for `sbix`
    /// and `CBDT` image strikes.
    pub fn svg_document(&self, glyph_id: u16) -> Option<&SvgDocument<'a>> {
        self.svg.as_ref()?.document_for_glyph(glyph_id)
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

    /// Like [`Self::gsub_features_for_script`], but honours the GSUB
    /// **FeatureVariations** table (ISO/IEC 14496-22:2019 §6.2.9) at the
    /// font's current variation instance.
    ///
    /// A variable font may publish a version-1.1 GSUB header that swaps
    /// the lookups behind a feature for an alternate set when the
    /// current instance falls inside a normalised range on one or more
    /// `fvar` axes (the canonical use is optical-size- or
    /// weight-conditional substitution). This accessor evaluates the
    /// active condition set against [`Self::normalised_coords`] and, for
    /// every feature whose index is overridden by the matching
    /// FeatureTableSubstitution, returns the alternate lookup-index list
    /// while keeping the feature tag unchanged.
    ///
    /// For static fonts, v1.0 GSUB headers, or instances that match no
    /// condition set, the result is identical to
    /// [`Self::gsub_features_for_script`]. Set the instance with
    /// [`Self::set_variation_coords`] first.
    pub fn gsub_features_for_script_at_instance(
        &self,
        script_tag: [u8; 4],
        lang_tag: Option<[u8; 4]>,
    ) -> Vec<GsubFeature> {
        match self.gsub.as_ref() {
            Some(g) => {
                let coords = self.normalised_coords();
                g.features_for_script_at_coords(script_tag, lang_tag, &coords)
            }
            None => Vec::new(),
        }
    }

    /// `true` when the GSUB table carries a §6.2.9 FeatureVariations
    /// table (a version-1.1 header with a non-zero offset). When this is
    /// `false`, [`Self::gsub_features_for_script_at_instance`] is
    /// identical to [`Self::gsub_features_for_script`].
    pub fn gsub_has_feature_variations(&self) -> bool {
        self.gsub
            .as_ref()
            .map(|g| g.has_feature_variations())
            .unwrap_or(false)
    }

    /// Return all GPOS features active for `script_tag` under `lang_tag`,
    /// each resolved to the list of lookup indices that implement it.
    ///
    /// The GPOS sibling of [`Self::gsub_features_for_script`]: it walks
    /// the same OpenType Layout ScriptList / FeatureList / LangSys
    /// substructure but over the positioning table, so a shaper can
    /// discover which lookup indices implement `kern` / `mark` / `mkmk`
    /// / `curs` / `cpsp` for the current script and feed them to the
    /// matching `gpos_apply_lookup_type_*` path.
    ///
    /// `lang_tag = None` selects the script's `DefaultLangSys`; an
    /// unrecognised `lang_tag` falls back to it too. The required
    /// feature (when present) is emitted first, then the LangSys's
    /// declared features in order. Returns an empty `Vec` when the font
    /// has no GPOS table or the script is absent.
    pub fn gpos_features_for_script(
        &self,
        script_tag: [u8; 4],
        lang_tag: Option<[u8; 4]>,
    ) -> Vec<GposFeature> {
        match self.gpos.as_ref() {
            Some(g) => g.features_for_script(script_tag, lang_tag),
            None => Vec::new(),
        }
    }

    /// Like [`Self::gpos_features_for_script`], but honours the GPOS
    /// **FeatureVariations** table (the shared ISO/IEC 14496-22:2019
    /// §6.2.9 substructure, reachable through a version-1.1 GPOS header)
    /// at the font's current variation instance.
    ///
    /// A variable font may publish a version-1.1 GPOS header that swaps
    /// the lookups behind a positioning feature for an alternate set
    /// when the current instance falls inside a normalised range on one
    /// or more `fvar` axes (e.g. weight-conditional kerning). This
    /// accessor evaluates the active condition set against
    /// [`Self::normalised_coords`] and, for every feature whose index is
    /// overridden by the matching FeatureTableSubstitution, returns the
    /// alternate lookup-index list while keeping the feature tag
    /// unchanged.
    ///
    /// For static fonts, v1.0 GPOS headers, or instances that match no
    /// condition set, the result is identical to
    /// [`Self::gpos_features_for_script`]. Set the instance with
    /// [`Self::set_variation_coords`] first.
    pub fn gpos_features_for_script_at_instance(
        &self,
        script_tag: [u8; 4],
        lang_tag: Option<[u8; 4]>,
    ) -> Vec<GposFeature> {
        match self.gpos.as_ref() {
            Some(g) => {
                let coords = self.normalised_coords();
                g.features_for_script_at_coords(script_tag, lang_tag, &coords)
            }
            None => Vec::new(),
        }
    }

    /// `true` when the GPOS table carries a §6.2.9 FeatureVariations
    /// table (a version-1.1 header with a non-zero offset). When this is
    /// `false`, [`Self::gpos_features_for_script_at_instance`] is
    /// identical to [`Self::gpos_features_for_script`].
    pub fn gpos_has_feature_variations(&self) -> bool {
        self.gpos
            .as_ref()
            .map(|g| g.has_feature_variations())
            .unwrap_or(false)
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

    /// Decode the GDEF `ItemVariationStore` (v1.3+), if present. The
    /// store feeds every variable-font GPOS / GDEF VariationIndex
    /// resolution. Returns `None` for fonts without a GDEF IVS or when
    /// the embedded store is malformed.
    fn gdef_item_variation_store(&self) -> Option<ItemVariationStore> {
        let bytes = self.gdef.as_ref()?.item_var_store_bytes()?;
        ItemVariationStore::parse(bytes).ok()
    }

    /// Variation-aware sibling of [`Self::lookup_kerning`].
    ///
    /// Resolves a GPOS pair's `xAdvance` VariationIndex against the GDEF
    /// `ItemVariationStore` at the font's current variation instance
    /// (set via [`Self::set_variation_coords`]), so variable kerning
    /// tracks the design axes. Falls back to the legacy `kern` table
    /// exactly like the static accessor. For a non-variable font, or
    /// one at its default instance, the result equals
    /// [`Self::lookup_kerning`].
    pub fn lookup_kerning_var(&self, left: u16, right: u16) -> i16 {
        if let Some(gpos) = &self.gpos {
            let ivs = self.gdef_item_variation_store();
            let coords = self.normalised_coords();
            let v = gpos.lookup_kerning_var(left, right, self.gdef.as_ref(), ivs.as_ref(), &coords);
            if v != 0 {
                return v;
            }
        }
        if let Some(kern) = &self.kern {
            return kern.lookup(left, right);
        }
        0
    }

    /// Variation-aware sibling of [`Self::lookup_mark_to_base`]:
    /// resolves AnchorFormat3 VariationIndex offsets against the GDEF
    /// `ItemVariationStore` at the current instance so the diacritic
    /// attachment point tracks the design axes.
    pub fn lookup_mark_to_base_var(&self, base: u16, mark: u16) -> Option<(i16, i16)> {
        let gpos = self.gpos.as_ref()?;
        let ivs = self.gdef_item_variation_store();
        let coords = self.normalised_coords();
        gpos.lookup_mark_to_base_var(base, mark, ivs.as_ref(), &coords)
    }

    /// Variation-aware sibling of [`Self::lookup_mark_to_mark`]:
    /// resolves AnchorFormat3 VariationIndex offsets against the GDEF
    /// `ItemVariationStore` at the current instance so the mark-on-mark
    /// stacking offset tracks the design axes.
    pub fn lookup_mark_to_mark_var(&self, mark1: u16, mark2: u16) -> Option<(i16, i16)> {
        let gpos = self.gpos.as_ref()?;
        let ivs = self.gdef_item_variation_store();
        let coords = self.normalised_coords();
        gpos.lookup_mark_to_mark_var(mark1, mark2, ivs.as_ref(), &coords)
    }

    /// Variation-aware sibling of [`Self::lookup_cursive_attachment`]:
    /// resolves AnchorFormat3 VariationIndex offsets on the entry / exit
    /// anchors against the GDEF `ItemVariationStore` at the current
    /// instance.
    pub fn lookup_cursive_attachment_var(&self, gid: u16) -> Option<CursiveAttachment> {
        let gpos = self.gpos.as_ref()?;
        let ivs = self.gdef_item_variation_store();
        let coords = self.normalised_coords();
        gpos.lookup_cursive_attachment_var(gid, ivs.as_ref(), &coords)
    }

    /// Variation-aware sibling of [`Self::gpos_apply_lookup_type_1`]:
    /// resolves the matched ValueRecord's VariationIndex device offsets
    /// against the GDEF `ItemVariationStore` at the current instance.
    pub fn gpos_apply_lookup_type_1_var(&self, lookup_index: u16, gid: u16) -> Option<PosValue> {
        let gpos = self.gpos.as_ref()?;
        let ivs = self.gdef_item_variation_store();
        let coords = self.normalised_coords();
        gpos.apply_lookup_type_1_var(lookup_index, gid, ivs.as_ref(), &coords)
    }

    /// Resolve a ligature glyph's GDEF carets to concrete font-unit
    /// coordinates at the current variation instance (CaretValueFormat3
    /// VariationIndex deltas applied from the GDEF `ItemVariationStore`;
    /// Format2 contour-point carets surface as `None`). Returns `None`
    /// when the font has no GDEF ligature-caret list covering `gid`.
    /// See [`GdefTable::ligature_carets_resolved`].
    pub fn ligature_carets_resolved(&self, gid: u16) -> Option<Vec<Option<i16>>> {
        let gdef = self.gdef.as_ref()?;
        let ivs = self.gdef_item_variation_store();
        let coords = self.normalised_coords();
        gdef.ligature_carets_resolved(gid, ivs.as_ref(), &coords)
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

    /// Variation-aware sibling of [`Self::lookup_mark_to_ligature`]:
    /// resolves AnchorFormat3 VariationIndex offsets against the GDEF
    /// `ItemVariationStore` at the font's current instance.
    pub fn lookup_mark_to_ligature_var(
        &self,
        ligature: u16,
        ligature_component: u16,
        mark: u16,
    ) -> Option<(i16, i16)> {
        let gpos = self.gpos.as_ref()?;
        let ivs = self.gdef_item_variation_store();
        let coords = self.normalised_coords();
        gpos.lookup_mark_to_ligature_var(ligature, ligature_component, mark, ivs.as_ref(), &coords)
    }

    /// Apply GPOS LookupType 7 (Contextual Positioning) to the glyph
    /// run starting at `pos` via the lookup at `lookup_index`.
    ///
    /// LookupType 7 is the non-chained sibling of LookupType 8: it
    /// matches an input glyph sequence (no backtrack / lookahead) and,
    /// on a hit, dispatches the rule's `SequenceLookupRecord[]` into
    /// nested per-glyph positioning lookups. Returns `Some(records)` —
    /// a `Vec<PosRecord>` of the per-glyph adjustments emitted — when a
    /// sub-table matches the input window at `pos`. Each
    /// `PosRecord.glyph_index` is an absolute offset into `gids`.
    ///
    /// All three sub-table formats (1 glyph-sequence, 2 class-based,
    /// 3 coverage-based) are supported. ExtensionPos (LookupType 9)
    /// wrappers are unwrapped transparently; nested records into
    /// LookupType 1 / 2 / 3 / 4 / 6 / 7 / 8 dispatch through the same
    /// bounded-recursion machinery as the chained path.
    pub fn gpos_apply_lookup_type_7(
        &self,
        lookup_index: u16,
        gids: &[u16],
        pos: usize,
    ) -> Option<Vec<PosRecord>> {
        self.gpos
            .as_ref()?
            .apply_lookup_type_7(lookup_index, gids, pos)
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

    /// The `lookupFlag` of GSUB lookup `lookup_index` (`0` when there's
    /// no GSUB or the index is out of range). The low-byte skip bits —
    /// RIGHT_TO_LEFT `0x0001`, IGNORE_BASE_GLYPHS `0x0002`,
    /// IGNORE_LIGATURES `0x0004`, IGNORE_MARKS `0x0008`,
    /// USE_MARK_FILTERING_SET `0x0010` — control which glyphs a shaper
    /// skips when matching the lookup's input; the high byte is the
    /// `markAttachmentType` class. [`Self::shape`] honours these.
    pub fn gsub_lookup_flags(&self, lookup_index: u16) -> u16 {
        self.gsub
            .as_ref()
            .map(|g| g.lookup_flags(lookup_index))
            .unwrap_or(0)
    }

    /// The `lookupFlag` of GPOS lookup `lookup_index` (`0` when there's
    /// no GPOS or the index is out of range). Same bit layout as
    /// [`Self::gsub_lookup_flags`].
    pub fn gpos_lookup_flags(&self, lookup_index: u16) -> u16 {
        self.gpos
            .as_ref()
            .map(|g| g.lookup_flags(lookup_index))
            .unwrap_or(0)
    }

    /// The `markFilteringSet` index of GSUB lookup `lookup_index`, or
    /// `None` when the lookup does not carry `USE_MARK_FILTERING_SET`
    /// (`0x0010`). When present, the value indexes the GDEF
    /// `MarkGlyphSets` structure and the layout engine skips every mark
    /// glyph *not* in that set ([`Self::shape`] honours this through the
    /// shared skip predicate).
    pub fn gsub_lookup_mark_filtering_set(&self, lookup_index: u16) -> Option<u16> {
        self.gsub
            .as_ref()
            .and_then(|g| g.mark_filtering_set(lookup_index))
    }

    /// The `markFilteringSet` index of GPOS lookup `lookup_index`, or
    /// `None` when the lookup does not carry `USE_MARK_FILTERING_SET`.
    /// See [`Self::gsub_lookup_mark_filtering_set`].
    pub fn gpos_lookup_mark_filtering_set(&self, lookup_index: u16) -> Option<u16> {
        self.gpos
            .as_ref()
            .and_then(|g| g.mark_filtering_set(lookup_index))
    }

    /// The shared §2 ("Common Table Formats") lookup skip predicate:
    /// returns `true` when a lookup with `flags` (its `lookupFlag`) and
    /// the optional `mark_filtering_set` index must *skip* `glyph_id`
    /// while matching its input / backtrack / lookahead sequences.
    ///
    /// The rule, per the LookupFlag bit enumeration:
    ///
    /// * `IGNORE_BASE_GLYPHS` (`0x0002`) — skip glyphs whose GDEF
    ///   GlyphClassDef class is *base* (1).
    /// * `IGNORE_LIGATURES` (`0x0004`) — skip glyphs whose class is
    ///   *ligature* (2).
    /// * `IGNORE_MARKS` (`0x0008`) — skip every mark glyph (class 3).
    /// * `MARK_ATTACHMENT_CLASS_FILTER` (high byte `0xFF00`, non-zero) —
    ///   skip every *mark* glyph whose GDEF MarkAttachClassDef class is
    ///   not the specified class. Non-mark glyphs are unaffected.
    /// * `USE_MARK_FILTERING_SET` (`0x0010`) — skip every *mark* glyph
    ///   that is not a member of the GDEF mark glyph set named by
    ///   `mark_filtering_set`.
    ///
    /// `IGNORE_MARKS` subsumes both mark-specific filters (a lookup that
    /// already skips all marks ignores the mark-class / filtering-set
    /// qualifiers). With no GDEF table the predicate degenerates to
    /// "never skip", matching the §2 requirement that a GlyphClassDef
    /// table be present whenever a skip bit is set.
    pub fn lookup_skips_glyph(
        &self,
        flags: u16,
        mark_filtering_set: Option<u16>,
        glyph_id: u16,
    ) -> bool {
        let gdef = match self.gdef.as_ref() {
            Some(g) => g,
            None => return false,
        };
        let class = gdef.glyph_class(glyph_id);
        if flags & 0x0002 != 0 && class == tables::gdef::CLASS_BASE {
            return true;
        }
        if flags & 0x0004 != 0 && class == tables::gdef::CLASS_LIGATURE {
            return true;
        }
        let is_mark = class == tables::gdef::CLASS_MARK;
        if flags & 0x0008 != 0 && is_mark {
            return true;
        }
        // The two mark-class qualifiers only filter mark glyphs, and only
        // matter when IGNORE_MARKS has not already removed every mark.
        if is_mark {
            let attach_class = (flags >> 8) & 0x00FF;
            if attach_class != 0 && gdef.mark_attach_class(glyph_id) != attach_class {
                return true;
            }
            if let Some(set) = mark_filtering_set {
                if !gdef.mark_glyph_set_contains(set, glyph_id) {
                    return true;
                }
            }
        }
        false
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

    // ---- monochrome / grayscale bitmap glyphs (EBDT/EBLC) ----------------

    /// `true` if this font ships an EBDT/EBLC pair — i.e. carries
    /// embedded monochrome or grayscale bitmap glyphs (legacy pixel /
    /// CJK bitmap faces, hand-hinted small-size strikes). Returns `false`
    /// for outline-only and colour-bitmap-only fonts.
    pub fn has_gray_bitmaps(&self) -> bool {
        self.eblc.is_some() && self.ebdt.is_some()
    }

    /// All `(ppem_x, ppem_y)` strikes the monochrome / grayscale bitmap
    /// tables ship, in declaration order. Empty when the font lacks
    /// EBDT/EBLC. Useful for picking a strike before calling
    /// [`Font::glyph_gray_bitmap`].
    pub fn gray_strike_sizes(&self) -> Vec<(u8, u8)> {
        self.eblc
            .as_ref()
            .map(|c| c.ppem_sizes().collect())
            .unwrap_or_default()
    }

    /// Resolve `glyph_id`'s monochrome / grayscale bitmap at the strike
    /// whose `ppem_y` is closest to `target_ppem`. Returns `None` if the
    /// font has no EBDT/EBLC tables OR no strike contains `glyph_id` OR
    /// the strike's per-glyph entry is in an EBDT format we don't decode
    /// (format 4 compressed).
    ///
    /// Composite formats 8 / 9 (§5.6.2.2.8 / §5.6.2.2.9) **are** decoded:
    /// the composite's component glyphs are resolved out of the same strike
    /// and blitted onto the composite's canvas at their per-component
    /// `(xOffset, yOffset)` offsets (nested composites are followed up to a
    /// bounded depth). The returned `GrayBitmap` is the assembled image.
    ///
    /// On success returns a [`GrayBitmap`] whose `pixels` field is an
    /// unpacked `width * height` row-major grid of alpha coverage
    /// (`0x00` = transparent, `0xFF` = opaque), ready to blit as a glyph
    /// mask at `(bearing_x, bearing_y)`. Bit depths 1 / 2 / 4 / 8 are all
    /// expanded to the full 0..=255 range (§5.6.2.2 / §5.6.3.1).
    pub fn glyph_gray_bitmap(&self, glyph_id: u16, target_ppem: u8) -> Option<GrayBitmap> {
        let eblc = self.eblc.as_ref()?;
        let ebdt = self.ebdt.as_ref()?;
        let entry = eblc.lookup_glyph(glyph_id, target_ppem)?;
        // Pixel formats (1/2/5/6/7) decode directly. Composite formats
        // (8/9) assemble component glyphs from the *same* strike — resolve
        // them recursively at that strike's exact ppemY so every component
        // lands in the same pixel grid.
        if matches!(entry.image_format, 8 | 9) {
            return self.composite_gray_bitmap(glyph_id, entry.ppem_y, 0);
        }
        ebdt.lookup(&entry).ok().flatten()
    }

    /// Maximum `EBDT` composite (format 8 / 9) nesting depth. §5.6.2.2 says
    /// "the number of nesting levels is determined by implementation stack
    /// space"; we bound it to keep a malformed self-referential composite
    /// from recursing without limit.
    const EBDT_COMPOSITE_MAX_DEPTH: u8 = 8;

    /// Resolve `glyph_id` to a `GrayBitmap` at the *exact* strike ppemY,
    /// assembling composite (format 8 / 9) glyphs by recursively resolving
    /// and blitting their components. `depth` guards against runaway
    /// recursion in a malformed font.
    fn composite_gray_bitmap(&self, glyph_id: u16, ppem_y: u8, depth: u8) -> Option<GrayBitmap> {
        if depth > Self::EBDT_COMPOSITE_MAX_DEPTH {
            return None;
        }
        let eblc = self.eblc.as_ref()?;
        let ebdt = self.ebdt.as_ref()?;
        let entry = eblc.lookup_glyph(glyph_id, ppem_y)?;
        // A non-composite component decodes directly as pixels.
        if !matches!(entry.image_format, 8 | 9) {
            return ebdt.lookup(&entry).ok().flatten();
        }
        let comp = ebdt.lookup_composite(&entry).ok().flatten()?;
        // Allocate the composite's own canvas (white / transparent).
        let cw = comp.width as usize;
        let ch = comp.height as usize;
        let mut canvas = vec![0u8; cw.checked_mul(ch)?];
        // §5.6.2.2: each component's (xOffset, yOffset) places the top-left
        // corner of the component bitmap within the composite. Components
        // are blitted back-to-front in array order; a non-zero alpha
        // pixel overwrites what is underneath (the bitmaps are alpha masks,
        // so "max coverage wins" preserves overlap without a true blend —
        // the spec leaves the composite raster model to the rasteriser).
        for component in &comp.components {
            // Guard against a component pointing back at itself.
            if component.glyph_id == glyph_id {
                continue;
            }
            let part = self.composite_gray_bitmap(component.glyph_id, ppem_y, depth + 1)?;
            let pw = part.width as usize;
            let ph = part.height as usize;
            for py in 0..ph {
                let cy = component.y_offset as isize + py as isize;
                if cy < 0 || cy as usize >= ch {
                    continue;
                }
                for px in 0..pw {
                    let cx = component.x_offset as isize + px as isize;
                    if cx < 0 || cx as usize >= cw {
                        continue;
                    }
                    let src = part.pixels.get(py * pw + px).copied().unwrap_or(0);
                    let dst = &mut canvas[cy as usize * cw + cx as usize];
                    *dst = (*dst).max(src);
                }
            }
        }
        Some(GrayBitmap {
            width: comp.width,
            height: comp.height,
            bearing_x: comp.bearing_x,
            bearing_y: comp.bearing_y,
            advance: comp.advance,
            ppem: comp.ppem,
            bit_depth: comp.bit_depth,
            pixels: canvas,
        })
    }

    // ---- scaled embedded bitmaps (EBSC) ----------------------------------

    /// `true` if this font ships an `EBSC` table (ISO/IEC 14496-22:2019
    /// §5.6.4) — i.e. declares one or more synthesised bitmap strikes
    /// built by scaling a real `EBLC`/`EBDT` strike. Returns `false` for
    /// fonts without `EBSC`, including the common case of a font that has
    /// real embedded bitmaps but never scales them.
    pub fn has_ebsc(&self) -> bool {
        self.ebsc.is_some()
    }

    /// The parsed `EBSC` table, for tooling that wants to introspect the
    /// `BitmapScale` records directly (target / substitute ppem pairs and
    /// the per-strike line metrics).
    pub fn ebsc_table(&self) -> Option<&EbscTable> {
        self.ebsc.as_ref()
    }

    /// All target `(ppemX, ppemY)` sizes the `EBSC` table can synthesise
    /// by scaling, in declaration order. These are sizes at which a
    /// rasteriser can obtain a bitmap *without* a real strike existing at
    /// that ppem — [`Font::glyph_gray_bitmap_scaled`] resolves them.
    /// Empty when the font has no `EBSC`.
    pub fn ebsc_target_sizes(&self) -> Vec<(u8, u8)> {
        self.ebsc
            .as_ref()
            .map(|t| t.target_ppem_sizes().collect())
            .unwrap_or_default()
    }

    /// Resolve `glyph_id` at an `EBSC`-synthesised strike whose target
    /// `ppemY` equals `target_ppem`, returning a [`GrayBitmap`] whose
    /// pixel grid is the **real** substitute strike's imagery with the
    /// per-glyph metrics (width, height, bearings, advance) scaled by the
    /// `target / substitute` ppem ratio and rounded to the nearest integer
    /// pixel per §5.6.4. The `ppem` field of the returned bitmap is set to
    /// the synthesised target so the caller knows the intended display
    /// size.
    ///
    /// The pixel buffer itself is *not* resampled here — §5.6.4 leaves the
    /// actual scaling to the rasteriser ("a font to define a bitmap strike
    /// as a scaled version of another strike"); this method performs the
    /// table-level redirection and the metric scaling the spec mandates,
    /// and hands the source pixels through so the consumer crate can
    /// resample at its chosen filter quality. The reported `width` /
    /// `height` are the scaled dimensions the resampled grid should target.
    ///
    /// Returns `None` when the font has no `EBSC`, no `BitmapScale` record
    /// targets `target_ppem`, no real strike exists at the record's
    /// `substitutePpemY`, the substitute strike lacks `glyph_id`, or the
    /// substitute entry is in an undecoded `EBDT` format.
    pub fn glyph_gray_bitmap_scaled(&self, glyph_id: u16, target_ppem: u8) -> Option<GrayBitmap> {
        use crate::tables::ebsc::scale_metric;
        let ebsc = self.ebsc.as_ref()?;
        let eblc = self.eblc.as_ref()?;
        let ebdt = self.ebdt.as_ref()?;
        let scale = ebsc.scale_for_target_ppem(target_ppem)?;
        // Pull the real (substitute) strike's bitmap. We ask for the exact
        // substitute ppemY; `lookup_glyph` picks the strike whose ppemY is
        // closest, which lands on an exact match when the substitute strike
        // is present.
        let entry = eblc.lookup_glyph(glyph_id, scale.substitute_ppem_y)?;
        let src = ebdt.lookup(&entry).ok().flatten()?;
        // Scale metrics independently in X and Y per §5.6.4 ("scaling in
        // the x direction is independent of scaling in the y direction").
        let sx = scale.substitute_ppem_x;
        let sy = scale.substitute_ppem_y;
        let scaled_width = scale_metric(src.width as i32, scale.ppem_x, sx).clamp(0, 255) as u8;
        let scaled_height = scale_metric(src.height as i32, scale.ppem_y, sy).clamp(0, 255) as u8;
        let scaled_bearing_x =
            scale_metric(src.bearing_x as i32, scale.ppem_x, sx).clamp(-128, 127) as i8;
        let scaled_bearing_y =
            scale_metric(src.bearing_y as i32, scale.ppem_y, sy).clamp(-128, 127) as i8;
        let scaled_advance = scale_metric(src.advance as i32, scale.ppem_x, sx).clamp(0, 255) as u8;
        Some(GrayBitmap {
            width: scaled_width,
            height: scaled_height,
            bearing_x: scaled_bearing_x,
            bearing_y: scaled_bearing_y,
            advance: scaled_advance,
            ppem: scale.ppem_y,
            bit_depth: src.bit_depth,
            pixels: src.pixels,
        })
    }

    // ---- color layer glyphs (COLR / CPAL) --------------------------------

    /// `true` if this font ships a `COLR` + `CPAL` pair — i.e. carries
    /// vector colour-emoji glyphs as a per-glyph layer stack
    /// (Microsoft's Segoe UI Emoji, Twemoji's Mozilla cut, FiraCode's
    /// "color" variant, and so on). Returns `false` for plain
    /// outline-only fonts and for CBDT-only colour-emoji fonts.
    ///
    /// Both COLR versions are decoded: the v0 flat layer stack through
    /// [`Font::color_layers`], and the v1 paint graph through
    /// [`Font::color_paint_root`] / [`Font::color_paint`]. The spec
    /// prefers a v1 paint graph over a v0 layer stack for the same
    /// base glyph, so check [`Font::color_paint_root`] first when
    /// [`Font::has_colr_v1`] is set.
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

    // ---- COLR v1 paint graph ---------------------------------------------

    /// `true` if this font's `COLR` table carries a version-1
    /// BaseGlyphList — i.e. at least one glyph is defined as a paint
    /// graph (gradients / transforms / composites) rather than, or in
    /// addition to, a v0 flat layer stack.
    pub fn has_colr_v1(&self) -> bool {
        self.colr
            .as_ref()
            .map(|c| c.has_paint_graph())
            .unwrap_or(false)
    }

    /// Resolve `glyph_id` to the root [`PaintRef`] of its COLR v1
    /// colour-glyph graph (a binary search over the BaseGlyphList).
    /// `None` when the font has no v1 COLR data or the glyph has no
    /// paint record — fall back to [`Font::color_layers`] then, per
    /// the spec's v1-over-v0 preference order.
    pub fn color_paint_root(&self, glyph_id: u16) -> Option<PaintRef> {
        self.colr.as_ref()?.base_glyph_paint(glyph_id)
    }

    /// Decode one Paint node of a COLR v1 graph **at the current
    /// variation instance** (set with [`Font::set_variation_coords`] /
    /// [`Font::set_axis_value`]; static fonts and the default instance
    /// resolve identically). Every `PaintVar*` wire form folds its
    /// deltas into the same resolved [`Paint`] variant as its static
    /// twin.
    ///
    /// Child paints are surfaced as further [`PaintRef`]s: the caller
    /// owns traversal and **must bound depth / track visited refs** —
    /// the spec requires the graph to be acyclic, but a hostile font
    /// can tie a loop (e.g. through `PaintColrGlyph`).
    ///
    /// Returns `None` for an unrecognised paint format (the spec's
    /// forward-compatibility rule is to ignore it) or a malformed
    /// node.
    pub fn color_paint(&self, paint: PaintRef) -> Option<Paint> {
        let coords = self.normalised_coords();
        self.colr.as_ref()?.paint(paint, &coords)
    }

    /// The raw wire `format` byte of the Paint table at `paint` —
    /// distinguishes e.g. the four scale wire forms that
    /// [`Font::color_paint`] folds into [`Paint::Scale`], and a
    /// `PaintVar*` from its static twin.
    pub fn color_paint_format(&self, paint: PaintRef) -> Option<u8> {
        self.colr.as_ref()?.paint_format(paint)
    }

    /// The precomputed COLR v1 clip box covering `glyph_id`, resolved
    /// at the current variation instance. Variable clip boxes
    /// (ClipBoxFormat 2) round *outward* per the spec so the box only
    /// ever expands. `None` when the font has no ClipList or no clip
    /// record covers the glyph — compute the bound from the graph
    /// then.
    pub fn color_clip_box(&self, glyph_id: u16) -> Option<ClipBox> {
        let coords = self.normalised_coords();
        self.colr.as_ref()?.clip_box(glyph_id, &coords)
    }

    /// `true` when the COLR table ships a varIndexMap in the OpenType
    /// 1.9 "format 1" layout, which is outside the staged spec
    /// chapters: the paint graph still decodes but every variation
    /// delta resolves to 0 (default-instance values).
    pub fn colr_var_index_map_unsupported(&self) -> bool {
        self.colr
            .as_ref()
            .map(|c| c.var_index_map_unsupported())
            .unwrap_or(false)
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

    /// CPAL v1 palette **label**: the `name` table ID of a UI string
    /// naming palette `palette_index` (e.g. "Regular", "High Contrast").
    /// Returns `None` when the font has no CPAL table, the table is v0,
    /// the `paletteLabelArray` is absent, the palette index is out of
    /// range, or the slot holds the `0xFFFF` "no label" sentinel. Pass
    /// the returned ID to a `name`-table lookup to fetch the localized
    /// string.
    pub fn cpal_palette_label(&self, palette_index: u16) -> Option<u16> {
        self.cpal.as_ref()?.palette_label(palette_index)
    }

    /// CPAL v1 palette-**entry** label: the `name` table ID of a UI
    /// string naming palette entry `entry_index` (e.g. "Outline",
    /// "Fill"). The label applies uniformly across every palette in the
    /// font. Returns `None` when the font has no CPAL table, the table
    /// is v0, the `paletteEntryLabelArray` is absent, the entry index is
    /// out of range, or the slot holds the `0xFFFF` "no label" sentinel.
    pub fn cpal_palette_entry_label(&self, entry_index: u16) -> Option<u16> {
        self.cpal.as_ref()?.palette_entry_label(entry_index)
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

    /// Index of the axis carrying the four-byte `tag` (e.g. `*b"wght"`),
    /// or `None` when the font has no such axis (or is static).
    pub fn axis_index(&self, tag: &[u8; 4]) -> Option<usize> {
        self.variation_axes().iter().position(|a| &a.tag == tag)
    }

    /// Current user-space value of the axis with the four-byte `tag`,
    /// or `None` when the font has no such axis.
    pub fn axis_value(&self, tag: &[u8; 4]) -> Option<f32> {
        let i = self.axis_index(tag)?;
        self.var_coords.get(i).copied()
    }

    /// Set a single variation axis (identified by its four-byte `tag`,
    /// e.g. `*b"wght"` / `*b"wdth"` / `*b"slnt"` / `*b"opsz"` / `*b"ital"`)
    /// to a user-space `value`, leaving every other axis at its current
    /// value. The value is clamped to the axis's `[min, max]` range, like
    /// [`Self::set_variation_coords`].
    ///
    /// Returns `true` when the axis was found and updated, `false` for a
    /// static font or an unknown tag (in which case nothing changes).
    pub fn set_axis_value(&mut self, tag: &[u8; 4], value: f32) -> bool {
        let axes = match self.fvar.as_ref() {
            Some(f) => f.axes(),
            None => return false,
        };
        let i = match axes.iter().position(|a| &a.tag == tag) {
            Some(i) => i,
            None => return false,
        };
        if i >= self.var_coords.len() {
            return false;
        }
        let a = &axes[i];
        self.var_coords[i] = value.clamp(a.min, a.max);
        true
    }

    /// Set the variation coordinates to the named instance at `index`
    /// (its position in [`Self::named_instances`]). After this call the
    /// font renders/shapes as that designer-chosen design variant (e.g.
    /// "Bold", "Condensed Light").
    ///
    /// Each named-instance coordinate is clamped to its axis range, like
    /// [`Self::set_variation_coords`]. Instances whose stored coordinate
    /// vector is shorter than the axis count leave the trailing axes at
    /// their current value; longer vectors are truncated.
    ///
    /// Returns `true` when the instance existed and was applied, `false`
    /// for a static font or an out-of-range `index`.
    pub fn apply_named_instance(&mut self, index: usize) -> bool {
        let coords = match self.fvar.as_ref() {
            Some(f) => match f.instances().get(index) {
                Some(inst) => inst.coords.clone(),
                None => return false,
            },
            None => return false,
        };
        self.set_variation_coords(&coords);
        true
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

    // ---- Control Value Table (cvt) + CVT variations (cvar) ---------------

    /// Number of entries in the `cvt ` Control Value Table, or `0` when
    /// the font has no `cvt ` table. Each entry is an `int16` FWORD
    /// (ISO/IEC 14496-22:2019 §5.3.2); the count is the table length
    /// divided by two (a trailing odd byte, if any, is ignored).
    pub fn cvt_count(&self) -> u16 {
        match self.cvt_bytes {
            Some(b) => (b.len() / 2).min(u16::MAX as usize) as u16,
            None => 0,
        }
    }

    /// The static (un-varied) value of `cvt ` entry `index`, or `None`
    /// when the font has no `cvt ` table or `index` is out of range.
    /// This is the raw FWORD as authored, before any `cvar` instance
    /// delta is applied — see [`Self::cvt_value_varied`].
    pub fn cvt_value(&self, index: u16) -> Option<i16> {
        let b = self.cvt_bytes?;
        let off = index as usize * 2;
        crate::parser::read_i16(b, off).ok()
    }

    /// `true` if the font ships a `cvar` CVT-variations table.
    pub fn has_cvar(&self) -> bool {
        self.cvar.is_some()
    }

    /// Borrow the parsed `cvar` table, when present.
    pub fn cvar_table(&self) -> Option<&CvarTable<'a>> {
        self.cvar.as_ref()
    }

    /// Per-`cvt`-entry deltas for the current variation instance,
    /// computed against the `avar`-bent normalised coordinate vector
    /// (ISO/IEC 14496-22:2019 §7.3.2). Returns a `Vec<i32>` of length
    /// [`Self::cvt_count`]; every entry is `0` for a static font, a
    /// font without `cvar`, or the default instance. Index `i` is the
    /// delta to add to `cvt ` entry `i`.
    pub fn cvt_deltas(&self) -> Vec<i32> {
        let n = self.cvt_count();
        let cvar = match self.cvar.as_ref() {
            Some(c) => c,
            None => return vec![0; n as usize],
        };
        let axis_count = self.fvar.as_ref().map(|f| f.axes().len()).unwrap_or(0) as u16;
        let coords = self.normalised_coords();
        cvar.cvt_deltas(axis_count, n, &coords)
            .unwrap_or_else(|_| vec![0; n as usize])
    }

    /// The `cvt ` entry `index` with the current instance's `cvar`
    /// delta applied (saturating to the `i16` FWORD range), or `None`
    /// when the font has no `cvt ` table or `index` is out of range.
    /// For a static font or the default instance this equals
    /// [`Self::cvt_value`].
    pub fn cvt_value_varied(&self, index: u16) -> Option<i16> {
        let base = self.cvt_value(index)? as i32;
        let delta = match self.cvar.as_ref() {
            Some(cvar) => {
                let axis_count = self.fvar.as_ref().map(|f| f.axes().len()).unwrap_or(0) as u16;
                let coords = self.normalised_coords();
                cvar.cvt_deltas(axis_count, self.cvt_count(), &coords)
                    .ok()
                    .and_then(|d| d.get(index as usize).copied())
                    .unwrap_or(0)
            }
            None => 0,
        };
        Some((base + delta).clamp(i16::MIN as i32, i16::MAX as i32) as i16)
    }

    // ---- TrueType hinting programs (fpgm / prep) -------------------------

    /// The raw `fpgm` font-program bytes (TrueType bytecode run once when
    /// the font is first used, ISO/IEC 14496-22:2019 §5.3.3), or `None`
    /// when the font ships no `fpgm` table.
    ///
    /// This crate does **not** execute the bytecode (TrueType hinting is
    /// out of scope — modern anti-aliasing at typical sizes does not need
    /// it). The bytes are surfaced for tooling that introspects, edits, or
    /// round-trips the hinting program, and for a downstream interpreter.
    pub fn fpgm_program(&self) -> Option<&'a [u8]> {
        self.fpgm_bytes
    }

    /// The raw `prep` control-value-program bytes (TrueType bytecode run
    /// whenever the point size / font / transform changes, ISO/IEC
    /// 14496-22:2019 §5.3.x), or `None` when absent. Like `fpgm`, surfaced
    /// raw and not executed.
    pub fn prep_program(&self) -> Option<&'a [u8]> {
        self.prep_bytes
    }

    /// `true` if the font carries any TrueType hinting program (`fpgm`,
    /// `prep`, or a non-empty `cvt `). A purely outline-driven font with no
    /// hinting returns `false`. Note the bytecode is surfaced raw, never
    /// executed.
    pub fn has_hinting_program(&self) -> bool {
        self.fpgm_bytes.is_some_and(|b| !b.is_empty())
            || self.prep_bytes.is_some_and(|b| !b.is_empty())
            || self.cvt_count() != 0
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

    /// Borrow the parsed `HVAR` table, when present.
    pub fn hvar_table(&self) -> Option<&HvarTable> {
        self.hvar.as_ref()
    }

    /// Interpolated `HVAR` adjustment to the advance width of
    /// `glyph_id` at the current variation coordinates.
    ///
    /// Per ISO/IEC 14496-22:2019 §7.3.5.3, the application reads the
    /// default advance width from `hmtx` and adds this delta to derive
    /// the per-instance advance. When an `advanceWidthMapping` table
    /// is published, that map provides the `(outer, inner)` index
    /// pair; otherwise the glyph ID itself acts as the inner index
    /// and the outer index is zero (the implicit form).
    ///
    /// Returns `None` when the font lacks `HVAR` or when the resolved
    /// index pair is out of range for the embedded item variation
    /// store. Returns `Some(0.0)` when the variation evaluates to
    /// zero at the current instance (e.g. at the axis defaults).
    pub fn advance_width_variation_delta(&self, glyph_id: u16) -> Option<f32> {
        let h = self.hvar.as_ref()?;
        let coords = self.normalised_coords();
        h.advance_width_delta(glyph_id, &coords)
    }

    /// Interpolated `HVAR` adjustment to the left side bearing of
    /// `glyph_id`. Requires that the font ship a left-side-bearing
    /// mapping table (§7.3.5.2 says LSB / RSB lookups always need
    /// one); returns `None` otherwise.
    pub fn lsb_variation_delta(&self, glyph_id: u16) -> Option<f32> {
        let h = self.hvar.as_ref()?;
        let coords = self.normalised_coords();
        h.lsb_delta(glyph_id, &coords)
    }

    /// Interpolated `HVAR` adjustment to the right side bearing of
    /// `glyph_id`. Requires a right-side-bearing mapping table per
    /// §7.3.5.2; returns `None` otherwise.
    pub fn rsb_variation_delta(&self, glyph_id: u16) -> Option<f32> {
        let h = self.hvar.as_ref()?;
        let coords = self.normalised_coords();
        h.rsb_delta(glyph_id, &coords)
    }

    /// Per-glyph advance width **at the current variation instance**:
    /// the static `hmtx` advance (see [`Self::glyph_advance`]) plus the
    /// `HVAR` delta (§7.3.5.3), rounded to the nearest font unit. For a
    /// static font, a font without `HVAR`, or the default instance this
    /// equals [`Self::glyph_advance`]. The result is clamped to the
    /// `i32` range only in pathological inputs; advances are unsigned in
    /// `hmtx` but the fused value is returned signed for symmetry with
    /// [`Self::glyph_advance`].
    pub fn glyph_advance_varied(&self, glyph_id: u16) -> i16 {
        let base = self.hmtx.advance(glyph_id) as f32;
        let delta = self.advance_width_variation_delta(glyph_id).unwrap_or(0.0);
        (base + delta)
            .round()
            .clamp(i16::MIN as f32, i16::MAX as f32) as i16
    }

    /// Per-glyph left-side bearing **at the current variation
    /// instance**: the static `hmtx` LSB (see [`Self::glyph_lsb`]) plus
    /// the `HVAR` LSB delta (§7.3.5.2), rounded to the nearest font
    /// unit. Equals [`Self::glyph_lsb`] for a static font, a font
    /// without an `HVAR` LSB mapping, or the default instance.
    pub fn glyph_lsb_varied(&self, glyph_id: u16) -> i16 {
        let base = self.hmtx.lsb(glyph_id) as f32;
        let delta = self.lsb_variation_delta(glyph_id).unwrap_or(0.0);
        (base + delta)
            .round()
            .clamp(i16::MIN as f32, i16::MAX as f32) as i16
    }

    /// Borrow the parsed `VVAR` table, when present.
    pub fn vvar_table(&self) -> Option<&VvarTable> {
        self.vvar.as_ref()
    }

    /// Interpolated `VVAR` adjustment to the advance height of
    /// `glyph_id` at the current variation coordinates.
    ///
    /// Per ISO/IEC 14496-22:2019 §7.3.8.2 (cross-referenced back to
    /// §7.3.5.3), the application reads the default advance height
    /// from `vmtx` and adds this delta to derive the per-instance
    /// advance. When an `advanceHeightMapping` table is published,
    /// that map provides the `(outer, inner)` index pair; otherwise
    /// the glyph ID itself acts as the inner index and the outer index
    /// is zero (the implicit form).
    ///
    /// Returns `None` when the font lacks `VVAR` or when the resolved
    /// index pair is out of range for the embedded item variation
    /// store. Returns `Some(0.0)` when the variation evaluates to zero
    /// at the current instance (e.g. at the axis defaults).
    pub fn advance_height_variation_delta(&self, glyph_id: u16) -> Option<f32> {
        let v = self.vvar.as_ref()?;
        let coords = self.normalised_coords();
        v.advance_height_delta(glyph_id, &coords)
    }

    /// Interpolated `VVAR` adjustment to the top side bearing of
    /// `glyph_id`. Requires that the font ship a top-side-bearing
    /// mapping table (§7.3.8.2 inherits the §7.3.5.2 rule that side-
    /// bearing lookups always need a map); returns `None` otherwise.
    pub fn tsb_variation_delta(&self, glyph_id: u16) -> Option<f32> {
        let v = self.vvar.as_ref()?;
        let coords = self.normalised_coords();
        v.tsb_delta(glyph_id, &coords)
    }

    /// Interpolated `VVAR` adjustment to the bottom side bearing of
    /// `glyph_id`. Requires a bottom-side-bearing mapping table per
    /// §7.3.8.2; returns `None` otherwise.
    pub fn bsb_variation_delta(&self, glyph_id: u16) -> Option<f32> {
        let v = self.vvar.as_ref()?;
        let coords = self.normalised_coords();
        v.bsb_delta(glyph_id, &coords)
    }

    /// Per-glyph advance height **at the current variation instance**:
    /// the static `vmtx` advance height (see
    /// [`Self::glyph_advance_height`]) plus the `VVAR` advance-height
    /// delta (§7.3.8.2), rounded to the nearest font unit. Returns
    /// `None` when the font lacks `vhea`/`vmtx`. Equals
    /// [`Self::glyph_advance_height`] for a font without `VVAR` or at
    /// the default instance.
    pub fn glyph_advance_height_varied(&self, glyph_id: u16) -> Option<u16> {
        let base = self.vmtx.as_ref()?.advance_height(glyph_id) as f32;
        let delta = self.advance_height_variation_delta(glyph_id).unwrap_or(0.0);
        Some((base + delta).round().clamp(0.0, u16::MAX as f32) as u16)
    }

    /// Interpolated `VVAR` adjustment to the vertical-origin Y of
    /// `glyph_id`. §7.3.8.2 final paragraph: a mapping table is
    /// required for vertical-origin variation data, and the data is
    /// "not used in fonts with TrueType outlines" — populated only by
    /// CFF2 variable fonts that publish a `VORG` table. Returns
    /// `None` otherwise.
    pub fn vorg_variation_delta(&self, glyph_id: u16) -> Option<f32> {
        let v = self.vvar.as_ref()?;
        let coords = self.normalised_coords();
        v.vorg_delta(glyph_id, &coords)
    }

    /// Borrow the parsed `STAT` table, when present. Static fonts may
    /// omit it; variable fonts are required by ISO/IEC 14496-22:2019
    /// §7.3.7 to ship one.
    pub fn stat_table(&self) -> Option<&StatTable> {
        self.stat.as_ref()
    }

    /// `STAT.designAxes` — one record per design axis. For a variable
    /// font, every `fvar` axis must appear here; the order is arbitrary
    /// (sort by `axis_ordering` if a stable UI order is needed).
    /// Returns an empty slice when no STAT table is present.
    pub fn stat_axes(&self) -> &[StatAxisRecord] {
        match self.stat.as_ref() {
            Some(s) => s.axes(),
            None => &[],
        }
    }

    /// `STAT.axisValueTables` — every axis value record in document
    /// order. Filter by axis tag with [`Self::stat_axis_values_for_tag`]
    /// or walk by format to compose subfamily strings under the
    /// R/B/I/BI, WWS, or unrestricted naming models (§7.3.7.3).
    /// Returns an empty slice when no STAT table is present.
    pub fn stat_axis_values(&self) -> &[StatAxisValue] {
        match self.stat.as_ref() {
            Some(s) => s.axis_values(),
            None => &[],
        }
    }

    /// `STAT.elidedFallbackNameID` — the `name` table nameID applied
    /// when every component of a composed subfamily string would be
    /// elided (§7.3.7.1). Returns `None` when the font ships no STAT
    /// table; returns name ID 2 ("Regular") for the deprecated v1.0
    /// header that lacked the field.
    pub fn stat_elided_fallback_name_id(&self) -> Option<u16> {
        Some(self.stat.as_ref()?.elided_fallback_name_id())
    }

    /// Every STAT axis-value record whose axis is `axis_tag` (e.g.
    /// `*b"wght"`, `*b"wdth"`). Format-4 records are matched when one
    /// of their contributing axes references this tag. Returns an
    /// empty iterator when the font has no STAT table or the tag is
    /// not in the design-axes array.
    pub fn stat_axis_values_for_tag(
        &self,
        axis_tag: [u8; 4],
    ) -> Box<dyn Iterator<Item = &StatAxisValue> + '_> {
        match self.stat.as_ref() {
            Some(s) => Box::new(s.axis_values_for_tag(axis_tag)),
            None => Box::new(core::iter::empty()),
        }
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
