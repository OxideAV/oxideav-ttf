//! `VDMX` — vertical device metrics.
//!
//! Spec: ISO/IEC 14496-22:2019 §5.7.8 ("VDMX – Vertical device
//! metrics"). The `VDMX` table relates to OFF fonts with TrueType
//! outlines. Under Windows the `usWinAscent` / `usWinDescent` values
//! from the `OS/2` table determine the maximum black height for a
//! font at any given size ("Font Height"). Because TrueType
//! instructions can lead to Font Heights that differ from the actual
//! scaled and rounded values, basing the Font Height strictly on
//! `head.yMax` / `head.yMin` can result in "lost pixels". To avoid
//! grid-fitting the entire font to learn the correct height, `VDMX`
//! publishes, at a curated set of ppem sizes (and optionally per
//! aspect-ratio), the maximum and minimum vertical pixel coordinates
//! reached by any glyph after hinting.
//!
//! `VDMX` complements the horizontal-advance precomputation in
//! [`hdmx`](crate::tables::hdmx) and the "from this ppem onwards
//! linear scaling is safe" threshold in
//! [`LTSH`](crate::tables::ltsh): together the three tables let a
//! rasteriser avoid scan-converting a glyph in a number of common
//! "just laying out a line" situations.
//!
//! ## Layout (§5.7.8)
//!
//! ```text
//! VDMX Header
//! uint16 version              // 0 or 1
//! uint16 numRecs              // number of VDMX groups (vTable groupings)
//! uint16 numRatios            // number of RatioRange records
//! RatioRange ratRange[numRatios]
//! Offset16 offset[numRatios]  // each points to one VDMX group
//! Vdmx groups[numRecs]        // the actual VDMX groupings (see below)
//!
//! RatioRange Record (4 bytes)
//! uint8 bCharSet
//! uint8 xRatio
//! uint8 yStartRatio
//! uint8 yEndRatio
//!
//! VDMX Group
//! uint16 recs                 // number of vTable records in this group
//! uint8  startsz              // starting yPelHeight
//! uint8  endsz                // ending yPelHeight
//! vTable entry[recs]
//!
//! vTable Record (6 bytes)
//! uint16 yPelHeight           // sorted ascending
//! int16  yMax                 // max pels for this yPelHeight
//! int16  yMin                 // min pels for this yPelHeight
//! ```
//!
//! ## Aspect-ratio matching (§5.7.8 "Range checks")
//!
//! Ratios let the font ship distinct max/min curves for non-square
//! pixels. The §5.7.8 conceptual range check is
//!
//! ```text
//! (deviceXRatio == xRatio)
//!     && (deviceYRatio >= yStartRatio)
//!     && (deviceYRatio <= yEndRatio)
//! ```
//!
//! "Once a match is found, the search stops." The sentinel record
//! `(xRatio = 0, yStartRatio = 0, yEndRatio = 0)` signals "applies
//! to all aspect ratios"; if present it must be the last record in
//! the array, and if encountered during the search it is taken. If
//! the search runs off the end of the array without a hit (and
//! without the sentinel), the spec says "there is no VDMX data for
//! that aspect ratio." `Ratios of 2:2 are the same as 1:1.`
//!
//! `numRatios` records and `numRatios` Offset16 entries appear in
//! parallel arrays; index `i` of the offset array picks the VDMX
//! group bound to the `i`-th RatioRange. Multiple ratios may share
//! one group (they just point at the same offset), so `numRecs` and
//! `numRatios` are independent.
//!
//! ## yPelHeight ordering (§5.7.8 "This table must appear in
//! sorted order")
//!
//! Within one VDMX group the §5.7.8 invariant is "sorted order
//! (sorted by `yPelHeight`), but need not be continuous". We enforce
//! strictly-increasing `yPelHeight` so a duplicate or out-of-order
//! record cannot silently shadow a later lookup. The group header
//! independently carries the `startsz` / `endsz` ppem bracket; a
//! conforming font writes `startsz == entry[0].yPelHeight` and
//! `endsz == entry[recs-1].yPelHeight`, but the spec phrases these
//! as "Starting" / "Ending" yPelHeight rather than as derived
//! fields, so we surface the on-wire bytes verbatim and do not
//! cross-check.
//!
//! ## yPelHeight extent (§5.7.8 closing paragraph)
//!
//! "Please note that while the Ratios structure can only support
//! ppem sizes up to 255, the vTable structure can support much
//! larger pel heights (up to 65535)." This is why `yPelHeight` is
//! `uint16` even though all device-ratio probing is bracketed at
//! `uint8`. Per-ppem lookup therefore takes a `u16` ppem argument.
//!
//! ## Use sites (§5.7.4 + §7.3.5)
//!
//! §5.7.4 names `hdmx` and `vdmx` as the precomputed-advance method
//! pair complementing the `LTSH` threshold method. §7.3.5 (Metrics
//! Variations) calls out that "the 'hdmx' and VDMX tables are not
//! used in variable fonts" — a variable-font implementation
//! interpolates the equivalent through `MVAR` instead. We parse
//! the table whenever it is present; a caller that wants to honour
//! the §7.3.5 rule can cross-check `is_variable()` before
//! consulting these accessors.

use crate::parser::{read_i16, read_u16, read_u8};
use crate::Error;

/// On-wire table tag (`b"VDMX"`, big-endian Fixed `0x56444D58`).
/// Exposed for callers that walk the table directory directly.
pub const VDMX_TABLE_TAG: u32 = 0x5644_4D58;

/// VDMX version 0 — the older format. §5.7.8 "Character Set Values"
/// table-bound semantics for `bCharSet` differ between versions; the
/// numeric layout of the header + groups is identical.
pub const VDMX_VERSION_0: u16 = 0;

/// VDMX version 1 — the recommended version per §5.7.8 "It is
/// recommended that VDMX version 1 be used."
pub const VDMX_VERSION_1: u16 = 1;

/// Header byte count: `version` (2) + `numRecs` (2) + `numRatios` (2)
/// = 6 bytes per §5.7.8.
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub const VDMX_HEADER_LEN: usize = 6;

/// One RatioRange record byte count: `bCharSet` + `xRatio` +
/// `yStartRatio` + `yEndRatio` = 4 bytes per §5.7.8.
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub const VDMX_RATIO_RECORD_LEN: usize = 4;

/// One Offset16 entry byte count following the ratio array.
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub const VDMX_OFFSET_LEN: usize = 2;

/// VDMX group header byte count: `recs` (2) + `startsz` (1) + `endsz`
/// (1) = 4 bytes per §5.7.8.
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub const VDMX_GROUP_HEADER_LEN: usize = 4;

/// One vTable record byte count: `yPelHeight` (2) + `yMax` (2) +
/// `yMin` (2) = 6 bytes per §5.7.8.
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub const VDMX_VTABLE_RECORD_LEN: usize = 6;

/// RatioRange record from the `VDMX` ratio array (§5.7.8).
///
/// Aspect-ratio selectors map a target device's pixel aspect ratio
/// to one VDMX group. Square-pixel monitors use `xRatio = 1`,
/// `yStartRatio = 1`, `yEndRatio = 1`. The sentinel record
/// `(xRatio = 0, yStartRatio = 0, yEndRatio = 0)` matches every
/// ratio and, when present, must be the last entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RatioRange {
    /// On-wire `bCharSet` byte. §5.7.8 "The bCharSet value is used to
    /// denote cases where the VDMX group was computed based on a
    /// subset of the glyphs present in the font file." The numeric
    /// semantics differ between version 0 and version 1; we surface
    /// the raw byte so the caller can interpret it per its `VDMX`
    /// version.
    pub char_set: u8,
    /// On-wire `xRatio`. Zero means "matches all aspect ratios"
    /// (sentinel record).
    pub x_ratio: u8,
    /// On-wire `yStartRatio`. The lower bound of the device-ratio
    /// match window (inclusive).
    pub y_start_ratio: u8,
    /// On-wire `yEndRatio`. The upper bound of the device-ratio
    /// match window (inclusive).
    pub y_end_ratio: u8,
}

impl RatioRange {
    /// `true` when this record is the §5.7.8 catch-all sentinel
    /// `(xRatio = 0, yStartRatio = 0, yEndRatio = 0)`. The spec
    /// requires the sentinel to be the last entry if present.
    pub fn is_sentinel(&self) -> bool {
        self.x_ratio == 0 && self.y_start_ratio == 0 && self.y_end_ratio == 0
    }

    /// Apply the §5.7.8 conceptual range check against a target
    /// device's `(deviceXRatio, deviceYRatio)` pair. Returns `true`
    /// for the catch-all sentinel.
    ///
    /// Per §5.7.8: `Ratios of 2:2 are the same as 1:1` — callers are
    /// expected to pass already-normalised ratios; this predicate
    /// only enforces the literal comparison the spec spells out.
    pub fn matches(&self, device_x_ratio: u8, device_y_ratio: u8) -> bool {
        if self.is_sentinel() {
            return true;
        }
        device_x_ratio == self.x_ratio
            && device_y_ratio >= self.y_start_ratio
            && device_y_ratio <= self.y_end_ratio
    }
}

/// One vTable record from a VDMX group (§5.7.8). Carries the
/// `(yMax, yMin)` pel envelope reached by the font's hinted glyphs at
/// a specific `yPelHeight`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VdmxVTableRecord {
    /// On-wire `yPelHeight`. The ppem at which the `(yMax, yMin)`
    /// envelope applies. `uint16`; per §5.7.8 closing paragraph,
    /// vTable supports up to 65535 even though aspect-ratio
    /// matching is bracketed at 255.
    pub y_pel_height: u16,
    /// On-wire `yMax`. Maximum vertical pel coordinate reached by any
    /// glyph at `y_pel_height`, in (typically positive) pixels above
    /// the baseline.
    pub y_max: i16,
    /// On-wire `yMin`. Minimum vertical pel coordinate, typically a
    /// negative value (pixels below the baseline).
    pub y_min: i16,
}

/// One VDMX group: `(startsz, endsz)` bracket plus a sorted
/// `entry[recs]` array of `(yPelHeight, yMax, yMin)` tuples.
#[derive(Debug, Clone)]
pub struct VdmxGroup {
    start_sz: u8,
    end_sz: u8,
    entries: Vec<VdmxVTableRecord>,
}

impl VdmxGroup {
    /// On-wire `startsz` byte — the "Starting yPelHeight" of the
    /// group. A conforming font matches `entries[0].yPelHeight`; we
    /// surface the on-wire byte without cross-checking, since
    /// §5.7.8 specifies them as separate fields.
    pub fn start_sz(&self) -> u8 {
        self.start_sz
    }

    /// On-wire `endsz` byte — the "Ending yPelHeight" of the group.
    pub fn end_sz(&self) -> u8 {
        self.end_sz
    }

    /// On-wire `recs` count — equals `entries().len()` after a
    /// successful parse.
    pub fn num_entries(&self) -> u16 {
        // entries.len() <= u16::MAX because parse() walked up from a
        // u16 `recs` field.
        self.entries.len() as u16
    }

    /// All vTable records in document order. §5.7.8 mandates
    /// strictly-ascending `yPelHeight`; this slice walks ppem values
    /// in ascending order.
    pub fn entries(&self) -> &[VdmxVTableRecord] {
        &self.entries
    }

    /// Pick the vTable record whose `yPelHeight` exactly equals
    /// `ppem`. §5.7.8 ("need not be continuous") allows gaps in
    /// the array; an unrecorded ppem returns `None` and the caller
    /// falls back to grid-fitting the font. A `u16` ppem is taken
    /// because the on-wire field is `uint16`.
    pub fn record_for_ppem(&self, ppem: u16) -> Option<&VdmxVTableRecord> {
        match self.entries.binary_search_by_key(&ppem, |e| e.y_pel_height) {
            Ok(i) => self.entries.get(i),
            Err(_) => None,
        }
    }

    /// `(yMax, yMin)` pel envelope at the requested `ppem`, or
    /// `None` if the group does not record that exact ppem.
    pub fn y_extent_for_ppem(&self, ppem: u16) -> Option<(i16, i16)> {
        self.record_for_ppem(ppem).map(|r| (r.y_max, r.y_min))
    }
}

/// Parsed `VDMX` table.
///
/// Groups are stored in the order discovered from the per-ratio
/// Offset16 array, **deduplicated** so that two ratios that point at
/// the same on-wire group still resolve to one stored `VdmxGroup`
/// (and share its index across multiple `RatioRange`s). The
/// `ratio_group_index` per-ratio mapping points back into the
/// canonicalised `groups` vector.
#[derive(Debug, Clone)]
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub struct VdmxTable {
    version: u16,
    num_recs: u16,
    ratios: Vec<RatioRange>,
    /// One entry per RatioRange — index into `groups` of the VDMX
    /// group assigned to that ratio. Multiple ratios may share a
    /// group.
    ratio_group_index: Vec<usize>,
    groups: Vec<VdmxGroup>,
}

impl VdmxTable {
    /// Parse a `VDMX` table from its raw slice. Validates the
    /// header, the ratio array, the offset array, and every VDMX
    /// group whose offset is referenced by at least one ratio.
    ///
    /// Cross-checks enforced at parse time:
    /// * Recognised `version` field (0 or 1).
    /// * `numRatios >= 1` per §5.7.8 "Each ratio grouping refers
    ///   to a specific VDMX record group" + "there must be at least
    ///   1 VDMX group in the table".
    /// * Each per-ratio `Offset16` lies inside the table bytes.
    /// * Per-group `recs` records fit in the remaining table bytes.
    /// * Within each referenced group the `yPelHeight` array is
    ///   strictly increasing (§5.7.8 "sorted by yPelHeight").
    /// * Sentinel `(0, 0, 0)` records — if present — appear only
    ///   as the last RatioRange entry (§5.7.8 "if present, this
    ///   must be the last Ratio group in the table").
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < VDMX_HEADER_LEN {
            return Err(Error::UnexpectedEof);
        }
        let version = read_u16(bytes, 0)?;
        if version != VDMX_VERSION_0 && version != VDMX_VERSION_1 {
            return Err(Error::BadStructure("VDMX: unrecognised version"));
        }
        let num_recs = read_u16(bytes, 2)?;
        let num_ratios = read_u16(bytes, 4)?;
        if num_ratios == 0 {
            return Err(Error::BadStructure("VDMX: numRatios must be at least 1"));
        }
        if num_recs == 0 {
            return Err(Error::BadStructure(
                "VDMX: numRecs must be at least 1 (§5.7.8)",
            ));
        }
        // Bounds check the ratio + offset arrays.
        let ratio_array_off = VDMX_HEADER_LEN;
        let ratio_array_bytes = (num_ratios as usize)
            .checked_mul(VDMX_RATIO_RECORD_LEN)
            .ok_or(Error::BadStructure("VDMX: numRatios overflow"))?;
        let offset_array_off = ratio_array_off
            .checked_add(ratio_array_bytes)
            .ok_or(Error::BadStructure("VDMX: ratio array overflow"))?;
        let offset_array_bytes = (num_ratios as usize)
            .checked_mul(VDMX_OFFSET_LEN)
            .ok_or(Error::BadStructure("VDMX: offset array overflow"))?;
        let after_offsets = offset_array_off
            .checked_add(offset_array_bytes)
            .ok_or(Error::BadStructure("VDMX: offset array overflow"))?;
        if bytes.len() < after_offsets {
            return Err(Error::UnexpectedEof);
        }
        // Walk the RatioRange array, enforcing the §5.7.8 sentinel-
        // last invariant.
        let mut ratios = Vec::with_capacity(num_ratios as usize);
        for i in 0..(num_ratios as usize) {
            let off = ratio_array_off + i * VDMX_RATIO_RECORD_LEN;
            let r = RatioRange {
                char_set: read_u8(bytes, off)?,
                x_ratio: read_u8(bytes, off + 1)?,
                y_start_ratio: read_u8(bytes, off + 2)?,
                y_end_ratio: read_u8(bytes, off + 3)?,
            };
            if r.is_sentinel() && (i + 1) != (num_ratios as usize) {
                return Err(Error::BadStructure(
                    "VDMX: sentinel ratio record must be last",
                ));
            }
            ratios.push(r);
        }
        // Walk the Offset16 array, collecting the raw per-ratio
        // offsets in document order. We canonicalise to unique
        // group offsets below so two ratios sharing one group resolve
        // to one parsed VdmxGroup.
        let mut raw_offsets = Vec::with_capacity(num_ratios as usize);
        for i in 0..(num_ratios as usize) {
            let off = offset_array_off + i * VDMX_OFFSET_LEN;
            raw_offsets.push(read_u16(bytes, off)? as usize);
        }
        // Build the canonical group list in first-seen order.
        let mut unique_offsets: Vec<usize> = Vec::new();
        let mut ratio_group_index = Vec::with_capacity(num_ratios as usize);
        for &off in &raw_offsets {
            if off == 0 {
                // §5.7.8 does not call out a NULL offset, but a
                // zero offset overlaps the header and cannot point
                // at a valid VDMX group. Reject so a corrupted
                // ratio array does not silently alias to header
                // bytes.
                return Err(Error::BadStructure(
                    "VDMX: per-ratio offset must not be zero",
                ));
            }
            let idx = match unique_offsets.iter().position(|&o| o == off) {
                Some(i) => i,
                None => {
                    unique_offsets.push(off);
                    unique_offsets.len() - 1
                }
            };
            ratio_group_index.push(idx);
        }
        // Parse every unique group.
        let mut groups = Vec::with_capacity(unique_offsets.len());
        for &off in &unique_offsets {
            groups.push(Self::parse_group(bytes, off)?);
        }
        // §5.7.8 "there must be at least 1 VDMX group in the table"
        // already enforced via numRecs > 0; surface a mismatch
        // between the header count and the unique-offset count as
        // BadStructure (it shouldn't drop below numRecs, but it can
        // exceed when the font shares groups).
        if (num_recs as usize) < unique_offsets.len() {
            return Err(Error::BadStructure(
                "VDMX: numRecs lower than number of distinct group offsets",
            ));
        }
        Ok(Self {
            version,
            num_recs,
            ratios,
            ratio_group_index,
            groups,
        })
    }

    fn parse_group(bytes: &[u8], off: usize) -> Result<VdmxGroup, Error> {
        let end = off
            .checked_add(VDMX_GROUP_HEADER_LEN)
            .ok_or(Error::BadStructure("VDMX: group offset overflow"))?;
        if bytes.len() < end {
            return Err(Error::BadOffset);
        }
        let recs = read_u16(bytes, off)?;
        let start_sz = read_u8(bytes, off + 2)?;
        let end_sz = read_u8(bytes, off + 3)?;
        let entries_off = end;
        let entries_bytes = (recs as usize)
            .checked_mul(VDMX_VTABLE_RECORD_LEN)
            .ok_or(Error::BadStructure("VDMX: group recs overflow"))?;
        let entries_end = entries_off
            .checked_add(entries_bytes)
            .ok_or(Error::BadStructure("VDMX: group recs overflow"))?;
        if bytes.len() < entries_end {
            return Err(Error::UnexpectedEof);
        }
        let mut entries = Vec::with_capacity(recs as usize);
        let mut prev_ppem: Option<u16> = None;
        for i in 0..(recs as usize) {
            let rec_off = entries_off + i * VDMX_VTABLE_RECORD_LEN;
            let y_pel_height = read_u16(bytes, rec_off)?;
            let y_max = read_i16(bytes, rec_off + 2)?;
            let y_min = read_i16(bytes, rec_off + 4)?;
            if let Some(prev) = prev_ppem {
                if y_pel_height <= prev {
                    return Err(Error::BadStructure(
                        "VDMX: vTable yPelHeight not strictly increasing",
                    ));
                }
            }
            prev_ppem = Some(y_pel_height);
            entries.push(VdmxVTableRecord {
                y_pel_height,
                y_max,
                y_min,
            });
        }
        Ok(VdmxGroup {
            start_sz,
            end_sz,
            entries,
        })
    }

    /// Raw `version` field — `0` or `1` per §5.7.8.
    pub fn version_raw(&self) -> u16 {
        self.version
    }

    /// On-wire `numRecs` field — the font's claimed VDMX group
    /// count. May exceed the number of unique stored groups when
    /// the spec writer counted groups that no ratio references
    /// (the parser is silent on those; only referenced groups are
    /// parsed).
    pub fn num_recs(&self) -> u16 {
        self.num_recs
    }

    /// `numRatios` — equal to `ratios().len()` after parse.
    pub fn num_ratios(&self) -> u16 {
        // ratios.len() <= u16::MAX because parse() walked up from a
        // non-zero u16.
        self.ratios.len() as u16
    }

    /// The full RatioRange array in document order.
    pub fn ratios(&self) -> &[RatioRange] {
        &self.ratios
    }

    /// All parsed VDMX groups, deduplicated so shared groups appear
    /// once. Use [`Self::group_for_ratio_index`] to walk back from a
    /// RatioRange to its group.
    pub fn groups(&self) -> &[VdmxGroup] {
        &self.groups
    }

    /// VDMX group bound to the `ratio_index`-th RatioRange entry.
    /// Returns `None` when `ratio_index` is outside `ratios()`.
    pub fn group_for_ratio_index(&self, ratio_index: usize) -> Option<&VdmxGroup> {
        let gi = *self.ratio_group_index.get(ratio_index)?;
        self.groups.get(gi)
    }

    /// Find the §5.7.8 first-match RatioRange entry for a device's
    /// `(deviceXRatio, deviceYRatio)` pair and return its bound VDMX
    /// group. Honours the spec's "once a match is found, the search
    /// stops"; the catch-all sentinel `(0, 0, 0)` matches every
    /// ratio if reached. Returns `None` when no RatioRange matches
    /// and no sentinel is present — per §5.7.8 "there is no VDMX
    /// data for that aspect ratio".
    pub fn group_for_device_ratio(
        &self,
        device_x_ratio: u8,
        device_y_ratio: u8,
    ) -> Option<&VdmxGroup> {
        for (i, r) in self.ratios.iter().enumerate() {
            if r.matches(device_x_ratio, device_y_ratio) {
                return self.group_for_ratio_index(i);
            }
        }
        None
    }

    /// `(yMax, yMin)` pel envelope for `(ppem, deviceXRatio,
    /// deviceYRatio)`. Convenience composition of
    /// [`Self::group_for_device_ratio`] + [`VdmxGroup::y_extent_for_ppem`].
    pub fn y_extent_for_device(
        &self,
        ppem: u16,
        device_x_ratio: u8,
        device_y_ratio: u8,
    ) -> Option<(i16, i16)> {
        self.group_for_device_ratio(device_x_ratio, device_y_ratio)?
            .y_extent_for_ppem(ppem)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a one-ratio one-group VDMX table for the common
    /// "square-pixel, all-glyphs" case. Returns bytes + the offset
    /// where the group starts.
    fn make_simple_vdmx(version: u16, entries: &[(u16, i16, i16)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&version.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes()); // numRecs
        out.extend_from_slice(&1u16.to_be_bytes()); // numRatios
                                                    // RatioRange: (charset=1, 1:1)
        out.extend_from_slice(&[1, 1, 1, 1]);
        // Offset16 to the single group.
        let group_off = VDMX_HEADER_LEN + VDMX_RATIO_RECORD_LEN + VDMX_OFFSET_LEN;
        out.extend_from_slice(&(group_off as u16).to_be_bytes());
        // Group header.
        out.extend_from_slice(&(entries.len() as u16).to_be_bytes());
        out.push(entries.first().map(|e| e.0 as u8).unwrap_or(0));
        out.push(entries.last().map(|e| e.0 as u8).unwrap_or(0));
        for &(ppem, ymax, ymin) in entries {
            out.extend_from_slice(&ppem.to_be_bytes());
            out.extend_from_slice(&ymax.to_be_bytes());
            out.extend_from_slice(&ymin.to_be_bytes());
        }
        out
    }

    #[test]
    fn parses_single_ratio_single_group() {
        let bytes = make_simple_vdmx(VDMX_VERSION_1, &[(8, 7, -2), (12, 11, -3), (16, 14, -4)]);
        let t = VdmxTable::parse(&bytes).expect("parse");
        assert_eq!(t.version_raw(), VDMX_VERSION_1);
        assert_eq!(t.num_recs(), 1);
        assert_eq!(t.num_ratios(), 1);
        let r = &t.ratios()[0];
        assert_eq!(r.x_ratio, 1);
        assert!(!r.is_sentinel());
        assert!(r.matches(1, 1));
        assert!(!r.matches(2, 1));
        let g = t.group_for_ratio_index(0).expect("group");
        assert_eq!(g.start_sz(), 8);
        assert_eq!(g.end_sz(), 16);
        assert_eq!(g.num_entries(), 3);
        assert_eq!(g.entries()[1].y_pel_height, 12);
        assert_eq!(g.y_extent_for_ppem(12), Some((11, -3)));
        assert_eq!(g.y_extent_for_ppem(14), None); // unrecorded, no fallback
        assert_eq!(t.y_extent_for_device(16, 1, 1), Some((14, -4)));
        assert_eq!(t.y_extent_for_device(16, 2, 1), None); // no matching ratio, no sentinel
    }

    #[test]
    fn rejects_short_header() {
        let bytes = vec![0u8; 5];
        assert!(matches!(
            VdmxTable::parse(&bytes),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_unknown_version() {
        let mut bytes = vec![0u8; VDMX_HEADER_LEN];
        bytes[0..2].copy_from_slice(&2u16.to_be_bytes());
        assert!(matches!(
            VdmxTable::parse(&bytes),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn rejects_zero_num_ratios() {
        let mut bytes = vec![0u8; VDMX_HEADER_LEN];
        bytes[0..2].copy_from_slice(&VDMX_VERSION_1.to_be_bytes());
        bytes[2..4].copy_from_slice(&1u16.to_be_bytes());
        bytes[4..6].copy_from_slice(&0u16.to_be_bytes());
        assert!(matches!(
            VdmxTable::parse(&bytes),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn rejects_zero_num_recs() {
        let mut bytes = vec![0u8; VDMX_HEADER_LEN];
        bytes[0..2].copy_from_slice(&VDMX_VERSION_1.to_be_bytes());
        bytes[2..4].copy_from_slice(&0u16.to_be_bytes());
        bytes[4..6].copy_from_slice(&1u16.to_be_bytes());
        assert!(matches!(
            VdmxTable::parse(&bytes),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn rejects_truncated_offset_array() {
        let mut bytes = vec![0u8; VDMX_HEADER_LEN + VDMX_RATIO_RECORD_LEN];
        bytes[0..2].copy_from_slice(&VDMX_VERSION_1.to_be_bytes());
        bytes[2..4].copy_from_slice(&1u16.to_be_bytes());
        bytes[4..6].copy_from_slice(&1u16.to_be_bytes());
        // Ratio record present but the Offset16 entry is missing.
        assert!(matches!(
            VdmxTable::parse(&bytes),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_zero_offset() {
        // Build a header that claims one ratio + one offset, but the
        // Offset16 entry is 0.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&VDMX_VERSION_1.to_be_bytes());
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(&[1, 1, 1, 1]);
        bytes.extend_from_slice(&0u16.to_be_bytes());
        assert!(matches!(
            VdmxTable::parse(&bytes),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn rejects_non_monotonic_vtable() {
        // Two records at the same yPelHeight inside a single group
        // trips the §5.7.8 sort invariant.
        let mut out = Vec::new();
        out.extend_from_slice(&VDMX_VERSION_1.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&[1, 1, 1, 1]);
        let group_off = VDMX_HEADER_LEN + VDMX_RATIO_RECORD_LEN + VDMX_OFFSET_LEN;
        out.extend_from_slice(&(group_off as u16).to_be_bytes());
        out.extend_from_slice(&2u16.to_be_bytes()); // recs
        out.push(12);
        out.push(12);
        out.extend_from_slice(&12u16.to_be_bytes());
        out.extend_from_slice(&5i16.to_be_bytes());
        out.extend_from_slice(&(-1i16).to_be_bytes());
        out.extend_from_slice(&12u16.to_be_bytes()); // duplicate ppem
        out.extend_from_slice(&6i16.to_be_bytes());
        out.extend_from_slice(&(-2i16).to_be_bytes());
        assert!(matches!(
            VdmxTable::parse(&out),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn sentinel_must_be_last() {
        // Build a 2-ratio table with the sentinel at index 0 — the
        // spec rejects this.
        let mut out = Vec::new();
        out.extend_from_slice(&VDMX_VERSION_1.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&2u16.to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]); // sentinel first
        out.extend_from_slice(&[1, 1, 1, 1]);
        // Two offsets pointing at the same group.
        let group_off = VDMX_HEADER_LEN + 2 * VDMX_RATIO_RECORD_LEN + 2 * VDMX_OFFSET_LEN;
        out.extend_from_slice(&(group_off as u16).to_be_bytes());
        out.extend_from_slice(&(group_off as u16).to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.push(10);
        out.push(10);
        out.extend_from_slice(&10u16.to_be_bytes());
        out.extend_from_slice(&5i16.to_be_bytes());
        out.extend_from_slice(&(-1i16).to_be_bytes());
        assert!(matches!(
            VdmxTable::parse(&out),
            Err(Error::BadStructure(_))
        ));
    }

    #[test]
    fn sentinel_at_end_matches_all_ratios() {
        // Two-ratio table: 1:1 at index 0, sentinel at index 1; the
        // sentinel's group is selected for any non-1:1 device.
        let mut out = Vec::new();
        out.extend_from_slice(&VDMX_VERSION_1.to_be_bytes());
        out.extend_from_slice(&2u16.to_be_bytes());
        out.extend_from_slice(&2u16.to_be_bytes());
        out.extend_from_slice(&[1, 1, 1, 1]);
        out.extend_from_slice(&[0, 0, 0, 0]);
        // Two distinct groups.
        let g0_off = VDMX_HEADER_LEN + 2 * VDMX_RATIO_RECORD_LEN + 2 * VDMX_OFFSET_LEN;
        // Group 0 holds one record at 10 ppem.
        let g0_size = VDMX_GROUP_HEADER_LEN + VDMX_VTABLE_RECORD_LEN;
        let g1_off = g0_off + g0_size;
        out.extend_from_slice(&(g0_off as u16).to_be_bytes());
        out.extend_from_slice(&(g1_off as u16).to_be_bytes());
        // Group 0: one record at 10 ppem.
        out.extend_from_slice(&1u16.to_be_bytes());
        out.push(10);
        out.push(10);
        out.extend_from_slice(&10u16.to_be_bytes());
        out.extend_from_slice(&7i16.to_be_bytes());
        out.extend_from_slice(&(-2i16).to_be_bytes());
        // Group 1: one record at 12 ppem.
        out.extend_from_slice(&1u16.to_be_bytes());
        out.push(12);
        out.push(12);
        out.extend_from_slice(&12u16.to_be_bytes());
        out.extend_from_slice(&9i16.to_be_bytes());
        out.extend_from_slice(&(-3i16).to_be_bytes());

        let t = VdmxTable::parse(&out).expect("parse");
        assert_eq!(t.num_ratios(), 2);
        assert_eq!(t.groups().len(), 2);
        // Device (1, 1): matches the explicit 1:1 ratio.
        assert_eq!(t.y_extent_for_device(10, 1, 1), Some((7, -2)));
        // Device (2, 3): no explicit match, sentinel catches it.
        assert_eq!(t.y_extent_for_device(12, 2, 3), Some((9, -3)));
        // ppem 10 is not in group 1, even though the sentinel selects
        // group 1 for (2,3) — no fallback to the other group.
        assert_eq!(t.y_extent_for_device(10, 2, 3), None);
    }

    #[test]
    fn ratios_can_share_one_group() {
        // Two ratios pointing at one group: groups.len() == 1.
        let mut out = Vec::new();
        out.extend_from_slice(&VDMX_VERSION_1.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&2u16.to_be_bytes());
        out.extend_from_slice(&[1, 1, 1, 1]);
        out.extend_from_slice(&[1, 4, 3, 3]);
        let group_off = VDMX_HEADER_LEN + 2 * VDMX_RATIO_RECORD_LEN + 2 * VDMX_OFFSET_LEN;
        out.extend_from_slice(&(group_off as u16).to_be_bytes());
        out.extend_from_slice(&(group_off as u16).to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.push(11);
        out.push(11);
        out.extend_from_slice(&11u16.to_be_bytes());
        out.extend_from_slice(&8i16.to_be_bytes());
        out.extend_from_slice(&(-2i16).to_be_bytes());

        let t = VdmxTable::parse(&out).expect("parse");
        assert_eq!(t.num_ratios(), 2);
        assert_eq!(t.groups().len(), 1); // de-duplicated
        let g0 = t.group_for_ratio_index(0).unwrap();
        let g1 = t.group_for_ratio_index(1).unwrap();
        // Same parsed VdmxGroup reachable from both ratios.
        assert_eq!(g0.entries().len(), g1.entries().len());
        assert_eq!(g0.y_extent_for_ppem(11), Some((8, -2)));
        assert_eq!(g1.y_extent_for_ppem(11), Some((8, -2)));
    }

    #[test]
    fn supports_y_pel_height_above_255() {
        // §5.7.8 closing paragraph: vTable yPelHeight is uint16 even
        // though ratio bracketing is uint8. A 1024-ppem record must
        // parse + look up correctly.
        let mut out = Vec::new();
        out.extend_from_slice(&VDMX_VERSION_1.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&[1, 1, 1, 1]);
        let group_off = VDMX_HEADER_LEN + VDMX_RATIO_RECORD_LEN + VDMX_OFFSET_LEN;
        out.extend_from_slice(&(group_off as u16).to_be_bytes());
        out.extend_from_slice(&2u16.to_be_bytes());
        out.push(0); // startsz / endsz lose precision above 255 (spec note)
        out.push(0);
        out.extend_from_slice(&12u16.to_be_bytes());
        out.extend_from_slice(&8i16.to_be_bytes());
        out.extend_from_slice(&(-2i16).to_be_bytes());
        out.extend_from_slice(&1024u16.to_be_bytes());
        out.extend_from_slice(&800i16.to_be_bytes());
        out.extend_from_slice(&(-200i16).to_be_bytes());

        let t = VdmxTable::parse(&out).expect("parse");
        let g = t.group_for_ratio_index(0).unwrap();
        assert_eq!(g.entries().len(), 2);
        assert_eq!(g.y_extent_for_ppem(12), Some((8, -2)));
        assert_eq!(g.y_extent_for_ppem(1024), Some((800, -200)));
    }

    #[test]
    fn ratio_matches_within_y_range() {
        let r = RatioRange {
            char_set: 1,
            x_ratio: 2,
            y_start_ratio: 1,
            y_end_ratio: 3,
        };
        assert!(r.matches(2, 1));
        assert!(r.matches(2, 2));
        assert!(r.matches(2, 3));
        assert!(!r.matches(2, 4));
        assert!(!r.matches(3, 2));
        assert!(!r.matches(0, 0)); // not a sentinel itself
    }

    #[test]
    fn version_0_also_parses() {
        let bytes = make_simple_vdmx(VDMX_VERSION_0, &[(10, 6, -2)]);
        let t = VdmxTable::parse(&bytes).expect("parse");
        assert_eq!(t.version_raw(), VDMX_VERSION_0);
        assert_eq!(t.y_extent_for_device(10, 1, 1), Some((6, -2)));
    }
}
