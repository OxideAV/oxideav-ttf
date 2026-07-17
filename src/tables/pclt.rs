//! `PCLT` — PCL 5 table.
//!
//! Spec: ISO/IEC 14496-22:2019 §5.7.7 ("PCLT – PCL 5 table"). This
//! optional table carries the font-selection attributes a PCL 5
//! printer uses to pick and report a font: the HP-assigned font
//! number, the PCL pitch / x-height / cap-height metrics, the packed
//! style / type-family / symbol-set words, the 16-byte typeface
//! "font print" string, the 8-byte character-complement bitfield,
//! the 6-byte PCL file name, and the PCL stroke-weight / width-type
//! / serif-style classification bytes.
//!
//! §5.7.7's opening sentence notes the table "is strongly discouraged
//! for OFF fonts with TrueType outlines" — it survives in legacy
//! fonts, so the parser decodes it whenever it is present and leaves
//! the deprecation policy to the caller.
//!
//! ## Layout (§5.7.7)
//!
//! ```text
//! uint16 majorVersion
//! uint16 minorVersion
//! uint32 FontNumber
//! uint16 Pitch
//! uint16 xHeight
//! uint16 Style
//! uint16 TypeFamily
//! uint16 CapHeight
//! uint16 SymbolSet
//! int8   Typeface[16]
//! int8   CharacterComplement[8]
//! int8   FileName[6]
//! int8   StrokeWeight
//! int8   WidthType
//! uint8  SerifStyle
//! uint8  Reserved (pad)
//! ```
//!
//! Total on-wire length is therefore a fixed 54 bytes
//! ([`PCLT_TABLE_LEN`]).
//!
//! ## Packed-field semantics (§5.7.7)
//!
//! - **FontNumber** — "segmented in two parts": the most significant
//!   bit indicates native versus converted format ("Only font vendors
//!   should create fonts with this bit zeroed"), the 7 next most
//!   significant bits are the HP-assigned vendor code (published as
//!   ASCII letters, e.g. `B` = Bitstream Inc., `M` = Monotype
//!   Typography Ltd.), and the least significant 24 bits are assigned
//!   by the vendor.
//! - **Style** — most significant 6 bits reserved; bits 5–9 encode
//!   *structure* (0 = solid … 17 = inverse with border, 18–31
//!   reserved); bits 2–4 encode *appearance width* (0 = normal,
//!   1 = condensed, … 7 = extra expanded); bits 0–1 encode *posture*
//!   (0 = upright, 1 = oblique/italic, 2 = alternate italic,
//!   3 = reserved).
//! - **TypeFamily** — bits 12–15 are the HP Boise Division vendor
//!   code (1 = Agfa, 2 = Bitstream, 3 = Linotype, 4 = Monotype,
//!   5 = Adobe, 6 = font repackagers, 7 = vendors of unique
//!   typefaces; 0 and 8–15 reserved); bits 0–11 are the typeface
//!   family code.
//! - **SymbolSet** — the most significant 11 bits are the symbol-set
//!   "number" field; the least significant 5 bits, "when added to
//!   64", give the ASCII value of the symbol-set "ID" letter (e.g.
//!   PCL `19U` = decimal 629: 629 >> 5 = 19, (629 & 31) + 64 = 'U').
//!   "Unbound fonts, or 'typefaces' should have a symbol set value
//!   of 0."
//! - **CharacterComplement** — an 8-byte bitfield where "each bit
//!   identifies a symbol collection and is independently
//!   interpreted"; the named collections sit at bits 31–22 (31 =
//!   ASCII, 30 = Latin 1 extensions, … 22 = Code Page Extensions).
//!   The spec's worked examples (e.g. Windows 3.1 "ANSI" =
//!   `0xFFFFFFFF37FFFFFE`) and the rule "Symbol set bound fonts
//!   should have this field set to all F's (except bit 0)" show
//!   that a *cleared* bit marks a provided collection. "Bit 0 must
//!   always be cleared when the font elements are provided in
//!   Unicode order."
//! - **FileName** — 3-byte industry-standard typeface family string,
//!   then one treatment character (`R` text, `I` italic, `B` bold,
//!   `J` bold italic, …), then two characters that are "either
//!   zeroes for an unbound font or a two character mnemonic for a
//!   symbol set".
//! - **StrokeWeight** — signed PCL stroke weight; "Only values in
//!   the range -7 to 7 are valid" (-7 = Ultra Thin, 0 = Book/text/
//!   regular, 3 = Bold, 7 = Ultra Black).
//! - **WidthType** — signed PCL appearance width; "Only values in
//!   the range -5 to 5 are valid" (-5 = Ultra Compressed, 0 =
//!   Normal, 2 = Expanded, 3 = Extra Expanded). §5.7.7 notes these
//!   values "are not directly related to those in the appearance
//!   width field of the style word above."
//! - **SerifStyle** — bottom 6 bits are the PCL serif-style value
//!   (0 = Sans Serif Square … 12 = Script Broken Letter); top 2
//!   bits classify the face (1 = Sans Serif/Monoline, 2 = Serif/
//!   Contrasting, 0 and 3 reserved).
//! - **Reserved** — "Should be set to zero." Surfaced, not
//!   validated: the field is a pad byte and a non-zero value does
//!   not change any other field's meaning.
//!
//! ## Versioning (§5.7.7)
//!
//! "The current PCLT table version is 1.0." The parser enforces
//! `majorVersion == 1` and surfaces `minorVersion` raw, following
//! the OpenType convention that minor-version bumps stay layout-
//! compatible.

use crate::parser::{read_i8, read_u16, read_u32, read_u8};
use crate::Error;

/// On-wire table tag (`b"PCLT"`, big-endian `0x50434C54`). Exposed for
/// callers that walk the table directory directly.
pub const PCLT_TABLE_TAG: u32 = 0x5043_4C54;

/// Fixed on-wire length of the `PCLT` table (§5.7.7 layout): 20 bytes
/// of version + number + metric/word fields, 16 + 8 + 6 bytes of
/// fixed-size strings, and 4 trailing classification/pad bytes.
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub const PCLT_TABLE_LEN: usize = 54;

/// The only spec-defined major version (`1`); §5.7.7: "The current
/// PCLT table version is 1.0."
pub const PCLT_MAJOR_VERSION: u16 = 1;

/// §5.7.7 StrokeWeight validity range: "Only values in the range -7
/// to 7 are valid."
pub const PCLT_STROKE_WEIGHT_RANGE: core::ops::RangeInclusive<i8> = -7..=7;

/// §5.7.7 WidthType validity range: "Only values in the range -5 to
/// 5 are valid."
pub const PCLT_WIDTH_TYPE_RANGE: core::ops::RangeInclusive<i8> = -5..=5;

/// Parsed `PCLT` table (§5.7.7). All fields are fixed-size and copied
/// out of the on-wire slice, so the struct carries no lifetime.
#[derive(Debug, Clone)]
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub struct PcltTable {
    major_version: u16,
    minor_version: u16,
    font_number: u32,
    pitch: u16,
    x_height: u16,
    style: u16,
    type_family: u16,
    cap_height: u16,
    symbol_set: u16,
    typeface: [u8; 16],
    character_complement: [u8; 8],
    file_name: [u8; 6],
    stroke_weight: i8,
    width_type: i8,
    serif_style: u8,
    reserved: u8,
}

impl PcltTable {
    /// Parse a `PCLT` table from its raw slice.
    ///
    /// The §5.7.7 layout is a fixed 54 bytes; a shorter slice is
    /// `UnexpectedEof` and trailing bytes (sfnt 4-byte record padding)
    /// are tolerated. `majorVersion != 1` is rejected as
    /// `BadStructure` per §5.7.7's "The current PCLT table version is
    /// 1.0"; `minorVersion` is surfaced raw.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < PCLT_TABLE_LEN {
            return Err(Error::UnexpectedEof);
        }
        let major_version = read_u16(bytes, 0)?;
        if major_version != PCLT_MAJOR_VERSION {
            return Err(Error::BadStructure("PCLT: unrecognised majorVersion"));
        }
        let minor_version = read_u16(bytes, 2)?;
        let font_number = read_u32(bytes, 4)?;
        let pitch = read_u16(bytes, 8)?;
        let x_height = read_u16(bytes, 10)?;
        let style = read_u16(bytes, 12)?;
        let type_family = read_u16(bytes, 14)?;
        let cap_height = read_u16(bytes, 16)?;
        let symbol_set = read_u16(bytes, 18)?;
        let mut typeface = [0u8; 16];
        typeface.copy_from_slice(&bytes[20..36]);
        let mut character_complement = [0u8; 8];
        character_complement.copy_from_slice(&bytes[36..44]);
        let mut file_name = [0u8; 6];
        file_name.copy_from_slice(&bytes[44..50]);
        let stroke_weight = read_i8(bytes, 50)?;
        let width_type = read_i8(bytes, 51)?;
        let serif_style = read_u8(bytes, 52)?;
        let reserved = read_u8(bytes, 53)?;
        Ok(Self {
            major_version,
            minor_version,
            font_number,
            pitch,
            x_height,
            style,
            type_family,
            cap_height,
            symbol_set,
            typeface,
            character_complement,
            file_name,
            stroke_weight,
            width_type,
            serif_style,
            reserved,
        })
    }

    // ---- version -----------------------------------------------------------

    /// `majorVersion` field — always [`PCLT_MAJOR_VERSION`] (`1`) after
    /// a successful parse.
    pub fn major_version(&self) -> u16 {
        self.major_version
    }

    /// `minorVersion` field — `0` for the §5.7.7 "current" 1.0 layout.
    /// Surfaced raw so a layout-compatible minor bump still decodes.
    pub fn minor_version(&self) -> u16 {
        self.minor_version
    }

    // ---- FontNumber --------------------------------------------------------

    /// Raw 32-bit `FontNumber` field.
    pub fn font_number(&self) -> u32 {
        self.font_number
    }

    /// `true` when the FontNumber's most significant bit is **zero** —
    /// §5.7.7: "The most significant bit indicates native versus
    /// converted format. Only font vendors should create fonts with
    /// this bit zeroed."
    pub fn font_number_is_native(&self) -> bool {
        self.font_number & 0x8000_0000 == 0
    }

    /// The HP-assigned vendor code — the 7 bits below the
    /// native/converted flag (§5.7.7: "The 7 next most significant
    /// bits are assigned by Hewlett-Packard Boise Printer Division to
    /// major font vendors"). The published assignments are ASCII
    /// letters: `A` = Adobe Systems, `B` = Bitstream Inc., `C` = Agfa
    /// Corporation, `H` = Bigelow & Holmes, `L` = Linotype Company,
    /// `M` = Monotype Typography Ltd.
    pub fn font_number_vendor_code(&self) -> u8 {
        ((self.font_number >> 24) & 0x7F) as u8
    }

    /// The vendor-assigned identifier — the least significant 24 bits
    /// (§5.7.7: "The least significant 24 bits are assigned by the
    /// vendor").
    pub fn font_number_vendor_assigned(&self) -> u32 {
        self.font_number & 0x00FF_FFFF
    }

    // ---- metrics -----------------------------------------------------------

    /// `Pitch` — "the width of the space in font design units"
    /// (§5.7.7; design units are described by `head.unitsPerEm`).
    /// "Monospace fonts derive the width of all characters from this
    /// field."
    pub fn pitch(&self) -> u16 {
        self.pitch
    }

    /// `xHeight` — "the height of the optical line describing the
    /// height of the lowercase x in font design units" (§5.7.7; the
    /// spec notes this "might not be the same as the measured height
    /// of the lowercase x").
    pub fn x_height(&self) -> u16 {
        self.x_height
    }

    /// `CapHeight` — "the height of the optical line describing the
    /// top of the uppercase H in font design units" (§5.7.7).
    pub fn cap_height(&self) -> u16 {
        self.cap_height
    }

    // ---- Style word --------------------------------------------------------

    /// Raw 16-bit `Style` word.
    pub fn style(&self) -> u16 {
        self.style
    }

    /// Style *posture* — the 2 least significant bits of the Style
    /// word (§5.7.7): 0 = upright, 1 = oblique/italic, 2 = alternate
    /// italic (backslanted, cursive, swash), 3 = reserved.
    pub fn style_posture(&self) -> u8 {
        (self.style & 0x0003) as u8
    }

    /// Style *appearance width* — bits 2–4 of the Style word
    /// (§5.7.7): 0 = normal, 1 = condensed, 2 = compressed / extra
    /// condensed, 3 = extra compressed, 4 = ultra compressed, 5 =
    /// reserved, 6 = expanded / extended, 7 = extra expanded / extra
    /// extended.
    pub fn style_width(&self) -> u8 {
        ((self.style >> 2) & 0x0007) as u8
    }

    /// Style *structure* — bits 5–9 of the Style word (§5.7.7): 0 =
    /// solid (normal, black), 1 = outline, 2 = inline, 3 = contour /
    /// edged, 4–7 = the same four with shadow, 8–15 = pattern-filled
    /// variants, 16 = inverse, 17 = inverse with border, 18–31 =
    /// reserved.
    pub fn style_structure(&self) -> u8 {
        ((self.style >> 5) & 0x001F) as u8
    }

    /// The reserved top 6 bits of the Style word (§5.7.7: "The most
    /// significant 6 bits are reserved"). Surfaced for tooling.
    pub fn style_reserved_bits(&self) -> u8 {
        (self.style >> 10) as u8
    }

    // ---- TypeFamily word ---------------------------------------------------

    /// Raw 16-bit `TypeFamily` word.
    pub fn type_family(&self) -> u16 {
        self.type_family
    }

    /// TypeFamily *vendor code* — the 4 most significant bits
    /// (§5.7.7): 1 = Agfa Corporation, 2 = Bitstream Inc., 3 =
    /// Linotype Company, 4 = Monotype Typography Ltd., 5 = Adobe
    /// Systems, 6 = font repackagers, 7 = vendors of unique
    /// typefaces; 0 and 8–15 reserved.
    pub fn type_family_vendor_code(&self) -> u8 {
        (self.type_family >> 12) as u8
    }

    /// TypeFamily *typeface family code* — the 12 least significant
    /// bits, "assigned by HP Boise Division" (§5.7.7).
    pub fn type_family_code(&self) -> u16 {
        self.type_family & 0x0FFF
    }

    // ---- SymbolSet word ----------------------------------------------------

    /// Raw 16-bit `SymbolSet` word. §5.7.7: unbound fonts
    /// ("typefaces") "should have a symbol set value of 0".
    pub fn symbol_set(&self) -> u16 {
        self.symbol_set
    }

    /// The symbol-set "number" field — the most significant 11 bits
    /// of the SymbolSet word (§5.7.7). E.g. PCL `19U` (decimal 629)
    /// yields 19.
    pub fn symbol_set_number(&self) -> u16 {
        self.symbol_set >> 5
    }

    /// The symbol-set "ID" letter — §5.7.7: "The value of the least
    /// significant 5 bits, when added to 64, is the ASCII value of
    /// the symbol set 'ID' field." E.g. PCL `19U` (decimal 629)
    /// yields `b'U'`.
    pub fn symbol_set_id(&self) -> u8 {
        (self.symbol_set & 0x001F) as u8 + 64
    }

    // ---- fixed-size strings ------------------------------------------------

    /// Raw 16-byte `Typeface` field — "this 16-byte ASCII string
    /// appears in the 'font print' of PCL printers" (§5.7.7).
    pub fn typeface_raw(&self) -> &[u8; 16] {
        &self.typeface
    }

    /// `Typeface` decoded as a `&str` with trailing NULs / spaces
    /// trimmed, or `None` when the field is not ASCII (§5.7.7 calls
    /// the field an ASCII string; non-conforming bytes are left to
    /// the raw accessor).
    pub fn typeface(&self) -> Option<&str> {
        trim_ascii_field(&self.typeface)
    }

    /// Raw 8-byte `CharacterComplement` field in on-wire (big-endian)
    /// order.
    pub fn character_complement_raw(&self) -> &[u8; 8] {
        &self.character_complement
    }

    /// `CharacterComplement` as a big-endian 64-bit bitfield. §5.7.7:
    /// "each bit identifies a symbol collection and is independently
    /// interpreted." The named collections sit at bits 31–22:
    ///
    /// | bit | collection |
    /// |-----|------------|
    /// | 31  | ASCII (supports several standard interpretations) |
    /// | 30  | Latin 1 extensions |
    /// | 29  | Latin 2 extensions |
    /// | 28  | Latin 5 extensions |
    /// | 27  | Desktop Publishing Extensions |
    /// | 26  | Accent Extensions (East and West Europe) |
    /// | 25  | PCL Extensions |
    /// | 24  | Macintosh Extensions |
    /// | 23  | PostScript Extensions |
    /// | 22  | Code Page Extensions |
    pub fn character_complement(&self) -> u64 {
        u64::from_be_bytes(self.character_complement)
    }

    /// `true` when the collection at `bit` is **provided** by the
    /// font, i.e. the bit is *cleared*. The polarity follows the
    /// §5.7.7 examples (Windows 3.1 "ANSI" = `0xFFFFFFFF37FFFFFE`
    /// clears bits 31/30/27 for its ASCII + Latin 1 + DTP coverage)
    /// and the rule "Symbol set bound fonts should have this field
    /// set to all F's (except bit 0)" — all-ones marks a font that
    /// declares no collections. Returns `false` for `bit >= 64`.
    pub fn provides_collection(&self, bit: u8) -> bool {
        bit < 64 && self.character_complement() & (1u64 << bit) == 0
    }

    /// `true` when bit 0 of the character complement is cleared —
    /// §5.7.7: "Bit 0 must always be cleared when the font elements
    /// are provided in Unicode order."
    pub fn is_unicode_indexed(&self) -> bool {
        self.character_complement() & 1 == 0
    }

    /// Raw 6-byte `FileName` field — "composed of 3 parts. The first
    /// 3 bytes are an industry standard typeface family string. The
    /// fourth byte is a treatment character, such as R, B, I. The
    /// last two characters are either zeroes for an unbound font or
    /// a two character mnemonic for a symbol set" (§5.7.7).
    pub fn file_name_raw(&self) -> &[u8; 6] {
        &self.file_name
    }

    /// `FileName` decoded as a `&str` with trailing NULs / spaces
    /// trimmed, or `None` when not ASCII.
    pub fn file_name(&self) -> Option<&str> {
        trim_ascii_field(&self.file_name)
    }

    /// The treatment character — the fourth `FileName` byte (§5.7.7
    /// treatment flags: `R` text/normal/book, `I` italic, `B` bold,
    /// `J` bold italic, `D` demibold, `K` black, `L` light, `C`
    /// condensed, `S` semibold, … — "other treatment flags are
    /// assigned over time").
    pub fn file_name_treatment(&self) -> u8 {
        self.file_name[3]
    }

    // ---- classification bytes ----------------------------------------------

    /// `StrokeWeight` — the signed PCL stroke-weight value (§5.7.7:
    /// -7 = Ultra Thin, -3 = Light, 0 = Book/text/regular, 1 =
    /// Semibold, 3 = Bold, 5 = Black, 7 = Ultra Black/Ultra).
    pub fn stroke_weight(&self) -> i8 {
        self.stroke_weight
    }

    /// `true` when [`Self::stroke_weight`] is inside the §5.7.7
    /// validity range ("Only values in the range -7 to 7 are valid").
    pub fn stroke_weight_is_valid(&self) -> bool {
        PCLT_STROKE_WEIGHT_RANGE.contains(&self.stroke_weight)
    }

    /// `WidthType` — the signed PCL appearance-width value (§5.7.7:
    /// -5 = Ultra Compressed, -2 = Condensed, 0 = Normal, 2 =
    /// Expanded, 3 = Extra Expanded). The spec notes these values
    /// "are not directly related to" the Style word's width bits.
    pub fn width_type(&self) -> i8 {
        self.width_type
    }

    /// `true` when [`Self::width_type`] is inside the §5.7.7 validity
    /// range ("Only values in the range -5 to 5 are valid").
    pub fn width_type_is_valid(&self) -> bool {
        PCLT_WIDTH_TYPE_RANGE.contains(&self.width_type)
    }

    /// Raw `SerifStyle` byte.
    pub fn serif_style(&self) -> u8 {
        self.serif_style
    }

    /// The PCL serif-style value — the bottom 6 bits of `SerifStyle`
    /// (§5.7.7): 0 = Sans Serif Square, 1 = Sans Serif Round, 2 =
    /// Serif Line, 3 = Serif Triangle, 4 = Serif Swath, 5 = Serif
    /// Block, 6 = Serif Bracket, 7 = Rounded Bracket, 8 = Flair
    /// Serif / Modified Sans, 9 = Script Nonconnecting, 10 = Script
    /// Joining, 11 = Script Calligraphic, 12 = Script Broken Letter.
    pub fn serif_style_value(&self) -> u8 {
        self.serif_style & 0x3F
    }

    /// The serif/contrast classification — the top 2 bits of
    /// `SerifStyle` (§5.7.7): 0 = reserved, 1 = Sans Serif/Monoline,
    /// 2 = Serif/Contrasting, 3 = reserved.
    pub fn serif_style_class(&self) -> u8 {
        self.serif_style >> 6
    }

    /// The trailing `Reserved` pad byte — §5.7.7: "Should be set to
    /// zero." Surfaced rather than validated.
    pub fn reserved(&self) -> u8 {
        self.reserved
    }
}

/// Trim trailing NUL and space bytes from a fixed-size §5.7.7 ASCII
/// field and decode the remainder as `&str`; `None` when any
/// remaining byte is outside printable ASCII.
fn trim_ascii_field(field: &[u8]) -> Option<&str> {
    let mut end = field.len();
    while end > 0 && (field[end - 1] == 0 || field[end - 1] == b' ') {
        end -= 1;
    }
    let trimmed = &field[..end];
    if trimmed.iter().all(|&b| (0x20..0x7F).contains(&b)) {
        // ASCII subset of UTF-8 — from_utf8 cannot fail here.
        core::str::from_utf8(trimmed).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a wire-format `PCLT` table. Fields default to the §5.7.7
    /// example shapes and are overridden per test.
    struct PcltBuilder {
        major: u16,
        minor: u16,
        font_number: u32,
        pitch: u16,
        x_height: u16,
        style: u16,
        type_family: u16,
        cap_height: u16,
        symbol_set: u16,
        typeface: [u8; 16],
        complement: [u8; 8],
        file_name: [u8; 6],
        stroke_weight: i8,
        width_type: i8,
        serif_style: u8,
        reserved: u8,
    }

    impl PcltBuilder {
        fn new() -> Self {
            let mut typeface = [b' '; 16];
            typeface[..9].copy_from_slice(b"Times New");
            Self {
                major: PCLT_MAJOR_VERSION,
                minor: 0,
                // Native (bit 31 clear), vendor 'B', vendor-assigned 5.
                font_number: (u32::from(b'B') << 24) | 5,
                pitch: 569,
                x_height: 1062,
                style: 0,
                type_family: (2 << 12) | 517,
                cap_height: 1466,
                // PCL 19U = decimal 629 (§5.7.7 example).
                symbol_set: 629,
                typeface,
                // §5.7.7 example: Windows 3.1 "ANSI".
                complement: 0xFFFF_FFFF_37FF_FFFEu64.to_be_bytes(),
                file_name: *b"TNRR00",
                stroke_weight: 0,
                width_type: 0,
                serif_style: (2 << 6) | 6,
                reserved: 0,
            }
        }

        fn build(&self) -> Vec<u8> {
            let mut b = Vec::with_capacity(PCLT_TABLE_LEN);
            b.extend_from_slice(&self.major.to_be_bytes());
            b.extend_from_slice(&self.minor.to_be_bytes());
            b.extend_from_slice(&self.font_number.to_be_bytes());
            b.extend_from_slice(&self.pitch.to_be_bytes());
            b.extend_from_slice(&self.x_height.to_be_bytes());
            b.extend_from_slice(&self.style.to_be_bytes());
            b.extend_from_slice(&self.type_family.to_be_bytes());
            b.extend_from_slice(&self.cap_height.to_be_bytes());
            b.extend_from_slice(&self.symbol_set.to_be_bytes());
            b.extend_from_slice(&self.typeface);
            b.extend_from_slice(&self.complement);
            b.extend_from_slice(&self.file_name);
            b.push(self.stroke_weight as u8);
            b.push(self.width_type as u8);
            b.push(self.serif_style);
            b.push(self.reserved);
            assert_eq!(b.len(), PCLT_TABLE_LEN);
            b
        }
    }

    #[test]
    fn parses_baseline_table() {
        let bytes = PcltBuilder::new().build();
        let t = PcltTable::parse(&bytes).expect("parse");
        assert_eq!(t.major_version(), 1);
        assert_eq!(t.minor_version(), 0);
        assert_eq!(t.pitch(), 569);
        assert_eq!(t.x_height(), 1062);
        assert_eq!(t.cap_height(), 1466);
        assert_eq!(t.reserved(), 0);
    }

    #[test]
    fn font_number_segments_decode() {
        // §5.7.7: MSB = native/converted, next 7 bits = vendor code,
        // low 24 bits = vendor-assigned.
        let mut b = PcltBuilder::new();
        b.font_number = (u32::from(b'M') << 24) | 0x00AB_CDEF;
        let t = PcltTable::parse(&b.build()).expect("parse");
        assert!(t.font_number_is_native());
        assert_eq!(t.font_number_vendor_code(), b'M');
        assert_eq!(t.font_number_vendor_assigned(), 0x00AB_CDEF);

        // Converted-format flag set (bit 31).
        b.font_number |= 0x8000_0000;
        let t = PcltTable::parse(&b.build()).expect("parse");
        assert!(!t.font_number_is_native());
        // Vendor code is the 7 bits below the flag — unchanged.
        assert_eq!(t.font_number_vendor_code(), b'M' & 0x7F);
    }

    #[test]
    fn style_word_bitfields_decode() {
        // structure = 4 (solid with shadow), width = 1 (condensed),
        // posture = 2 (alternate italic), per the §5.7.7 bit layout
        // (structure bits 5-9, width bits 2-4, posture bits 0-1).
        let mut b = PcltBuilder::new();
        b.style = (4 << 5) | (1 << 2) | 2;
        let t = PcltTable::parse(&b.build()).expect("parse");
        assert_eq!(t.style_structure(), 4);
        assert_eq!(t.style_width(), 1);
        assert_eq!(t.style_posture(), 2);
        assert_eq!(t.style_reserved_bits(), 0);

        // Reserved top 6 bits are surfaced, not rejected.
        b.style |= 0b101010 << 10;
        let t = PcltTable::parse(&b.build()).expect("parse");
        assert_eq!(t.style_reserved_bits(), 0b101010);
        assert_eq!(t.style_structure(), 4);
    }

    #[test]
    fn type_family_vendor_and_code_decode() {
        // Vendor 5 = Adobe Systems per §5.7.7; family code 0x123.
        let mut b = PcltBuilder::new();
        b.type_family = (5 << 12) | 0x123;
        let t = PcltTable::parse(&b.build()).expect("parse");
        assert_eq!(t.type_family_vendor_code(), 5);
        assert_eq!(t.type_family_code(), 0x123);
    }

    #[test]
    fn symbol_set_examples_from_spec_decode() {
        // §5.7.7 example table: (PCL id, decimal value).
        let cases: &[(u16, u16, u8)] = &[
            (629, 19, b'U'), // Windows 3.1 "ANSI" (19U)
            (309, 9, b'U'),  // Windows 3.0 "ANSI" (9U)
            (621, 19, b'M'), // Adobe "Symbol" (19M)
            (394, 12, b'J'), // Macintosh (12J)
            (362, 11, b'J'), // PostScript ISO Latin 1 (11J)
            (330, 10, b'J'), // PostScript Std. Encoding (10J)
            (298, 9, b'J'),  // Code Page 1004 (9J)
            (234, 7, b'J'),  // DeskTop (7J)
        ];
        for &(decimal, number, id) in cases {
            let mut b = PcltBuilder::new();
            b.symbol_set = decimal;
            let t = PcltTable::parse(&b.build()).expect("parse");
            assert_eq!(t.symbol_set_number(), number, "decimal {decimal}");
            assert_eq!(t.symbol_set_id(), id, "decimal {decimal}");
        }
        // Unbound fonts "should have a symbol set value of 0".
        let mut b = PcltBuilder::new();
        b.symbol_set = 0;
        let t = PcltTable::parse(&b.build()).expect("parse");
        assert_eq!(t.symbol_set(), 0);
    }

    #[test]
    fn typeface_string_trims_trailing_pad() {
        let bytes = PcltBuilder::new().build();
        let t = PcltTable::parse(&bytes).expect("parse");
        assert_eq!(t.typeface(), Some("Times New"));
        assert_eq!(t.typeface_raw().len(), 16);

        // §5.7.7 example "Times New  Bd" keeps inner spacing.
        let mut b = PcltBuilder::new();
        b.typeface = *b"Times New  Bd   ";
        let t = PcltTable::parse(&b.build()).expect("parse");
        assert_eq!(t.typeface(), Some("Times New  Bd"));

        // Non-ASCII bytes decode to None; raw stays available.
        let mut b = PcltBuilder::new();
        b.typeface[0] = 0xC0;
        let t = PcltTable::parse(&b.build()).expect("parse");
        assert_eq!(t.typeface(), None);
        assert_eq!(t.typeface_raw()[0], 0xC0);
    }

    #[test]
    fn character_complement_examples_from_spec() {
        // Windows 3.1 "ANSI" (0xFFFFFFFF37FFFFFE): clears bits
        // 31 (ASCII), 30 (Latin 1 ext), 27 (DTP ext), and bit 0
        // (Unicode index order).
        let t = PcltTable::parse(&PcltBuilder::new().build()).expect("parse");
        assert_eq!(t.character_complement(), 0xFFFF_FFFF_37FF_FFFE);
        assert!(t.provides_collection(31));
        assert!(t.provides_collection(30));
        assert!(!t.provides_collection(29)); // Latin 2 ext not provided
        assert!(!t.provides_collection(28)); // Latin 5 ext not provided
        assert!(t.provides_collection(27));
        assert!(!t.provides_collection(26));
        assert!(t.is_unicode_indexed());
        // Out-of-range bit is never "provided".
        assert!(!t.provides_collection(64));

        // Symbol-set-bound shape: "all F's (except bit 0)".
        let mut b = PcltBuilder::new();
        b.complement = 0xFFFF_FFFF_FFFF_FFFEu64.to_be_bytes();
        let t = PcltTable::parse(&b.build()).expect("parse");
        assert!(t.is_unicode_indexed());
        for bit in 22..=31 {
            assert!(!t.provides_collection(bit));
        }

        // ISO 8859-1 Latin 1 (0xFFFFFFFF3BFFFFFE): bits 31, 30, 26
        // cleared (ASCII + Latin 1 ext + Accent ext).
        let mut b = PcltBuilder::new();
        b.complement = 0xFFFF_FFFF_3BFF_FFFEu64.to_be_bytes();
        let t = PcltTable::parse(&b.build()).expect("parse");
        assert!(t.provides_collection(31));
        assert!(t.provides_collection(30));
        assert!(t.provides_collection(26));
        assert!(!t.provides_collection(27));
    }

    #[test]
    fn file_name_parts_decode() {
        // §5.7.7 example: TNRR00 = Times New, text weight, upright,
        // unbound ("00" tail).
        let t = PcltTable::parse(&PcltBuilder::new().build()).expect("parse");
        assert_eq!(t.file_name(), Some("TNRR00"));
        assert_eq!(t.file_name_treatment(), b'R');

        // TNRJ00 = Times New Bold Italic per the treatment-flag table.
        let mut b = PcltBuilder::new();
        b.file_name = *b"TNRJ00";
        let t = PcltTable::parse(&b.build()).expect("parse");
        assert_eq!(t.file_name_treatment(), b'J');
    }

    #[test]
    fn stroke_weight_and_width_type_ranges() {
        let mut b = PcltBuilder::new();
        b.stroke_weight = 3; // Bold
        b.width_type = -2; // Condensed
        let t = PcltTable::parse(&b.build()).expect("parse");
        assert_eq!(t.stroke_weight(), 3);
        assert!(t.stroke_weight_is_valid());
        assert_eq!(t.width_type(), -2);
        assert!(t.width_type_is_valid());

        // Out-of-range values parse (the table is data, not law) but
        // flag as invalid per the §5.7.7 validity sentences.
        b.stroke_weight = -8;
        b.width_type = 6;
        let t = PcltTable::parse(&b.build()).expect("parse");
        assert!(!t.stroke_weight_is_valid());
        assert!(!t.width_type_is_valid());
    }

    #[test]
    fn serif_style_bitfields_decode() {
        // Builder default: class 2 (Serif/Contrasting), value 6
        // (Serif Bracket).
        let t = PcltTable::parse(&PcltBuilder::new().build()).expect("parse");
        assert_eq!(t.serif_style_class(), 2);
        assert_eq!(t.serif_style_value(), 6);

        // Sans Serif/Monoline (class 1) + Sans Serif Round (value 1).
        let mut b = PcltBuilder::new();
        b.serif_style = (1 << 6) | 1;
        let t = PcltTable::parse(&b.build()).expect("parse");
        assert_eq!(t.serif_style_class(), 1);
        assert_eq!(t.serif_style_value(), 1);
    }

    #[test]
    fn rejects_unknown_major_version() {
        let mut b = PcltBuilder::new();
        b.major = 2;
        assert!(matches!(
            PcltTable::parse(&b.build()),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn minor_version_is_surfaced_not_rejected() {
        let mut b = PcltBuilder::new();
        b.minor = 1;
        let t = PcltTable::parse(&b.build()).expect("parse");
        assert_eq!(t.minor_version(), 1);
    }

    #[test]
    fn rejects_short_slice() {
        let bytes = PcltBuilder::new().build();
        for len in [0usize, 4, 20, PCLT_TABLE_LEN - 1] {
            assert!(
                matches!(PcltTable::parse(&bytes[..len]), Err(Error::UnexpectedEof)),
                "len {len}"
            );
        }
    }

    #[test]
    fn accepts_trailing_pad_bytes() {
        // sfnt table records pad to 4-byte boundaries; 54 bytes pads
        // with 2 trailing bytes. The parser ignores them.
        let mut bytes = PcltBuilder::new().build();
        bytes.extend_from_slice(&[0u8, 0u8]);
        let t = PcltTable::parse(&bytes).expect("parse");
        assert_eq!(t.pitch(), 569);
    }

    #[test]
    fn reserved_pad_byte_is_surfaced() {
        let mut b = PcltBuilder::new();
        b.reserved = 0x7E;
        let t = PcltTable::parse(&b.build()).expect("parse");
        assert_eq!(t.reserved(), 0x7E);
    }

    #[test]
    fn tag_bytes_match_constant() {
        assert_eq!(
            u32::from_be_bytes(*b"PCLT"),
            PCLT_TABLE_TAG,
            "PCLT_TABLE_TAG = 0x{:08X}",
            PCLT_TABLE_TAG
        );
    }
}
