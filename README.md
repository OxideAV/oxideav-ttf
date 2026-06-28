# oxideav-ttf

Pure-Rust TrueType font parser for the [oxideav](https://github.com/OxideAV)
framework. Implements the sfnt container, the core OpenType tables, and a
coherent GSUB / GPOS shaping engine (`Font::shape`) doing
Latin/Cyrillic/Greek/CJK/Arabic shaping with positional forms, ligatures,
kerning, and mark attachment.

## Supported tables

- sfnt + table directory walker.
- `head`, `hhea`, `maxp`, `cmap` (ALL base formats — 0, 2, 4, 6, 8,
  10, 12, 13 — plus format 14 Unicode Variation Sequences as a
  sidecar. Format 2 is the legacy mixed-8-/16-bit "high-byte mapping
  through table" layout for pre-Unicode CJK fonts, format 13 the
  "many-to-one range mappings" layout used by last-resort fonts.
  Format 8 is the discouraged mixed-16-/32-bit UTF-16 layout: its
  8 KiB `is32` lead-word bit array is enforced as a validity filter
  in both directions on top of the format-12-style sequential group
  search. Format 10 is the 32-bit trimmed-array analog of format 6
  for fonts covering one contiguous supplementary-plane window),
  `name`, `OS/2`, `hmtx`, `loca`, `glyf` (simple + composite — the
  composite path handles both component-placement forms from the "Composite
  glyph description" section: the `ARGS_ARE_XY_VALUES` offset-vector form
  *and* the point-matching form where `argument1`/`argument2` are parent /
  child point numbers aligned after the child's 2×2 transform is applied,
  with `SCALED_COMPONENT_OFFSET` / `UNSCALED_COMPONENT_OFFSET` honoured so
  a scaled offset is transformed into the parent grid while the default /
  unscaled offset is left raw; a reference to an unresolved phantom point
  degrades to zero-offset placement),
  `post` (ISO/IEC 14496-22:2019 §5.2.10 — full structural decode of
  v1.0 / v2.0 / v2.5 / v3.0: 32-byte common header with `italicAngle`
  / underline geometry / `isFixedPitch` / four PostScript memory
  hints; v2.0 `numGlyphs` + `glyphNameIndex[]` + Pascal-format
  `stringData[]` with the §5.2.10.2 ASCII allow-set + 63-byte
  recommendation surfaced through `has_oversize_glyph_name` /
  `has_non_conformant_glyph_name` diagnostic flags; v2.5
  `int8 offset[numGlyphs]` resolved per §5.2.10.3 to a standard
  Macintosh index; v3.0 accepted as the names-absent form (required
  for CFF v1 outlines per §5.2.10.4); Apple v4.0 rejected as
  out-of-scope. `Font::glyph_name_ref(gid)` exposes both the
  `Custom(&str)` (Pascal-string) and `StandardMac { index }` branches;
  `Font::glyph_name(gid)` is the convenience accessor returning
  `Some(&str)` for **both** branches — Pascal strings directly, and
  `StandardMac` indices resolved through the 258-name standard
  Macintosh glyph table (docs gap #1277 closed: the list is staged at
  `docs/text/opentype/post-standard-mac-glyph-names.md`, transcribed
  verbatim into `STANDARD_MAC_GLYPH_NAMES: [&str; 258]` and exposed
  alongside the `standard_mac_glyph_name(index)` helper and
  `PostTable::resolved_glyph_name(gid)`. Reverse mapping closes the
  loop: `Font::gid_for_glyph_name(name)` (over `PostTable::gid_for_name`)
  inverts the resolved name across all versions — custom and
  standard-Mac alike — returning the lowest glyph id carrying that
  name, and `Font::iter_glyph_names()` walks every `(gid, name)` pair),
  `VORG` (vertical origin table, ISO/IEC 14496-22:2019 §5.4.4 — header
  fields `defaultVertOriginY` + the optional sorted
  `vertOriginYMetrics` array of per-glyph overrides, with the §5.4.4
  "must be sorted by increasing glyphIndex" + "must not have more than
  one element with the same glyphIndex" invariants enforced at parse
  time; per-glyph lookup is a binary search. The §5.4.4 "If present in
  TrueType OFF fonts it must be ignored by font clients" rule is
  honoured at the `Font` layer — `vert_origin_y_from_vorg` returns
  `None` whenever `glyf` is present, even when the table itself parses
  cleanly through `vorg_table()` for tooling that wants to introspect
  it),
  `vhea` + `vmtx` (vertical-layout metrics for CJK / Mongolian fonts,
  ISO/IEC 14496-22:2019 §5.7.9 / §5.7.10 — both `vhea` versions
  parse: v1.0 with the centre-line-relative `ascent` / `descent`
  fields and v1.1 with the ideographic-em-box typographic
  `vertTypoAscender` / `vertTypoDescender` rename; `vmtx` covers the
  long-pair array plus the §5.7.10 monospaced top-side-bearing tail),
  `BASE` (baseline table, ISO/IEC 14496-22:2019 §6.3.1 — both v1.0 and
  v1.1 headers, with the v1.1 trailing `itemVarStoreOffset` bounds-
  checked and the IVS bytes surfaced for the shared `ItemVariationStore`
  decoder; HorizAxis and VertAxis trees each decode the BaseTagList +
  BaseScriptList; per script, BaseScript carries an optional BaseValues
  table giving one BaseCoord per baseline tag plus an optional default
  MinMax table with per-feature `FeatMinMaxRecord` overrides and an
  array of `BaseLangSysRecord` language-specific MinMax overrides;
  BaseCoord covers all three §6.3.1.3 formats — design-unit-only,
  design-unit + reference-glyph/contour-point pair, and design-unit +
  Device-table / VariationIndex offset for size- and instance-dependent
  adjustment. `Font::base_horiz_y_for_script_baseline(script_tag,
  baseline_tag)` walks the HorizAxis Y coordinate for a (script,
  baseline) pair; the mirror `base_vert_x_for_script_baseline` walks the
  VertAxis X coordinate),
  `gasp` (grid-fitting and scan-conversion procedure table, ISO/IEC
  14496-22:2019 §5.3.7 — both version 0 (pre-ClearType) and version 1
  (adds the two `GASP_SYMMETRIC_*` ClearType bits) parse identically;
  per-record `(rangeMaxPPEM, rangeGaspBehavior)` decoded with the four
  defined flags (`GASP_GRIDFIT`, `GASP_DOGRAY`, `GASP_SYMMETRIC_GRIDFIT`,
  `GASP_SYMMETRIC_SMOOTHING`); strictly-increasing-`rangeMaxPPEM`
  invariant enforced at parse time so a malformed array does not
  shadow later records; reserved bits `0xFFF0` tolerated and surfaced
  through `GaspRange::reserved_bits()`; `Font::gasp_behavior_for_ppem(ppem)`
  picks the first record whose `rangeMaxPPEM` is at least the requested
  ppem, returning `None` for fonts without `gasp` or when every limit
  sits below the request — caller falls back to rasteriser default per
  §5.3.7; `GaspTable::covers_all_sizes()` flags the single-`0xFFFF`-
  sentinel shortcut. MVAR coupling to the `gsp0`..`gsp9` value tags is
  documented for variable-font interpolation of `rangeMaxPPEM`),
  `LTSH` (linear-threshold table, ISO/IEC 14496-22:2019 §5.7.4 — the
  4-byte header (`uint16 version`, `uint16 numGlyphs`) plus the
  `uint8 yPels[numGlyphs]` array publishing the lowest ppem at which
  each glyph's grid-fitted advance has converged on the rounded linear
  advance per §5.7.4 criterion (a) `ppem ≥ 50 ∧ |Δ| ≤ 2 %` or (b)
  exact equality; the §5.7.4 sentinel `yPels = 1` marks glyphs
  without sidebearing instructions as "always scales linearly".
  `numGlyphs` is cross-checked against `maxp` at parse time so a
  mismatch is rejected as `BadStructure` instead of silently
  truncating per-glyph lookups; trailing 4-byte sfnt padding is
  tolerated. `Font::ltsh_threshold(gid)` returns the recorded ppem,
  `Font::ltsh_linearly_scales_at_ppem(gid, ppem)` honours the
  `ppem ≥ yPels[gid]` inequality and falls through to `false` for
  fonts without `LTSH` — §5.7.4's prescription is to grid-fit in
  that case, which is what the predicate signals; `LtshTable::
  all_always_linear()` short-circuits the "every glyph carries the
  sentinel" common case for rasterisers that want to skip per-glyph
  probing. §5.7.4 names `hdmx` and `vdmx` as the complementary
  precomputed-advance methods; both `hdmx` and `VDMX` are also
  supported (see below)),
  `hdmx` (horizontal device metrics, ISO/IEC 14496-22:2019 §5.7.2 —
  the 8-byte header (`uint16 version`, `int16 numRecords`,
  `int32 sizeDeviceRecord`) plus `numRecords` device records, each
  carrying `uint8 pixelSize` + `uint8 maxWidth` + `uint8 widths[numGlyphs]`
  and padded to the long-word-aligned per-record stride. Each record
  publishes the grid-fitted integer-pixel advance widths of every
  glyph at a single recorded ppem so a rasteriser can short-circuit
  scan-converting at one of those sizes. Per-record `widths[]` length
  is cross-checked against `maxp.numGlyphs` at parse time (under-
  sized `sizeDeviceRecord` rejected as `BadStructure`); the §5.7.2
  "sorted by pixel size" invariant is enforced as strict-monotonic
  increase so a corrupted record cannot shadow later ones;
  `sizeDeviceRecord` is honoured as the stride so writers that
  long-align past the minimum body still decode with the trailing
  bytes ignored. `Font::hdmx_advance_pixels(gid, ppem)` is the
  per-`(glyph, ppem)` accessor; §5.7.2 has no "nearest neighbour"
  rule so an unrecorded ppem returns `None` and the caller falls
  back to scan-converting. `Font::hdmx_recorded_ppem_sizes()` lists
  the recorded sizes in ascending order. §7.3.5 forbids `hdmx` in
  variable fonts — we parse it whenever present and leave the
  cross-check to the caller),
  `VDMX` (vertical device metrics, ISO/IEC 14496-22:2019 §5.7.8 —
  the 6-byte header (`uint16 version`, `uint16 numRecs`,
  `uint16 numRatios`) plus a `RatioRange[numRatios]` aspect-ratio
  selector array, a parallel `Offset16[numRatios]` array pointing
  at one VDMX group per ratio, and the VDMX groups themselves: each
  group is a 4-byte `(recs, startsz, endsz)` header followed by a
  sorted `vTable[recs]` array of `(yPelHeight, yMax, yMin)` tuples
  giving the font-wide vertical pel envelope at each recorded ppem.
  Both versions 0 and 1 parse identically — `bCharSet` semantics
  differ between them but the numeric layout doesn't, so the raw
  byte is surfaced for the caller. The §5.7.8 "sorted by yPelHeight,
  need not be continuous" rule is enforced as strict-monotonic
  increase per group so a corrupted vTable cannot shadow later
  records; the `(xRatio=0, yStartRatio=0, yEndRatio=0)` catch-all
  sentinel is range-validated as the last RatioRange entry per
  §5.7.8 ("if present, this must be the last Ratio group in the
  table"); shared groups (two RatioRange entries pointing at one
  on-wire group) deduplicate to one parsed `VdmxGroup` while the
  per-ratio mapping is preserved so both ratios still resolve to
  the shared records. `Font::vdmx_y_extent_for_device(ppem,
  deviceXRatio, deviceYRatio)` runs the §5.7.8 "once a match is
  found, the search stops" first-match RatioRange walk and returns
  the matched group's `(yMax, yMin)` at the requested ppem, or
  `None` for a non-matching device (no sentinel) or an unrecorded
  ppem (no nearest-neighbour fallback); the `vdmx_y_extent_square`
  shortcut hard-codes the common 1:1 lookup. yPelHeight is `uint16`
  so the spec's note about per-record ppem reaching 65535 is
  honoured even when RatioRange's `uint8` bracketing caps at 255.
  §7.3.5 forbids `VDMX` in variable fonts — we parse it whenever
  present and leave the cross-check to the caller, matching the
  `hdmx` policy),
  `meta` (metadata table, ISO/IEC 14496-22:2019 §5.7.6 — the
  16-byte header (`uint32 version`, `uint32 flags`, `uint32
  reserved`, `uint32 dataMapsCount`) plus a sorted-by-disk-order
  `DataMap[dataMapsCount]` array of `(Tag, Offset32 dataOffset,
  uint32 dataLength)` records and the payload bytes themselves.
  The table is the OpenType-level grab-bag for font-wide key/value
  metadata pairs keyed by four-character ASCII tags. §5.7.6.2
  reserves two registered tags — `'dlng'` (design languages) and
  `'slng'` (supported languages), both UTF-8 ASCII text with
  §5.7.6.3-grammar comma-separated ScriptLangTag values — and two
  Apple-reserved tags (`'appl'`, `'bild'`); vendor-private tags
  follow the §5.7.6.2 paragraph 4 uppercase + digit grammar. The
  parser enforces `version == 1` per §5.7.6.1, `flags == 0` per
  the spec's "currently unused" mandate, the §5.7.6.2 tag
  character class (letter-led, letters / digits / trailing spaces
  only) at every DataMap.tag, the in-bounds invariant on every
  `dataOffset + dataLength` slice (out-of-range payload rejected
  as `BadStructure`), and caps `dataMapsCount` at 1024 to bound
  worst-case allocation. The `reserved` field is surfaced rather
  than validated — §5.7.6.1's NOTE acknowledges that legacy Apple
  TrueType fonts may carry a non-zero data offset there. Document
  order is preserved in `MetaTable::records()` so tooling can
  round-trip the table without re-sorting. `Font::has_meta()` and
  `Font::meta_table()` expose the parsed table; `Font::meta_record(tag)`
  returns the first record matching a tag (honouring §5.7.6.1's
  "any instances after the first may be ignored" rule for the
  registered single-record tags); `Font::meta_design_languages()`
  and `Font::meta_supported_languages()` are convenience
  accessors that decode `'dlng'` / `'slng'` payloads as UTF-8
  text. A free-function `script_lang_tags(payload)` splits a
  `'dlng'` / `'slng'` value into the §5.7.6.3 ScriptLangTag
  fragments — comma-separated, trimmed, non-empty, ASCII-only,
  and rejecting leading / trailing / doubled hyphens per the
  spec's BNF; deeper validation against the IANA Language Subtag
  Registry and ISO 15924 stays in the caller),
  `PCLT` (PCL 5 table, ISO/IEC 14496-22:2019 §5.7.7 — the fixed
  54-byte struct of PCL 5 font-selection attributes, "strongly
  discouraged for OFF fonts with TrueType outlines" per the spec
  but still shipped by legacy faces. Every packed word decodes
  through typed accessors: FontNumber splits into the
  native-vs-converted MSB, the 7-bit HP-assigned vendor letter,
  and the 24-bit vendor-assigned id; Style splits into structure
  (bits 5–9) / appearance width (bits 2–4) / posture (bits 0–1)
  with the reserved top 6 bits surfaced; TypeFamily splits into
  the 4-bit HP vendor code + 12-bit family code; SymbolSet
  follows the §5.7.7 rule "the least significant 5 bits, when
  added to 64, is the ASCII value of the symbol set ID field"
  (all eight spec example values round-trip, e.g. 629 → 19U).
  The 16-byte Typeface and 6-byte FileName ASCII fields trim
  trailing pad to `&str` (with raw-byte fallbacks), the 8-byte
  CharacterComplement decodes to a big-endian u64 with
  `provides_collection(bit)` honouring the cleared-bit-means-
  provided polarity established by the spec's worked examples
  and `is_unicode_indexed()` reading bit 0 per "Bit 0 must
  always be cleared when the font elements are provided in
  Unicode order"; StrokeWeight / WidthType surface raw with
  `*_is_valid()` range checks against the §5.7.7 "-7 to 7" /
  "-5 to 5" validity sentences, and SerifStyle splits into the
  6-bit serif value + 2-bit serif/contrast class. `majorVersion
  != 1` is rejected per "The current PCLT table version is 1.0";
  `minorVersion` and the trailing Reserved pad byte are surfaced
  raw. `Font::has_pclt()` / `Font::pclt_table()` expose the
  parsed table),
  `DSIG` (digital signature table, ISO/IEC 14496-22:2019 §8.x — the
  8-byte header (`uint32 version == 1`, `uint16 numSignatures`,
  `uint16 flags`) plus a `SignatureRecord[numSignatures]` array
  (`uint32 format`, `uint32 length`, `Offset32 offset`) and the
  signature blocks themselves. For the only block format the spec
  defines — Signature Block Format 1 — the `reserved1` / `reserved2` /
  `signatureLength` sub-header is decoded and the PKCS#7 packet is
  surfaced **raw** as a borrowed `&[u8]` (`Signature::pkcs7_packet`); the
  PKCS#7 / X.509 / ASN.1 contents are *not* parsed and the signature is
  *not* verified — that is the host application's policy decision and is
  out of scope for a font-table parser, matching the raw-payload policy
  used for `sbix` / `CBDT` / `SVG ` blobs. The `signatureLength` field is
  bounds-checked against its block, every `SignatureRecord` block range
  against the table, and `version == 1` enforced per the spec; the
  "cannot be resigned" permission bit (flags bit 0) is decoded into
  `DsigTable::cannot_be_resigned()`. Unrecognised block formats surface
  their format id, declared length, and raw block bytes for
  forward-compatibility. `Font::has_dsig()` / `Font::dsig_table()` expose
  the parsed table),
- `name` table: full accessor API beyond family / full name — the
  registered nameID registry (`name_id` constants), typed accessors
  (subfamily, PostScript, version, copyright, trademark, manufacturer,
  designer, description, vendor / designer / licence URLs, typographic
  family / subfamily with the TN5149 §1.4 fallback), exact-locale lookup
  (`name_string_for(id, platform, language)`), and full record
  enumeration (`name_records()` → `NameRecord` with the `(platform,
  encoding, language, name)` locator tuple). Macintosh non-Roman scripts
  surface their locator + raw bytes but decode to `None` (legacy
  codepage tables not staged).
- Legacy `kern` table (ISO/IEC 14496-22:2019 §5.7.3) — both subtable
  formats the spec defines for the Microsoft / OpenType header variant:
  **Format 0** (a sorted, binary-searchable list of explicit
  `(left, right) → value` pairs) and **Format 2** (the class-based
  two-dimensional array — left and right glyphs map to classes through
  per-side class tables, and the kerning value is the array cell at
  `(leftClass, rightClass)`, addressed through the spec's pre-multiplied
  class values). Formats 1 and 3..255 are reserved by the spec and
  skipped, as are "minimum" (floor, not delta) and non-horizontal
  subtables. Kerning subtables are additive, so `KernTable::lookup` sums
  every matching format-0 pair and format-2 cell. The Apple `kern` header
  variant is accepted structurally but its subtable bodies are not decoded
  (the byte layout is not in the staged spec).
- `GSUB` LookupType 1 (single substitution: positional forms,
  small-caps, vertical alternates), LookupType 2 (multiple
  substitution — split one input glyph into N), LookupType 3
  (alternate substitution — `aalt` / `salt` per-coverage alternates),
  LookupType 4 (ligature substitution — exposed both as a "walk every
  lookup" helper and as a lookup-index-specific apply path for
  feature-driven shaping of `liga` / `rlig` / `dlig`), LookupType 5
  (contextual substitution — formats 1 / 2 / 3, predecessor of LT6
  minus backtrack/lookahead), LookupType 6 (chained contexts
  substitution — formats 1 / 2 / 3, with recursive dispatch into
  nested LookupType 1 / 2 / 3 / 4 / 5 / 6 sub-lookups), and LookupType
  8 (reverse chained context single substitution — used by some Arabic
  fonts). All sit behind a ScriptList / FeatureList walk so callers
  can ask "which lookup indices implement feature `init` for script
  `arab`?"
- `GSUB` **FeatureVariations** (ISO/IEC 14496-22:2019 §6.2.9) — the
  version-1.1 header's `featureVariationsOffset` is decoded so a
  variable font can swap the lookups behind a feature for an alternate
  set at the current variation instance (the canonical use is
  optical-size- or weight-conditional substitution). The shared
  `FeatureVariations` / `ConditionSet` / `ConditionTableFormat1` (font
  variation axis range, the only defined condition format) /
  `FeatureTableSubstitution` substructure evaluates each record's
  AND-ed condition set against the avar-bent normalised coordinate
  vector and applies the §6.2.9 first-match rule (universal-match on a
  zero condition-set offset; unrecognised condition formats and
  unsupported substitution-table versions both fail the record so a
  later record can win, the spec's forward-compatibility behaviour).
  `Font::gsub_features_for_script_at_instance(script, lang)` returns the
  per-feature lookup lists for the current instance — identical to
  `gsub_features_for_script` for static fonts, v1.0 headers, or
  instances matching no condition set; `Font::gsub_has_feature_variations()`
  gates. Set the instance with `set_variation_coords` first. Alternate
  feature tables keep the default feature's tag per §6.2.9.
- `GPOS` LookupType 1 (single positioning — formats 1 + 2),
  LookupType 2 (pair-adjustment / kerning), LookupType 3 (cursive
  attachment — entry/exit anchor pairs for Arabic Nastaliq +
  script-font cursive chaining), LookupType 4 (mark-to-base
  attachment), LookupType 5 (mark-to-ligature attachment — closes
  the `fi`-ligature + above-mark gap), LookupType 6 (mark-to-mark
  stacking), LookupType 7 (contextual positioning — `SequenceContext`
  formats 1 / 2 / 3, the non-chained sibling of LT 8, with nested
  LT 1 / 2 / 3 / 4 / 6 / 7 / 8 dispatch), and LookupType 8
  (chained-context positioning — formats 1 / 2 / 3, with nested
  LT 1 / 2 / 3 / 4 / 6 / 8 dispatch).
  ExtensionPos (LookupType 9) is unwrapped transparently — both at
  the sub-table level (a LT-9 sub-table inside any lookup) and at
  the lookup level (a whole lookup whose `lookupType` is 9 wrapping
  any of the supported inner types). `Font::gpos_lookup_list()` +
  `Font::gsub_lookup_list()` enumerate every lookup as
  `(index, effective_type, subtable_count)` for shapers that need to
  find e.g. every chained-context lookup without probing each index.
  GPOS also exposes the same ScriptList / FeatureList walk as GSUB:
  `Font::gpos_features_for_script(script, lang)` resolves a feature tag
  (`kern` / `mark` / `mkmk` / `curs` / `cpsp` …) to the lookup-index
  list that implements it for the active script, with the required
  feature emitted first. A version-1.1 GPOS header's
  `featureVariationsOffset` is decoded through the shared §6.2.9
  FeatureVariations substructure so a variable font can swap the lookups
  behind a positioning feature at the current variation instance —
  `Font::gpos_features_for_script_at_instance(script, lang)` runs the
  AND-ed condition-set evaluation against the avar-bent normalised
  coordinate vector (set the instance with `set_variation_coords`
  first), and `Font::gpos_has_feature_variations()` gates. Alternate
  feature tables keep the default feature's tag per §6.2.9.
- `GDEF` glyph-definition table — v1.0 / v1.2 / v1.3 headers, with
  `glyphClassDef` (skip-mark filter for GPOS / GSUB), `AttachList`
  (per-glyph contour-point indices), `LigCaretList` (per-ligature
  caret coordinates as `CaretValue::DesignUnits` / `ContourPoint` /
  `DesignUnitsWithDevice`), `MarkAttachClassDef` (the class compared
  against `lookupFlag.markAttachmentType`), `MarkGlyphSetsDef` (the
  Offset32 Coverage arrays consulted by `lookupFlag.useMarkFilteringSet`),
  and an `item_var_store_bytes()` raw slice for the v1.3
  ItemVariationStore feeding CaretValueFormat3 VariationIndex
  references through the same IVS decoder shared with MVAR / HVAR /
  VVAR.
- **Variable-font GPOS / GDEF VariationIndex resolution.** The shared
  `tables::device::DeviceOrVariationIndex` decoder reads the 6-byte
  Device / VariationIndex sub-table referenced from GPOS ValueRecords,
  GPOS AnchorFormat3 fields, and GDEF CaretValueFormat3, discriminating
  on `deltaFormat` (`0x0001`/`0x0002`/`0x0003` classic Device tables —
  2/4/8-bit MSB-first packed pixel deltas, unpacked for tooling — versus
  `0x8000` VariationIndex). A VariationIndex `(outer, inner)` pair is
  resolved against the GDEF ItemVariationStore at the current normalised
  instance, yielding a font-unit delta; classic Device tables contribute
  nothing at the font-unit layer (pixel snapping is render-time). Every
  GPOS positioning accessor has a variation sibling that folds these
  deltas in at the instance set via `set_variation_coords`:
  `Font::lookup_kerning_var` (PairPos `xAdvance`, honouring the spec's
  per-format device-offset base — PairSet for format 1, sub-table for
  format 2), `lookup_mark_to_base_var` / `lookup_mark_to_mark_var` /
  `lookup_mark_to_ligature_var` / `lookup_cursive_attachment_var`
  (AnchorFormat3 X/Y), `gpos_apply_lookup_type_1_var` (SinglePos), and
  `Font::ligature_carets_resolved` (CaretValueFormat3 carets to concrete
  font-unit coordinates; Format2 contour-point carets surface as `None`
  since they need the TT bytecode interpreter). BASE baseline positions
  resolve too — `Font::base_horiz_y_for_script_baseline_var` /
  `base_vert_x_for_script_baseline_var` fold a `BaseCoordFormat3`
  VariationIndex delta from the BASE ItemVariationStore. The static
  accessors are unchanged and equal the `_var` results at the default
  instance.
- **`Font::shape(text, script, lang, features)` — end-to-end OpenType
  shaping.** The integration capstone over the GSUB / GPOS / GDEF
  primitives above: it maps text to nominal glyphs through `cmap`, runs
  the requested features' GSUB lookups, then their GPOS lookups,
  returning a `Vec<ShapedGlyph>` carrying `glyph_id`, originating
  `cluster` (input byte index, preserved across ligation and
  multiple-substitution expansion), and `(x_offset, y_offset,
  x_advance, y_advance)` in font design units (TT Y-up). Per the
  common-table-format rules the *union* of lookups behind the active
  features is applied **in LookupList order** (not feature order), so
  lookups from different features interleave correctly. The GSUB stage
  drives single / multiple / alternate / ligature / contextual /
  chained-context / reverse-chained substitution; the GPOS stage seeds
  advances from `hmtx` then layers single / pair-kern / cursive /
  mark-to-base / mark-to-ligature / mark-to-mark / contextual /
  chained-context positioning, accumulating placement and advance
  deltas. Variation-instance-aware feature resolution is used so a
  variable font shaped after `set_variation_coords` honours its
  FeatureVariations substitutions. Lookup `lookupFlag` bits are honoured
  through the shared §2 ("Common Table Formats") skip predicate
  `Font::lookup_skips_glyph`: IGNORE_BASE_GLYPHS (`0x0002`),
  IGNORE_LIGATURES (`0x0004`), IGNORE_MARKS (`0x0008`), the high-byte
  MARK_ATTACHMENT_CLASS_FILTER (`0xFF00`), and USE_MARK_FILTERING_SET
  (`0x0010`) all resolve against the GDEF GlyphClassDef /
  MarkAttachClassDef / MarkGlyphSets structures
  (`Font::{gsub,gpos}_lookup_mark_filtering_set` reads the trailing
  `markFilteringSet` field at `6 + 2 * subTableCount`). The predicate
  drives the multi-glyph match paths: a ligature lookup with
  IGNORE_MARKS matches over the non-mark glyphs and keeps interspersed
  combining marks (to re-anchor in GPOS) while a lookup without the flag
  stays correctly blocked by an intervening mark, and GPOS pair-kerning +
  cursive attachment pair the current glyph with the next *non-skipped*
  glyph so a kern pair separated by an ignored mark still kerns. The
  mark-to-base / mark-to-mark / mark-to-ligature attachment scans locate
  the nearest *non-skipped* preceding attachment glyph through the same
  predicate, so a `mkmk` lookup carrying a mark-attachment class or mark
  filtering set binds only to glyphs in that class / set.
  Validated against DejaVu Sans (Latin
  `kern` advance reduction + `liga` ligation) and Noto Sans Arabic
  (`init`/`medi`/`fina` joining + `mark` mark-to-base attachment).
  General shaper — no script-specific glyph reordering (which the spec
  places in the text-processing client); driven directly by the
  requested feature set.
- sbix `'dupe'` indirection chasing (`sbix_glyph_resolved`): walks
  the per-strike indirection chain up to `SBIX_MAX_DUPE_DEPTH` (= 8)
  hops with explicit cycle detection (two-glyph, self-loop, and
  forward-chain overflow all bail to `None`). The raw `sbix_glyph`
  accessor still surfaces the `'dupe'` sentinel untouched for
  byte-level consumers.
- `EBDT` / `EBLC` embedded monochrome + grayscale bitmaps (ISO/IEC
  14496-22:2019 §5.6.2 / §5.6.3) — the location side (`EBLC`) reuses
  the same `CblcTable` walker that drives `CBLC` (it already accepts
  the `majorVersion == 2` header and all five IndexSubTable formats
  1–5), and the `EBDT` image-data decoder covers the five
  bit-packed §5.6.2.2 pixel formats: format 1 (small metrics,
  byte-aligned), 2 (small metrics, bit-aligned), 5 (bit-aligned data
  only, metrics lifted from the EBLC IndexSubTable 2/5 `BigGlyphMetrics`),
  6 (big metrics, byte-aligned) and 7 (big metrics, bit-aligned). Each
  `bitDepth` of 1 / 2 / 4 / 8 (§5.6.3.1) is unpacked MSB-first,
  left-to-right, top-to-bottom into a `width × height` row-major grid
  of one alpha-coverage byte per pixel, the `bitDepth`-bit sample
  scaled to the full 0..=255 range (1-bit "1 = black" → `0xFF`). The
  byte-aligned formats pad each row up to a byte boundary; the
  bit-aligned formats pack the whole glyph contiguously. `Font::
  glyph_gray_bitmap(gid, target_ppem)` returns a `GrayBitmap` from the
  closest strike (same closest-ppem-with-larger-wins tie-break as the
  colour path); `Font::has_gray_bitmaps()` / `Font::gray_strike_sizes()`
  gate and enumerate. Composite formats 8 (small metrics) and 9 (big
  metrics) per §5.6.2.2.8 / §5.6.2.2.9 are also assembled: the
  `EbdtComponent` array (`glyphID` + `int8 xOffset` + `int8 yOffset`)
  is decoded through `EbdtTable::lookup_composite` into a `CompositeBitmap`
  descriptor, then `glyph_gray_bitmap` resolves each component glyph out of
  the *same* strike and blits it onto the composite's canvas at its
  per-component `(xOffset, yOffset)` placement — nested composites are
  followed up to a bounded depth (`EBDT_COMPOSITE_MAX_DEPTH` = 8) with
  self-reference guarded, and out-of-canvas component pixels clip. Format 4
  (compressed) decodes to `None`; `bitDepth == 32` (BGRA) routes to the
  `CBDT` colour path instead.
- `EBSC` embedded bitmap scaling table (ISO/IEC 14496-22:2019 §5.6.4) —
  the 8-byte header (`uint16 majorVersion == 2`, `uint16 minorVersion`,
  `uint32 numSizes`) plus a `BitmapScale[numSizes]` array, each record
  carrying a `hori` / `vert` `SbitLineMetrics` pair (the §5.6.3.2 12-byte
  line-metrics struct shared with `EBLC`'s `BitmapSize`) and the four
  ppem bytes `(ppemX, ppemY, substitutePpemX, substitutePpemY)`. `EBSC`
  carries no glyph imagery: each `BitmapScale` declares a *synthesised*
  strike at `(ppemX, ppemY)` produced by scaling the real
  `EBLC`/`EBDT` strike at `(substitutePpemX, substitutePpemY)` — the
  spec's motivating case is small Kanji sizes where scaling an authored
  bitmap reads better than scan-converting an outline. `majorVersion` is
  pinned to 2 (the minor version is surfaced rather than fixed so a
  future `2.x` revision still decodes); `numSizes` is capped at 256 to
  bound allocation. `Font::has_ebsc()` / `Font::ebsc_table()` expose the
  parsed table and `Font::ebsc_target_sizes()` lists the synthesisable
  `(ppemX, ppemY)` targets. `Font::glyph_gray_bitmap_scaled(gid,
  target_ppem)` resolves a glyph at the `BitmapScale` whose target
  `ppemY` matches, pulling the substitute strike's pixels and scaling the
  per-glyph metrics (width / height / bearings / advance) independently
  in X and Y by the §5.6.4 `target / substitute` ppem ratio, rounded to
  the nearest integer pixel; the source pixel grid passes through
  unresampled so the consumer crate can resample at its chosen filter
  quality (§5.6.4 leaves the actual scaling to the rasteriser). The
  reported `width` / `height` are the scaled dimensions the resampled
  grid should target.
- `COLR` / `CPAL` tables — the palette-indexed colour-glyph mechanism.
  `COLR` **v0** maps a base glyph to a flat back-to-front layer stack
  (`Font::color_layers(gid)`), each layer tagged with a `CPAL`
  palette-entry index (`0xFFFF` = renderer foreground). `CPAL` **v0**
  resolves `(palette, entry)` to sRGB RGBA (`Font::cpal_color`,
  `Font::cpal_palette`); **v1** adds the full sidecar (ISO/IEC
  14496-22:2019 §5.7.11): per-palette type flags
  (`USABLE_WITH_LIGHT_BACKGROUND` / `USABLE_WITH_DARK_BACKGROUND` via
  `Font::cpal_palette_type`), per-palette UI labels
  (`Font::cpal_palette_label` → a `name`-table ID, e.g. "High
  Contrast"), and per-entry UI labels applied across all palettes
  (`Font::cpal_palette_entry_label` → a `name`-table ID, e.g. "Outline"
  / "Fill"); both label accessors map the `0xFFFF` "no label" sentinel
  to `None`. COLR **v1** paint graphs (gradients / transforms /
  composites) remain out of scope (docs gap — the paint-graph spec is
  not in the docs tree).
- `SVG ` table (ISO/IEC 14496-22:2019/Amd.1:2020 §5.5.1) — the fourth
  colour-glyph mechanism, carrying per-glyph-range SVG 1.1 vector
  documents (an alternative to `COLR`/`CPAL`, `CBDT`/`CBLC`, and `sbix`).
  The 10-byte header (`version`, `offsetToSVGDocumentList`, `reserved`)
  and the SVGDocumentList (`numEntries` + 12-byte `SVGDocumentRecord[]`,
  each `startGlyphID` / `endGlyphID` / `Offset32 svgDocOffset` /
  `uint32 svgDocLength`) decode with the §5.5.1 invariants enforced at
  parse time: `version == 0`, the document-list offset non-zero and in
  bounds, `numEntries` non-zero, `startGlyphID ≤ endGlyphID` per record,
  the strictly-ascending-disjoint range ordering (`startGlyphID` greater
  than the previous record's `endGlyphID`, so the ranges never overlap
  or touch), non-zero `svgDocOffset` / `svgDocLength`, and each document
  slice in bounds (`svgDocOffset` measured from the SVGDocumentList
  start, not the table start). Document payloads are surfaced **raw** —
  plain UTF-8 SVG 1.1 markup *or* gzip-encoded — with
  `SvgDocument::is_gzip_encoded()` testing the §5.5.2 `0x1F 0x8B 0x08`
  gzip magic; actual deflate inflation and XML parsing are left to the
  consumer renderer, matching the raw-payload policy already used for
  `sbix` PNG/JPEG/TIFF blobs and `CBDT` PNG strikes. Two records may
  point at one document so a single SVG covers discontinuous glyph-ID
  ranges (§5.5.1 NOTE); both ranges still resolve. `Font::has_svg()` /
  `Font::svg_table()` gate and expose the table, and
  `Font::svg_document(gid)` binary-searches the sorted range records to
  resolve the document covering a glyph. The §5.5.2 SVG capability
  restrictions (no `<text>` / `<script>` / `<a>` elements, no relative
  `em` / `ex` units, …) are a renderer concern, not a table-decode one.
- `CFF ` table — PostScript (Type 2 charstring) outlines, so
  OTTO-flavoured fonts now produce glyph outlines. The `tables::cff`
  module walks the Compact Font Format container (Adobe TN #5176: fixed
  header, Name / Top-DICT / String / Global-Subr INDEXes, the
  Top-DICT-referenced CharStrings INDEX, charset formats 0/1/2, the
  Private DICT + local subrs) and runs each glyph's Type 2 charstring
  (Adobe TN #5177) through a full interpreter — every path operator
  (moveto/lineto/curveto families incl. `hhcurveto`/`vvcurveto`/
  `hvcurveto`/`vhcurveto`/`rcurveline`/`rlinecurve` and the
  `flex`/`hflex`/`hflex1`/`flex1` hints), the stem-hint operators with
  `hintmask`/`cntrmask` mask-byte skipping, the arithmetic/storage/
  conditional escaped operators, and biased `callsubr`/`callgsubr`/
  `return`/`endchar` with depth-bounded recursion. Cubic Béziers are
  flattened to on-curve polylines so CFF and TrueType outlines share one
  `TtOutline`. CID-keyed fonts work end-to-end (`ROS` → FDArray +
  FDSelect formats 0/3 select per-glyph Font-DICT local subrs and
  `nominalWidthX`). `Font::glyph_outline` transparently falls back to
  CFF when `glyf` is absent; `Font::has_cff_outlines` / `cff_table` /
  `is_cid_keyed` gate and expose it. Glyph names resolve through the
  charset: the walker keeps the String INDEX and the 391 CFF standard
  strings (Adobe TN #5176 Appendix A), so `CffTable::string_for_sid` /
  `CffTable::glyph_name(gid)` map a glyph to its PostScript name, and
  `Font::glyph_name` falls back to the CFF charset when the `post` table
  has no names (the common OTTO `post` v3.0 case).
- `CFF2` table — variable PostScript outlines (OpenType CFF2). The
  `tables::cff2` module walks the CFF2 container (fixed 5-byte header +
  `topDictSize`, Top DICT, Global Subr INDEX, CharStrings INDEX,
  VariationStore, the always-present FDArray + optional FDSelect formats
  0/3, per-Font-DICT Private DICT + local subrs + default `vsindex`) and
  renders the outline of each glyph **at any variation instance**. CFF2
  INDEXes carry a 32-bit count (`Index::parse_wide`); the shared Type 2
  interpreter has a CFF2 mode that suppresses the width prefix, ends at
  the charstring's data boundary, and implements `vsindex` / `blend` —
  `blend` computes `default + Σ scalarᵣ · deltaᵣ` using the per-`vsindex`
  region scalars read from the VariationStore at the target instance
  (`mvar::ItemVariationStore::region_scalars`), collapsing to the default
  shape when coordinates are unset. `Cff2Table::glyph_outline_at(gid,
  coords)` renders any instance; `Font::glyph_outline` feeds the
  avar-bent normalised coordinates into the CFF2 path, so a variable CFF2
  font retargets with `Font::set_variation_coords` just like the `gvar`
  path. `Font::glyph_outline` falls back to CFF2 when `glyf`/`CFF ` are
  absent; `Font::has_cff2_outlines` / `cff2_table` expose it.
- `MATH` table — mathematical typesetting parameters (ISO/IEC
  14496-22:2019 §6.3.6). `tables::math` decodes the full table:
  `MathConstants` (the four scalar fields, all 51 `MathValueRecord`
  constants addressed by name through `math::constant::*`, and the
  trailing `radicalDegreeBottomRaisePercent`), `MathGlyphInfo`
  (per-glyph italics correction, top-accent attachment, extended-shape
  flag, and height-dependent four-corner `MathKern`), and `MathVariants`
  (`minConnectorOverlap`, ready-made stretchy variants, and general
  glyph-assembly parts with the extender flag, for both vertical and
  horizontal growth). Coverage lookups reuse the shared common-layout
  Coverage parser. `Font::has_math` / `Font::math_table` expose it.
  Variable-font value resolution (§6.3.6.2.1): every `MathValueRecord`
  carries an optional device / VariationIndex offset measured from its
  parent sub-table, and the `*_resolved` accessors
  (`MathConstants::value_resolved`, `MathGlyphInfo`'s
  italics-correction / top-accent / math-kern resolvers, and the
  glyph-assembly italics resolver) — surfaced font-wide through
  `Font::math_constant_var` / `math_italics_correction_var` /
  `math_top_accent_attachment_var` / `math_kern_var` /
  `math_assembly_italics_correction_var` — fold in the VariationIndex
  delta against the GDEF `ItemVariationStore` at the current instance.
  Classic ppem-indexed Device tables (a render-time concern) contribute
  no font-unit adjustment, so a static font's resolved values equal its
  plain design-unit values.
- `JSTF` table — justification suggestions (ISO/IEC 14496-22:2019
  §6.3.5). `tables::jstf` decodes the GSUB/GPOS-shaped navigation:
  `JstfTable` (script-record list), `JstfScript` (extender glyphs —
  e.g. Arabic kashidas — default + per-language `JstfLangSys`),
  `JstfLangSys` (priority-ordered suggestions), and `JstfPriority`
  exposing all ten slots via the `JstfMod` enum — the eight
  enable/disable slots resolve to GSUB/GPOS lookup-index lists
  (`mod_list`), the two `JstfMax` slots to an inline lookup count.
  `Font::has_jstf` / `Font::jstf_table` expose it.
- Adobe Glyph List (AGL) glyph-name → Unicode resolution
  (`glyph_name_to_codepoints` / `glyph_name_to_char`). Direct table
  lookup against the embedded AGL 2.0 data: a PostScript glyph name
  (e.g. from a `post` v2 table or a CFF charset) maps to its Unicode
  scalar-value sequence; ligature / base+points names yield a short
  sequence. The AGL Specification's algorithmic fallback (suffix
  stripping, `uniXXXX` synthetic names) is intentionally out of scope —
  only the staged `glyphlist.txt` data drives it.
- `MVAR` (font-wide Metrics Variations, ISO/IEC 14496-22:2019 §7.3.6)
  with the shared `ItemVariationStore` substructure (§7.2.3) decoded
  inline: `Font::metric_variation_delta(tag)` returns the interpolated
  signed adjustment for any §7.3.6.3 metric tag ('xhgt', 'cpht', 'stro',
  'unds', 'hasc', 'hdsc', 'gsp0'…'gsp9', …) at the current variation
  coordinates. Region scalars are computed per §7.1 (peak-0 axes
  ignored, opposite-sign coords zero, linear interpolation on rising /
  falling edges), `avar` is honoured so wght=700 with a non-identity
  axis-value map produces the bent normalised value the spec mandates,
  and the `valueRecordSize` field is treated as the record stride so
  minor-version bumps that grow ValueRecord (per the §7.3.6.1 note)
  decode correctly with the unknown trailing bytes ignored.
- `HVAR` (per-glyph horizontal-metrics variations, ISO/IEC 14496-22:2019
  §7.3.5) with the same `ItemVariationStore` substructure shared with
  MVAR: `Font::advance_width_variation_delta(gid)` returns the
  interpolated advance-width adjustment for `glyph_id` at the current
  variation coordinates, and `Font::lsb_variation_delta(gid)` /
  `Font::rsb_variation_delta(gid)` cover the optional side-bearing
  mappings. The optional `DeltaSetIndexMap` sub-table (§7.3.5.2) is
  decoded for all four supported entry sizes (1 / 2 / 3 / 4 bytes per
  entry, 1..16 inner-index bits) with the §7.3.5.2 "glyph IDs beyond
  mapCount-1 use the last entry" clamp; when `advanceWidthMappingOffset`
  is zero, the §7.3.5.3 implicit form (outer = 0, inner = glyph ID) is
  used instead. For callers that want the per-instance metric directly,
  `Font::glyph_advance_varied(gid)` / `Font::glyph_lsb_varied(gid)` fuse
  the static `hmtx` value with the HVAR delta (rounded + clamped).
- `VVAR` (per-glyph vertical-metrics variations, ISO/IEC 14496-22:2019
  §7.3.8) reusing the same `ItemVariationStore` + `DeltaSetIndexMap`
  substructures as HVAR. `Font::advance_height_variation_delta(gid)`
  returns the interpolated advance-height adjustment for `glyph_id`;
  `Font::tsb_variation_delta(gid)` and `Font::bsb_variation_delta(gid)`
  cover the optional top- and bottom-side-bearing mappings; and
  `Font::vorg_variation_delta(gid)` covers the CFF2-only vertical-
  origin Y mapping (§7.3.8.2 final paragraph: "Mappings and variation
  data for vertical origins are not used in fonts with TrueType
  outlines"). The implicit "outer=0, inner=gid" form applies to
  advance heights when `advanceHeightMappingOffset` is zero, matching
  the §7.3.8.2 cross-reference back to §7.3.5.3.
  `Font::glyph_advance_height_varied(gid)` fuses the static `vmtx`
  advance height with the VVAR delta for the current instance.
- `STAT` (style attributes table, ISO/IEC 14496-22:2019 §7.3.7) — v1.0
  / v1.1 / v1.2 headers (the v1.0 deprecated form is parsed and its
  missing `elidedFallbackNameID` defaulted to the conventional name ID
  2 = "Regular"). `Font::stat_axes()` exposes the §7.3.7.2 design-axis
  records (`axisTag` / `axisNameID` / `axisOrdering`) with the
  `designAxisSize` stride honoured so future minor-version growth
  decodes transparently. `Font::stat_axis_values()` exposes every
  §7.3.7.3 axis value record — format 1 (single value), format 2
  (nominal + `[rangeMin, rangeMax]` with the `0x80000000` / `0x7FFFFFFF`
  ±∞ sentinels surfaced as `STAT_RANGE_MIN_NEG_INFINITY` /
  `STAT_RANGE_MAX_POS_INFINITY`), format 3 (single + `linkedValue` for
  style-linked Bold/Italic UI), and format 4 (multi-axis combinations
  for non-analytic instance names with the spec's "different axisIndex
  per record" rule enforced). The
  `OLDER_SIBLING_FONT_ATTRIBUTE` / `ELIDABLE_AXIS_VALUE_NAME` flag bits
  are decoded into `is_older_sibling_font_attribute()` /
  `is_elidable()` convenience accessors, and `Font::stat_axis_values_for_tag`
  filters the array down to a single axis (resolving format-4
  contributors that touch it).
- `gvar` **composite-glyph variation** (ISO/IEC 14496-22:2019 §7.3.4.3) —
  `Font::glyph_outline` now retargets composite glyphs (accented Latin,
  CJK radicals, …) at the active variation instance, not just simple
  glyphs. For a composite the gvar packed point numbers address the
  *components* (pseudo-points `0..componentCount`) plus the four trailing
  lsb / rsb / tsb / bsb phantom points — **not** flattened outline points;
  `GvarTable::glyph_component_deltas` interpolates the per-component
  `(dx, dy)` placement deltas and `GlyfTable::glyph_outline_var` folds each
  into the component's `argument1` / `argument2` X/Y offset (point-matched
  components take no delta, and a `SCALED_COMPONENT_OFFSET` component scales
  the delta-adjusted offset). Crucially each component glyph is re-decoded
  with **its own** gvar deltas applied before placement, matching the
  spec's "most deeply-nested first" order — verified against
  InterVariable.ttf, where the base 'e' sub-outline inside a varied 'é'
  equals the standalone varied 'e' outline up to a single component
  offset across the wght axis. Phantom-point deltas (metrics) are out of
  scope of this geometry path.
- `gvar` **inferred-delta (IUP) interpolation for simple glyphs**
  (ISO/IEC 14496-22:2019 §7.3.4.4) — variable fonts list explicit deltas
  for only the structurally significant points of each tuple and infer
  the rest along the contour. `Font::glyph_outline` now completes simple
  variable glyphs through `GvarTable::glyph_deltas_iup`, which takes the
  static contour structure (`SimpleOutlineInfo`: per-contour end indices
  + default grid coordinates) and infers un-referenced points **per
  region** on the *unscaled* deltas before the tuple scalar is applied —
  so the result is independent of region-processing order, as the spec
  requires. All §7.3.4.4 cases are covered: equal-coordinate neighbours
  propagate a shared delta (zero on disagreement); a single referenced
  point fills its whole contour; targets outside the neighbour range
  take the nearer neighbour's delta; targets between neighbours linear-
  interpolate by proportional position (the spec worked example's +10.5
  reproduced). Phantom points are never inferred (spec NOTE). Verified
  against InterVariable.ttf: the majority of a glyph's points move under
  a strong weight change on both axis signs, and the varied outline
  stays within its derived bounding box — neither held before IUP, when
  un-referenced points stayed pinned.
- `cvar` **CVT variations** + `cvt ` **Control Value Table**
  (ISO/IEC 14496-22:2019 §7.3.2 / §5.3.2). The `cvt ` table is exposed
  as a raw `int16` FWORD array (`Font::cvt_count` / `Font::cvt_value`).
  `cvar` is decoded as a single tuple variation store (§7.2.2),
  reusing the `gvar` packed-point / packed-delta / tuple-scalar
  machinery — embedded peaks, intermediate regions, and shared /
  private point sets are all handled, with "point numbers" read as CVT
  indices and **no** IUP inference (per the §7.2.2.4 NOTE; omitted CVTs
  simply take no adjustment). `Font::cvt_deltas()` interpolates the
  per-CVT deltas for the current instance against the `avar`-bent
  normalised coordinates, and `Font::cvt_value_varied(i)` returns the
  saturated varied entry. (CVTs feed TrueType bytecode hinting, which
  this crate does not execute; the varied values are surfaced for a
  downstream interpreter.)

The companion [`oxideav-scribe`](https://github.com/OxideAV/oxideav-scribe)
crate consumes the outlines + shaping output to rasterise text to RGBA
bitmaps for subtitles and the scene compositor.

## Public API

```rust
use oxideav_ttf::Font;

let bytes = std::fs::read("DejaVuSansMono.ttf")?;
let font  = Font::from_bytes(&bytes)?;

// Metadata.
let _ = font.family_name();         // Some("DejaVu Sans Mono")
let _ = font.units_per_em();        // 2048
let _ = font.glyph_count();
let _ = font.ascent();
let _ = font.descent();
let _ = font.line_gap();

// name-table strings — typed accessors for the well-known nameIDs.
let _ = font.subfamily_name();      // Some("Book")
let _ = font.postscript_name();     // Some("DejaVuSans")
let _ = font.version_string();      // Some("Version 2.37")
let _ = font.copyright();
let _ = font.license_url();
let _ = font.vendor_url();

// Exact locale (no ranking): the Japanese family name, if the font
// ships one. (name_id::FAMILY, platform::WINDOWS, 0x0411 = ja-JP)
use oxideav_ttf::{name_id, platform};
let _ = font.name_string_for(name_id::FAMILY, platform::WINDOWS, 0x0411);

// Enumerate every name record with its (platform, encoding, language,
// nameID) locator tuple.
for rec in font.name_records() {
    let _ = (rec.platform_id, rec.encoding_id, rec.language_id, rec.name_id);
    if let Some(s) = &rec.string {
        let _ = s; // decoded UTF-8 (None for Mac non-Roman scripts)
    }
}

// Glyph lookup.
let gid_a = font.glyph_index('A').unwrap();
let _ = font.glyph_advance(gid_a);  // i16 advance width in font units
let _ = font.glyph_lsb(gid_a);
let _ = font.glyph_bounding_box(gid_a);
let _ = font.glyph_outline(gid_a)?; // contours of i16 points

// Vertical-layout metrics (vhea + vmtx). Present only on fonts that
// support top-to-bottom writing — typically CJK faces. The accessors
// return None on horizontal-only fonts.
if font.has_vertical_metrics() {
    let _ = font.vertical_ascent();      // i16, vertTypoAscender in v1.1
    let _ = font.vertical_descent();
    let _ = font.vertical_line_gap();
    let _ = font.advance_height_max();   // i16 per §5.7.9
    let _ = font.glyph_advance_height(gid_a);
    let _ = font.glyph_top_side_bearing(gid_a);
    // Y of the vertical origin = topSideBearing + glyf.yMax (§5.7.10).
    let _ = font.glyph_vertical_origin_y(gid_a);
}

// VORG — vertical origin (§5.4.4). CFF-flavoured CJK sfnts ship this
// to give the canonical Y of each glyph's vertical origin without the
// caller having to compute a bbox; the spec restricts the table to
// CFF sfnts so `vert_origin_y_from_vorg` returns None for any font
// with a `glyf` table (which is the §5.4.4 ignore policy).
if font.has_vorg() {
    let _ = font.vorg_default_vert_origin_y();   // i16 design units
    let _ = font.vert_origin_y_from_vorg(gid_a); // None on TrueType
    if let Some(table) = font.vorg_table() {
        for entry in table.metrics() {
            // entry.glyph_index, entry.vert_origin_y
            let _ = entry;
        }
    }
}

// Shaping helpers.
let gid_f = font.glyph_index('f').unwrap();
let gid_i = font.glyph_index('i').unwrap();
if let Some((replacement_gid, consumed)) = font.lookup_ligature(&[gid_f, gid_i]) {
    // `fi` ligature substitutes 2 input glyphs with `replacement_gid`.
    let _ = (replacement_gid, consumed);
}

let gid_v = font.glyph_index('V').unwrap();
let _ = font.lookup_kerning(gid_a, gid_v); // negative i16 in font units

// GSUB feature-tagged lookups (Arabic positional forms, small-caps, …).
// Discover which lookup indices implement `init` for script `arab`,
// then apply LookupType 1 to a single glyph id.
for feat in font.gsub_features_for_script(*b"arab", None) {
    if &feat.tag == b"init" {
        if let Some(beh) = font.glyph_index('\u{0628}') {
            for &li in &feat.lookup_indices {
                if let Some(initial_form) = font.gsub_apply_lookup_type_1(li, beh) {
                    let _ = initial_form;
                    break;
                }
            }
        }
    }
}

// LookupType 4 — ligature substitution dispatched per-feature.
// Resolve the `liga` feature for `latn` and apply each of its
// LookupType-4 lookups to a glyph run; the apply method returns
// (replacement_gid, consumed_count) on a hit.
for feat in font.gsub_features_for_script(*b"latn", None) {
    if &feat.tag == b"liga" {
        let f = font.glyph_index('f').unwrap();
        let i = font.glyph_index('i').unwrap();
        for &li in &feat.lookup_indices {
            if let Some((fi_gid, consumed)) = font.gsub_apply_lookup_type_4(li, &[f, i]) {
                let _ = (fi_gid, consumed); // typically (fi-codepoint-gid, 2)
                break;
            }
        }
    }
}

// LookupType 6 — chained-context substitution. Returns the rewritten
// run starting at `pos` (or None when no chain rule matches the
// (backtrack, input, lookahead) window). Formats 1 (glyph sequence),
// 2 (class-based) and 3 (coverage-based) are all supported.
for feat in font.gsub_features_for_script(*b"arab", None) {
    if &feat.tag == b"calt" {
        let run: Vec<u16> = vec![/* ... shaped Arabic glyph run ... */];
        for &li in &feat.lookup_indices {
            for pos in 0..run.len() {
                if let Some(rewritten) = font.gsub_apply_lookup_type_6(li, &run, pos) {
                    let _ = rewritten;
                    break;
                }
            }
        }
    }
}

// LookupType 2 — multiple substitution. Splits one input glyph into a
// sequence of replacement glyphs (e.g. some script normalisations that
// expand a precomposed glyph into base + mark cluster).
let some_gid = 42u16;
if let Some(seq) = font.gsub_apply_lookup_type_2(/* lookup_index */ 0, some_gid) {
    let _ = seq; // Vec<u16> of substitute glyphs
}

// LookupType 3 — alternate substitution. Each covered glyph carries an
// AlternateSet of alternates; the caller picks an index. Used by `aalt`
// / `salt` features.
for feat in font.gsub_features_for_script(*b"latn", None) {
    if &feat.tag == b"salt" {
        let glyph_a = font.glyph_index('a').unwrap();
        for &li in &feat.lookup_indices {
            // alternate_index = 0 picks the first registered alternate.
            if let Some(alt_a) = font.gsub_apply_lookup_type_3(li, glyph_a, 0) {
                let _ = alt_a;
                break;
            }
        }
    }
}

// LookupType 5 — contextual substitution (LT6 minus backtrack/lookahead).
// Same return shape as LookupType 6.
for feat in font.gsub_features_for_script(*b"arab", None) {
    if &feat.tag == b"calt" {
        let run: Vec<u16> = vec![/* ... shaped run ... */];
        for &li in &feat.lookup_indices {
            for pos in 0..run.len() {
                if let Some(rewritten) = font.gsub_apply_lookup_type_5(li, &run, pos) {
                    let _ = rewritten;
                    break;
                }
            }
        }
    }
}

// LookupType 8 — reverse chained context single substitution. Returns
// the replacement gid for `gids[pos]` when the (backtrack, input,
// lookahead) coverage triple matches. The spec mandates reverse-text
// processing: a higher-level shaper walks `pos` from right to left.
for feat in font.gsub_features_for_script(*b"arab", None) {
    if &feat.tag == b"isol" {
        let run: Vec<u16> = vec![/* ... shaped run ... */];
        for &li in &feat.lookup_indices {
            for pos in (0..run.len()).rev() {
                if let Some(replacement) = font.gsub_apply_lookup_type_8(li, &run, pos) {
                    let _ = replacement;
                }
            }
        }
    }
}

// GPOS LookupType 1 — single-glyph positioning. Returns four signed
// i16 deltas: x_placement / y_placement / x_advance / y_advance. Used
// by features like `cpsp` (capital spacing).
for (lookup_index, lookup_type, _sub_count) in font.gpos_lookup_list() {
    if lookup_type == 1 {
        if let Some(adj) = font.gpos_apply_lookup_type_1(lookup_index, gid_a) {
            let _ = (adj.x_placement, adj.y_placement, adj.x_advance, adj.y_advance);
        }
    }
}

// GPOS LookupType 3 — cursive attachment. Returns a CursiveAttachment
// with (entry, exit) anchor points (each Option). Chain glyph N+1's
// entry onto glyph N's exit: per-glyph delta = prev.exit - this.entry.
if let Some(attach) = font.lookup_cursive_attachment(gid_a) {
    let _ = (attach.entry, attach.exit);
}

// GPOS LookupType 5 — mark-to-ligature attachment. Pick the ligature
// component the mark sits over (0-indexed: 0 = first component, etc.).
// Returns (dx, dy) to shift the mark's pen origin.
if let Some(lig_gid) = font.glyph_index('\u{FEFB}') {
    // LAM-ALEF is a 2-component ligature; its second component (ALEF)
    // is index 1.
    if let Some(mark_gid) = font.glyph_index('\u{064E}') {
        // FATHA above LAM (component 0 of LAM-ALEF).
        let _ = font.lookup_mark_to_ligature(lig_gid, 0, mark_gid);
    }
}

// GPOS LookupType 8 — chained-context positioning. Returns a Vec of
// PosRecord(absolute glyph index, four-field PosValue). The shaper
// folds these deltas into its own glyph-position state.
for (lookup_index, lookup_type, _sub_count) in font.gpos_lookup_list() {
    if lookup_type == 8 {
        let run: Vec<u16> = vec![/* ... shaped run ... */];
        for pos in 0..run.len() {
            if let Some(records) = font.gpos_apply_lookup_type_8(lookup_index, &run, pos) {
                for r in records {
                    let _ = (r.glyph_index, r.value.x_advance, r.value.y_advance);
                }
            }
        }
    }
}

// LookupList enumeration — find every lookup of a given (effective,
// post-extension-unwrap) type without probing each index in turn.
let chain_pos_lookups: Vec<u16> = font
    .gpos_lookup_list()
    .into_iter()
    .filter_map(|(idx, ty, _)| (ty == 8).then_some(idx))
    .collect();
let _ = chain_pos_lookups;

// Unicode Variation Sequences (cmap format 14). Used by emoji
// presentation selectors and registered IVS for CJK.
let _ = font.lookup_variation('\u{1F600}', '\u{FE0F}'); // grinning face + VS-16

// Colour glyphs — four families covered:
//
//   COLR/CPAL: vector layer stack (Microsoft Segoe UI Emoji, Twemoji-Mozilla, …)
//   CBDT/CBLC: PNG-payload bitmap strikes (Noto Color Emoji and friends)
//   sbix:      Apple-style PNG/JPEG bitmap strikes (Apple Color Emoji)
//   SVG :      SVG 1.1 vector documents (per-glyph-range; Twitter Twemoji SVG, …)
//
if font.has_color_layers() {
    for layer in font.color_layers(gid_a) {
        let rgba = font.cpal_color(0, layer.palette_index); // Option<[u8;4]>
        let _ = (layer.layer_glyph_id, rgba);
    }
}
if font.has_color_bitmaps() {
    let _ = font.glyph_color_bitmap(gid_a, /* target_ppem */ 64);
}
if font.has_sbix() {
    // Raw access — `'dupe'` entries are surfaced as-is for callers
    // that want to introspect the indirection target themselves.
    let _ = font.sbix_glyph(gid_a, /* target_ppem */ 64);

    // Resolved access — chases `'dupe'` indirections within the
    // chosen strike up to `SBIX_MAX_DUPE_DEPTH` (= 8) hops with
    // explicit cycle detection; returns the first non-`'dupe'`
    // entry or `None` if the chain cycles / overflows / dangles.
    let _ = font.sbix_glyph_resolved(gid_a, 64);
}

// SVG — per-glyph-range SVG 1.1 vector colour-glyph documents. The
// returned document bytes are raw: plain UTF-8 markup or gzip-encoded
// (test with `is_gzip_encoded`). Inflation + XML parsing live in the
// consumer renderer.
if font.has_svg() {
    if let Some(doc) = font.svg_document(gid_a) {
        let _ = (doc.start_glyph_id, doc.end_glyph_id);
        if doc.is_gzip_encoded() {
            // RFC 1952/1951 deflate stream — inflate before parsing.
        } else {
            // doc.data is plain UTF-8 SVG 1.1 markup.
        }
    }
}

// TTC (TrueType Collection) — pick one subfont from a `.ttc` file.
let _ = oxideav_ttf::is_collection(&bytes);
let _ = Font::from_collection_bytes(&bytes, /* index */ 0);

// Variable fonts (fvar / avar / gvar). Pick a coord vector in
// user-space units (e.g. wght 100..900); glyph_outline() then
// returns the gvar-deltad outline.
let mut vfont = Font::from_bytes(&bytes)?;
if vfont.is_variable() {
    for axis in vfont.variation_axes() {
        // axis.tag, axis.min, axis.default, axis.max, axis.name_id
    }
    for inst in vfont.named_instances() {
        // inst.subfamily_name_id + inst.coords
    }
    // Set one axis by its tag (clamped to range), or snap to a named
    // instance — no manual index bookkeeping needed.
    vfont.set_axis_value(b"wght", 700.0);
    let _ = vfont.axis_value(b"wght"); // -> Some(700.0)
    vfont.apply_named_instance(0); // e.g. the first designer variant
    // (or set the whole vector at once:)
    let coords = vfont.variation_coords().to_vec();
    vfont.set_variation_coords(&coords);
    let bold = vfont.glyph_outline(vfont.glyph_index('A').unwrap())?;
    let _ = bold; // gvar-deltad + IUP-completed outline at this instance
}
```

## Shaping coverage

All seven public GSUB lookup types (1 single, 2 multiple, 3 alternate,
4 ligature, 5 contextual, 6 chained context, 8 reverse chained context)
are implemented. GPOS covers LookupTypes 1 (single), 2 (pair),
3 (cursive attachment), 4 (mark-to-base), 5 (mark-to-ligature),
6 (mark-to-mark), and 8 (chained context with nested dispatch);
LookupType 7 plays no shaping role of its own. ExtensionSubst (GSUB
LookupType 7) and ExtensionPos (GPOS LookupType 9) wrappers are
unwrapped transparently at both the sub-table and lookup level. Every
base cmap subtable format (0, 2, 4, 6, 8, 10, 12, 13) plus the
format-14 UVS sidecar is decoded.

## Not yet supported

- CFF / Type 2 charstrings — belongs in a sibling `oxideav-otf` crate.
- Bidi, Arabic shaping, Indic conjuncts, and other complex contextual
  shaping beyond the GSUB/GPOS lookup coverage above.
- TrueType bytecode hinting (modern anti-aliasing at ≥ 16 px does not
  need it).
- COLR **v1** paint graph (gradients, transforms, composites) — only
  the v0 flat layer stack is supported.
- avar **v2** delta-set index map (variable-axis remap). avar v2 is an
  OpenType 1.9 (post-2020) addition that is **not present in the
  in-tree spec** (`docs/text/opentype/`, whose Amd1 stops at the 2020
  colour-font update) — blocked on a docs update, not on
  implementation effort. The GPOS / GSUB FeatureVariations paths honour
  the current normalised instance but neither runs the avar v2 remap.
- The `STAT` format-2 overlapping-range tie-break (§7.3.7.3) is left to
  caller policy; the full document-order record array is exposed
  unchanged.

## Test fixtures

- `tests/fixtures/DejaVuSansMono.ttf` is the upstream DejaVu Sans
  Mono 2.37 under the Bitstream Vera license
  (see `tests/fixtures/DEJAVU-LICENSE`).
- `tests/fixtures/InterVariable.ttf` is Inter 4.0 (variable font,
  `wght` + `opsz` axes) under the SIL Open Font License 1.1
  (see `tests/fixtures/INTER-OFL-LICENSE.txt`).
- `tests/fixtures/NotoSansArabic-Regular.ttf` is Noto Sans Arabic
  2022 (used to exercise GSUB feature-tagged single substitution
  for the `arab` script's positional joining forms) under the SIL
  Open Font License 1.1 (see `tests/fixtures/NOTO-ARABIC-OFL-LICENSE.txt`).

## License

MIT — see [`LICENSE`](LICENSE).
