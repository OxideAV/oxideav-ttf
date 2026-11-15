//! [`SharedFont`]: an owned, reference-counted, `'static` font handle.
//!
//! [`Font`] borrows the font file bytes, which is the cheapest shape for
//! a caller that already holds them for the duration of the work. A
//! caller that wants to *keep* a parsed font — in a cache keyed by face,
//! behind an `Arc`, across threads — would otherwise have to store the
//! bytes and the `Font` borrowing them side by side. `SharedFont` is
//! that pairing done once, here, where the invariants it rests on can be
//! checked against `Font`'s internals.

use std::sync::Arc;

use crate::{Error, Font, ShapedGlyph};

/// A parsed font that owns its bytes: cheap to clone (one reference
/// count), `Send + Sync`, and `'static`.
///
/// Every clone shares one [`Font`], so whatever that font has parsed or
/// built so far — lazily parsed tables, shaping plans, per-lookup
/// shaping accelerators — is computed once and seen by all clones, on
/// all threads.
///
/// [`SharedFont::font`] hands out the underlying [`Font`], whose whole
/// borrowed API is available unchanged:
///
/// ```
/// # fn main() -> Result<(), oxideav_ttf::Error> {
/// # let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/DejaVuSans.ttf")).unwrap();
/// use oxideav_ttf::SharedFont;
///
/// let font = SharedFont::from_bytes(bytes)?; // Vec<u8>, Box<[u8]>, Arc<[u8]>, &[u8] …
/// let for_worker = font.clone();
/// let handle = std::thread::spawn(move || {
///     for_worker.shape("office", *b"latn", None, &[*b"liga", *b"kern"]).len()
/// });
/// assert_eq!(font.font().units_per_em(), 2048);
/// assert!(handle.join().unwrap() < "office".len());
/// # Ok(())
/// # }
/// ```
///
/// The variation instance of a `SharedFont` is fixed at construction
/// (the shared `Font` is never mutated);
/// [`SharedFont::with_variation_coords`] derives a handle for another
/// instance over the same bytes.
#[derive(Clone)]
pub struct SharedFont {
    inner: Arc<Inner>,
}

struct Inner {
    /// Borrows from `data`. Declared first so it is dropped first.
    ///
    /// The `'static` is a private fiction: the true lifetime is "as
    /// long as `data`". It never escapes — [`SharedFont::font`] re-binds
    /// it to the borrow of the handle.
    font: Font<'static>,
    /// The font file. Held for the life of `font`; never mutated, never
    /// replaced.
    data: Arc<[u8]>,
    /// The subfont index, when opened as a member of a collection.
    collection_index: Option<u32>,
}

impl SharedFont {
    /// Parse a single font (`.ttf` / `.otf`) and take ownership of its
    /// bytes. Accepts anything convertible to `Arc<[u8]>`: a `Vec<u8>`
    /// or `Box<[u8]>` (moved into the shared allocation), an existing
    /// `Arc<[u8]>` (shared, no copy), or a `&[u8]` (copied).
    ///
    /// Same validation and same errors as [`Font::from_bytes`].
    pub fn from_bytes(data: impl Into<Arc<[u8]>>) -> Result<Self, Error> {
        Self::build(data.into(), None, None)
    }

    /// Parse the `index`-th subfont of a TrueType Collection (`.ttc`).
    /// Same validation and same errors as
    /// [`Font::from_collection_bytes`].
    pub fn from_collection_bytes(data: impl Into<Arc<[u8]>>, index: u32) -> Result<Self, Error> {
        Self::build(data.into(), Some(index), None)
    }

    #[allow(unsafe_code)]
    fn build(
        data: Arc<[u8]>,
        collection_index: Option<u32>,
        coords: Option<&[f32]>,
    ) -> Result<Self, Error> {
        // SAFETY: `bytes` points into the heap allocation owned by
        // `data`. That allocation is neither moved nor freed while any
        // `Arc` to it exists, and `Inner` keeps one for at least as long
        // as the `Font` built from `bytes` (`font` is dropped before
        // `data`; if parsing fails, the borrow ends before `data` is
        // dropped at the end of this function). Nothing can mutate the
        // bytes: `Arc<[u8]>` only gives shared access while more than
        // one handle exists or while a shared borrow is live, and this
        // module never asks for exclusive access. The `'static` lifetime
        // is not observable outside this module (see `font`).
        let bytes: &'static [u8] = unsafe { &*Arc::as_ptr(&data) };
        let mut font = match collection_index {
            Some(index) => Font::from_collection_bytes(bytes, index)?,
            None => Font::from_bytes(bytes)?,
        };
        if let Some(coords) = coords {
            font.set_variation_coords(coords);
        }
        Ok(Self {
            inner: Arc::new(Inner {
                font,
                data,
                collection_index,
            }),
        })
    }

    /// The parsed font. Everything [`Font`] offers is reachable through
    /// this reference; results that borrow font data live as long as the
    /// borrow of this handle (clone the handle to keep them alive
    /// elsewhere).
    #[inline]
    #[allow(unsafe_code)]
    pub fn font(&self) -> &Font<'_> {
        // SAFETY: this narrows `&'s Font<'static>` to `&'s Font<'s>`.
        // `Font<'a>` only ever *produces* `'a` data: its constructors are
        // the sole functions taking `&'a` input, and its interior
        // caches (the `OnceLock`s of `LazyTable` and of the decoded
        // name strings — the reason the compiler treats `'a` as
        // invariant rather than covariant) are filled
        // exclusively by `Font`'s own parsers from the font's own
        // bytes, never from caller-supplied borrows. So no value with a
        // lifetime shorter than the data can be written through the
        // narrowed reference, and everything read through it is bounded
        // by `'s`, during which `self` keeps the data alive. The two
        // types differ only in that lifetime parameter, so their layout
        // is identical.
        unsafe { std::mem::transmute::<&Font<'static>, &Font<'_>>(&self.inner.font) }
    }

    /// The font file bytes this handle owns.
    #[inline]
    pub fn data(&self) -> &[u8] {
        &self.inner.data
    }

    /// Another reference to the shared byte allocation.
    pub fn shared_data(&self) -> Arc<[u8]> {
        Arc::clone(&self.inner.data)
    }

    /// The subfont index this handle was opened with, for a collection
    /// member.
    pub fn collection_index(&self) -> Option<u32> {
        self.inner.collection_index
    }

    /// `true` when both handles share one parsed font (clones of one
    /// another).
    pub fn ptr_eq(&self, other: &SharedFont) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// A handle for the variation instance at `coords` (user-space axis
    /// values, as for [`Font::set_variation_coords`]), over the same
    /// byte allocation. The new handle parses its tables and builds its
    /// shaping accelerators independently of `self`.
    pub fn with_variation_coords(&self, coords: &[f32]) -> SharedFont {
        Self::build(
            Arc::clone(&self.inner.data),
            self.inner.collection_index,
            Some(coords),
        )
        .expect("bytes that parsed once parse again")
    }

    /// Shape a run of text; see [`Font::shape`].
    pub fn shape(
        &self,
        text: &str,
        script: [u8; 4],
        lang: Option<[u8; 4]>,
        features: &[[u8; 4]],
    ) -> Vec<ShapedGlyph> {
        self.font().shape(text, script, lang, features)
    }
}

impl core::fmt::Debug for SharedFont {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SharedFont")
            .field("bytes", &self.inner.data.len())
            .field("collection_index", &self.inner.collection_index)
            .field("handles", &Arc::strong_count(&self.inner))
            .finish()
    }
}

// The whole point of the type: usable from any thread, storable
// anywhere. Checked at compile time so a future `Font` field that is not
// thread-safe fails here rather than in a consumer.
const _: fn() = || {
    fn assert_shareable<T: Send + Sync + Clone + 'static>() {}
    assert_shareable::<SharedFont>();
};
