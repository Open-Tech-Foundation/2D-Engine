//! Stage 3 — Euler-spiral flattening (T3.1).
//!
//! The criteria in Doc 04 T3.1 are three: the output must stay within the
//! stated tolerance, it must use fewer segments than recursive subdivision at
//! the same tolerance, and the tolerance must come from `Affine::max_scale`
//! rather than being fixed. Each has a test here. The recursive subdivider the
//! comparison needs lives in this file rather than in the crate: shipping two
//! flatteners is what T3.1 says not to do, and deleting the reference outright
//! would delete the evidence with it.

use otf_2d_engine_geom::{Affine, Path, PathBuilder, PathVerb, Point, Rect, RectRadii};
use otf_2d_engine_raster::{Flattener, Segment};

/// A deterministic generator. Random Béziers, same ones every run: a
/// flattening bug that only shows up on one seed in ten is a bug that lands on
/// a user, and a test that only sometimes finds it is worse than none.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        // SplitMix64.
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z = z ^ (z >> 31);
        (z >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, low: f64, high: f64) -> f64 {
        low + self.next() * (high - low)
    }

    fn point(&mut self, extent: f64) -> Point {
        Point::new(self.range(0.0, extent), self.range(0.0, extent))
    }
}

fn cubic_path(curve: [Point; 4]) -> Path {
    PathBuilder::new()
        .move_to(curve[0])
        .curve_to(curve[1], curve[2], curve[3])
        .build()
}

/// The scene arena's representation of a path: a verb byte stream and a flat
/// coordinate buffer, which is what `Flattener` consumes.
fn raw(path: &Path) -> (Vec<u8>, Vec<f64>) {
    let verbs = path.verbs().iter().map(|verb| *verb as u8).collect();
    let points = path
        .points()
        .iter()
        .flat_map(|point| [point.x, point.y])
        .collect();
    (verbs, points)
}

fn flatten(path: &Path, transform: Affine, tolerance: f64) -> Vec<Segment> {
    let (verbs, points) = raw(path);
    let mut flattener = Flattener::new();
    flattener.add_path(&verbs, &points, transform, tolerance);
    flattener.segments().to_vec()
}

/// The polyline the flattener produced, as points, in order.
///
/// `Flattener` drops horizontal segments — they carry no winding — so the
/// polyline is rebuilt from the curve's own geometry instead: flatten a path
/// that has been rotated by an angle no sample lands on, then rotate back.
/// That keeps every vertex.
fn polyline(curve: [Point; 4], tolerance: f64) -> Vec<Point> {
    const TILT: f64 = 0.401_425_7;
    let forward = Affine::rotate(TILT);
    let back = Affine::rotate(-TILT);
    let segments = flatten(&cubic_path(curve), forward, tolerance);
    let mut out = Vec::with_capacity(segments.len() + 1);
    for (index, segment) in segments.iter().enumerate() {
        if index == 0 {
            out.push(back.transform_point(Point::new(segment.x0 as f64, segment.y0 as f64)));
        }
        out.push(back.transform_point(Point::new(segment.x1 as f64, segment.y1 as f64)));
    }
    out
}

fn eval_cubic(curve: &[Point; 4], t: f64) -> Point {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Point::new(
        a * curve[0].x + b * curve[1].x + c * curve[2].x + d * curve[3].x,
        a * curve[0].y + b * curve[1].y + c * curve[2].y + d * curve[3].y,
    )
}

fn distance_to_polyline(p: Point, polyline: &[Point]) -> f64 {
    let mut best = f64::INFINITY;
    for window in polyline.windows(2) {
        let (a, b) = (window[0], window[1]);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let length = dx * dx + dy * dy;
        let closest = if length == 0.0 {
            a
        } else {
            let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / length).clamp(0.0, 1.0);
            Point::new(a.x + t * dx, a.y + t * dy)
        };
        best = best.min(p.distance(closest));
    }
    best
}

/// The largest distance from the true curve to the flattened polyline, found by
/// dense sampling.
fn max_deviation(curve: [Point; 4], tolerance: f64) -> f64 {
    const SAMPLES: usize = 1200;
    let line = polyline(curve, tolerance);
    assert!(line.len() >= 2, "flattening produced no polyline");
    let mut worst: f64 = 0.0;
    for sample in 0..=SAMPLES {
        let t = sample as f64 / SAMPLES as f64;
        worst = worst.max(distance_to_polyline(eval_cubic(&curve, t), &line));
    }
    worst
}

// ---------------------------------------------------------------------------
// Recursive midpoint subdivision — the reference T3.1 compares against.

/// Subdivides at the midpoint until the control polygon is flat enough,
/// counting the segments it would emit.
///
/// The flatness test is the standard one: the curve stays within ¾ of the
/// largest distance from a control point to the chord, so that bound is what
/// gets compared against the tolerance. This is the classical algorithm Doc 01
/// §4 rules out, implemented faithfully enough for the comparison to mean
/// something.
fn recursive_subdivision_count(curve: [Point; 4], tolerance: f64) -> usize {
    fn recurse(curve: [Point; 4], tolerance: f64, depth: u32) -> usize {
        let (dx, dy) = (curve[3].x - curve[0].x, curve[3].y - curve[0].y);
        let chord = (dx * dx + dy * dy).sqrt();
        let deviation = if chord == 0.0 {
            curve[0].distance(curve[1]).max(curve[0].distance(curve[2]))
        } else {
            let cross =
                |p: Point| ((p.x - curve[0].x) * dy - (p.y - curve[0].y) * dx).abs() / chord;
            cross(curve[1]).max(cross(curve[2]))
        };
        if 0.75 * deviation <= tolerance || depth >= 20 {
            return 1;
        }
        let mid = |a: Point, b: Point| Point::new(0.5 * (a.x + b.x), 0.5 * (a.y + b.y));
        let p01 = mid(curve[0], curve[1]);
        let p12 = mid(curve[1], curve[2]);
        let p23 = mid(curve[2], curve[3]);
        let p012 = mid(p01, p12);
        let p123 = mid(p12, p23);
        let p = mid(p012, p123);
        recurse([curve[0], p01, p012, p], tolerance, depth + 1)
            + recurse([p, p123, p23, curve[3]], tolerance, depth + 1)
    }
    recurse(curve, tolerance, 0)
}

// ---------------------------------------------------------------------------

#[test]
fn lines_pass_through_unchanged() {
    let path = PathBuilder::new()
        .move_to(Point::new(1.0, 2.0))
        .line_to(Point::new(30.0, 40.0))
        .line_to(Point::new(5.0, 90.0))
        .close()
        .build();
    let segments = flatten(&path, Affine::IDENTITY, 0.25);
    assert_eq!(segments.len(), 3, "three edges, including the closing one");
    assert_eq!(segments[0], Segment::new(1.0, 2.0, 30.0, 40.0));
    assert_eq!(segments[1], Segment::new(30.0, 40.0, 5.0, 90.0));
    assert_eq!(segments[2], Segment::new(5.0, 90.0, 1.0, 2.0));
}

#[test]
fn an_unclosed_subpath_still_gets_its_closing_edge() {
    let path = PathBuilder::new()
        .move_to(Point::new(0.0, 0.0))
        .line_to(Point::new(10.0, 0.0))
        .line_to(Point::new(10.0, 10.0))
        .build();
    // The horizontal edge carries no winding and is dropped, so two remain.
    let segments = flatten(&path, Affine::IDENTITY, 0.25);
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[1], Segment::new(10.0, 10.0, 0.0, 0.0));
}

/// T3.1: max deviation from the true curve ≤ tolerance, over random Béziers.
#[test]
fn deviation_never_exceeds_the_tolerance() {
    let mut rng = Rng(0x51ed_2701);
    let mut worst = 0.0f64;
    let mut worst_case = None;
    for _ in 0..600 {
        let extent = [16.0, 120.0, 700.0, 2000.0][(rng.next() * 4.0) as usize % 4];
        let curve = [
            rng.point(extent),
            rng.point(extent),
            rng.point(extent),
            rng.point(extent),
        ];
        for tolerance in [0.05, 0.25, 1.0] {
            let deviation = max_deviation(curve, tolerance);
            let ratio = deviation / tolerance;
            if ratio > worst {
                worst = ratio;
                worst_case = Some((curve, tolerance, deviation));
            }
        }
    }
    assert!(
        worst <= 1.0,
        "flattening strayed past the tolerance: {worst_case:?} is {worst}× the budget"
    );
}

/// T3.1: fewer segments than recursive subdivision at the same tolerance.
#[test]
fn uses_fewer_segments_than_recursive_subdivision() {
    let mut rng = Rng(0x2b7c_9f11);
    let (mut spiral_total, mut recursive_total) = (0usize, 0usize);
    let mut spiral_worse = 0usize;
    let cases = 400;
    for _ in 0..cases {
        let extent = [16.0, 120.0, 700.0, 2000.0][(rng.next() * 4.0) as usize % 4];
        let curve = [
            rng.point(extent),
            rng.point(extent),
            rng.point(extent),
            rng.point(extent),
        ];
        let tolerance = 0.25;
        let spiral = polyline(curve, tolerance).len() - 1;
        let recursive = recursive_subdivision_count(curve, tolerance);
        spiral_total += spiral;
        recursive_total += recursive;
        if spiral > recursive {
            spiral_worse += 1;
        }
    }
    println!(
        "euler {spiral_total} vs recursive {recursive_total} segments; recursive won on {spiral_worse}/{cases}"
    );
    assert!(
        spiral_total * 100 < recursive_total * 90,
        "Euler-spiral flattening emitted {spiral_total} segments against \
         recursive subdivision's {recursive_total}; it is supposed to be well under"
    );
    // Recursive subdivision can land on a lucky power of two for a curve here
    // and there. What it cannot do is win often.
    assert!(
        spiral_worse * 4 < cases,
        "recursive subdivision won on {spiral_worse} of {cases} curves"
    );
}

/// T3.1: the tolerance is derived from `Affine::max_scale`, not fixed.
#[test]
fn tolerance_follows_the_transform_scale() {
    let curve = [
        Point::new(0.0, 0.0),
        Point::new(40.0, 0.0),
        Point::new(100.0, 60.0),
        Point::new(100.0, 100.0),
    ];
    let path = cubic_path(curve);
    let tolerance = 0.25;
    let base = flatten(&path, Affine::IDENTITY, tolerance).len();
    // Segment count for a fixed error goes as the square root of the scale, so
    // 4× the scale is 2× the segments. Allow a segment of rounding either way.
    for scale in [4.0, 16.0] {
        let scaled = flatten(&path, Affine::scale(scale), tolerance).len();
        let expected = base as f64 * scale.sqrt();
        assert!(
            (scaled as f64 - expected).abs() <= 0.15 * expected + 1.0,
            "at scale {scale} the flattener emitted {scaled} segments, expected about {expected}"
        );
    }
    // Shrinking asks for fewer, not the same.
    let shrunk = flatten(&path, Affine::scale(0.05), tolerance).len();
    assert!(shrunk < base, "a shrunk path should flatten more coarsely");

    // A scaled transform and pre-scaled geometry must agree: the tolerance
    // came from the transform, so the two describe the same device-space curve.
    let big = [
        Point::new(curve[0].x * 4.0, curve[0].y * 4.0),
        Point::new(curve[1].x * 4.0, curve[1].y * 4.0),
        Point::new(curve[2].x * 4.0, curve[2].y * 4.0),
        Point::new(curve[3].x * 4.0, curve[3].y * 4.0),
    ];
    let pre_scaled = flatten(&cubic_path(big), Affine::IDENTITY, tolerance).len();
    let transformed = flatten(&path, Affine::scale(4.0), tolerance).len();
    assert_eq!(pre_scaled, transformed);
}

#[test]
fn a_rotation_does_not_change_the_segment_count() {
    let path = PathBuilder::new()
        .rounded_rect(
            Rect::new(10.0, 10.0, 210.0, 130.0),
            RectRadii::uniform(24.0),
        )
        .build();
    let straight = flatten(&path, Affine::IDENTITY, 0.25).len();
    for turns in 1..8 {
        let angle = turns as f64 * 0.37;
        let rotated = flatten(&path, Affine::rotate(angle), 0.25).len();
        assert!(
            (rotated as i64 - straight as i64).abs() <= 2,
            "rotation by {angle} changed the segment count from {straight} to {rotated}"
        );
    }
}

#[test]
fn degenerate_curves_do_not_panic_and_stay_finite() {
    let point = Point::new(7.0, 9.0);
    let cases: Vec<[Point; 4]> = vec![
        // Everything at one place.
        [point; 4],
        // Zero-length with control points elsewhere.
        [point, Point::new(20.0, 9.0), Point::new(-4.0, 9.0), point],
        // Collinear: a straight line wearing a curve's clothes.
        [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(20.0, 0.0),
            Point::new(30.0, 0.0),
        ],
        // Collinear but doubling back — a cusp at the turn.
        [
            Point::new(0.0, 0.0),
            Point::new(30.0, 0.0),
            Point::new(-30.0, 0.0),
            Point::new(0.0, 0.0),
        ],
        // A genuine cusp.
        [
            Point::new(0.0, 0.0),
            Point::new(60.0, 60.0),
            Point::new(-60.0, 60.0),
            Point::new(0.0, 0.0),
        ],
        // A loop.
        [
            Point::new(0.0, 0.0),
            Point::new(200.0, 100.0),
            Point::new(-200.0, 100.0),
            Point::new(0.0, 0.0),
        ],
        // Two coincident control points at the start.
        [
            Point::new(5.0, 5.0),
            Point::new(5.0, 5.0),
            Point::new(80.0, 40.0),
            Point::new(90.0, 90.0),
        ],
        // Very large coordinates.
        [
            Point::new(-1.0e7, 3.0e7),
            Point::new(1.0e7, -2.0e7),
            Point::new(4.0e7, 5.0e7),
            Point::new(-3.0e7, 1.0e7),
        ],
        // Very small ones.
        [
            Point::new(0.0, 0.0),
            Point::new(1.0e-7, 2.0e-7),
            Point::new(-1.0e-7, 3.0e-7),
            Point::new(2.0e-7, 0.0),
        ],
    ];
    for curve in cases {
        for tolerance in [1.0e-3, 0.25, 4.0] {
            let segments = flatten(&cubic_path(curve), Affine::IDENTITY, tolerance);
            for segment in &segments {
                assert!(
                    segment.is_finite(),
                    "non-finite segment {segment:?} from {curve:?}"
                );
            }
        }
    }
}

#[test]
fn a_degenerate_transform_still_terminates() {
    let path = cubic_path([
        Point::new(0.0, 0.0),
        Point::new(40.0, 0.0),
        Point::new(100.0, 60.0),
        Point::new(100.0, 100.0),
    ]);
    for transform in [
        Affine::scale(0.0),
        Affine::scale_non_uniform(1.0, 0.0),
        Affine::new([0.0, 0.0, 0.0, 0.0, 12.0, 4.0]),
    ] {
        let segments = flatten(&path, transform, 0.25);
        for segment in &segments {
            assert!(segment.is_finite());
        }
    }
}

#[test]
fn a_quadratic_flattens_like_the_cubic_it_equals() {
    let (start, control, end) = (
        Point::new(0.0, 0.0),
        Point::new(120.0, 200.0),
        Point::new(240.0, 0.0),
    );
    let quadratic = PathBuilder::new()
        .move_to(start)
        .quad_to(control, end)
        .build();
    let elevated = cubic_path([
        start,
        Point::new(
            start.x + (2.0 / 3.0) * (control.x - start.x),
            start.y + (2.0 / 3.0) * (control.y - start.y),
        ),
        Point::new(
            end.x + (2.0 / 3.0) * (control.x - end.x),
            end.y + (2.0 / 3.0) * (control.y - end.y),
        ),
        end,
    ]);
    let a = flatten(&quadratic, Affine::IDENTITY, 0.25);
    let b = flatten(&elevated, Affine::IDENTITY, 0.25);
    assert_eq!(a, b);
}

#[test]
fn a_circle_flattens_to_the_count_its_geometry_asks_for() {
    // Chords of a circle stay within `tolerance` when each spans an angle of
    // `2·acos(1 − tolerance/radius)`, which is the exact optimum a flattener
    // can reach. Being inside 15% of it is the whole point of the exercise.
    for radius in [8.0, 40.0, 200.0, 900.0] {
        let tolerance = 0.25;
        let path = PathBuilder::new()
            .circle(Point::new(radius + 4.0, radius + 4.0), radius)
            .build();
        let count = flatten(&path, Affine::IDENTITY, tolerance).len();
        println!(
            "circle r={radius}: {count} segments, ideal {:.1}",
            core::f64::consts::TAU / (2.0 * (1.0 - tolerance / radius).clamp(-1.0, 1.0).acos())
        );
        let ideal =
            core::f64::consts::TAU / (2.0 * (1.0 - tolerance / radius).clamp(-1.0, 1.0).acos());
        assert!(
            count as f64 <= ideal * 1.15 + 4.0,
            "a circle of radius {radius} took {count} segments against an ideal of {ideal:.1}"
        );
        assert!(
            count as f64 >= ideal * 0.85 - 4.0,
            "a circle of radius {radius} took only {count} segments; the ideal is {ideal:.1}"
        );
    }
}

#[test]
fn every_verb_advances_the_cursor_correctly() {
    // A path mixing all four curve verbs must consume its point stream exactly;
    // a miscount would silently shift the geometry.
    let path = PathBuilder::new()
        .move_to(Point::new(0.0, 0.0))
        .line_to(Point::new(50.0, 0.0))
        .quad_to(Point::new(80.0, 30.0), Point::new(50.0, 60.0))
        .curve_to(
            Point::new(20.0, 90.0),
            Point::new(-20.0, 30.0),
            Point::new(0.0, 20.0),
        )
        .close()
        .build();
    assert!(path.verbs().contains(&PathVerb::QuadTo));
    let segments = flatten(&path, Affine::IDENTITY, 0.25);
    assert!(segments.len() > 4);
    // The polyline must be continuous and must close.
    for pair in segments.windows(2) {
        assert_eq!((pair[0].x1, pair[0].y1), (pair[1].x0, pair[1].y0));
    }
    // The closing edge returns to the subpath's start. The opening edge is
    // horizontal and dropped, so the first segment is not where to look for it.
    let last = segments.last().expect("segments");
    assert!(
        last.x1.abs() < 1e-3 && last.y1.abs() < 1e-3,
        "the closing edge ended at ({}, {})",
        last.x1,
        last.y1
    );
}
