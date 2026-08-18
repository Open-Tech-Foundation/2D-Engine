//! Stage 3 stroke expansion (T3.2).
//!
//! A stroke's outline is checked against what the shape is worth on paper:
//! the area it encloses. That is a number the geometry decides and the code
//! cannot argue with, and it catches the mistakes a picture would not — an
//! outline wound the wrong way, a cap left off, an offset on the wrong side.

use otf_2d_engine_geom::{Affine, Path, PathBuilder, Point, Rect};
use otf_2d_engine_raster::{Flattener, Segment, StrokeSpec};
use otf_2d_engine_scene::{Cap, Join};

const TOLERANCE: f64 = 0.01;

fn spec(width: f64, join: Join, cap: Cap) -> StrokeSpec {
    StrokeSpec {
        width,
        join,
        start_cap: cap,
        end_cap: cap,
    }
}

fn butt(width: f64) -> StrokeSpec {
    spec(width, Join::Bevel, Cap::Butt)
}

fn raw(path: &Path) -> (Vec<u8>, Vec<f64>) {
    (
        path.verbs().iter().map(|v| *v as u8).collect(),
        path.points().iter().flat_map(|p| [p.x, p.y]).collect(),
    )
}

fn outline(path: &Path, spec: &StrokeSpec) -> Vec<Segment> {
    outline_at(path, spec, Affine::IDENTITY)
}

fn outline_at(path: &Path, spec: &StrokeSpec, transform: Affine) -> Vec<Segment> {
    let (verbs, points) = raw(path);
    let mut flattener = Flattener::new();
    flattener.add_stroke(&verbs, &points, transform, TOLERANCE, spec);
    flattener.segments().to_vec()
}

/// The signed area the outline encloses.
///
/// `∮ x dy` rather than the symmetric shoelace, because a horizontal edge
/// carries no winding and stage 3 drops it — and `dy` is zero on exactly those
/// edges, so the two agree on every closed outline.
fn area(segments: &[Segment]) -> f64 {
    segments
        .iter()
        .map(|s| {
            let (x0, y0) = (s.x0 as f64, s.y0 as f64);
            let (x1, y1) = (s.x1 as f64, s.y1 as f64);
            0.5 * (x0 + x1) * (y1 - y0)
        })
        .sum()
}

/// The area the outline actually paints, under the non-zero rule.
///
/// Not the same number as [`area`] wherever the outline crosses itself, which
/// is every corner a stroke turns: the two sides overlap on the inside of the
/// bend, and a line integral counts that twice where the fill counts it once.
fn painted(segments: &[Segment], step: f64) -> f64 {
    let mut bounds = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for s in segments {
        for (x, y) in [(s.x0 as f64, s.y0 as f64), (s.x1 as f64, s.y1 as f64)] {
            bounds.0 = bounds.0.min(x);
            bounds.1 = bounds.1.min(y);
            bounds.2 = bounds.2.max(x);
            bounds.3 = bounds.3.max(y);
        }
    }
    if segments.is_empty() {
        return 0.0;
    }
    let columns = ((bounds.2 - bounds.0) / step).ceil() as i64 + 2;
    let rows = ((bounds.3 - bounds.1) / step).ceil() as i64 + 2;
    let mut inside = 0u64;
    for row in 0..rows {
        // Off the half-step so no sample lands on an edge.
        let y = bounds.1 - 0.5 * step + (row as f64 + 0.5) * step;
        for column in 0..columns {
            let x = bounds.0 - 0.5 * step + (column as f64 + 0.5) * step;
            let point = Point::new(x, y);
            let winding: i32 = segments.iter().map(|s| crossing(s, point)).sum();
            if winding != 0 {
                inside += 1;
            }
        }
    }
    inside as f64 * step * step
}

fn line(from: Point, to: Point) -> Path {
    let mut builder = PathBuilder::new();
    builder.move_to(from);
    builder.line_to(to);
    builder.build()
}

fn close_enough(measured: f64, expected: f64, share: f64) -> bool {
    (measured - expected).abs() <= share * expected.abs().max(1.0)
}

// ---------------------------------------------------------------- open ends

#[test]
fn a_straight_line_strokes_to_a_rectangle() {
    let path = line(Point::new(10.0, 20.0), Point::new(110.0, 20.0));
    let measured = area(&outline(&path, &butt(8.0)));
    assert!(
        close_enough(measured.abs(), 800.0, 1e-9),
        "a 100 by 8 stroke enclosed {measured}"
    );
}

#[test]
fn a_square_cap_adds_half_a_width_at_each_end() {
    let path = line(Point::new(10.0, 20.0), Point::new(110.0, 20.0));
    let measured = area(&outline(&path, &spec(8.0, Join::Bevel, Cap::Square))).abs();
    assert!(
        close_enough(measured, 108.0 * 8.0, 1e-9),
        "square caps enclosed {measured}, wanted {}",
        108.0 * 8.0
    );
}

#[test]
fn a_round_cap_adds_a_disc_between_the_two_of_them() {
    let path = line(Point::new(10.0, 20.0), Point::new(110.0, 20.0));
    let measured = area(&outline(&path, &spec(8.0, Join::Bevel, Cap::Round))).abs();
    let expected = 800.0 + core::f64::consts::PI * 16.0;
    assert!(
        close_enough(measured, expected, 1e-3),
        "round caps enclosed {measured}, wanted {expected}"
    );
}

#[test]
fn a_diagonal_strokes_to_the_same_area_as_a_horizontal() {
    let flat = line(Point::new(0.0, 0.0), Point::new(100.0, 0.0));
    let slanted = line(Point::new(0.0, 0.0), Point::new(60.0, 80.0));
    let a = area(&outline(&flat, &butt(6.0))).abs();
    let b = area(&outline(&slanted, &butt(6.0))).abs();
    // Segments are `f32`, so a hundred units out the last bit is worth this
    // much and no assertion can be tighter than it.
    assert!(
        close_enough(b, a, 1e-5),
        "a slanted 100 by 6 stroke enclosed {b} against {a}"
    );
}

// ------------------------------------------------------------------ closed

#[test]
fn a_closed_square_strokes_to_the_ring_around_it() {
    let path = PathBuilder::new()
        .rect(Rect::new(20.0, 20.0, 120.0, 120.0))
        .build();
    let width = 10.0;
    let measured = area(&outline(
        &path,
        &spec(width, Join::Miter { limit: 4.0 }, Cap::Butt),
    ));
    // The ring between a square grown by half a width and one shrunk by it.
    let expected = (100.0 + width) * (100.0 + width) - (100.0 - width) * (100.0 - width);
    assert!(
        close_enough(measured.abs(), expected, 1e-9),
        "a stroked square enclosed {measured}, wanted {expected}"
    );
}

#[test]
fn a_closed_circle_strokes_to_an_annulus() {
    let radius = 60.0;
    let width = 12.0;
    let path = PathBuilder::new()
        .circle(Point::new(100.0, 100.0), radius)
        .build();
    let measured = area(&outline(&path, &butt(width))).abs();
    let expected =
        core::f64::consts::PI * ((radius + 0.5 * width).powi(2) - (radius - 0.5 * width).powi(2));
    assert!(
        close_enough(measured, expected, 2e-3),
        "a stroked circle enclosed {measured}, wanted {expected}"
    );
}

#[test]
fn the_two_sides_of_a_closed_stroke_wind_opposite_ways() {
    // The outer contour and the inner one have to disagree, or the middle
    // fills in — which is the whole reason the ring is two contours and not
    // one.
    let path = PathBuilder::new()
        .circle(Point::new(0.0, 0.0), 40.0)
        .build();
    let segments = outline(&path, &butt(6.0));
    let inside = Point::new(0.0, 0.0);
    let winding: i32 = segments.iter().map(|s| crossing(s, inside)).sum();
    assert_eq!(winding, 0, "the hole of the annulus is filled");
    let on_the_ring = Point::new(40.0, 0.0);
    let winding: i32 = segments.iter().map(|s| crossing(s, on_the_ring)).sum();
    assert_eq!(winding.abs(), 1, "the ring itself is not filled once");
}

/// The winding a segment contributes at `point`, by a ray going right.
fn crossing(s: &Segment, point: Point) -> i32 {
    let (x0, y0) = (s.x0 as f64, s.y0 as f64);
    let (x1, y1) = (s.x1 as f64, s.y1 as f64);
    if (y0 > point.y) == (y1 > point.y) {
        return 0;
    }
    let at = (point.y - y0) / (y1 - y0);
    if x0 + at * (x1 - x0) <= point.x {
        return 0;
    }
    if y1 > y0 { 1 } else { -1 }
}

// ------------------------------------------------------------------- joins

#[test]
fn a_miter_join_reaches_further_than_a_bevel() {
    let mut builder = PathBuilder::new();
    builder.move_to(Point::new(0.0, 0.0));
    builder.line_to(Point::new(50.0, 0.0));
    builder.line_to(Point::new(50.0, 50.0));
    let path = builder.build();
    let step = 0.05;
    let bevel = painted(&outline(&path, &spec(10.0, Join::Bevel, Cap::Butt)), step);
    let miter = painted(
        &outline(&path, &spec(10.0, Join::Miter { limit: 4.0 }, Cap::Butt)),
        step,
    );
    let round = painted(&outline(&path, &spec(10.0, Join::Round, Cap::Butt)), step);
    assert!(
        miter > round && round > bevel,
        "miter {miter}, round {round}, bevel {bevel} are not in that order"
    );
    // Outside a right-angle corner the two half-width squares leave a triangle
    // between them. A bevel cuts it in half, a miter fills it, and a round
    // join takes the quarter-disc out of it.
    assert!(
        close_enough(miter - bevel, 12.5, 1e-2),
        "the miter added {} over the bevel, wanted 12.5",
        miter - bevel
    );
    assert!(
        close_enough(
            round - bevel,
            0.25 * core::f64::consts::PI * 25.0 - 12.5,
            2e-2
        ),
        "the round join added {} over the bevel",
        round - bevel
    );
}

#[test]
fn a_miter_past_its_limit_falls_back_to_a_bevel() {
    // A hairpin: the spike would be many half-widths long.
    let mut builder = PathBuilder::new();
    builder.move_to(Point::new(0.0, 0.0));
    builder.line_to(Point::new(50.0, 0.0));
    builder.line_to(Point::new(0.0, 1.0));
    let path = builder.build();
    let bevel = area(&outline(&path, &spec(10.0, Join::Bevel, Cap::Butt))).abs();
    let limited = area(&outline(
        &path,
        &spec(10.0, Join::Miter { limit: 4.0 }, Cap::Butt),
    ))
    .abs();
    assert!(
        close_enough(limited, bevel, 1e-9),
        "a miter past its limit enclosed {limited} against the bevel's {bevel}"
    );
}

// ------------------------------------------------------------- degenerate

#[test]
fn a_stroke_with_no_width_paints_nothing() {
    let path = line(Point::new(0.0, 0.0), Point::new(50.0, 50.0));
    for width in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let segments = outline(&path, &butt(width));
        assert!(
            segments.is_empty() || segments.iter().all(|s| s.is_finite()),
            "width {width} produced {segments:?}"
        );
        if width <= 0.0 || !width.is_finite() {
            assert!(segments.is_empty(), "width {width} painted something");
        }
    }
}

#[test]
fn a_subpath_with_no_length_is_a_dot_under_a_round_cap() {
    let mut builder = PathBuilder::new();
    builder.move_to(Point::new(30.0, 30.0));
    builder.line_to(Point::new(30.0, 30.0));
    builder.close();
    let path = builder.build();

    let round = area(&outline(&path, &spec(10.0, Join::Round, Cap::Round))).abs();
    let expected = core::f64::consts::PI * 25.0;
    // A flattened circle is inscribed, so it falls short by the same bound
    // every flattened outline does.
    let bound = (2.0 / 3.0) * core::f64::consts::TAU * 5.0 * TOLERANCE;
    assert!(
        (expected - round) >= 0.0 && (expected - round) <= bound,
        "a round dot enclosed {round}, wanted {expected} within {bound}"
    );

    let square = area(&outline(&path, &spec(10.0, Join::Round, Cap::Square))).abs();
    assert!(
        close_enough(square, 100.0, 1e-9),
        "a square dot enclosed {square}, wanted 100"
    );

    assert!(
        outline(&path, &butt(10.0)).is_empty(),
        "a butt cap gave a dot some width"
    );
}

#[test]
fn a_cusp_does_not_panic_and_stays_finite() {
    // A curve that doubles back on itself, stroked wider than it is tall.
    let mut builder = PathBuilder::new();
    builder.move_to(Point::new(0.0, 0.0));
    builder.curve_to(
        Point::new(40.0, 0.0),
        Point::new(-40.0, 0.0),
        Point::new(0.0, 0.0),
    );
    let path = builder.build();
    let segments = outline(&path, &spec(20.0, Join::Round, Cap::Round));
    assert!(segments.iter().all(|s| s.is_finite()), "{segments:?}");
}

#[test]
fn a_stroke_wider_than_its_own_bend_stays_finite() {
    // Half the width is past the centre of curvature, so the inner parallel
    // curve turns itself inside out.
    let path = PathBuilder::new().circle(Point::new(0.0, 0.0), 5.0).build();
    let segments = outline(&path, &butt(40.0));
    assert!(!segments.is_empty());
    assert!(segments.iter().all(|s| s.is_finite()), "{segments:?}");
}

#[test]
fn a_degenerate_transform_terminates() {
    let path = PathBuilder::new()
        .circle(Point::new(10.0, 10.0), 20.0)
        .build();
    let flat = Affine::new([1.0, 0.0, 2.0, 0.0, 0.0, 0.0]);
    let segments = outline_at(&path, &spec(4.0, Join::Round, Cap::Round), flat);
    assert!(segments.iter().all(|s| s.is_finite()), "{segments:?}");
}

// ------------------------------------------------------------------ scale

#[test]
fn a_transform_scales_the_stroke_with_the_path() {
    let path = line(Point::new(0.0, 0.0), Point::new(100.0, 0.0));
    let plain = area(&outline(&path, &butt(6.0))).abs();
    let scaled = area(&outline_at(&path, &butt(6.0), Affine::scale(3.0))).abs();
    assert!(
        close_enough(scaled, plain * 9.0, 1e-9),
        "a stroke scaled by three enclosed {scaled}, wanted {}",
        plain * 9.0
    );
}

#[test]
fn a_rotation_leaves_the_area_alone() {
    let path = PathBuilder::new()
        .circle(Point::new(50.0, 50.0), 30.0)
        .build();
    let plain = area(&outline(&path, &butt(8.0))).abs();
    let turned = area(&outline_at(&path, &butt(8.0), Affine::rotate(0.7))).abs();
    assert!(
        close_enough(turned, plain, 2e-3),
        "a rotated stroke enclosed {turned} against {plain}"
    );
}
