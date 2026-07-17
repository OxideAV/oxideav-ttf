//! `DSIG` — Digital Signature Table (ISO/IEC 14496-22:2019 §8.x
//! "DSIG – Digital signature table").
//!
//! The `DSIG` table carries the font file's digital signature. It is a
//! header followed by an array of `SignatureRecord`s, each pointing at a
//! *signature block* whose payload (for the only defined block format,
//! Format 1) is a PKCS#7 packet.
//!
//! ```text
//! DSIG Header
//!   uint32 version          (0x00000001)
//!   uint16 numSignatures
//!   uint16 flags            (bit 0 = cannot-be-resigned; bits 1-7 reserved)
//!   SignatureRecord signatureRecords[numSignatures]
//!
//! SignatureRecord
//!   uint32   format         (1 = the only defined block format)
//!   uint32   length         (length of the signature block, bytes)
//!   Offset32 offset         (to the block, from the start of the table)
//!
//! Signature Block Format 1
//!   uint16 reserved1        (0)
//!   uint16 reserved2        (0)
//!   uint32 signatureLength
//!   uint8  signature[signatureLength]   (PKCS#7 packet)
//! ```
//!
//! This module performs the **structural** decode: the header, the record
//! array, and — for Format-1 blocks — the reserved words, the length, and
//! the raw PKCS#7 payload surfaced as a borrowed `&[u8]`. It does **not**
//! parse the PKCS#7 / X.509 / ASN.1 contents or verify the signature
//! cryptographically; that is the host application's policy decision and is
//! out of scope for a font-table parser. Non-Format-1 blocks (none are
//! currently defined by the spec, but the format field is forward-looking)
//! surface their format id, declared length, and the raw block bytes.

use crate::parser::{read_u16, read_u32};
use crate::Error;

/// The current (and only) defined `DSIG` table version per the spec.
pub const DSIG_VERSION: u32 = 0x0000_0001;

/// The only signature-block format defined by the spec ("Signature Block
/// Format 1" — a PKCS#7 packet).
pub const DSIG_BLOCK_FORMAT_PKCS7: u32 = 1;

/// One `SignatureRecord` plus its resolved signature block.
#[derive(Debug, Clone)]
pub struct Signature<'a> {
    /// Block format id. `1` (= [`DSIG_BLOCK_FORMAT_PKCS7`]) is the only
    /// format the spec defines; other values surface raw for
    /// forward-compatibility.
    pub format: u32,
    /// Declared length of the signature block in bytes (the
    /// `SignatureRecord.length` field).
    pub length: u32,
    /// For a Format-1 block, the PKCS#7 packet bytes (the `signature`
    /// field), borrowed from the `DSIG` slice. `None` for an unrecognised
    /// block format — use [`Signature::raw_block`] to reach those bytes.
    pkcs7: Option<&'a [u8]>,
    /// The whole signature block as it sits on the wire (including the
    /// 8-byte Format-1 sub-header for Format-1 blocks), borrowed from the
    /// `DSIG` slice.
    raw_block: &'a [u8],
}

impl<'a> Signature<'a> {
    /// `true` if this is the spec's Format-1 (PKCS#7) signature block.
    pub fn is_pkcs7(&self) -> bool {
        self.format == DSIG_BLOCK_FORMAT_PKCS7
    }

    /// The PKCS#7 packet bytes for a Format-1 block, or `None` for an
    /// unrecognised block format. The bytes are surfaced raw; this crate
    /// does not parse or verify the PKCS#7 / X.509 contents.
    pub fn pkcs7_packet(&self) -> Option<&'a [u8]> {
        self.pkcs7
    }

    /// The entire signature block as it sits on the wire, including (for a
    /// Format-1 block) the 8-byte `reserved1` / `reserved2` /
    /// `signatureLength` sub-header. Useful for tooling that re-hashes or
    /// re-emits the block verbatim.
    pub fn raw_block(&self) -> &'a [u8] {
        self.raw_block
    }
}

/// Parsed `DSIG` table.
#[derive(Debug, Clone)]
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub struct DsigTable<'a> {
    version: u32,
    flags: u16,
    signatures: Vec<Signature<'a>>,
}

impl<'a> DsigTable<'a> {
    /// Decode a `DSIG` table from its byte slice.
    ///
    /// Validates the header `version == 1` per the spec. Each
    /// `SignatureRecord`'s `(offset, length)` is bounds-checked against the
    /// table; a record whose block runs past the table end is rejected as
    /// `BadStructure`. For a Format-1 block, the `signatureLength` field is
    /// bounds-checked against the block so a corrupt length cannot make the
    /// PKCS#7 slice escape the table.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < 8 {
            return Err(Error::UnexpectedEof);
        }
        let version = read_u32(bytes, 0)?;
        if version != DSIG_VERSION {
            return Err(Error::BadStructure("DSIG: unsupported version"));
        }
        let num_signatures = read_u16(bytes, 4)? as usize;
        let flags = read_u16(bytes, 6)?;

        // The SignatureRecord array (12 bytes each) follows the 8-byte
        // header. Bound the count against the table so a corrupt
        // numSignatures cannot over-read.
        let records_end = 8usize
            .checked_add(
                num_signatures
                    .checked_mul(12)
                    .ok_or(Error::BadStructure("DSIG: signature count overflow"))?,
            )
            .ok_or(Error::BadStructure("DSIG: record array overflow"))?;
        if records_end > bytes.len() {
            return Err(Error::UnexpectedEof);
        }

        let mut signatures = Vec::with_capacity(num_signatures);
        for i in 0..num_signatures {
            let rec = 8 + i * 12;
            let format = read_u32(bytes, rec)?;
            let length = read_u32(bytes, rec + 4)?;
            let offset = read_u32(bytes, rec + 8)? as usize;
            let block_end = offset
                .checked_add(length as usize)
                .ok_or(Error::BadStructure("DSIG: block range overflow"))?;
            if offset < 8 || block_end > bytes.len() {
                return Err(Error::BadStructure("DSIG: signature block out of bounds"));
            }
            let raw_block = &bytes[offset..block_end];

            // Format 1 = "Signature Block Format 1": uint16 reserved1,
            // uint16 reserved2, uint32 signatureLength, uint8 packet[].
            let pkcs7 = if format == DSIG_BLOCK_FORMAT_PKCS7 {
                if raw_block.len() < 8 {
                    return Err(Error::BadStructure("DSIG: Format-1 block too short"));
                }
                let sig_len = read_u32(raw_block, 4)? as usize;
                let packet_end = 8usize
                    .checked_add(sig_len)
                    .ok_or(Error::BadStructure("DSIG: signatureLength overflow"))?;
                if packet_end > raw_block.len() {
                    return Err(Error::BadStructure("DSIG: signatureLength past block"));
                }
                Some(&raw_block[8..packet_end])
            } else {
                None
            };

            signatures.push(Signature {
                format,
                length,
                pkcs7,
                raw_block,
            });
        }

        Ok(Self {
            version,
            flags,
            signatures,
        })
    }

    /// The table version (always [`DSIG_VERSION`] = 1 after a successful
    /// parse).
    pub fn version(&self) -> u32 {
        self.version
    }

    /// The header permission flags. Bit 0 set means the font cannot be
    /// re-signed (a signer asserting their signature is the last);
    /// bits 1-7 are reserved.
    pub fn flags(&self) -> u16 {
        self.flags
    }

    /// `true` if the header's "cannot be resigned" permission bit (bit 0)
    /// is set.
    pub fn cannot_be_resigned(&self) -> bool {
        (self.flags & 0x0001) != 0
    }

    /// The decoded signatures, in record order.
    pub fn signatures(&self) -> &[Signature<'a>] {
        &self.signatures
    }

    /// Number of signatures in the table.
    pub fn signature_count(&self) -> usize {
        self.signatures.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a DSIG with `n` Format-1 signature blocks, each carrying the
    /// PKCS#7 payload `payloads[i]`.
    fn build_dsig(flags: u16, payloads: &[&[u8]]) -> Vec<u8> {
        let n = payloads.len();
        let mut out = Vec::new();
        out.extend_from_slice(&DSIG_VERSION.to_be_bytes());
        out.extend_from_slice(&(n as u16).to_be_bytes());
        out.extend_from_slice(&flags.to_be_bytes());
        // Record array placeholder; blocks go after it.
        let records_off = out.len();
        out.extend(std::iter::repeat_n(0u8, n * 12));
        // Append each block and patch its record.
        for (i, payload) in payloads.iter().enumerate() {
            let block_off = out.len();
            // Format-1 block: reserved1(2) reserved2(2) signatureLength(4) packet.
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            out.extend_from_slice(payload);
            let block_len = (out.len() - block_off) as u32;
            let rec = records_off + i * 12;
            out[rec..rec + 4].copy_from_slice(&DSIG_BLOCK_FORMAT_PKCS7.to_be_bytes());
            out[rec + 4..rec + 8].copy_from_slice(&block_len.to_be_bytes());
            out[rec + 8..rec + 12].copy_from_slice(&(block_off as u32).to_be_bytes());
        }
        out
    }

    #[test]
    fn decodes_single_pkcs7_signature() {
        let payload: &[u8] = &[0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02];
        let bytes = build_dsig(0, &[payload]);
        let d = DsigTable::parse(&bytes).unwrap();
        assert_eq!(d.version(), DSIG_VERSION);
        assert_eq!(d.signature_count(), 1);
        assert!(!d.cannot_be_resigned());
        let sig = &d.signatures()[0];
        assert!(sig.is_pkcs7());
        assert_eq!(sig.pkcs7_packet(), Some(payload));
        // The raw block is the 8-byte Format-1 sub-header + payload.
        assert_eq!(sig.raw_block().len(), 8 + payload.len());
    }

    #[test]
    fn decodes_two_signatures_and_flags() {
        let p0: &[u8] = &[1, 2, 3];
        let p1: &[u8] = &[9, 8, 7, 6, 5];
        let bytes = build_dsig(0x0001, &[p0, p1]);
        let d = DsigTable::parse(&bytes).unwrap();
        assert_eq!(d.signature_count(), 2);
        assert!(d.cannot_be_resigned());
        assert_eq!(d.signatures()[0].pkcs7_packet(), Some(p0));
        assert_eq!(d.signatures()[1].pkcs7_packet(), Some(p1));
        assert_eq!(d.signatures()[1].length, (8 + p1.len()) as u32);
    }

    #[test]
    fn empty_signature_table() {
        let bytes = build_dsig(0, &[]);
        let d = DsigTable::parse(&bytes).unwrap();
        assert_eq!(d.signature_count(), 0);
        assert!(d.signatures().is_empty());
    }

    #[test]
    fn rejects_bad_version() {
        let mut bytes = build_dsig(0, &[&[1, 2]]);
        bytes[0..4].copy_from_slice(&2u32.to_be_bytes());
        assert!(matches!(
            DsigTable::parse(&bytes),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn rejects_short_header() {
        assert!(matches!(
            DsigTable::parse(&[0u8; 4]),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_block_out_of_bounds() {
        let mut bytes = build_dsig(0, &[&[1, 2, 3]]);
        // Patch record 0's offset to point past the table.
        bytes[8 + 8..8 + 12].copy_from_slice(&9999u32.to_be_bytes());
        assert!(matches!(
            DsigTable::parse(&bytes),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn rejects_signature_length_past_block() {
        let mut bytes = build_dsig(0, &[&[1, 2, 3, 4]]);
        // The block sits at the offset in record 0; bump its
        // signatureLength field (block_off + 4) past the block.
        let block_off = read_u32(&bytes, 8 + 8).unwrap() as usize;
        bytes[block_off + 4..block_off + 8].copy_from_slice(&9999u32.to_be_bytes());
        assert!(matches!(
            DsigTable::parse(&bytes),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn unknown_block_format_surfaces_raw() {
        let mut bytes = build_dsig(0, &[&[1, 2, 3, 4]]);
        // Change record 0's format to an undefined value.
        bytes[8..12].copy_from_slice(&7u32.to_be_bytes());
        let d = DsigTable::parse(&bytes).unwrap();
        let sig = &d.signatures()[0];
        assert!(!sig.is_pkcs7());
        assert_eq!(sig.format, 7);
        assert_eq!(sig.pkcs7_packet(), None);
        // The raw block is still reachable.
        assert!(!sig.raw_block().is_empty());
    }
}
