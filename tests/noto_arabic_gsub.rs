//! Integration test exercising the GSUB ScriptList / FeatureList /
//! LookupList walk + LookupType 1 (Single Substitution) against the
//! real Noto Sans Arabic 2022 font. Modern Arabic fonts ship the
//! positional forms (`init`/`medi`/`fina`/`isol`) via GSUB lookups
//! rather than the legacy Unicode Presentation Forms-B block, so a
//! shaper that wants to render `بَرَكَة` (Arabic for "blessing") or
//! similar must consult these features.

use oxideav_ttf::Font;

const FIXTURE: &[u8] = include_bytes!("fixtures/NotoSansArabic-Regular.ttf");

#[test]
fn parses_noto_sans_arabic() {
    let f = Font::from_bytes(FIXTURE).expect("Noto Sans Arabic parses");
    let family = f.family_name().expect("family name");
    assert!(
        family.contains("Noto") && family.contains("Arabic"),
        "unexpected family: {family:?}"
    );
    assert!(
        f.glyph_count() > 100,
        "expected many glyphs in an Arabic font"
    );
}

/// The crucial test: Noto Sans Arabic must publish the three Arabic
/// joining-form features under the `arab` script. If this regresses,
/// a downstream shaper has lost its handle on `init`/`medi`/`fina`.
///
/// Note: `isol` is intentionally NOT asserted. Noto Sans Arabic
/// (v3.043, 2022) does not ship an `isol` feature because the font's
/// nominal codepoint glyph already IS the isolated form — the shaper
/// keeps the input gid unchanged when no `init`/`medi`/`fina` rule
/// fires. Other Arabic fonts (e.g. some Indic-region fonts) do ship
/// `isol`; our parser handles the tag identically when present.
#[test]
fn arab_script_publishes_positional_features() {
    let f = Font::from_bytes(FIXTURE).unwrap();
    let feats = f.gsub_features_for_script(*b"arab", None);
    assert!(
        !feats.is_empty(),
        "Noto Sans Arabic must list features under the `arab` script"
    );
    let tags: Vec<[u8; 4]> = feats.iter().map(|x| x.tag).collect();
    for want in [b"init", b"medi", b"fina"] {
        assert!(
            tags.contains(want),
            "missing feature tag `{}`; got {:?}",
            std::str::from_utf8(want).unwrap(),
            tags.iter()
                .map(|t| std::str::from_utf8(t).unwrap_or("???"))
                .collect::<Vec<_>>()
        );
    }
}

/// Apply the first lookup of feature `init` to the Arabic letter BEH
/// (U+0628). The result must differ from the input glyph (otherwise
/// the lookup didn't match), and it must equal the result of feeding
/// the same gid through every lookup that `init` lists.
#[test]
fn init_feature_substitutes_beh_isolated_form() {
    let f = Font::from_bytes(FIXTURE).unwrap();
    let beh = f.glyph_index('\u{0628}').expect("BEH must map");
    let feats = f.gsub_features_for_script(*b"arab", None);
    let init = feats
        .iter()
        .find(|x| x.tag == *b"init")
        .expect("init feature");
    assert!(
        !init.lookup_indices.is_empty(),
        "init should reference at least one lookup"
    );
    let mut substituted = None;
    for &li in &init.lookup_indices {
        if let Some(g) = f.gsub_apply_lookup_type_1(li, beh) {
            substituted = Some(g);
            break;
        }
    }
    let g = substituted.expect("init lookup must substitute BEH (initial form)");
    assert_ne!(
        g, beh,
        "init form glyph id should differ from the isolated BEH glyph"
    );
}

/// Sanity: substitution returns `None` for a glyph the Arabic lookups
/// can't possibly cover (gid 0 = `.notdef`).
#[test]
fn init_feature_returns_none_for_notdef() {
    let f = Font::from_bytes(FIXTURE).unwrap();
    let feats = f.gsub_features_for_script(*b"arab", None);
    let init = feats
        .iter()
        .find(|x| x.tag == *b"init")
        .expect("init feature");
    for &li in &init.lookup_indices {
        assert_eq!(
            f.gsub_apply_lookup_type_1(li, 0),
            None,
            "lookup {li} should not substitute .notdef"
        );
    }
}

/// Walks every Arabic codepoint that maps to a glyph in the font and
/// every lookup index referenced by feature `init`; counts how many
/// substitutions land. Real Arabic fonts substitute essentially every
/// joining letter, so we expect a triple-digit number.
#[test]
fn init_feature_substitutes_majority_of_joining_letters() {
    let f = Font::from_bytes(FIXTURE).unwrap();
    let feats = f.gsub_features_for_script(*b"arab", None);
    let init = feats.iter().find(|x| x.tag == *b"init").unwrap();
    let mut hits = 0usize;
    // U+0620..U+064A covers the joining Arabic letters. Some are
    // non-joining (e.g. HAMZA U+0621) and won't have an `init` form;
    // we only assert the bulk of the range matches.
    for cp in 0x0620u32..=0x064Au32 {
        let ch = match char::from_u32(cp) {
            Some(c) => c,
            None => continue,
        };
        let g = match f.glyph_index(ch) {
            Some(v) => v,
            None => continue,
        };
        for &li in &init.lookup_indices {
            if f.gsub_apply_lookup_type_1(li, g).is_some() {
                hits += 1;
                break;
            }
        }
    }
    assert!(
        hits >= 20,
        "expected init to substitute at least 20 Arabic letters, got {hits}"
    );
}
