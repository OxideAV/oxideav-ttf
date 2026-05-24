# oxideav-ttf

Pure-Rust TrueType font parser for the [oxideav](https://github.com/OxideAV)
framework. Implements the sfnt container, the core OpenType tables, and
just enough of GSUB / GPOS to do Latin/Cyrillic/Greek/CJK shaping with
ligatures and kerning.

## Round-1 scope (this release)

- sfnt + table directory walker.
- `head`, `hhea`, `maxp`, `cmap` (base formats 0, 4, 6, 12 + format 14
  Unicode Variation Sequences as a sidecar), `name`, `OS/2`, `hmtx`,
  `loca`, `glyf` (simple + composite), `post`.
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
- `GDEF` (glyph class definitions, used to skip mark glyphs).
- Adobe Glyph List (AGL) glyph-name → Unicode resolution
  (`glyph_name_to_codepoints` / `glyph_name_to_char`). Direct table
  lookup against the embedded AGL 2.0 data: a PostScript glyph name
  (e.g. from a `post` v2 table or a CFF charset) maps to its Unicode
  scalar-value sequence; ligature / base+points names yield a short
  sequence. The AGL Specification's algorithmic fallback (suffix
  stripping, `uniXXXX` synthetic names) is intentionally out of scope —
  only the staged `glyphlist.txt` data drives it.

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
    let _ = font.sbix_glyph(gid_a, /* target_ppem */ 64);
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
- cmap formats 2, 8, 10, 13.
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
- sbix `'dupe'` chasing (the indirection sentinel is surfaced
  as-is; consumers chase it with their own cycle detection).
- avar **v2** delta-set index map (variable-axis remap).
- HVAR / VVAR / MVAR (per-glyph horizontal-metrics / vertical-metrics
  / per-table metric variations).
- gvar delta propagation into composite-glyph component offsets and
  the four phantom points.

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
