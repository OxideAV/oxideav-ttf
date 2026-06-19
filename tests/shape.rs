//! End-to-end tests of the OpenType GSUB/GPOS shaping pipeline
//! ([`Font::shape`]) against the real font fixtures. These exercise the
//! full text → glyph-buffer → substitution → positioning path, validating
//! that the per-lookup-type GSUB/GPOS primitives compose into correct
//! positioned glyph runs.

use oxideav_ttf::Font;

const DEJAVU: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");
const ARABIC: &[u8] = include_bytes!("fixtures/NotoSansArabic-Regular.ttf");

/// Baseline: shaping with no features still produces one positioned
/// glyph per input character, with each advance equal to the nominal
/// `hmtx` advance and zero placement offsets.
#[test]
fn shape_without_features_is_nominal_cmap_plus_hmtx() {
    let f = Font::from_bytes(DEJAVU).unwrap();
    let text = "Hello";
    let sh = f.shape(text, *b"latn", None, &[]);
    assert_eq!(sh.len(), text.chars().count());
    for (i, (byte_idx, ch)) in text.char_indices().enumerate() {
        let gid = f.glyph_index(ch).unwrap();
        assert_eq!(sh[i].glyph_id, gid, "glyph id at {i}");
        assert_eq!(sh[i].cluster, byte_idx as u32, "cluster at {i}");
        assert_eq!(sh[i].x_offset, 0);
        assert_eq!(sh[i].y_offset, 0);
        assert_eq!(
            sh[i].x_advance,
            f.glyph_advance(gid) as i32,
            "nominal advance at {i}"
        );
    }
}

/// GPOS kerning: shaping "AVATAR" with the `kern` feature must reduce the
/// total horizontal advance versus the unkerned run, because the
/// AV / VA / TA / AR letter pairs all carry negative kerning in DejaVu
/// Sans. The kerned advance lands on the *left* glyph of each pair.
#[test]
fn shape_applies_latin_kerning() {
    let f = Font::from_bytes(DEJAVU).unwrap();
    let plain: i32 = f
        .shape("AVATAR", *b"latn", None, &[])
        .iter()
        .map(|g| g.x_advance)
        .sum();
    let kerned: i32 = f
        .shape("AVATAR", *b"latn", None, &[*b"kern"])
        .iter()
        .map(|g| g.x_advance)
        .sum();
    assert!(
        kerned < plain,
        "kerned advance {kerned} should be less than unkerned {plain}"
    );
    // The glyph buffer length is unchanged by a positioning-only stage.
    assert_eq!(f.shape("AVATAR", *b"latn", None, &[*b"kern"]).len(), 6);
}

/// GSUB ligature substitution: shaping "office" with the `liga` feature
/// must collapse the `ffi` (or `fi`) cluster into fewer glyphs than the
/// six input characters, and the resulting ligature glyph must inherit
/// the cluster of its first component.
#[test]
fn shape_applies_latin_ligature() {
    let f = Font::from_bytes(DEJAVU).unwrap();
    let plain = f.shape("office", *b"latn", None, &[]);
    let ligated = f.shape("office", *b"latn", None, &[*b"liga"]);
    assert_eq!(plain.len(), 6, "unligated 'office' is six glyphs");
    assert!(
        ligated.len() < plain.len(),
        "liga must reduce glyph count: {} vs {}",
        ligated.len(),
        plain.len()
    );
    // Every output cluster must be a byte index into "office" (0..=5) and
    // the clusters must be non-decreasing.
    let mut prev = 0u32;
    for g in &ligated {
        assert!(g.cluster <= 5, "cluster {} out of range", g.cluster);
        assert!(g.cluster >= prev, "clusters must be monotonic");
        prev = g.cluster;
    }
}

/// Disabling the ligature feature leaves the run unligated even when the
/// font ships the lookup — only requested features fire.
#[test]
fn shape_respects_requested_feature_set() {
    let f = Font::from_bytes(DEJAVU).unwrap();
    let no_liga = f.shape("fi", *b"latn", None, &[]);
    let with_liga = f.shape("fi", *b"latn", None, &[*b"liga"]);
    assert_eq!(no_liga.len(), 2, "'fi' stays two glyphs without liga");
    // DejaVu ligates fi; with liga the count drops (or, for a font that
    // doesn't, stays the same — assert it never grows).
    assert!(with_liga.len() <= no_liga.len());
}

/// GSUB Arabic joining: shaping a four-letter Arabic word substitutes the
/// nominal isolated glyphs for their positional (initial/medial/final)
/// forms. The output gids must differ from the nominal cmap gids, and the
/// glyph count is preserved (joining is 1:1 single substitution).
#[test]
fn shape_applies_arabic_joining_forms() {
    let f = Font::from_bytes(ARABIC).unwrap();
    let word = "\u{0628}\u{0631}\u{0643}\u{0629}"; // بركة
    let nominal: Vec<u16> = word.chars().map(|c| f.glyph_index(c).unwrap()).collect();
    let sh = f.shape(
        word,
        *b"arab",
        None,
        &[*b"init", *b"medi", *b"fina", *b"isol"],
    );
    assert_eq!(sh.len(), 4, "joining is 1:1, four letters → four glyphs");
    let shaped: Vec<u16> = sh.iter().map(|g| g.glyph_id).collect();
    assert_ne!(
        shaped, nominal,
        "positional substitution must change at least one glyph"
    );
    // The leading letter takes an initial form, the trailing letter a
    // final form: at least two of four glyphs change.
    let changed = shaped.iter().zip(&nominal).filter(|(a, b)| a != b).count();
    assert!(
        changed >= 2,
        "expected >=2 positional substitutions, got {changed}"
    );
    // Clusters track UTF-8 byte offsets (Arabic letters are 2 bytes each).
    assert_eq!(
        sh.iter().map(|g| g.cluster).collect::<Vec<_>>(),
        vec![0, 2, 4, 6]
    );
}

/// GPOS mark-to-base: shaping an Arabic base letter followed by a fatha
/// (U+064E, a combining mark) with the `mark` feature must offset the
/// mark glyph so it attaches over the base — the mark gains a non-zero
/// placement offset while the base stays at the origin.
#[test]
fn shape_attaches_arabic_mark_to_base() {
    let f = Font::from_bytes(ARABIC).unwrap();
    // BEH + FATHA, isolated (no joining feature so the base keeps its
    // nominal isolated glyph the mark anchors are authored against).
    let word = "\u{0628}\u{064E}";
    let sh = f.shape(word, *b"arab", None, &[*b"mark"]);
    assert_eq!(sh.len(), 2);
    // First glyph is the base, unmoved.
    assert_eq!(sh[0].x_offset, 0);
    assert_eq!(sh[0].y_offset, 0);
    // The mark glyph must be a GDEF mark and must have been repositioned.
    assert!(
        f.is_mark_glyph(sh[1].glyph_id),
        "second glyph should be a combining mark"
    );
    assert!(
        sh[1].x_offset != 0 || sh[1].y_offset != 0,
        "mark must receive an attachment offset, got ({}, {})",
        sh[1].x_offset,
        sh[1].y_offset
    );
}

/// Without the `mark` feature, the same base+mark run is left unattached
/// (no placement offset) — mark positioning is opt-in like every other
/// feature.
#[test]
fn shape_leaves_mark_unattached_without_mark_feature() {
    let f = Font::from_bytes(ARABIC).unwrap();
    let word = "\u{0628}\u{064E}";
    let sh = f.shape(word, *b"arab", None, &[]);
    assert_eq!(sh.len(), 2);
    assert_eq!((sh[1].x_offset, sh[1].y_offset), (0, 0));
}

/// A font with no GSUB/GPOS coverage for the requested script still
/// shapes to nominal glyphs with hmtx advances — shaping never panics on
/// an unmatched script.
#[test]
fn shape_unknown_script_falls_back_to_nominal() {
    let f = Font::from_bytes(DEJAVU).unwrap();
    let sh = f.shape("test", *b"zzzz", None, &[*b"kern", *b"liga"]);
    assert_eq!(sh.len(), 4);
    for (i, ch) in "test".chars().enumerate() {
        let gid = f.glyph_index(ch).unwrap();
        assert_eq!(sh[i].glyph_id, gid);
        assert_eq!(sh[i].x_advance, f.glyph_advance(gid) as i32);
    }
}

/// Empty input shapes to an empty run.
#[test]
fn shape_empty_text() {
    let f = Font::from_bytes(DEJAVU).unwrap();
    assert!(f.shape("", *b"latn", None, &[*b"kern"]).is_empty());
}

/// Shaping must be panic-free across a stress mix of scripts, features,
/// and codepoints (including unmapped ones that resolve to .notdef).
#[test]
fn shape_is_panic_free_across_mixed_input() {
    let f = Font::from_bytes(ARABIC).unwrap();
    let inputs = [
        "\u{0628}\u{064E}\u{0631}\u{064F}\u{0643}\u{0650}\u{0629}",
        "\u{0627}\u{0644}\u{0644}\u{0647}", // ALLAH (ligature-prone)
        "abc\u{FFFD}\u{10000}",             // mixed unmapped + astral
    ];
    let feats: &[[u8; 4]] = &[
        *b"init", *b"medi", *b"fina", *b"isol", *b"liga", *b"rlig", *b"calt", *b"mark", *b"mkmk",
        *b"curs", *b"kern",
    ];
    for w in inputs {
        let sh = f.shape(w, *b"arab", None, feats);
        // No glyph's cluster may exceed the input byte length.
        for g in &sh {
            assert!((g.cluster as usize) <= w.len());
        }
    }
}

/// The GSUB/GPOS `lookupFlag` accessors the shaper relies on report the
/// on-disk flag bits. DejaVu Sans ships ligature lookups both with and
/// without IGNORE_MARKS, plus single-substitution lookups with the flag;
/// we assert at least one of each kind is visible and that the bit
/// decoding is consistent (an out-of-range index reports 0).
#[test]
fn lookup_flags_accessor_reports_ignore_marks_bit() {
    let f = Font::from_bytes(DEJAVU).unwrap();
    let mut any_ignore_marks = false;
    let mut any_plain = false;
    for (idx, kind, _) in f.gsub_lookup_list() {
        let fl = f.gsub_lookup_flags(idx);
        if kind == 4 {
            if (fl & 0x0008) != 0 {
                any_ignore_marks = true;
            } else {
                any_plain = true;
            }
        }
    }
    assert!(
        any_ignore_marks,
        "DejaVu must ship at least one IGNORE_MARKS ligature lookup"
    );
    assert!(
        any_plain,
        "DejaVu must ship at least one non-IGNORE_MARKS ligature lookup"
    );
    // Out-of-range / no-table indices report 0.
    assert_eq!(f.gsub_lookup_flags(u16::MAX), 0);
    assert_eq!(f.gpos_lookup_flags(u16::MAX), 0);
}

/// Spec-correct mark handling: the DejaVu `liga` "fi" ligature lookup
/// does NOT set IGNORE_MARKS, so a combining mark between `f` and `i`
/// legitimately blocks the ligature — the run stays three glyphs. (A
/// lookup that DID set IGNORE_MARKS would ligate across the mark; the
/// shaper honours whichever the lookup declares.)
#[test]
fn shape_mark_blocks_non_ignore_marks_ligature() {
    let f = Font::from_bytes(DEJAVU).unwrap();
    // Without a mark, fi ligates to a single glyph.
    let ligated = f.shape("fi", *b"latn", None, &[*b"liga"]);
    assert_eq!(ligated.len(), 1, "'fi' ligates to one glyph");
    // With a combining acute between f and i, the (non-ignore-marks)
    // liga lookup cannot match across the mark, so no ligature forms.
    let blocked = f.shape("f\u{0301}i", *b"latn", None, &[*b"liga"]);
    assert_eq!(
        blocked.len(),
        3,
        "a mark between the components blocks a non-IGNORE_MARKS ligature"
    );
}
