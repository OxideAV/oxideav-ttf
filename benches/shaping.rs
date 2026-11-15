//! Shaping benchmark for [`oxideav_ttf::Font::shape`].
//!
//! Std-only timing harness (`harness = false`, no external bench
//! framework). Run with:
//!
//! ```text
//! cargo bench --bench shaping
//! ```
//!
//! Every scenario shapes on a `Font` that was parsed once and is kept
//! alive across calls (the way a text stack that caches its fonts uses
//! the crate), and reports the median per-call time over several
//! batches, with the fastest batch in parentheses (the figure least
//! disturbed by other load on the machine). The `*/words` scenarios shape the same text one
//! whitespace-separated word at a time — the short-run pattern of a
//! line breaker — instead of as one long run.

use std::hint::black_box;
use std::time::{Duration, Instant};

use oxideav_ttf::Font;

#[path = "../tests/common/mod.rs"]
mod common;
use common::*;

const BATCHES: usize = 15;
const TARGET_BATCH: Duration = Duration::from_millis(40);

/// Per-iteration time of `f` in microseconds: (median batch, fastest
/// batch).
fn measure(mut f: impl FnMut()) -> (f64, f64) {
    let mut iters = 1usize;
    loop {
        let t = Instant::now();
        for _ in 0..iters {
            f();
        }
        let el = t.elapsed();
        if el >= TARGET_BATCH / 4 || iters >= 1 << 24 {
            let per = el.as_secs_f64() / iters as f64;
            iters = ((TARGET_BATCH.as_secs_f64() / per.max(1e-9)) as usize).max(1);
            break;
        }
        iters *= 2;
    }
    let mut samples: Vec<f64> = (0..BATCHES)
        .map(|_| {
            let t = Instant::now();
            for _ in 0..iters {
                f();
            }
            t.elapsed().as_secs_f64() * 1e6 / iters as f64
        })
        .collect();
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (samples[samples.len() / 2], samples[0])
}

fn run(label: &str, font: &Font<'_>, text: &str, script: Tag, features: &[Tag]) {
    let glyphs = font.shape(text, script, None, features).len();
    let whole = measure(|| {
        black_box(font.shape(black_box(text), script, None, features));
    });
    let words: Vec<&str> = text.split_whitespace().collect();
    let by_word = measure(|| {
        for w in &words {
            black_box(font.shape(black_box(w), script, None, features));
        }
    });
    println!(
        "{label:<24} {:>4} chars {glyphs:>4} glyphs   run {:>9.2} us ({:>9.2})   words {:>9.2} us ({:>9.2})",
        text.chars().count(),
        whole.0,
        whole.1,
        by_word.0,
        by_word.1
    );
}

fn main() {
    let inter = Font::from_bytes(INTER).unwrap();
    let dejavu = Font::from_bytes(DEJAVU).unwrap();
    let arabic = Font::from_bytes(NOTO_ARABIC).unwrap();

    println!("shaping: median of {BATCHES} batches (fastest batch), font parsed once and reused");
    run(
        "latin_paragraph/inter",
        &inter,
        LATIN_PARAGRAPH,
        *b"latn",
        LATN_DEFAULT,
    );
    run(
        "latin_paragraph/dejavu",
        &dejavu,
        LATIN_PARAGRAPH,
        *b"latn",
        LATN_DEFAULT,
    );
    run(
        "latin_rich/inter",
        &inter,
        LATIN_PARAGRAPH,
        *b"latn",
        LATN_RICH,
    );
    run(
        "liga_kern_heavy/inter",
        &inter,
        LIGA_KERN_HEAVY,
        *b"latn",
        LATN_DEFAULT,
    );
    run(
        "liga_kern_heavy/dejavu",
        &dejavu,
        LIGA_KERN_HEAVY,
        *b"latn",
        LATN_DEFAULT,
    );
    run(
        "latin_combining/dejavu",
        &dejavu,
        LATIN_COMBINING,
        *b"latn",
        LATN_DEFAULT,
    );
    run(
        "arabic_paragraph/noto",
        &arabic,
        ARABIC_PARAGRAPH,
        *b"arab",
        ARAB_DEFAULT,
    );
    run(
        "arabic_paragraph/dejavu",
        &dejavu,
        ARABIC_PARAGRAPH,
        *b"arab",
        ARAB_DEFAULT,
    );

    // One-shot use: parse + shape a short string on a fresh `Font`.
    let cold = measure(|| {
        let f = Font::from_bytes(black_box(INTER)).unwrap();
        black_box(f.shape("Waffle office", *b"latn", None, LATN_DEFAULT));
    });
    println!(
        "{:<24} parse + shape 13 chars    {:>9.2} us ({:>9.2})",
        "cold/inter", cold.0, cold.1
    );
}
