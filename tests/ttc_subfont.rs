//! Integration test for `Font::from_collection_bytes` against a real
//! `.ttc` (NotoSansCJK-Medium) fixture.
//!
//! TTCs are too large to vendor in-tree (~19 MB), so this test reads
//! from a previously-cached copy at
//! `<workspace>/target/test-fixtures/fonts/NotoSansCJK-Medium.ttc` if
//! present and exits silently otherwise. The cache is populated by the
//! consumer crate (`oxideav-scribe`)'s round-5 fixture helper when
//! `OXIDEAV_NETWORK_TESTS=1` is set; we deliberately don't pull the
//! 19 MB blob into oxideav-ttf's own dev-dependencies.
//!
//! The interesting bit this exercises is that a TTC subfont's table
//! directory holds FILE-relative offsets (per the OpenType §"Font
//! Collections" spec), so a naive `bytes.get(offset..)` slice followed
//! by `from_bytes(sub)` underflows the offsets and produces
//! `Error::BadOffset`. The fix in `from_collection_bytes` keeps the
//! original buffer base and only shifts the *header* lookup.

use std::path::PathBuf;

use oxideav_ttf::{is_collection, CollectionHeader, Font};

fn cached_ttc() -> Option<Vec<u8>> {
    // Walk up from CARGO_MANIFEST_DIR to find the workspace's
    // `target/test-fixtures/fonts/NotoSansCJK-Medium.ttc`. Honour
    // `CARGO_TARGET_DIR` first if it's set (matches how the per-crate
    // isolation pattern wires up a /tmp target dir).
    let candidates: Vec<PathBuf> = {
        let mut v = Vec::new();
        if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
            let mut p = PathBuf::from(dir);
            p.push("test-fixtures/fonts/NotoSansCJK-Medium.ttc");
            v.push(p);
        }
        let manifest = std::env::var("CARGO_MANIFEST_DIR").ok();
        if let Some(m) = manifest {
            let base = PathBuf::from(m);
            // crates/oxideav-ttf -> ../../target/test-fixtures/fonts/...
            v.push(
                base.join("../..")
                    .join("target/test-fixtures/fonts/NotoSansCJK-Medium.ttc"),
            );
        }
        v
    };
    for path in candidates {
        if let Ok(bytes) = std::fs::read(&path) {
            eprintln!("[ttc_subfont] using cached {}", path.display());
            return Some(bytes);
        }
    }
    eprintln!(
        "[ttc_subfont] no cached NotoSansCJK-Medium.ttc under \
         target/test-fixtures/fonts/ — skipping (run scribe round5 \
         with OXIDEAV_NETWORK_TESTS=1 to populate)"
    );
    None
}

#[test]
fn parses_real_ttc_subfont_zero() {
    let bytes = match cached_ttc() {
        Some(b) => b,
        None => return,
    };

    assert!(
        is_collection(&bytes),
        "expected NotoSansCJK-Medium.ttc to start with 'ttcf'"
    );
    let hdr = CollectionHeader::parse(&bytes).expect("ttc header parse");
    assert!(
        hdr.num_fonts() >= 5,
        "Noto Sans CJK Medium should ship at least 5 subfonts (J/K/SC/TC/HK), got {}",
        hdr.num_fonts()
    );

    // Subfont 0 is the Japanese cut. The bug this regression-tests was a
    // BadOffset out of `from_collection_bytes` because the subfont's
    // table directory carries file-relative offsets (per the OpenType
    // §"Font Collections" spec) and the old code passed a sub-sliced
    // buffer to `from_bytes`, underflowing every offset by ~32 bytes.
    let font = Font::from_collection_bytes(&bytes, 0)
        .expect("subfont 0 must parse (TTC file-relative offset regression)");
    assert!(font.glyph_count() > 0);
    let family = font.family_name().unwrap_or("(unknown)");
    eprintln!(
        "[ttc_subfont] subfont 0: family={family:?}, glyphs={}",
        font.glyph_count()
    );

    // Every subfont in the collection should parse cleanly with the
    // file-relative-offset fix in place.
    for i in 0..hdr.num_fonts() {
        Font::from_collection_bytes(&bytes, i)
            .unwrap_or_else(|e| panic!("subfont {i} failed: {e:?}"));
    }
}

#[test]
fn collection_subfont_out_of_range() {
    let bytes = match cached_ttc() {
        Some(b) => b,
        None => return,
    };
    let hdr = CollectionHeader::parse(&bytes).expect("ttc header parse");
    let oob = hdr.num_fonts();
    let err = Font::from_collection_bytes(&bytes, oob).unwrap_err();
    match err {
        oxideav_ttf::Error::SubfontOutOfRange(i) if i == oob => {}
        other => panic!("expected SubfontOutOfRange({oob}), got {other:?}"),
    }
}
