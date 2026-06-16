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
