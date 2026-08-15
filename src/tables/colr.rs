//! `COLR` — Color Table (versions 0 and 1).
//!
//! The COLR table defines colour glyphs two ways:
//!
//! * **Version 0** maps a "base glyph" to an ordered stack of "layer
//!   glyphs", each tagged with a CPAL palette-entry index. To render a
//!   coloured glyph, the consumer takes each layer in order
//!   (back-to-front), resolves its palette index against the CPAL
//!   palette to get an RGBA colour, then paints the layer glyph's
//!   outline at the same pen origin filled with that colour.
//! * **Version 1** additionally maps a base glyph to the root of a
//!   **directed acyclic graph of Paint tables** (spec §"COLR — Color
//!   Table", OpenType 1.9.1): solid fills, linear / radial / sweep
//!   gradients, glyph-outline clip regions, affine transforms
//!   (translate / scale / rotate / skew / general 2×3), composite /
//!   blend nodes, and layer slices out of a shared LayerList. In
//!   variable fonts every `PaintVar*` form carries a `varIndexBase`
//!   into a `DeltaSetIndexMap` + `ItemVariationStore` pair embedded in
//!   the COLR table itself.
//!
//! This module decodes both. The v1 surface is deliberately
//! **node-by-node**: [`ColrTable::base_glyph_paint`] resolves a base
//! glyph to a [`PaintRef`] (an opaque validated offset), and
//! [`ColrTable::paint`] decodes one Paint table into the [`Paint`]
//! enum with all variation deltas already folded in for the caller's
//! normalised instance coordinates. Child paints are surfaced as
//! further `PaintRef`s so the *caller* owns graph traversal and can
//! bound depth / detect cycles however it likes (the spec requires the
//! graph to be acyclic, but a hostile font can still tie a loop —
//! never recurse unboundedly over `PaintRef`s).
//!
//! ## Version-0 header layout (14 bytes)
//!
//! ```text
//! Offset  Field                    Type      Notes
//! ------  ----------------------   --------  -------------------------------
//!  +0     version                  uint16    0 or 1
//!  +2     numBaseGlyphRecords      uint16    BaseGlyph record count
//!  +4     baseGlyphRecordsOffset   Offset32  from start of COLR
//!  +8     layerRecordsOffset       Offset32  from start of COLR
//! +12     numLayerRecords          uint16    Layer record count
//! ```
//!
//! A version-1 header continues with five more Offset32 fields
//! (each may be NULL):
//!
//! ```text
//! +14     baseGlyphListOffset      Offset32  BaseGlyphList (paint roots)
//! +18     layerListOffset          Offset32  LayerList (PaintColrLayers)
//! +22     clipListOffset           Offset32  ClipList (precomputed boxes)
//! +26     varIndexMapOffset        Offset32  DeltaSetIndexMap
//! +30     itemVariationStoreOffset Offset32  ItemVariationStore
//! ```
//!
//! ## Variation scheme (spec §"COLR" / staged paint-graph reference §7)
//!
//! Each variable table/record carries a `uint32 varIndexBase`; its
//! variable fields consume mapping entries `varIndexBase + 0`,
//! `varIndexBase + 1`, … in field order. `varIndexBase == 0xFFFFFFFF`
//! means "no variation data". With a `DeltaSetIndexMap` present the
//! computed index selects a map entry (clamping to the last entry when
//! out of range; an entry of `0xFFFF/0xFFFF` means "no variation
//! data"); without one, an implicit identity mapping splits the index
//! into `outer = index >> 16`, `inner = index & 0xFFFF`. Deltas are
//! integers in the wire units of the varied field (F2DOT14 fields are
//! varied in 1/16384 steps, Fixed fields in 1/65536 steps, FWORD /
//! UFWORD fields in font units) — the same convention the staged avar
//! v2 reference states explicitly for its F2DOT14 deltas.
//!
//! The `DeltaSetIndexMap` decoder shared with `HVAR` implements both
//! formats from the staged OFF common-formats chapter: format 0
//! (16-bit `mapCount`, byte-identical to the ISO/IEC 14496-22:2019
//! §7.3.5.2 layout) and format 1 (32-bit `mapCount`). The embedded
//! `ItemVariationStore` also honours the chapter's `LONG_WORDS`
//! `wordDeltaCount` flag — the int32 + int16 delta representation the
//! chapter reserves for 32-bit-variable top-level tables, currently
//! COLR only. A map with an unrecognised future format byte still
//! parses the font, but its variation deltas resolve to 0 and
//! [`ColrTable::var_index_map_unsupported`] reports the degradation.

use crate::parser::{read_i16, read_u16, read_u24, read_u32, read_u8};
use crate::tables::hvar::DeltaSetIndexMap;
use crate::tables::mvar::ItemVariationStore;
use crate::Error;

/// One layer of a version-0 colour glyph: an outline-glyph id plus a
/// CPAL palette-entry index. `palette_index == 0xFFFF` is the spec's
/// "use the text foreground colour" sentinel; the consumer renderer is
/// expected to substitute its own foreground in that case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorLayer {
    /// Glyph id of the outline that paints this layer (TT/CFF/CFF2).
    pub layer_glyph_id: u16,
    /// CPAL palette-entry index; `0xFFFF` = foreground colour.
    pub palette_index: u16,
}

/// Opaque reference to one Paint table inside the COLR slice. Obtained
/// from [`ColrTable::base_glyph_paint`] or from a decoded [`Paint`]
/// node's child fields; dereferenced with [`ColrTable::paint`].
///
/// The wrapped value is the paint's byte offset from the start of the
/// COLR table — surfaced so tooling can log / dedupe graph nodes, and
/// so callers can cycle-check traversals by collecting visited offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaintRef(pub u32);

/// Gradient color-line extend mode. Unrecognised wire values decode to
/// `Pad` per the spec ("Unrecognized extend values default to
/// EXTEND_PAD").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extend {
    /// Use the nearest colour stop outside the stop range.
    Pad,
    /// Repeat from the farthest colour stop.
    Repeat,
    /// Mirror the colour line from the nearest end.
    Reflect,
}

impl Extend {
    fn from_wire(v: u8) -> Self {
        match v {
            1 => Extend::Repeat,
            2 => Extend::Reflect,
            _ => Extend::Pad,
        }
    }
}

/// One gradient colour stop, resolved at the requested instance:
/// `stop_offset` / `alpha` have any variation deltas folded in and
/// `alpha` is clamped to `[0.0, 1.0]` per the spec ("values outside
/// this range are reserved and must be clamped").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorStop {
    /// Position on the colour line (typically in `[0, 1]`, but any
    /// F2DOT14 value is legal wire data).
    pub stop_offset: f32,
    /// CPAL palette-entry index; `0xFFFF` = foreground colour.
    pub palette_index: u16,
    /// Alpha in `[0.0, 1.0]`; multiplied with the CPAL entry's own
    /// alpha by the renderer.
    pub alpha: f32,
}

/// A gradient colour line: extend mode plus the resolved stops, sorted
/// ascending by `stop_offset`. The sort happens **after** instance
/// stop-offset values are derived, as the spec requires for variable
/// fonts ("order is established after applying variation deltas").
#[derive(Debug, Clone, PartialEq)]
pub struct ColorLine {
    /// Behaviour outside the defined stop range.
    pub extend: Extend,
    /// Stops sorted ascending by `stop_offset` (stable sort — equal
    /// offsets keep wire order).
    pub stops: Vec<ColorStop>,
}

/// A resolved 2×3 affine matrix (`Affine2x3` / `VarAffine2x3`).
/// Post-transform position: `x' = xx·x + xy·y + dx`,
/// `y' = yx·x + yy·y + dy`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine2x3 {
    /// x-component of the transformed x-basis vector.
    pub xx: f32,
    /// y-component of the transformed x-basis vector.
    pub yx: f32,
    /// x-component of the transformed y-basis vector.
    pub xy: f32,
    /// y-component of the transformed y-basis vector.
    pub yy: f32,
    /// Translation in x.
    pub dx: f32,
    /// Translation in y.
    pub dy: f32,
}

/// `PaintComposite` mode (spec CompositeMode enumeration). The twelve
/// Porter-Duff modes, the eleven separable blend modes, and the four
/// non-separable HSL blend modes. Unrecognised wire values decode to
/// `Clear` per the spec ("Unrecognized modes must use
/// COMPOSITE_CLEAR").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(missing_docs)] // variant names mirror the spec enumeration 1:1
pub enum CompositeMode {
    Clear,
    Src,
    Dest,
    SrcOver,
    DestOver,
    SrcIn,
    DestIn,
    SrcOut,
    DestOut,
    SrcAtop,
    DestAtop,
    Xor,
    Plus,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Multiply,
    HslHue,
    HslSaturation,
    HslColor,
    HslLuminosity,
}

impl CompositeMode {
    fn from_wire(v: u8) -> Self {
        use CompositeMode::*;
        match v {
            0 => Clear,
            1 => Src,
            2 => Dest,
            3 => SrcOver,
            4 => DestOver,
            5 => SrcIn,
            6 => DestIn,
            7 => SrcOut,
            8 => DestOut,
            9 => SrcAtop,
            10 => DestAtop,
            11 => Xor,
            12 => Plus,
            13 => Screen,
            14 => Overlay,
            15 => Darken,
            16 => Lighten,
            17 => ColorDodge,
            18 => ColorBurn,
            19 => HardLight,
            20 => SoftLight,
            21 => Difference,
            22 => Exclusion,
            23 => Multiply,
            24 => HslHue,
            25 => HslSaturation,
            26 => HslColor,
            27 => HslLuminosity,
            _ => Clear,
        }
    }

    /// Whether a `PaintComposite` sub-graph using this mode is
    /// *bounded*, given the boundedness of its source and backdrop
    /// sub-graphs (spec §"PaintComposite" boundedness table). Version-1
    /// colour glyph definitions are required to be bounded.
    pub fn is_bounded(self, source_bounded: bool, backdrop_bounded: bool) -> bool {
        use CompositeMode::*;
        match self {
            Clear => true,
            Src | SrcOut => source_bounded,
            Dest | DestOut => backdrop_bounded,
            SrcIn | DestIn => source_bounded || backdrop_bounded,
            _ => source_bounded && backdrop_bounded,
        }
    }
}

/// A precomputed colour-glyph clip box from the ClipList, resolved at
/// the requested instance. Variable boxes round *outward* (mins toward
/// −∞, maxes toward +∞) per the spec, so the box only ever expands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipBox {
    /// Minimum x of the clip box, font units.
    pub x_min: i32,
    /// Minimum y of the clip box, font units.
    pub y_min: i32,
    /// Maximum x of the clip box, font units.
    pub x_max: i32,
    /// Maximum y of the clip box, font units.
    pub y_max: i32,
}

/// One decoded Paint table, values resolved at the caller's variation
/// instance (the `PaintVar*` twin of each format folds its deltas into
/// the same variant — a renderer never sees the var/non-var split).
/// Child paints are [`PaintRef`]s to be decoded with
/// [`ColrTable::paint`]; the caller owns traversal and must bound
/// depth (hostile fonts can tie cycles through `PaintColrGlyph`).
#[derive(Debug, Clone, PartialEq)]
pub enum Paint {
    /// Formats 1: a bottom-up z-ordered slice of the LayerList,
    /// composited with source-over.
    ColrLayers {
        /// Child paints, bottom (first) to top (last).
        layers: Vec<PaintRef>,
    },
    /// Formats 2/3: a solid CPAL-palette fill.
    Solid {
        /// CPAL palette-entry index; `0xFFFF` = foreground colour.
        palette_index: u16,
        /// Alpha in `[0.0, 1.0]`.
        alpha: f32,
    },
    /// Formats 4/5: linear gradient along p₀→p₁ with rotation point p₂.
    LinearGradient {
        /// The gradient colour line.
        color_line: ColorLine,
        /// Start point p₀ x, font units.
        x0: f32,
        /// Start point p₀ y.
        y0: f32,
        /// End point p₁ x.
        x1: f32,
        /// End point p₁ y.
        y1: f32,
        /// Rotation point p₂ x.
        x2: f32,
        /// Rotation point p₂ y.
        y2: f32,
    },
    /// Formats 6/7: radial gradient between two circles. Radii are
    /// unsigned on the wire but variation deltas may drive them
    /// negative; the spec's r(ω) algorithm handles that, so the
    /// resolved values are surfaced un-clamped.
    RadialGradient {
        /// The gradient colour line.
        color_line: ColorLine,
        /// Start circle centre x, font units.
        x0: f32,
        /// Start circle centre y.
        y0: f32,
        /// Start circle radius.
        radius0: f32,
        /// End circle centre x.
        x1: f32,
        /// End circle centre y.
        y1: f32,
        /// End circle radius.
        radius1: f32,
    },
    /// Formats 8/9: sweep gradient around a centre. Angles are in
    /// counter-clockwise **degrees** with the spec's +1.0 bias already
    /// applied (wire F2DOT14 −2.0 → −180°, 0.0 → +180°, 1.0 → +360°).
    SweepGradient {
        /// The gradient colour line.
        color_line: ColorLine,
        /// Centre x, font units.
        center_x: f32,
        /// Centre y, font units.
        center_y: f32,
        /// Start angle, degrees counter-clockwise from the positive
        /// x-axis direction.
        start_angle_degrees: f32,
        /// End angle, degrees counter-clockwise.
        end_angle_degrees: f32,
    },
    /// Format 10: use a glyph outline as the clip region for the child
    /// fill sub-graph. Any COLR data for `glyph_id` itself is ignored
    /// here — it must be an ordinary outline glyph.
    Glyph {
        /// The fill sub-graph, clipped to the outline.
        paint: PaintRef,
        /// Outline glyph id (`glyf`/`CFF `/`CFF2`).
        glyph_id: u16,
    },
    /// Format 11: reuse another base glyph's whole paint graph as a
    /// child sub-graph. Resolve through
    /// [`ColrTable::base_glyph_paint`]; a missing record makes the
    /// colour glyph not well-formed per the spec.
    ColrGlyph {
        /// BaseGlyphList base glyph id.
        glyph_id: u16,
    },
    /// Formats 12/13: general 2×3 affine transform of the child.
    Transform {
        /// The transformed sub-graph.
        paint: PaintRef,
        /// The resolved matrix.
        transform: Affine2x3,
    },
    /// Formats 14/15: translation of the child, font units.
    Translate {
        /// The translated sub-graph.
        paint: PaintRef,
        /// Translation in x.
        dx: f32,
        /// Translation in y.
        dy: f32,
    },
    /// Formats 16–23: scaling of the child about `(center_x,
    /// center_y)`. The four wire forms (x/y vs. uniform, origin vs.
    /// explicit centre) all fold here — uniform forms set
    /// `scale_x == scale_y`, origin-centred forms set the centre to
    /// `(0, 0)`; [`ColrTable::paint_format`] recovers the wire form.
    Scale {
        /// The scaled sub-graph.
        paint: PaintRef,
        /// Scale factor in x.
        scale_x: f32,
        /// Scale factor in y.
        scale_y: f32,
        /// Centre of scaling x, font units.
        center_x: f32,
        /// Centre of scaling y, font units.
        center_y: f32,
    },
    /// Formats 24–27: rotation of the child about `(center_x,
    /// center_y)`. Degrees counter-clockwise, **no** bias (wire
    /// F2DOT14 × 180).
    Rotate {
        /// The rotated sub-graph.
        paint: PaintRef,
        /// Rotation angle, degrees counter-clockwise.
        angle_degrees: f32,
        /// Centre of rotation x, font units.
        center_x: f32,
        /// Centre of rotation y, font units.
        center_y: f32,
    },
    /// Formats 28–31: skew of the child about `(center_x, center_y)`.
    /// Degrees, no bias.
    Skew {
        /// The skewed sub-graph.
        paint: PaintRef,
        /// Skew angle in the x-axis direction, degrees.
        x_skew_degrees: f32,
        /// Skew angle in the y-axis direction, degrees.
        y_skew_degrees: f32,
        /// Centre of skew x, font units.
        center_x: f32,
        /// Centre of skew y, font units.
        center_y: f32,
    },
    /// Format 32: render `backdrop`, render `source`, combine with
    /// `mode`, then composite onto the surface.
    Composite {
        /// Source sub-graph (rendered second).
        source: PaintRef,
        /// Combining mode.
        mode: CompositeMode,
        /// Backdrop sub-graph (rendered first).
        backdrop: PaintRef,
    },
}

/// Cap on eagerly-decoded v1 array lengths that a hostile header could
/// otherwise inflate (each entry is bounds-checked against the table
/// anyway; the cap just bounds allocation before that check bites).
const MAX_V1_RECORDS: u32 = 1 << 20;

/// Path-depth cap for the boundedness analysis. The spec caps nothing,
/// but a legitimate paint graph nests a few dozen levels at most.
const BOUNDEDNESS_MAX_DEPTH: usize = 64;

/// Total node-visit budget for the boundedness analysis: shared
/// sub-graphs (diamonds) re-evaluate on every path, so an adversarial
/// DAG could otherwise cost exponential work.
const BOUNDEDNESS_BUDGET: u32 = 4096;

/// Parsed COLR table (v0 layer stacks + the v1 paint graph).
#[derive(Debug, Clone)]
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub struct ColrTable<'a> {
    bytes: &'a [u8],
    /// Number of `BaseGlyphRecord`s (always v0-array-shaped).
    num_base_records: u16,
    /// Byte offset (from start of COLR) of the base record array.
    base_records_offset: u32,
    /// Number of `LayerRecord`s.
    num_layer_records: u16,
    /// Byte offset of the layer record array.
    layer_records_offset: u32,
    /// v1 BaseGlyphPaintRecords: `(glyphID, absolute paint offset)`,
    /// wire order (sorted ascending by glyphID per spec).
    base_glyph_paints: Vec<(u16, u32)>,
    /// v1 LayerList: absolute paint offsets, wire order.
    layer_list: Vec<u32>,
    /// v1 ClipList: `(startGlyphID, endGlyphID, absolute ClipBox
    /// offset)`, wire order (sorted ascending by startGlyphID).
    clip_records: Vec<(u16, u16, u32)>,
    /// v1 DeltaSetIndexMap (format 0 or 1).
    var_index_map: Option<DeltaSetIndexMap>,
    /// A varIndexMap was present but not decodable (an unrecognised
    /// future format byte, reserved entryFormat bits, or a truncated
    /// map); variation deltas degrade to 0.
    var_index_map_unsupported: bool,
    /// v1 ItemVariationStore.
    ivs: Option<ItemVariationStore>,
}

impl<'a> ColrTable<'a> {
    /// Validate the header, remember the v0 array offsets, and — for a
    /// version-1 table — eagerly decode the BaseGlyphList / LayerList /
    /// ClipList arrays plus the embedded DeltaSetIndexMap and
    /// ItemVariationStore. Versions above 1 are accepted (the v0/v1
    /// fields keep their offsets; unknown trailing extensions are
    /// ignored).
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < 14 {
            return Err(Error::UnexpectedEof);
        }
        let version = read_u16(bytes, 0)?;
        let num_base_records = read_u16(bytes, 2)?;
        let base_records_offset = read_u32(bytes, 4)?;
        let layer_records_offset = read_u32(bytes, 8)?;
        let num_layer_records = read_u16(bytes, 12)?;

        // Range-check the two v0 arrays against the COLR slice. We
        // allow an empty array (numFoo == 0) regardless of offset.
        if num_base_records > 0 {
            let end = (base_records_offset as u64)
                .checked_add(num_base_records as u64 * 6)
                .ok_or(Error::BadOffset)?;
            if end > bytes.len() as u64 {
                return Err(Error::BadOffset);
            }
        }
        if num_layer_records > 0 {
            let end = (layer_records_offset as u64)
                .checked_add(num_layer_records as u64 * 4)
                .ok_or(Error::BadOffset)?;
            if end > bytes.len() as u64 {
                return Err(Error::BadOffset);
            }
        }

        let mut table = Self {
            bytes,
            num_base_records,
            base_records_offset,
            num_layer_records,
            layer_records_offset,
            base_glyph_paints: Vec::new(),
            layer_list: Vec::new(),
            clip_records: Vec::new(),
            var_index_map: None,
            var_index_map_unsupported: false,
            ivs: None,
        };

        if version >= 1 && bytes.len() >= 34 {
            table.parse_v1_extras()?;
        }
        Ok(table)
    }

    /// Decode the five v1 header offsets and the structures they point
    /// at. All offsets are from the start of the COLR table; each may
    /// be NULL (0).
    fn parse_v1_extras(&mut self) -> Result<(), Error> {
        let bytes = self.bytes;
        let base_glyph_list_off = read_u32(bytes, 14)? as usize;
        let layer_list_off = read_u32(bytes, 18)? as usize;
        let clip_list_off = read_u32(bytes, 22)? as usize;
        let var_index_map_off = read_u32(bytes, 26)? as usize;
        let ivs_off = read_u32(bytes, 30)? as usize;

        if base_glyph_list_off != 0 {
            if base_glyph_list_off + 4 > bytes.len() {
                return Err(Error::BadOffset);
            }
            let count = read_u32(bytes, base_glyph_list_off)?;
            if count > MAX_V1_RECORDS {
                return Err(Error::BadStructure("COLR BaseGlyphList count exceeds cap"));
            }
            let end = (base_glyph_list_off as u64)
                .checked_add(4 + count as u64 * 6)
                .ok_or(Error::BadOffset)?;
            if end > bytes.len() as u64 {
                return Err(Error::BadOffset);
            }
            self.base_glyph_paints.reserve(count as usize);
            for i in 0..count as usize {
                let off = base_glyph_list_off + 4 + i * 6;
                let gid = read_u16(bytes, off)?;
                let paint_off = read_u32(bytes, off + 2)?;
                // Paint offsets are relative to the BaseGlyphList
                // start; store absolute + validated-in-bounds.
                let abs = (base_glyph_list_off as u64)
                    .checked_add(paint_off as u64)
                    .ok_or(Error::BadOffset)?;
                if paint_off == 0 || abs >= bytes.len() as u64 {
                    return Err(Error::BadOffset);
                }
                self.base_glyph_paints.push((gid, abs as u32));
            }
        }

        if layer_list_off != 0 {
            if layer_list_off + 4 > bytes.len() {
                return Err(Error::BadOffset);
            }
            let count = read_u32(bytes, layer_list_off)?;
            if count > MAX_V1_RECORDS {
                return Err(Error::BadStructure("COLR LayerList count exceeds cap"));
            }
            let end = (layer_list_off as u64)
                .checked_add(4 + count as u64 * 4)
                .ok_or(Error::BadOffset)?;
            if end > bytes.len() as u64 {
                return Err(Error::BadOffset);
            }
            self.layer_list.reserve(count as usize);
            for i in 0..count as usize {
                let off = layer_list_off + 4 + i * 4;
                let paint_off = read_u32(bytes, off)?;
                let abs = (layer_list_off as u64)
                    .checked_add(paint_off as u64)
                    .ok_or(Error::BadOffset)?;
                if paint_off == 0 || abs >= bytes.len() as u64 {
                    return Err(Error::BadOffset);
                }
                self.layer_list.push(abs as u32);
            }
        }

        if clip_list_off != 0 {
            if clip_list_off + 5 > bytes.len() {
                return Err(Error::BadOffset);
            }
            let format = read_u8(bytes, clip_list_off)?;
            // Only format 1 is defined; ignore an unrecognised format
            // (forward compatibility) rather than rejecting the font.
            if format == 1 {
                let count = read_u32(bytes, clip_list_off + 1)?;
                if count > MAX_V1_RECORDS {
                    return Err(Error::BadStructure("COLR ClipList count exceeds cap"));
                }
                let end = (clip_list_off as u64)
                    .checked_add(5 + count as u64 * 7)
                    .ok_or(Error::BadOffset)?;
                if end > bytes.len() as u64 {
                    return Err(Error::BadOffset);
                }
                self.clip_records.reserve(count as usize);
                for i in 0..count as usize {
                    let off = clip_list_off + 5 + i * 7;
                    let start = read_u16(bytes, off)?;
                    let end_gid = read_u16(bytes, off + 2)?;
                    let box_off = read_u24(bytes, off + 4)?;
                    let abs = (clip_list_off as u64)
                        .checked_add(box_off as u64)
                        .ok_or(Error::BadOffset)?;
                    if box_off == 0 || abs >= bytes.len() as u64 {
                        return Err(Error::BadOffset);
                    }
                    self.clip_records.push((start, end_gid, abs as u32));
                }
            }
        }

        if var_index_map_off != 0 {
            if var_index_map_off >= bytes.len() {
                return Err(Error::BadOffset);
            }
            // The shared decoder implements both defined map formats
            // (0 and 1). An unrecognised future format / malformed
            // map degrades to no-variation rather than rejecting the
            // font, and is flagged.
            match DeltaSetIndexMap::parse(&bytes[var_index_map_off..]) {
                Ok(map) => self.var_index_map = Some(map),
                Err(_) => self.var_index_map_unsupported = true,
            }
        }

        if ivs_off != 0 {
            if ivs_off >= bytes.len() {
                return Err(Error::BadOffset);
            }
            self.ivs = Some(ItemVariationStore::parse(&bytes[ivs_off..])?);
        }
        Ok(())
    }

    // ---- v0 ---------------------------------------------------------------

    /// Locate `glyph_id`'s BaseGlyphRecord by binary search and decode
    /// its `(first_layer_index, num_layers)` pair. Returns `None` when
    /// the glyph isn't a base — i.e. it's a single-colour outline glyph
    /// or a layer-only glyph.
    fn find_base_record(&self, glyph_id: u16) -> Option<(u16, u16)> {
        // Records are required to be sorted ascending by glyphID.
        let base = self.base_records_offset as usize;
        let mut lo = 0i32;
        let mut hi = self.num_base_records as i32 - 1;
        while lo <= hi {
            let mid = ((lo + hi) >> 1) as usize;
            let off = base + mid * 6;
            let gid = read_u16(self.bytes, off).ok()?;
            match gid.cmp(&glyph_id) {
                std::cmp::Ordering::Less => lo = mid as i32 + 1,
                std::cmp::Ordering::Greater => hi = mid as i32 - 1,
                std::cmp::Ordering::Equal => {
                    let first = read_u16(self.bytes, off + 2).ok()?;
                    let count = read_u16(self.bytes, off + 4).ok()?;
                    return Some((first, count));
                }
            }
        }
        None
    }

    /// All version-0 colour layers for `glyph_id`, in back-to-front
    /// paint order (= the order the layer records appear in the table).
    /// Returns an empty `Vec` when the glyph isn't a colour glyph or
    /// the COLR table is empty. Note the spec prefers a version-1
    /// paint graph over a version-0 layer stack for the same base
    /// glyph — check [`Self::base_glyph_paint`] first.
    pub fn layers(&self, glyph_id: u16) -> Vec<ColorLayer> {
        let (first, count) = match self.find_base_record(glyph_id) {
            Some(p) => p,
            None => return Vec::new(),
        };
        let mut out = Vec::with_capacity(count as usize);
        let layer_base = self.layer_records_offset as usize;
        for i in 0..count {
            let idx = first as usize + i as usize;
            // Spec says firstLayerIndex+numLayers must be <=
            // numLayerRecords; we range-check defensively anyway.
            if idx >= self.num_layer_records as usize {
                break;
            }
            let off = layer_base + idx * 4;
            let layer_glyph_id = match read_u16(self.bytes, off) {
                Ok(v) => v,
                Err(_) => break,
            };
            let palette_index = match read_u16(self.bytes, off + 2) {
                Ok(v) => v,
                Err(_) => break,
            };
            out.push(ColorLayer {
                layer_glyph_id,
                palette_index,
            });
        }
        out
    }

    /// Number of `BaseGlyphRecord`s the table ships. Mostly useful for
    /// tests / debug printing; consumers should call `layers` directly.
    pub fn num_base_records(&self) -> u16 {
        self.num_base_records
    }

    // ---- v1: base glyphs, layers, clips -----------------------------------

    /// `true` when the table carries a version-1 BaseGlyphList with at
    /// least one paint record.
    pub fn has_paint_graph(&self) -> bool {
        !self.base_glyph_paints.is_empty()
    }

    /// Number of `BaseGlyphPaintRecord`s in the BaseGlyphList.
    pub fn num_base_glyph_paint_records(&self) -> u32 {
        self.base_glyph_paints.len() as u32
    }

    /// Resolve `glyph_id` to the root Paint of its version-1 colour
    /// glyph graph (binary search over the sorted BaseGlyphList).
    pub fn base_glyph_paint(&self, glyph_id: u16) -> Option<PaintRef> {
        self.base_glyph_paints
            .binary_search_by_key(&glyph_id, |&(g, _)| g)
            .ok()
            .map(|i| PaintRef(self.base_glyph_paints[i].1))
    }

    /// Enumerate every `(glyphID, root PaintRef)` pair in the
    /// BaseGlyphList, wire order.
    pub fn base_glyph_paint_records(&self) -> impl Iterator<Item = (u16, PaintRef)> + '_ {
        self.base_glyph_paints
            .iter()
            .map(|&(g, off)| (g, PaintRef(off)))
    }

    /// Number of entries in the LayerList.
    pub fn layer_list_len(&self) -> u32 {
        self.layer_list.len() as u32
    }

    /// A varIndexMap is present but does not decode — an
    /// unrecognised format byte (a future revision), reserved
    /// entryFormat bits, or a truncated map. Both defined formats
    /// (0 and 1) decode, so this only fires on malformed or
    /// future-format maps — all variation deltas resolve to 0 for
    /// such a font.
    pub fn var_index_map_unsupported(&self) -> bool {
        self.var_index_map_unsupported
    }

    /// `true` when the table embeds an `ItemVariationStore` (i.e. the
    /// paint graph can vary across instances).
    pub fn has_variations(&self) -> bool {
        self.ivs.is_some()
    }

    /// The precomputed clip box covering `glyph_id`, resolved at
    /// `coords` (pass `&[]` for the static instance). Binary search
    /// over the sorted, non-overlapping ClipList ranges. Variable
    /// boxes (ClipBoxFormat 2) fold their deltas and round outward.
    pub fn clip_box(&self, glyph_id: u16, coords: &[f32]) -> Option<ClipBox> {
        let idx = self
            .clip_records
            .partition_point(|&(start, _, _)| start <= glyph_id)
            .checked_sub(1)?;
        let (start, end, abs) = self.clip_records[idx];
        if glyph_id < start || glyph_id > end {
            return None;
        }
        let off = abs as usize;
        let format = read_u8(self.bytes, off).ok()?;
        let x_min = read_i16(self.bytes, off + 1).ok()?;
        let y_min = read_i16(self.bytes, off + 3).ok()?;
        let x_max = read_i16(self.bytes, off + 5).ok()?;
        let y_max = read_i16(self.bytes, off + 7).ok()?;
        match format {
            1 => Some(ClipBox {
                x_min: x_min as i32,
                y_min: y_min as i32,
                x_max: x_max as i32,
                y_max: y_max as i32,
            }),
            2 => {
                let base = read_u32(self.bytes, off + 9).ok()?;
                // Round so the box expands: mins toward −∞, maxes
                // toward +∞ (spec ClipBoxFormat2 rule).
                Some(ClipBox {
                    x_min: (x_min as f32 + self.var_delta(base, 0, coords)).floor() as i32,
                    y_min: (y_min as f32 + self.var_delta(base, 1, coords)).floor() as i32,
                    x_max: (x_max as f32 + self.var_delta(base, 2, coords)).ceil() as i32,
                    y_max: (y_max as f32 + self.var_delta(base, 3, coords)).ceil() as i32,
                })
            }
            _ => None,
        }
    }

    // ---- v1: variation resolution ------------------------------------------

    /// Delta for variable field `field` of the record based at
    /// `var_index_base`, at `coords`. 0.0 whenever there is no
    /// variation data (no IVS, the 0xFFFFFFFF sentinel, a 0xFFFF/0xFFFF
    /// map entry, an out-of-range store index, or an undecodable map).
    fn var_delta(&self, var_index_base: u32, field: u32, coords: &[f32]) -> f32 {
        if var_index_base == 0xFFFF_FFFF {
            return 0.0;
        }
        let Some(ivs) = self.ivs.as_ref() else {
            // Spec: without an ItemVariationStore, varIndexBase is
            // ignored entirely.
            return 0.0;
        };
        if self.var_index_map_unsupported {
            return 0.0;
        }
        let Some(index) = var_index_base.checked_add(field) else {
            // "The index sequence must not exceed 0xFFFFFFFF."
            return 0.0;
        };
        let (outer, inner) = match self.var_index_map.as_ref() {
            Some(map) => {
                let entries = map.entries();
                if entries.is_empty() {
                    return 0.0;
                }
                // Out-of-range indices clamp to the last entry.
                let e = entries[(index as usize).min(entries.len() - 1)];
                if e == (0xFFFF, 0xFFFF) {
                    // "No variation data for this item."
                    return 0.0;
                }
                e
            }
            // Implicit identity mapping: high 16 bits outer, low 16
            // bits inner.
            None => ((index >> 16) as u16, (index & 0xFFFF) as u16),
        };
        ivs.delta(outer, inner, coords).unwrap_or(0.0)
    }

    // ---- v1: boundedness ---------------------------------------------------

    /// Whether the colour glyph rooted at `glyph_id` is *bounded* — a
    /// well-formedness requirement for version-1 colour glyphs (staged
    /// reference §9: "A version-1 color glyph definition must be
    /// bounded"). `Some(false)` means the graph decodes but paints an
    /// unbounded region (e.g. a bare gradient with no `PaintGlyph`
    /// clip); `None` means the graph is not well-formed (missing base
    /// glyph, undecodable node, a cycle, or an adversarially-deep /
    /// -wide graph that exhausts the analysis budget).
    pub fn color_glyph_is_bounded(&self, glyph_id: u16) -> Option<bool> {
        let root = self.base_glyph_paint(glyph_id)?;
        self.paint_is_bounded(root)
    }

    /// [`Self::color_glyph_is_bounded`] for an arbitrary sub-graph
    /// root.
    pub fn paint_is_bounded(&self, paint: PaintRef) -> Option<bool> {
        let mut path = Vec::new();
        let mut budget = BOUNDEDNESS_BUDGET;
        self.bounded_inner(paint, &mut path, &mut budget)
    }

    fn bounded_inner(
        &self,
        paint: PaintRef,
        path: &mut Vec<u32>,
        budget: &mut u32,
    ) -> Option<bool> {
        if *budget == 0 || path.len() >= BOUNDEDNESS_MAX_DEPTH {
            return None;
        }
        *budget -= 1;
        if path.contains(&paint.0) {
            // A cycle is not a DAG: the glyph is not well-formed.
            return None;
        }
        path.push(paint.0);
        // Boundedness is structural — modes, shapes, and graph edges
        // don't move with variation deltas — so the default instance
        // suffices.
        let result = match self.paint(paint, &[])? {
            // The union of bounded regions is bounded; an empty layer
            // slice paints nothing (bounded).
            Paint::ColrLayers { layers } => {
                let mut all = true;
                for layer in layers {
                    match self.bounded_inner(layer, path, budget) {
                        Some(b) => all &= b,
                        None => {
                            path.pop();
                            return None;
                        }
                    }
                }
                Some(all)
            }
            // Fills cover the whole clip region: unbounded on their
            // own.
            Paint::Solid { .. }
            | Paint::LinearGradient { .. }
            | Paint::RadialGradient { .. }
            | Paint::SweepGradient { .. } => Some(false),
            // §9: PaintGlyph is inherently bounded (the child fill is
            // clipped to the outline).
            Paint::Glyph { .. } => Some(true),
            // Reuse: bounded iff the referenced glyph's graph is.
            // A missing BaseGlyphPaintRecord is not well-formed.
            Paint::ColrGlyph { glyph_id } => {
                let root = self.base_glyph_paint(glyph_id);
                match root {
                    Some(root) => self.bounded_inner(root, path, budget),
                    None => None,
                }
            }
            // An affine image of a bounded region is bounded.
            Paint::Transform { paint, .. }
            | Paint::Translate { paint, .. }
            | Paint::Scale { paint, .. }
            | Paint::Rotate { paint, .. }
            | Paint::Skew { paint, .. } => self.bounded_inner(paint, path, budget),
            // The §6 per-mode table via CompositeMode::is_bounded.
            Paint::Composite {
                source,
                mode,
                backdrop,
            } => {
                let s = self.bounded_inner(source, path, budget);
                let b = self.bounded_inner(backdrop, path, budget);
                match (s, b) {
                    (Some(s), Some(b)) => Some(mode.is_bounded(s, b)),
                    _ => None,
                }
            }
        };
        path.pop();
        result
    }

    // ---- v1: paint decode ---------------------------------------------------

    /// The wire `format` byte of the Paint table at `paint`, without
    /// decoding it. Lets tooling distinguish e.g. the four scale wire
    /// forms that [`Self::paint`] folds into [`Paint::Scale`], and a
    /// `PaintVar*` from its static twin.
    pub fn paint_format(&self, paint: PaintRef) -> Option<u8> {
        read_u8(self.bytes, paint.0 as usize).ok()
    }

    /// Resolve an `Offset24` child-paint field at `off` (relative to
    /// the paint table at `base`) into a validated [`PaintRef`].
    fn child_paint(&self, base: usize, off: usize) -> Option<PaintRef> {
        let rel = read_u24(self.bytes, base + off).ok()?;
        if rel == 0 {
            return None;
        }
        let abs = (base as u64).checked_add(rel as u64)?;
        if abs >= self.bytes.len() as u64 {
            return None;
        }
        Some(PaintRef(abs as u32))
    }

    /// Decode the ColorLine / VarColorLine at absolute offset `abs`.
    fn color_line(&self, abs: usize, variable: bool, coords: &[f32]) -> Option<ColorLine> {
        let extend = Extend::from_wire(read_u8(self.bytes, abs).ok()?);
        let num_stops = read_u16(self.bytes, abs + 1).ok()?;
        let stride = if variable { 10 } else { 6 };
        let mut stops = Vec::with_capacity(num_stops as usize);
        for i in 0..num_stops as usize {
            let off = abs + 3 + i * stride;
            let raw_offset = read_i16(self.bytes, off).ok()?;
            let palette_index = read_u16(self.bytes, off + 2).ok()?;
            let raw_alpha = read_i16(self.bytes, off + 4).ok()?;
            let (d_offset, d_alpha) = if variable {
                let base = read_u32(self.bytes, off + 6).ok()?;
                (
                    self.var_delta(base, 0, coords),
                    self.var_delta(base, 1, coords),
                )
            } else {
                (0.0, 0.0)
            };
            stops.push(ColorStop {
                stop_offset: f2dot14_var(raw_offset, d_offset),
                palette_index,
                alpha: f2dot14_var(raw_alpha, d_alpha).clamp(0.0, 1.0),
            });
        }
        // "Color stops must be applied in increasing stopOffset order",
        // established *after* instance values are derived.
        stops.sort_by(|a, b| {
            a.stop_offset
                .partial_cmp(&b.stop_offset)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Some(ColorLine { extend, stops })
    }

    /// Decode the `Offset24` ColorLine field at `off` within the paint
    /// at `base`.
    fn paint_color_line(
        &self,
        base: usize,
        off: usize,
        variable: bool,
        coords: &[f32],
    ) -> Option<ColorLine> {
        let rel = read_u24(self.bytes, base + off).ok()?;
        if rel == 0 {
            return None;
        }
        let abs = (base as u64).checked_add(rel as u64)?;
        if abs >= self.bytes.len() as u64 {
            return None;
        }
        self.color_line(abs as usize, variable, coords)
    }

    /// Decode one Paint table at the caller's variation instance
    /// (`coords` = the avar-bent normalised coordinate vector; pass
    /// `&[]` for the default / static instance). Every `PaintVar*`
    /// form resolves to the same [`Paint`] variant as its static twin
    /// with the deltas folded in. Returns `None` for an unrecognised
    /// format (the spec's forward-compatibility behaviour: ignore) or
    /// a malformed table.
    pub fn paint(&self, paint: PaintRef, coords: &[f32]) -> Option<Paint> {
        let b = self.bytes;
        let p = paint.0 as usize;
        let format = read_u8(b, p).ok()?;
        match format {
            // PaintColrLayers
            1 => {
                let num_layers = read_u8(b, p + 1).ok()? as usize;
                let first = read_u32(b, p + 2).ok()? as usize;
                let mut layers = Vec::with_capacity(num_layers);
                for i in 0..num_layers {
                    // Defensive: a slice reaching past the LayerList
                    // truncates rather than failing the whole node.
                    let Some(&abs) = self.layer_list.get(first + i) else {
                        break;
                    };
                    layers.push(PaintRef(abs));
                }
                Some(Paint::ColrLayers { layers })
            }
            // PaintSolid / PaintVarSolid
            2 | 3 => {
                let palette_index = read_u16(b, p + 1).ok()?;
                let raw_alpha = read_i16(b, p + 3).ok()?;
                let d_alpha = if format == 3 {
                    let base = read_u32(b, p + 5).ok()?;
                    self.var_delta(base, 0, coords)
                } else {
                    0.0
                };
                Some(Paint::Solid {
                    palette_index,
                    alpha: f2dot14_var(raw_alpha, d_alpha).clamp(0.0, 1.0),
                })
            }
            // PaintLinearGradient / PaintVarLinearGradient
            4 | 5 => {
                let variable = format == 5;
                let color_line = self.paint_color_line(p, 1, variable, coords)?;
                let mut v = [0.0f32; 6];
                let vb = if variable {
                    read_u32(b, p + 16).ok()?
                } else {
                    0xFFFF_FFFF
                };
                for (i, slot) in v.iter_mut().enumerate() {
                    let raw = read_i16(b, p + 4 + i * 2).ok()?;
                    let d = if variable {
                        self.var_delta(vb, i as u32, coords)
                    } else {
                        0.0
                    };
                    *slot = raw as f32 + d;
                }
                Some(Paint::LinearGradient {
                    color_line,
                    x0: v[0],
                    y0: v[1],
                    x1: v[2],
                    y1: v[3],
                    x2: v[4],
                    y2: v[5],
                })
            }
            // PaintRadialGradient / PaintVarRadialGradient
            6 | 7 => {
                let variable = format == 7;
                let color_line = self.paint_color_line(p, 1, variable, coords)?;
                let vb = if variable {
                    read_u32(b, p + 16).ok()?
                } else {
                    0xFFFF_FFFF
                };
                let mut v = [0.0f32; 6];
                for (i, slot) in v.iter_mut().enumerate() {
                    // Fields 2 and 5 (radius0 / radius1) are UFWORD;
                    // the rest FWORD. Deltas are font-unit integers
                    // either way and may drive a radius negative.
                    let raw = if i == 2 || i == 5 {
                        read_u16(b, p + 4 + i * 2).ok()? as f32
                    } else {
                        read_i16(b, p + 4 + i * 2).ok()? as f32
                    };
                    let d = if variable {
                        self.var_delta(vb, i as u32, coords)
                    } else {
                        0.0
                    };
                    *slot = raw + d;
                }
                Some(Paint::RadialGradient {
                    color_line,
                    x0: v[0],
                    y0: v[1],
                    radius0: v[2],
                    x1: v[3],
                    y1: v[4],
                    radius1: v[5],
                })
            }
            // PaintSweepGradient / PaintVarSweepGradient
            8 | 9 => {
                let variable = format == 9;
                let color_line = self.paint_color_line(p, 1, variable, coords)?;
                let vb = if variable {
                    read_u32(b, p + 12).ok()?
                } else {
                    0xFFFF_FFFF
                };
                let cx = read_i16(b, p + 4).ok()?;
                let cy = read_i16(b, p + 6).ok()?;
                let sa = read_i16(b, p + 8).ok()?;
                let ea = read_i16(b, p + 10).ok()?;
                let (d0, d1, d2, d3) = if variable {
                    (
                        self.var_delta(vb, 0, coords),
                        self.var_delta(vb, 1, coords),
                        self.var_delta(vb, 2, coords),
                        self.var_delta(vb, 3, coords),
                    )
                } else {
                    (0.0, 0.0, 0.0, 0.0)
                };
                // Sweep angles carry the +1.0 bias: degrees =
                // (value + 1.0) × 180, counter-clockwise.
                Some(Paint::SweepGradient {
                    color_line,
                    center_x: cx as f32 + d0,
                    center_y: cy as f32 + d1,
                    start_angle_degrees: (f2dot14_var(sa, d2) + 1.0) * 180.0,
                    end_angle_degrees: (f2dot14_var(ea, d3) + 1.0) * 180.0,
                })
            }
            // PaintGlyph
            10 => {
                let child = self.child_paint(p, 1)?;
                let glyph_id = read_u16(b, p + 4).ok()?;
                Some(Paint::Glyph {
                    paint: child,
                    glyph_id,
                })
            }
            // PaintColrGlyph
            11 => {
                let glyph_id = read_u16(b, p + 1).ok()?;
                Some(Paint::ColrGlyph { glyph_id })
            }
            // PaintTransform / PaintVarTransform
            12 | 13 => {
                let child = self.child_paint(p, 1)?;
                let t_rel = read_u24(b, p + 4).ok()?;
                if t_rel == 0 {
                    return None;
                }
                let t = (p as u64).checked_add(t_rel as u64)?;
                if t >= b.len() as u64 {
                    return None;
                }
                let t = t as usize;
                let vb = if format == 13 {
                    read_u32(b, t + 24).ok()?
                } else {
                    0xFFFF_FFFF
                };
                let mut v = [0.0f32; 6];
                for (i, slot) in v.iter_mut().enumerate() {
                    let raw = crate::parser::read_i32(b, t + i * 4).ok()?;
                    let d = if format == 13 {
                        self.var_delta(vb, i as u32, coords)
                    } else {
                        0.0
                    };
                    // Fixed (16.16): deltas are integers in 1/65536
                    // wire steps.
                    *slot = (raw as f32 + d) / 65536.0;
                }
                Some(Paint::Transform {
                    paint: child,
                    transform: Affine2x3 {
                        xx: v[0],
                        yx: v[1],
                        xy: v[2],
                        yy: v[3],
                        dx: v[4],
                        dy: v[5],
                    },
                })
            }
            // PaintTranslate / PaintVarTranslate
            14 | 15 => {
                let child = self.child_paint(p, 1)?;
                let dx = read_i16(b, p + 4).ok()?;
                let dy = read_i16(b, p + 6).ok()?;
                let (d0, d1) = if format == 15 {
                    let vb = read_u32(b, p + 8).ok()?;
                    (self.var_delta(vb, 0, coords), self.var_delta(vb, 1, coords))
                } else {
                    (0.0, 0.0)
                };
                Some(Paint::Translate {
                    paint: child,
                    dx: dx as f32 + d0,
                    dy: dy as f32 + d1,
                })
            }
            // PaintScale family (16..=23)
            16..=23 => {
                let child = self.child_paint(p, 1)?;
                let uniform = format >= 20;
                let around_center = matches!(format, 18 | 19 | 22 | 23);
                let variable = format % 2 == 1;
                // Field layout after the child offset: scale factors
                // (1 or 2 × F2DOT14), then optional centre (2 × FWORD),
                // then optional varIndexBase.
                let n_scales = if uniform { 1 } else { 2 };
                let mut off = p + 4;
                let mut raw = [0i16; 4];
                let n_fields = n_scales + if around_center { 2 } else { 0 };
                for slot in raw.iter_mut().take(n_fields) {
                    *slot = read_i16(b, off).ok()?;
                    off += 2;
                }
                let vb = if variable {
                    read_u32(b, off).ok()?
                } else {
                    0xFFFF_FFFF
                };
                let d = |i: u32| -> f32 {
                    if variable {
                        self.var_delta(vb, i, coords)
                    } else {
                        0.0
                    }
                };
                let scale_x = f2dot14_var(raw[0], d(0));
                let scale_y = if uniform {
                    scale_x
                } else {
                    f2dot14_var(raw[1], d(1))
                };
                let (center_x, center_y) = if around_center {
                    let ci = n_scales as u32;
                    (
                        raw[n_scales] as f32 + d(ci),
                        raw[n_scales + 1] as f32 + d(ci + 1),
                    )
                } else {
                    (0.0, 0.0)
                };
                Some(Paint::Scale {
                    paint: child,
                    scale_x,
                    scale_y,
                    center_x,
                    center_y,
                })
            }
            // PaintRotate family (24..=27)
            24..=27 => {
                let child = self.child_paint(p, 1)?;
                let around_center = format >= 26;
                let variable = format % 2 == 1;
                let angle = read_i16(b, p + 4).ok()?;
                let (cx, cy) = if around_center {
                    (read_i16(b, p + 6).ok()?, read_i16(b, p + 8).ok()?)
                } else {
                    (0, 0)
                };
                let vb_off = if around_center { p + 10 } else { p + 6 };
                let vb = if variable {
                    read_u32(b, vb_off).ok()?
                } else {
                    0xFFFF_FFFF
                };
                let d = |i: u32| -> f32 {
                    if variable {
                        self.var_delta(vb, i, coords)
                    } else {
                        0.0
                    }
                };
                // No bias for rotate angles: degrees = value × 180.
                Some(Paint::Rotate {
                    paint: child,
                    angle_degrees: f2dot14_var(angle, d(0)) * 180.0,
                    center_x: if around_center { cx as f32 + d(1) } else { 0.0 },
                    center_y: if around_center { cy as f32 + d(2) } else { 0.0 },
                })
            }
            // PaintSkew family (28..=31)
            28..=31 => {
                let child = self.child_paint(p, 1)?;
                let around_center = format >= 30;
                let variable = format % 2 == 1;
                let xa = read_i16(b, p + 4).ok()?;
                let ya = read_i16(b, p + 6).ok()?;
                let (cx, cy) = if around_center {
                    (read_i16(b, p + 8).ok()?, read_i16(b, p + 10).ok()?)
                } else {
                    (0, 0)
                };
                let vb_off = if around_center { p + 12 } else { p + 8 };
                let vb = if variable {
                    read_u32(b, vb_off).ok()?
                } else {
                    0xFFFF_FFFF
                };
                let d = |i: u32| -> f32 {
                    if variable {
                        self.var_delta(vb, i, coords)
                    } else {
                        0.0
                    }
                };
                Some(Paint::Skew {
                    paint: child,
                    x_skew_degrees: f2dot14_var(xa, d(0)) * 180.0,
                    y_skew_degrees: f2dot14_var(ya, d(1)) * 180.0,
                    center_x: if around_center { cx as f32 + d(2) } else { 0.0 },
                    center_y: if around_center { cy as f32 + d(3) } else { 0.0 },
                })
            }
            // PaintComposite
            32 => {
                let source = self.child_paint(p, 1)?;
                let mode = CompositeMode::from_wire(read_u8(b, p + 4).ok()?);
                let backdrop = self.child_paint(p, 5)?;
                Some(Paint::Composite {
                    source,
                    mode,
                    backdrop,
                })
            }
            // Unrecognised paint formats should be ignored (future
            // minor versions may add formats).
            _ => None,
        }
    }
}

/// Resolve an F2DOT14 wire value plus an integer wire-unit delta into
/// its real value.
#[inline]
fn f2dot14_var(raw: i16, delta: f32) -> f32 {
    (raw as f32 + delta) / 16384.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-build a 4-byte-aligned COLR v0 fragment with one base
    /// glyph (gid 65) that points at three layers.
    fn synth_colr_one_base_three_layers() -> Vec<u8> {
        // Header (14 B) + 1 BaseGlyphRecord (6 B) + 3 LayerRecord (12 B) = 32 B
        let mut bytes = vec![0u8; 32];
        // version = 0
        bytes[0..2].copy_from_slice(&0u16.to_be_bytes());
        // numBaseGlyphRecords = 1
        bytes[2..4].copy_from_slice(&1u16.to_be_bytes());
        // baseGlyphRecordsOffset = 14
        bytes[4..8].copy_from_slice(&14u32.to_be_bytes());
        // layerRecordsOffset = 20
        bytes[8..12].copy_from_slice(&20u32.to_be_bytes());
        // numLayerRecords = 3
        bytes[12..14].copy_from_slice(&3u16.to_be_bytes());

        // BaseGlyphRecord at +14: glyphID=65, firstLayerIndex=0, numLayers=3
        bytes[14..16].copy_from_slice(&65u16.to_be_bytes());
        bytes[16..18].copy_from_slice(&0u16.to_be_bytes());
        bytes[18..20].copy_from_slice(&3u16.to_be_bytes());

        // LayerRecord[0..3] at +20
        // Layer 0: glyphID=100, paletteIndex=2
        bytes[20..22].copy_from_slice(&100u16.to_be_bytes());
        bytes[22..24].copy_from_slice(&2u16.to_be_bytes());
        // Layer 1: glyphID=101, paletteIndex=5
        bytes[24..26].copy_from_slice(&101u16.to_be_bytes());
        bytes[26..28].copy_from_slice(&5u16.to_be_bytes());
        // Layer 2: glyphID=102, paletteIndex=0xFFFF (foreground)
        bytes[28..30].copy_from_slice(&102u16.to_be_bytes());
        bytes[30..32].copy_from_slice(&0xFFFFu16.to_be_bytes());
        bytes
    }

    #[test]
    fn parses_v0_header() {
        let bytes = synth_colr_one_base_three_layers();
        let colr = ColrTable::parse(&bytes).expect("parse");
        assert_eq!(colr.num_base_records(), 1);
        assert!(!colr.has_paint_graph());
        assert!(!colr.has_variations());
    }

    #[test]
    fn layers_for_known_base_glyph() {
        let bytes = synth_colr_one_base_three_layers();
        let colr = ColrTable::parse(&bytes).expect("parse");
        let layers = colr.layers(65);
        assert_eq!(
            layers,
            vec![
                ColorLayer {
                    layer_glyph_id: 100,
                    palette_index: 2
                },
                ColorLayer {
                    layer_glyph_id: 101,
                    palette_index: 5
                },
                ColorLayer {
                    layer_glyph_id: 102,
                    palette_index: 0xFFFF
                },
            ]
        );
    }

    #[test]
    fn layers_for_non_base_glyph_is_empty() {
        let bytes = synth_colr_one_base_three_layers();
        let colr = ColrTable::parse(&bytes).expect("parse");
        assert!(colr.layers(0).is_empty());
        assert!(colr.layers(64).is_empty());
        assert!(colr.layers(66).is_empty());
        assert!(colr.layers(0xFFFF).is_empty());
    }

    #[test]
    fn rejects_truncated_header() {
        assert!(matches!(
            ColrTable::parse(&[0u8; 10]),
            Err(Error::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_array_past_end() {
        let mut bytes = vec![0u8; 14];
        // numBaseGlyphRecords = 1, baseRecordsOffset = 14 (but no data after).
        bytes[2..4].copy_from_slice(&1u16.to_be_bytes());
        bytes[4..8].copy_from_slice(&14u32.to_be_bytes());
        assert!(matches!(ColrTable::parse(&bytes), Err(Error::BadOffset)));
    }

    /// Three base glyphs with random-but-sorted gids: verify binary
    /// search lands on the correct middle / left / right elements.
    #[test]
    fn binary_search_three_records() {
        let mut bytes = vec![0u8; 14 + 18 + 12];
        bytes[0..2].copy_from_slice(&0u16.to_be_bytes());
        bytes[2..4].copy_from_slice(&3u16.to_be_bytes());
        bytes[4..8].copy_from_slice(&14u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&32u32.to_be_bytes());
        bytes[12..14].copy_from_slice(&3u16.to_be_bytes());

        // Records (sorted by gid): 10/0/1, 50/1/1, 200/2/1
        let recs: [(u16, u16, u16); 3] = [(10, 0, 1), (50, 1, 1), (200, 2, 1)];
        for (i, (g, first, count)) in recs.iter().enumerate() {
            let off = 14 + i * 6;
            bytes[off..off + 2].copy_from_slice(&g.to_be_bytes());
            bytes[off + 2..off + 4].copy_from_slice(&first.to_be_bytes());
            bytes[off + 4..off + 6].copy_from_slice(&count.to_be_bytes());
        }
        // Layers: gid 1000+i / palette i
        for i in 0..3 {
            let off = 32 + i * 4;
            bytes[off..off + 2].copy_from_slice(&(1000 + i as u16).to_be_bytes());
            bytes[off + 2..off + 4].copy_from_slice(&(i as u16).to_be_bytes());
        }

        let colr = ColrTable::parse(&bytes).expect("parse");
        // Hits
        for (gid, _first, _count) in &recs {
            let layers = colr.layers(*gid);
            assert_eq!(layers.len(), 1, "gid {gid}");
        }
        // Misses
        assert!(colr.layers(0).is_empty());
        assert!(colr.layers(11).is_empty());
        assert!(colr.layers(199).is_empty());
        assert!(colr.layers(201).is_empty());
    }
}
