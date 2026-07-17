//! OpenType table parsers.
//!
//! Each module here decodes one specific table from a `&[u8]` slice
//! borrowed from the parent font; nothing in this directory does its
//! own I/O. The four-byte ASCII table tags (`b"head"`, `b"glyf"`, …)
//! are documented per-module.

// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod avar;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod base;
pub mod cbdt;
pub mod cblc;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod cff;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod cff2;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod cmap;
pub mod colr;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod cpal;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod cvar;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod device;
pub mod dsig;
pub mod ebdt;
pub mod ebsc;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod feature_variations;
pub mod fvar;
pub mod gasp;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod gdef;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod glyf;
pub mod gpos;
pub mod gsub;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod gvar;
pub mod hdmx;
pub mod head;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod hhea;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod hmtx;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod hvar;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod jstf;
pub mod kern;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod loca;
pub mod ltsh;
pub mod math;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod maxp;
pub mod merg;
pub mod meta;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod mvar;
pub mod name;
pub mod os2;
pub mod pclt;
pub mod post;
pub mod sbix;
pub mod stat;
pub mod svg;
pub mod vdmx;
pub mod vhea;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod vmtx;
pub mod vorg;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod vvar;
