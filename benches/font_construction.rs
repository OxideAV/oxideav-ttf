//! Construction-cost benchmark for [`oxideav_ttf::Font`].
//!
//! Std-only timing harness (`harness = false`, no external bench
//! framework). Run with:
//!
//! ```text
//! cargo bench --bench font_construction
//! ```
//!
//! Each scenario builds a fresh `Font` per iteration, the way callers
//! that construct a font per shaping call / per outline fetch do, and
//! reports the median per-iteration time over several batches.

use std::hint::black_box;
use std::time::{Duration, Instant};

use oxideav_ttf::Font;

const INTER: &[u8] = include_bytes!("../tests/fixtures/InterVariable.ttf");
const DEJAVU: &[u8] = include_bytes!("../tests/fixtures/DejaVuSans.ttf");

const BATCHES: usize = 15;
const TARGET_BATCH: Duration = Duration::from_millis(40);

/// Median per-iteration time of `f`, in microseconds.
fn measure(mut f: impl FnMut()) -> f64 {
    // Calibrate the batch size so one batch takes ~TARGET_BATCH.
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
    samples[samples.len() / 2]
}

fn run(label: &str, bytes: &'static [u8]) {
    // 100 code points: printable ASCII (95) + 5 Latin-1 letters.
    let chars: Vec<char> = (0x20u32..0x7f)
        .chain([0xe0, 0xe9, 0xf1, 0xf6, 0xfc])
        .filter_map(char::from_u32)
        .collect();
    assert_eq!(chars.len(), 100);
    let probe = Font::from_bytes(bytes).expect("fixture parses");
    let gid_a = probe.glyph_index('a').expect("fixture maps 'a'");

    let construct = measure(|| {
        black_box(Font::from_bytes(black_box(bytes)).unwrap());
    });
    let cmap100 = measure(|| {
        let f = Font::from_bytes(black_box(bytes)).unwrap();
        for &c in &chars {
            black_box(f.glyph_index(c));
        }
    });
    let outline = measure(|| {
        let f = Font::from_bytes(black_box(bytes)).unwrap();
        black_box(f.glyph_outline(gid_a).unwrap());
    });
    let gsub = measure(|| {
        let f = Font::from_bytes(black_box(bytes)).unwrap();
        black_box(f.gsub_features_for_script(*b"latn", None));
    });
    let validate = measure(|| {
        let f = Font::from_bytes(black_box(bytes)).unwrap();
        f.validate().unwrap();
        black_box(&f);
    });
    println!("{label}:");
    println!("  from_bytes                      {construct:>9.2} us");
    println!("  from_bytes + 100x glyph_index   {cmap100:>9.2} us");
    println!("  from_bytes + 1x glyph_outline   {outline:>9.2} us");
    println!("  from_bytes + GSUB feature list  {gsub:>9.2} us");
    println!("  from_bytes + validate (all)     {validate:>9.2} us");
}

fn main() {
    // `cargo bench` passes `--bench`; any other invocation (notably
    // `cargo test --all-targets`, which runs this binary without it) gets
    // a quick smoke pass instead of the timed run.
    if !std::env::args().any(|a| a == "--bench") {
        for bytes in [INTER, DEJAVU] {
            let f = Font::from_bytes(bytes).unwrap();
            f.validate().unwrap();
        }
        return;
    }
    run("InterVariable.ttf", INTER);
    run("DejaVuSans.ttf", DEJAVU);
}
