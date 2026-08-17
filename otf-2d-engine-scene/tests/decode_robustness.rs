//! `Scene::from_bytes` must be total: any byte slice either decodes to a scene
//! with in-range handles, or returns an error. Never a panic, never an
//! out-of-memory reservation, never a scene that faults downstream.
//!
//! The module docs in `serialize.rs` make that claim; this is what backs it.

use otf_2d_engine_color::Color;
use otf_2d_engine_geom::{Affine, PathBuilder, Point, Rect, RectRadii, Vec2};
use otf_2d_engine_scene::{
    ColorStop, Extend, FillRule, Glyph, GlyphOptions, NO_REF, Paint, Scene, SceneUnit, StrokeStyle,
};
use proptest::prelude::*;

/// A scene touching every buffer, so mutations have somewhere interesting to
/// land.
fn sample_scene() -> Scene {
    let mut scene = Scene::with_unit(SceneUnit::Millimeter);
    let transform = scene.encode_transform(Affine::translate(Vec2::new(2.0, 3.0)));
    let stops = scene.encode_stops(&[
        ColorStop::new(0.0, Color::from_srgb8(255, 0, 0, 255)),
        ColorStop::new(1.0, Color::from_srgb8(0, 0, 255, 255)),
    ]);
    let gradient = scene.encode_paint(&Paint::LinearGradient {
        start: Point::new(0.0, 0.0),
        end: Point::new(32.0, 0.0),
        stops,
        extend: Extend::Pad,
    });
    let solid = scene.encode_paint(&Paint::hex(0x112233ff));
    let stroke = scene.encode_stroke(&StrokeStyle::new(3.0));

    let rect = scene.encode_path(
        &PathBuilder::new()
            .rect(Rect::new(0.0, 0.0, 10.0, 10.0))
            .build(),
    );
    let round = scene.encode_path(
        &PathBuilder::new()
            .rounded_rect(Rect::new(0.0, 0.0, 10.0, 10.0), RectRadii::uniform(2.0))
            .build(),
    );
    let layer = scene.encode_push_layer(
        otf_2d_engine_color::BlendMode::SrcOver,
        0.5,
        transform,
        rect,
    );
    scene.encode_fill(FillRule::NonZero, transform, solid, rect);
    scene.encode_fill(FillRule::EvenOdd, transform, gradient, round);
    scene.encode_stroke_draw(stroke, transform, solid, round);

    let variations = scene.encode_variations(&[400.0]);
    let run = scene.encode_glyph_run(
        otf_2d_engine_scene::FontRef::new(0),
        16.0,
        &[Glyph::new(1, 0.0, 0.0), Glyph::new(2, 8.0, 0.0)],
        &GlyphOptions {
            variations,
            ..GlyphOptions::default()
        },
    );
    scene.encode_glyphs(run, transform, solid);
    scene.encode_pop_layer(layer);
    scene
}

/// Everything a decoded scene promises stage 2, checked directly.
fn assert_handles_in_range(scene: &Scene) {
    let optional = |handle: u32, total: usize| handle == NO_REF || (handle as usize) < total;

    for path in scene.paths() {
        assert!(path.verb_offset as usize + path.verb_len as usize <= scene.path_verbs().len());
        assert!(path.point_offset as usize + path.point_len as usize <= scene.path_data().len());
        assert_eq!(path.point_len % 2, 0);
    }
    for paint in scene.paints() {
        assert!(paint.stops_offset as usize + paint.stops_len as usize <= scene.stops().len());
        assert!(optional(paint.transform, scene.transforms().len()));
    }
    for stroke in scene.strokes() {
        assert!(
            stroke.dash_offset_index as usize + stroke.dash_len as usize <= scene.dash_data().len()
        );
    }
    for run in scene.glyph_runs() {
        assert!(run.glyph_offset as usize + run.glyph_len as usize <= scene.glyphs().len());
        assert!(
            run.variations_offset as usize + run.variations_len as usize
                <= scene.variations().len()
        );
    }
    for layer in scene.layers() {
        assert!(optional(layer.clip_path, scene.paths().len()));
        assert!(optional(layer.push_tag, scene.tags().len()));
        assert!(optional(layer.pop_tag, scene.tags().len()));
    }
    for tag in scene.tags() {
        assert!(optional(tag.transform, scene.transforms().len()));
        assert!(optional(tag.paint, scene.paints().len()));
    }
    assert_eq!(scene.node_hashes().len(), scene.node_descs().len());
}

proptest! {
    /// Corrupt one byte anywhere in a valid scene. Whatever comes back must be
    /// an error or a scene that upholds every bound.
    #[test]
    fn a_single_corrupted_byte_never_panics(at: prop::sample::Index, value: u8) {
        let mut bytes = sample_scene().to_bytes();
        let index = at.index(bytes.len());
        bytes[index] = value;
        if let Ok(scene) = Scene::from_bytes(&bytes) {
            assert_handles_in_range(&scene);
        }
    }

    /// Truncation at any point is an error, never a partial scene.
    #[test]
    fn truncation_is_always_detected(at: prop::sample::Index) {
        let bytes = sample_scene().to_bytes();
        let cut = at.index(bytes.len());
        prop_assert!(Scene::from_bytes(&bytes[..cut]).is_err());
    }

    /// Arbitrary bytes behind valid magic must not get past an error. This is
    /// the shape a corrupted or hostile cache entry takes.
    #[test]
    fn arbitrary_bytes_behind_valid_magic_are_safe(
        tail in prop::collection::vec(any::<u8>(), 0..512),
    ) {
        let mut bytes = b"OTF2DSCN".to_vec();
        bytes.extend_from_slice(&tail);
        if let Ok(scene) = Scene::from_bytes(&bytes) {
            assert_handles_in_range(&scene);
        }
    }

    /// The round trip is the identity on content, whatever the unit.
    #[test]
    fn the_round_trip_preserves_content_and_unit(unit in 0u8..4) {
        let unit = SceneUnit::from_u8(unit).expect("0..4 are the four units");
        let mut scene = Scene::with_unit(unit);
        scene.encode_transform(Affine::translate(Vec2::new(1.0, 2.0)));
        scene.encode_paint(&Paint::hex(0x00ff00ff));

        let round = Scene::from_bytes(&scene.to_bytes()).expect("decode");
        prop_assert_eq!(round.content_hash(), scene.content_hash());
        prop_assert_eq!(round.unit(), unit);

        let source = sample_scene();
        let round = Scene::from_bytes(&source.to_bytes()).expect("decode");
        prop_assert_eq!(round.content_hash(), source.content_hash());
    }
}
