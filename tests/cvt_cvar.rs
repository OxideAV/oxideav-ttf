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

#[test]
fn dejavu_exposes_fpgm_and_prep_programs() {
    let font = Font::from_bytes(DEJAVU).expect("parse DejaVuSans");
    // DejaVu Sans is manually hinted: it ships both bytecode programs.
    let fpgm = font.fpgm_program().expect("DejaVu has fpgm");
    let prep = font.prep_program().expect("DejaVu has prep");
    assert!(!fpgm.is_empty(), "fpgm program should carry bytecode");
    assert!(!prep.is_empty(), "prep program should carry bytecode");
    assert!(font.has_hinting_program());
    // Cross-check the fpgm length against the on-wire table directory.
    let n = u16::from_be_bytes([DEJAVU[4], DEJAVU[5]]) as usize;
    let mut fpgm_len = None;
    let mut prep_len = None;
    for i in 0..n {
        let off = 12 + i * 16;
        let tag = &DEJAVU[off..off + 4];
        let len = u32::from_be_bytes([
            DEJAVU[off + 12],
            DEJAVU[off + 13],
            DEJAVU[off + 14],
            DEJAVU[off + 15],
        ]) as usize;
        if tag == b"fpgm" {
            fpgm_len = Some(len);
        } else if tag == b"prep" {
            prep_len = Some(len);
        }
    }
    assert_eq!(Some(fpgm.len()), fpgm_len);
    assert_eq!(Some(prep.len()), prep_len);
}

#[test]
fn auto_hinted_font_has_no_hinting_program() {
    // InterVariable is auto-hinted at render time: no fpgm / prep / cvt.
    const INTER: &[u8] = include_bytes!("fixtures/InterVariable.ttf");
    let font = Font::from_bytes(INTER).expect("parse InterVariable");
    assert!(font.fpgm_program().is_none());
    assert!(font.prep_program().is_none());
    assert!(!font.has_hinting_program());
}
