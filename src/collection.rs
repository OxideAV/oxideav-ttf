//! TrueType Collection (`.ttc` / `.ttcf`) header parser.
//!
//! A TTC file packs several sfnt-flavoured fonts into one file with a
//! shared "TTC header" up front. The header announces the table:
//!
//! ```text
//! TTCHeader {
//!     u32 ttcTag;        // 'ttcf' (0x74746366)
//!     u16 majorVersion;  // 1 or 2
//!     u16 minorVersion;  // 0
//!     u32 numFonts;
//!     u32 offsetTable[numFonts]; // each points at a per-subfont sfnt header
//!     // version 2 only:
//!     // u32 dsigTag, dsigLength, dsigOffset
//! }
//! ```
//!
//! Each `offsetTable[i]` is the file-relative byte offset of the i-th
//! subfont's sfnt directory (the same `0x00010000` / `OTTO` magic + 12 byte
//! sfnt header that `parser.rs` parses). To consume a TTC, the caller picks
//! a subfont index and then runs the existing sfnt parsing path against
//! `&bytes[offset..]`.
//!
//! Spec references:
//! - Microsoft OpenType 1.9 §"Font Collections" / TTC header layout.
//! - Apple TrueType Reference Manual / "The Font File", "TrueType
//!   Collections".
//!
//! We accept versions 1.0 AND 2.0; the version-2-only DSIG (digital
//! signature) trailer is ignored — we never validate signatures.

use crate::parser::{read_u16, read_u32};
use crate::Error;

/// Magic four-byte tag that identifies the TTC container.
pub const TTC_MAGIC: u32 = 0x7474_6366; // 'ttcf' (big-endian)

/// Maximum subfont count we will accept. Real-world TTCs contain tens of
/// subfonts (Noto Sans CJK ships 7); the cap exists purely to bound how
/// much we read on malformed input.
const MAX_SUBFONTS: u32 = 1024;

/// Parsed TTC header. Carries the per-subfont byte offsets so the caller
/// can construct a `Font<'_>` over `&bytes[offset..]`.
#[derive(Debug, Clone)]
pub struct CollectionHeader {
    /// `(major, minor)` from the TTC header. Always `(1, 0)` or `(2, 0)`
    /// in real-world fonts; we don't enforce minor==0 strictly.
    pub version: (u16, u16),
    /// Per-subfont byte offsets within the parent file.
    pub offsets: Vec<u32>,
}

impl CollectionHeader {
    /// Try to parse a TTC header at the start of `bytes`. Returns
    /// `Error::BadMagic` if the leading 4 bytes are not `'ttcf'` —
    /// callers can use that to differentiate between a TTC and a plain
    /// sfnt without an explicit container probe.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 12 {
            return Err(Error::UnexpectedEof);
        }
        let tag = read_u32(bytes, 0)?;
        if tag != TTC_MAGIC {
            return Err(Error::BadMagic);
        }
        let major = read_u16(bytes, 4)?;
        let minor = read_u16(bytes, 6)?;
        if major != 1 && major != 2 {
            return Err(Error::BadHeader);
        }
        let num_fonts = read_u32(bytes, 8)?;
        if num_fonts == 0 || num_fonts > MAX_SUBFONTS {
            return Err(Error::BadHeader);
        }
        let table_end = 12usize
            .checked_add(num_fonts as usize * 4)
            .ok_or(Error::BadHeader)?;
        if bytes.len() < table_end {
            return Err(Error::UnexpectedEof);
        }
        let mut offsets = Vec::with_capacity(num_fonts as usize);
        for i in 0..num_fonts as usize {
            let off = read_u32(bytes, 12 + i * 4)?;
            // The offset must point into the buffer with at least 12 bytes
            // (the sfnt header) accessible, otherwise the subfont parse
            // would fail in a hard-to-diagnose way.
            if (off as usize)
                .checked_add(12)
                .map(|end| end > bytes.len())
                .unwrap_or(true)
            {
                return Err(Error::BadOffset);
            }
            offsets.push(off);
        }
        // version 2 trailer (DSIG) is left untouched.
        Ok(Self {
            version: (major, minor),
            offsets,
        })
    }

    /// Number of subfonts in this collection.
    pub fn num_fonts(&self) -> u32 {
        self.offsets.len() as u32
    }

    /// File-relative byte offset of subfont `index`. Returns `None` if
    /// `index` is out of range.
    pub fn font_offset(&self, index: u32) -> Option<u32> {
        self.offsets.get(index as usize).copied()
    }
}

/// `true` if `bytes` starts with the TTC magic (`ttcf`).
pub fn is_collection(bytes: &[u8]) -> bool {
    if bytes.len() < 4 {
        return false;
    }
    read_u32(bytes, 0).map(|t| t == TTC_MAGIC).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-built minimal TTC header: version 1, two subfonts, with
    /// pretend per-subfont offsets that point at fake but in-range data.
    fn synth_ttc_header() -> Vec<u8> {
        let mut bytes = vec![0u8; 256];
        // ttcTag
        bytes[0..4].copy_from_slice(&TTC_MAGIC.to_be_bytes());
        // version 1.0
        bytes[4..6].copy_from_slice(&1u16.to_be_bytes());
        bytes[6..8].copy_from_slice(&0u16.to_be_bytes());
        // numFonts = 2
        bytes[8..12].copy_from_slice(&2u32.to_be_bytes());
        // offsetTable[0] = 100, offsetTable[1] = 200
        bytes[12..16].copy_from_slice(&100u32.to_be_bytes());
        bytes[16..20].copy_from_slice(&200u32.to_be_bytes());
        bytes
    }

    #[test]
    fn parses_minimal_v1_collection() {
        let bytes = synth_ttc_header();
        let hdr = CollectionHeader::parse(&bytes).expect("parse");
        assert_eq!(hdr.version, (1, 0));
        assert_eq!(hdr.num_fonts(), 2);
        assert_eq!(hdr.font_offset(0), Some(100));
        assert_eq!(hdr.font_offset(1), Some(200));
        assert_eq!(hdr.font_offset(2), None);
    }

    #[test]
    fn rejects_non_ttc_magic() {
        let mut bytes = synth_ttc_header();
        bytes[0..4].copy_from_slice(&0x00010000u32.to_be_bytes());
        assert!(matches!(
            CollectionHeader::parse(&bytes),
            Err(Error::BadMagic)
        ));
    }

    #[test]
    fn rejects_offset_past_eof() {
        let mut bytes = synth_ttc_header();
        // Drop the buffer so offset 200 lands past end (need 12 bytes
        // accessible at the offset).
        bytes.truncate(150);
        // First subfont (offset 100) needs 12 bytes — fits in 150-byte
        // buffer (100..=112). Second (200) doesn't — should fail.
        assert!(matches!(
            CollectionHeader::parse(&bytes),
            Err(Error::BadOffset)
        ));
    }

    #[test]
    fn rejects_zero_subfonts() {
        let mut bytes = synth_ttc_header();
        bytes[8..12].copy_from_slice(&0u32.to_be_bytes());
        assert!(matches!(
            CollectionHeader::parse(&bytes),
            Err(Error::BadHeader)
        ));
    }

    #[test]
    fn is_collection_distinguishes() {
        assert!(is_collection(&TTC_MAGIC.to_be_bytes()));
        assert!(!is_collection(&0x00010000u32.to_be_bytes()));
        assert!(!is_collection(&0x4F54544Fu32.to_be_bytes())); // OTTO
        assert!(!is_collection(&[]));
    }
}
