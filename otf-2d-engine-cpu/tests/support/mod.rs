//! Shared scaffolding for the CPU backend's image tests.
//!
//! Compiled separately into each test binary, so anything one of them does not
//! use looks dead from that binary's point of view.
#![allow(dead_code)]

use otf_2d_engine_color::Color;
use otf_2d_engine_cpu::{CpuRenderer, PixelFormat, Pixmap, RenderParams};
use otf_2d_engine_geom::{Path, PathBuilder, Point, Rect};
use otf_2d_engine_scene::{Paint, Scene, SceneBuilder};
use otf_2d_engine_testing::image::Image;

/// The dark blue every case draws with unless it says otherwise.
pub fn ink() -> Paint {
    Paint::Solid(Color::from_srgb8(24, 42, 96, 255))
}

/// A second, clearly distinguishable colour.
pub fn accent() -> Paint {
    Paint::Solid(Color::from_srgb8(206, 68, 44, 255))
}

pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Path {
    PathBuilder::new().rect(Rect::new(x0, y0, x1, y1)).build()
}

pub fn polygon(points: &[(f64, f64)]) -> Path {
    let mut b = PathBuilder::new();
    for (index, &(x, y)) in points.iter().enumerate() {
        if index == 0 {
            b.move_to(Point::new(x, y));
        } else {
            b.line_to(Point::new(x, y));
        }
    }
    b.close();
    b.build()
}

/// A star, whose self-intersection is where the two fill rules disagree.
pub fn star(center: (f64, f64), outer: f64, points: usize) -> Path {
    let mut b = PathBuilder::new();
    let step = core::f64::consts::TAU / points as f64;
    for index in 0..points {
        // Stepping two vertices at a time is what makes the outline cross
        // itself rather than trace a convex polygon.
        let angle = (index * 2) as f64 * step - core::f64::consts::FRAC_PI_2;
        let p = Point::new(
            center.0 + outer * angle.cos(),
            center.1 + outer * angle.sin(),
        );
        if index == 0 {
            b.move_to(p);
        } else {
            b.line_to(p);
        }
    }
    b.close();
    b.build()
}

/// The background every case draws onto.
///
/// Opaque on purpose: the target is premultiplied and a golden PNG is
/// straight-alpha, and at alpha 1 the two coincide exactly. Comparing against
/// a reference that had to be unpremultiplied would be comparing against a
/// lossy transform of the render rather than the render itself.
pub fn background() -> Color {
    Color::from_srgb8(255, 255, 255, 255)
}

/// Builds a scene, renders it, and returns the pixels as a golden image.
pub fn render_case(
    width: u32,
    height: u32,
    build: impl FnOnce(&mut SceneBuilder<'_>),
) -> Result<Image, String> {
    render_case_with_tolerance(
        width,
        height,
        otf_2d_engine_raster::DEFAULT_TOLERANCE,
        build,
    )
}

/// The same, at a chosen flattening tolerance.
///
/// Curves reach the rasterizer as chords, and a chord of a convex arc lies
/// inside it — so a filled curve is always slightly *smaller* than the true
/// shape, by an amount the tolerance bounds. Being able to vary it is what
/// turns "the area is about right" into "the area converges as the tolerance
/// tightens", which is the claim worth testing.
pub fn render_case_with_tolerance(
    width: u32,
    height: u32,
    tolerance: f64,
    build: impl FnOnce(&mut SceneBuilder<'_>),
) -> Result<Image, String> {
    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        build(&mut sb);
        sb.finish().map_err(|e| format!("unbalanced scene: {e}"))?;
    }

    let mut pixmap = Pixmap::new(width, height, PixelFormat::Rgba8Premul);
    let mut renderer = CpuRenderer::new();
    let mut params = RenderParams::new(width, height);
    params.base_color = background();
    params.tolerance = tolerance;

    {
        let mut target = pixmap.as_target();
        renderer
            .render(&scene, &mut target, &params)
            .map_err(|e| format!("render failed: {e}"))?;
    }
    Image::from_rgba8(width, height, pixmap.into_data()).map_err(|e| format!("{e}"))
}
