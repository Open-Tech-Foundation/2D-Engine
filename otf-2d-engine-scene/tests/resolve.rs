//! T1.5 acceptance tests for stage 2.
//!
//! The four criteria from the plan: no tree in the output, absolute transforms
//! that match hand-computed composition, off-target draws absent, and output
//! that is resolution-independent.

use otf_2d_engine_color::{BlendMode, Color};
use otf_2d_engine_geom::{Affine, Path, PathBuilder, PathVerb, Point, Rect, RectRadii, Vec2};
use otf_2d_engine_scene::{
    Cap, ClipMask, FillRule, Join, Paint, ResolveParams, ResolvedClip, ResolvedDraw, ResolvedKind,
    ResolvedLayer, Resolver, Scene, SceneBuilder, StrokeStyle,
};
use otf_2d_engine_testing::alloc::{CountingAllocator, measure};
use proptest::prelude::*;

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator::new();

// Criterion 1, type-level: a recursive field would need heap indirection, and
// nothing heap-indirected is `Copy`. These four records are the whole output.
static_assertions::assert_impl_all!(ResolvedDraw: Copy);
static_assertions::assert_impl_all!(ResolvedLayer: Copy);
static_assertions::assert_impl_all!(ResolvedClip: Copy);
static_assertions::assert_impl_all!(ClipMask: Copy);

fn target() -> Rect {
    Rect::new(0.0, 0.0, 100.0, 100.0)
}

fn rect_path(r: Rect) -> Path {
    PathBuilder::new().rect(r).build()
}

/// A path with a cubic in it, so "curves stay curves" has something to check.
fn curvy() -> Path {
    let mut b = PathBuilder::new();
    b.move_to(Point::new(10.0, 10.0));
    b.curve_to(
        Point::new(20.0, 40.0),
        Point::new(40.0, -20.0),
        Point::new(50.0, 10.0),
    );
    b.line_to(Point::new(30.0, 50.0));
    b.close();
    b.build()
}

fn black() -> Paint {
    Paint::Solid(Color::from_srgb8(0, 0, 0, 255))
}

/// Composes two affines by hand, from the `[a b c d e f]` definition, so the
/// transform test is not checking `then` against itself.
fn compose(first: [f64; 6], second: [f64; 6]) -> [f64; 6] {
    let [a1, b1, c1, d1, e1, f1] = first;
    let [a2, b2, c2, d2, e2, f2] = second;
    [
        a2 * a1 + c2 * b1,
        b2 * a1 + d2 * b1,
        a2 * c1 + c2 * d1,
        b2 * c1 + d2 * d1,
        a2 * e1 + c2 * f1 + e2,
        b2 * e1 + d2 * f1 + f2,
    ]
}

fn assert_affine_eq(actual: Affine, expected: [f64; 6]) {
    let got = actual.as_coefficients();
    for (i, (g, e)) in got.iter().zip(expected.iter()).enumerate() {
        assert!(
            (g - e).abs() <= 1e-9 * e.abs().max(1.0),
            "coefficient {i}: {g} != {e} (got {got:?}, want {expected:?})"
        );
    }
}

// ------------------------------------------------------------ transforms

#[test]
fn a_draws_transform_is_its_own_affine_composed_with_the_device_transform() {
    let scene_affine = Affine::translate(Vec2::new(3.0, 5.0));
    let device = Affine::scale(2.0);

    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        sb.fill(
            FillRule::NonZero,
            scene_affine,
            &black(),
            &rect_path(Rect::new(0.0, 0.0, 10.0, 10.0)),
        )
        .expect("fill");
        sb.finish().expect("balanced");
    }

    let mut resolver = Resolver::new();
    let resolved = resolver.resolve(&scene, &ResolveParams::new(target()).with_transform(device));

    assert_eq!(resolved.draws().len(), 1);
    assert_affine_eq(
        resolved.draws()[0].transform,
        compose(scene_affine.as_coefficients(), device.as_coefficients()),
    );
}

#[test]
fn enclosing_layers_do_not_change_a_draws_transform() {
    // There is no transform stack to inherit from: `SceneBuilder` has no
    // current transform (I-3), and a layer's transform describes its clip.
    let draw_affine = Affine::rotate(0.7).then(Affine::translate(Vec2::new(2.0, 1.0)));
    let device = Affine::scale(1.5);

    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        for depth in 0..3 {
            sb.push_layer(
                BlendMode::SrcOver,
                1.0,
                Affine::translate(Vec2::new(depth as f64 * 7.0, 0.0)),
                None,
            )
            .expect("push");
        }
        sb.fill(
            FillRule::NonZero,
            draw_affine,
            &black(),
            &rect_path(Rect::new(0.0, 0.0, 4.0, 4.0)),
        )
        .expect("fill");
        for _ in 0..3 {
            sb.pop_layer().expect("pop");
        }
        sb.finish().expect("balanced");
    }

    let mut resolver = Resolver::new();
    let resolved = resolver.resolve(&scene, &ResolveParams::new(target()).with_transform(device));
    let fill = resolved
        .draws()
        .iter()
        .find(|d| matches!(d.kind, ResolvedKind::Fill { .. }))
        .expect("the fill survived");
    assert_affine_eq(
        fill.transform,
        compose(draw_affine.as_coefficients(), device.as_coefficients()),
    );
}

// ------------------------------------------------------------ no tree

#[test]
fn layer_nesting_survives_only_as_order() {
    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        sb.push_layer(BlendMode::SrcOver, 0.5, Affine::IDENTITY, None)
            .expect("push");
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &black(),
            &rect_path(Rect::new(0.0, 0.0, 10.0, 10.0)),
        )
        .expect("fill");
        sb.push_layer(BlendMode::SrcOver, 1.0, Affine::IDENTITY, None)
            .expect("push");
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &black(),
            &rect_path(Rect::new(2.0, 2.0, 6.0, 6.0)),
        )
        .expect("fill");
        sb.pop_layer().expect("pop");
        sb.pop_layer().expect("pop");
        sb.finish().expect("balanced");
    }

    let mut resolver = Resolver::new();
    let resolved = resolver.resolve(&scene, &ResolveParams::new(target()));

    let kinds: Vec<_> = resolved.draws().iter().map(|d| d.kind).collect();
    assert!(matches!(kinds[0], ResolvedKind::BeginLayer { layer: 0 }));
    assert!(matches!(kinds[1], ResolvedKind::Fill { .. }));
    assert!(matches!(kinds[2], ResolvedKind::BeginLayer { layer: 1 }));
    assert!(matches!(kinds[3], ResolvedKind::Fill { .. }));
    assert!(matches!(kinds[4], ResolvedKind::EndLayer { layer: 1 }));
    assert!(matches!(kinds[5], ResolvedKind::EndLayer { layer: 0 }));

    let outer = resolved.layers()[0];
    assert_eq!(outer.first_draw, 0);
    assert_eq!(outer.draw_len, 6, "the outer layer spans the whole list");
    let inner = resolved.layers()[1];
    assert_eq!(inner.first_draw, 2);
    assert_eq!(inner.draw_len, 3);
    assert_eq!(
        outer.bounds,
        Rect::new(0.0, 0.0, 10.0, 10.0),
        "a layer sizes to its content, not the surface"
    );
    assert_eq!(inner.bounds, Rect::new(2.0, 2.0, 6.0, 6.0));
}

// ------------------------------------------------------------ culling

#[test]
fn draws_outside_the_target_are_absent() {
    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        // Inside, straddling, and far outside.
        for r in [
            Rect::new(10.0, 10.0, 20.0, 20.0),
            Rect::new(-5.0, -5.0, 5.0, 5.0),
            Rect::new(500.0, 500.0, 600.0, 600.0),
            Rect::new(-600.0, 10.0, -500.0, 20.0),
        ] {
            sb.fill(FillRule::NonZero, Affine::IDENTITY, &black(), &rect_path(r))
                .expect("fill");
        }
        sb.finish().expect("balanced");
    }

    let mut resolver = Resolver::new();
    let resolved = resolver.resolve(&scene, &ResolveParams::new(target()));
    assert_eq!(resolved.draws().len(), 2);
    assert_eq!(resolved.stats().culled, 2);
    for draw in resolved.draws() {
        assert!(draw.bounds.intersects(target()));
    }
}

#[test]
fn culling_is_disableable_and_keeps_every_visible_draw() {
    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        for i in 0..12 {
            let x = i as f64 * 40.0 - 200.0;
            sb.fill(
                FillRule::NonZero,
                Affine::IDENTITY,
                &black(),
                &rect_path(Rect::new(x, 10.0, x + 20.0, 30.0)),
            )
            .expect("fill");
        }
        sb.finish().expect("balanced");
    }

    let mut resolver = Resolver::new();
    let culled: Vec<ResolvedDraw> = resolver
        .resolve(&scene, &ResolveParams::new(target()))
        .draws()
        .to_vec();
    let uncut: Vec<ResolvedDraw> = resolver
        .resolve(&scene, &ResolveParams::new(target()).without_culling())
        .draws()
        .to_vec();

    assert_eq!(
        uncut.len(),
        12,
        "P5: with the optimisation off, nothing is dropped"
    );
    assert!(culled.len() < uncut.len());
    // The two agree on everything that can affect the target, which is the
    // equality that makes culling safe to enable.
    let visible: Vec<ResolvedDraw> = uncut
        .into_iter()
        .filter(|d| d.bounds.intersects(target()))
        .collect();
    assert_eq!(culled, visible);
}

#[test]
fn damage_narrows_what_survives() {
    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &black(),
            &rect_path(Rect::new(0.0, 0.0, 10.0, 10.0)),
        )
        .expect("fill");
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &black(),
            &rect_path(Rect::new(80.0, 80.0, 90.0, 90.0)),
        )
        .expect("fill");
        sb.finish().expect("balanced");
    }

    let mut resolver = Resolver::new();
    let resolved = resolver.resolve(
        &scene,
        &ResolveParams::new(target()).with_damage(Rect::new(70.0, 70.0, 100.0, 100.0)),
    );
    assert_eq!(resolved.draws().len(), 1);
    assert_eq!(
        resolved.draws()[0].bounds,
        Rect::new(80.0, 80.0, 90.0, 90.0)
    );
}

#[test]
fn a_stroke_is_culled_by_its_outset_not_its_path() {
    // The path is entirely outside the target, but a fat stroke reaches in.
    let path = rect_path(Rect::new(-8.0, 10.0, -4.0, 20.0));
    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        sb.stroke(
            &StrokeStyle::new(20.0)
                .with_join(Join::Round)
                .with_caps(Cap::Butt),
            Affine::IDENTITY,
            &black(),
            &path,
        )
        .expect("stroke");
        sb.finish().expect("balanced");
    }

    let mut resolver = Resolver::new();
    let resolved = resolver.resolve(&scene, &ResolveParams::new(target()));
    assert_eq!(
        resolved.draws().len(),
        1,
        "a stroke reaching in must survive"
    );
    assert_eq!(resolved.stats().culled, 0);
}

#[test]
fn text_and_images_are_never_culled_before_their_extents_are_known() {
    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        sb.draw_glyphs(
            otf_2d_engine_scene::FontRef::new(0),
            16.0,
            Affine::translate(Vec2::new(-9000.0, -9000.0)),
            &black(),
            &[otf_2d_engine_scene::Glyph::new(1, 0.0, 0.0)],
            otf_2d_engine_scene::GlyphOptions::default(),
        )
        .expect("glyphs");
        sb.draw_image(
            otf_2d_engine_scene::ImageRef::new(0),
            Affine::translate(Vec2::new(-9000.0, -9000.0)),
            otf_2d_engine_scene::Sampling::Nearest,
            1.0,
        )
        .expect("image");
        sb.finish().expect("balanced");
    }

    let mut resolver = Resolver::new();
    let resolved = resolver.resolve(&scene, &ResolveParams::new(target()));
    assert_eq!(
        resolved.draws().len(),
        2,
        "outlines arrive in M4 and image extents live in the caller's registry; \
         culling on a guess would drop visible content"
    );
    assert_eq!(resolved.stats().culled, 0);
}

// ------------------------------------------------------------ clips

#[test]
fn nested_rectangular_clips_collapse_to_one_rect() {
    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        sb.push_layer(
            BlendMode::SrcOver,
            1.0,
            Affine::IDENTITY,
            Some(&rect_path(Rect::new(10.0, 10.0, 60.0, 60.0))),
        )
        .expect("push");
        sb.push_layer(
            BlendMode::SrcOver,
            1.0,
            Affine::IDENTITY,
            Some(&rect_path(Rect::new(40.0, 0.0, 90.0, 40.0))),
        )
        .expect("push");
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &black(),
            &rect_path(Rect::new(0.0, 0.0, 100.0, 100.0)),
        )
        .expect("fill");
        sb.pop_layer().expect("pop");
        sb.pop_layer().expect("pop");
        sb.finish().expect("balanced");
    }

    let mut resolver = Resolver::new();
    let resolved = resolver.resolve(&scene, &ResolveParams::new(target()));
    let fill = resolved
        .draws()
        .iter()
        .find(|d| matches!(d.kind, ResolvedKind::Fill { .. }))
        .expect("the fill");
    let clip = resolved.clip(fill);
    assert!(clip.is_rectangular(), "two rect clips need no mask");
    assert_eq!(clip.rect, Rect::new(40.0, 10.0, 60.0, 40.0));
    assert_eq!(resolved.masks().len(), 0);
}

#[test]
fn a_non_rectangular_clip_becomes_a_mask() {
    let rounded = PathBuilder::new()
        .rounded_rect(Rect::new(10.0, 10.0, 60.0, 60.0), RectRadii::uniform(8.0))
        .build();
    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        sb.push_layer(BlendMode::SrcOver, 1.0, Affine::IDENTITY, Some(&rounded))
            .expect("push");
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &black(),
            &rect_path(Rect::new(0.0, 0.0, 100.0, 100.0)),
        )
        .expect("fill");
        sb.pop_layer().expect("pop");
        sb.finish().expect("balanced");
    }

    let mut resolver = Resolver::new();
    let resolved = resolver.resolve(&scene, &ResolveParams::new(target()));
    let fill = resolved
        .draws()
        .iter()
        .find(|d| matches!(d.kind, ResolvedKind::Fill { .. }))
        .expect("the fill");
    let clip = resolved.clip(fill);
    assert!(!clip.is_rectangular());
    assert_eq!(resolved.clip_masks(&clip).len(), 1);
    // The rect still narrows: a mask does not stop the bounding box collapsing.
    assert_eq!(clip.rect, Rect::new(10.0, 10.0, 60.0, 60.0));
}

#[test]
fn a_rotated_rect_clip_cannot_collapse_to_a_rect() {
    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        sb.push_layer(
            BlendMode::SrcOver,
            1.0,
            Affine::rotate(0.3),
            Some(&rect_path(Rect::new(10.0, 10.0, 60.0, 60.0))),
        )
        .expect("push");
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &black(),
            &rect_path(Rect::new(0.0, 0.0, 100.0, 100.0)),
        )
        .expect("fill");
        sb.pop_layer().expect("pop");
        sb.finish().expect("balanced");
    }

    let mut resolver = Resolver::new();
    let resolved = resolver.resolve(&scene, &ResolveParams::new(target()));
    let fill = resolved
        .draws()
        .iter()
        .find(|d| matches!(d.kind, ResolvedKind::Fill { .. }))
        .expect("the fill");
    assert!(
        !resolved.clip(fill).is_rectangular(),
        "a rotated rectangle is not a device-space rectangle"
    );
}

#[test]
fn sibling_clips_do_not_borrow_each_others_masks() {
    let rounded = |r: Rect| {
        PathBuilder::new()
            .rounded_rect(r, RectRadii::uniform(4.0))
            .build()
    };
    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        sb.push_layer(
            BlendMode::SrcOver,
            1.0,
            Affine::IDENTITY,
            Some(&rounded(Rect::new(0.0, 0.0, 80.0, 80.0))),
        )
        .expect("push");
        for r in [
            Rect::new(10.0, 10.0, 30.0, 30.0),
            Rect::new(40.0, 40.0, 60.0, 60.0),
        ] {
            sb.push_layer(BlendMode::SrcOver, 1.0, Affine::IDENTITY, Some(&rounded(r)))
                .expect("push");
            sb.fill(
                FillRule::NonZero,
                Affine::IDENTITY,
                &black(),
                &rect_path(Rect::new(0.0, 0.0, 100.0, 100.0)),
            )
            .expect("fill");
            sb.pop_layer().expect("pop");
        }
        sb.pop_layer().expect("pop");
        sb.finish().expect("balanced");
    }

    let mut resolver = Resolver::new();
    let resolved = resolver.resolve(&scene, &ResolveParams::new(target()));
    let fills: Vec<&ResolvedDraw> = resolved
        .draws()
        .iter()
        .filter(|d| matches!(d.kind, ResolvedKind::Fill { .. }))
        .collect();
    assert_eq!(fills.len(), 2);
    for (fill, expected) in fills.iter().zip([
        Rect::new(10.0, 10.0, 30.0, 30.0),
        Rect::new(40.0, 40.0, 60.0, 60.0),
    ]) {
        let clip = resolved.clip(fill);
        let masks = resolved.clip_masks(&clip);
        assert_eq!(masks.len(), 2, "the outer rounded clip plus its own");
        assert_eq!(masks[1].bounds, expected, "a sibling's mask leaked in");
    }
}

// ------------------------------------------------------------ resolution

#[test]
fn output_is_resolution_independent() {
    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        sb.fill(FillRule::NonZero, Affine::IDENTITY, &black(), &curvy())
            .expect("fill");
        sb.finish().expect("balanced");
    }

    let mut resolver = Resolver::new();
    for scale in [0.1, 1.0, 64.0] {
        let resolved = resolver.resolve(
            &scene,
            &ResolveParams::new(Rect::new(-1e5, -1e5, 1e5, 1e5))
                .with_transform(Affine::scale(scale)),
        );
        let ResolvedKind::Fill { path, .. } = resolved.draws()[0].kind else {
            panic!("expected a fill");
        };
        let view = resolved.scene().path(path).expect("path");
        let verbs: Vec<PathVerb> = view.verbs().collect();
        assert!(
            verbs.contains(&PathVerb::CurveTo),
            "at scale {scale} the cubic became something else: {verbs:?}"
        );
        assert_eq!(
            verbs,
            curvy().verbs().to_vec(),
            "stage 2 must not touch geometry at all"
        );
    }
}

// ------------------------------------------------------------ robustness

#[test]
fn an_unbalanced_scene_still_resolves_to_a_balanced_list() {
    // Only the raw encoder can produce this; `SceneBuilder` cannot.
    let mut scene = Scene::new();
    let transform = scene.encode_transform(Affine::IDENTITY);
    let path = scene.encode_path(&rect_path(Rect::new(0.0, 0.0, 10.0, 10.0)));
    let paint = scene.encode_paint(&black());
    scene.encode_push_layer(BlendMode::SrcOver, 1.0, transform, path);
    scene.encode_fill(FillRule::NonZero, transform, paint, path);
    scene.encode_pop_layer(0);
    scene.encode_pop_layer(0); // one pop too many
    scene.encode_push_layer(BlendMode::SrcOver, 1.0, transform, path); // never popped

    let mut resolver = Resolver::new();
    let resolved = resolver.resolve(&scene, &ResolveParams::new(target()));

    let mut depth = 0i32;
    for draw in resolved.draws() {
        match draw.kind {
            ResolvedKind::BeginLayer { .. } => depth += 1,
            ResolvedKind::EndLayer { .. } => depth -= 1,
            _ => {}
        }
        assert!(depth >= 0, "an end without a begin reached the draw list");
    }
    assert_eq!(depth, 0, "the draw list must be balanced");
    assert_eq!(resolved.stats().unclosed_layers, 1);
}

#[test]
fn resolving_a_second_frame_allocates_nothing() {
    let paths: Vec<Path> = (0..150)
        .map(|i| rect_path(Rect::new(i as f64, 0.0, i as f64 + 8.0, 8.0)))
        .collect();
    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        sb.push_layer(BlendMode::SrcOver, 1.0, Affine::IDENTITY, Some(&paths[0]))
            .expect("push");
        for path in &paths {
            sb.fill(FillRule::NonZero, Affine::IDENTITY, &black(), path)
                .expect("fill");
        }
        sb.pop_layer().expect("pop");
        sb.finish().expect("balanced");
    }

    let params = ResolveParams::new(target());
    let mut resolver = Resolver::new();
    let (first_count, first) = measure(|| resolver.resolve(&scene, &params).draws().len());
    assert!(
        first.acquisitions() > 0,
        "the counting allocator is not installed"
    );

    let (second_count, second) = measure(|| resolver.resolve(&scene, &params).draws().len());
    assert_eq!(first_count, second_count);
    assert_eq!(
        second.acquisitions(),
        0,
        "I-9: a steady-state resolve allocated ({second:?})"
    );
}

// ------------------------------------------------------------ properties

proptest! {
    /// Criterion 2, over random input: the resolved affine is the hand-computed
    /// composition, whatever the nesting.
    #[test]
    fn resolved_transforms_match_manual_composition(
        draws in prop::collection::vec(
            prop::array::uniform6(-50.0f64..50.0),
            1..8,
        ),
        device in prop::array::uniform6(-4.0f64..4.0),
        depth in 0usize..4,
    ) {
        let device = Affine::new(device);
        let mut scene = Scene::new();
        {
            let mut sb = SceneBuilder::new(&mut scene);
            for _ in 0..depth {
                sb.push_layer(BlendMode::SrcOver, 1.0, Affine::IDENTITY, None)
                    .expect("push");
            }
            for coefficients in &draws {
                sb.fill(
                    FillRule::NonZero,
                    Affine::new(*coefficients),
                    &black(),
                    &rect_path(Rect::new(0.0, 0.0, 4.0, 4.0)),
                )
                .expect("fill");
            }
            for _ in 0..depth {
                sb.pop_layer().expect("pop");
            }
            sb.finish().expect("balanced");
        }

        let mut resolver = Resolver::new();
        let resolved = resolver.resolve(
            &scene,
            &ResolveParams::new(Rect::new(-1e9, -1e9, 1e9, 1e9)).with_transform(device),
        );
        let fills: Vec<Affine> = resolved
            .draws()
            .iter()
            .filter(|d| matches!(d.kind, ResolvedKind::Fill { .. }))
            .map(|d| d.transform)
            .collect();
        prop_assert_eq!(fills.len(), draws.len());
        for (actual, coefficients) in fills.iter().zip(&draws) {
            assert_affine_eq(*actual, compose(*coefficients, device.as_coefficients()));
        }
    }

    /// Culling never removes a draw that touches the visible region.
    #[test]
    fn culling_only_removes_invisible_draws(
        rects in prop::collection::vec(
            (-300.0f64..300.0, -300.0f64..300.0, 1.0f64..80.0, 1.0f64..80.0),
            0..16,
        ),
    ) {
        let mut scene = Scene::new();
        {
            let mut sb = SceneBuilder::new(&mut scene);
            for &(x, y, w, h) in &rects {
                sb.fill(
                    FillRule::NonZero,
                    Affine::IDENTITY,
                    &black(),
                    &rect_path(Rect::new(x, y, x + w, y + h)),
                )
                .expect("fill");
            }
            sb.finish().expect("balanced");
        }

        let mut resolver = Resolver::new();
        let kept: Vec<Rect> = resolver
            .resolve(&scene, &ResolveParams::new(target()))
            .draws()
            .iter()
            .map(|d| d.bounds)
            .collect();
        let expected: Vec<Rect> = rects
            .iter()
            .map(|&(x, y, w, h)| Rect::new(x, y, x + w, y + h))
            .filter(|r| r.intersects(target()))
            .collect();
        prop_assert_eq!(kept, expected);
    }
}
