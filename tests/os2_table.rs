//! Integration coverage for the expanded `OS/2` table decode (ISO/IEC
//! 14496-22:2019 §5.2.3), exercised against the bundled DejaVu fixtures.

use oxideav_ttf::Font;

const DEJAVU_MONO: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");
const DEJAVU_SANS: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");

#[test]
fn dejavu_mono_os2_fields_decode() {
    let font = Font::from_bytes(DEJAVU_MONO).expect("parse DejaVuSansMono");
    let os2 = font.os2_table().expect("DejaVu ships OS/2");

    // DejaVu is a regular-weight, normal-width face.
    assert_eq!(os2.us_weight_class, 400);
    assert_eq!(os2.us_width_class, 5);
    assert_eq!(font.weight_class(), 400);
    assert_eq!(font.width_class(), 5);

    // Vendor id is a 4-char ASCII tag; trimmed and printable.
    let vid = os2.vendor_id().expect("ascii vendor id");
    assert!(!vid.is_empty());
    assert!(vid.chars().all(|c| c.is_ascii_graphic() || c == ' '));

    // PANOSE classification is present (10 bytes; first byte is the family
    // kind — DejaVu Mono is a monospaced Latin text face, non-zero).
    assert_eq!(os2.panose.len(), 10);

    // Typo + Windows vertical metrics decode.
    assert!(os2.s_typo_ascender.is_some());
    assert!(os2.us_win_ascent.is_some());
    assert!(os2.us_win_descent.is_some());

    // The Latin-1 Unicode coverage bit (range1 bit 0) is set for a Latin
    // text font.
    assert_ne!(os2.ul_unicode_range1 & 0x1, 0);

    // fsType embedding state is queryable.
    let _ = font.embedding_installable().expect("OS/2 present");
    let _ = os2.embedding_restricted();
}

#[test]
fn dejavu_sans_style_bits() {
    let font = Font::from_bytes(DEJAVU_SANS).expect("parse DejaVuSans");
    let os2 = font.os2_table().expect("OS/2");
    // DejaVu Sans Book is the regular upright face: not bold, not italic.
    assert!(!os2.is_bold());
    assert!(!os2.is_italic());
    // first/last char index bracket the covered codepoints.
    assert!(os2.us_first_char_index <= os2.us_last_char_index);
}
