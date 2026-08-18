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

/// Ribbons whose curvature varies along their length, which is where
/// flattening has something to decide.
///
/// Circles and rounded corners are constant-curvature: every flattener, however
/// naive, places the same number of chords along them. A curve that is nearly
/// straight in places and tight in others is where an Euler-spiral flattener
/// spends segments where they are needed and skips them where they are not
/// (T3.1), and it is the shape most real artwork and glyph outlines are made
/// of.
fn varied_curvature() -> Scene {
    let mut paths = Vec::new();
    for index in 0..48 {
        let phase = index as f64 * 0.37;
        let top = 20.0 + (index % 12) as f64 * 85.0;
        let left = 20.0 + (index / 12) as f64 * 470.0;
        let mut builder = PathBuilder::new();
        builder.move_to(Point::new(left, top + 30.0));
        for step in 0..6 {
            let x = left + step as f64 * 75.0;
            let sway = (phase + step as f64 * 1.1).sin() * 26.0;
            let pinch = (phase + step as f64 * 0.7).cos() * 22.0;
            builder.curve_to(
                Point::new(x + 18.0, top + 30.0 + sway * 2.4),
                Point::new(x + 52.0, top + 30.0 - pinch * 2.1),
                Point::new(x + 75.0, top + 30.0 + sway * 0.5),
            );
        }
        for step in (0..6).rev() {
            let x = left + step as f64 * 75.0;
            let sway = (phase + step as f64 * 1.1).sin() * 26.0;
            let pinch = (phase + step as f64 * 0.7).cos() * 22.0;
            builder.curve_to(
                Point::new(x + 52.0, top + 48.0 - pinch * 2.1),
                Point::new(x + 18.0, top + 48.0 + sway * 2.4),
                Point::new(x, top + 48.0 + sway * 0.5),
            );
        }
        builder.close();
        paths.push((builder.build(), FillRule::NonZero));
    }
    scene_of(paths)
}

/// Registers the group.
pub fn register(criterion: &mut Criterion, registry: &mut Registry) {
    /// A named scene constructor.
    type Case = (&'static str, fn() -> Scene);

    let cases: [Case; 4] = [
        ("solid_rect_1080p", full_surface),
        ("rounded_rects_1080p", many_shapes),
        ("circles_1080p", circles),
        ("varied_curvature_1080p", varied_curvature),
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
