//! Solid-fill benchmarks (T2.6).
//!
//! These measure the whole pipeline — resolve, flatten, bin, strip, fine —
//! because that is what a consumer pays for. Per-stage attribution comes from
//! `RenderStats` once stage timings land; until then the group's job is to
//! pin the end-to-end number so a regression cannot arrive unannounced.

use std::hint::black_box;

use criterion::Criterion;
use otf_2d_engine_color::Color;
use otf_2d_engine_cpu::{CpuRenderer, PixelFormat, Pixmap, RenderParams};
use otf_2d_engine_geom::{Affine, Path, PathBuilder, Point, Rect};
use otf_2d_engine_scene::{FillRule, Paint, Scene, SceneBuilder};

use crate::Registry;

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;

fn ink() -> Paint {
    Paint::Solid(Color::from_srgb8(24, 42, 96, 255))
}

/// A scene built once, outside the measured region: the benchmark is about
/// rendering, and encoding is measured by its own group when M3 adds one.
fn scene_of(paths: Vec<(Path, FillRule)>) -> Scene {
    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        for (path, rule) in &paths {
            sb.fill(*rule, Affine::IDENTITY, &ink(), path)
                .expect("fill");
        }
        sb.finish().expect("balanced");
    }
    scene
}

fn full_surface() -> Scene {
    scene_of(vec![(
        PathBuilder::new()
            .rect(Rect::new(0.5, 0.5, WIDTH as f64 - 0.5, HEIGHT as f64 - 0.5))
            .build(),
        FillRule::NonZero,
    )])
}

/// A UI-shaped frame: many small antialiased shapes rather than one large one,
/// so the perimeter-bound work dominates instead of the fill.
fn many_shapes() -> Scene {
    let mut paths = Vec::new();
    for row in 0..24 {
        for column in 0..40 {
            let x = 8.0 + column as f64 * 47.5;
            let y = 8.0 + row as f64 * 44.5;
            paths.push((
                PathBuilder::new()
                    .rounded_rect(
                        Rect::new(x, y, x + 40.25, y + 36.75),
                        otf_2d_engine_geom::RectRadii::uniform(6.0),
                    )
                    .build(),
                FillRule::NonZero,
            ));
        }
    }
    scene_of(paths)
}

fn circles() -> Scene {
    let mut paths = Vec::new();
    for index in 0..256 {
        let angle = index as f64 * 0.618 * core::f64::consts::TAU;
        let radius = 40.0 + (index % 7) as f64 * 12.0;
        let center = Point::new(
            WIDTH as f64 * 0.5 + angle.cos() * 700.0,
            HEIGHT as f64 * 0.5 + angle.sin() * 380.0,
        );
        paths.push((
            PathBuilder::new().circle(center, radius).build(),
            FillRule::NonZero,
        ));
    }
    scene_of(paths)
}

/// Registers the group.
pub fn register(criterion: &mut Criterion, registry: &mut Registry) {
    /// A named scene constructor.
    type Case = (&'static str, fn() -> Scene);

    let cases: [Case; 3] = [
        ("solid_rect_1080p", full_surface),
        ("rounded_rects_1080p", many_shapes),
        ("circles_1080p", circles),
    ];

    let mut group = criterion.benchmark_group("fill");
    for (name, build) in cases {
        let scene = build();
        let mut renderer = CpuRenderer::new();
        let mut pixmap = Pixmap::new(WIDTH, HEIGHT, PixelFormat::Rgba8Premul);
        let mut params = RenderParams::new(WIDTH, HEIGHT);
        params.base_color = Color::from_srgb8(255, 255, 255, 255);

        group.bench_function(name, |bencher| {
            bencher.iter(|| {
                let mut target = pixmap.as_target();
                let stats = renderer
                    .render(black_box(&scene), &mut target, &params)
                    .expect("render");
                black_box(stats.draws_resolved)
            });
        });
        registry.record("fill", name);
    }
    group.finish();
}
