//! T1.4 acceptance tests for `SceneBuilder`.
//!
//! One test per [`EncodeError`] variant, each asserting the specific error,
//! plus the structural guarantees behind I-8: a scene that encoded is balanced,
//! every handle in it is in range, and encoding a frame allocates nothing.

use otf_2d_engine_color::{BlendMode, Color};
use otf_2d_engine_geom::{Affine, Path, PathBuilder, Point, Rect, Vec2};
use otf_2d_engine_scene::{
    Cap, ColorStop, Dash, EncodeError, Extend, FillRule, FontRef, Glyph, GlyphOptions, ImageRef,
    Join, MAX_LAYER_DEPTH, MAX_PATH_POINTS, NO_REF, NodeId, Paint, Sampling, Scene, SceneBuilder,
    StrokeStyle,
};
use otf_2d_engine_testing::alloc::{CountingAllocator, measure};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator::new();

fn square() -> Path {
    PathBuilder::new()
        .rect(Rect::new(0.0, 0.0, 10.0, 10.0))
        .build()
}

fn black() -> Paint {
    Paint::Solid(Color::from_srgb8(0, 0, 0, 255))
}

/// Every scene a builder produces must survive full structural validation,
/// which is what `from_bytes` runs. Stage 2 gets the same guarantee.
fn assert_valid(scene: &Scene) {
    Scene::from_bytes(&scene.to_bytes()).expect("a builder-produced scene must validate");
    for layer in scene.layers() {
        assert_ne!(layer.pop_tag, NO_REF, "layer left open");
    }
}

// ------------------------------------------------------------ happy path

#[test]
fn a_simple_frame_encodes_and_validates() {
    let mut scene = Scene::new();
    let mut sb = SceneBuilder::new(&mut scene);
    sb.fill(FillRule::NonZero, Affine::IDENTITY, &black(), &square())
        .expect("fill");
    sb.stroke(
        &StrokeStyle::new(2.0)
            .with_join(Join::Round)
            .with_caps(Cap::Square),
        Affine::translate(Vec2::new(1.0, 1.0)),
        &black(),
        &square(),
    )
    .expect("stroke");
    sb.draw_image(ImageRef::new(0), Affine::IDENTITY, Sampling::Bilinear, 0.5)
        .expect("image");
    sb.finish().expect("balanced");

    assert_eq!(scene.tags().len(), 3);
    assert_valid(&scene);
}

#[test]
fn gradients_name_their_stops_by_handle() {
    let mut scene = Scene::new();
    let mut sb = SceneBuilder::new(&mut scene);
    let stops = sb
        .intern_stops(&[
            ColorStop::new(0.0, Color::from_srgb8(255, 0, 0, 255)),
            ColorStop::new(1.0, Color::from_srgb8(0, 0, 255, 255)),
        ])
        .expect("stops");
    sb.fill(
        FillRule::NonZero,
        Affine::IDENTITY,
        &Paint::LinearGradient {
            start: Point::new(0.0, 0.0),
            end: Point::new(10.0, 0.0),
            stops,
            extend: Extend::Pad,
        },
        &square(),
    )
    .expect("fill");
    sb.finish().expect("balanced");

    assert_eq!(scene.stops_of(stops).len(), 2, "the handle carries its run");
    let paint = scene.paints().last().expect("a paint");
    assert_eq!(scene.paint_stops(paint).len(), 2);
    assert_valid(&scene);
}

#[test]
fn a_glyph_run_carries_its_variations() {
    let mut scene = Scene::new();
    let mut sb = SceneBuilder::new(&mut scene);
    let variations = sb.intern_variations(&[400.0, 100.0]).expect("variations");
    sb.draw_glyphs(
        FontRef::new(0),
        16.0,
        Affine::IDENTITY,
        &black(),
        &[Glyph::new(1, 0.0, 0.0), Glyph::new(2, 9.0, 0.0)],
        GlyphOptions {
            variations,
            ..GlyphOptions::default()
        },
    )
    .expect("glyphs");
    sb.finish().expect("balanced");

    let run = scene.glyph_runs().last().expect("a run");
    assert_eq!(scene.run_glyphs(run).len(), 2);
    assert_eq!(scene.run_variations(run), &[400.0, 100.0]);
    assert_valid(&scene);
}

#[test]
fn layers_nest_and_balance() {
    let mut scene = Scene::new();
    let mut sb = SceneBuilder::new(&mut scene);
    sb.push_layer(BlendMode::SrcOver, 0.5, Affine::IDENTITY, Some(&square()))
        .expect("push");
    assert_eq!(sb.depth(), 1);
    sb.push_layer(BlendMode::SrcOver, 1.0, Affine::IDENTITY, None)
        .expect("push");
    sb.fill(FillRule::NonZero, Affine::IDENTITY, &black(), &square())
        .expect("fill");
    sb.pop_layer().expect("pop");
    sb.pop_layer().expect("pop");
    assert_eq!(sb.depth(), 0);
    sb.finish().expect("balanced");

    assert_eq!(scene.layers().len(), 2);
    assert_valid(&scene);
}

#[test]
fn a_node_scope_closes_its_node() {
    let mut scene = Scene::new();
    let mut sb = SceneBuilder::new(&mut scene);
    assert!(
        !sb.reuse_node(NodeId(1), Affine::IDENTITY),
        "the node cache lands in M6; nothing can be reused yet"
    );
    {
        let mut outer = sb.push_node(NodeId(1));
        outer
            .fill(FillRule::NonZero, Affine::IDENTITY, &black(), &square())
            .expect("fill");
        {
            let mut inner = outer.push_node(NodeId(2));
            inner
                .fill(FillRule::NonZero, Affine::IDENTITY, &black(), &square())
                .expect("fill");
        }
    }
    sb.finish().expect("balanced");

    let nodes = scene.node_descs();
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].tag_len, 2, "the outer node covers both fills");
    assert_eq!(nodes[1].tag_len, 1);
    assert_eq!(
        nodes[1].parent, 0,
        "nesting is recorded from the scope stack"
    );
    assert_valid(&scene);
}

// ------------------------------------------------------------ error variants

#[test]
fn a_non_finite_transform_is_rejected() {
    let mut scene = Scene::new();
    let mut sb = SceneBuilder::new(&mut scene);
    let broken = Affine::translate(Vec2::new(f64::NAN, 0.0));
    assert_eq!(
        sb.fill(FillRule::NonZero, broken, &black(), &square()),
        Err(EncodeError::NonFiniteCoordinate)
    );
    assert!(sb.scene().is_empty(), "a rejected call encodes nothing");
}

#[test]
fn every_non_finite_argument_is_rejected() {
    let mut scene = Scene::new();
    let mut sb = SceneBuilder::new(&mut scene);
    let inf = f64::INFINITY;

    let mut b = PathBuilder::new();
    b.move_to(Point::new(0.0, 0.0));
    b.line_to(Point::new(f64::NAN, 1.0));
    let broken_path = b.build();
    assert_eq!(
        sb.fill(FillRule::NonZero, Affine::IDENTITY, &black(), &broken_path),
        Err(EncodeError::NonFiniteCoordinate),
        "path coordinate"
    );

    let broken_paint = Paint::RadialGradient {
        center: Point::new(0.0, 0.0),
        radius: inf,
        focal: None,
        stops: otf_2d_engine_scene::StopsRef::NONE,
        extend: Extend::Pad,
    };
    assert_eq!(
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &broken_paint,
            &square()
        ),
        Err(EncodeError::NonFiniteCoordinate),
        "gradient geometry"
    );

    assert_eq!(
        sb.stroke(
            &StrokeStyle::new(f32::NAN),
            Affine::IDENTITY,
            &black(),
            &square()
        ),
        Err(EncodeError::NonFiniteCoordinate),
        "stroke width"
    );
    assert_eq!(
        sb.stroke(
            &StrokeStyle::new(1.0).with_dash(Dash::new(&[1.0, f32::NAN], 0.0)),
            Affine::IDENTITY,
            &black(),
            &square()
        ),
        Err(EncodeError::NonFiniteCoordinate),
        "dash pattern"
    );
    assert_eq!(
        sb.draw_image(
            ImageRef::new(0),
            Affine::IDENTITY,
            Sampling::Nearest,
            f32::NAN
        ),
        Err(EncodeError::NonFiniteCoordinate),
        "image alpha"
    );
    assert_eq!(
        sb.push_layer(BlendMode::SrcOver, f32::NAN, Affine::IDENTITY, None),
        Err(EncodeError::NonFiniteCoordinate),
        "layer alpha"
    );
    assert_eq!(
        sb.intern_stops(&[ColorStop::new(f32::NAN, Color::TRANSPARENT)]),
        Err(EncodeError::NonFiniteCoordinate),
        "stop offset"
    );
    assert_eq!(
        sb.intern_variations(&[f32::INFINITY]),
        Err(EncodeError::NonFiniteCoordinate),
        "variation coordinate"
    );

    assert!(sb.scene().is_empty(), "no rejected call encoded anything");
}

#[test]
fn popping_a_layer_that_is_not_open_is_unbalanced() {
    let mut scene = Scene::new();
    let mut sb = SceneBuilder::new(&mut scene);
    assert_eq!(sb.pop_layer(), Err(EncodeError::UnbalancedLayer));
}

#[test]
fn finishing_with_an_open_layer_is_unbalanced() {
    let mut scene = Scene::new();
    let mut sb = SceneBuilder::new(&mut scene);
    sb.push_layer(BlendMode::SrcOver, 1.0, Affine::IDENTITY, None)
        .expect("push");
    assert_eq!(sb.finish(), Err(EncodeError::UnbalancedLayer));

    // Reported, and still closed: an unbalanced push must not reach stage 6.
    assert_valid(&scene);
}

#[test]
fn a_dropped_builder_closes_open_layers() {
    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        sb.push_layer(BlendMode::SrcOver, 1.0, Affine::IDENTITY, None)
            .expect("push");
        sb.push_layer(BlendMode::SrcOver, 1.0, Affine::IDENTITY, None)
            .expect("push");
        sb.fill(FillRule::NonZero, Affine::IDENTITY, &black(), &square())
            .expect("fill");
    }
    assert_valid(&scene);
    assert_eq!(scene.layers().len(), 2);
}

#[test]
fn layer_nesting_has_a_hard_limit() {
    let mut scene = Scene::new();
    let mut sb = SceneBuilder::new(&mut scene);
    for _ in 0..MAX_LAYER_DEPTH {
        sb.push_layer(BlendMode::SrcOver, 1.0, Affine::IDENTITY, None)
            .expect("push");
    }
    assert_eq!(
        sb.push_layer(BlendMode::SrcOver, 1.0, Affine::IDENTITY, None),
        Err(EncodeError::LayerDepthExceeded {
            max: MAX_LAYER_DEPTH
        })
    );
    while sb.depth() > 0 {
        sb.pop_layer().expect("pop");
    }
    sb.finish().expect("balanced");
    assert_valid(&scene);
}

#[test]
fn a_path_beyond_the_limit_is_rejected() {
    let mut b = PathBuilder::with_capacity(MAX_PATH_POINTS + 2, MAX_PATH_POINTS + 2);
    b.move_to(Point::new(0.0, 0.0));
    for i in 0..MAX_PATH_POINTS {
        b.line_to(Point::new(i as f64, 1.0));
    }
    let huge = b.build();
    assert!(huge.points().len() > MAX_PATH_POINTS);

    let mut scene = Scene::new();
    let mut sb = SceneBuilder::new(&mut scene);
    assert_eq!(
        sb.fill(FillRule::NonZero, Affine::IDENTITY, &black(), &huge),
        Err(EncodeError::PathTooLarge {
            limit: MAX_PATH_POINTS
        })
    );
    assert!(sb.scene().is_empty());
}

#[test]
fn an_empty_or_malformed_glyph_run_is_rejected() {
    let mut scene = Scene::new();
    let mut sb = SceneBuilder::new(&mut scene);
    let options = GlyphOptions::default();
    let glyph = [Glyph::new(1, 0.0, 0.0)];

    assert_eq!(
        sb.draw_glyphs(
            FontRef::new(0),
            16.0,
            Affine::IDENTITY,
            &black(),
            &[],
            options
        ),
        Err(EncodeError::InvalidGlyphRun),
        "empty run"
    );
    assert_eq!(
        sb.draw_glyphs(
            FontRef::new(0),
            0.0,
            Affine::IDENTITY,
            &black(),
            &glyph,
            options
        ),
        Err(EncodeError::InvalidGlyphRun),
        "zero size"
    );
    assert_eq!(
        sb.draw_glyphs(
            FontRef::new(0),
            f32::NAN,
            Affine::IDENTITY,
            &black(),
            &glyph,
            options
        ),
        Err(EncodeError::InvalidGlyphRun),
        "non-finite size"
    );
    assert_eq!(
        sb.draw_glyphs(
            FontRef::new(0),
            16.0,
            Affine::IDENTITY,
            &black(),
            &[Glyph::new(1, f32::INFINITY, 0.0)],
            options
        ),
        Err(EncodeError::InvalidGlyphRun),
        "non-finite position"
    );
    assert_eq!(
        sb.draw_glyphs(
            FontRef::new(0),
            16.0,
            Affine::IDENTITY,
            &black(),
            &glyph,
            GlyphOptions {
                synthetic_skew: f32::NAN,
                ..options
            }
        ),
        Err(EncodeError::InvalidGlyphRun),
        "non-finite skew"
    );
    assert!(sb.scene().is_empty());
}

#[test]
fn errors_describe_themselves() {
    let text = format!("{}", EncodeError::LayerDepthExceeded { max: 8 });
    assert!(text.contains('8'), "{text}");
}

// ------------------------------------------------------------ I-9

#[test]
fn encoding_a_frame_through_the_builder_allocates_nothing() {
    let paths: Vec<Path> = (0..200)
        .map(|i| {
            PathBuilder::new()
                .rect(Rect::new(i as f64, 0.0, i as f64 + 4.0, 4.0))
                .build()
        })
        .collect();
    let paint = black();
    let stroke = StrokeStyle::new(1.5);

    let frame = |scene: &mut Scene| {
        let mut sb = SceneBuilder::new(scene);
        sb.push_layer(BlendMode::SrcOver, 1.0, Affine::IDENTITY, Some(&paths[0]))
            .expect("push");
        for (i, path) in paths.iter().enumerate() {
            let transform = Affine::translate(Vec2::new(i as f64, 0.0));
            if i % 2 == 0 {
                sb.fill(FillRule::NonZero, transform, &paint, path)
                    .expect("fill");
            } else {
                sb.stroke(&stroke, transform, &paint, path).expect("stroke");
            }
        }
        sb.pop_layer().expect("pop");
        sb.finish().expect("balanced");
    };

    let mut scene = Scene::new();
    let (_, first) = measure(|| frame(&mut scene));
    assert!(
        first.acquisitions() > 0,
        "the counting allocator is not installed"
    );
    let hash = scene.content_hash();

    scene.reset();
    let (_, second) = measure(|| frame(&mut scene));
    assert_eq!(
        second.acquisitions(),
        0,
        "I-9: a builder-encoded frame allocated ({second:?})"
    );
    assert_eq!(scene.content_hash(), hash);
    assert_valid(&scene);
}
