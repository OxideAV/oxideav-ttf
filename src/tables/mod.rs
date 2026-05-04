//! OpenType table parsers.
//!
//! Each module here decodes one specific table from a `&[u8]` slice
//! borrowed from the parent font; nothing in this directory does its
//! own I/O. The four-byte ASCII table tags (`b"head"`, `b"glyf"`, …)
//! are documented per-module.

pub mod cbdt;
pub mod cblc;
pub mod cmap;
pub mod colr;
pub mod cpal;
pub mod gdef;
pub mod glyf;
pub mod gpos;
pub mod gsub;
pub mod head;
pub mod hhea;
pub mod hmtx;
pub mod kern;
pub mod loca;
pub mod maxp;
pub mod name;
pub mod os2;
pub mod post;
