//! OpenType table parsers.
//!
//! Each module here decodes one specific table from a `&[u8]` slice
//! borrowed from the parent font; nothing in this directory does its
//! own I/O. The four-byte ASCII table tags (`b"head"`, `b"glyf"`, …)
//! are documented per-module.

pub mod avar;
pub mod base;
pub mod cbdt;
pub mod cblc;
pub mod cmap;
pub mod colr;
pub mod cpal;
pub mod fvar;
pub mod gasp;
pub mod gdef;
pub mod glyf;
pub mod gpos;
pub mod gsub;
pub mod gvar;
pub mod hdmx;
pub mod head;
pub mod hhea;
pub mod hmtx;
pub mod hvar;
pub mod kern;
pub mod loca;
pub mod ltsh;
pub mod maxp;
pub mod mvar;
pub mod name;
pub mod os2;
pub mod post;
pub mod sbix;
pub mod stat;
pub mod vdmx;
pub mod vhea;
pub mod vmtx;
pub mod vorg;
pub mod vvar;
