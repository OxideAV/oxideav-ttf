# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
