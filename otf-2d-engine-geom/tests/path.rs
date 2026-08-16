//! T1.1 path criteria: `rounded_rect` bounds equal the input rect, arcs become
//! cubics at build time, and `rect`/`rounded_rect` are recognised primitives
//! rather than sugar.

use otf_2d_engine_geom::{
    Affine, Path, PathBuilder, PathEl, PathSeg, PathShape, PathVerb, Point, Rect, RectRadii, Vec2,
};
use proptest::prelude::*;

fn rect_strategy() -> impl Strategy<Value = Rect> {
    (
        -1000.0f64..1000.0,
        -1000.0f64..1000.0,
        0.1f64..2000.0,
        0.1f64..2000.0,
    )
        .prop_map(|(x, y, w, h)| Rect::new(x, y, x + w, y + h))
}

fn radii_strategy() -> impl Strategy<Value = RectRadii> {
    (0.0f64..500.0, 0.0f64..500.0, 0.0f64..500.0, 0.0f64..500.0)
        .prop_map(|(a, b, c, d)| RectRadii::new(a, b, c, d))
}

fn assert_bounds_eq(actual: Rect, expected: Rect, what: &str) {
    let scale = expected.width().max(expected.height()).max(1.0);
    let tolerance = 1e-9 * scale;
    for (a, e, name) in [
        (actual.x0, expected.x0, "x0"),
        (actual.y0, expected.y0, "y0"),
        (actual.x1, expected.x1, "x1"),
        (actual.y1, expected.y1, "y1"),
    ] {
        assert!(
            (a - e).abs() <= tolerance,
            "{what}: {name} was {a}, expected {e}"
        );
    }
}

proptest! {
    /// The T1.1 criterion, stated exactly: a rounded rect's bounds are the
    /// rect it was built from. Clamping the radii is what makes this hold for
    /// every radius, including absurd ones.
    #[test]
    fn rounded_rect_bounds_equal_the_input_rect(r in rect_strategy(), radii in radii_strategy()) {
        let path = PathBuilder::new().rounded_rect(r, radii).build();
        assert_bounds_eq(path.control_bounds(), r, "rounded_rect control bounds");
    }

    /// Every point of the path, not just the extremes, lies inside the rect.
    #[test]
    fn rounded_rect_never_leaves_the_input_rect(r in rect_strategy(), radii in radii_strategy()) {
        let path = PathBuilder::new().rounded_rect(r, radii).build();
        let slack = 1e-9 * r.width().max(r.height()).max(1.0);
        for p in path.points() {
            prop_assert!(
                p.x >= r.x0 - slack && p.x <= r.x1 + slack
                    && p.y >= r.y0 - slack && p.y <= r.y1 + slack,
                "{p:?} escapes {r:?}"
            );
        }
    }

    #[test]
    fn rect_bounds_equal_the_input_rect(r in rect_strategy()) {
        let path = PathBuilder::new().rect(r).build();
        assert_bounds_eq(path.control_bounds(), r, "rect control bounds");
    }

    /// Clamping preserves circular corners: every radius scales by the same
    /// factor, which is what CSS `border-radius` does.
    #[test]
    fn clamping_scales_all_radii_uniformly(r in rect_strategy(), radii in radii_strategy()) {
        let clamped = radii.clamped_to(r);
        prop_assert!(clamped.top_left + clamped.top_right <= r.width() + 1e-9);
        prop_assert!(clamped.bottom_left + clamped.bottom_right <= r.width() + 1e-9);
        prop_assert!(clamped.top_left + clamped.bottom_left <= r.height() + 1e-9);
        prop_assert!(clamped.top_right + clamped.bottom_right <= r.height() + 1e-9);

        // Ratios between corners survive, up to the corners that were zero.
        let pairs = [
            (radii.top_left, clamped.top_left),
            (radii.top_right, clamped.top_right),
            (radii.bottom_right, clamped.bottom_right),
            (radii.bottom_left, clamped.bottom_left),
        ];
        let factors: Vec<f64> =
            pairs.iter().filter(|(o, _)| *o > 1e-6).map(|(o, c)| c / o).collect();
        if let Some(&first) = factors.first() {
            for f in &factors {
                prop_assert!((f - first).abs() < 1e-9, "radii scaled unevenly: {factors:?}");
            }
        }
    }

    /// An ellipse built from cubics stays within its radii and reaches them.
    #[test]
    fn ellipse_bounds_match_its_radii(
        cx in -100.0f64..100.0, cy in -100.0f64..100.0,
        rx in 0.5f64..500.0, ry in 0.5f64..500.0,
    ) {
        let center = Point::new(cx, cy);
        let path = PathBuilder::new().ellipse(center, Vec2::new(rx, ry)).build();
        let expected = Rect::new(cx - rx, cy - ry, cx + rx, cy + ry);
        // Control points of the corner cubics sit slightly outside the curve,
        // by the standard 4/3·tan(δ/4) handle length, so allow that much.
        let slack = 0.552 * rx.max(ry) + 1e-9;
        let bounds = path.control_bounds();
        prop_assert!(bounds.x0 >= expected.x0 - slack && bounds.x1 <= expected.x1 + slack);
        prop_assert!(bounds.y0 >= expected.y0 - slack && bounds.y1 <= expected.y1 + slack);
        // And the curve itself touches all four extremes.
        prop_assert!((bounds.x0 - expected.x0).abs() < 1e-9);
        prop_assert!((bounds.x1 - expected.x1).abs() < 1e-9);
        prop_assert!((bounds.y0 - expected.y0).abs() < 1e-9);
        prop_assert!((bounds.y1 - expected.y1).abs() < 1e-9);
    }

    /// The cubics an arc becomes track the true ellipse closely. The
    /// tangent-matching construction's worst radial error over a 90° span is
    /// about 2.7e-4 of the radius.
    #[test]
    fn arc_cubics_track_the_true_ellipse(
        radius in 1.0f64..1000.0,
        start in -6.3f64..6.3,
        sweep in -6.2f64..6.2,
    ) {
        prop_assume!(sweep.abs() > 1e-3);
        let center = Point::new(0.0, 0.0);
        let path = PathBuilder::new()
            .arc_to(center, Vec2::new(radius, radius), 0.0, start, sweep)
            .build();

        let mut worst: f64 = 0.0;
        for seg in path.segments() {
            let PathSeg::Cubic(p0, p1, p2, p3) = seg else { continue };
            for i in 0..=64 {
                let t = i as f64 / 64.0;
                let p = eval_cubic(p0, p1, p2, p3, t);
                worst = worst.max(((p.x * p.x + p.y * p.y).sqrt() - radius).abs());
            }
        }
        prop_assert!(worst <= 3e-4 * radius, "radial error {worst} on radius {radius}");
    }
}

fn eval_cubic(p0: Point, p1: Point, p2: Point, p3: Point, t: f64) -> Point {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Point::new(
        a * p0.x + b * p1.x + c * p2.x + d * p3.x,
        a * p0.y + b * p1.y + c * p2.y + d * p3.y,
    )
}

#[test]
fn a_new_path_is_empty_and_draws_nothing() {
    let path = Path::new();
    assert!(path.is_empty());
    assert_eq!(path.elements().count(), 0);
    assert_eq!(path.segments().count(), 0);
    assert_eq!(path.shape(), PathShape::General);
}

#[test]
fn rect_is_a_recognised_primitive() {
    let r = Rect::new(1.0, 2.0, 11.0, 22.0);
    let path = PathBuilder::new().rect(r).build();
    assert_eq!(path.shape(), PathShape::Rect(r));
    assert_eq!(
        path.verbs(),
        &[
            PathVerb::MoveTo,
            PathVerb::LineTo,
            PathVerb::LineTo,
            PathVerb::LineTo,
            PathVerb::ClosePath
        ]
    );
}

#[test]
fn rounded_rect_is_a_recognised_primitive_carrying_clamped_radii() {
    let r = Rect::new(0.0, 0.0, 10.0, 10.0);
    // 8 + 8 exceeds the 10-wide edge, so both scale by 10/16.
    let path = PathBuilder::new()
        .rounded_rect(r, RectRadii::uniform(8.0))
        .build();
    let PathShape::RoundedRect(shape_rect, radii) = path.shape() else {
        panic!("expected a rounded rect, got {:?}", path.shape());
    };
    assert_eq!(shape_rect, r);
    assert!((radii.top_left - 5.0).abs() < 1e-12, "{radii:?}");
    assert!((radii.bottom_right - 5.0).abs() < 1e-12, "{radii:?}");
}

#[test]
fn a_zero_radius_rounded_rect_is_still_flagged_as_one() {
    // The fast path wants to know it was asked for a rounded rect even when
    // the radii collapsed, so a consumer animating a radius to zero does not
    // fall off a cliff and back on again.
    let r = Rect::new(0.0, 0.0, 4.0, 4.0);
    let path = PathBuilder::new().rounded_rect(r, RectRadii::ZERO).build();
    assert!(matches!(path.shape(), PathShape::RoundedRect(_, _)));
    assert_eq!(path.control_bounds(), r);
}

#[test]
fn two_primitives_in_one_path_are_not_a_recognised_shape() {
    let mut b = PathBuilder::new();
    b.rect(Rect::new(0.0, 0.0, 1.0, 1.0));
    b.rect(Rect::new(2.0, 2.0, 3.0, 3.0));
    assert_eq!(b.build().shape(), PathShape::General);
}

#[test]
fn a_primitive_after_freehand_geometry_is_not_a_recognised_shape() {
    let mut b = PathBuilder::new();
    b.move_to((0.0, 0.0)).line_to((1.0, 1.0));
    b.rect(Rect::new(0.0, 0.0, 1.0, 1.0));
    assert_eq!(b.build().shape(), PathShape::General);
}

#[test]
fn a_non_normalised_rect_is_normalised_by_the_builder() {
    let path = PathBuilder::new()
        .rect(Rect::new(10.0, 20.0, 0.0, 5.0))
        .build();
    assert_eq!(
        path.shape(),
        PathShape::Rect(Rect::new(0.0, 5.0, 10.0, 20.0))
    );
    assert_eq!(path.control_bounds(), Rect::new(0.0, 5.0, 10.0, 20.0));
}

#[test]
fn there_are_no_arc_or_conic_verbs_in_the_ir() {
    // Doc 02 §3: exactly three curve types reach stages 3-5. An ellipse is
    // cubics by the time it is a Path.
    let path = PathBuilder::new().circle((0.0, 0.0), 10.0).build();
    assert!(!path.is_empty());
    for verb in path.verbs() {
        assert!(
            matches!(
                verb,
                PathVerb::MoveTo | PathVerb::CurveTo | PathVerb::ClosePath
            ),
            "unexpected verb {verb:?}"
        );
    }
    // A full circle needs four quadrant cubics.
    assert_eq!(
        path.verbs()
            .iter()
            .filter(|v| **v == PathVerb::CurveTo)
            .count(),
        4
    );
}

#[test]
fn elements_round_trip_through_the_builder() {
    let mut b = PathBuilder::new();
    b.move_to((0.0, 0.0))
        .line_to((10.0, 0.0))
        .quad_to((15.0, 5.0), (10.0, 10.0))
        .curve_to((5.0, 12.0), (2.0, 12.0), (0.0, 10.0))
        .close();
    let path = b.build();
    let elements: Vec<PathEl> = path.elements().collect();
    assert_eq!(
        elements,
        vec![
            PathEl::MoveTo(Point::new(0.0, 0.0)),
            PathEl::LineTo(Point::new(10.0, 0.0)),
            PathEl::QuadTo(Point::new(15.0, 5.0), Point::new(10.0, 10.0)),
            PathEl::CurveTo(
                Point::new(5.0, 12.0),
                Point::new(2.0, 12.0),
                Point::new(0.0, 10.0)
            ),
            PathEl::ClosePath,
        ]
    );

    let mut rebuilt = PathBuilder::new();
    rebuilt.extend_from_path(&path);
    assert_eq!(rebuilt.build().elements().collect::<Vec<_>>(), elements);
}

#[test]
fn closing_emits_the_line_back_to_the_subpath_start() {
    let mut b = PathBuilder::new();
    b.move_to((0.0, 0.0))
        .line_to((10.0, 0.0))
        .line_to((10.0, 10.0))
        .close();
    let segments: Vec<PathSeg> = b.build().segments().collect();
    assert_eq!(segments.len(), 3);
    assert_eq!(
        segments[2],
        PathSeg::Line(Point::new(10.0, 10.0), Point::new(0.0, 0.0))
    );
}

#[test]
fn closing_an_already_closed_subpath_emits_no_zero_length_line() {
    // A zero-length closing line would become a spurious join during stroke
    // expansion (T3.2), so it must not be emitted.
    let mut b = PathBuilder::new();
    b.move_to((0.0, 0.0))
        .line_to((10.0, 0.0))
        .line_to((0.0, 0.0))
        .close();
    let segments: Vec<PathSeg> = b.build().segments().collect();
    assert_eq!(segments.len(), 2, "{segments:?}");
}

#[test]
fn segments_track_the_subpath_start_across_multiple_subpaths() {
    let mut b = PathBuilder::new();
    b.move_to((0.0, 0.0)).line_to((1.0, 0.0)).close();
    b.move_to((5.0, 5.0)).line_to((6.0, 5.0)).close();
    let segments: Vec<PathSeg> = b.build().segments().collect();
    assert_eq!(segments.len(), 4);
    assert_eq!(
        segments[1],
        PathSeg::Line(Point::new(1.0, 0.0), Point::new(0.0, 0.0))
    );
    assert_eq!(
        segments[3],
        PathSeg::Line(Point::new(6.0, 5.0), Point::new(5.0, 5.0))
    );
}

#[test]
fn a_transformed_rect_keeps_its_shape_hint_when_axis_alignment_survives() {
    let r = Rect::new(0.0, 0.0, 10.0, 20.0);
    let path = PathBuilder::new().rect(r).build();

    let scaled = path.transformed(Affine::scale(2.0));
    assert_eq!(
        scaled.shape(),
        PathShape::Rect(Rect::new(0.0, 0.0, 20.0, 40.0))
    );

    let rotated = path.transformed(Affine::rotate(0.5));
    assert_eq!(rotated.shape(), PathShape::General);
}

#[test]
fn a_transformed_rounded_rect_keeps_its_hint_only_under_uniform_scale() {
    let r = Rect::new(0.0, 0.0, 10.0, 20.0);
    let path = PathBuilder::new()
        .rounded_rect(r, RectRadii::uniform(2.0))
        .build();

    let PathShape::RoundedRect(rect, radii) = path.transformed(Affine::scale(3.0)).shape() else {
        panic!("uniform scale should keep the hint");
    };
    assert_eq!(rect, Rect::new(0.0, 0.0, 30.0, 60.0));
    assert!((radii.top_left - 6.0).abs() < 1e-12);

    // A non-uniform scale turns circular corners into elliptical ones, which
    // the fast path cannot represent.
    assert_eq!(
        path.transformed(Affine::scale_non_uniform(2.0, 3.0))
            .shape(),
        PathShape::General
    );
}

#[test]
fn a_transformed_path_moves_every_point() {
    let mut b = PathBuilder::new();
    b.move_to((1.0, 2.0)).line_to((3.0, 4.0));
    let moved = b
        .build()
        .transformed(Affine::translate(Vec2::new(10.0, 20.0)));
    assert_eq!(
        moved.points(),
        &[Point::new(11.0, 22.0), Point::new(13.0, 24.0)]
    );
    assert_eq!(moved.control_bounds(), Rect::new(11.0, 22.0, 13.0, 24.0));
}

#[test]
fn is_finite_rejects_a_path_with_a_nan_coordinate() {
    let mut b = PathBuilder::new();
    b.move_to((0.0, 0.0)).line_to((f64::NAN, 1.0));
    assert!(!b.build().is_finite());

    let mut ok = PathBuilder::new();
    ok.move_to((0.0, 0.0)).line_to((1.0, 1.0));
    assert!(ok.build().is_finite());
}

#[test]
fn reset_clears_the_builder_including_its_shape_hint() {
    let mut b = PathBuilder::new();
    b.rect(Rect::new(0.0, 0.0, 1.0, 1.0));
    b.reset();
    assert!(b.is_empty());
    assert_eq!(b.build().shape(), PathShape::General);
    // And it is usable again, recognising a fresh primitive.
    b.rect(Rect::new(2.0, 2.0, 4.0, 4.0));
    assert_eq!(
        b.build().shape(),
        PathShape::Rect(Rect::new(2.0, 2.0, 4.0, 4.0))
    );
}

#[test]
fn build_leaves_the_builder_usable_and_finish_consumes_it() {
    let mut b = PathBuilder::new();
    b.rect(Rect::new(0.0, 0.0, 1.0, 1.0));
    let first = b.build();
    let second = b.build();
    assert_eq!(first, second);
    assert_eq!(b.finish(), first);
}

#[test]
fn a_degenerate_arc_appends_nothing() {
    let mut b = PathBuilder::new();
    b.move_to((0.0, 0.0));
    b.arc_to((0.0, 0.0), Vec2::new(0.0, 5.0), 0.0, 0.0, 1.0);
    // Zero radius: the line to the arc start is the only thing that can be
    // emitted, and the start coincides with the current point.
    assert_eq!(b.build().verbs(), &[PathVerb::MoveTo]);

    let mut c = PathBuilder::new();
    c.move_to((0.0, 0.0));
    c.arc_to((10.0, 0.0), Vec2::new(5.0, 5.0), 0.0, 0.0, 0.0);
    // Zero sweep: a line to the arc's start point, and no curve.
    assert_eq!(c.build().verbs(), &[PathVerb::MoveTo, PathVerb::LineTo]);
}

#[test]
fn arc_to_connects_from_the_current_point() {
    let mut b = PathBuilder::new();
    b.move_to((0.0, 0.0));
    b.arc_to(
        (100.0, 0.0),
        Vec2::new(10.0, 10.0),
        0.0,
        0.0,
        core::f64::consts::FRAC_PI_2,
    );
    let verbs = b.build();
    assert_eq!(verbs.verbs()[0], PathVerb::MoveTo);
    assert_eq!(verbs.verbs()[1], PathVerb::LineTo, "a gap must be bridged");
    assert_eq!(verbs.verbs()[2], PathVerb::CurveTo);
}

#[test]
fn a_full_turn_arc_is_split_into_four_cubics() {
    let mut b = PathBuilder::new();
    b.arc_to(
        (0.0, 0.0),
        Vec2::new(5.0, 5.0),
        0.0,
        0.0,
        core::f64::consts::TAU,
    );
    let path = b.build();
    assert_eq!(
        path.verbs()
            .iter()
            .filter(|v| **v == PathVerb::CurveTo)
            .count(),
        4
    );
}
