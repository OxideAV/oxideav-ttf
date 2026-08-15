//! End-to-end validation against the bundled conformance-style
//! variable COLR v1 fixture
//! (`tests/fixtures/test_glyphs-glyf_colr_1_variable.ttf`, Apache-2.0
//! — see `COLOR-FONTS-APACHE-LICENSE.txt`): a purpose-built test font
//! whose 44 design axes each drive one paint-graph parameter (sweep
//! angles, scale centres, gradient geometry, rotation, skew,
//! transform matrix cells, clip boxes, alpha).
//!
//! The font's embedded `ItemVariationStore` carries an
//! `ItemVariationData` subtable with the `LONG_WORDS` flag set
//! (`wordDeltaCount = 0x8001` — the int32 + int16 delta
//! representation the OFF common-formats chapter reserves for
//! 32-bit-variable top-level tables, currently COLR only) plus a
//! format-0 `DeltaSetIndexMap`, so merely *loading* it proves the
//! chapter's variation-data surface: before `LONG_WORDS` support the
//! whole font failed to parse (the flag bit read as an impossible
//! word-delta count).

use std::collections::HashSet;

use oxideav_ttf::{Font, Paint, PaintRef};

const FIXTURE: &[u8] = include_bytes!("fixtures/test_glyphs-glyf_colr_1_variable.ttf");

fn font() -> Font<'static> {
    Font::from_bytes(FIXTURE).expect("variable COLR fixture must parse")
}

/// Every base glyph id carrying a v1 paint graph root.
fn base_glyphs(font: &Font) -> Vec<u16> {
    (0..font.glyph_count())
        .filter(|&gid| font.color_paint_root(gid).is_some())
        .collect()
}

/// Depth-bounded, cycle-checked walk of a paint graph. Appends each
/// decoded node's `Debug` form to `dump` (a cheap canonical form for
/// cross-instance comparison) and counts nodes that fail to decode.
fn walk(
    font: &Font,
    node: PaintRef,
    visited: &mut HashSet<PaintRef>,
    depth: usize,
    dump: &mut String,
    undecodable: &mut u32,
) {
    if depth > 64 || !visited.insert(node) {
        return;
    }
    let Some(paint) = font.color_paint(node) else {
        *undecodable += 1;
        return;
    };
    dump.push_str(&format!("{paint:?};"));
    let mut children: Vec<PaintRef> = Vec::new();
    match &paint {
        Paint::ColrLayers { layers } => children.extend(layers.iter().copied()),
        Paint::Glyph { paint, .. }
        | Paint::Transform { paint, .. }
        | Paint::Translate { paint, .. }
        | Paint::Scale { paint, .. }
        | Paint::Rotate { paint, .. }
        | Paint::Skew { paint, .. } => children.push(*paint),
        Paint::Composite {
            source, backdrop, ..
        } => {
            children.push(*source);
            children.push(*backdrop);
        }
        Paint::ColrGlyph { glyph_id } => {
            if let Some(root) = font.color_paint_root(*glyph_id) {
                children.push(root);
            } else {
                *undecodable += 1;
            }
        }
        Paint::Solid { .. }
        | Paint::LinearGradient { .. }
        | Paint::RadialGradient { .. }
        | Paint::SweepGradient { .. } => {}
    }
    for child in children {
        walk(font, child, visited, depth + 1, dump, undecodable);
    }
}

/// Full-graph canonical dump for one base glyph at the font's current
/// instance. Returns `(dump, undecodable_count)`.
fn graph_dump(font: &Font, gid: u16) -> (String, u32) {
    let mut dump = String::new();
    let mut undecodable = 0;
    let root = font.color_paint_root(gid).expect("base glyph");
    walk(
        font,
        root,
        &mut HashSet::new(),
        0,
        &mut dump,
        &mut undecodable,
    );
    (dump, undecodable)
}

/// Push every design axis to the given end of its range.
fn set_all_axes(font: &mut Font, to_max: bool) {
    let axes = font.variation_axes().to_vec();
    for axis in axes {
        let value = if to_max { axis.max } else { axis.min };
        assert!(font.set_axis_value(&axis.tag, value));
    }
}

#[test]
fn long_words_variable_colr_font_loads() {
    let font = font();
    assert!(font.has_colr_v1());
    assert!(!font.colr_var_index_map_unsupported());
    assert!(font.is_variable());
    assert_eq!(font.variation_axes().len(), 44);
    let bases = base_glyphs(&font);
    assert!(
        bases.len() > 50,
        "expected a rich BaseGlyphList, got {}",
        bases.len()
    );
}

#[test]
fn every_paint_graph_decodes_at_default_and_extreme_instances() {
    let mut font = font();
    let bases = base_glyphs(&font);
    for &gid in &bases {
        let (dump, undecodable) = graph_dump(&font, gid);
        assert_eq!(undecodable, 0, "gid {gid} has undecodable nodes");
        assert!(!dump.is_empty(), "gid {gid} produced an empty graph");
    }
    // Same walk with every axis at its extreme — the deltas (routed
    // through the varIndexMap and the LONG_WORDS store) must resolve
    // without panicking or breaking any node's decode.
    for &to_max in &[true, false] {
        set_all_axes(&mut font, to_max);
        for &gid in &bases {
            let (_, undecodable) = graph_dump(&font, gid);
            assert_eq!(undecodable, 0, "gid {gid} undecodable at extreme");
        }
    }
}

#[test]
fn variation_deltas_move_paint_values() {
    let mut font = font();
    let bases = base_glyphs(&font);
    let default_dumps: Vec<String> = bases.iter().map(|&g| graph_dump(&font, g).0).collect();
    set_all_axes(&mut font, true);
    let varied_dumps: Vec<String> = bases.iter().map(|&g| graph_dump(&font, g).0).collect();
    let moved = default_dumps
        .iter()
        .zip(&varied_dumps)
        .filter(|(a, b)| a != b)
        .count();
    // The font is purpose-built so its axes drive paint parameters:
    // a healthy share of the graphs must change at the axis extremes.
    assert!(
        moved >= bases.len() / 4,
        "only {moved} of {} paint graphs moved at the axis extremes",
        bases.len()
    );
}

#[test]
fn boundedness_analysis_completes_on_every_base_glyph() {
    let font = font();
    let bases = base_glyphs(&font);
    let mut bounded = 0usize;
    for &gid in &bases {
        match font.color_glyph_is_bounded(gid) {
            Some(true) => bounded += 1,
            Some(false) => {} // deliberate unbounded test glyphs are fine
            None => {
                // The fixture ships deliberate not-well-formed test
                // glyphs (a two-glyph PaintColrGlyph cycle) — their
                // glyph names say so. The analysis must fire on
                // exactly those, never on a legitimate graph.
                let name = font.glyph_name(gid).unwrap_or("");
                assert!(
                    name.contains("cycle"),
                    "gid {gid} ({name:?}) not well-formed but not a cycle fixture"
                );
            }
        }
    }
    assert!(
        bounded > bases.len() / 2,
        "only {bounded} of {} base glyphs bounded",
        bases.len()
    );
}

#[test]
fn clip_boxes_resolve_and_track_the_clip_axes() {
    let mut font = font();
    let with_box: Vec<(u16, _)> = (0..font.glyph_count())
        .filter_map(|gid| font.color_clip_box(gid).map(|b| (gid, b)))
        .collect();
    assert!(!with_box.is_empty(), "fixture ships a ClipList");
    // The CLXI / CLYI / CLXA / CLYA axes vary clip-box edges; at the
    // axis extremes at least one resolved box must differ from its
    // default-instance form.
    set_all_axes(&mut font, true);
    let moved = with_box
        .iter()
        .any(|(gid, default_box)| font.color_clip_box(*gid).as_ref() != Some(default_box));
    assert!(moved, "no clip box tracked the clip axes");
}

#[test]
fn outlines_and_metrics_resolve_at_extremes() {
    // The fixture also ships gvar + HVAR; a quick end-to-end smoke
    // proves the non-COLR variation plumbing coexists.
    let mut font = font();
    set_all_axes(&mut font, true);
    let mut outlines = 0;
    for gid in 0..font.glyph_count().min(64) {
        if font.glyph_outline(gid).is_ok() {
            outlines += 1;
        }
        let _ = font.glyph_advance_varied(gid);
    }
    assert!(outlines > 0);
}
