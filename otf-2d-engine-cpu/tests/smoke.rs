//! A first end-to-end render: scene in, pixels out.

use otf_2d_engine_color::Color;
use otf_2d_engine_cpu::{CpuRenderer, PixelFormat, Pixmap, RenderParams};
use otf_2d_engine_geom::{Affine, PathBuilder, Rect};
use otf_2d_engine_scene::{FillRule, Paint, Scene, SceneBuilder};

#[test]
fn a_solid_rect_reaches_the_pixmap() {
    let mut scene = Scene::new();
    {
        let mut sb = SceneBuilder::new(&mut scene);
        sb.fill(
            FillRule::NonZero,
            Affine::IDENTITY,
            &Paint::Solid(Color::from_srgb8(255, 0, 0, 255)),
            &PathBuilder::new()
                .rect(Rect::new(4.0, 4.0, 12.0, 12.0))
                .build(),
        )
        .expect("fill");
        sb.finish().expect("balanced");
    }

    let mut pixmap = Pixmap::new(16, 16, PixelFormat::Rgba8Premul);
    let mut renderer = CpuRenderer::new();
    let mut params = RenderParams::new(16, 16);
    params.base_color = Color::from_srgb8(255, 255, 255, 255);

    let stats = {
        let mut target = pixmap.as_target();
        renderer
            .render(&scene, &mut target, &params)
            .expect("render")
    };

    assert_eq!(stats.draws_resolved, 1);
    assert_eq!(
        pixmap.pixel(8, 8),
        Some([255, 0, 0, 255]),
        "inside the rect"
    );
    assert_eq!(pixmap.pixel(1, 1), Some([255, 255, 255, 255]), "outside it");
    assert_eq!(
        pixmap.pixel(3, 8),
        Some([255, 255, 255, 255]),
        "just left of it"
    );
    assert_eq!(
        pixmap.pixel(4, 8),
        Some([255, 0, 0, 255]),
        "the first covered column"
    );
}
