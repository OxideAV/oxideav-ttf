//! [`SharedFont`]: the owned, clonable, thread-safe font handle gives
//! the same answers as a borrowed [`Font`] over the same bytes, from any
//! clone, on any thread, for as long as any clone is alive.

use std::sync::Arc;

use oxideav_ttf::{Error, Font, SharedFont};

mod common;
use common::*;

#[test]
fn shared_font_matches_borrowed_font() {
    for (bytes, text, script, features) in [
        (INTER, LIGA_KERN_HEAVY, *b"latn", LATN_RICH),
        (DEJAVU, LATIN_COMBINING, *b"latn", LATN_DEFAULT),
        (NOTO_ARABIC, ARABIC_PARAGRAPH, *b"arab", ARAB_DEFAULT),
    ] {
        let borrowed = Font::from_bytes(bytes).unwrap();
        let shared = SharedFont::from_bytes(bytes).unwrap();
        assert_eq!(shared.data(), bytes);
        assert_eq!(shared.collection_index(), None);
        assert_eq!(shared.font().glyph_count(), borrowed.glyph_count());
        assert_eq!(shared.font().family_name(), borrowed.family_name());
        assert_eq!(
            shared.shape(text, script, None, features),
            borrowed.shape(text, script, None, features)
        );
        let gid = borrowed.glyph_index('a').unwrap_or(1);
        assert_eq!(
            shared.font().glyph_outline(gid).unwrap(),
            borrowed.glyph_outline(gid).unwrap()
        );
    }
}

/// The handle is `'static`: it can be built from a temporary buffer,
/// outlive it, move into a long-lived cache and be returned from the
/// scope that created it.
#[test]
fn shared_font_outlives_its_source_buffer_and_first_handle() {
    fn open() -> SharedFont {
        let buffer: Vec<u8> = DEJAVU.to_vec();
        let font = SharedFont::from_bytes(buffer).unwrap();
        let clone = font.clone();
        assert!(font.ptr_eq(&clone));
        drop(font);
        clone
    }
    let cache: Vec<Box<dyn std::any::Any + Send + Sync>> = vec![Box::new(open())];
    let font = cache[0].downcast_ref::<SharedFont>().unwrap();
    let expected =
        Font::from_bytes(DEJAVU)
            .unwrap()
            .shape("AVATAR office", *b"latn", None, LATN_DEFAULT);
    assert_eq!(
        font.shape("AVATAR office", *b"latn", None, LATN_DEFAULT),
        expected
    );
    // Borrowed table data stays valid for the borrow of the handle.
    let name = font.font().family_name().map(str::to_owned);
    assert_eq!(name.as_deref(), Some("DejaVu Sans"));
}

#[test]
fn shared_font_shares_an_existing_arc_without_copying() {
    let data: Arc<[u8]> = Arc::from(INTER);
    let font = SharedFont::from_bytes(Arc::clone(&data)).unwrap();
    assert!(Arc::ptr_eq(&data, &font.shared_data()));
    assert_eq!(font.font().bytes().as_ptr(), data.as_ptr());
}

#[test]
fn shared_font_shapes_concurrently_from_clones() {
    let font = SharedFont::from_bytes(INTER).unwrap();
    let expected: Vec<_> = [LATIN_PARAGRAPH, LIGA_KERN_HEAVY, LATIN_COMBINING]
        .iter()
        .map(|t| {
            Font::from_bytes(INTER)
                .unwrap()
                .shape(t, *b"latn", None, LATN_RICH)
        })
        .collect();
    // Threads race to build the plans and accelerators of a cold font.
    let workers: Vec<_> = (0..6)
        .map(|w| {
            let font = font.clone();
            std::thread::spawn(move || {
                let mut out = Vec::new();
                for round in 0..4 {
                    let pick = (w + round) % 3;
                    let text = [LATIN_PARAGRAPH, LIGA_KERN_HEAVY, LATIN_COMBINING][pick];
                    out.push((pick, font.shape(text, *b"latn", None, LATN_RICH)));
                }
                out
            })
        })
        .collect();
    for worker in workers {
        for (pick, shaped) in worker.join().unwrap() {
            assert_eq!(shaped, expected[pick]);
        }
    }
}

#[test]
fn shared_font_variation_instances_are_independent_handles() {
    let default = SharedFont::from_bytes(INTER).unwrap();
    let bold = default.with_variation_coords(&[900.0, 32.0]);
    assert!(!default.ptr_eq(&bold));
    assert!(Arc::ptr_eq(&default.shared_data(), &bold.shared_data()));

    let mut reference = Font::from_bytes(INTER).unwrap();
    let at_default = reference.shape(LIGA_KERN_HEAVY, *b"latn", None, LATN_RICH);
    reference.set_variation_coords(&[900.0, 32.0]);
    assert_eq!(bold.font().variation_coords(), reference.variation_coords());
    assert_eq!(
        bold.shape(LIGA_KERN_HEAVY, *b"latn", None, LATN_RICH),
        reference.shape(LIGA_KERN_HEAVY, *b"latn", None, LATN_RICH)
    );
    let gid = reference.glyph_index('B').unwrap();
    assert_eq!(
        bold.font().glyph_outline(gid).unwrap(),
        reference.glyph_outline(gid).unwrap()
    );
    // The original handle is untouched.
    assert_eq!(
        default.shape(LIGA_KERN_HEAVY, *b"latn", None, LATN_RICH),
        at_default
    );
}

#[test]
fn shared_font_reports_parse_errors() {
    assert!(SharedFont::from_bytes(vec![0u8; 3]).is_err());
    let garbage: Result<SharedFont, Error> = SharedFont::from_bytes(&b"not a font at all"[..]);
    assert_eq!(garbage.err(), Font::from_bytes(b"not a font at all").err());
    // A plain font is not a collection.
    assert_eq!(
        SharedFont::from_collection_bytes(DEJAVU, 0).err(),
        Font::from_collection_bytes(DEJAVU, 0).err()
    );
}

#[test]
fn shared_font_without_accelerators_shapes_identically() {
    let fast = SharedFont::from_bytes(INTER)
        .unwrap()
        .with_variation_coords(&[650.0, 20.0]);
    let lean = fast.with_shaping_accelerators(false);
    assert!(fast.font().shaping_accelerators());
    assert!(!lean.font().shaping_accelerators());
    assert_eq!(
        lean.font().variation_coords(),
        fast.font().variation_coords()
    );
    assert!(Arc::ptr_eq(&fast.shared_data(), &lean.shared_data()));
    for text in ["AVATAR office waffle 1/2 -> Ty", LATIN_COMBINING] {
        assert_eq!(
            lean.shape(text, *b"latn", None, LATN_RICH),
            fast.shape(text, *b"latn", None, LATN_RICH)
        );
    }
    // Derived instances keep the setting.
    assert!(
        !lean
            .with_variation_coords(&[400.0, 14.0])
            .font()
            .shaping_accelerators()
    );
}
