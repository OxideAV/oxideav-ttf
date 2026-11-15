//! Shaping corpus pin: glyph ids, clusters and positions that
//! [`Font::shape`] produces for a fixed set of strings, fonts, scripts
//! and feature sets, compared against `fixtures/shape_corpus.golden`.
//!
//! The corpus has two parts:
//!
//! * **Curated runs** — the sample texts in `common/` (Latin prose,
//!   ligature / kerning-dense Latin, combining marks, Greek + Cyrillic,
//!   vocalised Arabic, pointed Hebrew) on every fixture font that covers
//!   them, whole and word by word. Each is pinned glyph by glyph.
//! * **Generated runs** — deterministic pseudo-random strings drawn from
//!   per-script alphabets (letters, digits, punctuation, combining
//!   marks) under pseudo-random feature subsets. Each is pinned by a
//!   64-bit FNV-1a digest of the full shaped output.
//!
//! The golden file is a behaviour snapshot, not a correctness oracle:
//! its job is to prove that performance work on the lookup machinery
//! leaves shaping output unchanged. To regenerate it after an
//! *intentional* shaping change run
//!
//! ```text
//! OXIDEAV_TTF_BLESS=1 cargo test --test shape_corpus
//! ```

use std::fmt::Write as _;

use oxideav_ttf::{Font, ShapedGlyph};

mod common;
use common::*;

const GOLDEN_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/shape_corpus.golden"
);

fn fnv1a(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn digest(glyphs: &[ShapedGlyph]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for g in glyphs {
        fnv1a(&mut h, &g.glyph_id.to_le_bytes());
        fnv1a(&mut h, &g.cluster.to_le_bytes());
        fnv1a(&mut h, &g.x_offset.to_le_bytes());
        fnv1a(&mut h, &g.y_offset.to_le_bytes());
        fnv1a(&mut h, &g.x_advance.to_le_bytes());
        fnv1a(&mut h, &g.y_advance.to_le_bytes());
    }
    h
}

fn dump(glyphs: &[ShapedGlyph]) -> String {
    let mut s = String::new();
    for g in glyphs {
        let _ = write!(
            s,
            " {}@{}:{},{},{},{}",
            g.glyph_id, g.cluster, g.x_offset, g.y_offset, g.x_advance, g.y_advance
        );
    }
    s
}

/// Deterministic 64-bit generator (xorshift*), so the generated corpus
/// is identical on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

struct Alphabet {
    script: Tag,
    /// Base characters (letters, digits, punctuation, space).
    bases: Vec<char>,
    /// Combining marks.
    marks: Vec<char>,
    /// Feature pool the pseudo-random subsets are drawn from.
    features: &'static [Tag],
}

fn chars(ranges: &[(u32, u32)]) -> Vec<char> {
    ranges
        .iter()
        .flat_map(|&(a, b)| a..=b)
        .filter_map(char::from_u32)
        .collect()
}

fn alphabets() -> Vec<Alphabet> {
    vec![
        Alphabet {
            script: *b"latn",
            bases: chars(&[
                (0x20, 0x7e),
                (0x20, 0x20),
                (0xc0, 0xff),
                (0x100, 0x17f),
                (0x2010, 0x2014),
            ]),
            marks: chars(&[(0x300, 0x30c), (0x323, 0x328)]),
            features: LATN_RICH,
        },
        Alphabet {
            script: *b"arab",
            bases: chars(&[
                (0x20, 0x20),
                (0x60c, 0x60c),
                (0x61b, 0x61b),
                (0x621, 0x64a),
                (0x660, 0x669),
                (0x671, 0x6d3),
                (0x30, 0x39),
            ]),
            marks: chars(&[(0x64b, 0x652), (0x670, 0x670)]),
            features: ARAB_DEFAULT,
        },
        Alphabet {
            script: *b"hebr",
            bases: chars(&[(0x20, 0x20), (0x5d0, 0x5ea), (0x5f0, 0x5f4)]),
            marks: chars(&[(0x5b0, 0x5bd), (0x5c1, 0x5c2)]),
            features: HEBR_DEFAULT,
        },
        Alphabet {
            script: *b"grek",
            bases: chars(&[(0x20, 0x20), (0x386, 0x3ce), (0x2c, 0x2e)]),
            marks: chars(&[(0x300, 0x304), (0x308, 0x308)]),
            features: LATN_RICH,
        },
        Alphabet {
            script: *b"cyrl",
            bases: chars(&[(0x20, 0x20), (0x400, 0x45f), (0x2c, 0x2e)]),
            marks: chars(&[(0x300, 0x308)]),
            features: LATN_RICH,
        },
        Alphabet {
            script: *b"DFLT",
            bases: chars(&[(0x20, 0x7e), (0x621, 0x64a), (0x391, 0x3c9)]),
            marks: chars(&[(0x300, 0x308), (0x64b, 0x652)]),
            features: LATN_RICH,
        },
    ]
}

/// Build every corpus line (without comparing).
fn build_corpus() -> Vec<String> {
    let fonts: [(&str, &[u8]); 4] = [
        ("inter", INTER),
        ("dejavu", DEJAVU),
        ("dejavu-mono", DEJAVU_MONO),
        ("noto-arabic", NOTO_ARABIC),
    ];
    let curated: [(&str, &str, Tag, &[Tag]); 13] = [
        ("latin-paragraph", LATIN_PARAGRAPH, *b"latn", LATN_DEFAULT),
        ("latin-paragraph-rich", LATIN_PARAGRAPH, *b"latn", LATN_RICH),
        ("latin-paragraph-kern", LATIN_PARAGRAPH, *b"latn", KERN_ONLY),
        ("liga-kern", LIGA_KERN_HEAVY, *b"latn", LATN_DEFAULT),
        ("liga-kern-rich", LIGA_KERN_HEAVY, *b"latn", LATN_RICH),
        ("liga-kern-dflt", LIGA_KERN_HEAVY, *b"DFLT", LATN_RICH),
        ("latin-combining", LATIN_COMBINING, *b"latn", LATN_DEFAULT),
        ("greek-cyrillic-grek", GREEK_CYRILLIC, *b"grek", LATN_RICH),
        (
            "greek-cyrillic-cyrl",
            GREEK_CYRILLIC,
            *b"cyrl",
            LATN_DEFAULT,
        ),
        ("arabic-paragraph", ARABIC_PARAGRAPH, *b"arab", ARAB_DEFAULT),
        (
            "arabic-paragraph-noforms",
            ARABIC_PARAGRAPH,
            *b"arab",
            ARAB_NO_FORMS,
        ),
        (
            "arabic-paragraph-dflt",
            ARABIC_PARAGRAPH,
            *b"DFLT",
            ARAB_DEFAULT,
        ),
        ("hebrew-pointed", HEBREW_POINTED, *b"hebr", HEBR_DEFAULT),
    ];

    let mut lines = Vec::new();
    for (font_name, bytes) in fonts {
        let font = Font::from_bytes(bytes).expect("fixture parses");

        // --- curated runs: whole text, then word by word -----------------
        for (name, text, script, features) in curated {
            let whole = font.shape(text, script, None, features);
            lines.push(format!("run {font_name} {name} :{}", dump(&whole)));
            let mut h = 0xcbf2_9ce4_8422_2325u64;
            let mut count = 0usize;
            for word in text.split_whitespace() {
                let sh = font.shape(word, script, None, features);
                count += sh.len();
                fnv1a(&mut h, &digest(&sh).to_le_bytes());
            }
            lines.push(format!("words {font_name} {name} : {count} {h:016x}"));
        }

        // --- generated runs ---------------------------------------------
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ font_name.len() as u64);
        for (ai, alpha) in alphabets().iter().enumerate() {
            for case in 0..40usize {
                let len = 1 + rng.below(if case % 8 == 7 { 160 } else { 40 });
                let mut text = String::new();
                for _ in 0..len {
                    match rng.below(10) {
                        // A run of one to three combining marks.
                        0 | 1 => {
                            for _ in 0..1 + rng.below(3) {
                                text.push(alpha.marks[rng.below(alpha.marks.len())]);
                            }
                        }
                        2 => text.push(' '),
                        _ => text.push(alpha.bases[rng.below(alpha.bases.len())]),
                    }
                }
                // Pseudo-random feature subset; every 5th case uses the
                // whole pool.
                let mask = if case % 5 == 0 {
                    u64::MAX
                } else {
                    rng.next() | rng.next()
                };
                let features: Vec<Tag> = alpha
                    .features
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| mask >> (i % 64) & 1 == 1)
                    .map(|(_, t)| *t)
                    .collect();
                let sh = font.shape(&text, alpha.script, None, &features);
                lines.push(format!(
                    "gen {font_name} {ai}/{case} : {} {:016x}",
                    sh.len(),
                    digest(&sh)
                ));
            }
        }
    }

    // --- variable-font instance: FeatureVariations-aware resolution ------
    let mut inter = Font::from_bytes(INTER).expect("fixture parses");
    for coords in [[100.0f32, 14.0], [900.0, 32.0], [650.0, 20.0]] {
        inter.set_variation_coords(&coords);
        let sh = inter.shape(LIGA_KERN_HEAVY, *b"latn", None, LATN_RICH);
        lines.push(format!(
            "var inter {:?} : {} {:016x}",
            coords,
            sh.len(),
            digest(&sh)
        ));
    }
    lines
}

#[test]
fn shaping_output_matches_golden_corpus() {
    let lines = build_corpus();
    let actual = lines.join("\n") + "\n";
    if std::env::var_os("OXIDEAV_TTF_BLESS").is_some() {
        std::fs::write(GOLDEN_PATH, &actual).expect("write golden");
        return;
    }
    let golden = std::fs::read_to_string(GOLDEN_PATH).expect("golden corpus present");
    let golden_lines: Vec<&str> = golden.lines().collect();
    assert_eq!(
        golden_lines.len(),
        lines.len(),
        "corpus line count changed (golden {} vs actual {})",
        golden_lines.len(),
        lines.len()
    );
    let mut mismatches = Vec::new();
    for (g, a) in golden_lines.iter().zip(&lines) {
        if g != a {
            let key = a.split(" :").next().unwrap_or("");
            mismatches.push(key.to_string());
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} corpus entries differ from the golden file; first: {:?}",
        mismatches.len(),
        lines.len(),
        &mismatches[..mismatches.len().min(12)]
    );
}

/// Shaping the same run twice on one `Font` (second call served from
/// whatever the first one cached) and on a freshly parsed `Font` gives
/// the same glyphs.
#[test]
fn shaping_is_repeatable_and_cache_independent() {
    for (bytes, text, script, features) in [
        (INTER, LIGA_KERN_HEAVY, *b"latn", LATN_RICH),
        (DEJAVU, LATIN_COMBINING, *b"latn", LATN_DEFAULT),
        (NOTO_ARABIC, ARABIC_PARAGRAPH, *b"arab", ARAB_DEFAULT),
    ] {
        let warm = Font::from_bytes(bytes).unwrap();
        let first = warm.shape(text, script, None, features);
        // Interleave other feature sets / scripts to churn any caches.
        let _ = warm.shape(text, *b"DFLT", None, KERN_ONLY);
        let _ = warm.shape("x", script, None, &[]);
        let second = warm.shape(text, script, None, features);
        let fresh = Font::from_bytes(bytes)
            .unwrap()
            .shape(text, script, None, features);
        assert_eq!(first, second);
        assert_eq!(first, fresh);
    }
}
