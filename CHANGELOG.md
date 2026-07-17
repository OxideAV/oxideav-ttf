# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **Internal public surface marked `#[doc(hidden)]`.** The internal table
  parsers (`XxxTable` structs and parse-tree types), the sfnt walker
  (`parser`), wholly-internal table modules (gvar / IUP / variation-store
  plumbing, glyf, cmap, ...), and structural layout constants are now
  `#[doc(hidden)]`: they remain `pub` for tests/fuzz but are no longer part
  of the stable semver-checked API. The stable surface -- `Font` and its
  documented methods, `Error`, the outline / shaping / collection / AGL
  types, and the curated crate-root re-exports -- is unchanged.

- **`hhea` + `maxp` full field decode (§5.2.4 / §5.2.5).** `hhea` now
  decodes the min left / right side-bearing extremes, `xMaxExtent`, the
  caret-slope rise / run / offset (with `caret_is_vertical()`), and
  `metricDataFormat` in addition to the ascent / descent / advance-max it
  already had. `maxp` now decodes the full v1.0 TrueType maxima
  (`maxPoints`, `maxContours`, composite limits, the bytecode-interpreter
  resource caps, `maxComponentDepth`) as a `v1: Option<MaxpV1>`, populated
  only for a v1.0 table. New `Font::hhea_table` / `Font::maxp_table`.
  Existing fields keep their names.

- **`head` table full field decode (§5.2.1).** The `head` decoder
  previously surfaced only `unitsPerEm`, the bbox, `macStyle`, and
  `indexToLocFormat`. It now decodes the whole table: `fontRevision`, the
  16-bit `flags` word (with `flag_baseline_at_y0` / `flag_lsb_at_x0` /
  `flag_instructions_alter_advance` / `flag_lossless` / `flag_last_resort`
  predicates), the created / modified timestamps, `lowestRecPPEM`,
  `fontDirectionHint`, `glyphDataFormat`, and the full `macStyle`
  predicate set. New `Font::head_table` / `Font::font_revision` /
  `Font::lowest_rec_ppem`. Existing fields keep their names.

- **`OS/2` table full field decode (§5.2.3).** The `OS/2` decoder
  previously surfaced only a handful of fields (weight class, fsSelection,
  typo + x/cap-height). It now decodes the complete field set across
  versions 0..5: `xAvgCharWidth`, `usWidthClass`, the `fsType` embedding
  permissions (with `embedding_installable` / `embedding_restricted` /
  `embedding_preview_print` / `embedding_editable` / `embedding_no_subsetting`
  / `embedding_bitmap_only` predicates), the sub/superscript + strikeout
  metrics, `sFamilyClass`, the 10-byte `panose`, the four `ulUnicodeRange`
  words, `achVendID` (with `vendor_id()`), `usFirstCharIndex` /
  `usLastCharIndex`, the Windows vertical metrics, the `fsSelection` style
  predicates (`is_bold` / `is_italic` / `is_regular` / `use_typo_metrics`),
  and the versioned tail (`ulCodePageRange1`/`2`, `sxHeight` / `sCapHeight`
  / `usDefaultChar` / `usBreakChar` / `usMaxContext`, optical-size window),
  each later-version field as `Option`. New `Font::os2_table` /
  `Font::width_class` / `Font::embedding_installable`. The previously
  surfaced fields keep their names, so existing callers are unaffected.

### Added

- **`USE_MY_METRICS` composite metric inheritance (§5.3.4).**
  `Font::glyph_advance` and `Font::glyph_lsb` now honour the composite
  `USE_MY_METRICS` flag: when a composite glyph references a component
  carrying the flag, the composite's advance width and left side bearing
  are taken from that component's `hmtx` entry rather than the composite's
  own (the spec's mechanism for making e.g. `i`-circumflex inherit
  dotless-`i`'s metrics). The last flagged component wins, and the chase
  through nested composites is depth-bounded. Outline-only and
  non-flagged glyphs are unaffected. New `GlyfTable::use_my_metrics_glyph`.

### Added

- **`fpgm` / `prep` hinting-program raw accessors (§5.3.3).** The `fpgm`
  font program and `prep` control-value program (TrueType bytecode) are
  now surfaced raw through `Font::fpgm_program()` / `Font::prep_program()`,
  completing the hinting-table family alongside the existing `cvt `
  accessors. The bytecode is **not** executed (out of scope); the bytes
  are for tooling, round-tripping, and a downstream interpreter. New
  `Font::has_hinting_program()` reports whether the font carries any of
  `fpgm` / `prep` / a non-empty `cvt `.

- **`MERG` merge table (§5.7.5).** New decoder for the glyph-merge table:
  the header, the array of ClassDef-table offsets, and the square
  `mergeClassCount²` array of `uint8` merge-entry bit-fields. Glyphs
  resolve to merge classes through the shared ClassDef parser
  (ClassDefFormat1/2, class 0 default); each `(firstClass, secondClass)`
  cell decodes through the new `MergeEntry` type into the six defined
  flags (`MERGE_LTR` / `GROUP_LTR` / `SECOND_IS_SUBORDINATE_LTR` and RTL
  siblings). `version == 0`, the merge-data array, and every ClassDef
  offset are bounds-checked. The §5.7.5.3 run-processing algorithm is left
  to the renderer. New `Font::has_merg` / `Font::merg_table` and the
  `MergTable` / `MergeEntry` public types.

- **`DSIG` digital signature table (§8.x).** New structural decoder for
  the digital-signature table: the header (`version`, `numSignatures`,
  `flags`), the `SignatureRecord` array (`format`, `length`, `offset`),
  and — for the spec's only defined block format, Signature Block
  Format 1 — the reserved words, `signatureLength`, and the PKCS#7 packet
  surfaced raw as a borrowed `&[u8]`. The PKCS#7 / X.509 contents are not
  parsed and the signature is not verified (host-application policy, out
  of a font-table parser's scope). `version == 1`, every block range, and
  the Format-1 `signatureLength` are bounds-checked; the "cannot be
  resigned" permission bit is decoded. New `Font::has_dsig` /
  `Font::dsig_table` and the `DsigTable` / `DsigSignature` public types.

- **`kern` table Format 2 — class-based two-dimensional array (§5.7.3).**
  The legacy `kern` decoder previously handled only Format 0 (explicit
  pair list). Format 2 — where left and right glyphs map to classes and
  the kerning value is the array cell at `(leftClass, rightClass)`,
  addressed through the spec's pre-multiplied class values — is now
  decoded for the Microsoft / OpenType header variant. Horizontal kerning
  subtables only; "minimum" and non-horizontal subtables are skipped, and
  kerning subtables are additive so `KernTable::lookup` sums every matching
  format-0 pair and format-2 cell. Formats 1 and 3..255 stay reserved per
  the spec. New `KernTable::format2_subtable_count`.

- **`EBDT` composite glyph bitmaps — formats 8 & 9 (§5.6.2.2.8 /
  §5.6.2.2.9).** Composite (component-data) embedded bitmaps were
  previously skipped (`glyph_gray_bitmap` returned `None`). The new
  `EbdtTable::lookup_composite` decodes the composite descriptor — the
  composite's own Small/BigGlyphMetrics plus its `EbdtComponent` array
  (`glyphID` + `int8 xOffset` + `int8 yOffset`) — and `Font::glyph_gray_bitmap`
  now assembles the finished glyph: each component's bitmap is resolved out
  of the *same* `EBLC` strike and blitted onto the composite's canvas at its
  `(xOffset, yOffset)` placement. Nested composites (a component that is
  itself a format-8/9 glyph) are followed up to a bounded depth, with
  self-reference guarded. Out-of-canvas component pixels clip. New public
  types `EbdtComponent` / `CompositeBitmap`.

- **Variable-font MATH value resolution (§6.3.6.2.1).** Every MATH
  `MathValueRecord` carries an optional device / VariationIndex offset
  measured from the start of its *parent* sub-table; the new
  `MathValueRecord::resolved_value` and the `*_resolved` accessors
  (`MathConstants::value_resolved`,
  `MathGlyphInfo::italics_correction_resolved` /
  `top_accent_attachment_resolved` / `math_kern_resolved`,
  `MathVariants::assembly_italics_correction_resolved`) fold in the
  VariationIndex delta evaluated against the GDEF `ItemVariationStore` at
  the current normalised instance. A classic ppem-indexed Device table
  (render-time only) contributes no font-unit adjustment, and a NULL
  offset collapses to the plain design-unit value, so a static instance is
  unchanged. Parent-relative offset bases are honoured per the spec
  (MathConstants, per-glyph value sub-tables, MathKern, GlyphAssembly).
  Reuses the shared Device/VariationIndex decoder. (Closes the MATH half
  of the variable-font VariationIndex sweep.)

- **`Font`-level MATH variation accessors.** `Font::math_constant_var`,
  `math_italics_correction_var`, `math_top_accent_attachment_var`,
  `math_kern_var` and `math_assembly_italics_correction_var` resolve the
  matching MATH value at the font's current variation instance (set via
  `set_variation_coords`), pulling the GDEF `ItemVariationStore` and
  normalised coordinates through the same plumbing the GPOS / GDEF / BASE
  `*_var` accessors use. Fonts without a MATH table decline cleanly.
  Integration coverage on DejaVu Sans (static MATH) confirms the resolved
  values equal the plain design-unit values when no axis is active.

- **Variable-font BASE baseline coordinates
  (`Font::base_horiz_y_for_script_baseline_var`,
  `base_vert_x_for_script_baseline_var`).** `BaseCoordFormat3`
  VariationIndex device offsets are now resolved against the BASE
  `ItemVariationStore` at the current instance, so a per-script baseline
  position tracks the design axes. `BaseCoord::parse` records each
  Format-3 device table's *absolute* position within the BASE table
  (new `device_abs_offset` field) and `BaseTable` retains its raw bytes,
  so `BaseCoord::resolve` can dereference the device table without
  re-walking the axis tree. Format 1/2 and classic Device tables are
  unchanged. (Closes the BASE half of the variable-font GPOS/GDEF/BASE
  VariationIndex sweep.)

- **CFF2 variable-glyph `blend` interpolation test + doc.** The CFF2
  charstring interpreter already evaluates `blend` as
  `default + Σ scalarᵣ · deltaᵣ` at an arbitrary instance via
  `glyph_outline_at`; this round adds a hand-built variable-CFF2 fixture
  (1-region VariationStore + a `blend`-ed move) proving a coordinate
  interpolates 100→300→500 across default/half/max, and corrects the
  module doc (it previously claimed only the default instance was
  rendered). Also documents that Type2 charstring 16-bit ints use the
  `[28, hi, lo]` form (not the DICT `[29, …]` 5-byte form).

- **Variable-font GPOS mark-to-ligature (`lookup_mark_to_ligature_var`,
  Font + GposTable).** The LookupType-5 accessor resolves AnchorFormat3
  VariationIndex offsets on both the mark anchor and the selected
  ligature component's anchor against the GDEF `ItemVariationStore`, so
  a mark attached to a ligature component tracks the design axes.
  (`parse_anchor` is now fully variation-aware: every GPOS anchor path
  — base, mark, cursive, ligature — goes through `parse_anchor_with`.)

- **Font-level variable-font GPOS / GDEF accessors.** `Font` now
  exposes `lookup_kerning_var`, `lookup_mark_to_base_var`,
  `lookup_mark_to_mark_var`, `lookup_cursive_attachment_var`,
  `lookup_mark_to_ligature_var`, `gpos_apply_lookup_type_1_var`, and
  `ligature_carets_resolved`. Each
  decodes the GDEF `ItemVariationStore` once and threads the font's
  current `normalised_coords()` (post-`avar`) into the table-level
  variation resolvers, so VariationIndex device offsets in GPOS value
  records / anchors and GDEF caret values are resolved at the active
  variation instance set via `set_variation_coords`. Two new
  InterVariable integration tests confirm var == static at the default
  instance and panic-free calls across the `wght` extremes.

- **Device / VariationIndex table decoder (`tables::device`).** New
  `DeviceOrVariationIndex` type decodes the shared 6-byte Device /
  VariationIndex sub-table referenced by GPOS ValueRecords, Anchors,
  GDEF caret values, and BASE coordinates, discriminating on the
  `deltaFormat` field: classic Device tables (`0x0001`/`0x0002`/`0x0003`
  — 2/4/8-bit MSB-first packed pixel deltas) are unpacked for tooling,
  and VariationIndex tables (`0x8000`) carry the `(outer, inner)`
  delta-set index pair. `font_unit_delta(ivs, coords)` resolves a
  VariationIndex against an `ItemVariationStore` at the current
  normalised instance; `resolve_device_delta` is the NULL-tolerant
  offset entry point.

- **Variable-font GPOS single adjustment (`apply_lookup_type_1_var`).**
  The new LookupType-1 sibling resolves a matched ValueRecord's
  VariationIndex device offsets against the GDEF `ItemVariationStore`,
  folding the interpolated font-unit deltas into the returned
  `PosValue` so a variable font's `wght`/`wdth`/`opsz` instance shifts
  single-adjustment placement and advance. Identical to the static path
  for value records without device offsets.

- **Variable-font GPOS attachment (`lookup_mark_to_base_var`,
  `lookup_mark_to_mark_var`, `lookup_cursive_attachment_var`).** The
  mark-to-base, mark-to-mark, and cursive accessors gained variation
  siblings that resolve AnchorFormat3 X/Y VariationIndex device offsets
  against the GDEF `ItemVariationStore` at the current instance, so
  diacritic attachment points, mark-on-mark stacking, and cursive
  connection geometry track the variable axes. AnchorFormat3 parsing
  now reads the two device offsets (was: skipped); format 1/2 and
  device-offset-free format-3 anchors are unchanged.

- **Variable-font GPOS kerning (`lookup_kerning_var`).** Pair
  adjustment now has a variation sibling that resolves the matched
  pair's `xAdvance` VariationIndex device offset against the GDEF
  `ItemVariationStore`, honouring the spec's per-format device-offset
  base (the PairSet table for PairPosFormat1, the sub-table for
  PairPosFormat2). Variable kerning tracks the design axes; static path
  unchanged for pairs without an `xAdvance` device offset.

- **Variable-font GDEF ligature carets
  (`GdefTable::ligature_carets_resolved`).** Resolves each ligature
  caret to a concrete font-unit coordinate at the current instance:
  CaretValueFormat1 passes through, Format3 folds in its VariationIndex
  delta resolved against the GDEF `ItemVariationStore` (device offset
  relative to the CaretValueFormat3 table base), and Format2
  contour-point carets surface as `None` (need the TT bytecode
  interpreter). Cursor placement inside a ligature now tracks the
  variable axes.

- **CFF reverse glyph-name lookup + iteration.**
  `CffTable::gid_for_name(name)` inverts the charset (name → lowest GID),
  and `CffTable::iter_glyph_names()` walks every `(gid, name)` pair.
  `Font::gid_for_glyph_name` and `Font::iter_glyph_names` now fall back to
  the CFF charset when the `post` table has no names, closing the
  bidirectional glyph-naming loop for OTTO `post` v3.0 fonts.
  (`Font::iter_glyph_names` now returns a boxed iterator so it can yield
  either the `post` or the CFF name stream.)

- **CFF glyph-name resolution — charset SID → PostScript name.** The
  `CFF ` walker now retains the String INDEX and ships the 391 CFF
  standard strings (Adobe TN #5176 Appendix A, in `cff::strings`).
  `CffTable::string_for_sid(sid)` resolves any SID (standard-strings
  table below 391, font String INDEX above), and
  `CffTable::glyph_name(gid)` maps a glyph through the charset
  (GID → SID → name). `Font::glyph_name` now falls back to the CFF
  charset when the `post` table has no names — the common OTTO `post`
  v3.0 case, where the CFF charset is the only glyph-name source.
  Verified against a system CFF OTF (every glyph name in the first 2000
  GIDs resolved; `'A' → "A"`, `'0' → "zero"`, `' ' → "space"`).

- **CFF2 per-instance variation — `blend` now interpolates at non-default
  coordinates.** The CFF2 charstring interpreter's `blend` (16) operator
  computes the full variation sum `default + Σ scalarᵣ · deltaᵣ` rather
  than only collapsing to defaults, with the per-`vsindex` region scalars
  computed from the VariationStore at the target instance via the new
  `mvar::ItemVariationStore::region_scalars(index, coords)`.
  `Cff2Table::glyph_outline_at(gid, normalised_coords)` renders a glyph
  at any instance (`glyph_outline` stays the default-instance shortcut),
  and `Font::glyph_outline` now feeds the avar-bent normalised coordinate
  vector into the CFF2 path when the caller has set non-default axis
  coordinates — so a variable CFF2 font's outlines retarget with
  `Font::set_variation_coords` just like the TrueType `gvar` path. New
  `Cff2Table::vsindex_count` / `region_count` accessors. Verified against
  a system variable CFF2 font (setting the weight axis to its extreme
  retargets ~all glyph outlines) — fixture not committed.

- **`CFF2` table — variable PostScript outlines (OpenType CFF2).** New
  `tables::cff2` module walks the CFF2 container (fixed 5-byte header +
  `topDictSize`, Top DICT, Global Subr INDEX, CharStrings INDEX,
  VariationStore, the always-present FDArray + optional FDSelect formats
  0/3, per-Font-DICT Private DICT with local subrs and default
  `vsindex`) and renders the **default-instance** outline of each glyph.
  CFF2 INDEXes use a 32-bit count — the shared CFF INDEX reader gained an
  `Index::parse_wide` variant for this, and the DICT operator range was
  widened to admit CFF2's `blend` (23) / `vstore` (24) DICT operators.
  The shared Type 2 interpreter gained a CFF2 mode (`Interp::new_cff2`)
  that suppresses the width prefix, allows charstrings to end at their
  data boundary (no `endchar`), and implements the `vsindex` (15) /
  `blend` (16) charstring operators — `blend` collapses each blended
  operand to its default value (deltas dropped via the per-`vsindex`
  region count read from the VariationStore), so the rendered outline
  matches the font at its default variation coordinates.
  `Font::glyph_outline` falls back to CFF2 when `glyf`/`CFF ` are
  absent; `Font::has_cff2_outlines` / `cff2_table` expose it. Validated
  locally, as a black-box check, against a system variable CFF2 font
  (hundreds of glyphs decoded, 5-region VariationStore) — not committed.

- **`JSTF` table — justification suggestions (ISO/IEC 14496-22:2019
  §6.3.5).** New `tables::jstf` module decodes the GSUB/GPOS-shaped
  navigation: `JstfTable` (header + script-record list, `script_tags` /
  `script(tag)`), `JstfScript` (`extender_glyphs` — e.g. Arabic
  kashidas, `default_lang_sys`, `lang_sys(tag)` / `lang_sys_tags`),
  `JstfLangSys` (priority-ordered `priority(index)`), and `JstfPriority`
  exposing all ten slots through the `JstfMod` enum — `mod_list(slot)`
  returns the GSUB/GPOS lookup-list indices in the
  `Jstf{GSUB,GPOS}ModList` for the eight enable/disable slots, and
  `jstf_max_lookup_count` reports the inline JstfMax lookup count for the
  two JstfMax slots. `Font::has_jstf` / `Font::jstf_table` expose it.
  Validated locally, as a black-box check, against system fonts that
  carry an `arab`-script JSTF table — fixtures not committed.

- **`MATH` table — mathematical typesetting parameters (ISO/IEC
  14496-22:2019 §6.3.6).** New `tables::math` module structurally
  decodes the whole MATH table and exposes typed accessors. `MathTable`
  validates the header's three sub-table offsets; `MathConstants` reads
  the four leading scalar fields, all 51 `MathValueRecord` constants
  (addressed by name through the `math::constant::*` index set in spec
  declaration order) and the trailing `radicalDegreeBottomRaisePercent`.
  `MathGlyphInfo` resolves per-glyph `italics_correction`,
  `top_accent_attachment`, `is_extended_shape`, and height-dependent
  `math_kern` (all four corners, with the §6.3.6.2.9 correction-height
  range search) via the shared common-layout Coverage parser.
  `MathVariants` exposes `min_connector_overlap`, ready-made stretchy
  `variants(gid, dir)`, and general glyph `assembly(gid, dir)` parts
  (with the extender flag) for both vertical and horizontal growth.
  `Font::has_math` / `Font::math_table` surface it. Validated locally,
  as a black-box check, against a system math OTF (STIX-class:
  hundreds of italic corrections, dozens of stretchy glyphs + glyph
  assemblies decoded) — fixture not committed.

- **`CFF ` table — PostScript outlines via a Type 2 charstring
  interpreter (Adobe TN #5176 + #5177).** OTTO-flavoured fonts (no
  `glyf`) now produce glyph outlines. The new `tables::cff` module walks
  the CFF container (fixed header + Name / Top DICT / String / Global
  Subr INDEXes, then the Top-DICT-referenced CharStrings INDEX, charset,
  Private DICT + local subrs), decodes DICTs (all integer/real operand
  encodings, one- and two-byte operators), and runs the per-glyph Type 2
  charstring through a full interpreter: every path operator
  (`rmoveto`/`hmoveto`/`vmoveto`, `rlineto`/`hlineto`/`vlineto`, the
  `rrcurveto`/`hhcurveto`/`vvcurveto`/`hvcurveto`/`vhcurveto`/`rcurveline`/`rlinecurve`
  curves, the `flex`/`hflex`/`hflex1`/`flex1` family), the hint operators
  (`hstem`/`vstem`/`hstemhm`/`vstemhm` plus `hintmask`/`cntrmask` mask-byte
  skipping), the arithmetic / storage / conditional escaped operators
  (`add`…`ifelse`, `put`/`get`, `index`/`roll`/`dup`/`exch`/`drop`), and
  biased `callsubr`/`callgsubr`/`return`/`endchar` with depth-bounded
  recursion. Cubic Beziers are flattened to on-curve polylines so CFF and
  TrueType outlines share one `TtOutline` type. CID-keyed fonts are
  supported end-to-end: `ROS` triggers the FDArray + FDSelect (formats 0
  and 3) path so each glyph picks the correct Font-DICT local subrs and
  `nominalWidthX`. charset formats 0/1/2 are decoded to per-GID SID/CID.
  `Font::glyph_outline` transparently falls back to CFF when `glyf` is
  absent; new accessors `Font::has_cff_outlines`, `Font::cff_table`,
  `Font::is_cid_keyed`, plus `CffTable::glyph_outline`/`glyph_width`/
  `sid_for_gid`.

### Changed

- **Shaper honours the full lookupFlag skip filter on multi-glyph
  match paths.** `Font::shape` now routes GSUB ligature matching and
  GPOS pair-adjustment (kerning) + cursive attachment through the new
  `Font::lookup_skips_glyph` predicate rather than an IGNORE_MARKS-only
  check. A kern pair (or cursive entry/exit pair) separated by an
  ignored combining mark now pairs the current glyph with the next
  *non-skipped* glyph, the canonical IGNORE_MARKS-on-kern case, and
  IGNORE_LIGATURES / MARK_ATTACHMENT_CLASS_FILTER / USE_MARK_FILTERING_SET
  narrow the ligature match the same way. The interleaved mark's own
  advance is left untouched. The GPOS mark-to-base / mark-to-mark /
  mark-to-ligature attachment scans now locate the nearest *non-skipped*
  preceding attachment glyph through the same predicate (replacing the
  hard-coded "stop at first non-mark" heuristic), so a `mkmk` lookup
  carrying MARK_ATTACHMENT_CLASS_FILTER or USE_MARK_FILTERING_SET binds
  the mark only to glyphs in the named class / set.

### Added

- **Unified lookupFlag skip predicate (§2 Common Table Formats).**
  `Font::lookup_skips_glyph(flags, mark_filtering_set, gid)` implements
  the full LookupFlag bit enumeration that controls which glyphs a
  GSUB / GPOS lookup ignores while matching its input: `IGNORE_BASE_GLYPHS`
  (`0x0002`), `IGNORE_LIGATURES` (`0x0004`), `IGNORE_MARKS` (`0x0008`),
  the high-byte `MARK_ATTACHMENT_CLASS_FILTER` (`0xFF00`), and
  `USE_MARK_FILTERING_SET` (`0x0010`) — the last two resolved against the
  GDEF MarkAttachClassDef and MarkGlyphSets structures. The new
  `Font::gsub_lookup_mark_filtering_set` / `gpos_lookup_mark_filtering_set`
  accessors (backed by `GsubTable::mark_filtering_set` /
  `GposTable::mark_filtering_set`) read the trailing `markFilteringSet`
  field at `6 + 2 * subTableCount` only when the `USE_MARK_FILTERING_SET`
  bit is set. With no GDEF the predicate never skips, matching the spec's
  "a GlyphClassDef table must be present" requirement.

- **Fused varied-metric accessors.** `Font::glyph_advance_varied(gid)`
  and `Font::glyph_lsb_varied(gid)` add the current instance's `HVAR`
  delta (§7.3.5.2/§7.3.5.3) to the static `hmtx` advance / LSB, rounded
  and clamped, so callers get the per-instance horizontal metric in one
  call; `Font::glyph_advance_height_varied(gid)` does the same for the
  `vmtx` advance height via `VVAR` (§7.3.8.2). All degrade to the static
  value for static fonts, fonts without the relevant variation table, or
  the default instance. Two InterVariable / DejaVu integration tests
  (HVAR varies an advance at wght=900 but not at the default; static
  font mirrors the un-varied metrics and reports no vertical height).

- **`cvar` CVT-variations table + `cvt ` Control Value Table access**
  (ISO/IEC 14496-22:2019 §7.3.2 / §5.3.2). `cvar` is parsed as a single
  tuple variation store (§7.2.2), reusing `gvar`'s packed-point /
  packed-delta / tuple-scalar machinery; embedded peak tuples,
  intermediate regions, shared and private point sets are all honoured,
  with "point numbers" interpreted as CVT indices and no IUP inference
  (per the §7.2.2.4 NOTE). New `CvarTable::cvt_deltas(axis_count,
  cvt_count, coords)` returns per-CVT deltas for an instance. On `Font`:
  `cvt_count` / `cvt_value(i)` expose the raw `cvt ` FWORD array;
  `has_cvar` / `cvar_table`; `cvt_deltas()` computes the current
  instance's deltas against the `avar`-bent normalised coords; and
  `cvt_value_varied(i)` returns the saturated varied CVT value. Seven
  `cvar` unit tests (header parse / version + EOF rejection / full +
  half + zero scalar interpolation / cvt-count padding) plus three
  integration tests against DejaVu Sans (real `cvt ` of 255 entries, no
  `cvar`) and InterVariable (no `cvt `).

- **CPAL v1 label arrays** (ISO/IEC 14496-22:2019 §5.7.11). The v1
  trailer's `paletteLabelArray` and `paletteEntryLabelArray` are now
  parsed alongside the previously-supported `paletteTypeArray`. New
  `CpalTable::palette_label(i)` returns the per-palette `name`-table ID
  (e.g. "Regular", "High Contrast") and
  `CpalTable::palette_entry_label(j)` the per-entry ID applied across
  all palettes (e.g. "Outline", "Fill"); both surface the `0xFFFF`
  "no label" sentinel as `None` (exported as `cpal::NO_NAME_ID`).
  Exposed on `Font` as `cpal_palette_label` / `cpal_palette_entry_label`.
  Four new unit tests cover present/sentinel/out-of-range labels and the
  v0 + types-only-no-labels degradations.

- **Ergonomic variation-instance API.** Selecting a design variant no
  longer requires the caller to find an axis index by hand. New
  `Font::axis_index(tag)` / `Font::axis_value(tag)` look an axis up by
  its four-byte tag (`*b"wght"`, `*b"wdth"`, …); `Font::set_axis_value(
  tag, value)` sets one axis (clamped to its range) leaving the others
  untouched, returning `false` for a static font or unknown tag; and
  `Font::apply_named_instance(index)` snaps the coordinates to a `fvar`
  named instance ("Bold", "Condensed Light", …). Four InterVariable.ttf
  integration tests (single-axis update + clamp + unknown-tag no-op,
  by-tag outline equals by-index outline, named-instance application +
  out-of-range no-op, static-font no-ops on DejaVu Sans).
- **`gvar` inferred-delta (IUP) interpolation for simple glyphs**
  (ISO/IEC 14496-22:2019 §7.3.4.4 "Inferred deltas for un-referenced
  point numbers"). Real variable fonts encode each `gvar` tuple with a
  *partial* point-number set — only the structurally significant points
  carry explicit deltas, and the rest are inferred along each contour.
  Previously un-referenced points stayed pinned at their default
  positions, shearing the varied outline; the new path completes every
  contour. New `GvarTable::glyph_deltas_iup(glyph_id, &SimpleOutlineInfo,
  coords)` takes the static glyph's contour structure + default grid
  coordinates and returns per-point `(dx, dy)` deltas with omitted
  outline points inferred. Inference runs **per region** on each tuple's
  *unscaled* deltas (before the tuple scalar is applied, exactly as the
  spec prescribes), implementing all §7.3.4.4 cases: equal-coordinate
  neighbours propagate a shared delta (or zero on disagreement);
  single-referenced contours adopt that one point verbatim; targets
  outside the neighbour coordinate range take the nearer neighbour's
  delta; targets between neighbours linear-interpolate by proportional
  position (fractional precision preserved until final rounding,
  matching the spec worked example's +10.5). Phantom points are never
  inferred. `Font::glyph_outline` now routes simple variable glyphs
  through this path, so an outline at any non-default instance is
  IUP-complete. New `SimpleOutlineInfo` type +
  `SimpleOutlineInfo::from_contours`. Eleven new gvar unit tests
  (per-axis inference cases, region propagation, the §7.3.4.4 worked
  example, end-to-end partial point sets, scalar scaling of inferred
  deltas, legacy-path equivalence when all points are referenced) plus
  three InterVariable.ttf integration tests proving the majority of a
  glyph's points move under a strong weight change on both axis signs
  and that the varied outline stays within its derived bounds.
- **`post`-table reverse glyph-name lookup.** New
  `PostTable::gid_for_name(name)` (surfaced as `Font::gid_for_glyph_name`)
  inverts `resolved_glyph_name` across every named glyph the table
  publishes — v2.0 custom Pascal strings and standard-Macintosh names
  alike (from v1.0, v2.0 with `glyphNameIndex < 258`, or v2.5) — and
  returns the lowest glyph id carrying that name (`None` for v3.0, an
  absent `post`, or an unknown name). A standard-name query resolves the
  target to its standard-Mac index once, so the per-glyph scan compares
  integers rather than strings. Companion `PostTable::named_glyph_count`
  (258 for v1.0; `numGlyphs` for v2.0/v2.5; 0 for v3.0) and
  `PostTable::iter_glyph_names` / `Font::iter_glyph_names` iterate every
  `(glyph_id, resolved_name)` pair, skipping ids with an unsatisfiable
  reference. Eight new unit tests (v1.0 full-set inversion, v2.0
  custom+standard, lowest-gid-wins on duplicate, v2.5 offset inversion,
  v3.0 empty, iter round-trip, unsatisfiable-Pascal skip) plus four
  DejaVu Sans integration tests (every-named-glyph round-trip,
  cmap-vs-post agreement on `'A'`, empty-name guard).
- **Lookup-flag-aware shaping.** New `GsubTable::lookup_flags` /
  `GposTable::lookup_flags` accessors (surfaced as
  `Font::gsub_lookup_flags` / `gpos_lookup_flags`) read the Lookup
  table's `lookupFlag` (offset +2): the low-byte skip filters
  (RIGHT_TO_LEFT `0x0001`, IGNORE_BASE_GLYPHS `0x0002`, IGNORE_LIGATURES
  `0x0004`, IGNORE_MARKS `0x0008`, USE_MARK_FILTERING_SET `0x0010`) and
  the high-byte `markAttachmentType` class. `Font::shape` now honours
  IGNORE_MARKS on ligature lookups: a ligature whose lookup sets the flag
  matches over the *non-mark* glyphs and removes only the consumed
  non-mark components, leaving interspersed combining marks in place to
  re-anchor during GPOS — while a lookup that does NOT set the flag stays
  spec-correctly blocked by an intervening mark. Two integration tests
  (`tests/shape.rs`) validate the accessor against DejaVu Sans (which
  ships ligature lookups both with and without IGNORE_MARKS) and the
  mark-blocks-non-ignore-marks-ligature invariant.
- **`Font::shape(text, script, lang, features)` — OpenType GSUB/GPOS
  shaping pipeline.** New `shape` module wiring the per-lookup-type GSUB
  substitution and GPOS positioning primitives into a single end-to-end
  entry point: text → `cmap` nominal glyphs → GSUB substitution stage →
  GPOS positioning stage → `Vec<ShapedGlyph>`. Each `ShapedGlyph`
  carries `glyph_id`, originating `cluster` (input byte index, preserved
  across ligature collapse and multiple-substitution expansion), and
  `(x_offset, y_offset, x_advance, y_advance)` in font design units (TT
  Y-up). Per the ISO/IEC 14496-22:2019 §6 common-table-format rules the
  union of lookups referenced by the active requested features is
  applied **in LookupList order** (not feature order), interleaving
  lookups from different features as the spec requires. The GSUB stage
  dispatches single / multiple / alternate / ligature / contextual /
  chained-context / reverse-chained-context substitution across the
  whole glyph buffer; the GPOS stage seeds advances from `hmtx` then
  layers single / pair (kerning) / cursive / mark-to-base /
  mark-to-ligature / mark-to-mark / contextual / chained-context
  positioning, accumulating placement and advance deltas. Uses the
  variation-instance-aware feature resolution
  (`gsub_features_for_script_at_instance` /
  `gpos_features_for_script_at_instance`) so variable-font
  FeatureVariations substitutions are honoured at the current instance.
  Three new per-lookup-index GPOS helpers
  (`GposTable::lookup_kerning_at`, `apply_mark_to_base_at`,
  `apply_mark_to_mark_at`) scope a pair / mark-to-base / mark-to-mark
  lookup to one resolved LookupList index (the whole-LookupList scans
  remain for non-shaper callers). Validated against DejaVu Sans (Latin
  kerning advance reduction + `liga` ligation, cluster monotonicity) and
  Noto Sans Arabic (`init`/`medi`/`fina` joining-form substitution +
  `mark` mark-to-base attachment with non-zero anchor offsets); 10 new
  integration tests in `tests/shape.rs`.
- `SVG ` table (ISO/IEC 14496-22:2019/Amd.1:2020 §5.5.1) — the fourth
  colour-glyph mechanism, carrying per-glyph-range SVG 1.1 vector
  documents. `SvgTable::parse` decodes the 10-byte header (`version`,
  `offsetToSVGDocumentList`, `reserved`) and the SVGDocumentList
  (`numEntries` + 12-byte `SVGDocumentRecord[]`), enforcing the §5.5.1
  invariants: `version == 0`, non-zero document-list offset, non-zero
  `numEntries`, `startGlyphID <= endGlyphID`, non-zero `svgDocOffset` /
  `svgDocLength`, the strictly-ascending-disjoint range ordering
  (`startGlyphID > previous endGlyphID`), and in-bounds document slices
  (`svgDocOffset` measured from the SVGDocumentList start). Document
  payloads are surfaced raw — plain UTF-8 markup or gzip-encoded, with
  `SvgDocument::is_gzip_encoded()` testing the §5.5.2 `0x1F 0x8B 0x08`
  magic — matching the raw-payload policy already used for `sbix` and
  `CBDT` image strikes; gzip inflation + XML parsing stay in the
  consumer renderer. `Font::has_svg()` / `Font::svg_table()` gate and
  expose the table; `Font::svg_document(gid)` binary-searches the range
  records to resolve the document covering a glyph. Shared documents
  (two records pointing at one payload for discontinuous ranges, §5.5.1
  NOTE) round-trip.

- `gvar` composite-glyph variation (ISO/IEC 14496-22:2019 §7.3.4.3):
  `Font::glyph_outline` now applies variation deltas to composite
  glyphs (previously only simple glyphs were retargeted). For a
  composite, gvar packed point numbers address the *components* plus
  the four phantom points rather than flattened outline points; the
  new `GvarTable::glyph_component_deltas` interpolates per-component
  `(dx, dy)` placement deltas and `GlyfTable::glyph_outline_var`
  (with `GlyfTable::composite_component_count`) folds each into the
  component's `argument1`/`argument2` X/Y offset — only for
  `ARGS_ARE_XY_VALUES` components, scaling the delta-adjusted offset
  under `SCALED_COMPONENT_OFFSET`. Each component glyph is re-decoded
  with its own gvar deltas applied before placement, per the spec's
  "most deeply-nested first" order. Phantom-point (metrics) deltas and
  the §7.3.4.4 simple-glyph-only IUP inference remain out of scope of
  this geometry path.

- `post` table — the 258 standard Macintosh glyph names
  (`STANDARD_MAC_GLYPH_NAMES: [&str; 258]` + the
  `standard_mac_glyph_name(index)` helper), transcribed verbatim from
  the now-staged `docs/text/opentype/post-standard-mac-glyph-names.md`
  (docs gap #1277 closed). `Font::glyph_name(gid)` and the new
  `PostTable::resolved_glyph_name(gid)` now resolve **both** the
  `Custom` (Pascal-string) and `StandardMac { index }` branches into a
  single `Option<&str>`, where previously the convenience accessor
  returned `None` for the `StandardMac` branch.

- `EBSC` embedded bitmap scaling table (ISO/IEC 14496-22:2019 §5.6.4):
  header + `BitmapScale[]` array with the shared §5.6.3.2
  `SbitLineMetrics` struct. `Font::has_ebsc()` / `Font::ebsc_table()` /
  `Font::ebsc_target_sizes()` introspect the table;
  `Font::glyph_gray_bitmap_scaled(gid, target_ppem)` redirects a
  requested ppem to the real `EBLC`/`EBDT` substitute strike and scales
  the per-glyph metrics by the §5.6.4 `target / substitute` ppem ratio
  (independent X/Y, nearest-integer-pixel rounding), passing the source
  pixel grid through unresampled.
- GPOS ScriptList / FeatureList walker (`Font::gpos_features_for_script`)
  plus version-1.1 FeatureVariations wiring
  (`Font::gpos_features_for_script_at_instance` /
  `Font::gpos_has_feature_variations`), reusing the shared §6.2.9
  substructure already driving GSUB. The new `GposFeature` struct
  resolves a positioning-feature tag to its lookup-index list.

## [0.1.7](https://github.com/OxideAV/oxideav-ttf/compare/v0.1.6...v0.1.7) - 2026-06-15

### Other

- decode FeatureVariations (§6.2.9) for variable-font feature substitution
- composite point-matching placement + scaled component offsets
- GPOS LookupType 7 (contextual positioning) — SequenceContext formats 1/2/3
- decode embedded monochrome + grayscale bitmap strikes
- formats 8 (mixed 16/32-bit) and 10 (trimmed array) — all 8 base subtable formats now decoded
- PCL 5 table parser (ISO/IEC 14496-22:2019 §5.7.7)
- metadata table parser + 'dlng'/'slng' accessors + ScriptLangTag splitter (ISO/IEC 14496-22:2019 §5.7.6)
- vertical device metrics table (ISO/IEC 14496-22:2019 §5.7.8)
- horizontal device metrics table (ISO/IEC 14496-22:2019 §5.7.2)
- drop release-plz.toml — use release-plz defaults across the workspace
- linear-threshold table (ISO/IEC 14496-22:2019 §5.7.4)
- full v1.0 / v2.0 / v2.5 / v3.0 structural decode + per-glyph PostScript name accessors (ISO/IEC 14496-22:2019 §5.2.10)
- grid-fitting and scan-conversion procedure table (ISO/IEC 14496-22:2019 §5.3.7)
- baseline table (ISO/IEC 14496-22:2019 §6.3.1)
- vertical origin table (ISO/IEC 14496-22:2019 §5.4.4)
- vhea + vmtx: vertical-layout metrics (ISO/IEC 14496-22:2019 §5.7.9 / §5.7.10)
- style-attributes table (axes + format 1/2/3/4 axis values + elided fallback)
- AttachList / LigCaretList / MarkAttachClassDef / MarkGlyphSetsDef / ItemVariationStore
- per-glyph vertical-metrics variations table (ISO/IEC 14496-22:2019 §7.3.8)
- per-glyph horizontal-metrics variations table (ISO/IEC 14496-22:2019 §7.3.5)
- font-wide metrics-variations table (ISO/IEC 14496-22:2019 §7.3.6)
- format 2 (high-byte mapping through table) for legacy CJK fonts

### Added — GSUB FeatureVariations (variable-font feature substitution) (2026-06-15)

GSUB now decodes the **FeatureVariations** substructure (ISO/IEC
14496-22:2019 §6.2.9), the mechanism a variable font uses to swap the
set of lookups behind a feature for an alternate set when the current
variation instance falls inside a normalised range on one or more
`fvar` axes (canonical use: optical-size- or weight-conditional
substitution).

- The GSUB header parse was extended to the **version 1.1** form: the
  `Offset32 featureVariationsOffset` after the three v1.0 offsets is
  read when `minorVersion >= 1` (v1.0 fonts and v1.1 fonts that ship no
  feature variations are unaffected; the offset is bounds-checked).
- New shared `tables::feature_variations` module decodes the whole
  §6.2.9 substructure:
  - **FeatureVariations** table (header + `FeatureVariationRecord[]`),
    with the first-match record-evaluation rule and the universal-match
    (zero `conditionSetOffset`) and no-substitution (zero
    `featureTableSubstitutionOffset`) special cases.
  - **ConditionSet** table — conditions are conjunctively AND-ed; an
    empty set matches all contexts.
  - **ConditionTableFormat1** (Font Variation Axis Range) — the only
    defined format; the F2DOT14 `[filterRangeMin, filterRangeMax]`
    inclusive range is tested against the axis's normalised value. An
    unrecognised condition format fails the set (§6.2.9
    forward-compatibility), and an axis index beyond the supplied
    coordinate vector degrades the record to non-matching.
  - **FeatureTableSubstitution** table — the sorted
    `FeatureTableSubstitutionRecord[]` with the §6.2.9 "first record
    with the index wins; stop on a higher index" lookup and the
    "reject this record on an unsupported substitution-table major
    version, continue to the next" rule. The alternate feature table
    keeps the default feature's tag.
- New public API:
  - `Font::gsub_features_for_script_at_instance(script, lang)` — like
    `gsub_features_for_script`, but applies the active FeatureVariations
    substitution at the font's current variation instance (evaluated
    against the existing avar-bent `Font::normalised_coords`). For every
    feature whose index is overridden, the returned `GsubFeature`
    carries the alternate lookup-index list while keeping the tag.
  - `Font::gsub_has_feature_variations()` — presence gate.
  - `GsubTable::features_for_script_at_coords` /
    `GsubTable::has_feature_variations` — the lower-level entry points.
- 7 unit tests in the new module (version rejection, zero-offset
  no-table, in-range / out-of-range / boundary substitution, missing
  axis coordinate, universal condition set, unsupported
  substitution-version skip) plus a new `tests/feature_variations.rs`
  integration suite (2 tests: a synthetic `wght` variable font that
  swaps `liga` lookups at high weight through the `Font` boundary, and
  a no-feature-variations baseline across the three bundled fixtures).

Out of scope this round: the identical substructure on the GPOS v1.1
header (GPOS has no `features_for_script` walker yet; the shared
decoder is ready to drive it when that lands).

Spec: ISO/IEC 14496-22:2019 §6.2.9 ("Feature variations") + the
version-1.1 GSUB header layout.

### Added — `glyf` composite point-matching + scaled component offsets (2026-06-14)

Two `glyf` composite-glyph placement mechanisms from the "Composite glyph
description" section that were previously approximated are now decoded to
spec:

- **Point-matching placement** (the `ARGS_ARE_XY_VALUES`-cleared form).
  `argument1` is now read as a point number in the parent glyph (the
  contours accumulated and re-numbered from previous components) and
  `argument2` as a point number in the child component. The child is
  transformed first (per "the transformation is applied to the child's
  point before the points are aligned"), then translated so child point
  `argument2` coincides with parent point `argument1`. A referenced index
  that lands on a phantom point — which would require `hmtx`/`vmtx`
  metrics not threaded through the outline resolver — degrades gracefully
  to zero-offset placement instead of dropping the component. Previously
  every point-matching component was placed at a flat `(0, 0)` offset.
- **`SCALED_COMPONENT_OFFSET` / `UNSCALED_COMPONENT_OFFSET`.** When the
  offset-vector form is used with a non-identity scale/transform, the
  `SCALED_COMPONENT_OFFSET` flag now applies the 2×2 transform to the
  `(x, y)` offset before it is added to the child points (the offset is in
  the component coordinate system); `UNSCALED_COMPONENT_OFFSET` and the
  recommended default-when-neither-set leave the offset untransformed (it
  is in the parent coordinate system). A font that sets both is treated as
  invalid and falls back to the unscaled default. Previously both flags
  were commented as "no effect on geometry" and the offset was always
  applied untransformed.

New `outline.rs` helpers (`TtOutline::transformed`, `flat_point`,
`append_translated`) support the point-matching path. Five new unit tests
pin the scaled/unscaled offset distinction, the point-alignment offset,
and the out-of-range/phantom-point fallback.

### Added — GPOS LookupType 7 (contextual positioning) (2026-06-14)

GPOS now decodes and applies **LookupType 7 (Contextual Positioning)**,
the non-chained sibling of the already-supported LookupType 8. It uses
the shared `SequenceContext` sub-table family from the OpenType Layout
Common Table Formats chapter (the GPOS-type-7 analogue of GSUB type 5):

- **Format 1** (`SequenceContextFormat1`) — Coverage on the first input
  glyph + per-coverage `SequenceRuleSet` of explicit input-glyph
  sequences, each carrying a `SequenceLookupRecord[]`.
- **Format 2** (`SequenceContextFormat2`) — Coverage gate + a `ClassDef`
  whose class of the first glyph selects a `ClassSequenceRuleSet`; the
  remaining positions match by class value.
- **Format 3** (`SequenceContextFormat3`) — one Coverage table per input
  position + a single `SequenceLookupRecord[]`.

On a match, each `SequenceLookupRecord { sequenceIndex, lookupListIndex }`
is recursively dispatched into the nested per-type positioning paths
(LT 1 / 2 / 3 / 4 / 6 / 7 / 8), reusing the existing chained-context
`apply_pos_records` machinery and `MAX_NESTED_LOOKUP_DEPTH` recursion
fence. ExtensionPos (LookupType 9) wrappers are unwrapped transparently.

New public API: `Font::gpos_apply_lookup_type_7` (plus the
`GposTable::apply_lookup_type_7` lower-level entry). Six new unit tests
cover all three formats (positive dispatch + no-match boundaries:
uncovered glyph, class mismatch, short window, wrong lookup type,
out-of-range index).

Spec: OpenType Layout Common Table Formats §"Sequence Context Format
1/2/3"; `GPOS` chapter §"Contextual positioning format 1/2/3".

### Added — EBDT / EBLC embedded monochrome + grayscale bitmaps (2026-06-13)

The font can now decode embedded *monochrome and grayscale* bitmap
strikes (ISO/IEC 14496-22:2019 §5.6.2 `EBDT` + §5.6.3 `EBLC`), the
non-colour counterpart to the already-supported `CBDT`/`CBLC` PNG
colour bitmaps.

- The location side (`EBLC`) reuses the existing `CblcTable` walker —
  it already accepted the `majorVersion == 2` `EBLC` header and all
  five IndexSubTable formats (1–5), so no new location code was
  needed; a separate `EBLC`/`EBDT` table pair is now wired into
  `Font`.
- New `EbdtTable` decodes the five bit-packed §5.6.2.2 image-data
  formats: format 1 (small metrics, byte-aligned), 2 (small metrics,
  bit-aligned), 5 (bit-aligned data only — metrics from the EBLC
  IndexSubTable 2/5 `BigGlyphMetrics`), 6 (big metrics, byte-aligned),
  7 (big metrics, bit-aligned). Pixels are unpacked MSB-first,
  left-to-right, top-to-bottom into a `width × height` row-major grid
  of one alpha-coverage byte per pixel; `bitDepth` 1 / 2 / 4 / 8
  (§5.6.3.1) is scaled to the full 0..=255 range. Byte-aligned formats
  pad each row to a byte boundary; bit-aligned formats pack the whole
  glyph contiguously.
- New public API: `Font::has_gray_bitmaps()`,
  `Font::gray_strike_sizes()`, `Font::glyph_gray_bitmap(gid,
  target_ppem) -> Option<GrayBitmap>` (closest-ppem strike selection,
  larger-ppem tie-break, matching the colour path), and the
  `GrayBitmap` struct.
- Out of scope this round: format 4 (compressed) and formats 8 / 9
  (composite) decode to `None`; `bitDepth == 32` (BGRA) routes to the
  `CBDT` path; `EBSC` (§5.6.4 scaled-strike substitution) is not yet
  decoded.

### Added — cmap subtable formats 8 and 10 (2026-06-11)

The character → glyph mapper now decodes ALL eight base cmap
subtable formats. New per the OpenType cmap chapter ("Format 8:
mixed 16-bit and 32-bit coverage" / "Format 10: Trimmed array"):

- **Format 8** — the discouraged UTF-16-oriented mixed-length
  layout: fixed 8208-byte header (u16 format/reserved, u32
  length/language, 8 KiB packed `is32` bit array, u32 `numGroups`)
  followed by format-12-style `SequentialMapGroup` records
  (`startCharCode` / `endCharCode` / `startGlyphID`, sequential
  glyph semantics, binary-searched). The `is32` array — bit test
  `is32[cp / 8] & (1 << (7 - cp % 8))` per the spec — is enforced
  as a validity filter in both directions: a 16-bit query whose own
  bit is set is the first half of a 32-bit code (not a character →
  miss), and a 32-bit query whose high word's bit is clear cannot
  exist under the font's encoding (→ miss).
- **Format 10** — the 32-bit trimmed-array analog of format 6: u32
  `startCharCode` + `numChars` bounding one dense u16
  `glyphIdArray[]` window; zero entries surface as missing glyphs
  exactly like format 6.
- **Picker ranking** — format 8 slots between 12 and 4 (it covers
  supplementary planes, so it outranks a BMP-only format 4 sibling,
  but the spec's standard 32-bit format 12 still wins); format 10
  slots between 4 and 6 (a single contiguous window is narrower
  coverage than a segmented BMP map, but richer than its 16-bit
  analog).

Seven new in-module tests: sequential round-trips across BMP +
supplementary groups, both `is32` gates, the 8-over-4 pick, the
trimmed-array round-trip with zero-entry misses, and single-format
pickability for both formats.

### Added — `PCLT` PCL 5 table parser (2026-06-10)

New [`tables::pclt::PcltTable`] decoder for the optional `PCLT`
(PCL 5) table per ISO/IEC 14496-22:2019 §5.7.7. The on-wire shape
is a fixed 54-byte struct ([`PCLT_TABLE_LEN`]): version pair,
`uint32 FontNumber`, the `Pitch` / `xHeight` / `CapHeight`
design-unit metrics, the packed `Style` / `TypeFamily` /
`SymbolSet` words, the `Typeface[16]` / `CharacterComplement[8]` /
`FileName[6]` fixed-size byte fields, and the `StrokeWeight` /
`WidthType` / `SerifStyle` classification bytes plus a trailing
`Reserved` pad. §5.7.7 deems the table "strongly discouraged for
OFF fonts with TrueType outlines"; it survives in legacy faces,
so the parser decodes it whenever present and leaves the
deprecation policy to the caller.

Typed accessors decode every packed field per the §5.7.7 prose:

- **FontNumber** — `font_number_is_native()` reads the
  most-significant native-vs-converted bit ("Only font vendors
  should create fonts with this bit zeroed"),
  `font_number_vendor_code()` the 7-bit HP-assigned vendor letter
  (`A` Adobe, `B` Bitstream, `C` Agfa, `H` Bigelow & Holmes,
  `L` Linotype, `M` Monotype), and
  `font_number_vendor_assigned()` the low 24 vendor-assigned bits.
- **Style** — `style_structure()` (bits 5–9, 0 = solid through
  17 = inverse with border), `style_width()` (bits 2–4),
  `style_posture()` (bits 0–1, 0 = upright / 1 = oblique-italic /
  2 = alternate italic), with the reserved top 6 bits surfaced
  through `style_reserved_bits()`.
- **TypeFamily** — `type_family_vendor_code()` (bits 12–15, the
  HP Boise Division vendor assignments) +
  `type_family_code()` (bits 0–11).
- **SymbolSet** — `symbol_set_number()` (top 11 bits) and
  `symbol_set_id()` implementing "the value of the least
  significant 5 bits, when added to 64, is the ASCII value of the
  symbol set 'ID' field"; all eight §5.7.7 example values
  round-trip in tests (629 → 19U, 394 → 12J, …).
- **Typeface / FileName** — trailing-pad-trimmed `&str` accessors
  with raw fixed-array fallbacks, plus `file_name_treatment()`
  for the fourth FileName byte (the `R` / `I` / `B` / `J` …
  treatment-flag character).
- **CharacterComplement** — `character_complement()` as a
  big-endian `u64`, `provides_collection(bit)` honouring the
  cleared-bit-means-provided polarity established by the spec's
  worked examples (Windows 3.1 "ANSI" = `0xFFFFFFFF37FFFFFE`
  clearing bits 31 / 30 / 27) and the "Symbol set bound fonts
  should have this field set to all F's (except bit 0)" rule,
  and `is_unicode_indexed()` reading bit 0 per "Bit 0 must always
  be cleared when the font elements are provided in Unicode
  order".
- **StrokeWeight / WidthType** — raw `i8` accessors with
  `stroke_weight_is_valid()` / `width_type_is_valid()` range
  checks against the §5.7.7 "Only values in the range -7 to 7
  are valid" / "-5 to 5" sentences ([`PCLT_STROKE_WEIGHT_RANGE`]
  / [`PCLT_WIDTH_TYPE_RANGE`]).
- **SerifStyle** — `serif_style_value()` (bottom 6 bits) +
  `serif_style_class()` (top 2 bits: 1 = Sans Serif/Monoline,
  2 = Serif/Contrasting).

`majorVersion != 1` is rejected as `BadStructure` per §5.7.7's
"The current PCLT table version is 1.0"; `minorVersion` and the
`Reserved` pad byte ("Should be set to zero") are surfaced raw.
[`Font::has_pclt`] and [`Font::pclt_table`] expose the parsed
table on the font boundary; integration tests cover the absent
path on all three bundled fixture fonts and a synthetic
TrueType-flavoured sfnt carrying a §5.7.7-example-shaped `PCLT`.

### Added — `meta` metadata-table parser + `'dlng'` / `'slng'` accessors + ScriptLangTag splitter (2026-06-08)

New [`tables::meta::MetaTable`] decoder for the optional `meta`
(metadata) table per ISO/IEC 14496-22:2019 §5.7.6. The on-wire
shape is a 16-byte header (`uint32 version`, `uint32 flags`,
`uint32 reserved`, `uint32 dataMapsCount`) followed by a
`DataMap[dataMapsCount]` array of `(Tag tag, Offset32 dataOffset,
uint32 dataLength)` records and the payload bytes themselves
(referenced from the DataMap offsets). The table is the
OpenType-level grab-bag for font-wide key/value metadata pairs
keyed by four-character ASCII tags.

The §5.7.6.2 tag-character class is enforced at parse time
through [`is_valid_meta_tag`]: tags begin with a letter
(`0x41..=0x5A` / `0x61..=0x7A`), use only letters / digits
(`0x30..=0x39`) / trailing spaces (`0x20`), and reject inner
spaces. Vendor-private tags (uppercase + digits per §5.7.6.2
paragraph 4) pass the same grammar so the parser does not need
a second pass for them.

The parser:

- enforces `version == 1` per §5.7.6.1 ("set to 1") and
  `flags == 0` per the spec's "currently unused" mandate;
- surfaces the `reserved` field through [`MetaTable::reserved`]
  rather than gating parsing on it — §5.7.6.1's NOTE acknowledges
  that legacy Apple TrueType fonts may carry a non-zero data
  offset there;
- validates that every `DataMap.dataOffset + dataLength` slice
  fits inside the on-wire `meta` byte range, rejecting any
  out-of-range entry as `BadStructure`;
- caps `dataMapsCount` at 1024 to match the directory-level cap
  on sfnt tables (a malformed table cannot allocate an
  arbitrarily large record vector);
- preserves document order in [`MetaTable::records`] — §5.7.6
  does not impose a sort order, and surfacing the on-wire order
  lets tooling round-trip the table without churn.

Two registered tags are first-class on the [`crate::Font`]
boundary:

- `'dlng'` (Design languages, §5.7.6.2) — exposed through
  [`Font::meta_design_languages`] which returns the payload as a
  UTF-8 string when present and well-formed (the spec restricts
  the encoding to Basic Latin / ASCII so UTF-8 validation is the
  appropriate filter).
- `'slng'` (Supported languages, §5.7.6.2) — exposed through
  [`Font::meta_supported_languages`] with the same UTF-8 contract.

[`Font::meta_record`] returns the first record whose tag equals
the supplied tag — honouring §5.7.6.1's closing paragraph ("If
only one record or value is permitted for a tag, then any
instances after the first may be ignored.") — and
[`Font::meta_table`] surfaces the full parsed table for callers
that want to walk every record themselves.

The §5.7.6.3 ScriptLangTag value format (`[language "-"] script
["-" region] *("-" variant) *("-" extension) ["-" privateuse]`)
is supported through a free-function helper [`script_lang_tags`]
that splits a comma-separated payload into the individual
[`ScriptLangTag`] values, trims surrounding whitespace, discards
empty fragments and non-ASCII fragments per the §5.7.6.3
"any ScriptLangTag value not conforming to these specifications
is ignored" rule, and rejects leading / trailing / doubled
hyphens (each of which would produce an empty subtag against the
spec's BNF). Deeper validation (IANA Language Subtag Registry,
ISO 15924 script subtags) is deliberately left to the caller —
those registries change on a cadence independent of the on-wire
format and pulling them into the parser would couple it to a
moving target.

Constants surfaced for callers / round-trippers:
[`META_VERSION_1`], [`META_HEADER_LEN`], [`META_DATA_MAP_LEN`],
[`META_TABLE_TAG`], [`META_TAG_DLNG`], [`META_TAG_SLNG`],
[`META_TAG_APPL`] (reserved — used by Apple), [`META_TAG_BILD`]
(reserved — used by Apple).

Coverage: 26 unit tests covering the §5.7.6.1 header invariants
(version / flags / reserved-tolerance / dataMapsCount cap /
truncated-array rejection / out-of-range payload rejection /
offset + length overflow rejection), the §5.7.6.2 tag character
class (letter-led / inner-space rejection / non-alphanumeric
rejection / short-tag-with-trailing-space acceptance),
shared-payload aliasing between two records, document-order
preservation, and the §5.7.6.3 ScriptLangTag splitter across
the worked-example patterns (`Latn`, `Latn, Cyrl, Grek`,
`sr-Cyrl, en-Dsrt, Hant-HK`), plus its rejection rules. 5
integration tests covering the absent path across the four
shipped fixtures (DejaVu Sans Mono / DejaVu Sans / Inter
Variable / Noto Sans Arabic — none ship a `meta` table per
§5.7.6 "optional") and a synthesised minimal TrueType font that
carries `'dlng'` + `'slng'` + a vendor-private record, exercising
the round-trip through the [`crate::Font`] accessors and
confirming that a version-2 header is caught at
`Font::from_bytes` rather than silently surfaced.

### Added — `VDMX` table parser + per-(ppem, aspect-ratio) y-extent accessors (2026-06-08)

New [`tables::vdmx::VdmxTable`] decoder for the optional `VDMX`
(vertical device metrics) table per ISO/IEC 14496-22:2019 §5.7.8.
The on-wire shape is a 6-byte header (`uint16 version`,
`uint16 numRecs`, `uint16 numRatios`) followed by a
`RatioRange[numRatios]` aspect-ratio selector array, a parallel
`Offset16[numRatios]` array binding each ratio to a VDMX group,
and the VDMX groups themselves: each group is a 4-byte
`(recs, startsz, endsz)` header followed by a sorted
`vTable[recs]` array of `(yPelHeight, yMax, yMin)` tuples giving
the font-wide vertical pel envelope at each recorded ppem.

`VDMX` is the precomputed-vertical-envelope counterpart to `hdmx`'s
precomputed-advance-width table: where `hdmx` records each glyph's
grid-fit advance at a fixed set of ppem sizes, `VDMX` records the
font-wide `(yMax, yMin)` extent at a (possibly per-aspect-ratio) ppem
set so a rasteriser can pick a glyph-row bitmap height without
grid-fitting every glyph in the font. §5.7.4 lists both as the
precomputed-data solution to the speed problem `LTSH` addresses via
its linear-scaling threshold; with this round, all three landed in
the crate.

The parser:

- accepts both versions 0 and 1 (the `bCharSet` semantics differ
  but the numeric layout is identical; the raw byte is surfaced
  through `RatioRange::char_set` for the caller to interpret per
  its `VdmxTable::version_raw()`);
- enforces strict-monotonic-increase on each group's `yPelHeight`
  per §5.7.8 "sorted by yPelHeight" so a duplicate or out-of-order
  record cannot silently shadow a later lookup;
- validates the §5.7.8 sentinel rule: a `(xRatio=0, yStartRatio=0,
  yEndRatio=0)` RatioRange record may only appear as the last
  entry ("if present, this must be the last Ratio group in the
  table"); a sentinel anywhere else is rejected as `BadStructure`;
- canonicalises shared groups so two ratios pointing at one on-wire
  group resolve to one parsed `VdmxGroup`, while the per-ratio
  mapping is preserved through `group_for_ratio_index`;
- rejects a zero `Offset16` entry as `BadStructure` so a corrupted
  ratio array cannot alias to the table header.

`Font` exposes the table through:

- `Font::has_vdmx() -> bool`
- `Font::vdmx_table() -> Option<&VdmxTable>` — full table for
  callers that want to walk the groups themselves;
- `Font::vdmx_y_extent_for_device(ppem: u16, device_x_ratio: u8,
  device_y_ratio: u8) -> Option<(i16, i16)>` — runs the §5.7.8
  first-match RatioRange walk and returns the matched group's
  `(yMax, yMin)` at the requested ppem, or `None` for a
  non-matching device (no `(0,0,0)` sentinel) or an unrecorded
  ppem (§5.7.8 has no nearest-neighbour fallback);
- `Font::vdmx_y_extent_square(ppem: u16) -> Option<(i16, i16)>` —
  convenience shortcut for the common 1:1 lookup.

§7.3.5 forbids `VDMX` in variable fonts; the parser still decodes
the table whenever present, matching the `hdmx` policy. Callers
can cross-check `Font::is_variable()` if they want to honour the
spec.

`yPelHeight` is `uint16`, so §5.7.8's closing note about per-record
ppem reaching 65535 is honoured even though the RatioRange's
`uint8` bracketing caps at 255 (the ratio array is only consulted
for the per-ratio selector — the ppem-keyed lookup is unaffected).

The `tables::vdmx` module ships 14 unit tests covering version
acceptance, short-header / unknown-version / zero-count rejection,
truncated offset / vTable bodies, the strict-monotonic sort
invariant, the sentinel-position rule, shared-group canonicalisation,
high-`yPelHeight` records, and the conceptual range-check predicate.
A new `tests/vdmx.rs` integration suite ships 6 tests covering the
"DejaVu Sans Mono / DejaVu Sans / Inter ship no `VDMX`" baseline
plus a synthetic-font round-trip through `Font::vdmx_y_extent_*`,
including the sentinel-catches-non-1:1 path and the
zero-offset-rejected path.

Spec: ISO/IEC 14496-22:2019 §5.7.8 ("VDMX – Vertical device
metrics"), with cross-references to §5.7.4 (Linear threshold table)
and §7.3.5 (HVAR — variable-font metrics replacement).

### Added — `hdmx` table parser + per-(glyph, ppem) device-metrics accessors (2026-06-08)

New [`tables::hdmx::HdmxTable`] decoder for the optional `hdmx`
(horizontal device metrics) table per ISO/IEC 14496-22:2019 §5.7.2.
The on-wire shape is an 8-byte header (`uint16 version`,
`int16 numRecords`, `int32 sizeDeviceRecord`) followed by
`numRecords` device records, each carrying `uint8 pixelSize` +
`uint8 maxWidth` + `uint8 widths[numGlyphs]` and padded with zeros
to the long-word-aligned per-record stride declared in
`sizeDeviceRecord`. Each record publishes the grid-fitted
integer-pixel advance widths of every glyph at one selected ppem
size, so a rasteriser at one of the recorded sizes can short-
circuit scan-converting to learn an advance width.

`hdmx` is the precomputed-advance counterpart to `LTSH`: where
`LTSH` records the threshold ppem at which the grid-fit advance
converges with the rounded linear advance (`yPels[]`), `hdmx`
records the actual integer-pixel advance at a handful of fixed ppem
sizes. §5.7.4 names the two tables as complementary speed-up
mechanisms for the same problem; both now decode in this crate.

The parser:

- cross-checks each record's `widths[]` length against
  `maxp.numGlyphs` per §5.7.2's "numGlyphs is from the 'maxp' table" —
  an undersized `sizeDeviceRecord` is rejected as
  `Error::BadStructure` rather than silently walking off the end of
  the per-glyph array;
- honours `sizeDeviceRecord` as the per-record stride so a font that
  long-aligns aggressively past the minimum-spec body still decodes,
  with the unknown trailing bytes ignored;
- enforces §5.7.2's "This table is sorted by pixel size" with a
  strict-monotonic `pixelSize` check, so corrupted records cannot
  shadow each other under per-ppem lookup;
- rejects negative `numRecords` and unknown header versions.

New public surface on [`Font`]:

- [`Font::has_hdmx`] — presence test.
- [`Font::hdmx_table`] — borrow the parsed table.
- [`Font::hdmx_advance_pixels`] — per-`(glyph_id, ppem)` accessor
  returning the recorded integer-pixel advance. §5.7.2 has no
  "nearest-neighbour" rule — an unrecorded ppem returns `None` and
  the caller falls back to scan-converting.
- [`Font::hdmx_recorded_ppem_sizes`] — `Vec<u8>` of recorded ppem
  values in ascending order.

[`HdmxTable`] further exposes [`HdmxTable::record_for_ppem`]
(binary-search lookup) and the [`HdmxRecord`] accessor trio
[`HdmxRecord::widths`] / [`HdmxRecord::max_width`] /
[`HdmxRecord::pixel_size`] so callers walking the device-record
array can introspect each entry.

`hdmx` is forbidden by §7.3.5 in variable fonts; we parse it
whenever it is present and leave the cross-check to the caller
(`Font::is_variable`).

Tests cover the empty-record table, single-record tables, multi-
record tables with the recommended long-aligned stride, the §5.7.2
sort-violation rejection, the under-sized-stride rejection, the
mismatched-`numGlyphs` rejection at the `Font` parse path, and the
exact-ppem-only lookup semantics against an absent-`hdmx` fixture
trio (DejaVu Sans Mono / DejaVu Sans / Inter).

### Added — `LTSH` table parser + per-glyph linear-threshold accessors (2026-06-07)

New [`tables::ltsh::LtshTable`] decoder for the optional `LTSH` (linear
threshold) table per ISO/IEC 14496-22:2019 §5.7.4. The on-wire shape is
a 4-byte header (`uint16 version`, `uint16 numGlyphs`) followed by a
`uint8 yPels[numGlyphs]` array recording the lowest pixel-per-em size at
which each glyph's grid-fitted advance width has converged on the
rounded linear advance — i.e. the threshold at which a rasteriser may
round the design-unit advance arithmetically without scan-converting.
The §5.7.4 sentinel `1` (`LTSH_ALWAYS_LINEAR`) flags glyphs without
instructions on their sidebearings (always linear at every ppem).

The parser cross-checks `LTSH.numGlyphs` against `maxp.numGlyphs` per
the §5.7.4 invariant so a truncating or over-reading mismatch is
rejected as `Error::BadStructure` rather than silently corrupting
per-glyph lookups. Unknown header versions are likewise rejected;
trailing pad bytes (sfnt records align to 4-byte boundaries) are
tolerated.

New public surface on [`Font`]:

- [`Font::has_ltsh`] — presence test.
- [`Font::ltsh_table`] — borrow the parsed table.
- [`Font::ltsh_threshold`] — per-glyph `Option<u8>` accessor returning
  the recorded threshold ppem.
- [`Font::ltsh_linearly_scales_at_ppem`] — `bool` predicate honouring
  the §5.7.4 `ppem >= yPels[gid]` inequality; returns `false` when
  the glyph is below threshold, when `glyph_id` is out of range, or
  when the font ships no `LTSH` table (in which case §5.7.4 prescribes
  grid-fitting).

Table-level helpers exposed on [`LtshTable`]:

- [`LtshTable::is_always_linear`] — typed check for the §5.7.4 `yPels =
  1` sentinel.
- [`LtshTable::all_always_linear`] — short-circuit predicate for the
  common "every glyph linear at every ppem" case.
- [`LtshTable::linear_threshold`] / [`LtshTable::y_pels`] /
  [`LtshTable::num_glyphs`] / [`LtshTable::version_raw`].

`LTSH` is the third of three complementary methods §5.7.4 cites for
side-stepping the small-ppem grid-fit speed problem; the table is a
hint for shapers that want to short-circuit advance-width scan-convert
at large ppem and grid-fit at small ppem. `hdmx` (precomputed
horizontal advances at selected ppem sizes) and `vdmx` (vertical
analogue) remain out-of-scope.

### Added — `post` table v1.0 / v2.0 / v2.5 / v3.0 structural decode + per-glyph PostScript name accessors (2026-06-06)

Extended the previously header-only `post` decoder to cover every
OpenType-published format per ISO/IEC 14496-22:2019 §5.2.10 / MS Learn
`otspec-post`. The fixed 32-byte common header now exposes
`min_mem_type{42,1}` / `max_mem_type{42,1}` alongside the existing
italic-angle / underline / `isFixedPitch` fields, and the trailing
data is decoded into a typed [`tables::post::PostFormat`] enum:

- `Version10` — no trailing data. The font asserts the standard
  Macintosh 258-glyph order; per-glyph name lookup yields
  [`GlyphNameRef::StandardMac { index: gid }`] for `gid < 258`.
- `Version20(PostV20)` — `uint16 numGlyphs` + `uint16
  glyphNameIndex[numGlyphs]` + Pascal-format `stringData[…]`. The
  index array is preserved verbatim; the Pascal string pool is split
  into a `Vec<String>` indexed by `(nameIndex - 258)` per §5.2.10.2.
  Names are kept as UTF-8 (the §5.2.10.2 "ASCII only" rule is a
  conformance check, not a hard parse rule). Two diagnostic flags —
  `has_oversize_glyph_name` (any name longer than the §5.2.10.2
  recommended 63-byte cap) and `has_non_conformant_glyph_name` (any
  byte outside the §5.2.10.2 allow-set `A..Z` / `a..z` / `0..9` / `.`
  / `_`) — surface conformance problems without rejecting the table.
- `Version25(PostV25)` — `uint16 numGlyphs` + `int8 offset[numGlyphs]`.
  Per §5.2.10.3 the standard-Macintosh index is `gid + offset[gid]`;
  the accessor returns `StandardMac { index }` for results in
  `[0, 258)` and `None` for malformed offsets that overflow that range.
- `Version30` — no trailing data, no glyph names. Accepted as the
  required form for CFF v1 outline fonts (§5.2.10.4).

Spec v4.0 (Apple-only, "not supported in OpenType" per §5.2.10) is
rejected as `Error::BadStructure`.

New typed accessor [`GlyphNameRef`] separates the two
naming-source branches the v2.0 layout produces:

- `StandardMac { index }` — the glyph's name is the `index`th entry
  of the 258-name standard Macintosh glyph set.
- `Custom(&str)` — the glyph's name is a font-supplied Pascal string,
  already trimmed of its length byte.

New public surface on [`Font`]:

- [`Font::has_post`] — presence test.
- [`Font::post_table`] — borrow the parsed table.
- [`Font::glyph_name_ref`] — per-glyph `GlyphNameRef` lookup; the
  low-level accessor that surfaces both `StandardMac` and `Custom`
  branches so tooling that ships its own 258-name array can resolve
  names today.
- [`Font::glyph_name`] — convenience `Option<&str>` accessor. Returns
  the Pascal string for the `Custom` branch and `None` for the
  `StandardMac { index }` branch (see the gap statement below).

Known open: #1277 — the 258 standard Macintosh glyph names list,
which ISO §5.2.10.1 and MS Learn `otspec-post` both delegate to
Apple's TrueType Reference Manual Chapter 6 `post` Format 1
(`RM06/Chap6post.html`), is the sole canonical source and is not yet
staged in `docs/text/opentype/`. Until the list lands the decoder
publishes the index without the name; the convenience
[`Font::glyph_name`] therefore returns `None` for glyphs whose name
would have come from the standard Macintosh set, and `Some(name)`
for the Pascal-string branch. The structure-decoder layout below is
the foundation a follow-up commit plugs the staged 258-name array
into — a single one-line route through `STANDARD_MAC_GLYPH_NAMES[i]`
inside the `StandardMac { index }` branch closes the gap once the
list is staged.

19 new unit tests in `src/tables/post.rs` cover the four version
codepoints, the §5.2.10.2 worked example
(`glyphNameIndex[302] = 217` → standard 217; `glyphNameIndex[408] =
262` → fifth Pascal string), the §5.2.10.3 worked example (font
glyphs 0 / 1 / 2 → standard 36 / 37 / 38 via three `+36` offsets),
zero-based Pascal-pool indexing, truncated-pascal-string rejection,
oversize + non-conformant flag propagation, in-range / negative /
out-of-range v2.5 offsets, the v3.0 short header, and Apple v4.0
rejection. 5 new integration tests in `tests/post.rs` validate the
end-to-end accessor against the DejaVu Sans / DejaVu Sans Mono
fixtures: `numGlyphs` agreement with `maxp`, at-least-one custom
glyph name, italic + monospace metadata, and out-of-range gid
returning `None`.

Spec coverage:
`docs/text/opentype/spec/ISO_IEC_14496-22-OFF-2019.pdf` §5.2.10
(`post` header + version layouts);
`docs/text/opentype/otspec-post.html` (the §5.2.10.2 v2.0 worked
example + the §5.2.10.2 ASCII-conformance allow-set).

### Added — `gasp` grid-fitting / scan-conversion table (2026-06-05)

Decoded the optional `gasp` table per ISO/IEC 14496-22:2019 §5.3.7.
The 4-byte header (`uint16 version` ∈ `{0, 1}`, `uint16 numRanges`) is
followed by `numRanges` `GaspRange` records, each
`(uint16 rangeMaxPPEM, uint16 rangeGaspBehavior)`. Records carry the
four §5.3.7 `RangeGaspBehavior` flags:

 - `GASP_GRIDFIT` (bit 0) — recommend grid-fitting (hinting).
 - `GASP_DOGRAY` (bit 1) — recommend grayscale rendering.
 - `GASP_SYMMETRIC_GRIDFIT` (bit 2) — recommend ClearType symmetric
   grid-fitting; meaningful only in a version-1 table.
 - `GASP_SYMMETRIC_SMOOTHING` (bit 3) — recommend multi-axis
   ClearType smoothing; meaningful only in a version-1 table.

§5.3.7 invariants enforced at parse time:
 - `version ∈ {0, 1}`; anything else is `Error::BadStructure`.
 - The `gaspRange[]` array must be sorted by strictly increasing
   `rangeMaxPPEM` (duplicates rejected — a duplicate would shadow the
   later record's behaviour). The §5.3.7 "sorted by ppem" rule rules
   out both descending and equal-key orderings.
 - The slice carries `4 + 4 * numRanges` bytes; truncation returns
   `Error::UnexpectedEof`.

Reserved bits (`0xFFF0` per §5.3.7 "Reserved flags – set to 0") are
preserved on `GaspRange.flags` and surfaced through
[`tables::gasp::GaspRange::reserved_bits`]; consumers can introspect
them while typed accessors (`gridfit()`, `dogray()`,
`symmetric_gridfit()`, `symmetric_smoothing()`) cover the defined set.

The `0xFFFF` sentinel `rangeMaxPPEM` from §5.3.7 (final-record
"all sizes ≥ previous limit + 1" marker) is exposed as
[`tables::gasp::GASP_PPEM_SENTINEL`]; the
[`tables::gasp::GaspTable::covers_all_sizes`] predicate detects the
single-sentinel-record shortcut the spec describes ("If the only entry
in 'gasp' is the 0xFFFF sentinel value, the behavior described will
be used for all sizes.").

New public surface on [`Font`]:
 - [`Font::has_gasp`] — presence test.
 - [`Font::gasp_table`] — borrow the parsed table.
 - [`Font::gasp_behavior_for_ppem`] — pick the first record whose
   `rangeMaxPPEM` is at least the requested ppem, returning `None`
   when the font ships no `gasp` table or every record's upper limit
   sits below the requested ppem (the caller should then apply the
   rasteriser default per §5.3.7).

The §5.3.7 MVAR coupling (value tags `gsp0`..`gsp9` adjusting the
`rangeMaxPPEM` of up to ten ranges in a variable font) is documented
on the module; the static (default-instance) values returned here are
the foundation a caller folds the
`Font::metric_variation_delta(tag)` deltas into before lookup. The
last record's `rangeMaxPPEM` (commonly `0xFFFF`) is never adjusted
per the spec.

10 new unit tests in `src/tables/gasp.rs` cover the §5.3.7 sample
table (version 1, four ranges), the single-sentinel-covers-all-sizes
shortcut, sorted-array rejection (descending + duplicate keys),
unrecognised version rejection, short-header / truncated-record EOF,
reserved-bits tolerance, behaviour-fall-off-end semantics, and the
4-byte tag-constant sanity check.

### Added — `BASE` baseline table (2026-06-04)

Decoded the optional Baseline table per ISO/IEC 14496-22:2019 §6.3.1.
The header (v1.0 — 8 bytes: `majorVersion = 1`, `minorVersion = 0`,
`horizAxisOffset`, `vertAxisOffset`; v1.1 — adds an `Offset32
itemVarStoreOffset` for variable-font BaseCoord deltas) is followed by
two layout-direction Axis trees. The HorizAxis carries Y coordinates
(horizontal text); the VertAxis carries X coordinates (vertical text).
Either offset may be `0`. Per §6.3.1.1, v1.1 fonts may publish an
`ItemVariationStore` consumed by `BaseCoordFormat3` VariationIndex
references; we bounds-check the offset and surface the IVS bytes
through the table so the shared
[`crate::tables::mvar::ItemVariationStore`] decoder consumes them on
demand.

Each Axis references:
 - a `BaseTagList` enumerating the §6.4.4 baseline identification
   tags (`romn`, `ideo`, `hang`, `icfb`, …) in the alphabetical order
   §6.3.1.3 mandates, and
 - a `BaseScriptList` enumerating the scripts rendered in the layout
   direction in `baseScriptTag` alphabetical order. Each script maps
   to a `BaseScript` table holding an optional `BaseValues` (default
   baseline index + one `BaseCoord` per `BaseTagList` entry, in the
   same order) plus an optional default `MinMax` (script-wide
   min/max extents and `FeatMinMaxRecord` overrides) and an array
   of `BaseLangSysRecord` entries that override extents for specific
   language systems.

`BaseCoord` decodes all three §6.3.1.3 formats:
 - **Format 1** — design-unit `coordinate` only (4 bytes).
 - **Format 2** — `coordinate` + `referenceGlyph` + `baseCoordPoint`
   for hinted size-dependent adjustment (8 bytes).
 - **Format 3** — `coordinate` + a Device-table (non-variable font)
   or VariationIndex (variable font) offset (6 bytes); the offset is
   surfaced for the caller's §6.2.8 Device decoder.

§6.3.1.3 invariants enforced at parse time:
 - `majorVersion == 1`; `minorVersion ∈ {0, 1}`.
 - Every Axis must publish a `baseScriptListOffset` (the only
   `Offset16` in the Axis table without an explicit "may be NULL"
   note).
 - Every `BaseLangSysRecord.minMaxOffset` must be non-zero (the
   spec lists "Offset to MinMax table" without a NULL fallback).
 - Every `BaseValues.baseCoords[i]` offset must be non-zero (one
   BaseCoord per BaseTagList entry; the parallel-array contract has
   no holes).
 - All sub-table offsets land inside the BASE slice. Truncated or
   overflowing arrays return `UnexpectedEof` / `BadStructure`.
 - v1.1 `itemVarStoreOffset` is bounds-checked against the slice.

New public surface on [`Font`]:

- `has_base()` — the table is present in the directory.
- `base_table()` — borrow the parsed table; access the HorizAxis /
  VertAxis trees, the v1.1 ItemVariationStore bytes, and the
  re-exported `BaseAxisTable::base_script_for_tag` /
  `baseline_index_for_tag` accessors.
- `base_horiz_y_for_script_baseline(script_tag, baseline_tag)` —
  walks the HorizAxis to surface the design-unit Y coordinate for a
  (script, baseline) pair. Returns `None` when the font has no BASE,
  the HorizAxis is absent, the script is not in the BaseScriptList
  (§6.3.1.3 "the text-processing client will render the script using
  the layout information specified for the entire font"), or the
  baseline tag is not in the Axis's BaseTagList.
- `base_vert_x_for_script_baseline(script_tag, baseline_tag)` —
  mirror for the VertAxis (X coordinates, vertical layout).

The integration suite covers five end-to-end paths:

- DejaVu Sans Mono / DejaVu Sans / Inter (no BASE) — every accessor
  returns `None`.
- Synthesised HorizAxis-only sfnt with the §6.3.1.4 dominant-run
  example shape (two scripts, two baselines): `latn` defaults to
  `romn`/0 with `ideo` = -120; `hani` defaults to `ideo`/0 with
  `romn` = +120. Per-script accessors return the correct coordinate
  for either baseline; unknown tags decline cleanly.
- Synthesised sfnt that publishes both HorizAxis (`latn`/`romn`) and
  VertAxis (`hani`/`ideo`): the two axes carry independent
  BaseScriptList arrays and the horizontal / vertical accessors walk
  them in isolation.

The §6.3.1.3 Device-table payload referenced from `BaseCoordFormat3`
in non-variable fonts and the v1.1 `ItemVariationStore` decoding for
`VariationIndex` references in variable fonts are surfaced through
offsets only — the shared `oxideav_ttf::tables::mvar::ItemVariationStore`
decoder (already used by HVAR / VVAR / MVAR) consumes the IVS bytes
when the variable-font layer needs per-instance baseline deltas.

Known open: #1277 Apple TrueType Reference Manual Chap6post (the
258 standard Macintosh glyph names for the `post` v2.0 / v2.5
mapping table) is still pending; bumped from r230 → r233.

Round 233. Spec coverage:
`docs/text/opentype/spec/ISO_IEC_14496-22-OFF-2019.pdf` §6.3.1 (BASE
table organization + structure + the v1.1 `itemVarStoreOffset`
trailer + three BaseCoord formats + MinMax / BaseLangSysRecord /
FeatMinMaxRecord layouts); `docs/text/opentype/registries/baseline-tags.html`
for the HorizAxis / VertAxis tag semantics (`romn`, `ideo`, `hang`,
`icfb`, `icft`, …) consulted only as registry context.

### Added — `VORG` vertical origin table (2026-06-04)

Decoded the optional vertical origin table per ISO/IEC 14496-22:2019
§5.4.4. The header (8 bytes: `majorVersion = 1`, `minorVersion = 0`,
`defaultVertOriginY: int16`, `numVertOriginYMetrics: uint16`) is
followed by a sorted array of `(glyphIndex: uint16, vertOriginY: int16)`
overrides. Per §5.4.4 the parser enforces:

- `majorVersion == 1` and `minorVersion == 0` (rejected otherwise as
  `BadStructure`).
- The metrics array is strictly increasing by `glyphIndex` ("must be
  sorted by increasing glyphIndex, and must not have more than one
  element with the same glyphIndex"). Duplicate or out-of-order
  entries fail at parse time so per-glyph queries can binary-search
  without revalidating.
- A truncated array (length field claims more entries than the slice
  provides) returns `UnexpectedEof`.

Per-glyph lookup follows the §5.4.4 fallback: an entry hit returns its
`vertOriginY`; a miss returns `defaultVertOriginY` ("glyphs whose
vertical origin's y coordinate equals defaultVertOriginY will not have
an entry in this array"). The empty-metrics size-optimised form
(every glyph at the default) parses as expected — a complete VORG of
8 bytes.

New public surface on [`Font`]:

- `has_vorg()` — the table is present in the directory.
- `vorg_table()` — borrow the parsed table (header + sorted overrides
  array via `metrics()` / `metrics_len()`).
- `vorg_default_vert_origin_y()` — convenience accessor for the §5.4.4
  fallback Y.
- `vert_origin_y_from_vorg(gid)` — the §5.4.4 lookup with the
  ignore-on-TrueType policy applied at the `Font` boundary: returns
  `None` whenever a `glyf` table is present, per §5.4.4 "If present in
  TrueType OFF fonts it must be ignored by font clients, just as any
  other unrecognized table would be." TrueType callers should keep
  using `glyph_vertical_origin_y` (§5.7.10 derivation from
  `topSideBearing + glyf.yMax`).

The integration suite covers four end-to-end paths:

- DejaVu Sans Mono / DejaVu Sans (no `VORG`) — every accessor returns
  `None`.
- TrueType-flavoured synthesised sfnt that carries a `VORG` table
  alongside `glyf`: the table is parsed (so tooling can introspect it)
  but `vert_origin_y_from_vorg` declines to consult it for every
  glyph, honouring the §5.4.4 ignore rule.
- CFF-flavoured synthesised sfnt (no `glyf`) carrying the spec's
  §5.4.4 worked-example table: `vert_origin_y_from_vorg` surfaces
  `defaultVertOriginY = 880` for unknown glyphs and the per-glyph
  overrides for the three populated entries.

The new accessor pairs with the previously-landed `VVAR` (§7.3.8)
`vorg_variation_delta` path: a CFF2 variable font caller adds the
variation delta to the new `vert_origin_y_from_vorg(gid)` baseline to
obtain the per-instance origin Y. TrueType variable fonts continue to
derive the origin from `vmtx.topSideBearing` + the gvar-deltad glyph
bbox per §5.7.10.

Round 230. Spec coverage: `docs/text/opentype/spec/ISO_IEC_14496-22-OFF-2019.pdf`
§5.4.4 (VORG header + vertOriginYMetrics format + the
TrueType-ignore policy).

### Added — `vhea` + `vmtx` vertical metrics (2026-06-04)

Decoded the vertical header (`vhea`, ISO/IEC 14496-22:2019 §5.7.9) and
vertical metrics (`vmtx`, §5.7.10) tables. Both versions of the
`vhea` header — v1.0 (`0x00010000`, with `ascent` / `descent` /
`lineGap` and "Reserved; set to 0" for `lineGap`) and v1.1
(`0x00011000`, with the renamed `vertTypoAscender` /
`vertTypoDescender` / `vertTypoLineGap` ideographic-em-box typographic
fields) — share the same 36-byte layout per §5.7.9 and parse
transparently; `VheaTable::version_raw()` / `is_v1_1()` expose the
distinction for callers that need to surface it. The 4 × `int16`
reserved fields and `metricDataFormat` are tolerated as non-zero per
the surrounding tables' permissiveness.

The `vmtx` table mirrors the `hmtx` two-array shape: a leading
`(advanceHeight: uint16, topSideBearing: int16)` pair array of
`vhea.numOfLongVerMetrics` entries followed by an optional bare
`int16[]` tail of top-side-bearings for monospaced trailing glyphs
(§5.7.10: "all the glyphs in this array shall have the same advance
height as the last entry in the vMetrics array"). The clamp matches
the `hmtx` idiom.

New public surface on [`Font`]:

- `has_vertical_metrics()` — both tables present.
- `vhea_table()` / `vmtx_table()` — raw parsed tables.
- `vertical_ascent()` / `vertical_descent()` / `vertical_line_gap()` —
  the §5.7.9 first three int16 fields (v1.1 naming).
- `advance_height_max()` — `vhea.advanceHeightMax` (int16 per the
  §5.7.9 v1.0 / v1.1 rows).
- `glyph_advance_height(gid)` / `glyph_top_side_bearing(gid)` —
  per-glyph metrics with the §5.7.10 monospaced-tail clamp.
- `glyph_vertical_origin_y(gid)` — derived Y coordinate of the glyph's
  vertical origin per §5.7.10 "Vertical Origin and Advance Height"
  (`topSideBearing + glyph_bounding_box.y_max`); returns `None` for
  empty / blank glyphs or fonts lacking `glyf`/`loca`.

A vhea-without-vmtx (or vmtx-without-vhea) font is now rejected with
`BadStructure` per §5.7.10 ("OFFvertical fonts require both").

This pairs with the previously-landed `VVAR` (§7.3.8) so the full
vertical-variation flow becomes consumable: callers add the
`advance_height_variation_delta(gid)` adjustment to the new
`glyph_advance_height(gid)` to obtain the per-instance advance.

Integration coverage:
- DejaVu Sans Mono / DejaVu Sans (no `vhea` / `vmtx`) — every
  vertical accessor returns `None` and `has_vertical_metrics()` is
  `false`.
- NotoSansCJK-Medium TTC subfont 0 (when the consumer-crate
  fixture-helper has populated the cached blob; the test
  silently skips otherwise) — full vertical-metric surface
  populated; sanity bounds on ascender / descender /
  advanceHeightMax; at least one non-zero advance height across
  a sampled glyph run.

### Added — `STAT` style attributes table (2026-06-03)

Decoded the `STAT` table per ISO/IEC 14496-22:2019 §7.3.7. Three
top-level pieces:

- **Header** — v1.0 / v1.1 / v1.2 all parsed. v1.0 is the deprecated
  18-byte form (no `elidedFallbackNameID`); we default the missing
  field to name ID 2 ("Regular") so callers don't need a version
  check. v1.1 (20-byte header) and v1.2 (same layout, format-4
  axis-value tables permitted) carry the field verbatim. The
  `designAxisSize` field is honoured as the per-record stride so a
  future minor-version bump that grows `AxisRecord` (preserving the
  first 8 bytes per §7.3.7.1) decodes correctly with the trailing
  bytes ignored.
- **AxisRecord array** — every `axisTag` / `axisNameID` /
  `axisOrdering` triple from the §7.3.7.2 design-axes array, exposed
  in document order. Variable fonts must list every `fvar` axis
  here using the matching name ID; the order is not required to
  match `fvar`'s, so callers walk by tag rather than index.
- **Axis value tables** — all four §7.3.7.3 formats decoded into a
  tagged `AxisValue` enum:
    - Format 1: `(axisIndex, flags, valueNameID, value)`.
    - Format 2: format 1 plus `(nominalValue, rangeMinValue,
      rangeMaxValue)`. The `0x80000000` / `0x7FFFFFFF` ±∞ sentinels
      are preserved verbatim alongside the f32-decoded values and
      surfaced as `STAT_RANGE_MIN_NEG_INFINITY` /
      `STAT_RANGE_MAX_POS_INFINITY` constants.
    - Format 3: format 1 plus `linkedValue` for style-linked Bold /
      Italic UI affordances. Inter's wght=400 ⇒ 700 ("Regular" →
      "Bold") and ital=0 ⇒ 1 ("Roman" → "Italic") are the canonical
      examples.
    - Format 4: an arbitrary-cardinality multi-axis combination for
      non-analytic instance names. The §7.3.7.3 "different axisIndex
      per record" rule is enforced (duplicate ⇒ `BadStructure`); the
      records are kept in file order since the spec allows any.
- **Flag accessors** — `is_older_sibling_font_attribute()` decodes
  the `OLDER_SIBLING_FONT_ATTRIBUTE` (0x0001) bit and
  `is_elidable()` the `ELIDABLE_AXIS_VALUE_NAME` (0x0002) bit on
  every variant, including format 4.

Surfaced through `Font` as:

- `Font::stat_table()` → `Option<&StatTable>` (`None` for static
  fonts that omit it; variable fonts are required to ship one).
- `Font::stat_axes()` / `Font::stat_axis_values()` —
  pass-through accessors that return an empty slice when STAT is
  absent so consumers don't need to unwrap an `Option`.
- `Font::stat_elided_fallback_name_id()` → `Option<u16>` — the
  `name` table nameID to use when every component of a composed
  subfamily string would be elided.
- `Font::stat_axis_values_for_tag(axis_tag)` — filters the array to
  one axis tag, including format-4 records that touch it.

The unit-test suite covers a synthetic v1.1 header, every format-2 /
3 / 4 path, the `axisIndex`-out-of-range rejection, and the
format-4 duplicate-axis-index rejection. The integration suite
against InterVariable.ttf asserts the 3-axis / 12-record shape, the
9 distinct wght values (100…900 in 100-step increments), the
format-3 wght=400 → 700 + ital=0 → 1 style links, and the
elided-fallback-name-ID. A DejaVu Sans Mono regression confirms
that all accessors degrade cleanly to empty / `None` when STAT is
not in the table directory.

The format-2 "two tables with overlapping ranges on one axis"
tie-break (§7.3.7.3) is documented as caller policy; the parser
preserves all records.

### Added — `GDEF` AttachList / LigCaretList / MarkAttachClassDef / MarkGlyphSetsDef / ItemVariationStore (2026-06-03)

Extended the `GDEF` table parser past the round-1 `glyphClassDef`-only
slice to cover all six header offsets defined by the OpenType spec.
The table header now distinguishes v1.0 (12-byte header, four
Offset16), v1.2 (adds `markGlyphSetsDefOffset`, 14-byte header), and
v1.3 (adds `itemVarStoreOffset`, 18-byte header) so callers can choose
which sub-tables to consume per the published `minor` revision.

- `GdefTable::attach_points(glyph_id)` decodes the AttachList →
  Coverage → AttachPoint chain and returns the contour-point indices
  that mark glyph cache slots. Indices come back in the increasing
  numerical order mandated by the spec.
- `GdefTable::ligature_carets(glyph_id)` decodes LigCaretList →
  Coverage → LigGlyph → CaretValue, returning every caret on a
  ligature glyph as a tagged `CaretValue` enum that distinguishes
  Format 1 (design units), Format 2 (contour-point index), and
  Format 3 (design units + Device/VariationIndex offset). The
  Format-3 device offset is surfaced verbatim so a
  VariationIndex-aware shaper can interpolate it through the GDEF
  ItemVariationStore.
- `GdefTable::mark_attach_class(glyph_id)` consults the
  `markAttachClassDef` ClassDef, returning the class value compared
  against `lookupFlag.markAttachmentType` (the high byte of
  `lookupFlag`) when GSUB / GPOS filters mark glyphs.
- `GdefTable::mark_glyph_set_count()` /
  `GdefTable::mark_glyph_set_contains(set_index, glyph_id)` decode
  the v1.2 MarkGlyphSetsDef sub-table, exposing the Coverage arrays
  referenced by `lookupFlag.useMarkFilteringSet`. The MarkGlyphSets
  sub-table uses Offset32 rather than Offset16 (the only place in
  GDEF that does), which the parser handles inline.
- `GdefTable::item_var_store_bytes()` exposes the v1.3 GDEF
  ItemVariationStore as a raw byte slice. The same IVS decoder
  already shipping with MVAR / HVAR / VVAR can consume the slice
  when a CaretValueFormat3 `device_offset` points at a
  VariationIndex table.
- Header validation now rejects truncated v1.2 / v1.3 GDEF tables
  before reading the trailing offset fields, instead of returning
  garbage. Null offsets continue to decode to "absent" per spec.

Round 212. Spec coverage: `docs/text/opentype/spec/ISO_IEC_14496-22-OFF-2019.pdf`
§5.10 (GDEF), §5.10.2 (Attachment List), §5.10.3 (Ligature Caret List
+ CaretValueFormat 1/2/3), §5.10.4 (Mark Attachment Class Definition),
§5.10.5 (Mark Glyph Sets Definition), §5.10.6 (Item Variation Store
for GDEF, v1.3).

### Added — `VVAR` per-glyph vertical-metrics variations (2026-06-02)

OpenType Font Variations `VVAR` table (ISO/IEC 14496-22:2019 §7.3.8),
reusing the `ItemVariationStore` (§7.2.3) decoder shared with MVAR and
the `DeltaSetIndexMap` (§7.3.5.2) decoder shared with HVAR. VVAR
carries per-glyph adjustments to the advance heights in `vmtx`, plus
optional adjustments to the top and bottom side bearings, plus — for
CFF2 variable fonts that publish a `VORG` table — vertical-origin Y
deltas (§7.3.8.2 final paragraph: "Mappings and variation data for
vertical origins are not used in fonts with TrueType outlines").

- `VvarTable::parse` decodes the v1 header (majorVersion, five 32-bit
  offsets to the IVS, advance-height map, TSB map, BSB map, and vOrg
  map). A zero offset in the four optional map slots is normal and
  means "no mapping for that quantity" (with the §7.3.8.2 → §7.3.5.3
  implicit outer=0/inner=gid fallback applying only to advance
  heights).
- The HVAR `DeltaSetIndexMap` decoder is reused verbatim per the
  §7.3.8.2 "See the horizontal metrics variations ('HVAR') table
  description for remaining details" cross-reference, so all four
  supported `entryFormat` widths (1 / 2 / 3 / 4 bytes per entry,
  1..16 inner-index bits) and the §7.3.5.2 "glyph IDs beyond
  mapCount-1 use the last entry" clamp apply to VVAR too.
- `Font::advance_height_variation_delta(gid)` /
  `Font::tsb_variation_delta(gid)` / `Font::bsb_variation_delta(gid)` /
  `Font::vorg_variation_delta(gid)` return the interpolated adjustment
  at the current variation coordinates, with the §7.3.5.2 clamp
  applied. The CFF2-only `vorg_variation_delta` is gated on the font
  actually publishing a vOrg mapping table.
- `Font::vvar_table()` exposes the parsed `VvarTable` for callers that
  want direct access (e.g. for inspecting the embedded IVS shape).
- Twelve unit tests on `tables::vvar` cover header parsing without
  mappings, implicit advance-height routing, advance-map clamping for
  out-of-range glyph IDs, the optional vOrg mapping (parse + query
  path), and rejection of `majorVersion != 1`, null IVS offset,
  truncated header, out-of-range IVS offset, and out-of-range mapping
  offsets. Two integration tests against `InterVariable.ttf` (which
  ships no VVAR — Inter is horizontal-only) confirm the absent-table
  fallback path: every `*_variation_delta` accessor on `Font` returns
  `None` and `Font::vvar_table()` returns `None`.

### Added — `HVAR` per-glyph horizontal-metrics variations (2026-06-01)

OpenType Font Variations `HVAR` table (ISO/IEC 14496-22:2019 §7.3.5),
reusing the `ItemVariationStore` (§7.2.3) decoder shared with MVAR.
HVAR carries per-glyph adjustments to the advance widths in `hmtx`,
plus optional adjustments to the left and right side bearings (the
latter two need explicit `DeltaSetIndexMap` mappings per §7.3.5.2).

- `HvarTable::parse` decodes the v1 header (majorVersion, four 32-bit
  offsets to the IVS, advance-width map, LSB map, RSB map). A zero
  offset in the three optional map slots is normal and means "no
  mapping for that quantity" (with the §7.3.5.3 implicit
  outer=0/inner=gid fallback applying only to advance widths).
- `DeltaSetIndexMap::parse` decodes the §7.3.5.2 packed-entry array
  for all four supported `entryFormat` widths (1 / 2 / 3 / 4 bytes
  per entry, 1..16 inner-index bits) and rejects any reserved-bit
  setting in `entryFormat` as a `BadStructure`.
- `Font::advance_width_variation_delta(gid)` /
  `Font::lsb_variation_delta(gid)` /
  `Font::rsb_variation_delta(gid)` return the interpolated adjustment
  at the current variation coordinates, with the §7.3.5.2 "glyph IDs
  beyond mapCount-1 use the last entry" clamp applied.
- Integration coverage against `InterVariable.ttf`: the bit-exact
  parsed advance-width map (2926 entries, entryFormat 0x0018) routes
  glyph 100 to IVD[1] row 274 with reference deltas +24 at wght=900
  and -35 at wght=100, and glyphs 2 / 3 / 4 to IVD[3] row 45 with
  reference deltas +215 / -103. LSB / RSB queries return `None` for
  Inter since it ships no side-bearing maps.

### Added — `MVAR` metrics-variations table (2026-05-31)

OpenType Font Variations `MVAR` table (ISO/IEC 14496-22:2019 §7.3.6),
plus the shared `ItemVariationStore` substructure (§7.2.3) it embeds.
MVAR carries per-instance adjustments for font-wide metric fields in
`OS/2` (sxHeight, sCapHeight, sTypoAscender, …), `hhea` / `vhea`
(caret slope, line gap), `post` (underline thickness / position), and
`gasp` (rangeMaxPPEM) keyed by the §7.3.6.3 four-byte tag registry.

- `MvarTable::parse` decodes the header (majorVersion=1, valueRecordSize,
  valueRecordCount, itemVariationStoreOffset) and the value-record array,
  using `valueRecordSize` as the per-record stride so the future-version
  minor-bump path the spec mentions in §7.3.6.1 parses cleanly with
  unknown trailing bytes ignored.
- `ItemVariationStore::parse` decodes format 1 stores: VariationRegionList
  with `(start, peak, end)` triples per axis per region, plus the array
  of `ItemVariationData` subtables (itemCount × regionIndexCount delta
  matrix where the leading shortDeltaCount columns are int16 and the
  rest int8).
- `Font::metric_variation_delta(tag)` returns the interpolated
  adjustment at the current variation coordinates (the existing
  `set_variation_coords` + `normalised_coords` path), with `avar`
  bending applied for axes that publish a non-identity axis-value map
  (Inter's wght=700 → +0.6 → +0.54 covered by an integration test).
- Region-scalar computation follows §7.1 / §7.2.3.1: peak=0 axes are
  ignored (multiplier 1), coords outside `[start, end]` zero the
  scalar, and the rising and falling edges interpolate linearly between
  start, peak, and end.
- `Font::mvar_table()` and `MvarTable::value_records()` /
  `item_variation_store()` accessors expose the raw payload for tests
  / debugging.
- Eleven unit tests on `tables::mvar` cover the minimal happy path,
  delta interpolation along a single axis, the zero-record fast-path
  with a missing IVS, valueRecordSize<8 rejection, larger-stride
  forward-compatibility, the region-scalar function in isolation
  (axis-ignored / opposite-sign / falling-edge cases), and rejection of
  IVS format ≠ 1 and out-of-range region indices in an IVD.
- Eight integration tests against `tests/fixtures/InterVariable.ttf`
  exercise the table end-to-end: value-record set, IVS shape, zero
  delta at axis defaults, the four corner-of-design-space deltas
  (max-wght / max-opsz / min-wght / max-wght+max-opsz), the interior
  point at wght=700 (which honours Inter's avar wght-axis bend), and
  the `None` return for tags absent from the font's MVAR.

Out of scope this round (per ## Roadmap): `HVAR` / `VVAR` per-glyph
metric variations (the IVS plumbing lands here; the index-map decoder
+ per-glyph resolution path remain), `STAT` style attributes,
`COLR v1` paint graph, `avar v2` delta-set index map, gvar delta
propagation into composite-glyph component offsets and phantom points.

### Added — `cmap` format 2 (high-byte mapping through table) (2026-05-30)

OpenType cmap subtable format 2 decoder. Format 2 is the legacy
mixed-8-/16-bit encoding the cmap chapter describes for pre-Unicode
Japanese / Chinese / Korean fonts ("not commonly used today" per the
spec but still present in older system fonts).

Lookup input for a format-2 subtable is interpreted as a raw codeunit
in the font's native encoding (Shift-JIS / GB2312 / Big5 / KSC-5601 /
etc.), NOT as a Unicode scalar value: a single-byte character is the
8-bit code, a 2-byte character is `(high << 8) | low`. Callers that
want to drive a format-2 font from Unicode are responsible for the
Unicode → native-encoding transcoding step.

- `lookup_format2` walks `subHeaderKeys[256]` to pick a SubHeader
  record from the variable-length `subHeaders[]` region, then evaluates
  the spec's `*(idRangeOffset/2 + (low - firstCode) + &idRangeOffset)`
  formula against the trailing `glyphIdArray[]`. `idDelta` is added
  modulo 65536 (spec-explicit) so deltas that underflow wrap into the
  upper-half u16 range. A zero raw entry returns `None` regardless of
  `idDelta`, matching the "if the value … is not 0" guard.
- `Subtable::Format2(&[u8])` variant; `is_supported_format` now
  accepts `2`; `subtable_length` already routed format 2 through the
  u16-length-at-offset+2 branch and needed no change.
- A 2-byte codeunit whose high byte's `subHeaderKey` is 0 (i.e. the
  high byte is NOT a registered lead byte) would otherwise route to
  SubHeader 0, which the spec reserves for single-byte chars; we
  reject such an input rather than treating it as a 1-byte char with
  the low byte.
- Picker ranks format 2 at `60` — below format 0 (`100`) so a font
  shipping both a Unicode subtable and a format-2 sidecar always
  picks Unicode, but above format 13 (`50`) so a true legacy CJK font
  that ships ONLY format 2 still parses. Platform/encoding scoring
  uses the catch-all branch for the legacy Macintosh (1, 1/2/3/5)
  script pairs that historically hosted format 2.
- Nine new unit tests covering: SubHeader 0 single-byte fallback,
  `idDelta` applied to non-zero glyph-array entries with a zero entry
  still missing, modulo-65536 wraparound on negative deltas, 2-byte
  lookup routing through a non-zero SubHeader (lead byte 0x81 covers
  0x40..0x42), two SubHeaders sharing one sub-array with distinct
  `idDelta` (the headline "why idDelta exists" case), the
  zero-glyph-array-entry → `None` guard, the
  high-byte-through-SubHeader-0 rejection, format-2 not displacing a
  format-12 sidecar, and format-2-only fonts staying pickable.

Out of scope: cmap formats 8 and 10 (Unicode supplementary-plane
mixed-length encodings that the spec calls out as also rare).

## [0.1.6](https://github.com/OxideAV/oxideav-ttf/compare/v0.1.5...v0.1.6) - 2026-05-30

### Other

- format 13 (many-to-one range mappings) for last-resort fonts
- drop HarfBuzz behavioural-citation comments (clean-room)
- distinguish Apple from Microsoft header; expose HeaderVariant
- `'dupe'` indirection chain resolver with bounded depth + cycle detection

### Added — `cmap` format 13 (many-to-one range mappings) (2026-05-30)

OpenType cmap subtable format 13 decoder. Shares its on-wire layout
with format 12 (`u32 numGroups` + groups of `u32 startCharCode,
u32 endCharCode, u32 glyphID`) but differs in semantics: every
codepoint in `[startCharCode..=endCharCode]` maps to the SAME
`glyphID`, not to a running sequence anchored on `startGlyphID`.
The OpenType cmap chapter calls this out explicitly: *"Subtable
format 13 has the same structure as format 12; it differs only in
the interpretation of the startGlyphID/glyphID fields."*

- New `lookup_format13` walker with the same binary search the
  format-12 walker uses. Group records carrying `glyphID = 0`
  return `None` (consistent with how every other format handles a
  hit on `.notdef`).
- `Subtable::Format13(&[u8])` variant; `is_supported_format` now
  accepts `13`.
- Picker ranks format 13 at `50` (below format 0 = `100`), so a
  font shipping both a real-coverage subtable and a format-13
  last-resort fallback always picks the real one. The dedicated
  (platform 0, encoding 6) "Unicode full repertoire — for use with
  subtable format 13" platform/encoding pair from the cmap chapter
  is recognised by `subtable_rank` so true last-resort fonts
  (format-13-only) still parse.
- Seven new unit tests covering: single all-BMP range, multi-range
  with distinct per-range glyphs, the headline many-to-one property
  (a 3-codepoint range with `glyphID = 7` resolves to glyph 7 for
  ALL three inputs, NOT to 7/8/9), `glyphID = 0` → `None`,
  binary-search-many-ranges regression at 200 single-codepoint
  ranges, mixed format-12 + format-13 fonts (the format-12 base map
  wins), and format-13-only fonts (still pickable).

Out of scope: per the cmap chapter we do not gate format-13
decoding on the `head` table's flag bit 14 (last-resort marker);
the spec only suggests format-13 is most often paired with that
flag, it does not require it.

### Fixed — `kern` header sniff conflated Microsoft and Apple variants (2026-05-29)

The `kern` table parser previously dispatched both Microsoft (`u16
version = 0, u16 nTables`) and Apple (`u32 version = 0x00010000,
u32 nTables`) headers through the Microsoft body walker on the
spurious-but-symmetric condition "first u16 == 0". In big-endian
the Apple version u32 reads as bytes `00 01 00 00`, so the high u16
is actually `0x0001`, not `0` — the previous detection branch was
dead. Apple-headered fonts therefore mis-parsed: nTables read as
garbage from the low half of the version field, and either bogus
pair lists got populated or the subtable walk bailed with a typed
error. Symptom: every macOS-bundled `.ttf` that ships a legacy
`kern` table (Helvetica, Lucida, Times, the Snow Leopard / Mavericks
system stack) couldn't get its kerning data surfaced.

- Header sniff now reads the first u16 and dispatches: `0` ⇒
  Microsoft variant (4-byte header, decoded as before); `0x0001` ⇒
  Apple variant (8-byte header; the parser confirms the low u16 of
  the version field is also zero, reads `u32 nTables`, and accepts
  the table without walking the subtable list); any other value ⇒
  typed `Error::BadStructure("kern: bad version")`.
- Apple-format subtable bodies are NOT decoded — the per-subtable
  header layout differs from the Microsoft variant (`u32 length,
  u16 coverage, u16 tupleIndex` instead of `u16 version, u16 length,
  u16 coverage`) and the byte-level details aren't fully covered by
  the staged `docs/text/opentype/` spec material. The parser
  therefore surfaces Apple-headered tables as "structurally valid,
  zero kerning pairs"; `lookup` returns 0 for every glyph pair,
  letting consumer-crate shapers degrade to "no legacy kerning"
  rather than crashing on a misparsed body. A clean-room reference
  for the Apple `kern` / `kerx` subtable bodies is queued as a docs
  follow-up.
- New `pub enum tables::kern::HeaderVariant { Microsoft, Apple }`
  re-exported at the crate root as `oxideav_ttf::KernHeaderVariant`.
- New `KernTable::header_variant() -> HeaderVariant` and
  `KernTable::pair_count() -> usize` accessors.
- New `Font::kern_header_variant() -> Option<KernHeaderVariant>`
  Font-level convenience; returns `None` for fonts that lack a `kern`
  table altogether (every modern OpenType font that ships GPOS
  LookupType 2 instead).
- New unit tests in `tables::kern::tests` (5):
  `apple_header_parses_as_empty_table` (minimal Apple table —
  nTables = 0; previously rejected),
  `apple_header_with_nonzero_n_tables_parses` (Apple with
  nTables = 3 — the realistic shape; previously misparsed),
  `apple_header_truncated_returns_eof` (short input bailing as
  `UnexpectedEof` rather than indexing OOB),
  `unknown_version_rejected` (first u16 ≠ {0, 1} surfaces
  `BadStructure`),
  `apple_header_with_dirty_low_half_rejected` (version field with
  high u16 = 0x0001 but low u16 ≠ 0 is malformed and rejected
  rather than dispatched into the Apple body path).
  `round_trips_one_pair` is extended to assert the Microsoft
  variant is reported and `pair_count == 1`.

Spec: Microsoft OpenType §"kern — Kerning Table" (the
spec-canonical Microsoft variant); Apple TrueType Reference
§"kern" (the Apple-format variant — TOC-only in the staged HTML,
hence the "accept structurally but don't decode subtable bodies"
posture).

### Added — `sbix` `'dupe'` indirection chain resolver with cycle detection (2026-05-27)

The previous round surfaced the `'dupe'` graphic-type sentinel as-is on
[`SbixTable::glyph`] / [`Font::sbix_glyph`], deferring the indirection
chase to the consumer (which was expected to do its own cycle
detection). This round closes that follow-up: new APIs follow `'dupe'`
links within the selected strike up to a bounded depth, with explicit
cycle detection — visited glyph ids are tracked and a revisit bails
to `None` rather than recursing.

- `pub const oxideav_ttf::SBIX_MAX_DUPE_DEPTH: usize = 8` (re-exported
  from `tables::sbix::MAX_DUPE_DEPTH`). The cap defuses pathological
  forward chains that don't form a strict cycle (e.g. `A → B → C → …`).
- `SbixGlyph::is_dupe()` — `true` when `graphic_type == *b"dupe"`.
- `SbixGlyph::dupe_target() -> Option<u16>` — decodes the 2-byte
  big-endian indirection target glyph id; returns `None` for
  non-`'dupe'` entries or malformed (<2 byte) payloads.
- `SbixTable::resolve_dupe_chain(strike_index, glyph_id) -> Option<SbixGlyph<'a>>`
  — follow `'dupe'` indirections inside one strike, returning the
  first non-`'dupe'` entry. `None` on: zero-length / out-of-range
  entry, chain length > `MAX_DUPE_DEPTH`, two-glyph cycle, self-loop,
  malformed payload, dangling target.
- `SbixTable::lookup_best_fit_resolved(glyph_id, target_ppem)
  -> Option<SbixGlyph<'a>>` — picks the closest-ppem strike covering
  the glyph (same tie-break policy as `lookup_best_fit`: larger size
  wins) and then chases the indirection within that strike.
- `Font::sbix_glyph_resolved(glyph_id, ppem) -> Option<SbixGlyph<'_>>`
  — Font-level convenience wired to `lookup_best_fit_resolved`.
- New unit tests in `tables::sbix::tests` (8):
  `dupe_predicates_decode_target_glyph_id`,
  `dupe_predicates_return_none_on_short_payload`,
  `resolve_dupe_chain_follows_indirection_to_real_entry` (zero / one /
  two hops), `resolve_dupe_chain_detects_two_glyph_cycle`,
  `resolve_dupe_chain_detects_self_cycle`,
  `resolve_dupe_chain_caps_depth` (forward chain `MAX_DUPE_DEPTH + 1`
  long, last entry still a dupe — must bail without reaching a real
  blob), `resolve_dupe_chain_returns_none_for_oob_dupe_target`,
  `lookup_best_fit_resolved_walks_through_dupe`.

Spec: Microsoft OpenType §"sbix — Standard Bitmap Graphics Table"
(graphicType `'dupe'` semantics — 2-byte big-endian glyph id payload
substitutes the bitmap of the indirect glyph). Apple TrueType
Reference §"sbix".

## [0.1.5](https://github.com/OxideAV/oxideav-ttf/compare/v0.1.4...v0.1.5) - 2026-05-24

### Other

- full public accessor API + well-known nameID registry
- glyph-name → Unicode resolver from staged Adobe Glyph List
- binary-search format-4 + harden indirect path; glyf: composite depth-guard tests
- drop committed Cargo.lock + relax oxideav-core to "0.1"

### Added — `name`-table public accessor API + well-known nameID registry (2026-05-24)

The `name` table was parsed since round 1 but only `family_name()` and
`full_name()` were exposed. This round surfaces the whole table: the
registered nameID semantics (Adobe TN5149 §1.3–1.10), locale-targeted
lookup, and full record enumeration with the `(platformID, encodingID,
languageID, nameID)` locator tuple.

- New `name_id` module of well-known nameID constants — `COPYRIGHT` (0),
  `FAMILY` (1), `SUBFAMILY` (2), `UNIQUE_ID` (3), `FULL_NAME` (4),
  `VERSION` (5), `POSTSCRIPT` (6), `TRADEMARK` (7), `MANUFACTURER` (8),
  `DESIGNER` (9), `DESCRIPTION` (10), `VENDOR_URL` (11), `DESIGNER_URL`
  (12), `LICENSE` (13), `LICENSE_URL` (14), `TYPOGRAPHIC_FAMILY` (16),
  `TYPOGRAPHIC_SUBFAMILY` (17), `COMPATIBLE_FULL` (18), `SAMPLE_TEXT`
  (19), `POSTSCRIPT_CID` (20) — and a `platform` module (`UNICODE` 0,
  `MACINTOSH` 1, `WINDOWS` 3). Both re-exported at the crate root.
- New `oxideav_ttf::NameRecord { platform_id, encoding_id, language_id,
  name_id, string: Option<String> }`. `string` is `Some` for the
  encodings we decode without an external legacy codepage table (Unicode
  platform; Windows Unicode BMP / UCS-4; Macintosh Roman ASCII) and
  `None` for Macintosh non-Roman scripts (Japanese / Chinese / Korean —
  TN5149 §1.2), whose Shift-JIS / Big5 / EUC tables are not staged under
  `docs/`. The locator tuple and raw bytes are surfaced regardless.
- New `Font` accessors: `subfamily_name`, `typographic_family_name`
  (nameID 16, falling back to 1 per TN5149 §1.4's omission rule),
  `typographic_subfamily_name` (17 → 2), `postscript_name`,
  `version_string`, `copyright`, `trademark`, `manufacturer`, `designer`,
  `description`, `vendor_url`, `designer_url`, `license_description`,
  `license_url`. Plus the general `name_string(name_id)` (best-ranked
  locale, Windows English first), `name_string_for(name_id, platform_id,
  language_id)` (exact locale, no ranking — e.g. the Japanese family
  name `(FAMILY, WINDOWS, 0x0411)`), and `name_records()` (every record,
  decoded where possible).
- `NameTable` gains `len`, `is_empty`, `records`, `record_bytes`, and
  `find_for`. The format-0/1 record walk is unchanged (the format-1
  langTagRecord array is documented but not needed for the string data).
- Tests: 5 new unit tests in `tables::name::tests` (nameID-registry
  spot-check, record enumeration, exact-locale `find_for` vs. ranked
  `find`, Mac non-Roman undecodable-but-surfaced, Mac Roman ASCII) and a
  new `tests/name_records.rs` integration suite (4 tests) driving the
  accessors against DejaVu Sans 2.37 (well-known accessors, generic vs.
  typed lookup, exact-locale lookup, record-enumeration locator tuples).

Spec: Adobe Technical Note #5149 "OpenType-CID/CFF CJK Fonts: 'name'
Table Tutorial" §1.2 (Platform / Script / Language IDs) and §1.3–1.10
(per-nameID semantics). Microsoft OpenType §"name — Naming Table".
Apple TrueType Reference §"name".

### Added — Adobe Glyph List (AGL) glyph-name → Unicode resolution (2026-05-23)

New `agl` module with two public functions, re-exported at the crate
root:

- `glyph_name_to_codepoints(name) -> Option<&'static [u32]>` — resolves
  a PostScript glyph name (as found in a `post` version-2.0 table or a
  CFF charset) to its Unicode scalar-value sequence via a direct lookup
  in the Adobe Glyph List. Most names map to a single codepoint;
  ligature and Hebrew base+points names map to a short sequence
  (e.g. `dalethatafpatah` → `[U+05D3, U+05B2]`).
- `glyph_name_to_char(name) -> Option<char>` — convenience for names
  that map to exactly one valid (non-surrogate) scalar value.

The AGL data file (`agl-glyphlist.txt`, table version 2.0) is embedded
verbatim via `include_str!` — a byte-identical copy of the staged
`docs/text/opentype/spec/agl-glyphlist.txt`, BSD-style licence header
preserved. The list is parsed lazily into a `HashMap` on first use
(`OnceLock`-cached).

Scope is the **direct table lookup only**. The AGL Specification's
algorithmic fallback for names absent from the table (suffix stripping
after the first period, underscore-split component names, and the
`uniXXXX` / `uXXXXX...` synthetic-name convention) is **not**
implemented: that algorithm lives in the AGL Specification §2/§6, which
is not staged under `docs/text/opentype/`. Seven new unit tests cover
basic Latin, accented, multi-codepoint sequences, presentation-form
ligatures, unknown-name `None`, table size floor / `OnceLock` identity,
and comment/blank-line skipping.

### Added — cmap format-4 binary search + composite-glyph depth-guard tests (2026-05-21)

Two correctness-and-robustness improvements, no public-API change.

- `tables::cmap::lookup_format4` now binary-searches `endCode[]`
  instead of linear-scanning. Format-4 mandates `endCode[]` be sorted
  ascending and the spec ships the `searchRange / entrySelector /
  rangeShift` triple precisely so divide-free reverse-engineered
  binary search is possible; we have division, so we do a normal
  log-N search. CJK fonts shipping 100+ segments (Source Han Sans /
  Noto Sans CJK) now resolve BMP codepoints in O(log N) instead of
  O(N). New test
  `format4_binary_search_resolves_many_segments` exercises a
  200-segment synthetic cmap and asserts both hits AND
  immediately-adjacent misses to pin down off-by-ones.
- `tables::cmap::lookup_format4` now uses `checked_add` for the
  `target` byte offset in the indirect-mapping (glyphIdArray) path.
  In practice the operands are all u16-bounded so the sum cannot
  overflow `usize`, but the previous code did unchecked arithmetic
  on inputs that could in principle be attacker-controlled. New
  test `format4_indirect_mapping_resolves_through_glyph_id_array`
  rounds-trips the indirect path explicitly (the existing
  `format4_round_trip` only exercises the direct
  `id_range_offset == 0` branch). New test
  `format4_truncated_arrays_does_not_panic` exposes the lookup to
  a deliberately length-trimmed subtable to prove the existing
  per-read bounds checks suffice to keep us panic-free.
- `tables::glyf` now has explicit boundary tests for
  `MAX_COMPOSITE_DEPTH`:
  - `composite_chain_at_max_depth_succeeds` — a chain of
    composites K levels deep, with K = `MAX_COMPOSITE_DEPTH - 1`,
    decodes the leaf triangle.
  - `composite_chain_over_max_depth_returns_composite_too_deep` —
    one link deeper, we get `Error::CompositeTooDeep` instead of a
    stack overflow.
  - `composite_self_cycle_terminates_with_composite_too_deep` — a
    glyph that references itself (the simplest pathological
    cycle a malformed / malicious font can ship) is rejected
    cleanly rather than overflowing the recursion stack.

Spec: Microsoft OpenType §"cmap — Character to Glyph Index Mapping
Table" / §"Format 4: Segment mapping to delta values". §"glyf —
Glyph Data" / §"Composite Glyph Description". Apple TrueType
Reference §"cmap" / §"glyf". ISO/IEC 14496-22 §5.

## [0.1.4](https://github.com/OxideAV/oxideav-ttf/compare/v0.1.3...v0.1.4) - 2026-05-05

### Other

- LookupType 3 (cursive) + LookupType 5 (mark-to-ligature) + real Arabic fixture

### Added — GPOS LookupType 3 (cursive attachment) + LookupType 5 (mark-to-ligature) + lookup-level ExtensionPos coverage + real Arabic GPOS fixture (2026-05-04)

Closes the remaining GPOS lookup-type gaps left from the prior
round: cursive attachment for Arabic Nastaliq / script-font
cursive chaining, and mark-to-ligature for the
"`fi` ligature + above-mark" cluster. Adds an integration test
suite against Noto Sans Arabic that drives the new apply paths
(LT 1 + LT 5) against the real-world LAM-ALEF ligature.

- `tables::gpos::CursiveAttachment` (re-exported as
  `oxideav_ttf::CursiveAttachment`) — entry/exit anchor pair for
  one cursive glyph: `entry: Option<(i16, i16)>` (the connecting
  point on the leading edge, `None` for first-of-cluster glyphs)
  and `exit: Option<(i16, i16)>` (trailing edge, `None` for
  last-of-cluster). All coordinates are in TT font units (Y-up).
- `tables::gpos::GposTable::apply_lookup_type_3(lookup_index, gid)
   -> Option<CursiveAttachment>` — Cursive Attachment
  (CursivePosFormat1). Walks every sub-table in the named lookup;
  first hit wins. Anchor formats 1, 2 and 3 are accepted (format
  2's anchor point and format 3's device tables are silently
  ignored). The shaper chains glyph N+1 onto glyph N by
  translating glyph N+1's pen origin so its `entry` lands on
  glyph N's `exit`: `delta = prev.exit - this.entry`.
- `tables::gpos::GposTable::lookup_cursive_attachment(gid)
   -> Option<CursiveAttachment>` — convenience walker that scans
  the entire LookupList rather than a single index. Useful for
  fonts that ship a single `curs` lookup (the common case).
- `tables::gpos::GposTable::apply_lookup_type_5(lookup_index,
   ligature, ligature_component, mark) -> Option<(i16, i16)>` —
  Mark-to-Ligature Attachment (MarkLigPosFormat1). Per-component
  anchor lookup: `ligature_component` is 0-indexed (0 = first
  component, e.g. `LAM` of `LAM-ALEF`). Returns the `(dx, dy)`
  offset to add to the mark's pen origin so its class anchor
  lands on the selected component's anchor. Returns `None` on
  coverage miss, out-of-range component, or null-anchor for the
  mark's class on the requested component.
- `tables::gpos::GposTable::lookup_mark_to_ligature(ligature,
   ligature_component, mark) -> Option<(i16, i16)>` — convenience
  walker scanning the entire LookupList.
- `apply_pos_records` (the GPOS chain-context nested-dispatch
  helper) now also handles nested LookupType 3 references —
  emits a single `PosRecord` at the absolute glyph index
  carrying `prev.exit - this.entry` as `(x_placement, y_placement)`
  when both anchors line up across the abs_idx-1 / abs_idx pair.
- ExtensionPos (LookupType 9) is now confirmed to unwrap
  transparently both at the sub-table level (LT-9 sub-table inside
  any kind=N lookup) AND at the **lookup level** (a whole lookup
  whose `lookupType` is 9 wrapping any of LT 1 / 2 / 3 / 4 / 5 /
  6 / 8) — the new `extension_wrapper_unwraps_for_cursive_pos_lookup`
  test pins the lookup-level case down. This closes the previously-
  deferred "LookupType 7 / lookup-level extension wrapper" gap (the
  pattern we mirror is GSUB's LT-7 handling — for GPOS it's LT 9).
- Public `Font` API:
  - `Font::gpos_apply_lookup_type_3(lookup_index, gid) -> Option<CursiveAttachment>`
  - `Font::lookup_cursive_attachment(gid) -> Option<CursiveAttachment>`
  - `Font::gpos_apply_lookup_type_5(lookup_index, ligature, ligature_component, mark) -> Option<(i16, i16)>`
  - `Font::lookup_mark_to_ligature(ligature, ligature_component, mark) -> Option<(i16, i16)>`
- New unit tests (in `tables::gpos::tests`):
  - `cursive_pos_format1_returns_entry_and_exit_anchors` —
    round-trip: gid 5 (entry-only), gid 6 (both), gid 7
    (exit-only) match their on-disk anchor coords exactly.
  - `cursive_pos_returns_none_off_coverage`,
    `cursive_pos_returns_none_when_lookup_is_not_type_3`,
    `lookup_cursive_attachment_walks_lookup_list`.
  - `mark_to_ligature_attaches_to_each_component` — round-trip:
    2-component LAM-ALEF-style ligature with FATHA-style mark on
    each component.
  - `mark_to_ligature_returns_none_for_out_of_range_component`,
    `mark_to_ligature_returns_none_for_uncovered_glyphs`,
    `lookup_mark_to_ligature_walks_lookup_list`.
  - `extension_wrapper_unwraps_for_cursive_pos_lookup` — verifies
    that a Lookup whose `lookupType=9` wrapping a CursivePosFormat1
    sub-table is correctly unwrapped by `apply_lookup_type_3`.
- New integration test file `tests/noto_arabic_gpos.rs` — drives
  the new apply paths against Noto Sans Arabic 2022:
  - `noto_arabic_gpos_lookup_list_has_lt_1_4_5_6_8` — proves the
    fixture exposes every lookup type we now support.
  - `lt1_single_positioning_fires_on_real_arabic_glyphs` — counts
    ≥100 distinct non-zero LT-1 single-position adjustments
    across the real glyph table.
  - `lt5_mark_to_ligature_anchors_marks_on_lam_alef` — for every
    Arabic vowel mark in U+064B..U+0652, confirms the LAM-ALEF
    (U+FEFB) ligature anchors it on both components 0 (LAM) and
    1 (ALEF). Sanity-bounds the returned `(dx, dy)` to ±5 UPMs.
  - `lt5_returns_none_for_out_of_range_component`,
    `lt5_returns_none_for_uncovered_mark`,
    `lt3_cursive_attachment_returns_none_in_noto_sans_arabic`
    (Noto's regular Arabic cut isn't Nastaliq).
  - `new_gpos_lookup_types_are_panic_free_on_real_arabic_run` —
    drives every lookup index through every new apply path.
  - `lt1_plus_lt5_combined_shaping_pass_on_lam_alef_with_fatha` —
    the headline real-font dual exercise: applies LT 1 (advance
    trim walk) + LT 5 (FATHA on LAM component 0 of LAM-ALEF) in
    a single pass and asserts the LT-5 anchor is non-zero.

Spec: Microsoft OpenType §"Cursive Attachment Positioning Subtable"
(LookupType 3 Format 1), §"Mark-to-Ligature Attachment Positioning
Subtable" (LookupType 5 Format 1). Apple TrueType Reference §"GPOS",
ISO/IEC 14496-22 §6 (OFF).

## [0.1.3](https://github.com/OxideAV/oxideav-ttf/compare/v0.1.2...v0.1.3) - 2026-05-05

### Other

- LookupType 1 (single pos) + LookupType 8 (chained context) + LookupList enum

### Added — GPOS LookupType 1 + LookupType 8 + LookupList enumeration (2026-05-04)

Closes the highest-impact gap left in the GPOS lookup-type grid:
single-glyph positioning (LT 1) and chained-context positioning
(LT 8) plus a public LookupList enumeration that lets downstream
shapers find every chained / mark-to-ligature / extension lookup
without probing each index in turn.

- `tables::gpos::PosValue` (re-exported as `oxideav_ttf::PosValue`)
  — a decoded ValueRecord: `x_placement`, `y_placement`,
  `x_advance`, `y_advance` (all `i16`, TT Y-up font units). Fields
  whose `valueFormat` bit isn't set come back as `0`. The four
  device-table offsets in the high byte of `valueFormat` are
  skipped over (we don't run the TT bytecode interpreter so
  device-pixel snapping is out of scope).
- `tables::gpos::PosRecord` (re-exported as `oxideav_ttf::PosRecord`)
  — one per-glyph adjustment emitted by a chained-context match:
  `glyph_index` (absolute offset into the input glyph run) +
  `value: PosValue`. Multiple records may target the same glyph
  index when nested lookups stack adjustments; callers add (don't
  replace) deltas.
- `tables::gpos::GposTable::apply_lookup_type_1(lookup_index, gid)
   -> Option<PosValue>` — Single Adjustment Positioning. Both
  formats are supported:
  - **Format 1** — one shared `ValueRecord` applied to every
    glyph the coverage table lists (typical for "shift this whole
    script up by N units" / `valt` features).
  - **Format 2** — per-glyph `ValueRecord` indexed by the
    coverage index (used by `cpsp` capital-spacing and similar).

  ExtensionPos (LookupType 9) wrappers are unwrapped transparently.
- `tables::gpos::GposTable::apply_lookup_type_8(lookup_index, gids,
   pos) -> Option<Vec<PosRecord>>` — Chained Contexts Positioning.
  Same wire shape as GSUB LookupType 6 — formats 1 (glyph
  sequence), 2 (class-based) and 3 (coverage-based) — but each
  `PosLookupRecord { sequenceIndex, lookupListIndex }` references
  another GPOS lookup. The walker dispatches nested LookupType 1 /
  2 (kerning) / 4 (mark-to-base) / 6 (mark-to-mark) / 8 (recursive)
  references and returns a single concatenated `Vec<PosRecord>`.
  Recursion is bounded by `MAX_NESTED_LOOKUP_DEPTH = 8` to defuse
  pathological self-referential graphs (same fence as GSUB).
  Backtrack sequences are matched in reverse-text order per spec.
- `tables::gpos::GposTable::lookup_list() -> impl Iterator<Item =
   (u16, u16, u16)>` and the matching
  `tables::gsub::GsubTable::lookup_list()` — enumerate every
  lookup as `(lookup_index, lookup_type, subtable_count)`. The
  reported `lookup_type` is the **effective** type after
  unwrapping any LookupType-7 ExtensionSubst (GSUB) or
  LookupType-9 ExtensionPos (GPOS) wrapper, so callers don't
  need to know whether a lookup is wrapped.
- Public `Font` API:
  - `Font::gpos_apply_lookup_type_1(lookup_index, gid) -> Option<PosValue>`
  - `Font::gpos_apply_lookup_type_8(lookup_index, gids, pos) -> Option<Vec<PosRecord>>`
  - `Font::gpos_lookup_list() -> Vec<(u16, u16, u16)>`
  - `Font::gsub_lookup_list() -> Vec<(u16, u16, u16)>`
- New tests:
  - `tables::gpos::tests::single_pos_format1_returns_shared_value_for_every_covered_glyph`
  - `tables::gpos::tests::single_pos_format2_returns_per_glyph_value`
  - `tables::gpos::tests::single_pos_returns_none_when_lookup_index_out_of_range`
  - `tables::gpos::tests::single_pos_returns_none_when_lookup_is_not_type_1`
  - `tables::gpos::tests::chain_context_pos_format1_dispatches_nested_single_pos`
  - `tables::gpos::tests::chain_context_pos_format1_no_match_when_backtrack_or_lookahead_misses`
  - `tables::gpos::tests::chain_context_pos_format3_coverage_based_dispatch`
  - `tables::gpos::tests::chain_context_pos_format3_no_match_when_window_short_or_uncovered`
  - `tables::gpos::tests::chain_context_pos_format2_class_based_dispatch`
  - `tables::gpos::tests::chain_context_pos_format2_no_match_when_class_differs`
  - `tables::gpos::tests::lookup_list_reports_index_type_and_subtable_count`
  - `tables::gpos::tests::value_record_size_packs_low_byte_only`
  - Integration tests against DejaVu Sans:
    `gpos_lookup_list_enumerates_pair_pos_in_dejavu_sans`,
    `gsub_lookup_list_enumerates_ligature_in_dejavu_sans`,
    `new_gpos_lookup_types_are_panic_free_across_dejavu_lookups`
    (drives every LT 1 / LT 8 lookup found in the live GPOS table
    through both apply paths to prove panic freedom on real
    on-disk geometry).

GPOS LookupTypes 3 (cursive attachment), 5 (mark-to-ligature), 7
(extension at the lookup level — the LT 9 wrapper handles the
sub-table level transparently for every supported type) are still
deferred. LT 3 unblocks Arabic Nastaliq / script-font cursive
chaining; LT 5 closes the ligature + mark gap (fi + dot-above).

Spec: Microsoft OpenType §"Single Adjustment Positioning Subtable"
(LookupType 1 Format 1 / 2), §"Chained Sequence Context Format
1 / 2 / 3" (the GSUB and GPOS tables share the wire format),
Apple TrueType Reference §"GPOS", ISO/IEC 14496-22 §6 (OFF).

## [0.1.2](https://github.com/OxideAV/oxideav-ttf/compare/v0.1.1...v0.1.2) - 2026-05-04

### Other

- LookupTypes 2 (multiple) / 3 (alternate) / 5 (contextual) / 8 (reverse-chain)
- Delete Cargo.lock
- drop private-item intra-doc link to MAX_NESTED_LOOKUP_DEPTH
- LookupType 4 wiring + LookupType 6 chained context (formats 1/2/3)
- feature-tagged single substitution (LookupType 1) for Arabic shaping
- variable-font axes, axis remap, glyph TupleVariationStore
- document COLR/CPAL + sbix + TTC subfont APIs
- Apple-style PNG/JPEG bitmap-strike colour glyph parser
- vector colour-emoji layer-stack parsers (v0 / v0+v1)
- fix from_collection_bytes BadOffset on real .ttc subfonts
- document cmap format 14 + lookup_variation example
- implement format 14 (Unicode Variation Sequences) lookup
- skip unsupported subtable formats before length validation

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
  the pair is in the default UVS table (format-14 default-UVS
  semantic — variation selector chooses default presentation),
  or `None` otherwise.
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
