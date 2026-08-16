//! T1.1 affine criteria: inverse round-trips to the identity, and `max_scale`
//! agrees with an independently computed largest singular value.

use otf_2d_engine_geom::{Affine, Point, Rect, Vec2};
use proptest::prelude::*;

/// Coefficients large enough to exercise real transforms, bounded enough that
/// `f64` cancellation is not what the test is measuring.
fn coefficient() -> impl Strategy<Value = f64> {
    -1000.0f64..1000.0
}

fn affine() -> impl Strategy<Value = Affine> {
    [
        coefficient(),
        coefficient(),
        coefficient(),
        coefficient(),
        coefficient(),
        coefficient(),
    ]
    .prop_map(Affine::new)
}

/// The largest singular value, computed the textbook way: the square root of
/// the largest eigenvalue of MᵀM. Deliberately unlike the closed form in
/// `Affine::max_scale`, so agreement means something.
fn largest_singular_value(m: Affine) -> f64 {
    let [a, b, c, d, _, _] = m.as_coefficients();
    // MᵀM for M = [[a, c], [b, d]].
    let p = a * a + b * b;
    let q = c * c + d * d;
    let r = a * c + b * d;
    let trace = p + q;
    let disc = ((p - q) * (p - q) + 4.0 * r * r).max(0.0).sqrt();
    ((trace + disc) * 0.5).max(0.0).sqrt()
}

fn smallest_singular_value(m: Affine) -> f64 {
    let [a, b, c, d, _, _] = m.as_coefficients();
    let p = a * a + b * b;
    let q = c * c + d * d;
    let r = a * c + b * d;
    let trace = p + q;
    let disc = ((p - q) * (p - q) + 4.0 * r * r).max(0.0).sqrt();
    ((trace - disc) * 0.5).max(0.0).sqrt()
}

fn assert_close(actual: f64, expected: f64, tolerance: f64, what: &str) {
    let scale = expected.abs().max(1.0);
    assert!(
        (actual - expected).abs() <= tolerance * scale,
        "{what}: got {actual}, expected {expected}"
    );
}

proptest! {
    /// `A.then(A⁻¹)` is the identity, to within accumulated rounding.
    #[test]
    fn inverse_round_trips_to_identity(m in affine()) {
        let Some(inv) = m.inverse() else { return Ok(()); };
        let round_trip = m.then(inv).as_coefficients();
        let identity = Affine::IDENTITY.as_coefficients();

        // A near-singular matrix amplifies rounding; scale the tolerance by
        // how badly conditioned it is rather than skipping those cases.
        let condition = m.max_scale() / m.min_scale().max(f64::MIN_POSITIVE);
        let tolerance = 1e-12 * condition.clamp(1.0, 1e6);
        for i in 0..6 {
            prop_assert!(
                (round_trip[i] - identity[i]).abs() <= tolerance,
                "coefficient {i}: {} vs {} (tolerance {tolerance})",
                round_trip[i], identity[i]
            );
        }
    }

    /// The other order too: `A⁻¹.then(A)` is also the identity.
    #[test]
    fn inverse_round_trips_in_both_orders(m in affine()) {
        let Some(inv) = m.inverse() else { return Ok(()); };
        let condition = m.max_scale() / m.min_scale().max(f64::MIN_POSITIVE);
        let tolerance = 1e-12 * condition.clamp(1.0, 1e6);
        let round_trip = inv.then(m).as_coefficients();
        for (i, &v) in round_trip.iter().enumerate() {
            prop_assert!((v - Affine::IDENTITY.as_coefficients()[i]).abs() <= tolerance);
        }
    }

    /// Round-tripping a point through a transform and its inverse returns it.
    #[test]
    fn a_point_survives_a_transform_and_its_inverse(
        m in affine(),
        x in -1e4f64..1e4,
        y in -1e4f64..1e4,
    ) {
        let Some(inv) = m.inverse() else { return Ok(()); };
        // Only meaningful when the transform is not close to collapsing.
        prop_assume!(m.min_scale() > 1e-3);
        let p = Point::new(x, y);
        let back = inv.transform_point(m.transform_point(p));
        let tolerance = 1e-9 * (m.max_scale() / m.min_scale()).max(1.0);
        prop_assert!((back.x - p.x).abs() <= tolerance, "{back:?} vs {p:?}");
        prop_assert!((back.y - p.y).abs() <= tolerance, "{back:?} vs {p:?}");
    }

    #[test]
    fn max_scale_is_the_largest_singular_value(m in affine()) {
        assert_close(m.max_scale(), largest_singular_value(m), 1e-9, "max_scale");
    }

    #[test]
    fn min_scale_is_the_smallest_singular_value(m in affine()) {
        assert_close(m.min_scale(), smallest_singular_value(m), 1e-9, "min_scale");
    }

    /// Sampled directly: no unit vector is stretched by more than `max_scale`,
    /// and some direction reaches it. This checks the *meaning*, not just an
    /// alternative formula.
    #[test]
    fn max_scale_bounds_every_direction_and_is_attained(m in affine()) {
        let max_scale = m.max_scale();
        let min_scale = m.min_scale();
        let mut observed_max: f64 = 0.0;
        let mut observed_min = f64::INFINITY;
        const SAMPLES: usize = 2048;
        for i in 0..SAMPLES {
            let theta = core::f64::consts::TAU * i as f64 / SAMPLES as f64;
            let len = m.transform_vec2(Vec2::new(theta.cos(), theta.sin())).length();
            observed_max = observed_max.max(len);
            observed_min = observed_min.min(len);
        }
        let scale = max_scale.max(1.0);
        prop_assert!(observed_max <= max_scale + 1e-9 * scale,
            "sampled {observed_max} exceeds max_scale {max_scale}");
        prop_assert!(observed_max >= max_scale - 1e-4 * scale,
            "max_scale {max_scale} never attained; best sample {observed_max}");
        prop_assert!(observed_min >= min_scale - 1e-9 * scale,
            "sampled {observed_min} below min_scale {min_scale}");
        // Near the minimising direction the length grows like max_scale·φ,
        // so a finite angular step cannot land closer than that.
        let angular_step = core::f64::consts::TAU / SAMPLES as f64;
        let min_tolerance = max_scale * angular_step + 1e-9 * scale;
        prop_assert!(observed_min <= min_scale + min_tolerance,
            "min_scale {min_scale} never attained; smallest sample {observed_min}");
    }

    /// Composition is associative, so a transform stack can be collapsed in
    /// any order — which is what stage 2 does.
    #[test]
    fn composition_is_associative(a in affine(), b in affine(), c in affine()) {
        let left = a.then(b).then(c).as_coefficients();
        let right = a.then(b.then(c)).as_coefficients();
        for i in 0..6 {
            let tolerance = 1e-6 * left[i].abs().max(right[i].abs()).max(1.0);
            prop_assert!((left[i] - right[i]).abs() <= tolerance, "coefficient {i}");
        }
    }

    /// `then` composes the transforms in the order the points travel through
    /// them, which is what makes it readable left to right.
    #[test]
    fn then_applies_self_before_other(
        a in affine(), b in affine(), x in -1e3f64..1e3, y in -1e3f64..1e3,
    ) {
        let p = Point::new(x, y);
        let composed = a.then(b).transform_point(p);
        let stepwise = b.transform_point(a.transform_point(p));
        let scale = composed.x.abs().max(composed.y.abs()).max(1.0);
        prop_assert!((composed.x - stepwise.x).abs() <= 1e-6 * scale);
        prop_assert!((composed.y - stepwise.y).abs() <= 1e-6 * scale);
    }

    /// `determinant` is the signed area scale factor, so it equals the product
    /// of the singular values up to sign.
    #[test]
    fn determinant_is_the_product_of_singular_values(m in affine()) {
        assert_close(
            m.determinant().abs(),
            m.max_scale() * m.min_scale(),
            1e-9,
            "|det|",
        );
    }
}

#[test]
fn identity_is_its_own_inverse() {
    assert_eq!(Affine::IDENTITY.inverse(), Some(Affine::IDENTITY));
    assert!(Affine::IDENTITY.is_identity());
    assert_eq!(Affine::IDENTITY.max_scale(), 1.0);
    assert_eq!(Affine::IDENTITY.min_scale(), 1.0);
}

#[test]
fn a_singular_transform_has_no_inverse() {
    // Collapses the plane onto the x axis.
    assert_eq!(Affine::scale_non_uniform(1.0, 0.0).inverse(), None);
    // Collapses it onto a line through the origin.
    assert_eq!(Affine::new([1.0, 2.0, 2.0, 4.0, 0.0, 0.0]).inverse(), None);
    assert_eq!(Affine::scale(0.0).inverse(), None);
}

#[test]
fn a_non_finite_transform_has_no_inverse() {
    assert_eq!(
        Affine::new([f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0]).inverse(),
        None
    );
    assert_eq!(
        Affine::new([f64::INFINITY, 0.0, 0.0, 1.0, 0.0, 0.0]).inverse(),
        None
    );
    // Finite coefficients whose inverse overflows are rejected too.
    let nearly_singular = Affine::new([1e-320, 0.0, 0.0, 1e-320, 0.0, 0.0]);
    assert_eq!(nearly_singular.inverse(), None);
}

#[test]
fn translation_moves_points_but_not_vectors() {
    let t = Affine::translate(Vec2::new(3.0, -4.0));
    assert_eq!(
        t.transform_point(Point::new(1.0, 1.0)),
        Point::new(4.0, -3.0)
    );
    assert_eq!(t.transform_vec2(Vec2::new(1.0, 1.0)), Vec2::new(1.0, 1.0));
    assert_eq!(t.max_scale(), 1.0);
    assert!(t.is_translation());
}

#[test]
fn rotation_turns_from_x_toward_y() {
    let r = Affine::rotate(core::f64::consts::FRAC_PI_2);
    let p = r.transform_point(Point::new(1.0, 0.0));
    assert!((p.x - 0.0).abs() < 1e-15, "{p:?}");
    assert!((p.y - 1.0).abs() < 1e-15, "{p:?}");
    assert!((r.max_scale() - 1.0).abs() < 1e-15);
}

#[test]
fn rotate_about_keeps_the_centre_fixed() {
    let c = Point::new(10.0, 20.0);
    let r = Affine::rotate_about(0.7, c);
    let moved = r.transform_point(c);
    assert!(
        (moved.x - c.x).abs() < 1e-12 && (moved.y - c.y).abs() < 1e-12,
        "{moved:?}"
    );
}

#[test]
fn axis_alignment_survives_scale_and_quarter_turns_only() {
    assert!(Affine::IDENTITY.preserves_axis_alignment());
    assert!(Affine::scale_non_uniform(2.0, -3.0).preserves_axis_alignment());
    assert!(Affine::translate(Vec2::new(5.0, 5.0)).preserves_axis_alignment());
    // An exact quarter turn, written exactly.
    assert!(Affine::new([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]).preserves_axis_alignment());
    assert!(!Affine::rotate(0.4).preserves_axis_alignment());
    assert!(!Affine::skew(0.3, 0.0).preserves_axis_alignment());
}

#[test]
fn a_computed_quarter_turn_is_not_exactly_axis_aligned() {
    // cos(π/2) is 6.1e-17, not 0, so the predicate says no. This is the
    // intended behaviour: the test exists so the trade-off stays visible.
    // A consumer that wants the fast path for a right-angle rotation should
    // build the matrix exactly rather than rely on trigonometry.
    assert!(!Affine::rotate(core::f64::consts::FRAC_PI_2).preserves_axis_alignment());
}

#[test]
fn transform_rect_bbox_is_exact_for_axis_aligned_transforms() {
    let r = Rect::new(1.0, 2.0, 5.0, 8.0);
    let m = Affine::scale(2.0).then_translate(Vec2::new(1.0, 1.0));
    assert_eq!(m.transform_rect_bbox(r), Rect::new(3.0, 5.0, 11.0, 17.0));
}

#[test]
fn the_mul_operator_is_the_reverse_of_then() {
    let a = Affine::rotate(0.3);
    let b = Affine::translate(Vec2::new(2.0, 3.0));
    assert_eq!((b * a).as_coefficients(), a.then(b).as_coefficients());
}
