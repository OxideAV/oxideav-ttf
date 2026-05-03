# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added — TrueType Collection (`ttcf`) support (2026-05-04)

- New `collection` module exposing `CollectionHeader::parse(bytes) ->
  Result<Self, Error>` for the leading 12-byte TTC header + per-subfont
  offset table (`'ttcf'` magic, version 1.0 or 2.0, `numFonts: u32`,
  `offsetTable[numFonts]`). The version-2-only DSIG trailer is parsed
  (only insofar as we accept the header) but not validated.
- `Font::from_collection_bytes(bytes, index) -> Result<Font<'_>, Error>`
  — convenience constructor that reads the TTC header, picks the
  `index`-th subfont, and runs the regular sfnt parse path against
  `&bytes[offset..]`. Returns `Error::SubfontOutOfRange(index)` if the
  index exceeds `numFonts`.
- `is_collection(bytes) -> bool` — quick magic-only probe so callers
  can dispatch between TTC and plain sfnt without try-then-catch.
- New error variant `Error::SubfontOutOfRange(u32)`.

### Added — GPOS LookupType 6 (mark-to-mark) (2026-05-04)

- `Font::lookup_mark_to_mark(mark1, mark2) -> Option<(i16, i16)>` —
  walks GPOS LookupType 6 (Mark-to-Mark Attachment) sub-tables for a
  `(mark1, mark2)` glyph pair where `mark1` is the previously-placed
  mark and `mark2` is the new mark to stack on top of (or below) it.
  Returns the `(dx, dy)` offset (font units, TT Y-up) to add to
  `mark2`'s pen origin so its anchor snaps onto `mark1`'s anchor for
  `mark2`'s class. Used by consumer-crate shapers to build multi-mark
  stacks (polytonic Greek `α + tonos + dialytika`, Vietnamese
  `a + circumflex + acute`).
- `GposTable::lookup_mark_to_mark` — internal walker that handles
  MarkMarkPosFormat1 directly and unwraps ExtensionPos (LookupType 9)
  transparently. Same Anchor format support as the mark-to-base path
  (formats 1, 2 and 3 accepted; device tables / anchor points
  ignored). Layout is structurally identical to MarkBasePosFormat1
  except mark2Coverage replaces baseCoverage.

### Added — GPOS LookupType 4 (mark-to-base) (2026-05-04)

- `Font::lookup_mark_to_base(base, mark) -> Option<(i16, i16)>` — walks
  GPOS LookupType 4 (Mark-to-Base Attachment) sub-tables for a `(base,
  mark)` glyph pair and returns the `(dx, dy)` offset (font units, TT
  Y-up) to add to the mark's pen origin so its anchor lands on the
  base's anchor for the mark's class. Used by consumer-crate shapers
  to position diacritics above / below their base glyph (essential
  for European Latin extended, Vietnamese, polytonic Greek).
- `Font::is_mark_glyph(glyph_id) -> bool` — convenience wrapper around
  `GdefTable::is_mark` so the consumer crate doesn't have to peek at
  GDEF directly. Returns `false` if the font has no GDEF.
- `GposTable::lookup_mark_to_base` — internal walker that handles
  MarkBasePosFormat1 directly and unwraps ExtensionPos (LookupType 9)
  transparently. Anchor formats 1, 2 and 3 are accepted; format 2's
  anchor-point and format 3's device tables are silently ignored
  (we don't run the TT bytecode and there's no LCD device map).

## [0.1.0](https://github.com/OxideAV/oxideav-ttf/compare/v0.0.1...v0.1.0) - 2026-05-03

### Other

- Delete Cargo.lock
- promote to 0.1
- drop duplicate semver_check key
- replace never-match regex with semver_check = false

## [0.0.1] - 2026-05-02

### Added

- Initial round-1 release of the pure-Rust TrueType font parser.
- sfnt header + table directory walker (`parser.rs`).
- Core OpenType tables: `head`, `hhea`, `maxp`, `cmap` (formats 0/4/6/12),
  `name`, `OS/2`, `hmtx`, `loca`, `glyf` (simple + composite), `post`.
- Layout tables: `kern` (legacy format-0 subtable), `GSUB` (LookupType 4
  ligature substitution), `GPOS` (LookupType 2 pair-adjustment / kerning),
  `GDEF` (glyph class definitions used to skip mark glyphs).
- Public glyph-lookup API: `glyph_index`, `glyph_outline`, `glyph_advance`,
  `glyph_lsb`, `glyph_bounding_box`.
- Public shaping support: `lookup_ligature`, `lookup_kerning` (GPOS first,
  legacy `kern` fallback).
- Metadata accessors: `family_name`, `full_name`, `units_per_em`,
  `ascent`, `descent`, `line_gap`, `glyph_count`, `weight_class`,
  `italic_angle`.
- DejaVu Sans Mono integration test fixture (Bitstream Vera license).

### Deferred (round 2+)

- CFF / Type 2 charstring outlines (will live in `oxideav-otf`).
- Bidi (UAX #9), Arabic shaping, Indic conjuncts.
- Variable fonts (`fvar`/`gvar`/`MVAR`).
- TrueType bytecode hinting interpreter.
- cmap formats 2, 8, 10, 13, 14.
- GSUB lookup types 1/2/3/5/6/7/8 and GPOS lookup types 1/3..9.
