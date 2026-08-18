//! Stroke-expansion benchmarks (T3.2).
//!
//! Stroking is stage 3 doing several times the work a fill asks of it: two
//! parallel curves to fit and flatten instead of one centre line, a join at
//! every corner and a cap at every end. These pin what that costs, on the two
//! shapes it costs it differently for — corners, where the joins dominate, and
//! curves, where the parallel-curve fit does.

use std::hint::black_box;

use criterion::Criterion;
use otf_2d_engine_color::Color;
use otf_2d_engine_cpu::{CpuRenderer, PixelFormat, Pixmap, RenderParams};
use otf_2d_engine_geom::{Affine, Path, PathBuilder, Point};
use otf_2d_engine_scene::{Cap, Join, Paint, Scene, SceneBuilder, StrokeStyle};

use crate::Registry;

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;

fn ink() -> Paint {
    Paint::Solid(Color::from_srgb8(24, 42, 96, 255))
}

fn scene_of(paths: Vec<(Path, StrokeStyle)>) -> Scene {
    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        for (path, style) in &paths {
            sb.stroke(style, Affine::IDENTITY, &ink(), path)
                .expect("stroke");
        }
        sb.finish().expect("balanced");
    }
    scene
}

/// A chart's worth of polylines: every vertex is a join, and nothing curves.
fn polylines() -> Scene {
    let mut paths = Vec::new();
    for index in 0..64 {
        let phase = index as f64 * 0.41;
        let top = 12.0 + (index % 16) as f64 * 66.0;
        let left = 20.0 + (index / 16) as f64 * 470.0;
        let mut builder = PathBuilder::new();
        builder.move_to(Point::new(left, top));
        for step in 1..=14 {
            let x = left + step as f64 * 31.0;
            let y = top + (phase + step as f64 * 0.9).sin() * 24.0;
            builder.line_to(Point::new(x, y));
        }
        paths.push((
            builder.build(),
            StrokeStyle::new(5.0).with_join(Join::Miter { limit: 4.0 }),
        ));
    }
    scene_of(paths)
}

/// Rings: closed, curved, and every one of them two parallel curves.
fn rings() -> Scene {
    let mut paths = Vec::new();
    for index in 0..192 {
        let angle = index as f64 * 0.618 * core::f64::consts::TAU;
        let radius = 20.0 + (index % 5) as f64 * 14.0;
        let centre = Point::new(
            WIDTH as f64 * 0.5 + angle.cos() * 720.0,
            HEIGHT as f64 * 0.5 + angle.sin() * 400.0,
        );
        paths.push((
            PathBuilder::new().circle(centre, radius).build(),
            StrokeStyle::new(6.0),
        ));
    }
    scene_of(paths)
}

/// Curves with caps and round joins, which is what a drawing tool emits.
fn ribbons() -> Scene {
    let mut paths = Vec::new();
    for index in 0..48 {
        let phase = index as f64 * 0.37;
        let top = 24.0 + (index % 12) as f64 * 86.0;
        let left = 20.0 + (index / 12) as f64 * 470.0;
        let mut builder = PathBuilder::new();
        builder.move_to(Point::new(left, top));
        for step in 0..6 {
            let x = left + step as f64 * 74.0;
            let sway = (phase + step as f64 * 1.1).sin() * 26.0;
            let pinch = (phase + step as f64 * 0.7).cos() * 22.0;
            builder.curve_to(
                Point::new(x + 18.0, top + sway * 2.0),
                Point::new(x + 52.0, top - pinch * 1.8),
                Point::new(x + 74.0, top + sway * 0.5),
            );
        }
        paths.push((
            builder.build(),
            StrokeStyle::new(7.0)
                .with_join(Join::Round)
                .with_caps(Cap::Round),
        ));
    }
    scene_of(paths)
}

/// Registers the group.
pub fn register(criterion: &mut Criterion, registry: &mut Registry) {
    /// A named scene constructor.
    type Case = (&'static str, fn() -> Scene);

    let cases: [Case; 3] = [
        ("polylines_1080p", polylines),
        ("rings_1080p", rings),
        ("ribbons_1080p", ribbons),
    ];

    let mut group = criterion.benchmark_group("stroke");
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
        registry.record("stroke", name);
    }
    group.finish();
}
