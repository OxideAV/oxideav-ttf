# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added — GSUB LookupTypes 2 / 3 / 5 / 8 (2026-05-04)

Closes the GSUB lookup-type grid; every type 1-8 (minus the implicit
ExtensionSubst wrapper) now has a public per-lookup entry point.

- `tables::gsub::GsubTable::apply_lookup_type_2(lookup_index, gid)`
  — Multiple Substitution (Format 1). Splits one input glyph into a
  `Vec<u16>` substitute sequence per the matched `Sequence` record.
  Empty sequences (`glyphCount = 0`, deletion) are returned as
  `Some(Vec::new())`. Used by some script normalisations that expand
  a precomposed glyph into a base + mark cluster.
- `tables::gsub::GsubTable::apply_lookup_type_3(lookup_index, gid, alternate_index)`
  — Alternate Substitution (Format 1). Each covered glyph carries an
  `AlternateSet` of substitute glyph IDs; the caller picks an index
  (default 0). Drives `aalt` / `salt` features. Out-of-range
  alternate index returns `None`.
- `tables::gsub::GsubTable::apply_lookup_type_5(lookup_index, gids, pos)`
  — Contextual Substitution. All three sub-table formats are decoded
  with the same `SubstLookupRecord` machinery as LookupType 6:
  - **Format 1** (simple glyph contexts) — coverage on input[0] +
    per-coverage `SubRuleSet` of explicit input glyph sequences.
  - **Format 2** (class-based) — coverage on input[0] + a single
    `ClassDef` + per-input-class `SubClassSet`.
  - **Format 3** (coverage-based) — `Coverage[]` array (one per input
    position) + a single `SubstLookupRecord[]`.

  LookupType 5 is the predecessor of LookupType 6 minus the backtrack
  / lookahead arrays (the input window IS the context). Older fonts
  encode contextual rules here; modern fonts overwhelmingly prefer
  LookupType 6.
- `tables::gsub::GsubTable::apply_lookup_type_8(lookup_index, gids, pos)`
  — Reverse Chained Context Single Substitution (Format 1). Coverage
  on `gids[pos]` plus backtrack and lookahead `Coverage[]` arrays
  plus a `substituteGlyphIDs[]` indexed by the input coverage index.
  Unlike LookupType 6, the substitution is single-glyph (no
  `SubstLookupRecord[]`) and the spec mandates reverse-text
  processing of the input run — a higher-level shaper walks `pos`
  from right to left; this entry point answers "does the rule fire
  here?" Used by some Arabic fonts for isolated forms.
- `apply_subst_records` (the LookupType 6 / 5 nested-dispatch helper)
  now also handles nested LookupType 2 / 3 / 5 references — previously
  only nested 1 / 4 / 6 were dispatched, the rest were silently
  skipped. Recursion bound is unchanged at
  `MAX_NESTED_LOOKUP_DEPTH = 8`.
- ExtensionSubst (LookupType 7) wrappers are unwrapped transparently
  for every new entry point.
- Public `Font` API:
  - `Font::gsub_apply_lookup_type_2(lookup_index, gid) -> Option<Vec<u16>>`
  - `Font::gsub_apply_lookup_type_3(lookup_index, gid, alternate_index) -> Option<u16>`
  - `Font::gsub_apply_lookup_type_5(lookup_index, gids, pos) -> Option<Vec<u16>>`
  - `Font::gsub_apply_lookup_type_8(lookup_index, gids, pos) -> Option<u16>`
- New tests:
  - `tables::gsub::tests::lookup_type_2_expands_one_glyph_into_sequence`,
    `lookup_type_2_returns_none_off_coverage`,
    `lookup_type_2_zero_glyph_count_means_deletion`.
  - `lookup_type_3_picks_default_alternate_zero`,
    `lookup_type_3_picks_indexed_alternates`,
    `lookup_type_3_out_of_range_alternate_returns_none`.
  - `lookup_type_5_format_1_simple_glyph_context`,
    `lookup_type_5_format_2_class_based_context`,
    `lookup_type_5_format_3_coverage_based_context`.
  - `lookup_type_8_reverse_chain_substitutes_under_context`,
    `lookup_type_8_no_match_when_backtrack_or_lookahead_misses`.
  - `chain_context_can_dispatch_nested_lookup_type_2` — verifies the
    extended `apply_subst_records` correctly expands a nested
    LookupType-2 reference under a LookupType-6 chain context.
  - Integration tests against Noto Sans Arabic:
    `new_gsub_lookup_types_are_panic_free_across_arab_features` and
    `lookup_type_5_returns_none_for_non_context_lookups_in_noto`
    drive every lookup index referenced by every Arabic feature
    through the four new entry points to prove the dispatch walks
    real-world tables without panic.

Spec: Microsoft OpenType §"Multiple Substitution Subtable" (LookupType
2 Format 1), §"Alternate Substitution Subtable" (LookupType 3 Format
1), §"Sequence Context Format 1 / 2 / 3" (LookupType 5),
§"Reverse Chaining Contextual Single Substitution Subtable"
(LookupType 8 Format 1). Apple TrueType Reference §"GSUB". ISO/IEC
14496-22 §6 (OFF).

### Added — GSUB LookupType 4 wiring + LookupType 6 chained context (2026-05-04)

- `tables::gsub::GsubTable::apply_lookup_type_4(lookup_index, glyphs)`
  — lookup-index-specific entry point for ligature substitution. Walks
  every sub-table inside the named LookupType-4 lookup and returns
  `Some((replacement_gid, consumed))` when one of them matches a prefix
  of `glyphs`. ExtensionSubst (LookupType 7) wrappers are unwrapped
  transparently. Complement to the existing `lookup_ligature` walker
  (which scans every lookup); the new method is what a feature-driven
  shaper calls after resolving `liga` / `rlig` / `dlig` for the active
  script via `features_for_script`.
- `tables::gsub::GsubTable::apply_lookup_type_6(lookup_index, gids, pos)`
  — chained-context substitution. All three sub-table formats are
  decoded:
  - **Format 1** (simple glyph contexts): coverage on the first input
    glyph + per-coverage `ChainSubRuleSet` of explicit
    `(backtrack, input, lookahead)` glyph sequences plus per-rule
    `SubstLookupRecord[]`.
  - **Format 2** (class-based contexts): coverage + three `ClassDef`
    tables (backtrack / input / lookahead) + per-input-class
    `ChainSubClassSet` whose rules are class sequences.
  - **Format 3** (coverage-based contexts): three independent
    `Coverage[]` arrays (backtrack / input / lookahead) + a single
    `SubstLookupRecord[]`.
  Each match's `SubstLookupRecord { sequenceIndex, lookupListIndex }`
  is recursively dispatched: nested LookupType 1 substitutes one glyph,
  nested LookupType 4 substitutes `componentCount` glyphs, nested
  LookupType 6 recurses (bounded depth = 8 to stop self-referential
  loops). Backtrack sequences are matched in reverse-text order per the
  spec. Returns the full rewritten glyph run (`Vec<u16>`) starting at
  `pos`, or `None` if no chain rule applies.
- Public `Font` API:
  - `Font::gsub_apply_lookup_type_4(lookup_index, gids) -> Option<(u16, usize)>`
  - `Font::gsub_apply_lookup_type_6(lookup_index, gids, pos) -> Option<Vec<u16>>`
- New tests:
  - `tables::gsub::tests::apply_lookup_type_4_consumes_correct_count`
    + `apply_lookup_type_4_skips_non_ligature_lookups`.
  - `tables::gsub::tests::gsub_lookup_type_6_format_1_chained_context_simple_sequence`,
    `gsub_lookup_type_6_format_3_coverage_based_chained_context`,
    plus class-based Format-2 round-trip + three "no match" guards
    (wrong backtrack, short window, wrong class).
  - Integration tests against DejaVu Sans:
    `gsub_lookup_type_4_ligature_substitution_applies_for_fi_in_dejavu`
    asserts the `liga` feature for `latn` resolves to a LookupType-4
    lookup that substitutes `[f, i]` → fi-ligature-glyph;
    `gsub_lookup_type_4_returns_consumed_count_2_for_2_glyph_ligature`
    cross-checks the `consumed` count against the global walker for
    the `[f, l]` pair.

This is the largest GSUB unlock since round-1: chained-context lookups
are how Arabic shaping cascades, Indic reordering, and
context-dependent ligatures (e.g. Latin `ct` only between word
boundaries) are encoded in modern fonts. LookupType 4 wiring closes
the loop on the `liga` feature dispatch path that
`gsub_features_for_script` was published for in the prior round.

Spec: Microsoft OpenType §"Ligature Substitution Subtable" (LookupType
4 Format 1) and §"Chained Sequence Context Format 1: simple glyph
contexts" / §"Chained Sequence Context Format 2: class-based glyph
contexts" / §"Chained Sequence Context Format 3: coverage-based glyph
contexts". Apple TrueType Reference §"GSUB". ISO/IEC 14496-22 §6 (OFF).

### Added — GSUB feature-tagged single substitution (LookupType 1) (2026-05-04)

- `tables::gsub::GsubTable` now decodes the **ScriptList** +
  **FeatureList** common-tables and exposes per-`(script, language)`
  feature lookup via `features_for_script(script_tag, lang_tag)`. The
  active LangSys (caller-supplied tag, falling back to
  `DefaultLangSys`) is resolved and each `featureIndex` reference is
  expanded into a `GsubFeature { tag: [u8; 4], lookup_indices: Vec<u16> }`.
- `tables::gsub::GsubTable::apply_lookup_type_1(lookup_index, gid)`
  walks every sub-table in the named lookup looking for a Single
  Substitution (LookupType 1) match. Both formats are supported:
  - **Format 1** — coverage + signed `deltaGlyphID`; result is
    `(gid + delta) mod 65536` so negative deltas wrap correctly.
  - **Format 2** — coverage + indexed `substituteGlyphIDs[]` array.
  ExtensionSubst (LookupType 7) wrappers are unwrapped transparently.
  Returns `None` when the lookup index is out of range, the input
  glyph is not in the lookup's coverage, or the referenced lookup is
  not a single-substitution lookup (e.g. a ligature lookup is
  silently skipped — call `lookup_ligature` for those).
- Public `Font` API:
  - `Font::gsub_features_for_script(script_tag, lang_tag) -> Vec<GsubFeature>`
  - `Font::gsub_apply_lookup_type_1(lookup_index, gid) -> Option<u16>`
- Public re-export: `oxideav_ttf::GsubFeature { tag, lookup_indices }`.
- New integration test fixture `tests/fixtures/NotoSansArabic-Regular.ttf`
  — Noto Sans Arabic 2022 (OFL/SIL, see `NOTO-ARABIC-OFL-LICENSE.txt`).
  The new `tests/noto_arabic_gsub.rs` resolves the `arab` script's
  feature list, asserts `init`/`medi`/`fina` are exposed, and
  applies feature `init` to U+0628 BEH to confirm the joining-form
  glyph differs from the isolated form. A bulk pass over
  U+0620..U+064A confirms the lookup substitutes most Arabic
  joining letters.

This unblocks the consumer crate's Arabic shaper for fonts that ship
positional forms via GSUB rather than the legacy Unicode
Presentation Forms-B block (PF-B is a fallback used by older fonts
like DejaVu Sans; modern Arabic fonts — Noto Sans Arabic UI, the
Indic-region Noto fonts, and most Adobe Source Sans variants — rely
on GSUB lookups).

Spec: Microsoft OpenType §"GSUB — Glyph Substitution Table" /
§"Common Table Formats" (ScriptList / FeatureList / LookupList) /
§"Single Substitution Subtable" (formats 1 and 2). Apple TrueType
Reference §"GSUB". ISO/IEC 14496-22 §6 (OFF).

### Added — variable fonts: fvar / avar / gvar parsers + delta-applied outlines (2026-05-04)

- New `tables::fvar::FvarTable` parser for the Font Variations Header.
  Decodes the version-1 axes array (4-byte tag, Fixed 16.16
  min/default/max, 16-bit flags + nameID) and the named-instance array
  (subfamily nameID + flags + per-axis coordinates + optional
  postScriptNameID). Both short and long instance encodings are
  recognised; out-of-bounds / disordered min/default/max combinations
  are rejected.
- New `tables::avar::AvarTable` parser for the Axis Variations Table.
  Decodes the per-axis piecewise-linear segment-map list (F2DOT14
  pairs); `remap_normalised(axis_index, value)` performs the bracket-
  and-interpolate lookup. v2 (variable-axis remap with delta-set
  index map) is accepted at the header but falls back to identity —
  v2 only matters when its delta-sets are applied, which we don't.
- New `tables::gvar::GvarTable` walker for the Glyph Variations Table.
  Handles both short and long per-glyph offset arrays; per-glyph data
  blocks decode the TupleVariationStore (embedded peak tuples vs
  shared-tuple references; intermediate-region start/end tuples;
  shared and PRIVATE_POINT_NUMBERS point-number sets;
  packed-points run-length stream; packed-deltas zero/byte/word
  run-length stream). The tuple-scalar product (default-region or
  intermediate-region) is computed against the caller's normalised
  coord vector and the per-point dx/dy deltas are accumulated into
  the static glyph outline.
- Public `Font` API:
  - `Font::is_variable() -> bool`
  - `Font::variation_axes() -> &[VariationAxis]`
  - `Font::named_instances() -> &[NamedInstance]`
  - `Font::variation_coords() -> &[f32]`
  - `Font::set_variation_coords(&mut self, coords: &[f32])` (clamps
    each value to its axis `[min, max]`; over-long vectors are
    truncated, shorter vectors leave trailing axes unchanged)
  - `Font::normalised_coords() -> Vec<f32>` (each entry in `[-1, +1]`,
    after the fvar normalisation rule + the avar remap)
- `Font::glyph_outline(glyph_id)` now applies gvar deltas when the
  current variation coords differ from the axis defaults. Static-font
  callers see no behaviour change. Composite glyphs are returned in
  static form (per-component variation propagation is deferred).
- Public re-exports: `oxideav_ttf::VariationAxis { tag, min, default,
  max, flags, name_id }` and `oxideav_ttf::NamedInstance {
  subfamily_name_id, flags, coords, post_script_name_id }`.
- New integration test fixture `tests/fixtures/InterVariable.ttf` —
  Inter 4.0 (OFL/SIL, see `INTER-OFL-LICENSE.txt`). 2 axes (`opsz`,
  `wght`) + 9 named instances; the avar table bends the wght axis
  non-linearly and gvar carries delta sets for every glyph. The new
  `tests/inter_variable.rs` walks `'A'` at wght=400 vs wght=900 vs
  wght=100, asserting topology preservation + per-point divergence.

Spec: Microsoft OpenType §"fvar — Font Variations Table" / §"avar
— Axis Variations Table" / §"gvar — Glyph Variations Table" /
§"GlyphVariationData table" / §"Tuple Variation Header" / §"Packed
Point Numbers" / §"Packed Deltas". Apple TrueType Reference §"fvar"
/ §"avar" / §"gvar".

### Added — sbix table parser (Apple Color Emoji bitmap strikes) (2026-05-04)

- New `tables::sbix::SbixTable` walker for Apple's Standard Bitmap
  Graphics container. Validates the version-1 header + per-strike
  offset array up front; per-strike `glyphDataOffsets` arrays are
  walked lazily via `glyph(strike_index, glyph_id)`.
- Public `Font` API:
  - `Font::has_sbix() -> bool`
  - `Font::sbix_strikes() -> Vec<u16>` (de-duped + sorted ascending
    list of strike ppems)
  - `Font::sbix_glyph(glyph_id, ppem) -> Option<SbixGlyph>` —
    best-fit lookup that scans every strike, picks the one whose
    ppem is closest to the request (ties favour the larger strike
    per the spec recommendation), and returns the per-glyph
    bitmap as `SbixGlyph { graphic_type: [u8; 4], bytes: &[u8],
    origin_x: i16, origin_y: i16 }`.
- Re-exports `oxideav_ttf::SbixGlyph`. `graphic_type` is one of
  `*b"png "`, `*b"jpg "`, `*b"tiff"`, or `*b"dupe"` (the spec's
  glyph-aliasing sentinel — payload is a 2-byte u16 glyph id; we
  expose it explicitly so the caller can do its own
  cycle-detection rather than recursing here).

Spec: Microsoft OpenType §"sbix — Standard Bitmap Graphics Table" /
Apple TrueType Reference §"sbix" (version 1 only).

### Added — COLR + CPAL parsers (vector colour-emoji layer stack) (2026-05-04)

- New `tables::colr::ColrTable` walker for COLR **version 0** — flat
  per-base-glyph layer stack with palette-indexed colours. Binary-
  searches the `BaseGlyphRecord` array on `glyph_id`, then walks the
  contiguous `LayerRecord` slice to produce
  `Vec<ColorLayer { layer_glyph_id, palette_index }>` in back-to-front
  paint order. Higher header versions are accepted at parse time
  (their additive trailing fields are ignored); v1 paint-graph
  decoding (gradients / transforms / composites) and v2/v3
  variable-COLR are out of scope for this round.
- New `tables::cpal::CpalTable` walker for CPAL **versions 0 and 1**.
  Decodes the per-palette colour-record-index array and validates the
  combined `ColorRecord` array; v1's three trailing offsets
  (`paletteTypesArrayOffset`, `paletteLabelsArrayOffset`,
  `paletteEntryLabelsArrayOffset`) are stored so consumers can read
  `palette_type(i)` for `USABLE_WITH_LIGHT_BACKGROUND` /
  `USABLE_WITH_DARK_BACKGROUND` hints.
- Public `Font` API:
  - `Font::has_color_layers() -> bool`
  - `Font::color_layers(glyph_id: u16) -> Vec<ColorLayer>`
  - `Font::cpal_color(palette_index, color_index) -> Option<[u8; 4]>`
    (RGBA byte order, swizzled out of CPAL's on-disk BGRA)
  - `Font::cpal_palette(palette_index) -> Option<Vec<[u8; 4]>>`
  - `Font::cpal_num_palettes() -> u16`
  - `Font::cpal_palette_type(palette_index) -> u32`
- Public re-export: `oxideav_ttf::ColorLayer { layer_glyph_id,
  palette_index }`. The reserved `palette_index == 0xFFFF` is the
  spec's "use foreground colour" sentinel — consumers substitute
  their own when they encounter it.

### Fixed — `from_collection_bytes` BadOffset on real `.ttc` (2026-05-04)

- `Font::from_collection_bytes(bytes, index)` previously sub-sliced
  `bytes` from the subfont header offset and ran the standard sfnt
  parse path against the sub-slice. That works for the *header*, but
  the per-table records inside a TTC subfont's table directory carry
  **file-relative** offsets (per OpenType §"Font Collections": *"The
  table offsets in all table directories within a TTC file are
  measured from the beginning of the TTC file"*), so every offset
  underflowed and the parse died with `Error::BadOffset` on the first
  table whose header sat below the subfont offset. Real-world impact:
  `NotoSansCJK-Medium.ttc` (and every other Noto Sans CJK collection)
  failed to parse subfont 0.
- Fix: thread a `header_offset` through `TableDirectory::parse` and
  `Font::from_bytes_at(bytes, header_offset)` so the directory header
  is read from `bytes[header_offset..]` while the per-table data
  slices remain anchored to the full `bytes` buffer.
- New regression test: `tests/ttc_subfont.rs` walks all subfonts of a
  cached `NotoSansCJK-Medium.ttc` (network-gated, skip-on-absent —
  populated by oxideav-scribe round-5's fixture helper).
- New unit test: `parser::tests::parses_directory_with_nonzero_header_offset`
  validates the new `header_offset` parameter against a hand-built
  buffer that distinguishes file-relative from header-relative
  offsets.

### Added — cmap format 14 (Unicode Variation Sequences) (2026-05-04)

- `Font::lookup_variation(codepoint, variation_selector) -> Option<u16>`
  — resolves a `(base codepoint, variation selector)` pair through the
  cmap format-14 subtable. Returns the per-pair glyph from the
  non-default UVS table when present, the base codepoint's glyph when
  the pair is in the default UVS table (matches HarfBuzz's
  `hb_font_get_variation_glyph` contract), or `None` otherwise.
- `CmapTable::lookup_variation` — internal walker that binary-searches
  the variation selector record list, then within each record's
  `DefaultUVS` (UnicodeRange list) and `NonDefaultUVS` (UVSMapping
  list) sub-tables. Both inner walks are also binary-searched —
  every list in format 14 is required to be sorted ascending per the
  Microsoft spec.
- `parser::read_u24` — big-endian 24-bit unsigned reader, used by the
  format-14 varSelector / startUnicodeValue / unicodeValue fields.

Real-world use cases this unblocks:

- Emoji presentation selectors `<emoji, U+FE0F>` (emoji form) and
  `<emoji, U+FE0E>` (text form).
- Skin-tone modifier sequences via fonts that publish them through
  format 14 instead of GSUB.
- Registered Ideographic Variation Sequences `<CJK, U+E0100..U+E01EF>`
  in pan-CJK fonts (Source Han Sans / Noto Sans CJK family).

### Fixed — cmap format-14 sibling rejected the whole font (2026-05-04)

- `cmap` parse no longer fails when the font ships a format-14 (Unicode
  Variation Selectors) subtable next to a supported subtable. Format-14
  records are now picked up as a sidecar UVS table at the encoding-record
  walk *before* per-format length validation runs, so the picker still
  finds the format-12 / format-4 sibling and the variation-sequence
  data is preserved for `lookup_variation`. Affected fonts include
  Noto Color Emoji (which ships `(0,5)/format-14` alongside
  `(3,10)/format-12`) and most CJK fonts that expose variation
  sequences.

## [0.1.1](https://github.com/OxideAV/oxideav-ttf/compare/v0.1.0...v0.1.1) - 2026-05-03

### Fixed

- *(clippy)* collapse 1|2|3 into 1..=3 + reword doc continuation

### Other

- color bitmap glyph tables (PNG-payload formats 17/18/19)
- TTC ('ttcf') header + Font::from_collection_bytes
- LookupType 6 mark-to-mark attachment for stacked diacritics
- LookupType 4 mark-to-base attachment for diacritics
- release v0.1.0

### Added — CBDT/CBLC color bitmap glyphs (2026-05-04)

- New `tables::cblc` module — Color Bitmap Location Table parser. Walks
  the CBLC header (major 2 = EBLC / 3 = CBLC), per-strike `BitmapSize`
  records (48 bytes including the two 12-byte `SbitLineMetrics`), the
  `IndexSubtableList`, and `IndexSubtable` formats 1/2/3/4/5. Resolves
  `(glyph_id, target_ppem)` to a `CblcEntry { image_format,
  image_data_offset, data_len, ppem_x, ppem_y, fixed_metrics }`.
  Strike picking is closest-ppem with larger-ppem tie-break (per the
  CBDT spec's "Scaling behavior" recommendation).
- `tables::cbdt` module — Color Bitmap Data Table parser. Decodes
  CBDT entry formats 17 (small metrics + PNG), 18 (big metrics + PNG),
  and 19 (raw PNG with metrics from CBLC). Returns a `ColorBitmap`
  with raw `png_bytes` for the consumer crate to decode via
  `oxideav-png`. Other CBDT formats (BGRA, monochrome, grayscale)
  return `Ok(None)`.
- `Font::has_color_bitmaps()`, `Font::color_strike_sizes()`,
  `Font::glyph_color_bitmap(gid, target_ppem) -> Option<ColorBitmap>`
  — public glue. The PNG byte stream is borrowed from the font input
  with the same `'a` lifetime; no allocation in the lookup path.
- `loca` and `glyf` are now jointly optional. CBDT/CBLC-only colour-
  emoji fonts (e.g. `NotoColorEmoji.ttf`) ship without either table;
  `from_bytes` no longer rejects them. `glyph_outline` returns an
  empty outline + `glyph_bounding_box` returns `None` when the pair
  is absent. Half-pair (only one of the two) is still rejected as
  malformed.
- `parser::read_i8` (was missing) added for the CBDT/CBLC byte readers.
- New public types: `ColorBitmap`, `BigGlyphMetrics`, `SmallGlyphMetrics`.

Spec: Microsoft OpenType §"CBLC" + §"CBDT" + §"EBLC" (CBLC inherits
the IndexSubtable layout). Microsoft "Color Bitmap Glyphs (CBDT/CBLC)
font format" / OpenType 1.9.1.

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
