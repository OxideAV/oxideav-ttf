//! Shaping over synthetic GSUB / GPOS tables, for the lookup types and
//! table shapes the fixture fonts do not exercise: Multiple
//! substitution (GSUB 2), Reverse Chaining substitution (GSUB 8),
//! Cursive attachment (GPOS 3), Contextual positioning (GPOS 7),
//! multi-sub-table lookups, Coverage format 2, and every lookup wrapped
//! in Extension records.
//!
//! The DejaVu Sans fixture supplies the glyphs, `cmap`, `hmtx` and
//! `GDEF`; its `GSUB` and `GPOS` are swapped for tables built here byte
//! by byte, following the sub-table layouts documented alongside this
//! crate's own GSUB / GPOS parsers (`src/tables/gsub.rs`,
//! `src/tables/gpos.rs`). Each scenario checks (a) that the lookups fire with
//! the expected effect and (b) that shaping with the lookup
//! accelerators on gives exactly what the plain table walk gives, over
//! a few thousand generated strings.

use oxideav_ttf::{Font, ShapedGlyph};

const DEJAVU: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");
const TEST: [u8; 4] = *b"test";

/// Rebuild an sfnt with table `tag`'s body replaced: the new body is
/// appended at EOF and the directory record repointed.
fn replace_table(font: &[u8], tag: [u8; 4], body: &[u8]) -> Vec<u8> {
    let num_tables = u16::from_be_bytes([font[4], font[5]]) as usize;
    let mut out = font.to_vec();
    let rec = (0..num_tables)
        .map(|i| 12 + i * 16)
        .find(|&rec| font[rec..rec + 4] == tag)
        .expect("table present");
    // Keep the appended body 4-byte aligned.
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    let at = out.len() as u32;
    out[rec + 8..rec + 12].copy_from_slice(&at.to_be_bytes());
    out[rec + 12..rec + 16].copy_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
    out
}

fn words(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}

/// Coverage format 1 (glyph list; must be sorted).
fn coverage(glyphs: &[u16]) -> Vec<u8> {
    let mut sorted = glyphs.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut b = words(&[1, sorted.len() as u16]);
    b.extend(words(&sorted));
    b
}

/// Coverage format 2 (one range per run of consecutive glyph ids).
fn coverage_ranges(glyphs: &[u16]) -> Vec<u8> {
    let mut sorted = glyphs.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut ranges: Vec<(u16, u16, u16)> = Vec::new();
    for (index, &g) in sorted.iter().enumerate() {
        match ranges.last_mut() {
            Some(last) if last.1 + 1 == g => last.1 = g,
            _ => ranges.push((g, g, index as u16)),
        }
    }
    let mut b = words(&[2, ranges.len() as u16]);
    for (start, end, start_index) in ranges {
        b.extend(words(&[start, end, start_index]));
    }
    b
}

/// A sub-table made of a fixed header, then offset-addressed parts: the
/// header's `u16` at each `slots[i]` is patched with the offset of
/// `parts[i]`.
fn with_parts(mut header: Vec<u8>, slots: &[usize], parts: &[Vec<u8>]) -> Vec<u8> {
    for (slot, part) in slots.iter().zip(parts) {
        let at = header.len() as u16;
        header[*slot..*slot + 2].copy_from_slice(&at.to_be_bytes());
        header.extend_from_slice(part);
    }
    header
}

struct Lookup {
    kind: u16,
    flags: u16,
    subtables: Vec<Vec<u8>>,
}

/// A complete GSUB / GPOS table: one script (`DFLT`), one feature
/// (`test`) referencing every lookup. With `extension == Some(t)` every
/// lookup is stored as lookup type `t` whose sub-tables are Extension
/// records pointing at the real ones.
fn layout_table(lookups: &[Lookup], extension: Option<u16>) -> Vec<u8> {
    // ScriptList: one script record → Script → DefaultLangSys.
    let mut script_list = words(&[1]);
    script_list.extend(b"DFLT");
    script_list.extend(words(&[8])); // Script offset
    script_list.extend(words(&[4, 0])); // defaultLangSys offset, langSysCount
    script_list.extend(words(&[0, 0xFFFF, 1, 0])); // LangSys: 1 feature, index 0

    // FeatureList: one feature listing every lookup.
    let mut feature_list = words(&[1]);
    feature_list.extend(TEST);
    feature_list.extend(words(&[8])); // Feature offset
    feature_list.extend(words(&[0, lookups.len() as u16]));
    feature_list.extend(words(&(0..lookups.len() as u16).collect::<Vec<_>>()));

    // LookupList.
    let mut lookup_tables: Vec<Vec<u8>> = Vec::new();
    for lookup in lookups {
        let n = lookup.subtables.len();
        let stored_kind = extension.unwrap_or(lookup.kind);
        let mut table = words(&[stored_kind, lookup.flags, n as u16]);
        let header_len = 6 + n * 2;
        let mut bodies: Vec<u8> = Vec::new();
        let mut offsets: Vec<u16> = Vec::new();
        match extension {
            None => {
                for sub in &lookup.subtables {
                    offsets.push((header_len + bodies.len()) as u16);
                    bodies.extend_from_slice(sub);
                }
            }
            Some(_) => {
                // n Extension records (8 bytes each), then the targets.
                let mut targets: Vec<u8> = Vec::new();
                for (i, sub) in lookup.subtables.iter().enumerate() {
                    let record_at = header_len + i * 8;
                    offsets.push(record_at as u16);
                    let target_at = header_len + n * 8 + targets.len();
                    bodies.extend(words(&[1, lookup.kind]));
                    bodies.extend(((target_at - record_at) as u32).to_be_bytes());
                    targets.extend_from_slice(sub);
                }
                bodies.extend(targets);
            }
        }
        table.extend(words(&offsets));
        table.extend(bodies);
        lookup_tables.push(table);
    }
    let mut lookup_list = words(&[lookups.len() as u16]);
    let mut at = 2 + lookups.len() * 2;
    for table in &lookup_tables {
        lookup_list.extend(words(&[at as u16]));
        at += table.len();
    }
    for table in &lookup_tables {
        lookup_list.extend_from_slice(table);
    }

    let script_at = 10usize;
    let feature_at = script_at + script_list.len();
    let lookup_at = feature_at + feature_list.len();
    let mut table = words(&[1, 0, script_at as u16, feature_at as u16, lookup_at as u16]);
    table.extend(script_list);
    table.extend(feature_list);
    table.extend(lookup_list);
    table
}

/// Glyph ids of the test alphabet in DejaVu Sans.
struct Glyphs {
    a: u16,
    b: u16,
    c: u16,
    d: u16,
    e: u16,
    f: u16,
    x: u16,
    y: u16,
    z: u16,
}

fn glyphs() -> Glyphs {
    let font = Font::from_bytes(DEJAVU).unwrap();
    let g = |c: char| font.glyph_index(c).unwrap();
    Glyphs {
        a: g('a'),
        b: g('b'),
        c: g('c'),
        d: g('d'),
        e: g('e'),
        f: g('f'),
        x: g('x'),
        y: g('y'),
        z: g('z'),
    }
}

/// GSUB with, in LookupList order: a Contextual lookup (d a) nesting
/// the Multiple lookup (the run grows inside a contextual rewrite),
/// Multiple substitution in two sub-tables (a → x y z; b → nothing;
/// c → d), Reverse Chaining (e → f when preceded by d and followed by
/// x), Single substitution over a range Coverage (y ↔ z), a Contextual
/// lookup (f f) nesting the Ligature lookup (the run shrinks inside a
/// contextual rewrite), and the Ligature lookup itself (f f → e).
fn synthetic_gsub(g: &Glyphs, extension: bool) -> Vec<u8> {
    // Lookup 0 — SequenceContextFormat3 (d a): apply lookup 1 at index 1.
    let context_grow = with_parts(
        words(&[3, 2, 1, 0, 0, 1, 1]),
        &[6, 8],
        &[coverage(&[g.d]), coverage_ranges(&[g.a])],
    );

    // Lookup 1 — MultipleSubstFormat1, sub-table A: a → [x, y, z], b → [].
    let (first, second) = if g.a < g.b { (g.a, g.b) } else { (g.b, g.a) };
    let seq_a = words(&[3, g.x, g.y, g.z]);
    let seq_b = words(&[0]);
    let (seq_first, seq_second) = if g.a < g.b {
        (seq_a, seq_b)
    } else {
        (seq_b, seq_a)
    };
    let multiple_a = with_parts(
        words(&[1, 0, 2, 0, 0]),
        &[2, 6, 8],
        &[coverage(&[first, second]), seq_first, seq_second],
    );
    // Sub-table B: c → [d].
    let multiple_b = with_parts(
        words(&[1, 0, 1, 0]),
        &[2, 6],
        &[coverage_ranges(&[g.c]), words(&[1, g.d])],
    );

    // Lookup 2 — ReverseChainSingleSubstFormat1: e → f after d, before x.
    let reverse = with_parts(
        words(&[1, 0, 1, 0, 1, 0, 1, g.f]),
        &[2, 6, 10],
        &[coverage(&[g.e]), coverage(&[g.d]), coverage_ranges(&[g.x])],
    );

    // Lookup 3 — SingleSubstFormat2 over a range Coverage: y → z, z → y
    // (in Coverage order).
    let (lo, hi) = if g.y < g.z { (g.y, g.z) } else { (g.z, g.y) };
    let single = with_parts(
        words(&[2, 0, 2, hi, lo]),
        &[2],
        &[coverage_ranges(&[lo, hi])],
    );

    // Lookup 4 — SequenceContextFormat3 (f f): apply lookup 5 at index 0.
    let context_shrink = with_parts(
        words(&[3, 2, 1, 0, 0, 0, 5]),
        &[6, 8],
        &[coverage(&[g.f]), coverage(&[g.f])],
    );

    // Lookup 5 — LigatureSubstFormat1: f f → e.
    let ligature_set = with_parts(words(&[1, 0]), &[2], &[words(&[g.e, 2, g.f])]);
    let ligature = with_parts(
        words(&[1, 0, 1, 0]),
        &[2, 6],
        &[coverage(&[g.f]), ligature_set],
    );

    layout_table(
        &[
            Lookup {
                kind: 5,
                flags: 0,
                subtables: vec![context_grow],
            },
            Lookup {
                kind: 2,
                flags: 0,
                subtables: vec![multiple_a, multiple_b],
            },
            Lookup {
                kind: 8,
                flags: 0,
                subtables: vec![reverse],
            },
            Lookup {
                kind: 1,
                flags: 0,
                subtables: vec![single],
            },
            Lookup {
                kind: 5,
                flags: 0,
                subtables: vec![context_shrink],
            },
            Lookup {
                kind: 4,
                flags: 0,
                subtables: vec![ligature],
            },
        ],
        extension.then_some(7),
    )
}

/// GPOS with: Cursive attachment (a, b, c with entry / exit anchors),
/// Contextual positioning (x y → run Single adjustment on the y),
/// Single adjustment in both formats, and a glyph-pair kerning lookup
/// with two sub-tables.
fn synthetic_gpos(g: &Glyphs, extension: bool) -> Vec<u8> {
    let anchor = |x: i16, y: i16| words(&[1, x as u16, y as u16]);

    // Lookup 0 — CursivePosFormat1 over {a, b, c}; EntryExitRecords in
    // Coverage (glyph id) order. One record has no entry anchor, one no
    // exit anchor.
    let mut cursive_glyphs = [
        (g.a, Some((10, 20)), Some((500, 40))),
        (g.b, None, Some((480, -30))),
        (g.c, Some((-15, 5)), None),
    ];
    cursive_glyphs.sort_by_key(|entry| entry.0);
    let mut cursive = words(&[1, 0, 3, 0, 0, 0, 0, 0, 0]);
    let mut anchors: Vec<u8> = Vec::new();
    for (i, (_, entry, exit)) in cursive_glyphs.iter().enumerate() {
        for (j, point) in [entry, exit].into_iter().enumerate() {
            if let Some((x, y)) = point {
                let at = (18 + anchors.len()) as u16;
                let slot = 6 + i * 4 + j * 2;
                cursive[slot..slot + 2].copy_from_slice(&at.to_be_bytes());
                anchors.extend(anchor(*x, *y));
            }
        }
    }
    cursive.extend(anchors);
    let cursive_coverage_at = cursive.len() as u16;
    cursive[2..4].copy_from_slice(&cursive_coverage_at.to_be_bytes());
    cursive.extend(coverage(&[g.a, g.b, g.c]));

    // Lookup 1 — SequenceContextFormat3 (x y): apply lookup 2 at index 1.
    let context = with_parts(
        words(&[3, 2, 1, 0, 0, 1, 2]),
        &[6, 8],
        &[coverage(&[g.x]), coverage_ranges(&[g.y])],
    );

    // Lookup 2 — SinglePos. Sub-table A, format 1: y gets xPlacement +7,
    // xAdvance -11 (valueFormat 0x0005). Sub-table B, format 2: d and e
    // get per-glyph yPlacement / yAdvance (valueFormat 0x000A).
    let single_a = with_parts(
        words(&[1, 0, 0x0005, 7, (-11i16) as u16]),
        &[2],
        &[coverage(&[g.y])],
    );
    let (lo, hi) = if g.d < g.e { (g.d, g.e) } else { (g.e, g.d) };
    let single_b = with_parts(
        words(&[2, 0, 0x000A, 2, 3, (-4i16) as u16, 5, 6]),
        &[2],
        &[coverage_ranges(&[lo, hi])],
    );

    // Lookup 3 — PairPosFormat1 in two sub-tables: (d, e) → -50 on d;
    // (f, f) → +25 on f. valueFormat1 = xAdvance, valueFormat2 = none.
    let pair = |left: u16, right: u16, adjust: i16| {
        let pair_set = words(&[1, right, adjust as u16]);
        with_parts(
            words(&[1, 0, 0x0004, 0, 1, 0]),
            &[2, 10],
            &[coverage(&[left]), pair_set],
        )
    };

    layout_table(
        &[
            Lookup {
                kind: 3,
                flags: 0,
                subtables: vec![cursive],
            },
            Lookup {
                kind: 7,
                flags: 0,
                subtables: vec![context],
            },
            Lookup {
                kind: 1,
                flags: 0,
                subtables: vec![single_a, single_b],
            },
            Lookup {
                kind: 2,
                flags: 0,
                subtables: vec![pair(g.d, g.e, -50), pair(g.f, g.f, 25)],
            },
        ],
        extension.then_some(9),
    )
}

fn synthetic_font(extension: bool) -> Vec<u8> {
    let g = glyphs();
    let font = replace_table(DEJAVU, *b"GSUB", &synthetic_gsub(&g, extension));
    replace_table(&font, *b"GPOS", &synthetic_gpos(&g, extension))
}

fn shape(font: &Font<'_>, text: &str) -> Vec<ShapedGlyph> {
    font.shape(text, *b"DFLT", None, &[TEST])
}

fn gids(shaped: &[ShapedGlyph]) -> Vec<u16> {
    shaped.iter().map(|s| s.glyph_id).collect()
}

#[test]
fn synthetic_lookups_fire_as_built() {
    let g = glyphs();
    for extension in [false, true] {
        let bytes = synthetic_font(extension);
        let font = Font::from_bytes(&bytes).unwrap();
        let nominal = |gid: u16| font.glyph_advance(gid) as i32;

        // Multiple substitution: a → x y z (then Single swaps y / z),
        // b deleted, c → d; all outputs keep the source cluster.
        let shaped = shape(&font, "abc");
        assert_eq!(gids(&shaped), [g.x, g.z, g.y, g.d], "extension {extension}");
        assert_eq!(
            shaped.iter().map(|s| s.cluster).collect::<Vec<_>>(),
            [0, 0, 0, 2]
        );

        // Reverse chaining: e → f between d and x; not elsewhere.
        assert_eq!(gids(&shape(&font, "dex")), [g.d, g.f, g.x]);
        assert_eq!(gids(&shape(&font, "xed")), [g.x, g.e, g.d]);

        // Ligature (through the contextual rule): f f → e.
        assert_eq!(gids(&shape(&font, "ff")), [g.e]);
        // Contextual rule (d a) expands the a in place.
        assert_eq!(gids(&shape(&font, "da")), [g.d, g.x, g.z, g.y]);

        // Glyph-pair kerning, second sub-table and first: "d e" pair.
        let shaped = shape(&font, "de");
        assert_eq!(shaped[0].x_advance, nominal(g.d) - 50);
        // …and SinglePos format 2 on d and e: yPlacement in Coverage
        // order. Its yAdvance is "only used for vertical layout" and
        // leaves a horizontal run alone.
        let (d_placement, e_placement) = if g.d < g.e { (3, 5) } else { (5, 3) };
        assert_eq!((shaped[0].y_offset, shaped[0].y_advance), (d_placement, 0));
        assert_eq!((shaped[1].y_offset, shaped[1].y_advance), (e_placement, 0));
    }

    // Cursive + contextual positioning read on a GSUB-free variant, so
    // the substitutions above do not rewrite the test strings.
    for extension in [false, true] {
        let gpos_only = replace_table(DEJAVU, *b"GPOS", &synthetic_gpos(&g, extension));
        let gpos_only = replace_table(&gpos_only, *b"GSUB", &layout_table(&[], None));
        let font = Font::from_bytes(&gpos_only).unwrap();
        let nominal = |gid: u16| font.glyph_advance(gid) as i32;

        // Where glyph `i` of a shaped run is drawn: the pen (sum of the
        // advances before it) plus its offset.
        let origin = |shaped: &[ShapedGlyph], i: usize| {
            let pen: i32 = shaped[..i].iter().map(|s| s.x_advance).sum();
            (pen + shaped[i].x_offset, shaped[i].y_offset)
        };
        // Cursive attachment: b (exit 480,-30) then c (entry -15,5) —
        // the two anchors land on the same point.
        let shaped = shape(&font, "bc");
        let (b_at, c_at) = (origin(&shaped, 0), origin(&shaped, 1));
        assert_eq!(
            (b_at.0 + 480, b_at.1 - 30),
            (c_at.0 - 15, c_at.1 + 5),
            "extension {extension}"
        );
        // The first glyph stays where it was; the pen after the pair is
        // where it would have been without the connection, moved by the
        // distance the second glyph was pulled.
        assert_eq!(b_at, (0, 0));
        assert_eq!(shaped[0].x_advance, 480);
        assert_eq!(shaped[1].x_advance, nominal(g.c) + 15);
        // a → c: a's exit (500, 40) meets c's entry (-15, 5).
        let shaped = shape(&font, "ac");
        let (a_at, c_at) = (origin(&shaped, 0), origin(&shaped, 1));
        assert_eq!((a_at.0 + 500, a_at.1 + 40), (c_at.0 - 15, c_at.1 + 5));
        // c has no exit: the chain breaks, a is not moved.
        let shaped = shape(&font, "ca");
        assert_eq!((shaped[1].x_offset, shaped[1].y_offset), (0, 0));
        assert_eq!(shaped[0].x_advance, nominal(g.c));

        // Contextual positioning: in "xy" the y is adjusted twice — once
        // by the context rule's nested SinglePos, once by the SinglePos
        // lookup itself; a lone y only once.
        let shaped = shape(&font, "xy");
        assert_eq!(shaped[1].x_offset, 14);
        assert_eq!(shaped[1].x_advance, nominal(g.y) - 22);
        let shaped = shape(&font, "y");
        assert_eq!(shaped[0].x_offset, 7);
        assert_eq!(shaped[0].x_advance, nominal(g.y) - 11);
    }
}

#[test]
fn accelerated_shaping_matches_table_walk_on_synthetic_lookups() {
    for extension in [false, true] {
        let bytes = synthetic_font(extension);
        let fast = Font::from_bytes(&bytes).unwrap();
        let mut walk = Font::from_bytes(&bytes).unwrap();
        walk.set_shaping_accelerators(false);

        let alphabet: Vec<char> = "abcdefxyz q".chars().collect();
        let mut state = 0x9e37_79b9_7f4a_7c15u64 ^ extension as u64;
        let mut next = |n: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % n as u64) as usize
        };
        let mut fired = 0usize;
        for case in 0..3000 {
            let len = 1 + next(if case % 50 == 0 { 120 } else { 14 });
            let text: String = (0..len).map(|_| alphabet[next(alphabet.len())]).collect();
            let expected = shape(&walk, &text);
            assert_eq!(shape(&fast, &text), expected, "text {text:?}");
            if gids(&expected).len() != text.len() {
                fired += 1;
            }
        }
        assert!(
            fired > 1000,
            "length-changing lookups fired in {fired} cases"
        );
    }
}
