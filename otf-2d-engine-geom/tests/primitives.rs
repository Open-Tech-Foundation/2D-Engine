//! Points, vectors, sizes and rects.

use otf_2d_engine_geom::{Point, Rect, RectRadii, Size, Vec2};
use proptest::prelude::*;
use static_assertions::assert_impl_all;

// Doc 01 P2: every public type crosses thread boundaries.
assert_impl_all!(Point: Send, Sync, Copy);
assert_impl_all!(Vec2: Send, Sync, Copy);
assert_impl_all!(Size: Send, Sync, Copy);
assert_impl_all!(Rect: Send, Sync, Copy);
assert_impl_all!(RectRadii: Send, Sync, Copy);
assert_impl_all!(otf_2d_engine_geom::Affine: Send, Sync, Copy);
assert_impl_all!(otf_2d_engine_geom::Path: Send, Sync);

#[test]
fn subtracting_points_gives_a_displacement() {
    let a = Point::new(5.0, 7.0);
    let b = Point::new(1.0, 2.0);
    assert_eq!(a - b, Vec2::new(4.0, 5.0));
    assert_eq!(b + (a - b), a);
}

#[test]
fn the_zero_vector_has_no_direction() {
    assert_eq!(Vec2::ZERO.normalize(), None);
    assert_eq!(Vec2::new(3.0, 4.0).length(), 5.0);
    assert_eq!(Vec2::new(3.0, 4.0).normalize(), Some(Vec2::new(0.6, 0.8)));
}

#[test]
fn cross_product_sign_gives_the_turn_direction() {
    let right = Vec2::new(1.0, 0.0);
    let down = Vec2::new(0.0, 1.0);
    assert!(right.cross(down) > 0.0);
    assert!(down.cross(right) < 0.0);
    assert_eq!(right.cross(right), 0.0);
    assert_eq!(right.dot(down), 0.0);
    assert_eq!(right.perpendicular(), down);
}

#[test]
fn an_inverted_rect_is_empty_until_normalised() {
    let r = Rect::new(10.0, 10.0, 0.0, 0.0);
    assert!(r.is_empty());
    assert_eq!(r.normalized(), Rect::new(0.0, 0.0, 10.0, 10.0));
    assert!(!r.normalized().is_empty());
}

#[test]
fn area_is_zero_for_every_empty_rect() {
    // Both inverted: the product of the extents is positive, so the emptiness
    // check has to come first.
    assert_eq!(Rect::new(10.0, 10.0, 0.0, 0.0).area(), 0.0);
    assert_eq!(Rect::NOTHING.area(), 0.0);
    assert_eq!(Rect::new(0.0, 0.0, 5.0, 0.0).area(), 0.0);
    assert_eq!(Rect::new(0.0, 0.0, 4.0, 5.0).area(), 20.0);
}

#[test]
fn a_zero_area_rect_is_empty() {
    assert!(Rect::new(1.0, 1.0, 1.0, 5.0).is_empty());
    assert!(Rect::new(1.0, 1.0, 5.0, 1.0).is_empty());
    assert!(Rect::ZERO.is_empty());
}

#[test]
fn nothing_is_the_identity_for_union() {
    let r = Rect::new(1.0, 2.0, 3.0, 4.0);
    assert_eq!(Rect::NOTHING.union(r), r);
    assert_eq!(r.union(Rect::NOTHING), r);
    assert!(Rect::NOTHING.is_empty());
    assert_eq!(Rect::NOTHING.area(), 0.0);
}

#[test]
fn intersection_of_disjoint_rects_is_empty() {
    let a = Rect::new(0.0, 0.0, 1.0, 1.0);
    let b = Rect::new(5.0, 5.0, 6.0, 6.0);
    assert!(a.intersect(b).is_empty());
    assert!(!a.intersects(b));
    // Touching edges share no area.
    assert!(!a.intersects(Rect::new(1.0, 0.0, 2.0, 1.0)));
}

#[test]
fn containment_is_half_open_so_tiling_does_not_double_count() {
    let r = Rect::new(0.0, 0.0, 10.0, 10.0);
    assert!(r.contains(Point::new(0.0, 0.0)));
    assert!(r.contains(Point::new(9.999, 9.999)));
    assert!(!r.contains(Point::new(10.0, 5.0)));
    assert!(!r.contains(Point::new(5.0, 10.0)));
}

#[test]
fn round_out_grows_to_integer_boundaries_and_round_in_shrinks() {
    let r = Rect::new(0.25, -0.25, 9.5, 10.75);
    assert_eq!(r.round_out(), Rect::new(0.0, -1.0, 10.0, 11.0));
    assert_eq!(r.round_in(), Rect::new(1.0, 0.0, 9.0, 10.0));
    let exact = Rect::new(1.0, 2.0, 3.0, 4.0);
    assert_eq!(exact.round_out(), exact);
    assert_eq!(exact.round_in(), exact);
}

#[test]
fn radii_clamp_to_a_stadium_when_they_exceed_the_rect() {
    // A 100x20 rect with 50px radii: the height, not the width, is the limit.
    let r = Rect::new(0.0, 0.0, 100.0, 20.0);
    let clamped = RectRadii::uniform(50.0).clamped_to(r);
    assert_eq!(clamped, RectRadii::uniform(10.0));
}

#[test]
fn negative_radii_become_zero() {
    let r = Rect::new(0.0, 0.0, 100.0, 100.0);
    let clamped = RectRadii::new(-5.0, 10.0, -0.0, 20.0).clamped_to(r);
    assert_eq!(clamped, RectRadii::new(0.0, 10.0, 0.0, 20.0));
    assert!(RectRadii::new(-1.0, -1.0, -1.0, -1.0).is_zero());
}

#[test]
fn radii_that_already_fit_are_untouched() {
    let r = Rect::new(0.0, 0.0, 100.0, 100.0);
    let radii = RectRadii::new(1.0, 2.0, 3.0, 4.0);
    assert_eq!(radii.clamped_to(r), radii);
}

#[test]
fn a_size_is_empty_when_either_dimension_is_not_positive() {
    assert!(Size::ZERO.is_empty());
    assert!(Size::new(0.0, 5.0).is_empty());
    assert!(Size::new(-1.0, 5.0).is_empty());
    assert!(!Size::new(1.0, 5.0).is_empty());
    assert_eq!(Size::new(2.0, 3.0).area(), 6.0);
}

#[test]
fn non_finite_coordinates_are_detectable() {
    assert!(!Point::new(f64::NAN, 0.0).is_finite());
    assert!(!Vec2::new(0.0, f64::INFINITY).is_finite());
    assert!(!Size::new(f64::NEG_INFINITY, 1.0).is_finite());
    assert!(!Rect::new(0.0, 0.0, f64::NAN, 1.0).is_finite());
    assert!(!RectRadii::uniform(f64::NAN).is_finite());
    assert!(Rect::new(0.0, 0.0, 1.0, 1.0).is_finite());
}

proptest! {
    #[test]
    fn union_contains_both_operands(
        ax in -100.0f64..100.0, ay in -100.0f64..100.0, aw in 0.1f64..100.0, ah in 0.1f64..100.0,
        bx in -100.0f64..100.0, by in -100.0f64..100.0, bw in 0.1f64..100.0, bh in 0.1f64..100.0,
    ) {
        let a = Rect::new(ax, ay, ax + aw, ay + ah);
        let b = Rect::new(bx, by, bx + bw, by + bh);
        let u = a.union(b);
        prop_assert!(u.contains_rect(a));
        prop_assert!(u.contains_rect(b));
        prop_assert!(u.area() >= a.area().max(b.area()));
    }

    #[test]
    fn intersection_is_contained_by_both_operands(
        ax in -100.0f64..100.0, ay in -100.0f64..100.0, aw in 0.1f64..100.0, ah in 0.1f64..100.0,
        bx in -100.0f64..100.0, by in -100.0f64..100.0, bw in 0.1f64..100.0, bh in 0.1f64..100.0,
    ) {
        let a = Rect::new(ax, ay, ax + aw, ay + ah);
        let b = Rect::new(bx, by, bx + bw, by + bh);
        let i = a.intersect(b);
        if !i.is_empty() {
            prop_assert!(a.contains_rect(i));
            prop_assert!(b.contains_rect(i));
        }
    }

    #[test]
    fn round_out_always_contains_the_original(
        x0 in -100.0f64..100.0, y0 in -100.0f64..100.0, w in 0.0f64..100.0, h in 0.0f64..100.0,
    ) {
        let r = Rect::new(x0, y0, x0 + w, y0 + h);
        prop_assert!(r.round_out().contains_rect(r));
    }
}
