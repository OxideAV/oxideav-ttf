//! Adobe Glyph List (AGL) — glyph-name → Unicode resolution.
//!
//! A `post` (version 2.0) table or a CFF charset names glyphs with
//! PostScript glyph names rather than Unicode codepoints. To recover
//! "what character does this glyph stand for" from such a name, the
//! Adobe Glyph List provides the canonical name → Unicode-scalar-value
//! mapping. The list is a flat, two-field text table:
//!
//! ```text
//! A;0041
//! AEacute;01FC
//! dalethatafpatah;05D3 05B2
//! ```
//!
//! where the second field is one or more space-separated four-or-more
//! hex-digit Unicode scalar values (a single glyph name may stand for a
//! short sequence of codepoints — ligatures and Hebrew points-with-base
//! are the common cases).
//!
//! # Scope (clean-room boundary)
//!
//! This module implements **only the direct table lookup** — a glyph
//! name that appears verbatim in the AGL resolves to its codepoint
//! sequence. The AGL Specification additionally defines an *algorithm*
//! (drop the suffix after the first period, split component names on
//! underscore, interpret `uniXXXX` / `uXXXXX...` names as literal
//! codepoint sequences, then fall back to this table) for names not in
//! the table. That algorithm is described in the AGL Specification
//! §2/§6, which is **not** present in `docs/text/opentype/`; so the
//! algorithmic fallback (including the `uniXXXX` synthetic-name
//! convention) is intentionally **not** implemented here. Only the
//! staged `glyphlist.txt` data drives this module.
//!
//! Data provenance: `docs/text/opentype/spec/agl-glyphlist.txt`
//! (Adobe Glyph List, table version 2.0, dated 2002-09-20),
//! redistributed under its BSD-style licence (the copyright/licence
//! header is preserved verbatim in the embedded `agl-glyphlist.txt`).

use std::collections::HashMap;
use std::sync::OnceLock;

/// The Adobe Glyph List, embedded at build time.
///
/// The file is a verbatim copy of the staged
/// `docs/text/opentype/spec/agl-glyphlist.txt`, including its
/// BSD-style licence header (lines beginning with `#`).
const AGL_GLYPHLIST: &str = include_str!("agl-glyphlist.txt");

/// Lazily-parsed name → codepoint-sequence index over [`AGL_GLYPHLIST`].
///
/// Values are owned `Vec<u32>` because a single AGL name may map to a
/// short sequence of scalar values (ligatures, Hebrew base+points).
fn table() -> &'static HashMap<&'static str, Vec<u32>> {
    static TABLE: OnceLock<HashMap<&'static str, Vec<u32>>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut map = HashMap::new();
        for line in AGL_GLYPHLIST.lines() {
            // Comments start with '#'; blank lines are ignored. Both are
            // mandated by the AGL file-format description.
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((name, values)) = line.split_once(';') else {
                // A data line without a ';' is malformed; skip it rather
                // than guess. (The shipped file has none.)
                continue;
            };
            let mut cps = Vec::new();
            let mut ok = true;
            for hex in values.split(' ') {
                if hex.is_empty() {
                    continue;
                }
                match u32::from_str_radix(hex, 16) {
                    Ok(v) => cps.push(v),
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok && !cps.is_empty() {
                // Last writer wins; the shipped list has unique names.
                map.insert(name, cps);
            }
        }
        map
    })
}

/// Resolve an AGL glyph name to its Unicode scalar-value sequence.
///
/// Returns the codepoint sequence the name stands for, or `None` if the
/// name does not appear verbatim in the Adobe Glyph List. Most names map
/// to a single codepoint; ligature and combining-mark names map to a
/// short sequence (e.g. `fi` → `[U+0066, U+0069]`).
///
/// This is a direct table lookup only; see the module docs for the
/// deliberately-unimplemented algorithmic fallback.
pub fn glyph_name_to_codepoints(name: &str) -> Option<&'static [u32]> {
    table().get(name).map(|v| v.as_slice())
}

/// Resolve an AGL glyph name to a single `char`.
///
/// Returns `Some(c)` only when the name maps to exactly one Unicode
/// scalar value *and* that value is a valid `char` (not a surrogate).
/// Names that map to a multi-codepoint sequence (ligatures, base+points)
/// return `None` — use [`glyph_name_to_codepoints`] for those.
pub fn glyph_name_to_char(name: &str) -> Option<char> {
    match glyph_name_to_codepoints(name)? {
        [cp] => char::from_u32(*cp),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_basic_latin() {
        assert_eq!(glyph_name_to_char("A"), Some('A'));
        assert_eq!(glyph_name_to_char("space"), Some(' '));
        assert_eq!(glyph_name_to_char("zero"), Some('0'));
        assert_eq!(glyph_name_to_char("exclam"), Some('!'));
    }

    #[test]
    fn resolves_accented() {
        // AEacute;01FC
        assert_eq!(glyph_name_to_char("AEacute"), Some('\u{01FC}'));
        // AE;00C6
        assert_eq!(glyph_name_to_char("AE"), Some('\u{00C6}'));
    }

    #[test]
    fn unknown_name_is_none() {
        assert!(glyph_name_to_codepoints("definitely_not_a_glyph_name").is_none());
        assert!(glyph_name_to_char("definitely_not_a_glyph_name").is_none());
    }

    #[test]
    fn multi_codepoint_sequence() {
        // dalethatafpatah;05D3 05B2 — a base letter plus a Hebrew point.
        let seq = glyph_name_to_codepoints("dalethatafpatah").expect("present in AGL");
        assert_eq!(seq, &[0x05D3, 0x05B2]);
        // A multi-codepoint name has no single-char form.
        assert_eq!(glyph_name_to_char("dalethatafpatah"), None);
    }

    #[test]
    fn ligature_maps_to_presentation_form() {
        // In the AGL the 'fi' ligature name maps to the single
        // Alphabetic-Presentation-Forms codepoint U+FB01 (fi;FB01), not
        // to a decomposed 'f' 'i' pair — so it has a single-char form.
        let seq = glyph_name_to_codepoints("fi").expect("present in AGL");
        assert_eq!(seq, &[0xFB01]);
        assert_eq!(glyph_name_to_char("fi"), Some('\u{FB01}'));
        // 'ffi' likewise maps to U+FB03.
        assert_eq!(glyph_name_to_char("ffi"), Some('\u{FB03}'));
    }

    #[test]
    fn table_is_nonempty_and_stable() {
        // Sanity-floor: the shipped 2.0 list has several thousand
        // entries. A drastically smaller count would mean the embedded
        // file or the parser regressed.
        assert!(table().len() > 4000, "AGL table unexpectedly small");
        // Idempotent: the OnceLock returns the same map each call.
        let a = table() as *const _;
        let b = table() as *const _;
        assert_eq!(a, b);
    }

    #[test]
    fn comment_and_blank_lines_skipped() {
        // The licence header lines (starting with '#') must not leak in
        // as glyph names.
        assert!(glyph_name_to_codepoints("Copyright").is_none());
        assert!(glyph_name_to_codepoints("").is_none());
    }
}
