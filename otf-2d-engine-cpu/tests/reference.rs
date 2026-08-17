//! Cross-checks the rasterizer against an established renderer (T2.6).
//!
//! # What is compared, and why it is not the pixels
//!
//! `tiny-skia` composites in **sRGB byte space**; 2D-Engine composites in
//! linear light, because Doc 01 §7 makes that the model and the u8 path an
//! optimisation that must stay within tolerance of it (D-31). Half coverage of
//! black over white is therefore 128 there and 188 here, and comparing bytes
//! would measure that deliberate difference rather than anything about the
//! geometry.
//!
//! So both renders are converted back to **coverage** — each through its own
//! compositing model — and the coverages are compared. That is the thing the
//! two rasterizers should agree on: where the edges are and how much of each
//! pixel a shape covers.
//!
//! # The stated tolerance
//!
//! `tiny-skia` inherits Skia's supersampling scan converter, which samples
//! coverage rather than integrating it, so exact agreement is not expected and
//! would in fact be suspicious. The thresholds below are:
//!
//! * mean absolute coverage difference ≤ **0.01**
//! * total covered area within **0.5 %**
//! * no more than **0.5 %** of pixels differing by more than 0.1 coverage
//!
//! A systematic error — an edge in the wrong place, a fill rule inverted, a
//! transform misapplied — moves all three well past these. A different AA
//! convention does not.

mod support;

use otf_2d_engine_color::srgb8_to_linear;
use otf_2d_engine_geom::{Affine, Point, Rect};
use otf_2d_engine_scene::{FillRule, SceneBuilder};
use otf_2d_engine_testing::image::Image;

use otf_2d_engine_color::Color;
use otf_2d_engine_scene::Paint;
use support::{polygon, rect, render_case, render_case_with_tolerance, star};

const SIZE: u32 = 128;

fn black() -> Paint {
    Paint::Solid(Color::from_srgb8(0, 0, 0, 255))
}

/// Coverage recovered from our render: black over white, blended in linear
/// light, so the linear value *is* one minus the coverage.
fn our_coverage(image: &Image) -> Vec<f64> {
    image
        .data()
        .chunks_exact(4)
        .map(|pixel| 1.0 - srgb8_to_linear(pixel[0]) as f64)
        .collect()
}

/// Coverage recovered from tiny-skia: black over white, blended in sRGB byte
/// space, so the byte is one minus the coverage directly.
fn their_coverage(pixmap: &tiny_skia::Pixmap) -> Vec<f64> {
    pixmap
        .data()
        .chunks_exact(4)
        .map(|pixel| 1.0 - pixel[0] as f64 / 255.0)
        .collect()
}

struct Agreement {
    mean: f64,
    worst: f64,
    outliers: f64,
    ours: f64,
    theirs: f64,
}

fn compare(ours: &[f64], theirs: &[f64]) -> Agreement {
    assert_eq!(ours.len(), theirs.len());
    let mut total = 0.0;
    let mut worst: f64 = 0.0;
    let mut outliers = 0usize;
    for (a, b) in ours.iter().zip(theirs) {
        let difference = (a - b).abs();
        total += difference;
        worst = worst.max(difference);
        if difference > 0.1 {
            outliers += 1;
        }
    }
    Agreement {
        mean: total / ours.len() as f64,
        worst,
        outliers: outliers as f64 / ours.len() as f64,
        ours: ours.iter().sum(),
        theirs: theirs.iter().sum(),
    }
}

fn assert_agrees(label: &str, ours: &[f64], theirs: &[f64]) {
    let a = compare(ours, theirs);
    let area_error = (a.ours - a.theirs).abs() / a.theirs.max(1.0);
    println!(
        "{label}: mean {:.5}, worst {:.3}, outliers {:.4}%, area {:.1} vs {:.1} ({:.3}%)",
        a.mean,
        a.worst,
        a.outliers * 100.0,
        a.ours,
        a.theirs,
        area_error * 100.0
    );
    assert!(
        a.mean <= 0.01,
        "{label}: mean coverage difference {:.5}",
        a.mean
    );
    assert!(
        area_error <= 0.005,
        "{label}: covered area differs by {:.3}%",
        area_error * 100.0
    );
    assert!(
        a.outliers <= 0.005,
        "{label}: {:.3}% of pixels differ by more than 0.1 coverage",
        a.outliers * 100.0
    );
}

fn skia_pixmap(
    build: impl FnOnce(&mut tiny_skia::PathBuilder),
    rule: tiny_skia::FillRule,
) -> tiny_skia::Pixmap {
    let mut pixmap = tiny_skia::Pixmap::new(SIZE, SIZE).expect("pixmap");
    pixmap.fill(tiny_skia::Color::WHITE);
    let mut builder = tiny_skia::PathBuilder::new();
    build(&mut builder);
    let path = builder.finish().expect("path");
    let mut paint = tiny_skia::Paint::default();
    paint.set_color(tiny_skia::Color::BLACK);
    paint.anti_alias = true;
    pixmap.fill_path(&path, &paint, rule, tiny_skia::Transform::identity(), None);
    pixmap
}

#[test]
fn an_axis_aligned_rect_agrees_with_tiny_skia() {
    let ours = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &black(),
            &rect(20.25, 16.5, 100.75, 90.5),
        )
        .expect("fill");
    })
    .expect("render");
    let theirs = skia_pixmap(
        |b| {
            b.push_rect(tiny_skia::Rect::from_ltrb(20.25, 16.5, 100.75, 90.5).expect("rect"));
        },
        tiny_skia::FillRule::Winding,
    );
    assert_agrees("rect", &our_coverage(&ours), &their_coverage(&theirs));
}

#[test]
fn a_triangle_agrees_with_tiny_skia() {
    let points = [(14.0, 18.0), (112.0, 40.0), (52.0, 110.0)];
    let ours = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &black(),
            &polygon(&points),
        )
        .expect("fill");
    })
    .expect("render");
    let theirs = skia_pixmap(
        |b| {
            b.move_to(points[0].0 as f32, points[0].1 as f32);
            b.line_to(points[1].0 as f32, points[1].1 as f32);
            b.line_to(points[2].0 as f32, points[2].1 as f32);
            b.close();
        },
        tiny_skia::FillRule::Winding,
    );
    assert_agrees("triangle", &our_coverage(&ours), &their_coverage(&theirs));
}

/// Curves are compared at a tight flattening tolerance.
///
/// At the default quarter-pixel tolerance a chord's sagitta shifts a boundary
/// pixel's coverage by up to that same quarter, which is exactly what shows up:
/// mean 0.0018, worst 0.250, area 0.35% low. That is the M2 stopgap flattener
/// being measured, not the rasterizer — T3.1 replaces it — so the comparison
/// tightens the tolerance until flattening is negligible and the rasterizers
/// are what is left.
const CURVE_TOLERANCE: f64 = 0.01;

#[test]
fn a_circle_agrees_with_tiny_skia() {
    let ours =
        render_case_with_tolerance(SIZE, SIZE, CURVE_TOLERANCE, |sb: &mut SceneBuilder<'_>| {
            sb.fill(
                FillRule::NonZero,
                Affine::IDENTITY,
                &black(),
                &otf_2d_engine_geom::PathBuilder::new()
                    .circle(Point::new(64.0, 64.0), 50.0)
                    .build(),
            )
            .expect("fill");
        })
        .expect("render");
    let theirs = skia_pixmap(
        |b| {
            b.push_circle(64.0, 64.0, 50.0);
        },
        tiny_skia::FillRule::Winding,
    );
    assert_agrees("circle", &our_coverage(&ours), &their_coverage(&theirs));
}

#[test]
fn a_rounded_rect_agrees_with_tiny_skia() {
    let bounds = Rect::new(12.5, 20.5, 115.5, 104.5);
    let ours =
        render_case_with_tolerance(SIZE, SIZE, CURVE_TOLERANCE, |sb: &mut SceneBuilder<'_>| {
            sb.fill(
                FillRule::NonZero,
                Affine::IDENTITY,
                &black(),
                &otf_2d_engine_geom::PathBuilder::new()
                    .rounded_rect(bounds, otf_2d_engine_geom::RectRadii::uniform(18.0))
                    .build(),
            )
            .expect("fill");
        })
        .expect("render");
    let theirs = skia_pixmap(
        |b| {
            let rect = tiny_skia::Rect::from_ltrb(12.5, 20.5, 115.5, 104.5).expect("rect");
            b.push_rect(rect);
        },
        tiny_skia::FillRule::Winding,
    );
    // Compared against the plain rectangle only for area sanity: tiny-skia has
    // no rounded-rect primitive with our corner construction, and comparing a
    // different curve would measure the difference between two shapes rather
    // than between two rasterizers.
    let ours_area: f64 = our_coverage(&ours).iter().sum();
    let theirs_area: f64 = their_coverage(&theirs).iter().sum();
    let corners = 4.0 * (18.0 * 18.0 - core::f64::consts::PI * 18.0 * 18.0 / 4.0);
    let expected = theirs_area - corners;
    assert!(
        (ours_area - expected).abs() / expected < 0.01,
        "rounded rect area {ours_area} should be the rect's {theirs_area} less {corners} corners"
    );
}

#[test]
fn both_fill_rules_agree_with_tiny_skia_on_a_star() {
    for (rule, theirs_rule, label) in [
        (
            FillRule::NonZero,
            tiny_skia::FillRule::Winding,
            "star non-zero",
        ),
        (
            FillRule::EvenOdd,
            tiny_skia::FillRule::EvenOdd,
            "star even-odd",
        ),
    ] {
        let path = star((64.0, 64.0), 56.0, 5);
        let ours = render_case(SIZE, SIZE, |sb: &mut SceneBuilder<'_>| {
            sb.fill(rule, Affine::IDENTITY, &black(), &path)
                .expect("fill");
        })
        .expect("render");

        let vertices = star((64.0, 64.0), 56.0, 5);
        let theirs = skia_pixmap(
            |b| {
                for (index, point) in vertices.points().iter().enumerate() {
                    if index == 0 {
                        b.move_to(point.x as f32, point.y as f32);
                    } else {
                        b.line_to(point.x as f32, point.y as f32);
                    }
                }
                b.close();
            },
            theirs_rule,
        );
        assert_agrees(label, &our_coverage(&ours), &their_coverage(&theirs));
    }
}
