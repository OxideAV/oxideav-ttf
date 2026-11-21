//! Shaping conformance: `Font::shape` / `Font::shape_glyphs` against
//! lookup data built table by table from the OpenType GSUB, GPOS, GDEF
//! and common-layout chapters (`common/tables.rs`).
//!
//! The DejaVu Sans fixture supplies glyphs, `cmap` and `hmtx`; its
//! GSUB, GPOS and GDEF are replaced by synthetic ones, so each test
//! names every anchor and value it checks. Letters stand in for the
//! roles a test needs — the synthetic GDEF decides which are marks or
//! ligatures. Positions are checked the way a renderer consumes them:
//! walk the pen over the advances, add each glyph's offset, and compare
//! where anchors land.
//!
//! Every scenario is shaped twice — lookup accelerators on and off —
//! and the two results must be identical.

use oxideav_ttf::{Direction, Font, RunGlyph, ShapeFeature, ShapeOptions, ShapedGlyph};

mod common;
use common::tables::*;
use common::{DEJAVU, INTER};

const TEST: [u8; 4] = *b"test";

/// Glyph id of `c` in the host font.
fn g(c: char) -> u16 {
    Font::from_bytes(DEJAVU).unwrap().glyph_index(c).unwrap()
}

/// GDEF classing each char of `bases` / `ligatures` / `marks`.
fn gdef_for(bases: &str, ligatures: &str, marks: &str) -> Vec<u8> {
    let classes: Vec<(u16, u16)> = bases
        .chars()
        .map(|c| (g(c), 1))
        .chain(ligatures.chars().map(|c| (g(c), 2)))
        .chain(marks.chars().map(|c| (g(c), 3)))
        .collect();
    gdef(&classes, &[], &[], None)
}

/// The host font with its three layout tables replaced.
fn font_with(gsub: &[u8], gpos: &[u8], gdef: &[u8]) -> Vec<u8> {
    let font = replace_table(DEJAVU, *b"GSUB", gsub);
    let font = replace_table(&font, *b"GPOS", gpos);
    replace_table(&font, *b"GDEF", gdef)
}

fn no_lookups() -> Vec<u8> {
    layout_table(&[], &[])
}

/// One feature, `test`, running every lookup.
fn all_lookups(lookups: &[Lookup]) -> Vec<u8> {
    let indices: Vec<u16> = (0..lookups.len() as u16).collect();
    layout_table(&[(TEST, &indices)], lookups)
}

/// Shape with accelerators on and off; the results must agree.
fn shape_both(bytes: &[u8], shape: impl Fn(&Font<'_>) -> Vec<ShapedGlyph>) -> Vec<ShapedGlyph> {
    let fast = Font::from_bytes(bytes).unwrap();
    let mut walk = Font::from_bytes(bytes).unwrap();
    walk.set_shaping_accelerators(false);
    let shaped = shape(&fast);
    assert_eq!(shaped, shape(&walk), "accelerated vs table walk");
    shaped
}

fn shape(bytes: &[u8], text: &str) -> Vec<ShapedGlyph> {
    shape_both(bytes, |font| font.shape(text, *b"DFLT", None, &[TEST]))
}

fn gids(shaped: &[ShapedGlyph]) -> Vec<u16> {
    shaped.iter().map(|s| s.glyph_id).collect()
}

fn chars(text: &str) -> Vec<u16> {
    text.chars().map(g).collect()
}

/// Where each glyph is drawn: the pen, moved by the advances of the
/// glyphs before it, plus the glyph's own offset.
fn origins(shaped: &[ShapedGlyph]) -> Vec<(i32, i32)> {
    let (mut pen_x, mut pen_y) = (0, 0);
    shaped
        .iter()
        .map(|s| {
            let at = (pen_x + s.x_offset, pen_y + s.y_offset);
            pen_x += s.x_advance;
            pen_y += s.y_advance;
            at
        })
        .collect()
}

fn add(a: (i32, i32), b: (i32, i32)) -> (i32, i32) {
    (a.0 + b.0, a.1 + b.1)
}

// ---- mark attachment --------------------------------------------------

/// Mark-to-base on `o` for the marks x / y / z, and mark-to-mark
/// stacking y on x and z on y.
fn stacking_gpos(extra: Vec<Lookup>) -> Vec<u8> {
    let mark_base = mark_base_pos(
        &[
            (g('x'), 0, at(10, 20)),
            (g('y'), 0, at(30, 40)),
            (g('z'), 0, at(5, 5)),
        ],
        &[(g('o'), &[Some(at(300, 900))])],
    );
    let mark_mark = mark_base_pos(
        &[(g('y'), 0, at(7, 11)), (g('z'), 0, at(3, 2))],
        &[
            (g('x'), &[Some(at(50, 400))]),
            (g('y'), &[Some(at(60, 350))]),
        ],
    );
    let mut lookups = vec![
        Lookup::new(4, vec![mark_base]),
        Lookup::new(6, vec![mark_mark]),
    ];
    lookups.extend(extra);
    all_lookups(&lookups)
}

/// GPOS LookupType 6: "the placement of the mark1 glyph is adjusted
/// such that the relevant attachment points coincide" — with the mark
/// below it *where that mark ended up*, so a stack of marks accumulates.
#[test]
fn mark_to_mark_resolves_against_the_final_position_of_the_mark_below() {
    let bytes = font_with(
        &no_lookups(),
        &stacking_gpos(vec![]),
        &gdef_for("o", "", "xyz"),
    );
    let shaped = shape(&bytes, "oxyz");
    let at = origins(&shaped);
    assert_eq!(at[0], (0, 0));
    // x on o: base anchor (300, 900) meets mark anchor (10, 20).
    assert_eq!(at[1], add(at[0], (300 - 10, 900 - 20)));
    // y on x: x's mark2 anchor (50, 400) meets y's anchor (7, 11).
    assert_eq!(at[2], add(at[1], (50 - 7, 400 - 11)));
    // z on y: y's mark2 anchor (60, 350) meets z's anchor (3, 2).
    assert_eq!(at[3], add(at[2], (60 - 3, 350 - 2)));
    // "Advance widths are not affected."
    let font = Font::from_bytes(&bytes).unwrap();
    for (s, c) in shaped.iter().zip("oxyz".chars()) {
        assert_eq!(s.x_advance, font.glyph_advance(g(c)) as i32);
    }

    // "The subtable applies precisely when that mark2 glyph is covered
    // by the mark2 Coverage": directly after the base, y keeps its
    // mark-to-base attachment.
    let shaped = shape(&bytes, "oy");
    assert_eq!(origins(&shaped)[1], (300 - 30, 900 - 40));
}

/// Lookups that run after an attachment and move the base (placement)
/// or lengthen its advance must not tear the marks off it.
#[test]
fn attached_marks_follow_later_adjustments_of_their_base() {
    let nudge = single_pos(
        &[g('o')],
        X_PLACEMENT | Y_PLACEMENT | X_ADVANCE,
        &[40, 7, 100],
    );
    let bytes = font_with(
        &no_lookups(),
        &stacking_gpos(vec![Lookup::new(1, vec![nudge])]),
        &gdef_for("o", "", "xyz"),
    );
    let shaped = shape(&bytes, "ooxyo");
    let at = origins(&shaped);
    let font = Font::from_bytes(&bytes).unwrap();
    let advance = font.glyph_advance(g('o')) as i32 + 100;
    assert_eq!(at[0], (40, 7));
    assert_eq!(at[1], (advance + 40, 7));
    assert_eq!(at[2], add(at[1], (300 - 10, 900 - 20)));
    assert_eq!(at[3], add(at[2], (50 - 7, 400 - 11)));
    // The glyph after the marks sits one base advance plus the marks'
    // own advances further on.
    let marks = font.glyph_advance(g('x')) as i32 + font.glyph_advance(g('y')) as i32;
    assert_eq!(at[4], (2 * advance + marks + 40, 7));
}

/// A mark-to-base lookup without IGNORE_MARKS still reaches the base
/// across other marks: the client "must look backward in the glyph
/// string from the mark to the preceding base glyph".
#[test]
fn mark_to_base_looks_back_past_marks_to_the_base() {
    let mark_base = mark_base_pos(
        &[(g('x'), 0, at(10, 20)), (g('y'), 1, at(30, 40))],
        &[(g('o'), &[Some(at(300, 900)), Some(at(250, -80))])],
    );
    let bytes = font_with(
        &no_lookups(),
        &all_lookups(&[Lookup::new(4, vec![mark_base])]),
        &gdef_for("o", "", "xy"),
    );
    let at = origins(&shape(&bytes, "oxy"));
    let x_advance = Font::from_bytes(&bytes).unwrap().glyph_advance(g('x')) as i32;
    assert_eq!(at[1], (300 - 10, 900 - 20));
    // y (class 1) attaches to o as well, not to the x before it. Its
    // offset undoes x's advance, so it lands on o's class-1 anchor.
    assert_eq!(at[2], (250 - 30, -80 - 40));
    let shaped = shape(&bytes, "oxy");
    let o_advance = Font::from_bytes(&bytes).unwrap().glyph_advance(g('o')) as i32;
    assert_eq!(shaped[2].x_offset, 250 - 30 - o_advance - x_advance);
}

/// GSUB with `f i → L` and `f f i → M`, ignoring marks, plus the
/// MarkLigPos data for both.
fn ligature_font() -> Vec<u8> {
    let ligatures = ligature_subst(&[
        (&[g('f'), g('f'), g('i')], g('M')),
        (&[g('f'), g('i')], g('L')),
    ]);
    let gsub = all_lookups(&[Lookup::new(4, vec![ligatures]).flags(IGNORE_MARKS)]);
    let mark_lig = mark_ligature_pos(
        &[(g('x'), 0, at(0, 0)), (g('y'), 1, at(0, 0))],
        &[
            (
                g('L'),
                &[
                    &[Some(at(100, 700)), Some(at(110, -50))],
                    &[Some(at(400, 650)), Some(at(410, -60))],
                ],
            ),
            (
                g('M'),
                &[
                    &[Some(at(100, 700)), Some(at(110, -50))],
                    &[Some(at(400, 650)), Some(at(410, -60))],
                    &[Some(at(800, 600)), None],
                ],
            ),
        ],
    );
    let gpos = all_lookups(&[Lookup::new(5, vec![mark_lig])]);
    font_with(&gsub, &gpos, &gdef_for("fi", "LM", "xy"))
}

/// GPOS LookupType 5: "the appropriate base attachment point is
/// determined by which ligature component the mark is associated
/// with", which the client tracks through glyph substitution.
#[test]
fn mark_to_ligature_attaches_to_the_component_the_mark_followed() {
    let bytes = ligature_font();
    // f x i y → L x y: x followed component 0, y component 1.
    let shaped = shape(&bytes, "fxiy");
    assert_eq!(gids(&shaped), chars("Lxy"));
    let at = origins(&shaped);
    assert_eq!(at[1], (100, 700));
    assert_eq!(at[2], (410, -60));
    // The ligature keeps the cluster of its first component; marks keep
    // their own.
    assert_eq!(
        shaped.iter().map(|s| s.cluster).collect::<Vec<_>>(),
        [0, 1, 3]
    );

    // Both marks after the last component.
    let at = origins(&shape(&bytes, "fixy"));
    assert_eq!((at[1], at[2]), ((400, 650), (410, -60)));
    // Both marks after the first component.
    let at = origins(&shape(&bytes, "fxyi"));
    assert_eq!((at[1], at[2]), ((100, 700), (110, -50)));

    // Three components, a mark after each; the last component has no
    // class-1 anchor, so a class-1 mark there stays unattached.
    let shaped = shape(&bytes, "fxfyixy");
    assert_eq!(gids(&shaped), chars("Mxyxy"));
    let at = origins(&shaped);
    assert_eq!((at[1], at[2], at[3]), ((100, 700), (410, -60), (800, 600)));
    assert_eq!((shaped[4].x_offset, shaped[4].y_offset), (0, 0));

    // A ligature glyph that did not come from a substitution has no
    // recorded components: a mark after it sits on the last one.
    let at = origins(&shape(&bytes, "Lx"));
    assert_eq!(at[1], (400, 650));
}

/// Marks sitting on different components of one ligature are not
/// stacked on one another by a mark-to-mark lookup; marks on the same
/// component are.
#[test]
fn mark_to_mark_stays_within_one_ligature_component() {
    let ligatures = ligature_subst(&[(&[g('f'), g('i')], g('L'))]);
    let gsub = all_lookups(&[Lookup::new(4, vec![ligatures]).flags(IGNORE_MARKS)]);
    let mark_lig = mark_ligature_pos(
        &[(g('x'), 0, at(0, 0))],
        &[(g('L'), &[&[Some(at(100, 700))], &[Some(at(400, 650))]])],
    );
    let mark_mark = mark_base_pos(&[(g('x'), 0, at(0, 0))], &[(g('x'), &[Some(at(0, 250))])]);
    let gpos = all_lookups(&[
        Lookup::new(5, vec![mark_lig]),
        Lookup::new(6, vec![mark_mark]),
    ]);
    let bytes = font_with(&gsub, &gpos, &gdef_for("fi", "L", "x"));
    // One mark per component: each on its own component.
    let at = origins(&shape(&bytes, "fxix"));
    assert_eq!((at[1], at[2]), ((100, 700), (400, 650)));
    // Two marks on the second component: the second stacks on the first.
    let at = origins(&shape(&bytes, "fixx"));
    assert_eq!((at[1], at[2]), ((400, 650), (400, 900)));
}

// ---- cursive attachment -----------------------------------------------

fn cursive_lookup(flags: u16) -> Lookup {
    Lookup::new(
        3,
        vec![cursive_pos(&[
            (g('a'), None, Some(at(500, 100))),
            (g('b'), Some(at(20, -30)), Some(at(450, 80))),
            (g('c'), Some(at(10, 10)), None),
        ])],
    )
    .flags(flags)
}

/// GPOS LookupType 3, left to right: each exit anchor coincides with
/// the entry anchor of the following glyph; across the line the second
/// glyph of each pair is the one moved, so offsets accumulate down the
/// chain.
#[test]
fn cursive_attachment_chains_left_to_right() {
    let bytes = font_with(
        &no_lookups(),
        &all_lookups(&[cursive_lookup(0)]),
        &gdef_for("abc", "", ""),
    );
    let shaped = shape(&bytes, "abc");
    let at = origins(&shaped);
    assert_eq!(at[0], (0, 0));
    assert_eq!(add(at[0], (500, 100)), add(at[1], (20, -30)));
    assert_eq!(add(at[1], (450, 80)), add(at[2], (10, 10)));
    assert_eq!((at[1].1, at[2].1), (130, 200));
    // "The layout engine adjusts the advance of the first glyph": the
    // pen leaves each connected glyph at its exit anchor.
    assert_eq!(shaped[0].x_advance, 500);
    assert_eq!(shaped[1].x_advance, 450 - 20);
    // The pen after the run is where the last glyph's advance puts it.
    let font = Font::from_bytes(&bytes).unwrap();
    let total: i32 = shaped.iter().map(|s| s.x_advance).sum();
    assert_eq!(total, at[2].0 + font.glyph_advance(g('c')) as i32);

    // No entry on a, no exit on c: "c a" does not connect.
    let shaped = shape(&bytes, "ca");
    assert_eq!(origins(&shaped)[1].1, 0);
    assert_eq!(shaped[0].x_advance, font.glyph_advance(g('c')) as i32);
}

/// "If the RIGHT_TO_LEFT lookup flag is set, then the last glyph in the
/// connected sequence keeps its initial position in the cross-stream
/// direction relative to the baseline, and the cross-stream positions
/// of the preceding, connected glyphs are adjusted."
#[test]
fn cursive_right_to_left_flag_keeps_the_last_glyph_on_the_baseline() {
    let bytes = font_with(
        &no_lookups(),
        &all_lookups(&[cursive_lookup(RIGHT_TO_LEFT)]),
        &gdef_for("abc", "", ""),
    );
    let at = origins(&shape(&bytes, "abc"));
    assert_eq!(add(at[0], (500, 100)), add(at[1], (20, -30)));
    assert_eq!(add(at[1], (450, 80)), add(at[2], (10, 10)));
    assert_eq!((at[0].1, at[1].1, at[2].1), (-200, -70, 0));
}

/// The same chain drawn right to left: the glyphs come back in drawing
/// order (c b a) and the anchors still coincide.
#[test]
fn cursive_attachment_in_a_right_to_left_run() {
    for (flags, baseline_glyph) in [(0, 2), (RIGHT_TO_LEFT, 0)] {
        let bytes = font_with(
            &no_lookups(),
            &all_lookups(&[cursive_lookup(flags)]),
            &gdef_for("abc", "", ""),
        );
        let options = ShapeOptions::new(*b"DFLT")
            .with_direction(Direction::RightToLeft)
            .with_feature(TEST);
        let shaped = shape_both(&bytes, |font| font.shape_text("abc", &options));
        assert_eq!(gids(&shaped), chars("cba"));
        assert_eq!(
            shaped.iter().map(|s| s.cluster).collect::<Vec<_>>(),
            [2, 1, 0]
        );
        let at = origins(&shaped);
        // Drawing order: c at [0], b at [1], a at [2].
        assert_eq!(add(at[2], (500, 100)), add(at[1], (20, -30)));
        assert_eq!(add(at[1], (450, 80)), add(at[0], (10, 10)));
        assert_eq!(at[baseline_glyph].1, 0);
    }
}

/// A mark attached to a glyph of a cursive chain rides the chain's
/// cross-stream shift, and a cursive lookup that ignores marks connects
/// across one.
#[test]
fn marks_ride_a_cursive_chain() {
    let mark_base = mark_base_pos(&[(g('x'), 0, at(0, 0))], &[(g('b'), &[Some(at(200, 800))])]);
    let bytes = font_with(
        &no_lookups(),
        &all_lookups(&[
            Lookup::new(4, vec![mark_base]),
            cursive_lookup(IGNORE_MARKS),
        ]),
        &gdef_for("abc", "", "x"),
    );
    let at = origins(&shape(&bytes, "abxc"));
    assert_eq!(add(at[0], (500, 100)), add(at[1], (20, -30)));
    // b → c across the mark.
    assert_eq!(add(at[1], (450, 80)), add(at[3], (10, 10)));
    // x sits on b's anchor, wherever the chain put b.
    assert_eq!(at[2], add(at[1], (200, 800)));
    assert_eq!(at[1].1, 130);
}

// ---- pair adjustment ----------------------------------------------------

/// PairPosFormat1: "a ValueRecord for each glyph" — every field of both
/// records is applied (the advance field of the line direction only).
#[test]
fn pair_adjustment_applies_both_value_records() {
    let first_format = X_PLACEMENT | Y_PLACEMENT | X_ADVANCE | Y_ADVANCE;
    let second_format = X_PLACEMENT | Y_PLACEMENT | X_ADVANCE;
    let pairs = pair_pos_glyphs(
        first_format,
        second_format,
        &[
            (g('A'), g('V'), &[5, 6, -70, 9], &[-20, 3, 15]),
            (g('V'), g('V'), &[0, 0, -33, 0], &[0, 0, 0]),
        ],
    );
    let bytes = font_with(
        &no_lookups(),
        &all_lookups(&[Lookup::new(2, vec![pairs])]),
        &gdef_for("AV", "", ""),
    );
    let font = Font::from_bytes(&bytes).unwrap();
    let nominal = |c: char| font.glyph_advance(g(c)) as i32;
    let shaped = shape(&bytes, "AV");
    assert_eq!(
        (shaped[0].x_offset, shaped[0].y_offset, shaped[0].x_advance),
        (5, 6, nominal('A') - 70)
    );
    assert_eq!(
        (shaped[1].x_offset, shaped[1].y_offset, shaped[1].x_advance),
        (-20, 3, nominal('V') + 15)
    );
    // yAdvance is "only used for vertical layout".
    assert_eq!((shaped[0].y_advance, shaped[1].y_advance), (0, 0));

    // valueFormat2 is not zero: the second glyph is consumed by the
    // pair, so the V V pair inside "AVV" is never looked up…
    let shaped = shape(&bytes, "AVV");
    assert_eq!(shaped[1].x_advance, nominal('V') + 15);
    // …but in "VVV" the first pair is (V, V), and the third V then
    // starts a pair of its own with nothing after it.
    let shaped = shape(&bytes, "VVV");
    assert_eq!(shaped[0].x_advance, nominal('V') - 33);
    assert_eq!(shaped[1].x_advance, nominal('V'));

    let query = font.gpos_pair_adjustment(0, g('A'), g('V')).unwrap();
    assert_eq!((query.first.x_placement, query.first.y_placement), (5, 6));
    assert_eq!((query.first.x_advance, query.first.y_advance), (-70, 9));
    assert_eq!(
        (query.second.x_placement, query.second.x_advance),
        (-20, 15)
    );
    assert!(query.second_has_record);
    assert_eq!(font.gpos_pair_adjustment(0, g('V'), g('A')), None);
}

/// "If valueFormat2 is set to 0, then the second glyph of the pair is
/// the 'next' glyph for which a lookup should be performed."
#[test]
fn pair_without_second_record_reuses_the_second_glyph() {
    let pairs = pair_pos_glyphs(
        X_PLACEMENT | X_ADVANCE,
        0,
        &[
            (g('A'), g('V'), &[4, -70], &[]),
            (g('V'), g('V'), &[-2, -33], &[]),
        ],
    );
    let bytes = font_with(
        &no_lookups(),
        &all_lookups(&[Lookup::new(2, vec![pairs])]),
        &gdef_for("AV", "", ""),
    );
    let font = Font::from_bytes(&bytes).unwrap();
    let nominal = |c: char| font.glyph_advance(g(c)) as i32;
    let shaped = shape(&bytes, "AVV");
    assert_eq!(
        (shaped[0].x_offset, shaped[0].x_advance),
        (4, nominal('A') - 70)
    );
    assert_eq!(
        (shaped[1].x_offset, shaped[1].x_advance),
        (-2, nominal('V') - 33)
    );
    assert_eq!((shaped[2].x_offset, shaped[2].x_advance), (0, nominal('V')));
}

/// PairPosFormat2 (class pairs), both ValueRecords, and a pair across a
/// mark the lookup ignores.
#[test]
fn class_pair_adjustment_with_both_records_and_ignored_marks() {
    let pairs = pair_pos_classes(
        &chars("AT"),
        X_PLACEMENT | X_ADVANCE,
        Y_PLACEMENT,
        &[(g('A'), 1), (g('T'), 2)],
        &[(g('V'), 1), (g('o'), 2)],
        &[((1, 1), &[3, -80], &[11]), ((2, 2), &[0, -120], &[-7])],
    );
    let bytes = font_with(
        &no_lookups(),
        &all_lookups(&[Lookup::new(2, vec![pairs]).flags(IGNORE_MARKS)]),
        &gdef_for("ATVo", "", "x"),
    );
    let font = Font::from_bytes(&bytes).unwrap();
    let nominal = |c: char| font.glyph_advance(g(c)) as i32;
    let shaped = shape(&bytes, "AVTxo");
    assert_eq!(
        (shaped[0].x_offset, shaped[0].x_advance),
        (3, nominal('A') - 80)
    );
    assert_eq!(shaped[1].y_offset, 11);
    assert_eq!(shaped[2].x_advance, nominal('T') - 120);
    assert_eq!((shaped[3].x_offset, shaped[3].y_offset), (0, 0));
    assert_eq!(shaped[4].y_offset, -7);
    // A class pair with no entry is a zero record, not a miss.
    let query = font.gpos_pair_adjustment(0, g('A'), g('o')).unwrap();
    assert_eq!((query.first.x_advance, query.second.y_placement), (0, 0));
}

// ---- variations ---------------------------------------------------------

/// The Inter variable fixture with synthetic GPOS + GDEF: every value
/// below has a VariationIndex table into the GDEF ItemVariationStore.
fn varied_font() -> (Vec<u8>, [u16; 4]) {
    let host = Font::from_bytes(INTER).unwrap();
    let gi = |c: char| host.glyph_index(c).unwrap();
    let (a, v, o, x) = (gi('A'), gi('V'), gi('o'), gi('x'));
    // One region: the first axis (wght) from its default to its
    // maximum; the second axis does not take part.
    let axis_count = host.variation_axes().len();
    let mut region = vec![(0.0, 1.0, 1.0)];
    region.resize(axis_count, (0.0, 0.0, 0.0));
    let store = item_variation_store(&[region], &[vec![40], vec![-30], vec![100], vec![-80]]);
    let mark_base = mark_base_pos(
        &[(x, 0, at(0, 0))],
        &[(
            o,
            &[Some(Anchor {
                x: 300,
                y: 900,
                variation: Some((2, 3)),
            })],
        )],
    );
    let gpos = all_lookups(&[
        Lookup::new(1, vec![single_pos_varied_advance(&[a], 10, 0)]),
        Lookup::new(2, vec![pair_pos_varied_advance(a, v, -100, 1)]),
        Lookup::new(4, vec![mark_base]),
    ]);
    let font = replace_table(INTER, *b"GPOS", &gpos);
    let font = replace_table(&font, *b"GSUB", &no_lookups());
    let gdef_table = gdef(&[(a, 1), (v, 1), (o, 1), (x, 3)], &[], &[], Some(&store));
    (replace_table(&font, *b"GDEF", &gdef_table), [a, v, o, x])
}

/// "In variable fonts, [a ValueRecord / AnchorFormat3] is required to
/// reference variation data to adjust [values] for different variation
/// instances": GPOS values and anchors follow the item variation store
/// at the font's current axis position, and nominal advances follow
/// `HVAR`.
#[test]
fn positioning_values_and_advances_follow_the_variation_instance() {
    let (bytes, [a, v, _, _]) = varied_font();
    for accelerated in [true, false] {
        let mut font = Font::from_bytes(&bytes).unwrap();
        font.set_shaping_accelerators(accelerated);
        let shape = |font: &Font<'_>, text: &str| font.shape(text, *b"DFLT", None, &[TEST]);

        // Default instance: the plain values.
        let default_advance = font.glyph_advance(a) as i32;
        let shaped = shape(&font, "AVox");
        assert_eq!(shaped[0].x_advance, default_advance + 10 - 100);
        assert_eq!(origins(&shaped)[3], add(origins(&shaped)[2], (300, 900)));

        for weight in [900.0, 650.0, 100.0] {
            assert!(font.set_axis_value(b"wght", weight));
            // The region's scalar: the normalised wght coordinate, or
            // nothing below the default.
            let scalar = font.normalised_coords()[0].max(0.0);
            let delta = |d: f32| (d * scalar).round() as i32;
            let shaped = shape(&font, "AVox");
            // hmtx + HVAR, then SinglePos (10 + 40·s), then the pair
            // (-100 - 30·s).
            assert_eq!(
                shaped[0].x_advance,
                font.glyph_advance_varied(a) as i32 + 10 + delta(40.0) - 100 + delta(-30.0),
                "wght {weight}"
            );
            assert_eq!(shaped[1].x_advance, font.glyph_advance_varied(v) as i32);
            let at = origins(&shaped);
            assert_eq!(
                at[3],
                add(at[2], (300 + delta(100.0), 900 + delta(-80.0))),
                "wght {weight}"
            );
            // The per-lookup queries read the same instance.
            let pair = font.gpos_pair_adjustment(1, a, v).unwrap();
            assert_eq!(pair.first.x_advance as i32, -100 + delta(-30.0));
        }
        // The heavy instance really is wider than the default.
        assert!(font.set_axis_value(b"wght", 900.0));
        assert!(font.glyph_advance_varied(a) as i32 > default_advance);
    }
}

// ---- lookup flags -------------------------------------------------------

/// GDEF with x, y, z as marks in mark attachment classes 1, 2, 2 and
/// one mark glyph set holding only y.
fn flag_gdef() -> Vec<u8> {
    gdef(
        &[
            (g('f'), 1),
            (g('i'), 1),
            (g('o'), 1),
            (g('L'), 2),
            (g('x'), 3),
            (g('y'), 3),
            (g('z'), 3),
        ],
        &[(g('x'), 1), (g('y'), 2), (g('z'), 2)],
        &[&[g('y')]],
        None,
    )
}

fn fi_ligature(lookup: impl Fn(Lookup) -> Lookup) -> Vec<u8> {
    let ligatures = ligature_subst(&[(&[g('f'), g('i')], g('L'))]);
    font_with(
        &all_lookups(&[lookup(Lookup::new(4, vec![ligatures]))]),
        &no_lookups(),
        &flag_gdef(),
    )
}

/// The lookup flag decides which glyphs "must be processed just as
/// though these glyphs were not present in the glyph sequence".
#[test]
fn ligature_matching_honours_every_mark_filter() {
    // No flag: a mark between the components blocks the ligature.
    let bytes = fi_ligature(|l| l);
    assert_eq!(gids(&shape(&bytes, "fi")), chars("L"));
    assert_eq!(gids(&shape(&bytes, "fxi")), chars("fxi"));

    // IGNORE_MARKS: every mark is stepped over and stays in place.
    let bytes = fi_ligature(|l| l.flags(IGNORE_MARKS));
    assert_eq!(gids(&shape(&bytes, "fxyi")), chars("Lxy"));

    // MARK_ATTACHMENT_CLASS_FILTER = 1: "skips over all marks not in
    // the specified mark attachment class" — y and z (class 2) are
    // skipped, x (class 1) is seen and blocks.
    let bytes = fi_ligature(|l| l.flags(mark_attachment_class(1)));
    assert_eq!(gids(&shape(&bytes, "fyzi")), chars("Lyz"));
    assert_eq!(gids(&shape(&bytes, "fxi")), chars("fxi"));

    // USE_MARK_FILTERING_SET {y}: marks outside the set are skipped.
    let bytes = fi_ligature(|l| l.mark_set(0));
    assert_eq!(gids(&shape(&bytes, "fxzi")), chars("Lxz"));
    assert_eq!(gids(&shape(&bytes, "fyi")), chars("fyi"));

    // "If the IGNORE_MARKS bit is set, this supersedes any mark
    // filtering set or mark attachment class indications."
    let bytes = fi_ligature(|l| l.mark_set(0).flags(0x0010 | IGNORE_MARKS));
    assert_eq!(gids(&shape(&bytes, "fyi")), chars("Ly"));
}

/// IGNORE_BASE_GLYPHS and IGNORE_LIGATURES remove those glyphs from
/// the sequence a lookup sees — and from the glyphs it acts on.
#[test]
fn base_and_ligature_glyphs_can_be_ignored() {
    // A ligature of two marks across a base glyph.
    let marks = ligature_subst(&[(&[g('x'), g('y')], g('z'))]);
    let bytes = font_with(
        &all_lookups(&[Lookup::new(4, vec![marks]).flags(IGNORE_BASE_GLYPHS)]),
        &no_lookups(),
        &flag_gdef(),
    );
    assert_eq!(gids(&shape(&bytes, "xoy")), chars("zo"));
    // A ligature glyph is not a base glyph: it is seen, and blocks.
    assert_eq!(gids(&shape(&bytes, "xLy")), chars("xLy"));

    // A single substitution that ignores ligatures leaves them alone.
    let single = single_subst(&[(g('L'), g('o')), (g('f'), g('i'))]);
    let bytes = font_with(
        &all_lookups(&[Lookup::new(1, vec![single]).flags(IGNORE_LIGATURES)]),
        &no_lookups(),
        &flag_gdef(),
    );
    assert_eq!(gids(&shape(&bytes, "fLf")), chars("iLi"));
}

/// Contextual lookups, all six sub-table formats: "Lookup flags affect
/// matching in backtrack and lookahead sequences as well as the input
/// sequence."
#[test]
fn context_rules_match_through_the_skip_filter() {
    let (f, i, o) = (g('f'), g('i'), g('o'));
    // Lookup 0 turns an i into L; the contextual lookup under test (1)
    // applies it to the second glyph of the input sequence "f i".
    let to_l = || Lookup::new(1, vec![single_subst(&[(i, g('L'))])]);
    let records: &[(u16, u16)] = &[(1, 0)];
    let classes = [(f, 1), (i, 2), (o, 3)];
    let plain_context: Vec<(u16, Vec<u8>)> = vec![
        (5, context_glyphs(&[(&[f, i], records)])),
        (5, context_classes(&[f], &classes, &[(&[1, 2], records)])),
        (5, context_coverage(&[&[f], &[i]], records)),
    ];
    for (kind, sub) in plain_context {
        for (flags, text, expected) in [
            (0, "fi", "fL"),
            (0, "fxi", "fxi"),
            (IGNORE_MARKS, "fxi", "fxL"),
            (IGNORE_MARKS, "fxxi", "fxxL"),
        ] {
            let gsub = layout_table(
                &[(TEST, &[1])],
                &[to_l(), Lookup::new(kind, vec![sub.clone()]).flags(flags)],
            );
            let bytes = font_with(&gsub, &no_lookups(), &flag_gdef());
            assert_eq!(gids(&shape(&bytes, text)), chars(expected), "{text}");
        }
    }
    // Chained: backtrack o, input f i, lookahead o.
    let chained: Vec<Vec<u8>> = vec![
        chain_glyphs(&[(&[o], &[f, i], &[o], records)]),
        chain_classes(&[f], &classes, &[(&[3], &[1, 2], &[3], records)]),
        chain_coverage(&[&[o]], &[&[f], &[i]], &[&[o]], records),
    ];
    for sub in chained {
        for (flags, text, expected) in [
            (0, "ofio", "ofLo"),
            (0, "fio", "fio"),
            (0, "ofi", "ofi"),
            (0, "oxfio", "oxfio"),
            (0, "ofixo", "ofixo"),
            (IGNORE_MARKS, "oxfxixo", "oxfxLxo"),
            (IGNORE_MARKS, "xfio", "xfio"),
        ] {
            let gsub = layout_table(
                &[(TEST, &[1])],
                &[to_l(), Lookup::new(6, vec![sub.clone()]).flags(flags)],
            );
            let bytes = font_with(&gsub, &no_lookups(), &flag_gdef());
            assert_eq!(gids(&shape(&bytes, text)), chars(expected), "{text}");
        }
    }
}

/// "Lookup flags in the main lookup table are not considered while the
/// nested lookups are being processed. Instead, lookup flags in the
/// nested lookups are considered."
#[test]
fn nested_lookups_use_their_own_flags() {
    let (f, i, x) = (g('f'), g('i'), g('x'));
    let ligature =
        |flags: u16| Lookup::new(4, vec![ligature_subst(&[(&[f, i], g('L'))])]).flags(flags);
    // Main lookup ignores marks and matches "f (x) i"; the nested
    // ligature lookup does not, so the x blocks it.
    let context = || Lookup::new(5, vec![context_glyphs(&[(&[f, i], &[(0, 0)])])]);
    let gsub = layout_table(
        &[(TEST, &[1])],
        &[ligature(0), context().flags(IGNORE_MARKS)],
    );
    let bytes = font_with(&gsub, &no_lookups(), &flag_gdef());
    assert_eq!(gids(&shape(&bytes, "fxi")), chars("fxi"));
    assert_eq!(gids(&shape(&bytes, "fi")), chars("L"));
    // The other way round: the main lookup sees the mark, so "f x i" is
    // not its input sequence at all; "f x" + nested ligature ignoring
    // marks reaches the i beyond the matched sequence.
    let context = Lookup::new(5, vec![context_glyphs(&[(&[f, x], &[(0, 0)])])]);
    let gsub = layout_table(&[(TEST, &[1])], &[ligature(IGNORE_MARKS), context]);
    let bytes = font_with(&gsub, &no_lookups(), &flag_gdef());
    assert_eq!(gids(&shape(&bytes, "fxi")), chars("Lx"));
}

/// "All of the sequence lookup records are processed in order, and
/// each applies to the results of the actions indicated by the
/// preceding record": after a record changes the length of the
/// sequence, `sequenceIndex` counts glyphs of the sequence as it now
/// stands.
#[test]
fn sequence_index_follows_length_changes() {
    let [a, b, c, d, p, q, r, z] = ['a', 'b', 'c', 'd', 'p', 'q', 'r', 'z'].map(g);
    let lookups = |context: Vec<u8>| {
        layout_table(
            &[(TEST, &[4])],
            &[
                // 0: a b → p (ligature)
                Lookup::new(4, vec![ligature_subst(&[(&[a, b], p)])]),
                // 1: c → z, q → r
                Lookup::new(1, vec![single_subst(&[(c, z), (q, r)])]),
                // 2: a → p q (multiple)
                Lookup::new(2, vec![multiple_subst(&[(a, &[p, q])])]),
                // 3: d → nothing-like marker: d → z
                Lookup::new(1, vec![single_subst(&[(d, z)])]),
                Lookup::new(5, vec![context]),
            ],
        )
    };
    // Ligature first: the sequence a b c d becomes p c d, so index 1 is
    // the c and index 2 the d.
    let gsub = lookups(context_glyphs(&[(
        &[a, b, c, d],
        &[(0, 0), (1, 1), (2, 3)],
    )]));
    let bytes = font_with(&gsub, &no_lookups(), &gdef_for("abcd", "", ""));
    assert_eq!(gids(&shape(&bytes, "abcd")), vec![p, z, z]);
    // Multiple substitution first: a b c becomes p q b c, so index 1 is
    // the q (→ r) and index 3 the c (→ z).
    let gsub = lookups(context_glyphs(&[(&[a, b, c], &[(0, 2), (1, 1), (3, 1)])]));
    let bytes = font_with(&gsub, &no_lookups(), &gdef_for("abcd", "", ""));
    assert_eq!(gids(&shape(&bytes, "abc")), vec![p, r, b, z]);
    // Records are applied in the order listed, not in sequence order.
    let gsub = lookups(context_glyphs(&[(&[a, b, c], &[(2, 1), (0, 0)])]));
    let bytes = font_with(&gsub, &no_lookups(), &gdef_for("abcd", "", ""));
    assert_eq!(gids(&shape(&bytes, "abc")), vec![p, z]);
}

/// After a contextual match "the client advances to the glyph position
/// immediately following the matched input sequence … in particular,
/// the client does not advance past the matched lookahead sequence".
#[test]
fn contextual_lookups_resume_after_the_input_sequence() {
    let (a, b) = (g('a'), g('b'));
    let to_b = || Lookup::new(1, vec![single_subst(&[(a, b)])]);
    // Input (a a), substitute the first: matches at 0 and 2, not at 1.
    let gsub = layout_table(
        &[(TEST, &[1])],
        &[
            to_b(),
            Lookup::new(5, vec![context_glyphs(&[(&[a, a], &[(0, 0)])])]),
        ],
    );
    let bytes = font_with(&gsub, &no_lookups(), &gdef_for("ab", "", ""));
    assert_eq!(gids(&shape(&bytes, "aaaa")), chars("baba"));
    assert_eq!(gids(&shape(&bytes, "aaa")), chars("baa"));
    // Input (a), lookahead (a): the lookahead glyph is not consumed.
    let gsub = layout_table(
        &[(TEST, &[1])],
        &[
            to_b(),
            Lookup::new(6, vec![chain_coverage(&[], &[&[a]], &[&[a]], &[(0, 0)])]),
        ],
    );
    let bytes = font_with(&gsub, &no_lookups(), &gdef_for("ab", "", ""));
    assert_eq!(gids(&shape(&bytes, "aaaa")), chars("bbba"));
}

/// Reverse chaining substitution: "processing of input glyph sequence
/// goes from end to start", so each glyph sees the substituted glyphs
/// after it; its context is matched through the skip filter too.
#[test]
fn reverse_chaining_runs_end_to_start_and_skips_ignored_glyphs() {
    let (a, b) = (g('a'), g('b'));
    // a → b when followed by b.
    let sub = reverse_chain(&[(a, b)], &[], &[&[b]]);
    let gsub = all_lookups(&[Lookup::new(8, vec![sub.clone()])]);
    let bytes = font_with(&gsub, &no_lookups(), &gdef_for("ab", "", "x"));
    // End to start: the last a turns first, then each one before it.
    assert_eq!(gids(&shape(&bytes, "aaab")), chars("bbbb"));
    assert_eq!(gids(&shape(&bytes, "aaa")), chars("aaa"));
    assert_eq!(gids(&shape(&bytes, "axb")), chars("axb"));
    let gsub = all_lookups(&[Lookup::new(8, vec![sub]).flags(IGNORE_MARKS)]);
    let bytes = font_with(&gsub, &no_lookups(), &gdef_for("ab", "", "x"));
    assert_eq!(gids(&shape(&bytes, "axaxb")), chars("bxbxb"));
    // Backtrack: a → b when preceded by b (nearest glyph first).
    let sub = reverse_chain(&[(a, b)], &[&[b]], &[]);
    let gsub = all_lookups(&[Lookup::new(8, vec![sub])]);
    let bytes = font_with(&gsub, &no_lookups(), &gdef_for("ab", "", "x"));
    // Going backwards, the a next to the b turns; the one before it
    // still has an (unsubstituted) a in front of it at that moment.
    assert_eq!(gids(&shape(&bytes, "baa")), chars("bba"));
}

/// Contextual positioning (GPOS LookupTypes 7 and 8) with nested
/// single, pair and mark-attachment lookups, through the skip filter.
#[test]
fn contextual_positioning_nests_every_positioning_type() {
    let (a, v, o, x) = (g('A'), g('V'), g('o'), g('x'));
    let lookups = |context: Lookup| {
        layout_table(
            &[(TEST, &[3])],
            &[
                Lookup::new(1, vec![single_pos(&[v], X_ADVANCE, &[50])]),
                Lookup::new(
                    2,
                    vec![pair_pos_glyphs(X_ADVANCE, 0, &[(a, v, &[-90], &[])])],
                )
                .flags(IGNORE_MARKS),
                Lookup::new(
                    4,
                    vec![mark_base_pos(
                        &[(x, 0, at(0, 0))],
                        &[(a, &[Some(at(350, 1500))])],
                    )],
                ),
                context,
            ],
        )
    };
    let gdef_table = gdef_for("AVo", "", "x");
    let host = Font::from_bytes(DEJAVU).unwrap();
    let nominal = |gid: u16| host.glyph_advance(gid) as i32;
    // Context (A V) after o, ignoring marks: pair on the A, single on
    // the V; a third record attaches nothing (index 0 is not a mark).
    let context = Lookup::new(
        8,
        vec![chain_coverage(
            &[&[o]],
            &[&[a], &[v]],
            &[],
            &[(0, 1), (1, 0), (0, 2)],
        )],
    )
    .flags(IGNORE_MARKS);
    let bytes = font_with(&no_lookups(), &lookups(context), &gdef_table);
    let shaped = shape(&bytes, "oAxV");
    assert_eq!(shaped[1].x_advance, nominal(a) - 90);
    assert_eq!(shaped[3].x_advance, nominal(v) + 50);
    // Without the o in front, nothing applies.
    let shaped = shape(&bytes, "AxV");
    assert_eq!(shaped[0].x_advance, nominal(a));
    assert_eq!(shaped[2].x_advance, nominal(v));
    // Context (A x): attach the mark at sequence index 1.
    let context = Lookup::new(7, vec![context_glyphs(&[(&[a, x], &[(1, 2)])])]);
    let bytes = font_with(&no_lookups(), &lookups(context), &gdef_table);
    let shaped = shape(&bytes, "Ax");
    assert_eq!(origins(&shaped)[1], (350, 1500));
}

/// Extension lookups (GSUB 7 / GPOS 9) wrap any other type.
#[test]
fn extension_lookups_are_transparent() {
    let ligatures = ligature_subst(&[(&[g('f'), g('i')], g('L'))]);
    let gsub = layout_table_with(
        &[(TEST, &[0])],
        None,
        &[Lookup::new(4, vec![ligatures]).flags(IGNORE_MARKS)],
        Some(7),
    );
    let mark_lig = mark_ligature_pos(
        &[(g('x'), 0, at(0, 0))],
        &[(g('L'), &[&[Some(at(100, 700))], &[Some(at(400, 650))]])],
    );
    let gpos = layout_table_with(
        &[(TEST, &[0])],
        None,
        &[Lookup::new(5, vec![mark_lig])],
        Some(9),
    );
    let bytes = font_with(&gsub, &gpos, &gdef_for("fi", "L", "x"));
    let shaped = shape(&bytes, "fxix");
    assert_eq!(gids(&shaped), chars("Lxx"));
    let at = origins(&shaped);
    assert_eq!(at[1], (100, 700));
    let font = Font::from_bytes(&bytes).unwrap();
    assert_eq!(font.gpos_ligature_component_count(0, g('L')), Some(2));
}

// ---- shaping API ----------------------------------------------------------

/// Per-glyph feature masks: a feature requested with a mask acts where
/// every glyph of the lookup's input sequence shares a bit with it.
#[test]
fn glyph_run_shaping_with_feature_masks() {
    let (a, b, f, i) = (g('a'), g('b'), g('f'), g('i'));
    let gsub = layout_table(
        &[(*b"aaaa", &[0]), (*b"bbbb", &[1]), (*b"cccc", &[0])],
        &[
            Lookup::new(1, vec![single_subst(&[(a, b)])]),
            Lookup::new(4, vec![ligature_subst(&[(&[f, i], g('L'))])]),
        ],
    );
    let bytes = font_with(&gsub, &no_lookups(), &gdef_for("abfi", "L", ""));
    let run = |glyphs: &[(u16, u32)], options: &ShapeOptions| {
        let glyphs: Vec<RunGlyph> = glyphs
            .iter()
            .enumerate()
            .map(|(k, &(gid, mask))| RunGlyph::new(gid, 100 + k as u32).with_mask(mask))
            .collect();
        shape_both(&bytes, |font| font.shape_glyphs(&glyphs, options))
    };
    let base = ShapeOptions::new(*b"DFLT");

    // Masked single substitution: only where the masks meet.
    let options = base.clone().with_masked_feature(*b"aaaa", 0b01);
    let shaped = run(&[(a, 0b01), (a, 0b10), (a, 0b11), (a, 0)], &options);
    assert_eq!(gids(&shaped), vec![b, a, b, a]);
    // Caller clusters come back untouched.
    assert_eq!(
        shaped.iter().map(|s| s.cluster).collect::<Vec<_>>(),
        [100, 101, 102, 103]
    );
    // An unmasked feature acts on every glyph, whatever its mask.
    let options = base.clone().with_feature(*b"aaaa");
    assert_eq!(gids(&run(&[(a, 0), (a, 5)], &options)), vec![b, b]);
    // A lookup reached from two features acts where either does.
    let options = base
        .clone()
        .with_masked_feature(*b"aaaa", 0b01)
        .with_masked_feature(*b"cccc", 0b10);
    let shaped = run(&[(a, 0b01), (a, 0b10), (a, 0b100)], &options);
    assert_eq!(gids(&shaped), vec![b, b, a]);
    // A ligature needs the mask on every component.
    let options = base.clone().with_masked_feature(*b"bbbb", 1);
    assert_eq!(gids(&run(&[(f, 1), (i, 1)], &options)), vec![g('L')]);
    assert_eq!(gids(&run(&[(f, 1), (i, 0)], &options)), vec![f, i]);
    assert_eq!(gids(&run(&[(f, 0), (i, 1)], &options)), vec![f, i]);
    // A feature the request does not name does nothing.
    assert_eq!(gids(&run(&[(a, 1), (f, 1), (i, 1)], &base)), vec![a, f, i]);
}

/// The LangSys required feature applies whether or not it is asked
/// for; alternates are picked by index.
#[test]
fn required_feature_and_alternate_selection() {
    let (a, b, c, d) = (g('a'), g('b'), g('c'), g('d'));
    let gsub = layout_table_with(
        &[(*b"rqrd", &[0]), (*b"salt", &[1])],
        Some(0),
        &[
            Lookup::new(1, vec![single_subst(&[(a, b)])]),
            Lookup::new(3, vec![alternate_subst(&[(b, &[c, d])])]),
        ],
        None,
    );
    let bytes = font_with(&gsub, &no_lookups(), &gdef_for("abcd", "", ""));
    let shaped = shape_both(&bytes, |font| font.shape("a", *b"DFLT", None, &[]));
    assert_eq!(gids(&shaped), vec![b]);
    let shaped = shape_both(&bytes, |font| font.shape("a", *b"DFLT", None, &[*b"salt"]));
    assert_eq!(gids(&shaped), vec![c]);
    let options = ShapeOptions::new(*b"DFLT")
        .with_shape_feature(ShapeFeature::new(*b"salt").with_alternate(1));
    let shaped = shape_both(&bytes, |font| font.shape_text("a", &options));
    assert_eq!(gids(&shaped), vec![d]);
    // Past the end of the AlternateSet: no substitution.
    let options = ShapeOptions::new(*b"DFLT")
        .with_shape_feature(ShapeFeature::new(*b"salt").with_alternate(2));
    let shaped = shape_both(&bytes, |font| font.shape_text("a", &options));
    assert_eq!(gids(&shaped), vec![b]);
}

/// A right-to-left run comes back in drawing order with marks still on
/// their anchors; `zero_mark_advances` keeps marks from moving the pen.
#[test]
fn right_to_left_runs_and_zeroed_mark_advances() {
    let bytes = font_with(
        &no_lookups(),
        &stacking_gpos(vec![]),
        &gdef_for("o", "", "xyz"),
    );
    let font = Font::from_bytes(&bytes).unwrap();
    let advance = |c: char| font.glyph_advance(g(c)) as i32;
    let rtl = ShapeOptions::new(*b"DFLT")
        .with_direction(Direction::RightToLeft)
        .with_feature(TEST);
    let shaped = shape_both(&bytes, |font| font.shape_text("ooxy", &rtl));
    assert_eq!(gids(&shaped), chars("yxoo"));
    let at = origins(&shaped);
    // Drawing order: y [0], x [1], then the o they belong to [2].
    assert_eq!(at[1], add(at[2], (300 - 10, 900 - 20)));
    assert_eq!(at[0], add(at[1], (50 - 7, 400 - 11)));
    assert_eq!(at[2], (advance('x') + advance('y'), 0));

    let zeroed = rtl.clone().with_zero_mark_advances(true);
    let shaped = shape_both(&bytes, |font| font.shape_text("ooxy", &zeroed));
    assert_eq!((shaped[0].x_advance, shaped[1].x_advance), (0, 0));
    let at = origins(&shaped);
    assert_eq!(at[2], (0, 0));
    assert_eq!(at[1], (300 - 10, 900 - 20));
    assert_eq!(at[3], (advance('o'), 0));

    // Top to bottom: the pen moves down; without `vmtx` by one em.
    let vertical = ShapeOptions::new(*b"DFLT").with_direction(Direction::TopToBottom);
    let shaped = shape_both(&bytes, |font| font.shape_text("oo", &vertical));
    let height = font
        .glyph_advance_height(g('o'))
        .map_or(font.units_per_em() as i32, i32::from);
    assert_eq!((shaped[0].x_advance, shaped[0].y_advance), (0, -height));
    assert_eq!(origins(&shaped)[1], (0, -height));
}

/// The per-lookup queries a layout engine drives lookups with.
#[test]
fn lookup_queries() {
    let bytes = ligature_font();
    let font = Font::from_bytes(&bytes).unwrap();
    // GSUB lookup 0: ligatures starting with f.
    assert_eq!(font.gsub_lookup_coverage(0), vec![g('f')]);
    assert!(font.gsub_lookup_covers(0, g('f')));
    assert!(!font.gsub_lookup_covers(0, g('i')));
    assert!(font.gsub_lookup_coverage(9).is_empty());
    // GPOS lookup 0: the attaching marks.
    let mut marks = vec![g('x'), g('y')];
    marks.sort_unstable();
    assert_eq!(font.gpos_lookup_coverage(0), marks);
    assert_eq!(font.gpos_ligature_component_count(0, g('M')), Some(3));
    let anchors = font
        .gpos_mark_ligature_anchors(0, g('M'), 1, g('y'))
        .unwrap();
    assert_eq!(anchors.mark_class, 1);
    assert_eq!(anchors.attach_anchor, (410, -60));
    assert_eq!(anchors.offset(), (410, -60));
    // No class-1 anchor on the third component; no fourth component.
    assert_eq!(font.gpos_mark_ligature_anchors(0, g('M'), 2, g('y')), None);
    assert_eq!(font.gpos_mark_ligature_anchors(0, g('M'), 3, g('x')), None);
    // Wrong lookup type.
    assert_eq!(font.gpos_mark_base_anchors(0, g('M'), g('x')), None);

    let bytes = font_with(
        &no_lookups(),
        &stacking_gpos(vec![cursive_lookup(0)]),
        &gdef_for("o", "", "xyz"),
    );
    let font = Font::from_bytes(&bytes).unwrap();
    let anchors = font.gpos_mark_base_anchors(0, g('o'), g('y')).unwrap();
    assert_eq!(
        (anchors.mark_anchor, anchors.attach_anchor),
        ((30, 40), (300, 900))
    );
    assert_eq!(anchors.offset(), (270, 860));
    let anchors = font.gpos_mark_mark_anchors(1, g('x'), g('z')).unwrap();
    assert_eq!(anchors.offset(), (50 - 3, 400 - 2));
    assert_eq!(font.gpos_mark_mark_anchors(1, g('z'), g('y')), None);
    let cursive = font.gpos_cursive_anchors(2, g('b')).unwrap();
    assert_eq!(
        (cursive.entry, cursive.exit),
        (Some((20, -30)), Some((450, 80)))
    );
    assert_eq!(font.gpos_cursive_anchors(2, g('a')).unwrap().entry, None);
    assert_eq!(font.gpos_cursive_anchors(2, g('o')), None);
}

/// The user-reported case: an `abvs` lookup of a Kannada font — a
/// class-based contextual substitution whose rule has two input glyphs
/// — joins a consonant and a vowel sign into one glyph. Runs when the
/// font is installed (path override: `OXIDEAV_TTF_KANNADA_FONT`).
#[test]
fn kannada_above_base_substitution_applies() {
    let path = std::env::var("OXIDEAV_TTF_KANNADA_FONT")
        .unwrap_or_else(|_| "/System/Library/Fonts/NotoSansKannada.ttc".to_string());
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("skipped: {path} not present");
        return;
    };
    let font = if oxideav_ttf::is_collection(&bytes) {
        Font::from_collection_bytes(&bytes, 0)
    } else {
        Font::from_bytes(&bytes)
    };
    let Ok(font) = font else {
        eprintln!("skipped: {path} does not parse");
        return;
    };
    // KA + VOWEL SIGN I.
    let text = "\u{0c95}\u{0cbf}";
    let script = [*b"knd2", *b"knda"]
        .into_iter()
        .find(|&s| !font.gsub_features_for_script(s, None).is_empty());
    let Some(script) = script else {
        eprintln!("skipped: {path} has no Kannada GSUB script");
        return;
    };
    if !font
        .gsub_features_for_script(script, None)
        .iter()
        .any(|f| f.tag == *b"abvs")
    {
        eprintln!("skipped: {path} has no abvs feature");
        return;
    }
    let nominal = font.shape(text, script, None, &[]);
    let shaped = font.shape(text, script, None, &[*b"abvs"]);
    assert_eq!(nominal.len(), 2);
    assert_eq!(shaped.len(), 1, "abvs joins consonant + vowel sign");
    assert_ne!(shaped[0].glyph_id, nominal[0].glyph_id);
    assert_eq!(shaped[0].cluster, 0);
}
