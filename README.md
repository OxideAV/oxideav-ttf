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
- Legacy `kern` table (format 0).
- `GSUB` LookupType 1 (single substitution: positional forms,
  small-caps, vertical alternates), LookupType 4 (ligature
  substitution — exposed both as a "walk every lookup" helper and
  as a lookup-index-specific apply path for feature-driven shaping
  of `liga` / `rlig` / `dlig`), and LookupType 6 (chained contexts
  substitution — formats 1 / 2 / 3, with recursive dispatch into
  nested LookupType 1 / 4 / 6 sub-lookups). All three sit behind a
  ScriptList / FeatureList walk so callers can ask "which lookup
  indices implement feature `init` for script `arab`?"
- `GPOS` LookupType 2 (pair-adjustment / kerning).
- `GDEF` (glyph class definitions, used to skip mark glyphs).

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
- GSUB lookup types 2 (multiple substitution), 3 (alternate), 5
  (contextual), 8 (reverse chained context). LookupType 1 (single),
  LookupType 4 (ligature), and LookupType 6 (chained context) are
  implemented; ExtensionSubst LookupType 7 is unwrapped transparently
  for all three. GPOS lookup types 1/3/5/7/8/9 also remain deferred
  (pair / mark-base / mark-mark are implemented).
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
