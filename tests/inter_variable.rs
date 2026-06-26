//! Integration tests against InterVariable.ttf — a real OFL-licensed
//! variable font with two axes (`opsz`, `wght`) and 9 named instances.
//! Fixture copyright belongs to The Inter Project Authors; redistribution
//! is governed by `tests/fixtures/INTER-OFL-LICENSE.txt`.
//!
//! These tests are the only place we touch a real `gvar` / `fvar` /
//! `avar` triple; the per-table unit tests in `src/tables/*.rs` use
//! hand-built byte buffers.

use oxideav_ttf::Font;

const FONT: &[u8] = include_bytes!("fixtures/InterVariable.ttf");

#[test]
fn parses_inter_variable() {
    let font = Font::from_bytes(FONT).expect("parse Inter variable");
    assert!(font.is_variable());
    assert_eq!(font.variation_axes().len(), 2);
    let tags: Vec<&[u8; 4]> = font.variation_axes().iter().map(|a| &a.tag).collect();
    assert!(tags.contains(&b"opsz"));
    assert!(tags.contains(&b"wght"));
}

#[test]
fn fvar_publishes_wght_range() {
    let font = Font::from_bytes(FONT).unwrap();
    let wght = font
        .variation_axes()
        .iter()
        .find(|a| &a.tag == b"wght")
        .expect("Inter ships a wght axis");
    assert_eq!(wght.min, 100.0);
    assert_eq!(wght.default, 400.0);
    assert_eq!(wght.max, 900.0);
}

#[test]
fn fvar_named_instances_present() {
    let font = Font::from_bytes(FONT).unwrap();
    // Inter ships 9 instances (Thin, ExtraLight, Light, Regular,
    // Medium, SemiBold, Bold, ExtraBold, Black).
    assert_eq!(font.named_instances().len(), 9);
}

#[test]
fn variation_coords_default_to_axis_defaults() {
    let font = Font::from_bytes(FONT).unwrap();
    let coords: Vec<f32> = font.variation_coords().to_vec();
    assert_eq!(coords.len(), 2);
    let axes = font.variation_axes();
    for (i, c) in coords.iter().enumerate() {
        assert_eq!(*c, axes[i].default);
    }
}

#[test]
fn set_variation_coords_clamps() {
    let mut font = Font::from_bytes(FONT).unwrap();
    // Push wght way over 900 — must clamp to 900.
    let wght_index = font
        .variation_axes()
        .iter()
        .position(|a| &a.tag == b"wght")
        .unwrap();
    let mut new_coords = font.variation_coords().to_vec();
    new_coords[wght_index] = 5000.0;
    font.set_variation_coords(&new_coords);
    assert_eq!(font.variation_coords()[wght_index], 900.0);
}

#[test]
fn normalised_coords_at_default_are_zero() {
    let font = Font::from_bytes(FONT).unwrap();
    let n = font.normalised_coords();
    assert_eq!(n.len(), 2);
    for v in &n {
        assert!(v.abs() < 1e-6, "expected 0 at axis default, got {}", v);
    }
}

#[test]
fn normalised_coords_at_max_are_one_after_avar() {
    let mut font = Font::from_bytes(FONT).unwrap();
    let wght_index = font
        .variation_axes()
        .iter()
        .position(|a| &a.tag == b"wght")
        .unwrap();
    let mut nc = font.variation_coords().to_vec();
    nc[wght_index] = 900.0;
    font.set_variation_coords(&nc);
    let normalised = font.normalised_coords();
    assert!(
        (normalised[wght_index] - 1.0).abs() < 1e-3,
        "wght=max must normalise to +1.0, got {}",
        normalised[wght_index]
    );
}

#[test]
fn gvar_full_weight_axis_changes_x_coords() {
    // Decode 'A' at wght=400 (default) and wght=900 (heaviest);
    // the heavy outline must differ from the default outline. In
    // a normal variable font like Inter the bold weight pushes the
    // stems wider — measured in font units the per-point deltas are
    // non-zero on at least one axis.
    let mut font = Font::from_bytes(FONT).unwrap();
    let gid = font.glyph_index('A').expect("A glyph");
    let regular = font.glyph_outline(gid).expect("regular outline");
    assert!(!regular.contours.is_empty());

    let wght_index = font
        .variation_axes()
        .iter()
        .position(|a| &a.tag == b"wght")
        .unwrap();
    let mut nc = font.variation_coords().to_vec();
    nc[wght_index] = 900.0;
    font.set_variation_coords(&nc);

    let bold = font.glyph_outline(gid).expect("bold outline");
    assert_eq!(
        bold.contours.len(),
        regular.contours.len(),
        "weight axis must not change topology"
    );

    // At least one point must have moved in x or y between the two
    // weights.
    let mut any_diff = false;
    for (rc, bc) in regular.contours.iter().zip(bold.contours.iter()) {
        for (rp, bp) in rc.points.iter().zip(bc.points.iter()) {
            if rp.x != bp.x || rp.y != bp.y {
                any_diff = true;
                break;
            }
        }
        if any_diff {
            break;
        }
    }
    assert!(
        any_diff,
        "expected at least one point delta between wght=400 and wght=900"
    );
}

#[test]
fn light_weight_also_differs_from_regular() {
    // Symmetric check: the negative-axis path (wght=100 < default
    // 400) must also return a different outline.
    let mut font = Font::from_bytes(FONT).unwrap();
    let gid = font.glyph_index('A').unwrap();
    let regular = font.glyph_outline(gid).unwrap();
    let wght_index = font
        .variation_axes()
        .iter()
        .position(|a| &a.tag == b"wght")
        .unwrap();
    let mut nc = font.variation_coords().to_vec();
    nc[wght_index] = 100.0;
    font.set_variation_coords(&nc);
    let thin = font.glyph_outline(gid).unwrap();
    let any_diff = regular
        .contours
        .iter()
        .zip(thin.contours.iter())
        .any(|(rc, tc)| {
            rc.points
                .iter()
                .zip(tc.points.iter())
                .any(|(rp, tp)| rp.x != tp.x || rp.y != tp.y)
        });
    assert!(any_diff, "wght=100 must differ from wght=400");
}

/// Helper: the `wght` axis index for InterVariable.
fn wght_axis(font: &Font) -> usize {
    font.variation_axes()
        .iter()
        .position(|a| &a.tag == b"wght")
        .expect("Inter ships a wght axis")
}

/// Helper: a glyph's outline at a given `wght` instance.
fn outline_at_wght(ch: char, wght: f32) -> oxideav_ttf::TtOutline {
    let mut font = Font::from_bytes(FONT).unwrap();
    let wi = wght_axis(&font);
    let gid = font.glyph_index(ch).expect("glyph present");
    let mut nc = font.variation_coords().to_vec();
    nc[wi] = wght;
    font.set_variation_coords(&nc);
    font.glyph_outline(gid).expect("outline")
}

/// A composite accented glyph ('é' = base 'e' + acute accent) must vary
/// under `wght` through the §7.3.4.3 composite-glyph path: each
/// component glyph is resolved with *its own* gvar deltas applied and
/// then placed at its (delta-adjusted) component offset. The
/// pre-§7.3.4.3 path mis-indexed component-addressed gvar deltas as
/// flattened outline points and could not reproduce this.
#[test]
fn composite_glyph_components_vary_under_weight() {
    let regular = outline_at_wght('é', 400.0);
    let bold = outline_at_wght('é', 900.0);

    // 'é' in Inter is a two-contour composite (base 'e' + accent).
    assert!(
        regular.contours.len() >= 2,
        "expected é to be a multi-contour composite"
    );
    assert_eq!(
        bold.contours.len(),
        regular.contours.len(),
        "weight axis must not change composite topology"
    );

    // The composite must actually move under the weight axis.
    let any_diff = regular
        .contours
        .iter()
        .zip(bold.contours.iter())
        .any(|(rc, bc)| {
            rc.points
                .iter()
                .zip(bc.points.iter())
                .any(|(rp, bp)| rp.x != bp.x || rp.y != bp.y)
        });
    assert!(
        any_diff,
        "composite é must vary under wght via the component path"
    );
}

/// The decisive §7.3.4.3 correctness anchor: the base component of a
/// variable composite must carry its **own** variation. The base 'e'
/// sub-outline embedded in a varied 'é' must equal the standalone
/// varied 'e' outline up to a single constant component-placement
/// offset — i.e. the component glyph was re-decoded with its gvar
/// deltas applied, not copied from a static cache. (A
/// flattened-point-only implementation cannot satisfy this, because it
/// would apply the parent's component-indexed deltas to the wrong
/// outline points and the base sub-outline would diverge from
/// standalone 'e'.)
#[test]
fn composite_base_component_matches_standalone_varied_glyph() {
    for &wght in &[100.0f32, 400.0, 900.0] {
        let e = outline_at_wght('e', wght);
        let eacute = outline_at_wght('é', wght);
        let ec = e.contours.first().expect("e has a contour");
        let base = eacute.contours.first().expect("é has a base contour");
        assert_eq!(
            base.points.len(),
            ec.points.len(),
            "base component point count must match standalone 'e' at wght={wght}"
        );
        // The base sub-outline must equal standalone 'e' shifted by a
        // single constant (dx, dy) placement offset.
        let d = (
            base.points[0].x as i32 - ec.points[0].x as i32,
            base.points[0].y as i32 - ec.points[0].y as i32,
        );
        let coherent = ec
            .points
            .iter()
            .zip(base.points.iter())
            .all(|(p, q)| (q.x as i32 - p.x as i32, q.y as i32 - p.y as i32) == d);
        assert!(
            coherent,
            "at wght={wght} the base component of 'é' must be the \
             standalone varied 'e' outline translated by a single \
             placement offset {d:?} (§7.3.4.3 per-component variation)"
        );
    }
}

/// IUP correctness anchor (§7.3.4.4 "Inferred deltas for un-referenced
/// point numbers"). Real variable fonts like Inter encode their `gvar`
/// tuples with *partial* point-number sets: only the structurally
/// important points carry explicit deltas, and the rest are inferred by
/// interpolation along each contour. Before inferred-delta support,
/// un-referenced points stayed pinned to their default positions,
/// shearing the outline. The decisive observable is that the **vast
/// majority** of a glyph's points move under a strong weight change —
/// not merely the small referenced subset.
#[test]
fn iup_moves_unreferenced_points_majority() {
    // 'e' is a smooth two-curve glyph with many off-curve control
    // points the font does not list explicitly in every tuple.
    let regular = outline_at_wght('e', 400.0);
    let bold = outline_at_wght('e', 900.0);

    assert_eq!(
        bold.contours.len(),
        regular.contours.len(),
        "weight must not change topology"
    );

    let mut total = 0usize;
    let mut moved = 0usize;
    for (rc, bc) in regular.contours.iter().zip(bold.contours.iter()) {
        for (rp, bp) in rc.points.iter().zip(bc.points.iter()) {
            total += 1;
            if rp.x != bp.x || rp.y != bp.y {
                moved += 1;
            }
        }
    }
    assert!(total > 0, "'e' must have points");
    // With IUP, a heavy weight change displaces nearly every point. A
    // broken implementation that only moves explicitly-referenced
    // points would leave a large fraction pinned. Require a strong
    // majority (>60%) to have moved.
    assert!(
        moved * 100 >= total * 60,
        "expected the majority of points to move under wght (IUP); \
         only {moved}/{total} moved"
    );
}

/// IUP must keep an outline *coherent*: between two referenced anchor
/// points an inferred point's displacement is bounded by the
/// displacements of its neighbours (linear interpolation never
/// overshoots the [min, max] of the two endpoints). We verify the
/// weaker but robust property that no inferred point flies far outside
/// the glyph's varied bounding box — a smeared, un-interpolated outline
/// would blow the bbox out. We compare the heavy-weight bbox derived
/// from the points to the reported bounds.
#[test]
fn iup_keeps_outline_within_derived_bounds() {
    let bold = outline_at_wght('e', 900.0);
    let bounds = bold.bounds.expect("varied outline has bounds");
    for c in &bold.contours {
        for p in &c.points {
            assert!(
                p.x >= bounds.x_min && p.x <= bounds.x_max,
                "point x {} outside varied bbox [{}, {}]",
                p.x,
                bounds.x_min,
                bounds.x_max
            );
            assert!(
                p.y >= bounds.y_min && p.y <= bounds.y_max,
                "point y {} outside varied bbox [{}, {}]",
                p.y,
                bounds.y_min,
                bounds.y_max
            );
        }
    }
}

/// Symmetry: IUP must work on the negative-axis (light) side too — the
/// thin instance must also move the majority of points.
#[test]
fn iup_moves_majority_on_light_side() {
    let regular = outline_at_wght('o', 400.0);
    let thin = outline_at_wght('o', 100.0);
    let mut total = 0usize;
    let mut moved = 0usize;
    for (rc, tc) in regular.contours.iter().zip(thin.contours.iter()) {
        for (rp, tp) in rc.points.iter().zip(tc.points.iter()) {
            total += 1;
            if rp.x != tp.x || rp.y != tp.y {
                moved += 1;
            }
        }
    }
    assert!(total > 0);
    assert!(
        moved * 100 >= total * 60,
        "expected majority of 'o' points to move at wght=100 (IUP); \
         {moved}/{total}"
    );
}

// ----- Ergonomic instance API ---------------------------------------

#[test]
fn set_axis_value_by_tag_updates_only_that_axis() {
    let mut font = Font::from_bytes(FONT).unwrap();
    let before: Vec<f32> = font.variation_coords().to_vec();
    // Set wght by tag and confirm it took (and is clamped to range).
    assert!(font.set_axis_value(b"wght", 700.0));
    assert_eq!(font.axis_value(b"wght"), Some(700.0));
    // Other axes (if any) are unchanged.
    let wi = font.axis_index(b"wght").unwrap();
    for (i, &v) in before.iter().enumerate() {
        if i != wi {
            assert_eq!(font.variation_coords()[i], v, "axis {i} must be untouched");
        }
    }
    // Out-of-range clamps to the axis max rather than overshooting.
    let max = font.variation_axes()[wi].max;
    assert!(font.set_axis_value(b"wght", 100000.0));
    assert_eq!(font.axis_value(b"wght"), Some(max));
    // Unknown tag is a no-op returning false.
    assert!(!font.set_axis_value(b"zzzz", 1.0));
    assert_eq!(font.axis_value(b"zzzz"), None);
}

#[test]
fn set_axis_value_by_tag_drives_outline() {
    // Driving wght by tag must produce the same outline as driving it by
    // index through set_variation_coords.
    let gid;
    let by_tag = {
        let mut font = Font::from_bytes(FONT).unwrap();
        gid = font.glyph_index('A').unwrap();
        assert!(font.set_axis_value(b"wght", 900.0));
        font.glyph_outline(gid).unwrap()
    };
    let by_index = outline_at_wght('A', 900.0);
    assert_eq!(by_tag.contours.len(), by_index.contours.len());
    for (a, b) in by_tag.contours.iter().zip(by_index.contours.iter()) {
        assert_eq!(a.points.len(), b.points.len());
        for (pa, pb) in a.points.iter().zip(b.points.iter()) {
            assert_eq!((pa.x, pa.y), (pb.x, pb.y));
        }
    }
}

#[test]
fn apply_named_instance_sets_coords() {
    let mut font = Font::from_bytes(FONT).unwrap();
    let n = font.named_instances().len();
    assert!(n > 0, "Inter ships named instances");
    // Apply the last named instance and confirm the coords now equal its
    // stored (clamped) coordinate vector.
    let last = n - 1;
    let expected: Vec<f32> = {
        let inst = &font.named_instances()[last];
        let axes = font.variation_axes();
        inst.coords
            .iter()
            .enumerate()
            .map(|(i, &v)| v.clamp(axes[i].min, axes[i].max))
            .collect()
    };
    assert!(font.apply_named_instance(last));
    assert_eq!(font.variation_coords(), expected.as_slice());
    // Out-of-range index is a no-op returning false.
    let snapshot: Vec<f32> = font.variation_coords().to_vec();
    assert!(!font.apply_named_instance(n + 100));
    assert_eq!(font.variation_coords(), snapshot.as_slice());
}

#[test]
fn axis_helpers_are_noops_on_static_font() {
    // DejaVuSans is static — every variable helper must report absence.
    const STATIC: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");
    let mut font = Font::from_bytes(STATIC).unwrap();
    assert_eq!(font.axis_index(b"wght"), None);
    assert_eq!(font.axis_value(b"wght"), None);
    assert!(!font.set_axis_value(b"wght", 700.0));
    assert!(!font.apply_named_instance(0));
}

#[test]
fn hvar_varies_advance_width_under_weight() {
    // Inter ships an HVAR table; bumping wght to its max should change
    // at least one glyph's varied advance width relative to the static
    // hmtx advance. We scan a handful of common Latin glyphs and assert
    // the fused accessor diverges for at least one of them.
    let mut font = Font::from_bytes(FONT).unwrap();
    let wght_index = font
        .variation_axes()
        .iter()
        .position(|a| &a.tag == b"wght")
        .unwrap();

    // At the default instance, varied == static for every glyph.
    for ch in "HOanmwgij".chars() {
        if let Some(gid) = font.glyph_index(ch) {
            assert_eq!(
                font.glyph_advance_varied(gid),
                font.glyph_advance(gid),
                "default instance must not vary advance for {ch}"
            );
        }
    }

    let mut nc = font.variation_coords().to_vec();
    nc[wght_index] = 900.0;
    font.set_variation_coords(&nc);

    let mut any_diff = false;
    for ch in "HOanmwgij".chars() {
        if let Some(gid) = font.glyph_index(ch) {
            // The HVAR delta accessor must resolve (Inter maps every gid).
            assert!(font.advance_width_variation_delta(gid).is_some());
            if font.glyph_advance_varied(gid) != font.glyph_advance(gid) {
                any_diff = true;
            }
        }
    }
    assert!(
        any_diff,
        "expected HVAR to vary at least one advance width at wght=900"
    );
}

#[test]
fn varied_advance_equals_static_on_font_without_hvar() {
    // DejaVuSans is static (no HVAR): glyph_advance_varied must mirror
    // glyph_advance exactly, and glyph_lsb_varied must mirror glyph_lsb.
    const STATIC: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");
    let font = Font::from_bytes(STATIC).unwrap();
    let gid = font.glyph_index('A').expect("A glyph");
    assert_eq!(font.glyph_advance_varied(gid), font.glyph_advance(gid));
    assert_eq!(font.glyph_lsb_varied(gid), font.glyph_lsb(gid));
    // No vertical metrics → varied height is None.
    assert_eq!(font.glyph_advance_height_varied(gid), None);
}

#[test]
fn var_kerning_equals_static_at_default_instance() {
    // At the default instance, the variation-aware GPOS kerning must
    // match the static path (every VariationIndex region scalar is 0
    // at the default coordinate). This exercises the new
    // Font::lookup_kerning_var plumbing end-to-end against a real
    // variable font's GDEF ItemVariationStore (if present) without
    // assuming Inter ships variable kerning.
    let mut font = Font::from_bytes(FONT).unwrap();
    // Default instance.
    let defaults: Vec<f32> = font.variation_axes().iter().map(|a| a.default).collect();
    font.set_variation_coords(&defaults);

    // Scan a handful of common Latin pairs; for each, the var and
    // static kerning must agree at the default instance.
    let probe = ['A', 'V', 'T', 'o', 'W', 'a', 'e', '.', ',', 'f'];
    for &l in &probe {
        for &r in &probe {
            let (Some(lg), Some(rg)) = (font.glyph_index(l), font.glyph_index(r)) else {
                continue;
            };
            assert_eq!(
                font.lookup_kerning_var(lg, rg),
                font.lookup_kerning(lg, rg),
                "var/static kerning differ at default instance for {l:?}{r:?}"
            );
        }
    }
}

#[test]
fn var_gpos_wrappers_callable_across_instances() {
    // The variation-aware GPOS / GDEF accessors must be safe to call at
    // any instance without panicking, for any glyph id. This guards the
    // device-offset resolution plumbing against out-of-range
    // ItemVariationStore indices in a real font.
    let mut font = Font::from_bytes(FONT).unwrap();
    let defaults: Vec<f32> = font.variation_axes().iter().map(|a| a.default).collect();
    let wght_idx = font
        .variation_axes()
        .iter()
        .position(|a| &a.tag == b"wght")
        .unwrap();

    for &coord in &[100.0f32, 400.0, 900.0] {
        let mut c = defaults.clone();
        c[wght_idx] = coord;
        font.set_variation_coords(&c);

        let a = font.glyph_index('A').unwrap();
        let grave = font.glyph_index('\u{0300}');
        // These all return Option/i16 and must never panic.
        let _ = font.lookup_kerning_var(a, a);
        let _ = font.gpos_apply_lookup_type_1_var(0, a);
        let _ = font.lookup_cursive_attachment_var(a);
        let _ = font.ligature_carets_resolved(a);
        if let Some(m) = grave {
            let _ = font.lookup_mark_to_base_var(a, m);
            let _ = font.lookup_mark_to_mark_var(m, m);
        }
    }
}
