//! The T3.2 fuzz target: random paths against random stroke styles.
//!
//! The property is the one a consumer can rely on without reading the code:
//! whatever path and style go in, stage 3 returns, and every coordinate it
//! produces is a real number. Nothing here checks that the outline is *right*
//! — `stroke.rs` does that against areas worked out on paper — only that no
//! input reaches the rasterizer as a panic or a `NaN`.
//!
//! Proptest rather than `cargo-fuzz`, for the reason `builder_fuzz` gives:
//! the input is a structure, not a byte string, and proptest generates
//! structures natively while shrinking counterexamples down to something a
//! person can read.

use otf_2d_engine_geom::{Affine, PathBuilder, Point};
use otf_2d_engine_raster::{Flattener, StrokeSpec};
use otf_2d_engine_scene::{Cap, Join};
use proptest::prelude::*;

/// One path verb, with room for the degenerate cases: coordinates that repeat,
/// coordinates far enough apart to lose precision, and subpaths with no length
/// at all.
#[derive(Debug, Clone)]
enum Verb {
    Move(f64, f64),
    Line(f64, f64),
    Quad(f64, f64, f64, f64),
    Curve(f64, f64, f64, f64, f64, f64),
    Close,
}

fn coordinate() -> impl Strategy<Value = f64> {
    prop_oneof![
        6 => -200.0f64..200.0,
        2 => -1e6f64..1e6,
        1 => Just(0.0),
        1 => Just(-1e-9),
    ]
}

fn verb() -> impl Strategy<Value = Verb> {
    prop_oneof![
        2 => (coordinate(), coordinate()).prop_map(|(x, y)| Verb::Move(x, y)),
        4 => (coordinate(), coordinate()).prop_map(|(x, y)| Verb::Line(x, y)),
        3 => (coordinate(), coordinate(), coordinate(), coordinate())
            .prop_map(|(a, b, c, d)| Verb::Quad(a, b, c, d)),
        4 => (
            coordinate(),
            coordinate(),
            coordinate(),
            coordinate(),
            coordinate(),
            coordinate()
        )
            .prop_map(|(a, b, c, d, e, f)| Verb::Curve(a, b, c, d, e, f)),
        2 => Just(Verb::Close),
    ]
}

fn join() -> impl Strategy<Value = Join> {
    prop_oneof![
        Just(Join::Bevel),
        Just(Join::Round),
        (-1.0f32..1e3).prop_map(|limit| Join::Miter { limit }),
    ]
}

fn cap() -> impl Strategy<Value = Cap> {
    prop_oneof![Just(Cap::Butt), Just(Cap::Round), Just(Cap::Square)]
}

fn width() -> impl Strategy<Value = f64> {
    prop_oneof![
        6 => 1e-3f64..80.0,
        1 => Just(0.0),
        1 => -10.0f64..0.0,
        1 => 1e-9f64..1e-6,
        1 => 1e5f64..1e7,
    ]
}

fn transform() -> impl Strategy<Value = Affine> {
    prop_oneof![
        4 => Just(Affine::IDENTITY),
        2 => (0.01f64..40.0).prop_map(Affine::scale),
        2 => (-6.3f64..6.3).prop_map(Affine::rotate),
        1 => Just(Affine::new([0.0, 0.0, 0.0, 0.0, 0.0, 0.0])),
        1 => Just(Affine::new([1.0, 2.0, 2.0, 4.0, 0.0, 0.0])),
    ]
}

fn build(verbs: &[Verb]) -> (Vec<u8>, Vec<f64>) {
    let mut builder = PathBuilder::new();
    for verb in verbs {
        match *verb {
            Verb::Move(x, y) => {
                builder.move_to(Point::new(x, y));
            }
            Verb::Line(x, y) => {
                builder.line_to(Point::new(x, y));
            }
            Verb::Quad(a, b, c, d) => {
                builder.quad_to(Point::new(a, b), Point::new(c, d));
            }
            Verb::Curve(a, b, c, d, e, f) => {
                builder.curve_to(Point::new(a, b), Point::new(c, d), Point::new(e, f));
            }
            Verb::Close => {
                builder.close();
            }
        }
    }
    let path = builder.build();
    (
        path.verbs().iter().map(|v| *v as u8).collect(),
        path.points().iter().flat_map(|p| [p.x, p.y]).collect(),
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, ..ProptestConfig::default() })]

    #[test]
    fn expansion_never_panics_and_never_leaves_a_coordinate_unreal(
        verbs in prop::collection::vec(verb(), 0..24),
        width in width(),
        join in join(),
        start_cap in cap(),
        end_cap in cap(),
        transform in transform(),
        tolerance in prop_oneof![Just(0.25f64), 1e-3f64..4.0, Just(0.0), Just(f64::NAN)],
    ) {
        let (raw_verbs, points) = build(&verbs);
        let spec = StrokeSpec {
            width,
            join,
            start_cap,
            end_cap,
        };
        let mut flattener = Flattener::new();
        flattener.add_stroke(&raw_verbs, &points, transform, tolerance, &spec);
        for segment in flattener.segments() {
            prop_assert!(
                segment.is_finite(),
                "stroke expansion produced {segment:?}"
            );
        }
    }
}
