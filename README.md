# oxideav-ttf

Pure-Rust TrueType font parser for the [oxideav](https://github.com/OxideAV)
framework. Implements the sfnt container, the core OpenType tables, and
just enough of GSUB / GPOS to do Latin/Cyrillic/Greek/CJK shaping with
ligatures and kerning.

## Round-1 scope (this release)

- sfnt + table directory walker.
- `head`, `hhea`, `maxp`, `cmap` (base formats 0, 2, 4, 6, 12, 13 +
  format 14 Unicode Variation Sequences as a sidecar — format 2 is
  the legacy mixed-8-/16-bit "high-byte mapping through table" layout
  for pre-Unicode CJK fonts, format 13 is the "many-to-one range
  mappings" layout used by last-resort fonts),
  `name`, `OS/2`, `hmtx`, `loca`, `glyf` (simple + composite), `post`,
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
  VertAxis X coordinate).
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
- Legacy `kern` table (format 0).
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
- `GPOS` LookupType 1 (single positioning — formats 1 + 2),
  LookupType 2 (pair-adjustment / kerning), LookupType 3 (cursive
  attachment — entry/exit anchor pairs for Arabic Nastaliq +
  script-font cursive chaining), LookupType 4 (mark-to-base
  attachment), LookupType 5 (mark-to-ligature attachment — closes
  the `fi`-ligature + above-mark gap), LookupType 6 (mark-to-mark
  stacking), and LookupType 8 (chained-context positioning —
  formats 1 / 2 / 3, with nested LT 1 / 2 / 3 / 4 / 6 / 8 dispatch).
  ExtensionPos (LookupType 9) is unwrapped transparently — both at
  the sub-table level (a LT-9 sub-table inside any lookup) and at
  the lookup level (a whole lookup whose `lookupType` is 9 wrapping
  any of the supported inner types). `Font::gpos_lookup_list()` +
  `Font::gsub_lookup_list()` enumerate every lookup as
  `(index, effective_type, subtable_count)` for shapers that need to
  find e.g. every chained-context lookup without probing each index.
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
- sbix `'dupe'` indirection chasing (`sbix_glyph_resolved`): walks
  the per-strike indirection chain up to `SBIX_MAX_DUPE_DEPTH` (= 8)
  hops with explicit cycle detection (two-glyph, self-loop, and
  forward-chain overflow all bail to `None`). The raw `sbix_glyph`
  accessor still surfaces the `'dupe'` sentinel untouched for
  byte-level consumers.
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
  used instead.
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

// Colour glyphs — three families covered:
//
//   COLR/CPAL: vector layer stack (Microsoft Segoe UI Emoji, Twemoji-Mozilla, …)
//   CBDT/CBLC: PNG-payload bitmap strikes (Noto Color Emoji and friends)
//   sbix:      Apple-style PNG/JPEG bitmap strikes (Apple Color Emoji)
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
    let mut coords = vfont.variation_coords().to_vec();
    if let Some(i) = vfont.variation_axes().iter().position(|a| &a.tag == b"wght") {
        coords[i] = 700.0;
    }
    vfont.set_variation_coords(&coords);
    let bold = vfont.glyph_outline(vfont.glyph_index('A').unwrap())?;
    let _ = bold;
}
```

## Out of scope (round 2+)

- CFF / Type 2 charstrings — moves to a sibling `oxideav-otf` crate.
- Bidi, Arabic shaping, Indic conjuncts, complex contextual GSUB/GPOS.
- TrueType bytecode hinting (modern AA at ≥ 16 px does not need it).
- cmap formats 8 and 10 (Unicode supplementary-plane mixed-length
  encodings — the spec calls these out as rare too). Format 2 —
  legacy mixed-8-/16-bit high-byte-through-table for pre-Unicode CJK
  fonts — and format 13 — many-to-one ranges for last-resort fonts —
  both landed; see above.
- All GPOS lookup types except LookupType 7 (the now-fully-handled
  LookupType 9 ExtensionPos wrapper plays its role) are implemented:
  1 (single), 2 (pair), 3 (cursive attachment), 4 (mark-to-base),
  5 (mark-to-ligature), 6 (mark-to-mark), 8 (chained context with
  nested LT 1/2/3/4/6/8 dispatch). All seven public GSUB lookup
  types (1 single, 2 multiple, 3 alternate, 4 ligature, 5
  contextual, 6 chained context, 8 reverse chained context) are
  implemented; ExtensionSubst LookupType 7 (GSUB) and ExtensionPos
  LookupType 9 (GPOS) are unwrapped transparently for every type
  both at the sub-table and lookup level.
- COLR **v1** paint graph (gradients, transforms, composites) — only
  the v0 flat layer stack is supported.
- avar **v2** delta-set index map (variable-axis remap).
- gvar delta propagation into composite-glyph component offsets and
  the four phantom points.
- STAT (style attributes) landed in r217 — see above. The format-2
  overlapping-range tie-break (§7.3.7.3) is documented as caller
  policy; we expose the full document-order record array unchanged.

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
