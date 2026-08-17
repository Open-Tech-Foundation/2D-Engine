//! The T1.4 fuzz target: random sequences of builder calls.
//!
//! The property is I-8 stated operationally — whatever a consumer does, every
//! call returns `Ok` or a typed [`EncodeError`], nothing panics, and the scene
//! left behind is structurally valid: balanced layers, balanced nodes, every
//! handle in range.
//!
//! Proptest rather than `cargo-fuzz`: coverage-guided fuzzing needs a nightly
//! toolchain, and the input here is a *sequence of typed calls* rather than a
//! byte string, which is what proptest generates natively. `from_bytes` is
//! where byte-level fuzzing belongs, and `decode_robustness.rs` covers it.

use otf_2d_engine_color::{BlendMode, Color};
use otf_2d_engine_geom::{Affine, Path, PathBuilder, Point};
use otf_2d_engine_scene::{
    ColorStop, Dash, EncodeError, Extend, FillRule, FontRef, Glyph, GlyphOptions, ImageRef, Join,
    NO_REF, NodeId, Paint, Sampling, Scene, SceneBuilder, StopsRef, StrokeStyle, VariationsRef,
};
use proptest::prelude::*;

/// One builder call. Handles are generated as raw indices, including ones that
/// name nothing, because a consumer can hold a handle from a scene that has
/// since been reset.
#[derive(Debug, Clone)]
enum Op {
    Fill {
        even_odd: bool,
        transform: [f64; 6],
        paint: PaintSpec,
        path: PathSpec,
    },
    Stroke {
        style: StrokeSpec,
        transform: [f64; 6],
        paint: PaintSpec,
        path: PathSpec,
    },
    Glyphs {
        size: f32,
        glyphs: Vec<(u32, f32, f32)>,
        variations: u32,
    },
    Image {
        alpha: f32,
        transform: [f64; 6],
    },
    PushLayer {
        alpha: f32,
        transform: [f64; 6],
        clip: Option<PathSpec>,
    },
    PopLayer,
    InternStops(Vec<(f32, [u8; 4])>),
    InternVariations(Vec<f32>),
    Node(Vec<Op>),
}

#[derive(Debug, Clone)]
enum PaintSpec {
    Solid([u8; 4]),
    Linear {
        start: (f64, f64),
        end: (f64, f64),
        stops: u32,
    },
    Radial {
        center: (f64, f64),
        radius: f64,
        focal: Option<(f64, f64)>,
        stops: u32,
    },
    Image,
}

#[derive(Debug, Clone)]
struct StrokeSpec {
    width: f32,
    miter: Option<f32>,
    dash: Option<(Vec<f32>, f32)>,
}

#[derive(Debug, Clone)]
struct PathSpec {
    points: Vec<(f64, f64)>,
    close: bool,
}

fn build_path(spec: &PathSpec) -> Path {
    let mut b = PathBuilder::new();
    let mut iter = spec.points.iter();
    if let Some(&(x, y)) = iter.next() {
        b.move_to(Point::new(x, y));
    }
    for &(x, y) in iter {
        b.line_to(Point::new(x, y));
    }
    if spec.close {
        b.close();
    }
    b.build()
}

fn color(rgba: [u8; 4]) -> Color {
    Color::from_srgb8(rgba[0], rgba[1], rgba[2], rgba[3])
}

fn paint(spec: &PaintSpec) -> Paint {
    match spec {
        PaintSpec::Solid(rgba) => Paint::Solid(color(*rgba)),
        PaintSpec::Linear { start, end, stops } => Paint::LinearGradient {
            start: Point::new(start.0, start.1),
            end: Point::new(end.0, end.1),
            stops: StopsRef::new(*stops),
            extend: Extend::Pad,
        },
        PaintSpec::Radial {
            center,
            radius,
            focal,
            stops,
        } => Paint::RadialGradient {
            center: Point::new(center.0, center.1),
            radius: *radius,
            focal: focal.map(|(x, y)| Point::new(x, y)),
            stops: StopsRef::new(*stops),
            extend: Extend::Reflect,
        },
        PaintSpec::Image => Paint::Image {
            image: ImageRef::new(0),
            sampling: Sampling::Bilinear,
            transform: otf_2d_engine_scene::TransformRef::NONE,
        },
    }
}

fn stroke(spec: &StrokeSpec) -> StrokeStyle {
    let mut style = StrokeStyle::new(spec.width);
    style.join = match spec.miter {
        Some(limit) => Join::Miter { limit },
        None => Join::Round,
    };
    if let Some((pattern, offset)) = &spec.dash {
        style = style.with_dash(Dash::new(pattern, *offset));
    }
    style
}

/// Runs one op. Every outcome must be `Ok` or a typed error — the `Result` is
/// deliberately discarded after being observed, since which one it is depends
/// on the generated input.
fn run(sb: &mut SceneBuilder<'_>, op: &Op) {
    let outcome: Result<(), EncodeError> = match op {
        Op::Fill {
            even_odd,
            transform,
            paint: p,
            path,
        } => {
            let rule = if *even_odd {
                FillRule::EvenOdd
            } else {
                FillRule::NonZero
            };
            sb.fill(rule, Affine::new(*transform), &paint(p), &build_path(path))
        }
        Op::Stroke {
            style,
            transform,
            paint: p,
            path,
        } => sb.stroke(
            &stroke(style),
            Affine::new(*transform),
            &paint(p),
            &build_path(path),
        ),
        Op::Glyphs {
            size,
            glyphs,
            variations,
        } => {
            let glyphs: Vec<Glyph> = glyphs
                .iter()
                .map(|&(id, x, y)| Glyph::new(id, x, y))
                .collect();
            sb.draw_glyphs(
                FontRef::new(0),
                *size,
                Affine::IDENTITY,
                &Paint::hex(0xffffffff),
                &glyphs,
                GlyphOptions {
                    variations: VariationsRef::new(*variations),
                    ..GlyphOptions::default()
                },
            )
        }
        Op::Image { alpha, transform } => sb.draw_image(
            ImageRef::new(0),
            Affine::new(*transform),
            Sampling::Nearest,
            *alpha,
        ),
        Op::PushLayer {
            alpha,
            transform,
            clip,
        } => {
            let clip = clip.as_ref().map(build_path);
            sb.push_layer(
                BlendMode::SrcOver,
                *alpha,
                Affine::new(*transform),
                clip.as_ref(),
            )
        }
        Op::PopLayer => sb.pop_layer(),
        Op::InternStops(stops) => {
            let stops: Vec<ColorStop> = stops
                .iter()
                .map(|&(offset, rgba)| ColorStop::new(offset, color(rgba)))
                .collect();
            sb.intern_stops(&stops).map(|_| ())
        }
        Op::InternVariations(coords) => sb.intern_variations(coords).map(|_| ()),
        Op::Node(inner) => {
            let mut scope = sb.push_node(NodeId(inner.len() as u64));
            for op in inner {
                run(&mut scope, op);
            }
            Ok(())
        }
    };
    let _ = outcome;
}

/// `any::<f64>()` already yields NaN, both infinities and subnormals, so the
/// only thing to add is a bias towards coordinates that actually encode —
/// otherwise almost every generated draw is rejected and the interesting paths
/// through the encoder go untested.
fn arb_f64() -> impl Strategy<Value = f64> {
    prop_oneof![3 => -1e4f64..1e4f64, 1 => any::<f64>()]
}

fn arb_f32() -> impl Strategy<Value = f32> {
    prop_oneof![3 => -1e3f32..1e3f32, 1 => any::<f32>()]
}

fn arb_path() -> impl Strategy<Value = PathSpec> {
    (
        prop::collection::vec((arb_f64(), arb_f64()), 0..8),
        any::<bool>(),
    )
        .prop_map(|(points, close)| PathSpec { points, close })
}

fn arb_paint() -> impl Strategy<Value = PaintSpec> {
    prop_oneof![
        any::<[u8; 4]>().prop_map(PaintSpec::Solid),
        ((arb_f64(), arb_f64()), (arb_f64(), arb_f64()), 0u32..4)
            .prop_map(|(start, end, stops)| PaintSpec::Linear { start, end, stops }),
        (
            (arb_f64(), arb_f64()),
            arb_f64(),
            prop::option::of((arb_f64(), arb_f64())),
            0u32..4
        )
            .prop_map(|(center, radius, focal, stops)| PaintSpec::Radial {
                center,
                radius,
                focal,
                stops
            }),
        Just(PaintSpec::Image),
    ]
}

fn arb_stroke() -> impl Strategy<Value = StrokeSpec> {
    (
        arb_f32(),
        prop::option::of(arb_f32()),
        prop::option::of((prop::collection::vec(arb_f32(), 0..4), arb_f32())),
    )
        .prop_map(|(width, miter, dash)| StrokeSpec { width, miter, dash })
}

fn arb_op() -> impl Strategy<Value = Op> {
    let leaf = prop_oneof![
        (any::<bool>(), arb_affine(), arb_paint(), arb_path()).prop_map(
            |(even_odd, transform, paint, path)| Op::Fill {
                even_odd,
                transform,
                paint,
                path
            }
        ),
        (arb_stroke(), arb_affine(), arb_paint(), arb_path()).prop_map(
            |(style, transform, paint, path)| Op::Stroke {
                style,
                transform,
                paint,
                path
            }
        ),
        (
            arb_f32(),
            prop::collection::vec((any::<u32>(), arb_f32(), arb_f32()), 0..6),
            0u32..4
        )
            .prop_map(|(size, glyphs, variations)| Op::Glyphs {
                size,
                glyphs,
                variations
            }),
        (arb_f32(), arb_affine()).prop_map(|(alpha, transform)| Op::Image { alpha, transform }),
        (arb_f32(), arb_affine(), prop::option::of(arb_path())).prop_map(
            |(alpha, transform, clip)| Op::PushLayer {
                alpha,
                transform,
                clip
            }
        ),
        Just(Op::PopLayer),
        prop::collection::vec((arb_f32(), any::<[u8; 4]>()), 0..4).prop_map(Op::InternStops),
        prop::collection::vec(arb_f32(), 0..4).prop_map(Op::InternVariations),
    ];
    leaf.prop_recursive(2, 12, 4, |inner| {
        prop::collection::vec(inner, 0..4).prop_map(Op::Node)
    })
}

fn arb_affine() -> impl Strategy<Value = [f64; 6]> {
    prop::array::uniform6(arb_f64())
}

/// Everything a scene promises stage 2, checked structurally.
fn assert_sound(scene: &Scene) {
    // `from_bytes` runs the full handle validation, so a round trip is the
    // strongest structural check available without stage 2 existing yet.
    let decoded = Scene::from_bytes(&scene.to_bytes()).expect("a built scene must validate");
    assert_eq!(decoded.content_hash(), scene.content_hash());
    for layer in scene.layers() {
        assert_ne!(layer.pop_tag, NO_REF, "a layer was left open");
    }
    for node in scene.node_descs() {
        assert!(node.tag_offset as usize + node.tag_len as usize <= scene.tags().len());
        assert!(node.parent == NO_REF || (node.parent as usize) < scene.node_descs().len());
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 192, ..ProptestConfig::default() })]

    /// A random call sequence never panics and always leaves a sound scene.
    #[test]
    fn random_call_sequences_are_sound(ops in prop::collection::vec(arb_op(), 0..24)) {
        let mut scene = Scene::new();
        {
            let mut sb = SceneBuilder::new(&mut scene);
            for op in &ops {
                run(&mut sb, op);
            }
            let _ = sb.finish();
        }
        assert_sound(&scene);
    }

    /// The same, across frame boundaries: `reset` must not leave the builder
    /// holding indices into buffers that no longer have them.
    #[test]
    fn call_sequences_survive_reset(
        frames in prop::collection::vec(prop::collection::vec(arb_op(), 0..12), 1..4),
    ) {
        let mut scene = Scene::new();
        for ops in &frames {
            scene.reset();
            {
                let mut sb = SceneBuilder::new(&mut scene);
                for op in ops {
                    run(&mut sb, op);
                }
                let _ = sb.finish();
            }
            assert_sound(&scene);
        }
    }
}
