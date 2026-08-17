//! Analytic checks on what the CPU backend draws.
//!
//! These exist so the golden references can be blessed on evidence rather than
//! on the assumption that whatever came out first was right. A golden image
//! locks in behaviour; it does not establish it. Each assertion here is a fact
//! about the shape that can be worked out on paper.

mod support;

use otf_2d_engine_color::{BlendMode, Color, srgb8_to_linear};
use otf_2d_engine_geom::{Affine, PathBuilder, Point, Vec2};
use otf_2d_engine_scene::{FillRule, SceneBuilder};
use otf_2d_engine_testing::image::Image;

use support::{accent, ink, polygon, rect, render_case, render_case_with_tolerance, star};

const SIZE: u32 = 96;

fn at(image: &Image, x: u32, y: u32) -> [u8; 4] {
    let data = image.data();
    let index = (y as usize * image.width() as usize + x as usize) * 4;
    [
        data[index],
        data[index + 1],
        data[index + 2],
        data[index + 3],
    ]
}

/// Coverage recovered from a pixel, given the render is `ink` over white.
///
/// The blend is linear-light, so coverage comes back by decoding both ends and
/// solving — not by reading the byte as though it were a fraction.
fn coverage(image: &Image, x: u32, y: u32) -> f64 {
    let pixel = at(image, x, y);
    let ink = srgb8_to_linear(24) as f64;
    let background = 1.0f64;
    let value = srgb8_to_linear(pixel[0]) as f64;
    ((background - value) / (background - ink)).clamp(0.0, 1.0)
}

/// Total covered area, in pixels.
fn area(image: &Image) -> f64 {
    (0..image.height())
        .flat_map(|y| (0..image.width()).map(move |x| (x, y)))
        .map(|(x, y)| coverage(image, x, y))
        .sum()
}

fn white(image: &Image, x: u32, y: u32) -> bool {
    at(image, x, y) == [255, 255, 255, 255]
}

// ------------------------------------------------------------ aligned

#[test]
fn a_pixel_aligned_rect_has_no_partial_pixels_and_the_right_area() {
    let image = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &ink(),
            &rect(16.0, 16.0, 80.0, 80.0),
        )
        .expect("fill");
    })
    .expect("render");

    assert_eq!(at(&image, 48, 48), [24, 42, 96, 255], "inside");
    assert!(white(&image, 8, 8), "outside");
    assert!(white(&image, 15, 48), "the column before the edge");
    assert_eq!(
        at(&image, 16, 48),
        [24, 42, 96, 255],
        "the first covered column"
    );
    assert_eq!(
        at(&image, 79, 48),
        [24, 42, 96, 255],
        "the last covered column"
    );
    assert!(white(&image, 80, 48), "the column after the edge");
    assert_eq!(area(&image), 64.0 * 64.0);
}

#[test]
fn a_full_surface_rect_covers_every_pixel() {
    let image = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &ink(),
            &rect(0.0, 0.0, 96.0, 96.0),
        )
        .expect("fill");
    })
    .expect("render");
    assert!(
        image.data().chunks_exact(4).all(|p| p == [24, 42, 96, 255]),
        "a full-surface fill left something unpainted"
    );
}

#[test]
fn a_one_pixel_rect_paints_exactly_one_pixel() {
    let image = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &ink(),
            &rect(48.0, 48.0, 49.0, 49.0),
        )
        .expect("fill");
    })
    .expect("render");
    let painted = image
        .data()
        .chunks_exact(4)
        .filter(|p| *p != [255, 255, 255, 255])
        .count();
    assert_eq!(painted, 1);
    assert_eq!(at(&image, 48, 48), [24, 42, 96, 255]);
}

// ------------------------------------------------------------ antialiasing

#[test]
fn a_half_pixel_rect_has_half_covered_edges_and_the_right_area() {
    let image = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &ink(),
            &rect(16.5, 16.5, 79.5, 79.5),
        )
        .expect("fill");
    })
    .expect("render");

    assert!((coverage(&image, 16, 48) - 0.5).abs() < 0.01, "left edge");
    assert!((coverage(&image, 79, 48) - 0.5).abs() < 0.01, "right edge");
    assert!((coverage(&image, 48, 16) - 0.5).abs() < 0.01, "top edge");
    assert!(
        (coverage(&image, 16, 16) - 0.25).abs() < 0.01,
        "the corner is a quarter"
    );
    assert!(
        (coverage(&image, 48, 48) - 1.0).abs() < 0.005,
        "the interior is solid"
    );
    assert!(
        (area(&image) - 63.0 * 63.0).abs() < 2.0,
        "area {} should be 63²",
        area(&image)
    );
}

#[test]
fn a_sub_pixel_sliver_is_drawn_rather_than_dropped() {
    let image = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &ink(),
            &rect(48.0, 8.0, 48.3, 88.0),
        )
        .expect("fill");
    })
    .expect("render");
    assert!(
        (coverage(&image, 48, 48) - 0.3).abs() < 0.01,
        "a 0.3-wide sliver should read as 30% coverage, got {}",
        coverage(&image, 48, 48)
    );
    assert!((area(&image) - 0.3 * 80.0).abs() < 0.5);
}

/// The area a flattened convex curve loses to its chords.
///
/// A chord of sagitta `s` cuts off about `(2/3)·chord·s`, and the flattener
/// keeps `s` under the tolerance, so the whole shape loses at most
/// `(2/3)·perimeter·tolerance`. The deficit is one-sided: chords lie inside, so
/// a filled curve is never *larger* than the true shape.
fn flattening_deficit(perimeter: f64, tolerance: f64) -> f64 {
    (2.0 / 3.0) * perimeter * tolerance
}

fn circle_image(tolerance: f64) -> Image {
    render_case_with_tolerance(SIZE, SIZE, tolerance, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &ink(),
            &PathBuilder::new()
                .circle(Point::new(48.0, 48.0), 36.0)
                .build(),
        )
        .expect("fill");
    })
    .expect("render")
}

#[test]
fn a_circle_covers_its_analytic_area_to_within_the_flattening_bound() {
    let radius = 36.0;
    let expected = core::f64::consts::PI * radius * radius;
    let bound = flattening_deficit(core::f64::consts::TAU * radius, 0.25);

    let measured = area(&circle_image(0.25));
    let deficit = expected - measured;
    assert!(
        deficit >= 0.0,
        "a flattened circle cannot be larger than the true one: {measured} > {expected}"
    );
    assert!(
        deficit <= bound,
        "circle lost {deficit} to flattening, bound is {bound}"
    );
}

#[test]
fn tightening_the_tolerance_converges_on_the_true_area() {
    let radius = 36.0;
    let expected = core::f64::consts::PI * radius * radius;
    let coarse = expected - area(&circle_image(1.0));
    let fine = expected - area(&circle_image(0.01));

    assert!(
        fine < coarse,
        "a tighter tolerance did not get closer: {fine} vs {coarse}"
    );
    assert!(
        fine / expected < 0.001,
        "at a hundredth-pixel tolerance the circle is still {:.3}% small",
        100.0 * fine / expected
    );
}

#[test]
fn an_ellipse_covers_its_analytic_area_to_within_the_flattening_bound() {
    let (a, b) = (40.0f64, 22.0f64);
    let image = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &ink(),
            &PathBuilder::new()
                .ellipse(Point::new(48.0, 48.0), Vec2::new(a, b))
                .build(),
        )
        .expect("fill");
    })
    .expect("render");

    let expected = core::f64::consts::PI * a * b;
    // Ramanujan's approximation; well inside the slack the bound allows.
    let perimeter =
        core::f64::consts::PI * (3.0 * (a + b) - ((3.0 * a + b) * (a + 3.0 * b)).sqrt());
    let deficit = expected - area(&image);
    assert!(deficit >= 0.0, "an ellipse came out larger than analytic");
    assert!(
        deficit <= flattening_deficit(perimeter, 0.25),
        "ellipse lost {deficit}, bound is {}",
        flattening_deficit(perimeter, 0.25)
    );
}

#[test]
fn a_triangle_covers_its_analytic_area() {
    let points = [(12.0, 12.0), (84.0, 30.0), (40.0, 84.0)];
    let image = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &ink(),
            &polygon(&points),
        )
        .expect("fill");
    })
    .expect("render");

    // The shoelace formula, on the same three vertices.
    let expected = 0.5
        * ((points[0].0 * (points[1].1 - points[2].1)
            + points[1].0 * (points[2].1 - points[0].1)
            + points[2].0 * (points[0].1 - points[1].1))
            .abs());
    assert!(
        (area(&image) - expected).abs() / expected < 0.002,
        "triangle area {} differs from the shoelace area {expected}",
        area(&image)
    );
}

// ------------------------------------------------------------ fill rules

#[test]
fn the_fill_rules_disagree_at_the_centre_of_a_star() {
    let non_zero = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &ink(),
            &star((48.0, 48.0), 40.0, 5),
        )
        .expect("fill");
    })
    .expect("render");
    let even_odd = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::EvenOdd,
            Affine::IDENTITY,
            &ink(),
            &star((48.0, 48.0), 40.0, 5),
        )
        .expect("fill");
    })
    .expect("render");

    assert_eq!(
        at(&non_zero, 48, 48),
        [24, 42, 96, 255],
        "non-zero fills the pentagon at the centre"
    );
    assert!(
        white(&even_odd, 48, 48),
        "even-odd leaves the centre of a star empty"
    );
    assert!(
        area(&non_zero) > area(&even_odd),
        "non-zero must cover at least as much as even-odd"
    );
    // The points are outside the self-intersection, so both rules fill them.
    assert!(coverage(&non_zero, 48, 12) > 0.9);
    assert!(coverage(&even_odd, 48, 12) > 0.9);
}

#[test]
fn an_annulus_has_a_hole() {
    let image = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        let mut b = PathBuilder::new();
        b.circle(Point::new(48.0, 48.0), 40.0);
        b.circle(Point::new(48.0, 48.0), 20.0);
        sb.fill(FillRule::EvenOdd, Affine::IDENTITY, &ink(), &b.build())
            .expect("fill");
    })
    .expect("render");

    assert!(white(&image, 48, 48), "the hole is not open");
    assert!(coverage(&image, 48, 18) > 0.9, "the ring is not filled");
    let expected = core::f64::consts::PI * (40.0 * 40.0 - 20.0 * 20.0);
    // Both circles lose area to their chords, and the inner one is a hole, so
    // its deficit *adds* to the ring rather than subtracting. The bound is the
    // sum of the two perimeters either way.
    let bound = flattening_deficit(core::f64::consts::TAU * (40.0 + 20.0), 0.25);
    assert!(
        (area(&image) - expected).abs() <= bound,
        "annulus area {} differs from {expected} by more than {bound}",
        area(&image)
    );
}

// ------------------------------------------------------------ clipping

#[test]
fn a_rect_clip_bounds_what_is_drawn() {
    let image = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.push_layer(
            BlendMode::SrcOver,
            1.0,
            Affine::IDENTITY,
            Some(&rect(24.0, 24.0, 72.0, 72.0)),
        )
        .expect("push");
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &ink(),
            &PathBuilder::new()
                .circle(Point::new(48.0, 48.0), 44.0)
                .build(),
        )
        .expect("fill");
        sb.pop_layer().expect("pop");
    })
    .expect("render");

    for y in 0..SIZE {
        for x in 0..SIZE {
            let inside = (24..72).contains(&x) && (24..72).contains(&y);
            if !inside {
                assert!(
                    white(&image, x, y),
                    "painted outside the clip at ({x}, {y})"
                );
            }
        }
    }
    assert_eq!(
        at(&image, 48, 48),
        [24, 42, 96, 255],
        "the clip emptied the fill"
    );
    assert_eq!(
        at(&image, 24, 24),
        [24, 42, 96, 255],
        "the clip's own corner"
    );
}

#[test]
fn a_fractional_clip_antialiases_its_own_edge() {
    let image = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.push_layer(
            BlendMode::SrcOver,
            1.0,
            Affine::IDENTITY,
            Some(&rect(20.25, 20.75, 75.75, 75.25)),
        )
        .expect("push");
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &ink(),
            &rect(0.0, 0.0, 96.0, 96.0),
        )
        .expect("fill");
        sb.pop_layer().expect("pop");
    })
    .expect("render");

    assert!(
        (coverage(&image, 20, 48) - 0.75).abs() < 0.01,
        "left edge at x=20.25"
    );
    assert!(
        (coverage(&image, 75, 48) - 0.75).abs() < 0.01,
        "right edge at x=75.75"
    );
    assert!(
        (coverage(&image, 48, 20) - 0.25).abs() < 0.01,
        "top edge at y=20.75"
    );
    assert!(
        (coverage(&image, 48, 75) - 0.25).abs() < 0.01,
        "bottom edge at y=75.25"
    );
    assert!((coverage(&image, 48, 48) - 1.0).abs() < 0.005);
    let expected = (75.75 - 20.25) * (75.25 - 20.75);
    assert!(
        (area(&image) - expected).abs() < 1.0,
        "clipped area {} should be {expected}",
        area(&image)
    );
}

#[test]
fn nested_clips_intersect() {
    let image = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.push_layer(
            BlendMode::SrcOver,
            1.0,
            Affine::IDENTITY,
            Some(&rect(12.0, 12.0, 72.0, 84.0)),
        )
        .expect("push");
        sb.push_layer(
            BlendMode::SrcOver,
            1.0,
            Affine::IDENTITY,
            Some(&rect(24.0, 6.0, 84.0, 60.0)),
        )
        .expect("push");
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &accent(),
            &rect(0.0, 0.0, 96.0, 96.0),
        )
        .expect("fill");
        sb.pop_layer().expect("pop");
        sb.pop_layer().expect("pop");
    })
    .expect("render");

    // The intersection is x 24..72, y 12..60.
    for y in 0..SIZE {
        for x in 0..SIZE {
            let inside = (24..72).contains(&x) && (12..60).contains(&y);
            assert_eq!(
                !white(&image, x, y),
                inside,
                "({x}, {y}) is on the wrong side of the intersected clip"
            );
        }
    }
}

// ------------------------------------------------------------ surface edges

#[test]
fn a_shape_running_off_the_surface_is_filled_up_to_the_edge() {
    let image = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &ink(),
            &rect(-20.5, -20.5, 60.5, 60.5),
        )
        .expect("fill");
    })
    .expect("render");

    assert_eq!(
        at(&image, 0, 0),
        [24, 42, 96, 255],
        "the corner at the surface edge"
    );
    assert!(
        (coverage(&image, 60, 30) - 0.5).abs() < 0.01,
        "the on-surface edge"
    );
    assert!(white(&image, 61, 30));
    assert!((area(&image) - 60.5 * 60.5).abs() < 1.0);
}

#[test]
fn a_shape_entirely_off_surface_paints_nothing() {
    let image = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &ink(),
            &rect(200.0, 200.0, 260.0, 260.0),
        )
        .expect("fill");
    })
    .expect("render");
    assert!(
        image
            .data()
            .chunks_exact(4)
            .all(|p| p == [255, 255, 255, 255])
    );
}

// ------------------------------------------------------------ transforms

#[test]
fn a_rotation_preserves_area() {
    let plain = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &ink(),
            &rect(20.0, 32.0, 76.0, 64.0),
        )
        .expect("fill");
    })
    .expect("render");
    let rotated = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::NonZero,
            Affine::rotate_about(0.4, Point::new(48.0, 48.0)),
            &ink(),
            &rect(20.0, 32.0, 76.0, 64.0),
        )
        .expect("fill");
    })
    .expect("render");

    let expected = 56.0 * 32.0;
    assert!((area(&plain) - expected).abs() < 0.5);
    assert!(
        (area(&rotated) - expected).abs() / expected < 0.005,
        "rotation changed the area from {expected} to {}",
        area(&rotated)
    );
}

#[test]
fn a_scale_multiplies_area_by_its_square() {
    let unit = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &ink(),
            &rect(10.0, 10.0, 30.0, 26.0),
        )
        .expect("fill");
    })
    .expect("render");
    let scaled = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::NonZero,
            Affine::scale(2.0),
            &ink(),
            &rect(10.0, 10.0, 30.0, 26.0),
        )
        .expect("fill");
    })
    .expect("render");

    assert!((area(&unit) - 20.0 * 16.0).abs() < 0.5);
    assert!((area(&scaled) - 4.0 * 20.0 * 16.0).abs() < 1.0);
}

#[test]
fn the_render_is_reproducible() {
    let once = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &ink(),
            &star((48.0, 48.0), 40.0, 5),
        )
        .expect("fill");
    })
    .expect("render");
    for _ in 0..8 {
        let again = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
            sb.fill(
                FillRule::NonZero,
                Affine::IDENTITY,
                &ink(),
                &star((48.0, 48.0), 40.0, 5),
            )
            .expect("fill");
        })
        .expect("render");
        assert_eq!(again.data(), once.data());
    }
    let _ = Color::TRANSPARENT;
}
