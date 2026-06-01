//! `VVAR` integration sanity test against InterVariable.ttf.
//!
//! Inter is a horizontal-only variable font: it does not ship `vmtx`
//! and consequently does not ship `VVAR` (the table is unconditionally
//! optional in TrueType variable fonts per ISO/IEC 14496-22:2019
//! §7.3.8.1). This test only verifies that the absent-table path is
//! wired through cleanly: every `*_variation_delta` query through the
//! `Font` accessor returns `None`, and `vvar_table()` itself yields
//! `None`. Bit-exact VVAR delta arithmetic is covered by the unit
//! tests on `tables::vvar` against the synthetic fixture there.

use oxideav_ttf::Font;

const FONT: &[u8] = include_bytes!("fixtures/InterVariable.ttf");

#[test]
fn inter_ships_no_vvar() {
    let font = Font::from_bytes(FONT).expect("parse");
    // Inter is horizontal-only — no `vmtx`, therefore no `VVAR`.
    assert!(font.vvar_table().is_none());
}

#[test]
fn vvar_accessors_return_none_without_table() {
    let font = Font::from_bytes(FONT).expect("parse");
    // Every VVAR query should fall through to `None` when the table is
    // absent — including for in-range glyph ids that DO have HVAR
    // entries.
    let gid = font.glyph_index('A').expect("Inter has 'A'");
    assert!(font.advance_height_variation_delta(gid).is_none());
    assert!(font.tsb_variation_delta(gid).is_none());
    assert!(font.bsb_variation_delta(gid).is_none());
    assert!(font.vorg_variation_delta(gid).is_none());
    // Edge: a max-range glyph id must not panic, just yields None.
    assert!(font.advance_height_variation_delta(u16::MAX).is_none());
}
