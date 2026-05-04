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
- `GSUB` LookupType 4 (ligature substitution).
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

// Unicode Variation Sequences (cmap format 14). Used by emoji
// presentation selectors and registered IVS for CJK.
let _ = font.lookup_variation('\u{1F600}', '\u{FE0F}'); // grinning face + VS-16
```

## Out of scope (round 2+)

- CFF / Type 2 charstrings — moves to a sibling `oxideav-otf` crate.
- Bidi, Arabic shaping, Indic conjuncts, complex contextual GSUB/GPOS.
- Variable fonts (`fvar` / `gvar` / `MVAR`).
- TrueType bytecode hinting (modern AA at ≥ 16 px does not need it).
- cmap formats 2, 8, 10, 13.
- GSUB lookup types 1/2/3/5/6/7/8 and GPOS lookup types 1/3..9.

## Test fixture

`tests/fixtures/DejaVuSansMono.ttf` is the upstream DejaVu Sans Mono 2.37
under the Bitstream Vera license (see `tests/fixtures/DEJAVU-LICENSE`).

## License

MIT — see [`LICENSE`](LICENSE).
