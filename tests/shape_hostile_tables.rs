//! Shaping over corrupted layout tables: the lookup accelerators must
//! agree with the plain table walk on *any* bytes, not just on
//! well-formed fonts.
//!
//! The accelerators are filters and flattened copies built from the
//! GSUB / GPOS / GDEF bytes; the table walk reads those bytes directly.
//! For each fixture font, this harness corrupts the three layout tables
//! with a fixed-seed generator — single big-endian `u16` fields
//! overwritten with small counts, offsets, glyph ids or noise, the kind
//! of damage that yields unsorted Coverage tables, overlapping class
//! ranges, dangling offsets and bogus lookup types — and shapes the same
//! strings on both paths. Any difference, or a panic on either path, is
//! a failure.

use oxideav_ttf::{Direction, Font, ShapeOptions, ShapedGlyph};

mod common;
use common::*;

/// Byte range of table `tag` in an sfnt file.
fn table_range(font: &[u8], tag: &[u8; 4]) -> Option<std::ops::Range<usize>> {
    let num_tables = u16::from_be_bytes([font[4], font[5]]) as usize;
    (0..num_tables).map(|i| 12 + i * 16).find_map(|rec| {
        if &font[rec..rec + 4] != tag {
            return None;
        }
        let word = |at: usize| u32::from_be_bytes(font[at..at + 4].try_into().unwrap()) as usize;
        let (offset, length) = (word(rec + 8), word(rec + 12));
        (offset + length <= font.len()).then_some(offset..offset + length)
    })
}

/// Deterministic generator (xorshift), identical on every platform.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// Overwrite `count` aligned `u16` fields of `table` (a sub-slice of the
/// font) with values likely to keep the table navigable but wrong.
fn corrupt(table: &mut [u8], rng: &mut Rng, count: usize, num_glyphs: u16) {
    let fields = table.len() / 2;
    if fields == 0 {
        return;
    }
    for _ in 0..count {
        // Bias towards the head of the table (headers, lists, lookup
        // records) where a change reaches the most data.
        let field = match rng.below(3) {
            0 => rng.below(fields.min(64)),
            1 => rng.below(fields.min(2048)),
            _ => rng.below(fields),
        };
        let at = field * 2;
        let old = u16::from_be_bytes([table[at], table[at + 1]]);
        let new = match rng.below(8) {
            0 => 0,
            1 => 0xFFFF,
            2 => old.wrapping_add(1),
            3 => old.wrapping_sub(1),
            4 => rng.below(12) as u16,
            5 => rng.below(num_glyphs.max(1) as usize) as u16,
            6 => old ^ (1 << rng.below(16)),
            _ => rng.below(table.len().min(0xFFFF)) as u16,
        };
        table[at..at + 2].copy_from_slice(&new.to_be_bytes());
    }
}

fn shape_both(
    bytes: &[u8],
    text: &str,
    script: Tag,
    features: &[Tag],
    with_queries: bool,
) -> Option<Vec<ShapedGlyph>> {
    // A corrupted layout table never fails construction (optional
    // tables are parsed lazily), but stay robust if it ever does.
    let fast = Font::from_bytes(bytes).ok()?;
    let mut walk = Font::from_bytes(bytes).ok()?;
    walk.set_shaping_accelerators(false);
    let expected = walk.shape(text, script, None, features);
    let actual = fast.shape(text, script, None, features);
    assert_eq!(actual, expected, "accelerated path diverged on {text:?}");
    // A second call is served from the plan cache, the memo tables and
    // the already-built accelerators.
    assert_eq!(fast.shape(text, script, None, features), expected);
    // The other directions go through the same lookups with different
    // attachment arithmetic.
    for direction in [Direction::RightToLeft, Direction::TopToBottom] {
        let mut options = ShapeOptions::new(script)
            .with_direction(direction)
            .with_zero_mark_advances(true);
        for &tag in features {
            options = options.with_feature(tag);
        }
        assert_eq!(
            fast.shape_text(text, &options),
            walk.shape_text(text, &options),
            "accelerated path diverged on {text:?} ({direction:?})"
        );
    }
    if !with_queries {
        return Some(expected);
    }
    // The per-lookup queries read the same corrupted tables.
    let glyphs: Vec<u16> = expected.iter().map(|g| g.glyph_id).take(3).collect();
    for lookup in 0..4u16 {
        assert_eq!(
            fast.gsub_lookup_coverage(lookup),
            walk.gsub_lookup_coverage(lookup)
        );
        assert_eq!(
            fast.gpos_lookup_coverage(lookup),
            walk.gpos_lookup_coverage(lookup)
        );
        for &a in &glyphs {
            assert_eq!(
                fast.gpos_cursive_anchors(lookup, a),
                walk.gpos_cursive_anchors(lookup, a)
            );
            assert_eq!(
                fast.gpos_ligature_component_count(lookup, a),
                walk.gpos_ligature_component_count(lookup, a)
            );
            for &b in &glyphs {
                assert_eq!(
                    fast.gpos_pair_adjustment(lookup, a, b),
                    walk.gpos_pair_adjustment(lookup, a, b)
                );
                assert_eq!(
                    fast.gpos_mark_base_anchors(lookup, a, b),
                    walk.gpos_mark_base_anchors(lookup, a, b)
                );
                assert_eq!(
                    fast.gpos_mark_mark_anchors(lookup, a, b),
                    walk.gpos_mark_mark_anchors(lookup, a, b)
                );
                assert_eq!(
                    fast.gpos_mark_ligature_anchors(lookup, a, 1, b),
                    walk.gpos_mark_ligature_anchors(lookup, a, 1, b)
                );
            }
        }
    }
    Some(expected)
}

/// A text to shape, with its script and feature set.
type Run = (&'static str, Tag, &'static [Tag]);

#[test]
fn accelerated_shaping_matches_table_walk_on_corrupted_layout_tables() {
    let scenarios: [(&[u8], &[Run]); 4] = [
        (
            INTER,
            &[
                ("Waffle office 1/2 -> AVATAR", *b"latn", LATN_RICH),
                ("fjord, Type To. 10:30 (A)", *b"latn", LATN_DEFAULT),
            ],
        ),
        (
            DEJAVU,
            &[
                ("office AVATAR e\u{0301}\u{0323} To", *b"latn", LATN_RICH),
                (
                    "\u{0644}\u{0627} \u{0628}\u{064e}\u{062d}\u{0652}\u{0631}",
                    *b"arab",
                    ARAB_DEFAULT,
                ),
                (
                    "\u{05e9}\u{05c1}\u{05b8}\u{05dc}\u{05d5}\u{05b9}",
                    *b"hebr",
                    HEBR_DEFAULT,
                ),
            ],
        ),
        (
            DEJAVU_MONO,
            &[(
                "fi a\u{0308} \u{0644}\u{0651}\u{064f}\u{063a}",
                *b"arab",
                ARAB_DEFAULT,
            )],
        ),
        (
            NOTO_ARABIC,
            &[
                (
                    "\u{0627}\u{0644}\u{0644}\u{0651}\u{064f}\u{063a}\u{064e}\u{0629}\u{064f} \u{0644}\u{0627}",
                    *b"arab",
                    ARAB_DEFAULT,
                ),
                (
                    "\u{0643}\u{062a}\u{0628}\u{060c} \u{0641}\u{064e}\u{0646}\u{0651}\u{064c}",
                    *b"DFLT",
                    ARAB_NO_FORMS,
                ),
            ],
        ),
    ];

    let mut total = 0usize;
    let mut changed = 0usize;
    for (font_index, (bytes, runs)) in scenarios.iter().enumerate() {
        let pristine = Font::from_bytes(bytes).unwrap();
        let num_glyphs = pristine.glyph_count();
        let baseline: Vec<Vec<ShapedGlyph>> = runs
            .iter()
            .map(|(text, script, features)| pristine.shape(text, *script, None, features))
            .collect();
        let tables: Vec<std::ops::Range<usize>> = [b"GSUB", b"GPOS", b"GDEF"]
            .iter()
            .filter_map(|tag| table_range(bytes, tag))
            .collect();
        assert!(!tables.is_empty());

        let mut rng = Rng(0x2545_f491_4f6c_dd1d ^ (font_index as u64 + 1) << 32);
        for round in 0..260usize {
            let mut mutated = bytes.to_vec();
            // One to three tables hit, 1 to 24 fields each.
            for _ in 0..1 + rng.below(3) {
                let range = tables[rng.below(tables.len())].clone();
                let count = 1 + rng.below(if round % 4 == 0 { 24 } else { 4 });
                corrupt(&mut mutated[range], &mut rng, count, num_glyphs);
            }
            for ((text, script, features), base) in runs.iter().zip(&baseline) {
                if let Some(shaped) = shape_both(&mutated, text, *script, features, round % 8 == 0)
                {
                    total += 1;
                    changed += (shaped != *base) as usize;
                }
            }
        }
    }
    // The corruption has to reach data shaping actually reads, or the
    // comparison proves nothing.
    assert!(
        changed * 10 > total,
        "only {changed} of {total} corrupted shapings differ from the pristine font"
    );
}
