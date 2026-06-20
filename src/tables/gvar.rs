//! `gvar` — Glyph Variations Table.
//!
//! Spec: Microsoft OpenType §"gvar — Glyph Variations Table" / OpenType
//! 1.9. Apple TrueType Reference §"gvar".
//!
//! Per-glyph **TupleVariationStore**: a list of tuple variations whose
//! deltas, scaled by the current axis-coord vector, are added to the
//! static `glyf` outline. Each glyph has its own store, located via
//! a per-glyph offset array (short or long, gated by the header
//! `flags` bit 0). The store layout is shared with `cvar` (and the
//! generic TupleVariationStore in OpenType).
//!
//! Header layout:
//!
//! ```text
//!   0  / 2  / majorVersion             (1)
//!   2  / 2  / minorVersion             (0)
//!   4  / 2  / axisCount
//!   6  / 2  / sharedTupleCount
//!   8  / 4  / sharedTuplesOffset       (relative to gvar start)
//!  12  / 2  / glyphCount
//!  14  / 2  / flags                    (bit 0 = long offsets)
//!  16  / 4  / glyphVariationDataArrayOffset
//!  20  / .. / glyphVariationDataOffsets[glyphCount + 1]
//! ```
//!
//! Each `GlyphVariationData` block (one per glyph; possibly empty):
//!
//! ```text
//!   0 / 2 / tupleVariationCount
//!   2 / 2 / dataOffset                  (offset to packed-data area,
//!                                        relative to *this* block)
//!   4 / .. / TupleVariationHeader[tupleVariationCount]
//!   .. / .. / packed point + delta data (starts at dataOffset)
//! ```
//!
//! Each TupleVariationHeader:
//!
//! ```text
//!   0 / 2 / variationDataSize           (length of this tuple's packed
//!                                        data inside the data area)
//!   2 / 2 / tupleIndex                  (low 12 bits = shared tuple
//!                                        index when EMBEDDED_PEAK_TUPLE
//!                                        is unset; high 4 bits flag bits)
//!   4 / 2*axisCount / peakTuple         (only when EMBEDDED_PEAK_TUPLE)
//!   .. / 4*axisCount / intermediateStartTuple + intermediateEndTuple
//!                                        (only when INTERMEDIATE_REGION)
//! ```
//!
//! Tuple-index high-bit flags (mask `0xF000`):
//!
//! ```text
//!   0x8000 EMBEDDED_PEAK_TUPLE
//!   0x4000 INTERMEDIATE_REGION
//!   0x2000 PRIVATE_POINT_NUMBERS
//!   (0x1000 reserved)
//! ```
//!
//! Inside each tuple's packed-data block:
//!
//! ```text
//!   - When PRIVATE_POINT_NUMBERS: packed point-number set, then
//!     packed deltas for x, then packed deltas for y.
//!   - Else: shared point-number set (lives at the start of the
//!     glyph's data area, before the first tuple's data) is used.
//!   - All-points sentinel: the packed point set begins with byte 0
//!     and we synthesise indices 0..numPoints (where numPoints is
//!     the static glyph's point count + the four phantom points).
//! ```
//!
//! Packed point numbers (OpenType §"Packed Point Numbers"):
//!
//! ```text
//!   - `00 00`           — zero points: legal-but-unusual all-points sentinel.
//!   - first byte n      — n-1 points if n & 0x80 == 0; n is 1..127.
//!   - first byte n      — high bit set: count = ((n & 0x7F) << 8) | next_byte
//!                         (so up to 32767 points).
//!   - then a stream of run controls:
//!       - control byte `c`:
//!           - bit 7: 0 ⇒ each point delta is u8;  1 ⇒ each delta is u16
//!           - low 7 bits = (run length - 1)
//!         followed by (control_low+1) deltas; first point is the absolute
//!         index of the first delta, subsequent points are running sums.
//! ```
//!
//! Packed deltas (per the same module):
//!
//! ```text
//!   control byte `c`:
//!     - bit 7: DELTAS_ARE_ZERO   — emit (low7 + 1) zeros.
//!     - bit 6: DELTAS_ARE_WORDS  — when DELTAS_ARE_ZERO is unset and
//!                                  this bit is set, each delta is i16.
//!     - low 6 bits: (run length - 1).
//!   When neither bit is set, each delta is i8.
//! ```
//!
//! We expose two entry points:
//!  - [`GvarTable::parse`] — validate header, hold the raw bytes for
//!    lazy per-glyph decoding.
//!  - [`GvarTable::glyph_deltas`] — given a glyph id, point count, and
//!    a normalised coord vector, return per-point `(dx, dy)` deltas
//!    (in font units) to add to the static outline.

use crate::parser::{read_i16, read_i8, read_u16, read_u32, read_u8};
use crate::Error;

/// Sanity cap on a single glyph's tuple-variation count.
const MAX_TUPLES_PER_GLYPH: u16 = 4096;
/// Sanity cap on points-per-glyph (OpenType field-width is u16; real
/// fonts top out around 1500 points). We also include the 4 phantom
/// points: leftSideBearing, rightSideBearing, top, bottom.
const MAX_POINTS_PER_GLYPH: usize = 0xFFFF;

/// Header bit on `gvar.flags` — when set, per-glyph data offsets are
/// u32; when unset, they are u16-÷-2 (the "short" encoding).
const FLAG_LONG_OFFSETS: u16 = 0x0001;

/// Tuple-index high-byte flags (mask `0xF000`).
const TI_EMBEDDED_PEAK: u16 = 0x8000;
const TI_INTERMEDIATE: u16 = 0x4000;
const TI_PRIVATE_POINTS: u16 = 0x2000;
const TI_TUPLE_INDEX_MASK: u16 = 0x0FFF;

#[derive(Debug, Clone)]
pub struct GvarTable<'a> {
    bytes: &'a [u8],
    axis_count: u16,
    shared_tuple_count: u16,
    shared_tuples_offset: usize,
    glyph_count: u16,
    /// Cached file offsets of each glyph's variation block, relative
    /// to `glyph_data_array_offset`. `[g]` start, `[g+1]` end.
    offsets: Vec<u32>,
    glyph_data_array_offset: usize,
}

impl<'a> GvarTable<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < 20 {
            return Err(Error::UnexpectedEof);
        }
        let major = read_u16(bytes, 0)?;
        if major != 1 {
            return Err(Error::BadStructure("gvar version not 1.x"));
        }
        let axis_count = read_u16(bytes, 4)?;
        let shared_tuple_count = read_u16(bytes, 6)?;
        let shared_tuples_offset = read_u32(bytes, 8)? as usize;
        let glyph_count = read_u16(bytes, 12)?;
        let flags = read_u16(bytes, 14)?;
        let long_offsets = flags & FLAG_LONG_OFFSETS != 0;
        let glyph_data_array_offset = read_u32(bytes, 16)? as usize;

        // Read per-glyph offset array.
        let entry = if long_offsets { 4 } else { 2 };
        let off_array_start = 20usize;
        let off_array_end = off_array_start
            .checked_add(entry * (glyph_count as usize + 1))
            .ok_or(Error::BadOffset)?;
        if bytes.len() < off_array_end {
            return Err(Error::UnexpectedEof);
        }
        let mut offsets = Vec::with_capacity(glyph_count as usize + 1);
        for i in 0..=glyph_count as usize {
            let off = off_array_start + i * entry;
            let v = if long_offsets {
                read_u32(bytes, off)?
            } else {
                read_u16(bytes, off)? as u32 * 2
            };
            offsets.push(v);
        }

        Ok(Self {
            bytes,
            axis_count,
            shared_tuple_count,
            shared_tuples_offset,
            glyph_count,
            offsets,
            glyph_data_array_offset,
        })
    }

    pub fn axis_count(&self) -> u16 {
        self.axis_count
    }

    pub fn glyph_count(&self) -> u16 {
        self.glyph_count
    }

    /// Read shared peak tuple `i` (length `axis_count`, F2DOT14 each).
    fn shared_tuple(&self, i: u16) -> Result<Vec<f32>, Error> {
        if i >= self.shared_tuple_count {
            return Err(Error::BadStructure("gvar shared tuple index out of range"));
        }
        let stride = self.axis_count as usize * 2;
        let off = self
            .shared_tuples_offset
            .checked_add(i as usize * stride)
            .ok_or(Error::BadOffset)?;
        if off + stride > self.bytes.len() {
            return Err(Error::UnexpectedEof);
        }
        let mut t = Vec::with_capacity(self.axis_count as usize);
        for ai in 0..self.axis_count as usize {
            t.push(f2dot14(read_i16(self.bytes, off + ai * 2)?));
        }
        Ok(t)
    }

    /// Decode the per-point `(dx, dy)` delta vectors for `glyph_id`
    /// at the normalised coordinate `coords` (length must equal
    /// [`axis_count`](Self::axis_count); each value in `[-1, +1]`).
    /// `num_points` is the static glyph's contour-point count
    /// **excluding** the four trailing phantom points (the gvar
    /// stream operates on `num_points + 4` points internally — we
    /// truncate the result to `num_points` for the caller).
    ///
    /// Returns `Ok(vec![(0,0); num_points])` for a glyph with no
    /// variation data (or for an all-zero coord request).
    ///
    /// This convenience entry point performs **no** inferred-delta
    /// (IUP) interpolation: tuples that reference only a subset of the
    /// glyph's points leave the un-referenced points unmoved. That is
    /// correct only for fonts whose every tuple lists all points. For
    /// the general case (partial point sets, which most real variable
    /// fonts use), call [`Self::glyph_deltas_iup`] with the glyph's
    /// contour structure and default coordinates so un-referenced
    /// points get inferred per ISO/IEC 14496-22:2019 §7.3.4.4
    /// ("Inferred deltas for un-referenced point numbers").
    pub fn glyph_deltas(
        &self,
        glyph_id: u16,
        num_points: u16,
        coords: &[f32],
    ) -> Result<Vec<(i32, i32)>, Error> {
        let np = num_points as usize;
        // The gvar stream addresses `np + 4` points (the four trailing
        // phantom points: lsb, rsb, tsb, bsb). We decode the full set
        // and hand the caller back only the `np` outline points.
        let total = np.checked_add(4).ok_or(Error::BadOffset)?;
        let mut full = self.decode_deltas(glyph_id, total, coords, None)?;
        full.truncate(np);
        Ok(full)
    }

    /// Decode the per-point `(dx, dy)` delta vectors for a **simple**
    /// `glyph_id`, performing inferred-delta (IUP) interpolation for
    /// any outline points a tuple omits from its packed point-number
    /// set (ISO/IEC 14496-22:2019 §7.3.4.4).
    ///
    /// `outline` describes the static glyph's contour structure
    /// (the per-contour last-point indices) and the default grid
    /// coordinates of every outline point — both are required because
    /// the inference of an un-referenced point's delta is computed
    /// from the *unscaled* region deltas of its nearest referenced
    /// contour neighbours and the **default** coordinates of all three
    /// points. Phantom points (the trailing four lsb/rsb/tsb/bsb
    /// pseudo-points) are never inferred — they are handled as plain
    /// referenced/un-referenced points and contribute zero when a
    /// tuple omits them.
    ///
    /// The returned vector has length `outline.points.len()` (outline
    /// points only — the four phantom-point deltas are dropped, like
    /// [`Self::glyph_deltas`]).
    ///
    /// Inference is done **per region** on each tuple's unscaled
    /// deltas, then the tuple scalar is applied and the result
    /// accumulated, exactly as the spec prescribes: "calculation of
    /// inferred variation deltas is based on the default positions of
    /// points and the unscaled delta values for a given region … the
    /// inferred deltas can be pre-computed before any processing for a
    /// specific instance is done."
    pub fn glyph_deltas_iup(
        &self,
        glyph_id: u16,
        outline: &SimpleOutlineInfo,
        coords: &[f32],
    ) -> Result<Vec<(i32, i32)>, Error> {
        let np = outline.points.len();
        let total = np.checked_add(4).ok_or(Error::BadOffset)?;
        let mut full = self.decode_deltas(glyph_id, total, coords, Some(outline))?;
        full.truncate(np);
        Ok(full)
    }

    /// Decode the per-**component** `(dx, dy)` placement deltas for a
    /// composite `glyph_id` at the normalised coordinate `coords`
    /// (ISO/IEC 14496-22:2019 §7.3.4.3 "Point Numbers and processing
    /// for composite glyphs").
    ///
    /// For a composite glyph the gvar packed point numbers do **not**
    /// address outline points: pseudo-point `i` (for `i` in
    /// `0..num_components`) is the *i*-th component's placement, and
    /// the four trailing pseudo-points (indices `num_components` through
    /// `num_components + 3`) are the lsb / rsb / tsb / bsb phantom
    /// points. This routine
    /// returns the interpolated `(dx, dy)` to add to each component's
    /// `argument1` / `argument2` X / Y placement offset; the caller
    /// applies them only to components whose `ARGS_ARE_XY_VALUES` flag
    /// is set (point-matched components take no delta per §7.3.4.3).
    ///
    /// Per §7.3.4.4's NOTE, inferred (IUP-style) deltas apply only to
    /// simple glyphs — never to composites — so this path never
    /// interpolates un-referenced pseudo-points; an omitted component
    /// simply keeps its default placement, which falls out of the
    /// zero-initialised accumulator.
    ///
    /// Returns `Ok(vec![(0,0); num_components])` for a glyph with no
    /// variation data or an all-default coord request.
    pub fn glyph_component_deltas(
        &self,
        glyph_id: u16,
        num_components: u16,
        coords: &[f32],
    ) -> Result<Vec<(i32, i32)>, Error> {
        let nc = num_components as usize;
        let total = nc.checked_add(4).ok_or(Error::BadOffset)?;
        let mut full = self.decode_deltas(glyph_id, total, coords, None)?;
        full.truncate(nc);
        Ok(full)
    }

    /// Shared TupleVariationStore decoder. `total_points` is the full
    /// pseudo-point count the gvar stream for this glyph addresses
    /// (outline points + 4 for simple glyphs; component count + 4 for
    /// composites). Returns a `(dx, dy)` vector of length
    /// `total_points`.
    ///
    /// When `outline` is `Some`, per-region inferred-delta (IUP)
    /// interpolation is performed for outline points a tuple omits
    /// (§7.3.4.4): each region's unscaled deltas are completed across
    /// every contour before the tuple scalar is applied. When `outline`
    /// is `None`, un-referenced points keep a zero region delta (the
    /// legacy [`Self::glyph_deltas`] / composite behaviour). The four
    /// trailing phantom points are never inferred regardless.
    fn decode_deltas(
        &self,
        glyph_id: u16,
        total_points: usize,
        coords: &[f32],
        outline: Option<&SimpleOutlineInfo>,
    ) -> Result<Vec<(i32, i32)>, Error> {
        if total_points > MAX_POINTS_PER_GLYPH {
            return Err(Error::BadStructure("gvar point count exceeds cap"));
        }
        let mut out = vec![(0i32, 0i32); total_points];

        if glyph_id >= self.glyph_count {
            return Err(Error::GlyphOutOfRange(glyph_id));
        }
        if coords.len() != self.axis_count as usize {
            return Err(Error::BadStructure(
                "gvar coord vector length != fvar axis count",
            ));
        }

        let start = self.offsets[glyph_id as usize] as usize;
        let end = self.offsets[glyph_id as usize + 1] as usize;
        if end <= start {
            return Ok(out);
        }
        let block_off = self
            .glyph_data_array_offset
            .checked_add(start)
            .ok_or(Error::BadOffset)?;
        let block_len = end - start;
        if block_off + block_len > self.bytes.len() {
            return Err(Error::UnexpectedEof);
        }
        let block = &self.bytes[block_off..block_off + block_len];
        if block.len() < 4 {
            return Ok(out);
        }
        let tuple_count = read_u16(block, 0)?;
        let n_tuples = tuple_count & 0x0FFF; // top 4 bits are flags
        if n_tuples > MAX_TUPLES_PER_GLYPH {
            return Err(Error::BadStructure("gvar tupleVariationCount > cap"));
        }
        let data_offset = read_u16(block, 2)? as usize;
        if data_offset > block.len() {
            return Err(Error::BadOffset);
        }

        // Walk the tuple-variation headers.
        let mut hdr_off = 4usize;
        // Per-glyph data area starts here. The optional shared
        // point-number set lives at the very start of it.
        let mut data_cursor = data_offset;

        // Decode the shared point-number set that lives at the very
        // top of the data area. Per OpenType §"GlyphVariationData
        // table" the shared set is **always** present (a length-zero
        // set is encoded as a single 0x00 byte → all-points) when
        // any tuple lacks PRIVATE_POINT_NUMBERS. We decode
        // unconditionally; tuples that supply private sets simply
        // skip past it via their own `variationDataSize`.
        let shared_points: Option<Vec<u16>> = if data_offset < block.len() {
            let shared_slice = &block[data_offset..];
            let (pts, used) = decode_packed_points(shared_slice, total_points as u16)?;
            data_cursor = data_offset + used;
            Some(pts)
        } else {
            None
        };

        for _ in 0..n_tuples {
            if hdr_off + 4 > block.len() {
                return Err(Error::BadStructure("gvar tuple header truncated"));
            }
            let var_data_size = read_u16(block, hdr_off)? as usize;
            let tuple_index = read_u16(block, hdr_off + 2)?;
            hdr_off += 4;
            // Optional embedded peak tuple.
            let peak = if tuple_index & TI_EMBEDDED_PEAK != 0 {
                let need = self.axis_count as usize * 2;
                if hdr_off + need > block.len() {
                    return Err(Error::BadStructure("gvar embedded peak truncated"));
                }
                let mut p = Vec::with_capacity(self.axis_count as usize);
                for ai in 0..self.axis_count as usize {
                    p.push(f2dot14(read_i16(block, hdr_off + ai * 2)?));
                }
                hdr_off += need;
                p
            } else {
                let idx = tuple_index & TI_TUPLE_INDEX_MASK;
                self.shared_tuple(idx)?
            };
            // Optional intermediate region.
            let (start_t, end_t) = if tuple_index & TI_INTERMEDIATE != 0 {
                let need = self.axis_count as usize * 4;
                if hdr_off + need > block.len() {
                    return Err(Error::BadStructure("gvar intermediate region truncated"));
                }
                let mut s = Vec::with_capacity(self.axis_count as usize);
                let mut e = Vec::with_capacity(self.axis_count as usize);
                for ai in 0..self.axis_count as usize {
                    s.push(f2dot14(read_i16(block, hdr_off + ai * 2)?));
                }
                for ai in 0..self.axis_count as usize {
                    e.push(f2dot14(read_i16(
                        block,
                        hdr_off + self.axis_count as usize * 2 + ai * 2,
                    )?));
                }
                hdr_off += need;
                (Some(s), Some(e))
            } else {
                (None, None)
            };

            // Compute scalar weight for this tuple given current coords.
            let scalar = tuple_scalar(coords, &peak, start_t.as_deref(), end_t.as_deref());

            // Locate this tuple's packed data inside the data area.
            if data_cursor + var_data_size > block.len() {
                return Err(Error::BadStructure("gvar tuple data overruns"));
            }
            let tuple_data = &block[data_cursor..data_cursor + var_data_size];
            data_cursor += var_data_size;

            // Skip cheap when scalar == 0 (this region doesn't apply).
            if scalar == 0.0 {
                continue;
            }

            // Decode this tuple's deltas.
            let mut td_off = 0usize;
            let points = if tuple_index & TI_PRIVATE_POINTS != 0 {
                let (pts, used) = decode_packed_points(tuple_data, total_points as u16)?;
                td_off += used;
                pts
            } else {
                shared_points.clone().unwrap_or_else(|| {
                    // No shared set was decoded — treat as all-points.
                    (0..total_points as u16).collect()
                })
            };
            let n_pts = points.len();
            // Decode dx[], then dy[].
            let dxs = decode_packed_deltas(tuple_data, &mut td_off, n_pts)?;
            let dys = decode_packed_deltas(tuple_data, &mut td_off, n_pts)?;

            // Whether this tuple references *all* of the glyph's
            // points (the "all points" sentinel, or an explicit set
            // covering every point). When it does, no inference is
            // needed regardless of `outline`.
            let all_points = n_pts == total_points;

            if let (Some(info), false) = (outline, all_points) {
                // §7.3.4.4 inferred-delta path. Build the full unscaled
                // (dx, dy) vector for every point in `total_points`,
                // inferring outline points the tuple omits on a
                // per-contour basis from the *default* coordinates, then
                // scale and accumulate.
                let region = infer_region_deltas(info, total_points, &points, &dxs, &dys);
                for (pi, (rdx, rdy)) in region.into_iter().enumerate() {
                    if rdx == 0.0 && rdy == 0.0 {
                        continue;
                    }
                    out[pi].0 += (rdx * scalar as f64).round() as i32;
                    out[pi].1 += (rdy * scalar as f64).round() as i32;
                }
            } else {
                // Legacy / composite / all-points path: apply scaled
                // deltas only to referenced points. Out-of-range point
                // indices (a corrupt point set addressing past the
                // declared pseudo-point count) are dropped.
                for (i, &p_idx) in points.iter().enumerate() {
                    let pi = p_idx as usize;
                    if pi >= total_points {
                        continue;
                    }
                    let dx = (dxs[i] as f32 * scalar).round() as i32;
                    let dy = (dys[i] as f32 * scalar).round() as i32;
                    out[pi].0 += dx;
                    out[pi].1 += dy;
                }
            }
        }
        Ok(out)
    }
}

/// Static contour structure of a **simple** glyph, used to drive
/// inferred-delta (IUP) interpolation in [`GvarTable::glyph_deltas_iup`].
///
/// `points` carries the default grid coordinates `(x, y)` of every
/// outline point of the glyph **in gvar point-number order** (which is
/// the contour-concatenated order: contour 0's points, then contour 1's,
/// …). `contour_ends` carries the *last* point index (inclusive) of
/// each contour, i.e. the `endPtsOfContours[]` values from the `glyf`
/// simple-glyph header. The phantom points are **not** included here —
/// the gvar layer appends the four trailing pseudo-points itself and
/// never infers them.
#[derive(Debug, Clone)]
pub struct SimpleOutlineInfo {
    /// Default `(x, y)` of each outline point, in point-number order.
    pub points: Vec<(i32, i32)>,
    /// Inclusive last-point index of each contour (`endPtsOfContours`).
    pub contour_ends: Vec<u16>,
}

impl SimpleOutlineInfo {
    /// Build outline info from per-contour point coordinate lists, the
    /// shape [`crate::tables::glyf`] produces. Each inner `Vec` is one
    /// contour's points (in order); the concatenation is the gvar
    /// point-number order and the running lengths give `contour_ends`.
    pub fn from_contours(contours: &[Vec<(i32, i32)>]) -> Self {
        let mut points = Vec::new();
        let mut contour_ends = Vec::with_capacity(contours.len());
        for c in contours {
            points.extend_from_slice(c);
            // endPtsOfContours is the *last* index, so length-1 after
            // appending. Empty contours are skipped (no valid end).
            if !points.is_empty() {
                contour_ends.push((points.len() - 1) as u16);
            }
        }
        Self {
            points,
            contour_ends,
        }
    }
}

/// Build the full unscaled `(dx, dy)` region delta vector of length
/// `total_points` (outline points + 4 phantom points) for one tuple
/// region, inferring outline points the tuple omits per §7.3.4.4.
///
/// `referenced` is the ascending point-number set the tuple supplied;
/// `dxs` / `dys` are its parallel unscaled deltas. Phantom points
/// (indices `>= info.points.len()`) are filled in directly from the
/// referenced set and never inferred.
fn infer_region_deltas(
    info: &SimpleOutlineInfo,
    total_points: usize,
    referenced: &[u16],
    dxs: &[i32],
    dys: &[i32],
) -> Vec<(f64, f64)> {
    let np = info.points.len();
    let mut delta = vec![(0f64, 0f64); total_points];
    // `has` marks whether a point carries an explicit (referenced) delta.
    let mut has = vec![false; total_points];
    for (i, &p) in referenced.iter().enumerate() {
        let pi = p as usize;
        if pi < total_points {
            delta[pi] = (dxs[i] as f64, dys[i] as f64);
            has[pi] = true;
        }
    }

    // Inference is contour-by-contour over outline points only.
    let mut contour_start = 0usize;
    for &end in &info.contour_ends {
        let end = end as usize;
        if end >= np || contour_start > end {
            // Defensive: malformed contour bounds — skip.
            contour_start = end + 1;
            continue;
        }
        infer_contour(&mut delta, &has, &info.points, contour_start, end);
        contour_start = end + 1;
    }
    delta
}

/// Infer un-referenced points within one contour spanning the inclusive
/// index range `[start, end]`, following ISO/IEC 14496-22:2019 §7.3.4.4.
fn infer_contour(
    delta: &mut [(f64, f64)],
    has: &[bool],
    points: &[(i32, i32)],
    start: usize,
    end: usize,
) {
    let n = end - start + 1;
    // Collect contour-local indices that are referenced (in ascending
    // point-number order, which equals ascending contour order).
    let referenced: Vec<usize> = (start..=end).filter(|&i| has[i]).collect();

    if referenced.is_empty() {
        // No point in this contour is referenced — nothing to infer
        // (the whole contour keeps a zero region delta).
        return;
    }
    if referenced.len() == n {
        // Every point already has an explicit delta — nothing to infer.
        return;
    }
    if referenced.len() == 1 {
        // §7.3.4.4 NOTE: a single referenced point makes every other
        // point in the contour adopt that point's delta verbatim.
        let r = referenced[0];
        let (dx, dy) = delta[r];
        for i in start..=end {
            if !has[i] {
                delta[i] = (dx, dy);
            }
        }
        return;
    }

    // General case: for each un-referenced point find the nearest
    // referenced points before and after it *in contour order*,
    // wrapping around the contour, then infer X and Y independently.
    let m = referenced.len();
    for ri in 0..m {
        let cur = referenced[ri];
        let nxt = referenced[(ri + 1) % m];
        // Walk the gap of un-referenced points strictly between `cur`
        // and `nxt`, wrapping past `end` back to `start` when needed.
        let mut t = next_in_contour(cur, start, end);
        while t != nxt {
            // `t` is un-referenced (it lies strictly between two
            // adjacent referenced points). Infer X then Y.
            let dx = infer_axis(
                points[t].0,
                points[cur].0,
                points[nxt].0,
                delta[cur].0,
                delta[nxt].0,
            );
            let dy = infer_axis(
                points[t].1,
                points[cur].1,
                points[nxt].1,
                delta[cur].1,
                delta[nxt].1,
            );
            delta[t] = (dx, dy);
            t = next_in_contour(t, start, end);
        }
    }
}

/// Next index within a contour `[start, end]`, wrapping `end → start`.
#[inline]
fn next_in_contour(i: usize, start: usize, end: usize) -> usize {
    if i >= end {
        start
    } else {
        i + 1
    }
}

/// Infer one axis's delta for a target point lying (in contour order)
/// between two referenced points `prec` (preceding) and `foll`
/// (following), given their default grid coordinates and unscaled
/// region deltas. Implements the §7.3.4.4 pseudo-code.
fn infer_axis(
    target_coord: i32,
    prec_coord: i32,
    foll_coord: i32,
    prec_delta: f64,
    foll_delta: f64,
) -> f64 {
    if prec_coord == foll_coord {
        // Same default coordinate: identical deltas propagate, differing
        // deltas yield zero.
        if prec_delta == foll_delta {
            prec_delta
        } else {
            0.0
        }
    } else {
        let (min_c, max_c) = if prec_coord < foll_coord {
            (prec_coord, foll_coord)
        } else {
            (foll_coord, prec_coord)
        };
        if target_coord <= min_c {
            // At/below the lower coordinate → take the delta of whichever
            // adjacent point has the lower coordinate.
            if prec_coord < foll_coord {
                prec_delta
            } else {
                foll_delta
            }
        } else if target_coord >= max_c {
            // At/above the higher coordinate → take the delta of whichever
            // adjacent point has the higher coordinate.
            if prec_coord > foll_coord {
                prec_delta
            } else {
                foll_delta
            }
        } else {
            // Strictly between: linear interpolation of the deltas by the
            // target's proportional position between the two coordinates.
            // proportion = (target - ref) / (comp - ref); deltaTarget =
            // deltaRef + proportion * (deltaComp - deltaRef). Reference =
            // preceding point, comparison = following point. The
            // fractional result is preserved (rounded only once at final
            // accumulation), matching the spec worked example's +10.5.
            let proportion = (target_coord - prec_coord) as f64 / (foll_coord - prec_coord) as f64;
            prec_delta + proportion * (foll_delta - prec_delta)
        }
    }
}

#[inline]
fn f2dot14(raw: i16) -> f32 {
    raw as f32 / 16384.0
}

/// Compute the tuple's scalar weight per the OpenType "Tuple variations
/// scalar" rule: zero outside the box, peak-product inside; if an
/// intermediate region is present the linear ramp goes (start → peak)
/// then (peak → end), else the ramp is (0 → peak) clamped at peak's
/// sign.
fn tuple_scalar(coords: &[f32], peak: &[f32], start: Option<&[f32]>, end: Option<&[f32]>) -> f32 {
    let mut s = 1.0f32;
    for (ai, &c) in coords.iter().enumerate() {
        let p = peak.get(ai).copied().unwrap_or(0.0);
        if p == 0.0 {
            // Axis doesn't participate — multiplier is 1 (the rule
            // is "if peak is 0, the scalar contribution for this axis
            // is 1").
            continue;
        }
        if c == p {
            // Exact peak — multiplier 1, continue.
            continue;
        }
        // Out-of-sign: c and p disagree, multiplier 0.
        if (c < 0.0) != (p < 0.0) && c != 0.0 {
            return 0.0;
        }
        match (start, end) {
            (Some(st), Some(en)) => {
                let s_v = st.get(ai).copied().unwrap_or(0.0);
                let e_v = en.get(ai).copied().unwrap_or(0.0);
                if c < s_v || c > e_v {
                    return 0.0;
                }
                if c < p {
                    if (p - s_v).abs() < f32::EPSILON {
                        // Degenerate; treat as multiplier 1 at peak,
                        // 0 elsewhere — but c != p was handled above.
                        return 0.0;
                    }
                    s *= (c - s_v) / (p - s_v);
                } else {
                    if (e_v - p).abs() < f32::EPSILON {
                        return 0.0;
                    }
                    s *= (e_v - c) / (e_v - p);
                }
            }
            _ => {
                // Default region. Rule: if peak is positive, the
                // valid range is [0, peak] (linear ramp), elsewhere
                // multiplier is 0 (above peak) or peak itself
                // (interpolate between 0 at coord=0 and 1 at coord=peak).
                if c.abs() > p.abs() {
                    return 0.0;
                }
                if p.abs() < f32::EPSILON {
                    return 0.0;
                }
                s *= c / p;
            }
        }
    }
    s
}

/// Decode a packed point-number set into an ascending list of point
/// indices. The "all points" sentinel (first byte == 0) returns
/// `0..total_points`. Returns the decoded set + the number of bytes
/// consumed from `bytes`.
pub(crate) fn decode_packed_points(
    bytes: &[u8],
    total_points: u16,
) -> Result<(Vec<u16>, usize), Error> {
    if bytes.is_empty() {
        return Err(Error::BadStructure("gvar packed points truncated"));
    }
    let mut off = 0usize;
    let first = read_u8(bytes, off)?;
    off += 1;
    // All-points sentinel: first byte 0 → return 0..total_points.
    if first == 0 {
        return Ok(((0..total_points).collect(), off));
    }
    let count = if first & 0x80 != 0 {
        let lo = read_u8(bytes, off)? as u16;
        off += 1;
        ((first & 0x7F) as u16) << 8 | lo
    } else {
        first as u16
    };
    let mut out = Vec::with_capacity(count as usize);
    let mut last: u16 = 0;
    while (out.len() as u16) < count {
        let ctrl = read_u8(bytes, off)?;
        off += 1;
        let words = ctrl & 0x80 != 0;
        let run = (ctrl & 0x7F) as u16 + 1;
        for _ in 0..run {
            if (out.len() as u16) >= count {
                break;
            }
            let delta = if words {
                let v = read_u16(bytes, off)?;
                off += 2;
                v
            } else {
                let v = read_u8(bytes, off)? as u16;
                off += 1;
                v
            };
            last = last
                .checked_add(delta)
                .ok_or(Error::BadStructure("gvar packed point overflow"))?;
            out.push(last);
        }
    }
    Ok((out, off))
}

/// Decode `n` packed delta values starting at `*off` inside `bytes`.
/// Advances `*off`. Each delta is an i32 (room for the runs we then
/// scale to f32).
pub(crate) fn decode_packed_deltas(
    bytes: &[u8],
    off: &mut usize,
    n: usize,
) -> Result<Vec<i32>, Error> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        if *off >= bytes.len() {
            return Err(Error::BadStructure("gvar packed deltas truncated"));
        }
        let ctrl = read_u8(bytes, *off)?;
        *off += 1;
        let zeros = ctrl & 0x80 != 0;
        let words = ctrl & 0x40 != 0;
        let run = (ctrl & 0x3F) as usize + 1;
        for _ in 0..run {
            if out.len() >= n {
                break;
            }
            if zeros {
                out.push(0);
            } else if words {
                let v = read_i16(bytes, *off)? as i32;
                *off += 2;
                out.push(v);
            } else {
                let v = read_i8(bytes, *off)? as i32;
                *off += 1;
                out.push(v);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal gvar: 1 axis, 0 shared tuples, 1 glyph with
    /// **no** variation data (zero-length offset block).
    fn build_empty_one_glyph() -> Vec<u8> {
        // Header (20 bytes) + offsets[2] (4 bytes for short = 4) + no data
        let mut b = vec![0u8; 20 + 4];
        b[0..2].copy_from_slice(&1u16.to_be_bytes()); // major
        b[4..6].copy_from_slice(&1u16.to_be_bytes()); // axisCount
                                                      // sharedTupleCount 0, sharedTuplesOffset 0
        b[12..14].copy_from_slice(&1u16.to_be_bytes()); // glyphCount
                                                        // flags = 0 (short offsets)
        b[16..20].copy_from_slice(&24u32.to_be_bytes()); // glyphVariationDataArrayOffset
                                                         // offsets[0] = 0, offsets[1] = 0 (already zero)
        b
    }

    #[test]
    fn gvar_zero_coords_yields_static_outline() {
        // A glyph with no variation data must return all-zero deltas
        // regardless of the requested coord vector.
        let raw = build_empty_one_glyph();
        let g = GvarTable::parse(&raw).expect("parse");
        let deltas = g.glyph_deltas(0, 5, &[0.5]).expect("deltas");
        assert_eq!(deltas.len(), 5);
        assert!(deltas.iter().all(|&(x, y)| x == 0 && y == 0));
        // Default-position request also yields zero.
        let deltas0 = g.glyph_deltas(0, 5, &[0.0]).expect("deltas");
        assert!(deltas0.iter().all(|&(x, y)| x == 0 && y == 0));
    }

    #[test]
    fn gvar_packed_points_all_sentinel() {
        let (pts, used) = decode_packed_points(&[0x00, 0xff, 0xff], 5).unwrap();
        assert_eq!(pts, vec![0, 1, 2, 3, 4]);
        assert_eq!(used, 1);
    }

    #[test]
    fn gvar_packed_points_short_run() {
        // 3 points, run of 3, byte deltas 1, 1, 1 → indices 1, 2, 3.
        // first = 3 (count==3), control = 0x02 (run=3, words=0),
        // deltas = 1, 1, 1.
        let raw = [3u8, 0x02, 1, 1, 1];
        let (pts, used) = decode_packed_points(&raw, 100).unwrap();
        assert_eq!(pts, vec![1, 2, 3]);
        assert_eq!(used, 5);
    }

    #[test]
    fn gvar_packed_deltas_words_then_zeros() {
        // 2 word deltas (10, -3), then 3 zeros.
        // ctrl = 0x41 (words=1, zero=0, run=2), word values, ctrl =
        // 0x82 (zero=1, run=3).
        let mut raw = vec![0x41u8];
        raw.extend_from_slice(&10i16.to_be_bytes());
        raw.extend_from_slice(&(-3i16).to_be_bytes());
        raw.push(0x82);
        let mut off = 0usize;
        let d = decode_packed_deltas(&raw, &mut off, 5).unwrap();
        assert_eq!(d, vec![10, -3, 0, 0, 0]);
        assert_eq!(off, 6);
    }

    #[test]
    fn gvar_packed_deltas_byte_run() {
        // 4 byte deltas: 1, -1, 2, -2.
        // ctrl = 0x03 (run=4, no flags) followed by four i8 bytes.
        let raw = [0x03u8, 1, 0xFF, 2, 0xFE];
        let mut off = 0usize;
        let d = decode_packed_deltas(&raw, &mut off, 4).unwrap();
        assert_eq!(d, vec![1, -1, 2, -2]);
    }

    #[test]
    fn gvar_tuple_scalar_at_peak_is_one() {
        let coords = [0.5];
        let peak = [0.5];
        assert!((tuple_scalar(&coords, &peak, None, None) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn gvar_tuple_scalar_at_zero_is_zero() {
        let coords = [0.0];
        let peak = [1.0];
        assert!(tuple_scalar(&coords, &peak, None, None).abs() < 1e-6);
    }

    #[test]
    fn gvar_tuple_scalar_default_region_linear() {
        // peak +1.0, coord +0.5 → 0.5
        assert!((tuple_scalar(&[0.5], &[1.0], None, None) - 0.5).abs() < 1e-6);
        // Negative-axis: peak -1, coord -0.25 → 0.25
        assert!((tuple_scalar(&[-0.25], &[-1.0], None, None) - 0.25).abs() < 1e-6);
        // Mixed sign → 0.
        assert_eq!(tuple_scalar(&[-0.5], &[1.0], None, None), 0.0);
    }

    #[test]
    fn gvar_tuple_scalar_intermediate_region() {
        // start=0, peak=1, end=2 — at coord 0.5 we should be 0.5.
        let s = tuple_scalar(&[0.5], &[1.0], Some(&[0.0]), Some(&[2.0]));
        assert!((s - 0.5).abs() < 1e-6);
        // At coord 1.5 (between peak and end) → linear from 1 to 0
        // over [1, 2] → 0.5.
        let s = tuple_scalar(&[1.5], &[1.0], Some(&[0.0]), Some(&[2.0]));
        assert!((s - 0.5).abs() < 1e-6);
        // Outside [start, end] → 0.
        let s = tuple_scalar(&[2.5], &[1.0], Some(&[0.0]), Some(&[2.0]));
        assert_eq!(s, 0.0);
    }

    /// Build a single-glyph gvar (1 axis, 0 shared tuples) holding one
    /// tuple with an embedded peak at +1.0, an all-points shared set,
    /// and the supplied per-point `dx` / `dy` word deltas. `total` is
    /// the number of pseudo-points the stream addresses (outline points
    /// or component count, **plus** the four phantom points).
    fn build_one_tuple_glyph(dxs: &[i16], dys: &[i16]) -> Vec<u8> {
        assert_eq!(dxs.len(), dys.len());
        let n = dxs.len();
        // --- per-glyph variation data block ---------------------------
        // shared point set: all-points sentinel (single 0x00 byte).
        let mut data_area = vec![0x00u8];
        // tuple data: dx[] then dy[] as word runs.
        // word run control: 0x40 | (n-1), each i16 BE.
        let mut tuple_data = Vec::new();
        tuple_data.push(0x40 | ((n - 1) as u8));
        for &d in dxs {
            tuple_data.extend_from_slice(&d.to_be_bytes());
        }
        tuple_data.push(0x40 | ((n - 1) as u8));
        for &d in dys {
            tuple_data.extend_from_slice(&d.to_be_bytes());
        }
        let var_data_size = tuple_data.len() as u16;
        data_area.extend_from_slice(&tuple_data);

        // GlyphVariationData header: tupleVariationCount=1, dataOffset.
        // header = 2 (count) + 2 (dataOffset) + tupleHeader(4 + 2 peak).
        let tuple_header_len = 4 + 2; // size + index + embedded peak (1 axis)
        let data_offset = (4 + tuple_header_len) as u16;
        let mut block = Vec::new();
        block.extend_from_slice(&1u16.to_be_bytes()); // tupleVariationCount
        block.extend_from_slice(&data_offset.to_be_bytes());
        // TupleVariationHeader: variationDataSize, tupleIndex (EMBEDDED_PEAK).
        block.extend_from_slice(&var_data_size.to_be_bytes());
        block.extend_from_slice(&TI_EMBEDDED_PEAK.to_be_bytes());
        // embedded peak tuple: +1.0 in F2DOT14 = 0x4000.
        block.extend_from_slice(&0x4000i16.to_be_bytes());
        block.extend_from_slice(&data_area);

        // --- gvar header (20) + offsets[2] (short, /2) ----------------
        let mut b = vec![0u8; 20];
        b[0..2].copy_from_slice(&1u16.to_be_bytes()); // major
        b[4..6].copy_from_slice(&1u16.to_be_bytes()); // axisCount
        b[12..14].copy_from_slice(&1u16.to_be_bytes()); // glyphCount
                                                        // flags = 0 (short offsets)
        let data_array_off = 24u32; // header(20) + offsets[2]*2 = 24
        b[16..20].copy_from_slice(&data_array_off.to_be_bytes());
        // offsets[0] = 0 (÷2), offsets[1] = block.len()/2. Block length
        // must be even for the short encoding; pad if odd.
        if block.len() % 2 != 0 {
            block.push(0);
        }
        let end_half = (block.len() / 2) as u16;
        b.extend_from_slice(&0u16.to_be_bytes()); // offsets[0]
        b.extend_from_slice(&end_half.to_be_bytes()); // offsets[1]
        b.extend_from_slice(&block);
        b
    }

    #[test]
    fn gvar_glyph_deltas_at_peak_returns_outline_deltas() {
        // 3 outline points + 4 phantom = 7 pseudo-points.
        let dxs = [10, 20, 30, 0, 0, 0, 0];
        let dys = [-1, -2, -3, 0, 0, 0, 0];
        let raw = build_one_tuple_glyph(&dxs, &dys);
        let g = GvarTable::parse(&raw).expect("parse");
        // At peak (+1.0) the deltas pass through unscaled; the 4 phantom
        // points are truncated away, leaving the 3 outline points.
        let out = g.glyph_deltas(0, 3, &[1.0]).expect("deltas");
        assert_eq!(out, vec![(10, -1), (20, -2), (30, -3)]);
        // Half-way (+0.5) scales linearly.
        let half = g.glyph_deltas(0, 3, &[0.5]).expect("deltas");
        assert_eq!(half, vec![(5, -1), (10, -1), (15, -2)]);
    }

    #[test]
    fn gvar_glyph_component_deltas_addresses_components_not_points() {
        // A composite with 2 components → pseudo-points 0,1 are the
        // component placements; 2..5 are the phantom points. The stream
        // addresses 2 + 4 = 6 pseudo-points.
        let dxs = [100, 200, 7, 7, 0, 0];
        let dys = [5, 6, 0, 0, 9, 9];
        let raw = build_one_tuple_glyph(&dxs, &dys);
        let g = GvarTable::parse(&raw).expect("parse");
        let comp = g.glyph_component_deltas(0, 2, &[1.0]).expect("comp deltas");
        // Only the first 2 (component) pseudo-points are returned; the
        // phantom-point deltas (indices 2..5) are dropped.
        assert_eq!(comp, vec![(100, 5), (200, 6)]);
        // Default position → no deltas.
        let comp0 = g.glyph_component_deltas(0, 2, &[0.0]).expect("comp deltas");
        assert_eq!(comp0, vec![(0, 0), (0, 0)]);
    }

    // ----- Inferred-delta (IUP) tests, §7.3.4.4 -----------------------

    #[test]
    fn infer_axis_same_coord_same_delta_propagates() {
        // precCoord == follCoord, equal deltas → that delta.
        assert_eq!(infer_axis(50, 10, 10, 7.0, 7.0), 7.0);
        // Equal coords, differing deltas → 0.
        assert_eq!(infer_axis(50, 10, 10, 7.0, -3.0), 0.0);
    }

    #[test]
    fn infer_axis_outside_takes_nearer_in_direction() {
        // prec.coord=20, foll.coord=80 (prec < foll).
        // target below min → delta of lower-coord point (prec).
        assert_eq!(infer_axis(5, 20, 80, 11.0, 99.0), 11.0);
        // target above max → delta of higher-coord point (foll).
        assert_eq!(infer_axis(200, 20, 80, 11.0, 99.0), 99.0);
        // Reversed ordering: prec.coord=80, foll.coord=20 (foll < prec).
        // target below min → lower-coord point is foll.
        assert_eq!(infer_axis(5, 80, 20, 11.0, 99.0), 99.0);
        // target above max → higher-coord point is prec.
        assert_eq!(infer_axis(200, 80, 20, 11.0, 99.0), 11.0);
    }

    #[test]
    fn infer_axis_between_linear_interpolates_spec_example() {
        // ISO/IEC 14496-22:2019 §7.3.4.4 worked example: P1 (ref) and
        // P3 (comp) referenced, P2 inferred. The X-direction proportion
        // is 0.25 with deltas +28 (P1) and -42 (P3) → +10.5.
        let dx = infer_axis(
            /*target*/ 0, /*prec=P1*/ 0, /*foll=P3*/ 4, 28.0, -42.0,
        );
        // proportion = (0-0)/(4-0)=0 → 28 here; pick a target giving 0.25.
        assert_eq!(dx, 28.0);
        // Now place the target a quarter of the way: prec.coord=0,
        // foll.coord=4, target.coord=1 → proportion 0.25 → 28 +
        // 0.25*(-42-28) = 28 - 17.5 = 10.5.
        let dx = infer_axis(1, 0, 4, 28.0, -42.0);
        assert!((dx - 10.5).abs() < 1e-9, "got {dx}");
    }

    #[test]
    fn simple_outline_info_from_contours_builds_ends() {
        // Two contours: 3 points then 2 points → ends [2, 4].
        let contours = vec![vec![(0, 0), (10, 0), (10, 10)], vec![(20, 20), (30, 20)]];
        let info = SimpleOutlineInfo::from_contours(&contours);
        assert_eq!(info.points.len(), 5);
        assert_eq!(info.contour_ends, vec![2, 4]);
    }

    #[test]
    fn infer_region_single_referenced_propagates_to_contour() {
        // One contour of 4 points; only point 1 is referenced with delta
        // (5, -9). Every other point in the contour adopts it; the four
        // phantom points (indices 4..7) stay zero.
        let info = SimpleOutlineInfo {
            points: vec![(0, 0), (10, 0), (10, 10), (0, 10)],
            contour_ends: vec![3],
        };
        let region = infer_region_deltas(&info, 8, &[1], &[5], &[-9]);
        for (i, d) in region.iter().enumerate().take(4) {
            assert_eq!(*d, (5.0, -9.0), "point {i}");
        }
        for d in region.iter().skip(4) {
            assert_eq!(*d, (0.0, 0.0));
        }
    }

    #[test]
    fn infer_region_unreferenced_contour_stays_zero() {
        // Two contours; only contour 1's point is referenced. Contour 0
        // has no referenced point → all its points stay zero.
        let info = SimpleOutlineInfo {
            points: vec![(0, 0), (10, 0), (50, 50), (60, 60)],
            contour_ends: vec![1, 3],
        };
        // Reference point index 2 (in contour 1) with delta (4, 4).
        let region = infer_region_deltas(&info, 8, &[2], &[4], &[4]);
        assert_eq!(region[0], (0.0, 0.0));
        assert_eq!(region[1], (0.0, 0.0));
        // Contour 1's single referenced point propagates to its neighbour.
        assert_eq!(region[2], (4.0, 4.0));
        assert_eq!(region[3], (4.0, 4.0));
    }

    #[test]
    fn infer_region_between_two_referenced_interpolates() {
        // One contour, 3 points P0,P1,P2. P0 and P2 referenced; P1
        // inferred. X: P0.x=0 (d=+28), P2.x=4 (d=-42), P1.x=1 →
        // proportion .25 → 10.5. Y: P0.y=0 (d=-62), P2.y=8 (d=-57),
        // P1.y=10 → above max → nearer (higher-coord = P2) delta -57.
        let info = SimpleOutlineInfo {
            points: vec![(0, 0), (1, 10), (4, 8)],
            contour_ends: vec![2],
        };
        // referenced = P0 (idx0) and P2 (idx2).
        let region = infer_region_deltas(&info, 7, &[0, 2], &[28, -42], &[-62, -57]);
        assert_eq!(region[0], (28.0, -62.0));
        assert_eq!(region[2], (-42.0, -57.0));
        // P1 inferred:
        assert!((region[1].0 - 10.5).abs() < 1e-9, "x {}", region[1].0);
        assert_eq!(region[1].1, -57.0);
    }

    /// Build a single-glyph gvar holding one tuple with an embedded peak
    /// at +1.0 and a **private** (partial) point-number set. `pts` is the
    /// ascending referenced point list; `dxs`/`dys` the matching word
    /// deltas (same length as `pts`). `total` pseudo-points are addressed.
    fn build_partial_points_glyph(pts: &[u16], dxs: &[i16], dys: &[i16]) -> Vec<u8> {
        assert_eq!(pts.len(), dxs.len());
        assert_eq!(pts.len(), dys.len());
        let n = pts.len();

        // Tuple data begins with a PRIVATE point-number set, then dx[],
        // then dy[]. Packed point set: count byte (n < 128), then a
        // single run with byte deltas (ascending diffs).
        let mut tuple_data = Vec::new();
        tuple_data.push(n as u8); // count (<128)
        tuple_data.push((n - 1) as u8); // run control: byte deltas, run=n
        let mut last = 0u16;
        for &p in pts {
            tuple_data.push((p - last) as u8);
            last = p;
        }
        // dx[] word run, dy[] word run.
        tuple_data.push(0x40 | ((n - 1) as u8));
        for &d in dxs {
            tuple_data.extend_from_slice(&d.to_be_bytes());
        }
        tuple_data.push(0x40 | ((n - 1) as u8));
        for &d in dys {
            tuple_data.extend_from_slice(&d.to_be_bytes());
        }
        let var_data_size = tuple_data.len() as u16;

        // Data area: an EMPTY shared point set (single 0x00 = all-points)
        // precedes the per-tuple area, but since our tuple uses PRIVATE
        // points the shared set is unused. We still emit it because the
        // decoder always decodes the shared set first.
        let mut data_area = vec![0x00u8];
        data_area.extend_from_slice(&tuple_data);

        // GlyphVariationData header.
        let tuple_header_len = 4 + 2; // size + index(+EMBEDDED_PEAK | PRIVATE) + peak
        let data_offset = (4 + tuple_header_len) as u16;
        let mut block = Vec::new();
        block.extend_from_slice(&1u16.to_be_bytes()); // tupleVariationCount
        block.extend_from_slice(&data_offset.to_be_bytes());
        block.extend_from_slice(&var_data_size.to_be_bytes());
        block.extend_from_slice(&(TI_EMBEDDED_PEAK | TI_PRIVATE_POINTS).to_be_bytes());
        block.extend_from_slice(&0x4000i16.to_be_bytes()); // peak +1.0
        block.extend_from_slice(&data_area);

        let mut b = vec![0u8; 20];
        b[0..2].copy_from_slice(&1u16.to_be_bytes());
        b[4..6].copy_from_slice(&1u16.to_be_bytes());
        b[12..14].copy_from_slice(&1u16.to_be_bytes());
        let data_array_off = 24u32;
        b[16..20].copy_from_slice(&data_array_off.to_be_bytes());
        if block.len() % 2 != 0 {
            block.push(0);
        }
        let end_half = (block.len() / 2) as u16;
        b.extend_from_slice(&0u16.to_be_bytes());
        b.extend_from_slice(&end_half.to_be_bytes());
        b.extend_from_slice(&block);
        b
    }

    #[test]
    fn glyph_deltas_iup_end_to_end_partial_set() {
        // 3 outline points (one contour), only P0 and P2 referenced.
        // P1 should be inferred. Same numbers as the unit test above.
        let raw = build_partial_points_glyph(&[0, 2], &[28, -42], &[-62, -57]);
        let g = GvarTable::parse(&raw).expect("parse");
        let info = SimpleOutlineInfo {
            points: vec![(0, 0), (1, 10), (4, 8)],
            contour_ends: vec![2],
        };
        // At peak (+1.0) the scalar is 1 → region deltas pass through,
        // rounded once at accumulation. P1.x = 10.5 → 11 (half away).
        let out = g.glyph_deltas_iup(0, &info, &[1.0]).expect("iup deltas");
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], (28, -62));
        assert_eq!(out[2], (-42, -57));
        assert_eq!(out[1], (11, -57)); // 10.5 → 11
    }

    #[test]
    fn glyph_deltas_iup_matches_legacy_when_all_points_referenced() {
        // When every point is referenced, IUP is a no-op and the result
        // equals the legacy glyph_deltas path.
        let dxs = [10, 20, 30, 0, 0, 0, 0];
        let dys = [-1, -2, -3, 0, 0, 0, 0];
        let raw = build_one_tuple_glyph(&dxs, &dys);
        let g = GvarTable::parse(&raw).expect("parse");
        let info = SimpleOutlineInfo {
            points: vec![(0, 0), (100, 0), (50, 100)],
            contour_ends: vec![2],
        };
        let legacy = g.glyph_deltas(0, 3, &[0.5]).expect("legacy");
        let iup = g.glyph_deltas_iup(0, &info, &[0.5]).expect("iup");
        assert_eq!(legacy, iup);
    }

    #[test]
    fn glyph_deltas_iup_scales_inferred_deltas() {
        // Inferred deltas must be scaled by the tuple scalar, like
        // explicit ones. At coord +0.5 the scalar is 0.5.
        let raw = build_partial_points_glyph(&[0, 2], &[28, -42], &[-62, -57]);
        let g = GvarTable::parse(&raw).expect("parse");
        let info = SimpleOutlineInfo {
            points: vec![(0, 0), (1, 10), (4, 8)],
            contour_ends: vec![2],
        };
        let out = g.glyph_deltas_iup(0, &info, &[0.5]).expect("iup");
        // P1.x = 10.5 * 0.5 = 5.25 → 5; P1.y = -57 * 0.5 = -28.5 → -28
        // (round half to even? no — f32 round() rounds half away from
        // zero → -29). round() in Rust rounds half away from zero.
        assert_eq!(
            out[0],
            (
                (28.0 * 0.5f64).round() as i32,
                (-62.0 * 0.5f64).round() as i32
            )
        );
        assert_eq!(out[1].0, (10.5 * 0.5f64).round() as i32);
        assert_eq!(out[1].1, (-57.0 * 0.5f64).round() as i32);
    }
}
