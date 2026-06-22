//! Integration coverage for the `cvt ` Control Value Table accessors
//! and the `cvar` CVT-variations API, exercised against the bundled
//! DejaVu Sans fixture (a static, manually-hinted font that ships a
//! `cvt ` + `fpgm` + `prep` triple but no `cvar`).

use oxideav_ttf::Font;

const DEJAVU: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");

#[test]
fn dejavu_exposes_cvt_table() {
    let font = Font::from_bytes(DEJAVU).expect("parse DejaVuSans");
    // DejaVu Sans ships a 510-byte cvt = 255 int16 entries.
    assert_eq!(font.cvt_count(), 255);
    // First two authored CVT values (verified against the raw table).
    assert_eq!(font.cvt_value(0), Some(309));
    assert_eq!(font.cvt_value(1), Some(184));
    // Out-of-range index returns None.
    assert_eq!(font.cvt_value(255), None);
    assert_eq!(font.cvt_value(60000), None);
}

#[test]
fn dejavu_has_no_cvar() {
    let font = Font::from_bytes(DEJAVU).expect("parse DejaVuSans");
    assert!(!font.has_cvar());
    assert!(font.cvar_table().is_none());
    // A static font with cvt but no cvar yields all-zero deltas and the
    // varied value equals the authored value.
    let deltas = font.cvt_deltas();
    assert_eq!(deltas.len(), 255);
    assert!(deltas.iter().all(|&d| d == 0));
    assert_eq!(font.cvt_value_varied(0), font.cvt_value(0));
    assert_eq!(font.cvt_value_varied(1), font.cvt_value(1));
}

#[test]
fn cvt_accessors_absent_when_no_cvt() {
    // InterVariable is auto-hinted: no cvt / cvar / fpgm / prep tables.
    const INTER: &[u8] = include_bytes!("fixtures/InterVariable.ttf");
    let font = Font::from_bytes(INTER).expect("parse InterVariable");
    assert_eq!(font.cvt_count(), 0);
    assert_eq!(font.cvt_value(0), None);
    assert!(!font.has_cvar());
    assert!(font.cvt_deltas().is_empty());
    assert_eq!(font.cvt_value_varied(0), None);
}
