//! The golden-image corpus for solid fills (T2.6).
//!
//! Cases are registered explicitly — no macro registry, no link-time
//! collection — so the corpus is a list you can read. Each renders twice, with
//! caches bypassed and not, and the two must be byte-equal before the
//! reference is consulted at all (I-6).
//!
//! Every case draws onto an opaque white background. Nothing here is
//! translucent, so the premultiplied target and the straight-alpha reference
//! PNG hold the same bytes.

mod support;

use otf_2d_engine_geom::{Affine, PathBuilder, Point, Rect, RectRadii, Vec2};
use otf_2d_engine_scene::{FillRule, SceneBuilder};
use otf_2d_engine_testing::golden::{GoldenCase, GoldenSuite};
use otf_2d_engine_testing::image::Image;

use support::{accent, ink, polygon, rect, render_case, star};

const SIZE: u32 = 96;

/// Registers a case, generating the plain `fn` the harness needs.
macro_rules! golden {
    ($suite:expr; $( $name:ident ($w:expr, $h:expr) = $build:expr; )*) => {
        $(
            fn $name(_bypass_caches: bool) -> Result<Image, String> {
                render_case($w, $h, $build)
            }
            $suite.register(GoldenCase::new(stringify!($name), $name));
        )*
    };
}

fn suite() -> GoldenSuite {
    let mut suite = GoldenSuite::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden"));

    golden! { suite;
        // ---- Pixel-aligned geometry: no antialiasing anywhere ----
        aligned_rect (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(), &rect(16.0, 16.0, 80.0, 80.0));
        };
        full_surface_rect (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(), &rect(0.0, 0.0, 96.0, 96.0));
        };
        single_pixel (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(), &rect(48.0, 48.0, 49.0, 49.0));
        };

        // ---- Fractional edges: every side antialiased ----
        half_pixel_rect (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(), &rect(16.5, 16.5, 79.5, 79.5));
        };
        quarter_pixel_rect (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(), &rect(16.25, 16.75, 79.75, 79.25));
        };
        sub_pixel_sliver (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(), &rect(48.0, 8.0, 48.3, 88.0));
        };
        hairline_grid (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            for i in 0..8 {
                let at = 8.0 + i as f64 * 10.5;
                let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(), &rect(at, 6.0, at + 0.4, 90.0));
                let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &accent(), &rect(6.0, at, 90.0, at + 0.4));
            }
        };

        // ---- Diagonals and curves ----
        triangle (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(),
                &polygon(&[(12.0, 12.0), (84.0, 30.0), (40.0, 84.0)]));
        };
        forty_five_degree_diamond (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(),
                &polygon(&[(48.5, 8.5), (88.5, 48.5), (48.5, 88.5), (8.5, 48.5)]));
        };
        circle (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(),
                &PathBuilder::new().circle(Point::new(48.0, 48.0), 36.0).build());
        };
        ellipse (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(),
                &PathBuilder::new().ellipse(Point::new(48.0, 48.0), Vec2::new(40.0, 22.0)).build());
        };
        rounded_rect (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(),
                &PathBuilder::new()
                    .rounded_rect(Rect::new(10.5, 18.5, 85.5, 77.5), RectRadii::uniform(16.0))
                    .build());
        };
        rounded_rect_uneven_radii (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(),
                &PathBuilder::new()
                    .rounded_rect(Rect::new(8.0, 8.0, 88.0, 88.0), RectRadii::new(2.0, 20.0, 40.0, 8.0))
                    .build());
        };
        cubic_blob (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let mut b = PathBuilder::new();
            b.move_to(Point::new(16.0, 48.0));
            b.curve_to(Point::new(16.0, 4.0), Point::new(80.0, 4.0), Point::new(80.0, 48.0));
            b.curve_to(Point::new(80.0, 92.0), Point::new(16.0, 92.0), Point::new(16.0, 48.0));
            b.close();
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(), &b.build());
        };
        quadratic_leaf (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let mut b = PathBuilder::new();
            b.move_to(Point::new(14.0, 82.0));
            b.quad_to(Point::new(14.0, 14.0), Point::new(82.0, 14.0));
            b.quad_to(Point::new(82.0, 82.0), Point::new(14.0, 82.0));
            b.close();
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(), &b.build());
        };

        // ---- Fill rules ----
        star_non_zero (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(), &star((48.0, 48.0), 40.0, 5));
        };
        star_even_odd (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.fill(FillRule::EvenOdd, Affine::IDENTITY, &ink(), &star((48.0, 48.0), 40.0, 5));
        };
        overlapping_squares_non_zero (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let mut b = PathBuilder::new();
            b.rect(Rect::new(12.5, 12.5, 60.5, 60.5));
            b.rect(Rect::new(36.5, 36.5, 84.5, 84.5));
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(), &b.build());
        };
        overlapping_squares_even_odd (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let mut b = PathBuilder::new();
            b.rect(Rect::new(12.5, 12.5, 60.5, 60.5));
            b.rect(Rect::new(36.5, 36.5, 84.5, 84.5));
            let _ = sb.fill(FillRule::EvenOdd, Affine::IDENTITY, &ink(), &b.build());
        };
        annulus (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let mut b = PathBuilder::new();
            b.circle(Point::new(48.0, 48.0), 40.0);
            b.circle(Point::new(48.0, 48.0), 20.0);
            let _ = sb.fill(FillRule::EvenOdd, Affine::IDENTITY, &ink(), &b.build());
        };

        // ---- Transforms ----
        rotated_rect (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.fill(
                FillRule::NonZero,
                Affine::rotate_about(0.4, Point::new(48.0, 48.0)),
                &ink(),
                &rect(20.0, 32.0, 76.0, 64.0),
            );
        };
        scaled_and_translated (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let transform = Affine::scale(3.0).then(Affine::translate(Vec2::new(9.5, 13.5)));
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(), &rect(0.0, 0.0, 8.0, 8.0));
            let _ = sb.fill(FillRule::NonZero, transform, &accent(), &rect(2.0, 2.0, 22.0, 20.0));
        };
        sheared_triangle (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let shear = Affine::new([1.0, 0.0, 0.45, 1.0, -12.0, 0.0]);
            let _ = sb.fill(FillRule::NonZero, shear, &ink(),
                &polygon(&[(20.0, 16.0), (76.0, 16.0), (48.0, 80.0)]));
        };

        // ---- Clipping ----
        rect_clip (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.push_layer(
                otf_2d_engine_color::BlendMode::SrcOver,
                1.0,
                Affine::IDENTITY,
                Some(&rect(24.0, 24.0, 72.0, 72.0)),
            );
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(),
                &PathBuilder::new().circle(Point::new(48.0, 48.0), 44.0).build());
            let _ = sb.pop_layer();
        };
        fractional_rect_clip (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.push_layer(
                otf_2d_engine_color::BlendMode::SrcOver,
                1.0,
                Affine::IDENTITY,
                Some(&rect(20.25, 20.75, 75.75, 75.25)),
            );
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(), &rect(0.0, 0.0, 96.0, 96.0));
            let _ = sb.pop_layer();
        };
        nested_rect_clips (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.push_layer(
                otf_2d_engine_color::BlendMode::SrcOver,
                1.0,
                Affine::IDENTITY,
                Some(&rect(12.0, 12.0, 72.5, 84.0)),
            );
            let _ = sb.push_layer(
                otf_2d_engine_color::BlendMode::SrcOver,
                1.0,
                Affine::IDENTITY,
                Some(&rect(24.5, 6.0, 84.0, 60.0)),
            );
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &accent(), &rect(0.0, 0.0, 96.0, 96.0));
            let _ = sb.pop_layer();
            let _ = sb.pop_layer();
        };

        // ---- Edges of the surface ----
        shape_off_every_edge (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(), &rect(-20.5, -20.5, 60.5, 60.5));
        };
        shape_entirely_off_surface (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &ink(),
                &rect(200.0, 200.0, 260.0, 260.0));
        };
        many_small_shapes (SIZE, SIZE) = |sb: &mut SceneBuilder<'_>| {
            for row in 0..8 {
                for column in 0..8 {
                    let x = 4.0 + column as f64 * 11.3;
                    let y = 4.0 + row as f64 * 11.3;
                    let paint = if (row + column) % 2 == 0 { ink() } else { accent() };
                    let _ = sb.fill(FillRule::NonZero, Affine::IDENTITY, &paint,
                        &PathBuilder::new().circle(Point::new(x + 4.0, y + 4.0), 4.2).build());
                }
            }
        };
    }

    suite
}

#[test]
fn golden_corpus() {
    let suite = suite();
    assert!(
        suite.len() >= 20,
        "T2.6 requires at least 20 cases, the corpus has {}",
        suite.len()
    );
    suite.run_or_panic();
}
