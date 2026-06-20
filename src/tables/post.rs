//! `post` — PostScript metadata + glyph names.
//!
//! Decoded per ISO/IEC 14496-22:2019 §5.2.10 / MS Learn `otspec-post`
//! (`docs/text/opentype/otspec-post.html`). The fixed 32-byte header
//! is identical across every version and carries `italicAngle`,
//! underline geometry, the `isFixedPitch` boolean, and the four
//! PostScript memory-usage hints. After the header the layout
//! diverges per `version`:
//!
//! - **`0x00010000` (v1.0)** — no trailing data. The font is asserted
//!   to contain exactly the 258 glyphs of the standard Macintosh
//!   TrueType font in the standard order; glyph names are looked up
//!   from the system Macintosh glyph table by glyph id.
//! - **`0x00020000` (v2.0)** — `uint16 numGlyphs` + `uint16
//!   glyphNameIndex[numGlyphs]` + Pascal-format `stringData[…]`. Each
//!   glyph id selects an index `nameIdx`. If `nameIdx < 258` the
//!   glyph name is the corresponding standard Macintosh name; if
//!   `nameIdx >= 258` the name is the `(nameIdx - 258)`th Pascal
//!   string in `stringData`.
//! - **`0x00025000` (v2.5, deprecated)** — `uint16 numGlyphs` +
//!   `int8 offset[numGlyphs]`. For each glyph id `gid` the standard
//!   Macintosh glyph index is `gid + offset[gid]`. Used by legacy
//!   fonts whose glyph set is a permutation or subset of the
//!   standard Macintosh order.
//! - **`0x00030000` (v3.0)** — no trailing data, no glyph names.
//!   Required form for CFF v1 outline fonts; permitted for any font
//!   that does not wish to publish glyph names.
//!
//! Spec v4.0 is defined by Apple for non-OpenType use and is out of
//! scope per §5.2.10 ("not supported in OpenType"); it is rejected
//! here as an unsupported version.
//!
//! ## Standard-Macintosh-glyph-name list
//!
//! ISO §5.2.10.1 defers to "Reference [2]" (Apple's TrueType
//! Reference Manual, Chap 6 — the `RM06/Chap6post.html` page) for the
//! list of the 258 standard Macintosh glyph names. The MS Learn
//! `otspec-post` page does the same. That 258-name array is now staged
//! at `docs/text/opentype/post-standard-mac-glyph-names.md` and
//! transcribed verbatim into [`STANDARD_MAC_GLYPH_NAMES`]. This module:
//!
//! - decodes the post-table **structure** for all four versions
//!   (header + numGlyphs + index array + Pascal strings);
//! - exposes the per-glyph name index ([`GlyphNameRef::StandardMac`]
//!   `{ index }`) and the per-glyph Pascal string
//!   ([`GlyphNameRef::Custom`] `(&str)`) for callers that want the raw
//!   reference;
//! - resolves both branches into a single `Option<&str>` through
//!   [`PostTable::resolved_glyph_name`] and the convenience
//!   [`Font::glyph_name`](crate::Font::glyph_name) accessor, looking a
//!   `StandardMac` index up in [`STANDARD_MAC_GLYPH_NAMES`].

use crate::parser::{read_i16, read_i32, read_u16, read_u32, read_u8};
use crate::Error;

/// `post` table tag (`b"post"`).
pub const POST_TABLE_TAG: [u8; 4] = *b"post";

/// Fixed 32-byte common header length.
pub const POST_HEADER_LEN: usize = 32;

/// Version 1.0 (`0x00010000`). All names come from the standard
/// Macintosh order, addressed by glyph id.
pub const POST_VERSION_10: u32 = 0x0001_0000;

/// Version 2.0 (`0x00020000`). The most common form: per-glyph index
/// into the standard-Mac set or the local Pascal-string pool.
pub const POST_VERSION_20: u32 = 0x0002_0000;

/// Version 2.5 (`0x00025000`, deprecated). Per-glyph signed `int8`
/// offset into the standard-Macintosh glyph order.
pub const POST_VERSION_25: u32 = 0x0002_5000;

/// Version 3.0 (`0x00030000`). No glyph names; the only form
/// permitted for CFF v1 fonts.
pub const POST_VERSION_30: u32 = 0x0003_0000;

/// Inclusive upper bound on the standard-Macintosh glyph-name index
/// space. `glyphNameIndex` values strictly below this address the
/// 258-name standard set; values at or above it address the Pascal
/// string pool offset by this constant.
pub const STANDARD_MAC_GLYPH_COUNT: u16 = 258;

/// Maximum length of a v2.0 PostScript glyph name in bytes, per
/// §5.2.10.2: "Names must be no longer than 63 characters; some older
/// implementations can assume a length limit of 31 characters." This
/// is a tolerance ceiling — names up to and including 63 bytes are
/// accepted; longer names are tolerated (the Pascal length byte is
/// itself a `u8`, capping at 255) but flagged through
/// [`PostTable::has_oversize_glyph_name`] so callers can downgrade
/// gracefully.
pub const RECOMMENDED_GLYPH_NAME_MAX_LEN: usize = 63;

/// The 258 standard Macintosh glyph names, in their canonical ordering,
/// as referenced by `post` table formats 1.0, 2.0 (for
/// `glyphNameIndex < 258`), and 2.5.
///
/// Index 0 = `.notdef`, 1 = `.null`, 2 = `nonmarkingreturn`, …,
/// 257 = `dcroat`. All 258 entries are distinct. The ordering and the
/// names themselves are standards data defined by Apple's *TrueType
/// Reference Manual*, Chapter 6 `post` table, "`'post'` Format 1"
/// (Microsoft's OpenType `post` spec defers to Apple for the list) —
/// transcribed verbatim from
/// `docs/text/opentype/post-standard-mac-glyph-names.md`.
pub static STANDARD_MAC_GLYPH_NAMES: [&str; STANDARD_MAC_GLYPH_COUNT as usize] = [
    ".notdef",
    ".null",
    "nonmarkingreturn",
    "space",
    "exclam",
    "quotedbl",
    "numbersign",
    "dollar",
    "percent",
    "ampersand",
    "quotesingle",
    "parenleft",
    "parenright",
    "asterisk",
    "plus",
    "comma",
    "hyphen",
    "period",
    "slash",
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "colon",
    "semicolon",
    "less",
    "equal",
    "greater",
    "question",
    "at",
    "A",
    "B",
    "C",
    "D",
    "E",
    "F",
    "G",
    "H",
    "I",
    "J",
    "K",
    "L",
    "M",
    "N",
    "O",
    "P",
    "Q",
    "R",
    "S",
    "T",
    "U",
    "V",
    "W",
    "X",
    "Y",
    "Z",
    "bracketleft",
    "backslash",
    "bracketright",
    "asciicircum",
    "underscore",
    "grave",
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z",
    "braceleft",
    "bar",
    "braceright",
    "asciitilde",
    "Adieresis",
    "Aring",
    "Ccedilla",
    "Eacute",
    "Ntilde",
    "Odieresis",
    "Udieresis",
    "aacute",
    "agrave",
    "acircumflex",
    "adieresis",
    "atilde",
    "aring",
    "ccedilla",
    "eacute",
    "egrave",
    "ecircumflex",
    "edieresis",
    "iacute",
    "igrave",
    "icircumflex",
    "idieresis",
    "ntilde",
    "oacute",
    "ograve",
    "ocircumflex",
    "odieresis",
    "otilde",
    "uacute",
    "ugrave",
    "ucircumflex",
    "udieresis",
    "dagger",
    "degree",
    "cent",
    "sterling",
    "section",
    "bullet",
    "paragraph",
    "germandbls",
    "registered",
    "copyright",
    "trademark",
    "acute",
    "dieresis",
    "notequal",
    "AE",
    "Oslash",
    "infinity",
    "plusminus",
    "lessequal",
    "greaterequal",
    "yen",
    "mu",
    "partialdiff",
    "summation",
    "product",
    "pi",
    "integral",
    "ordfeminine",
    "ordmasculine",
    "Omega",
    "ae",
    "oslash",
    "questiondown",
    "exclamdown",
    "logicalnot",
    "radical",
    "florin",
    "approxequal",
    "Delta",
    "guillemotleft",
    "guillemotright",
    "ellipsis",
    "nonbreakingspace",
    "Agrave",
    "Atilde",
    "Otilde",
    "OE",
    "oe",
    "endash",
    "emdash",
    "quotedblleft",
    "quotedblright",
    "quoteleft",
    "quoteright",
    "divide",
    "lozenge",
    "ydieresis",
    "Ydieresis",
    "fraction",
    "currency",
    "guilsinglleft",
    "guilsinglright",
    "fi",
    "fl",
    "daggerdbl",
    "periodcentered",
    "quotesinglbase",
    "quotedblbase",
    "perthousand",
    "Acircumflex",
    "Ecircumflex",
    "Aacute",
    "Edieresis",
    "Egrave",
    "Iacute",
    "Icircumflex",
    "Idieresis",
    "Igrave",
    "Oacute",
    "Ocircumflex",
    "apple",
    "Ograve",
    "Uacute",
    "Ucircumflex",
    "Ugrave",
    "dotlessi",
    "circumflex",
    "tilde",
    "macron",
    "breve",
    "dotaccent",
    "ring",
    "cedilla",
    "hungarumlaut",
    "ogonek",
    "caron",
    "Lslash",
    "lslash",
    "Scaron",
    "scaron",
    "Zcaron",
    "zcaron",
    "brokenbar",
    "Eth",
    "eth",
    "Yacute",
    "yacute",
    "Thorn",
    "thorn",
    "minus",
    "multiply",
    "onesuperior",
    "twosuperior",
    "threesuperior",
    "onehalf",
    "onequarter",
    "threequarters",
    "franc",
    "Gbreve",
    "gbreve",
    "Idotaccent",
    "Scedilla",
    "scedilla",
    "Cacute",
    "cacute",
    "Ccaron",
    "ccaron",
    "dcroat",
];

/// Resolve a standard-Macintosh glyph-name index to its name.
///
/// Returns `None` for `index >= 258` (outside the standard set). The
/// [`GlyphNameRef::StandardMac`] variant guarantees `index < 258`, so a
/// lookup driven by it always succeeds.
pub fn standard_mac_glyph_name(index: u16) -> Option<&'static str> {
    STANDARD_MAC_GLYPH_NAMES.get(index as usize).copied()
}

/// Parsed `post` table. The common header is always populated;
/// `format` carries the version-dependent trailing data.
#[derive(Debug, Clone)]
pub struct PostTable {
    /// Raw `Version16Dot16` from the header. Preserved verbatim so
    /// callers that want to introspect the exact published version
    /// (e.g. to distinguish v1.0 from v2.0 explicitly) can do so;
    /// typed `format` covers the structural reading.
    pub version_raw: u32,
    /// Italic angle in counter-clockwise degrees from the vertical.
    /// `0.0` for upright; negative for forward-slanted (the common
    /// case).
    pub italic_angle: f32,
    /// Suggested y-coordinate of the top of the underline.
    pub underline_position: i16,
    /// Suggested underline thickness.
    pub underline_thickness: i16,
    /// `true` when the font is monospaced (header `isFixedPitch`
    /// `!= 0`), `false` for proportionally-spaced fonts.
    pub is_fixed_pitch: bool,
    /// PostScript memory-management hints. Set to `0` when the font
    /// foundry did not measure them.
    pub min_mem_type42: u32,
    /// See [`PostTable::min_mem_type42`].
    pub max_mem_type42: u32,
    /// See [`PostTable::min_mem_type42`].
    pub min_mem_type1: u32,
    /// See [`PostTable::min_mem_type42`].
    pub max_mem_type1: u32,
    /// Version-specific tail.
    pub format: PostFormat,
}

/// Version-dependent trailing data.
#[derive(Debug, Clone)]
pub enum PostFormat {
    /// v1.0: no trailing data. The font claims to be the standard
    /// Macintosh 258-glyph layout.
    Version10,
    /// v2.0: per-glyph `(name_index, optional Pascal string)` table.
    Version20(PostV20),
    /// v2.5 (deprecated): per-glyph signed offset into the standard
    /// Macintosh order.
    Version25(PostV25),
    /// v3.0: no glyph names at all.
    Version30,
}

/// Version 2.0 trailing data — index array + Pascal-string pool.
#[derive(Debug, Clone)]
pub struct PostV20 {
    /// `numGlyphs` — must match `maxp.numGlyphs`. Preserved verbatim
    /// so callers can sanity-check against `maxp`.
    pub num_glyphs: u16,
    /// `glyphNameIndex[numGlyphs]` — per-glyph index into either the
    /// standard Macintosh 258-name set or the Pascal-string pool.
    pub glyph_name_indices: Vec<u16>,
    /// Pascal strings extracted from `stringData`, in publication
    /// order. Index `k` here corresponds to the v2.0 lookup rule
    /// "subtract 258 from `nameIndex` and use that as the array
    /// index"; that is, `pascal_strings[k]` is the name a
    /// `glyphNameIndex` value of `258 + k` selects.
    pub pascal_strings: Vec<String>,
    /// `true` when at least one Pascal string exceeds the §5.2.10.2
    /// recommended 63-byte cap. Names are kept verbatim regardless;
    /// the flag exists so callers that need strict conformance can
    /// detect it without re-scanning.
    pub has_oversize_glyph_name: bool,
    /// `true` when at least one Pascal string contains a byte outside
    /// the §5.2.10.2 allow-set (`A..Z`, `a..z`, `0..9`, `.`, `_`).
    /// Such names still decode (they are interpreted as ASCII bytes
    /// because the §5.2.10.2 wording requires ASCII) but flagged so
    /// strict consumers can reject the font.
    pub has_non_conformant_glyph_name: bool,
}

/// Version 2.5 trailing data — per-glyph signed offset into the
/// standard Macintosh order.
#[derive(Debug, Clone)]
pub struct PostV25 {
    /// `numGlyphs` — must match `maxp.numGlyphs`.
    pub num_glyphs: u16,
    /// `offset[numGlyphs]` — signed delta from this font's glyph id
    /// to the standard Macintosh order. Per §5.2.10.3 the standard
    /// glyph index is `glyph_id + offset[glyph_id]`.
    pub offsets: Vec<i8>,
}

/// Resolved name of a single glyph as carried by `post`.
///
/// Callers consume this via [`PostTable::glyph_name_ref`]. The
/// `StandardMac { index }` variant carries a 0-based index into the
/// 258-name standard Macintosh glyph order; resolve it through
/// [`standard_mac_glyph_name`] (or use [`PostTable::resolved_glyph_name`]
/// to resolve both branches at once). The raw index is surfaced so
/// tooling can introspect the reference without name resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphNameRef<'a> {
    /// The glyph's name is the `index`th entry of the 258-name
    /// standard Macintosh glyph order. `index < 258` is guaranteed.
    StandardMac { index: u16 },
    /// The glyph's name is the font-supplied Pascal string. Already
    /// trimmed of its length byte.
    Custom(&'a str),
}

impl PostTable {
    /// Parse the `post` table from its slice. Returns `BadStructure`
    /// for unrecognised versions and `UnexpectedEof` for truncated
    /// arrays.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < POST_HEADER_LEN {
            return Err(Error::UnexpectedEof);
        }
        let version_raw = read_u32(bytes, 0)?;
        let italic_raw = read_i32(bytes, 4)?;
        let italic_angle = italic_raw as f32 / 65536.0;
        let underline_position = read_i16(bytes, 8)?;
        let underline_thickness = read_i16(bytes, 10)?;
        let is_fixed_pitch = read_u32(bytes, 12)? != 0;
        let min_mem_type42 = read_u32(bytes, 16)?;
        let max_mem_type42 = read_u32(bytes, 20)?;
        let min_mem_type1 = read_u32(bytes, 24)?;
        let max_mem_type1 = read_u32(bytes, 28)?;

        let tail = &bytes[POST_HEADER_LEN..];
        let format = match version_raw {
            POST_VERSION_10 => PostFormat::Version10,
            POST_VERSION_20 => PostFormat::Version20(parse_v20(tail)?),
            POST_VERSION_25 => PostFormat::Version25(parse_v25(tail)?),
            POST_VERSION_30 => PostFormat::Version30,
            _ => return Err(Error::BadStructure("post: unsupported version")),
        };

        Ok(Self {
            version_raw,
            italic_angle,
            underline_position,
            underline_thickness,
            is_fixed_pitch,
            min_mem_type42,
            max_mem_type42,
            min_mem_type1,
            max_mem_type1,
            format,
        })
    }

    /// `true` when the table carries any glyph-name information
    /// (v1.0, v2.0, or v2.5). v3.0 returns `false`.
    pub fn has_glyph_names(&self) -> bool {
        !matches!(self.format, PostFormat::Version30)
    }

    /// `true` when at least one v2.0 Pascal string exceeds the
    /// §5.2.10.2 recommended 63-byte limit. `false` for every other
    /// version.
    pub fn has_oversize_glyph_name(&self) -> bool {
        match &self.format {
            PostFormat::Version20(v) => v.has_oversize_glyph_name,
            _ => false,
        }
    }

    /// `true` when at least one v2.0 Pascal string contains a byte
    /// outside the §5.2.10.2 allow-set. `false` for every other
    /// version.
    pub fn has_non_conformant_glyph_name(&self) -> bool {
        match &self.format {
            PostFormat::Version20(v) => v.has_non_conformant_glyph_name,
            _ => false,
        }
    }

    /// Number of distinct Pascal strings in the v2.0 string pool, or
    /// `0` for every other version.
    pub fn pascal_string_count(&self) -> usize {
        match &self.format {
            PostFormat::Version20(v) => v.pascal_strings.len(),
            _ => 0,
        }
    }

    /// Look up the `gid`th glyph's name reference.
    ///
    /// Returns `None` when:
    /// - the table is v3.0 (no names at all);
    /// - `gid` is out of range for the v2.0 / v2.5 array;
    /// - the v2.0 `glyphNameIndex[gid]` selects a Pascal string the
    ///   pool does not actually contain (malformed font; preserved
    ///   as `None` rather than treated as a parse error so a single
    ///   bad glyph does not poison the whole table);
    /// - the v2.5 offset overflows `u16` (likewise malformed).
    ///
    /// For v1.0 every `gid < 258` yields
    /// `Some(GlyphNameRef::StandardMac { index: gid })`; for v1.0 the
    /// numGlyphs upper bound is not encoded inside `post` so the
    /// caller is responsible for keeping `gid` below `maxp.numGlyphs`.
    pub fn glyph_name_ref(&self, gid: u16) -> Option<GlyphNameRef<'_>> {
        match &self.format {
            PostFormat::Version10 => {
                if gid < STANDARD_MAC_GLYPH_COUNT {
                    Some(GlyphNameRef::StandardMac { index: gid })
                } else {
                    None
                }
            }
            PostFormat::Version20(v) => {
                let idx = *v.glyph_name_indices.get(gid as usize)?;
                if idx < STANDARD_MAC_GLYPH_COUNT {
                    Some(GlyphNameRef::StandardMac { index: idx })
                } else {
                    let pi = (idx - STANDARD_MAC_GLYPH_COUNT) as usize;
                    v.pascal_strings
                        .get(pi)
                        .map(|s| GlyphNameRef::Custom(s.as_str()))
                }
            }
            PostFormat::Version25(v) => {
                let off = *v.offsets.get(gid as usize)?;
                // Standard glyph index = gid + offset; reject if it
                // falls outside `[0, 258)`.
                let std = i32::from(gid) + i32::from(off);
                if (0..i32::from(STANDARD_MAC_GLYPH_COUNT)).contains(&std) {
                    Some(GlyphNameRef::StandardMac { index: std as u16 })
                } else {
                    None
                }
            }
            PostFormat::Version30 => None,
        }
    }

    /// Convenience: the v2.0 Pascal string for a glyph, if the font
    /// names it through a custom string. Returns `None` for the
    /// `StandardMac` indices, for missing glyph ids, and for every
    /// non-v2.0 version.
    pub fn custom_glyph_name(&self, gid: u16) -> Option<&str> {
        match self.glyph_name_ref(gid)? {
            GlyphNameRef::Custom(s) => Some(s),
            GlyphNameRef::StandardMac { .. } => None,
        }
    }

    /// Fully resolve a glyph's PostScript name, covering **both**
    /// branches: a font-supplied v2.0 Pascal string is returned
    /// directly, and a `StandardMac { index }` reference is resolved
    /// through [`STANDARD_MAC_GLYPH_NAMES`] into the canonical standard
    /// Macintosh name. Returns `None` only when the table publishes no
    /// name for `gid` (no `post`, v3.0, out-of-range glyph id, or an
    /// unsatisfiable Pascal pool reference).
    pub fn resolved_glyph_name(&self, gid: u16) -> Option<&str> {
        match self.glyph_name_ref(gid)? {
            GlyphNameRef::Custom(s) => Some(s),
            GlyphNameRef::StandardMac { index } => standard_mac_glyph_name(index),
        }
    }

    /// The number of glyphs whose names this table spans, when the count
    /// is encoded inside `post` itself.
    ///
    /// - v2.0 / v2.5 carry an explicit `numGlyphs`, returned here.
    /// - v1.0 does **not** encode a count: it asserts the font is the
    ///   258-glyph standard Macintosh layout, so [`STANDARD_MAC_GLYPH_COUNT`]
    ///   (258) is returned as the implied span.
    /// - v3.0 publishes no names, so `0` is returned.
    pub fn named_glyph_count(&self) -> u16 {
        match &self.format {
            PostFormat::Version10 => STANDARD_MAC_GLYPH_COUNT,
            PostFormat::Version20(v) => v.num_glyphs,
            PostFormat::Version25(v) => v.num_glyphs,
            PostFormat::Version30 => 0,
        }
    }

    /// Reverse lookup: the **first** glyph id whose resolved PostScript
    /// name equals `name`, scanning glyph ids in ascending order.
    ///
    /// This inverts [`PostTable::resolved_glyph_name`] over every named
    /// glyph the table publishes: a v2.0 custom Pascal string, a
    /// standard-Macintosh name (from v1.0, v2.0 `glyphNameIndex < 258`,
    /// or v2.5), are all matched. `name` is compared by exact byte
    /// equality (PostScript glyph names are ASCII per §5.2.10.2).
    ///
    /// The search bound is [`PostTable::named_glyph_count`]: for v1.0
    /// the whole 258-name standard set is searched; for v2.0 / v2.5 the
    /// table's own `numGlyphs`. v3.0 always returns `None`.
    ///
    /// Returns the lowest matching glyph id, or `None` when no glyph in
    /// range carries that name. When several glyphs share a name (legal
    /// but unusual), the lowest id wins — matching the convention that a
    /// font's first occurrence of a name is its canonical owner.
    pub fn gid_for_name(&self, name: &str) -> Option<u16> {
        if matches!(self.format, PostFormat::Version30) {
            return None;
        }
        let count = self.named_glyph_count();
        // A fast path for v2.0 standard-name queries: resolve the target
        // name to a standard-Mac index once, then the per-glyph compare
        // is an integer match rather than a string compare. Custom names
        // still fall through to the string path below.
        let std_target = STANDARD_MAC_GLYPH_NAMES
            .iter()
            .position(|n| *n == name)
            .map(|i| i as u16);
        for gid in 0..count {
            match self.glyph_name_ref(gid) {
                Some(GlyphNameRef::StandardMac { index }) if Some(index) == std_target => {
                    return Some(gid);
                }
                Some(GlyphNameRef::Custom(s)) if s == name => {
                    return Some(gid);
                }
                _ => {}
            }
        }
        None
    }

    /// Iterate every `(glyph_id, resolved_name)` pair the table
    /// publishes, in ascending glyph-id order.
    ///
    /// Glyph ids whose entry resolves to no name (e.g. a v2.0
    /// `glyphNameIndex` pointing past the Pascal pool, or a v2.5 offset
    /// landing outside the standard set) are skipped. v3.0 yields an
    /// empty iterator. The bound is [`PostTable::named_glyph_count`].
    pub fn iter_glyph_names(&self) -> impl Iterator<Item = (u16, &str)> + '_ {
        let count = self.named_glyph_count();
        (0..count).filter_map(move |gid| self.resolved_glyph_name(gid).map(|n| (gid, n)))
    }
}

fn parse_v20(tail: &[u8]) -> Result<PostV20, Error> {
    if tail.len() < 2 {
        return Err(Error::UnexpectedEof);
    }
    let num_glyphs = read_u16(tail, 0)?;
    let idx_bytes_len = 2usize
        .checked_mul(num_glyphs as usize)
        .ok_or(Error::BadStructure("post v2.0: numGlyphs overflow"))?;
    let idx_end = 2usize
        .checked_add(idx_bytes_len)
        .ok_or(Error::BadStructure("post v2.0: numGlyphs overflow"))?;
    if tail.len() < idx_end {
        return Err(Error::UnexpectedEof);
    }
    let mut glyph_name_indices = Vec::with_capacity(num_glyphs as usize);
    let mut max_pascal_referenced: i32 = -1;
    for i in 0..num_glyphs as usize {
        let v = read_u16(tail, 2 + i * 2)?;
        if v >= STANDARD_MAC_GLYPH_COUNT {
            let pi = (v - STANDARD_MAC_GLYPH_COUNT) as i32;
            if pi > max_pascal_referenced {
                max_pascal_referenced = pi;
            }
        }
        glyph_name_indices.push(v);
    }

    // Pascal strings extend from `idx_end` to the end of the table.
    let pool = &tail[idx_end..];
    let mut pascal_strings: Vec<String> = Vec::new();
    let mut has_oversize_glyph_name = false;
    let mut has_non_conformant_glyph_name = false;
    let mut p = 0usize;
    while p < pool.len() {
        let len = read_u8(pool, p)? as usize;
        p += 1;
        if p + len > pool.len() {
            return Err(Error::UnexpectedEof);
        }
        let raw = &pool[p..p + len];
        if len > RECOMMENDED_GLYPH_NAME_MAX_LEN {
            has_oversize_glyph_name = true;
        }
        if !raw.iter().all(|b| is_conformant_glyph_name_byte(*b)) {
            has_non_conformant_glyph_name = true;
        }
        // Per §5.2.10.2 names are ASCII; for non-conformant bytes we
        // still keep the byte values (clamped into a `String` via
        // `from_utf8_lossy`) so caller diagnostics can see them.
        let s = match std::str::from_utf8(raw) {
            Ok(s) => s.to_string(),
            Err(_) => String::from_utf8_lossy(raw).into_owned(),
        };
        pascal_strings.push(s);
        p += len;
    }

    // §5.2.10.2 worked example: glyphNameIndex[408] == 262 selects
    // pascal_strings[4]. A font that references a Pascal index its
    // pool cannot satisfy is malformed; `glyph_name_ref` returns
    // `None` for those gids, but we accept the parse so the
    // well-formed glyphs still decode.
    let _ = max_pascal_referenced;

    Ok(PostV20 {
        num_glyphs,
        glyph_name_indices,
        pascal_strings,
        has_oversize_glyph_name,
        has_non_conformant_glyph_name,
    })
}

fn parse_v25(tail: &[u8]) -> Result<PostV25, Error> {
    if tail.len() < 2 {
        return Err(Error::UnexpectedEof);
    }
    let num_glyphs = read_u16(tail, 0)?;
    let needed = 2usize
        .checked_add(num_glyphs as usize)
        .ok_or(Error::BadStructure("post v2.5: numGlyphs overflow"))?;
    if tail.len() < needed {
        return Err(Error::UnexpectedEof);
    }
    let mut offsets = Vec::with_capacity(num_glyphs as usize);
    for i in 0..num_glyphs as usize {
        offsets.push(tail[2 + i] as i8);
    }
    Ok(PostV25 {
        num_glyphs,
        offsets,
    })
}

fn is_conformant_glyph_name_byte(b: u8) -> bool {
    // §5.2.10.2 glyph-name allow-set: A..Z, a..z, 0..9, '.' (0x2E),
    // '_' (0x5F).
    b.is_ascii_uppercase() || b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(version: u32) -> Vec<u8> {
        let mut b = vec![0u8; POST_HEADER_LEN];
        b[0..4].copy_from_slice(&version.to_be_bytes());
        // italicAngle = -10.0 (Fixed: -10 * 65536)
        b[4..8].copy_from_slice(&((-10i32) << 16).to_be_bytes());
        b[8..10].copy_from_slice(&(-100i16).to_be_bytes());
        b[10..12].copy_from_slice(&50i16.to_be_bytes());
        b[12..16].copy_from_slice(&1u32.to_be_bytes());
        b[16..20].copy_from_slice(&0u32.to_be_bytes());
        b[20..24].copy_from_slice(&0u32.to_be_bytes());
        b[24..28].copy_from_slice(&0u32.to_be_bytes());
        b[28..32].copy_from_slice(&0u32.to_be_bytes());
        b
    }

    #[test]
    fn parses_minimal_v3_header() {
        let b = header(POST_VERSION_30);
        let p = PostTable::parse(&b).unwrap();
        assert_eq!(p.version_raw, POST_VERSION_30);
        assert!((p.italic_angle - (-10.0)).abs() < 0.001);
        assert_eq!(p.underline_position, -100);
        assert_eq!(p.underline_thickness, 50);
        assert!(p.is_fixed_pitch);
        assert!(matches!(p.format, PostFormat::Version30));
        assert!(!p.has_glyph_names());
        assert!(p.glyph_name_ref(0).is_none());
    }

    #[test]
    fn parses_v10_returns_standard_mac_indices() {
        let b = header(POST_VERSION_10);
        let p = PostTable::parse(&b).unwrap();
        assert!(matches!(p.format, PostFormat::Version10));
        assert!(p.has_glyph_names());
        assert_eq!(
            p.glyph_name_ref(0),
            Some(GlyphNameRef::StandardMac { index: 0 })
        );
        assert_eq!(
            p.glyph_name_ref(217),
            Some(GlyphNameRef::StandardMac { index: 217 })
        );
        // gid == 258 is out of the standard set; v1.0 cannot name it.
        assert!(p.glyph_name_ref(258).is_none());
        // Resolution into the canonical standard Macintosh names.
        assert_eq!(p.resolved_glyph_name(0), Some(".notdef"));
        assert_eq!(p.resolved_glyph_name(217), Some("tilde"));
        assert!(p.resolved_glyph_name(258).is_none());
    }

    /// §5.2.10.2 worked example: glyphNameIndex[302] is 217 → standard
    /// Macintosh entry 217; glyphNameIndex[408] is 262 → fifth Pascal
    /// string (index 4).
    #[test]
    fn v20_resolves_spec_worked_example() {
        let num_glyphs: u16 = 409;
        let mut tail = Vec::new();
        tail.extend_from_slice(&num_glyphs.to_be_bytes());
        for gid in 0..num_glyphs {
            let idx: u16 = match gid {
                302 => 217,
                408 => 262, // 258 + 4 → pascal_strings[4]
                _ => 0,     // .notdef placeholder
            };
            tail.extend_from_slice(&idx.to_be_bytes());
        }
        // Pascal pool: 5 strings.
        for name in ["one", "two", "three", "four", "weird"] {
            tail.push(name.len() as u8);
            tail.extend_from_slice(name.as_bytes());
        }

        let mut bytes = header(POST_VERSION_20);
        bytes.extend_from_slice(&tail);
        let p = PostTable::parse(&bytes).unwrap();
        assert!(p.has_glyph_names());
        assert_eq!(p.pascal_string_count(), 5);
        assert_eq!(
            p.glyph_name_ref(302),
            Some(GlyphNameRef::StandardMac { index: 217 })
        );
        assert_eq!(p.glyph_name_ref(408), Some(GlyphNameRef::Custom("weird")));
        assert_eq!(p.custom_glyph_name(408), Some("weird"));
        assert!(p.custom_glyph_name(302).is_none());
        // resolved_glyph_name covers both branches: the standard-Mac
        // reference resolves to "tilde", the custom one to "weird".
        assert_eq!(p.resolved_glyph_name(302), Some("tilde"));
        assert_eq!(p.resolved_glyph_name(408), Some("weird"));
    }

    #[test]
    fn v20_pascal_pool_indices_are_zero_based() {
        // First custom Pascal string is `nameIndex == 258`.
        let num_glyphs: u16 = 2;
        let mut tail = Vec::new();
        tail.extend_from_slice(&num_glyphs.to_be_bytes());
        tail.extend_from_slice(&258u16.to_be_bytes()); // gid 0 -> "Alpha"
        tail.extend_from_slice(&259u16.to_be_bytes()); // gid 1 -> "Beta"
        for name in ["Alpha", "Beta"] {
            tail.push(name.len() as u8);
            tail.extend_from_slice(name.as_bytes());
        }
        let mut bytes = header(POST_VERSION_20);
        bytes.extend_from_slice(&tail);
        let p = PostTable::parse(&bytes).unwrap();
        assert_eq!(p.glyph_name_ref(0), Some(GlyphNameRef::Custom("Alpha")));
        assert_eq!(p.glyph_name_ref(1), Some(GlyphNameRef::Custom("Beta")));
    }

    #[test]
    fn v20_rejects_truncated_pascal_string() {
        // Length byte claims 5 chars, only 3 follow.
        let num_glyphs: u16 = 1;
        let mut tail = Vec::new();
        tail.extend_from_slice(&num_glyphs.to_be_bytes());
        tail.extend_from_slice(&258u16.to_be_bytes());
        tail.push(5); // claim 5 bytes
        tail.extend_from_slice(b"abc");
        let mut bytes = header(POST_VERSION_20);
        bytes.extend_from_slice(&tail);
        assert!(matches!(
            PostTable::parse(&bytes),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn v20_flags_oversize_and_non_conformant_names() {
        // 1 glyph naming index, 1 oversize name (64 chars of 'a') and
        // 1 non-conformant name (contains '/').
        let num_glyphs: u16 = 2;
        let mut tail = Vec::new();
        tail.extend_from_slice(&num_glyphs.to_be_bytes());
        tail.extend_from_slice(&258u16.to_be_bytes()); // -> pool[0] (oversize)
        tail.extend_from_slice(&259u16.to_be_bytes()); // -> pool[1] (non-conformant)
        let oversize = "a".repeat(64);
        tail.push(oversize.len() as u8);
        tail.extend_from_slice(oversize.as_bytes());
        let bad = "weird/name";
        tail.push(bad.len() as u8);
        tail.extend_from_slice(bad.as_bytes());
        let mut bytes = header(POST_VERSION_20);
        bytes.extend_from_slice(&tail);
        let p = PostTable::parse(&bytes).unwrap();
        assert!(p.has_oversize_glyph_name());
        assert!(p.has_non_conformant_glyph_name());
    }

    #[test]
    fn v25_resolves_signed_offset_into_standard_set() {
        // §5.2.10.3 worked example: 3 glyphs (font ids 0, 1, 2) =
        // standard ids 36, 37, 38 (A, B, C in standard order). Each
        // offset is +36.
        let num_glyphs: u16 = 3;
        let mut tail = Vec::new();
        tail.extend_from_slice(&num_glyphs.to_be_bytes());
        tail.push(36i8 as u8);
        tail.push(36i8 as u8);
        tail.push(36i8 as u8);
        let mut bytes = header(POST_VERSION_25);
        bytes.extend_from_slice(&tail);
        let p = PostTable::parse(&bytes).unwrap();
        assert!(p.has_glyph_names());
        assert_eq!(
            p.glyph_name_ref(0),
            Some(GlyphNameRef::StandardMac { index: 36 })
        );
        assert_eq!(
            p.glyph_name_ref(1),
            Some(GlyphNameRef::StandardMac { index: 37 })
        );
        assert_eq!(
            p.glyph_name_ref(2),
            Some(GlyphNameRef::StandardMac { index: 38 })
        );
        // Standard indices 36/37/38 are A/B/C.
        assert_eq!(p.resolved_glyph_name(0), Some("A"));
        assert_eq!(p.resolved_glyph_name(1), Some("B"));
        assert_eq!(p.resolved_glyph_name(2), Some("C"));
    }

    #[test]
    fn v25_negative_offset_below_zero_yields_none() {
        // gid 0 with offset -1 would map to standard index -1; reject.
        let num_glyphs: u16 = 1;
        let mut tail = Vec::new();
        tail.extend_from_slice(&num_glyphs.to_be_bytes());
        tail.push((-1i8) as u8);
        let mut bytes = header(POST_VERSION_25);
        bytes.extend_from_slice(&tail);
        let p = PostTable::parse(&bytes).unwrap();
        assert!(p.glyph_name_ref(0).is_none());
    }

    #[test]
    fn v25_offset_past_standard_set_yields_none() {
        // gid 0 with offset 257 maps to standard index 257 → still
        // legal. gid 0 with offset 127 + gid 250 with offset 127 maps
        // to 377 → out of range.
        let num_glyphs: u16 = 251;
        let mut tail = Vec::new();
        tail.extend_from_slice(&num_glyphs.to_be_bytes());
        for _ in 0..num_glyphs {
            tail.push(127i8 as u8);
        }
        let mut bytes = header(POST_VERSION_25);
        bytes.extend_from_slice(&tail);
        let p = PostTable::parse(&bytes).unwrap();
        // gid 0 + 127 = 127 (in range).
        assert_eq!(
            p.glyph_name_ref(0),
            Some(GlyphNameRef::StandardMac { index: 127 })
        );
        // gid 250 + 127 = 377 (out of [0,258)).
        assert!(p.glyph_name_ref(250).is_none());
    }

    #[test]
    fn standard_mac_glyph_names_table_is_well_formed() {
        // Exactly 258 entries, all distinct (the count is keyed off the
        // shared STANDARD_MAC_GLYPH_COUNT constant).
        assert_eq!(STANDARD_MAC_GLYPH_NAMES.len(), 258);
        assert_eq!(
            STANDARD_MAC_GLYPH_NAMES.len(),
            STANDARD_MAC_GLYPH_COUNT as usize
        );
        let mut sorted: Vec<&str> = STANDARD_MAC_GLYPH_NAMES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 258, "all 258 names must be distinct");
    }

    #[test]
    fn standard_mac_glyph_name_spot_checks() {
        // Anchors from the staged ordering doc.
        assert_eq!(standard_mac_glyph_name(0), Some(".notdef"));
        assert_eq!(standard_mac_glyph_name(1), Some(".null"));
        assert_eq!(standard_mac_glyph_name(2), Some("nonmarkingreturn"));
        assert_eq!(standard_mac_glyph_name(3), Some("space"));
        assert_eq!(standard_mac_glyph_name(36), Some("A"));
        assert_eq!(standard_mac_glyph_name(192), Some("fi"));
        assert_eq!(standard_mac_glyph_name(193), Some("fl"));
        assert_eq!(standard_mac_glyph_name(217), Some("tilde"));
        assert_eq!(standard_mac_glyph_name(257), Some("dcroat"));
        // Out of the standard set.
        assert_eq!(standard_mac_glyph_name(258), None);
        assert_eq!(standard_mac_glyph_name(u16::MAX), None);
    }

    #[test]
    fn rejects_unknown_version() {
        // Apple v4.0 is "not supported in OpenType" per §5.2.10.
        let b = header(0x0004_0000);
        assert!(matches!(PostTable::parse(&b), Err(Error::BadStructure(_))));
    }

    #[test]
    fn rejects_short_header() {
        let b = vec![0u8; 31];
        assert!(matches!(PostTable::parse(&b), Err(Error::UnexpectedEof)));
    }

    #[test]
    fn v20_truncated_index_array_rejected() {
        // numGlyphs=2 but only 2 bytes of index data (need 4).
        let mut tail = Vec::new();
        tail.extend_from_slice(&2u16.to_be_bytes());
        tail.extend_from_slice(&0u16.to_be_bytes()); // only one entry
        let mut bytes = header(POST_VERSION_20);
        bytes.extend_from_slice(&tail);
        assert!(matches!(
            PostTable::parse(&bytes),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn v20_pascal_index_out_of_pool_decodes_glyph_as_none() {
        // glyphNameIndex == 258 but no Pascal strings present.
        let num_glyphs: u16 = 1;
        let mut tail = Vec::new();
        tail.extend_from_slice(&num_glyphs.to_be_bytes());
        tail.extend_from_slice(&258u16.to_be_bytes());
        // No string-data bytes.
        let mut bytes = header(POST_VERSION_20);
        bytes.extend_from_slice(&tail);
        let p = PostTable::parse(&bytes).unwrap();
        assert!(p.glyph_name_ref(0).is_none());
    }

    #[test]
    fn v10_reverse_lookup_spans_full_standard_set() {
        let b = header(POST_VERSION_10);
        let p = PostTable::parse(&b).unwrap();
        assert_eq!(p.named_glyph_count(), 258);
        // Every standard name resolves back to its index for v1.0.
        assert_eq!(p.gid_for_name(".notdef"), Some(0));
        assert_eq!(p.gid_for_name("A"), Some(36));
        assert_eq!(p.gid_for_name("tilde"), Some(217));
        assert_eq!(p.gid_for_name("dcroat"), Some(257));
        // A name not in the standard set has no glyph.
        assert_eq!(p.gid_for_name("Alpha"), None);
        assert_eq!(p.gid_for_name(""), None);
    }

    #[test]
    fn v20_reverse_lookup_covers_custom_and_standard() {
        let num_glyphs: u16 = 4;
        let mut tail = Vec::new();
        tail.extend_from_slice(&num_glyphs.to_be_bytes());
        // gid 0 -> standard 0 (.notdef), gid 1 -> standard 36 (A),
        // gid 2 -> custom "Alpha", gid 3 -> custom "Beta".
        for idx in [0u16, 36, 258, 259] {
            tail.extend_from_slice(&idx.to_be_bytes());
        }
        for name in ["Alpha", "Beta"] {
            tail.push(name.len() as u8);
            tail.extend_from_slice(name.as_bytes());
        }
        let mut bytes = header(POST_VERSION_20);
        bytes.extend_from_slice(&tail);
        let p = PostTable::parse(&bytes).unwrap();
        assert_eq!(p.named_glyph_count(), 4);
        assert_eq!(p.gid_for_name(".notdef"), Some(0));
        assert_eq!(p.gid_for_name("A"), Some(1));
        assert_eq!(p.gid_for_name("Alpha"), Some(2));
        assert_eq!(p.gid_for_name("Beta"), Some(3));
        assert_eq!(p.gid_for_name("missing"), None);
    }

    #[test]
    fn reverse_lookup_returns_lowest_gid_on_duplicate() {
        // Two glyphs both named "A" (standard index 36).
        let num_glyphs: u16 = 3;
        let mut tail = Vec::new();
        tail.extend_from_slice(&num_glyphs.to_be_bytes());
        for idx in [0u16, 36, 36] {
            tail.extend_from_slice(&idx.to_be_bytes());
        }
        let mut bytes = header(POST_VERSION_20);
        bytes.extend_from_slice(&tail);
        let p = PostTable::parse(&bytes).unwrap();
        // Lowest matching gid (1) wins over gid 2.
        assert_eq!(p.gid_for_name("A"), Some(1));
    }

    #[test]
    fn v25_reverse_lookup_inverts_offset() {
        // §5.2.10.3 worked example: gids 0,1,2 -> standard 36,37,38.
        let num_glyphs: u16 = 3;
        let mut tail = Vec::new();
        tail.extend_from_slice(&num_glyphs.to_be_bytes());
        for _ in 0..3 {
            tail.push(36i8 as u8);
        }
        let mut bytes = header(POST_VERSION_25);
        bytes.extend_from_slice(&tail);
        let p = PostTable::parse(&bytes).unwrap();
        assert_eq!(p.named_glyph_count(), 3);
        assert_eq!(p.gid_for_name("A"), Some(0));
        assert_eq!(p.gid_for_name("B"), Some(1));
        assert_eq!(p.gid_for_name("C"), Some(2));
        // "D" is standard index 39 but no glyph maps to it here.
        assert_eq!(p.gid_for_name("D"), None);
    }

    #[test]
    fn v30_reverse_lookup_and_iter_are_empty() {
        let b = header(POST_VERSION_30);
        let p = PostTable::parse(&b).unwrap();
        assert_eq!(p.named_glyph_count(), 0);
        assert_eq!(p.gid_for_name(".notdef"), None);
        assert_eq!(p.iter_glyph_names().count(), 0);
    }

    #[test]
    fn iter_glyph_names_round_trips_through_reverse_lookup() {
        let num_glyphs: u16 = 4;
        let mut tail = Vec::new();
        tail.extend_from_slice(&num_glyphs.to_be_bytes());
        for idx in [0u16, 36, 258, 259] {
            tail.extend_from_slice(&idx.to_be_bytes());
        }
        for name in ["Alpha", "Beta"] {
            tail.push(name.len() as u8);
            tail.extend_from_slice(name.as_bytes());
        }
        let mut bytes = header(POST_VERSION_20);
        bytes.extend_from_slice(&tail);
        let p = PostTable::parse(&bytes).unwrap();
        let pairs: Vec<(u16, &str)> = p.iter_glyph_names().collect();
        assert_eq!(
            pairs,
            vec![(0, ".notdef"), (1, "A"), (2, "Alpha"), (3, "Beta")]
        );
        // Each iterated name reverse-resolves to its own gid (these are
        // all unique names, so the lowest-gid rule is exact here).
        for (gid, name) in pairs {
            assert_eq!(p.gid_for_name(name), Some(gid));
        }
    }

    #[test]
    fn v20_iter_skips_unsatisfiable_pascal_reference() {
        // gid 0 names .notdef; gid 1 references a Pascal string that the
        // empty pool cannot satisfy -> skipped by iter, no reverse hit.
        let num_glyphs: u16 = 2;
        let mut tail = Vec::new();
        tail.extend_from_slice(&num_glyphs.to_be_bytes());
        tail.extend_from_slice(&0u16.to_be_bytes());
        tail.extend_from_slice(&258u16.to_be_bytes());
        let mut bytes = header(POST_VERSION_20);
        bytes.extend_from_slice(&tail);
        let p = PostTable::parse(&bytes).unwrap();
        let pairs: Vec<(u16, &str)> = p.iter_glyph_names().collect();
        assert_eq!(pairs, vec![(0, ".notdef")]);
    }

    #[test]
    fn version_constants_sanity() {
        assert_eq!(POST_VERSION_10, 0x0001_0000);
        assert_eq!(POST_VERSION_20, 0x0002_0000);
        assert_eq!(POST_VERSION_25, 0x0002_5000);
        assert_eq!(POST_VERSION_30, 0x0003_0000);
        assert_eq!(STANDARD_MAC_GLYPH_COUNT, 258);
        assert_eq!(POST_HEADER_LEN, 32);
    }
}
