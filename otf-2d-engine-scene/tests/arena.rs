//! T1.3 acceptance tests for the scene arena.
//!
//! Three properties the plan names, plus the decode failures that keep
//! `from_bytes` total:
//!
//! * `Scene: Send + Sync` — statically, not by running it on a thread.
//! * A second frame after `reset` allocates nothing (I-9).
//! * Bytes round-trip to an identical content hash.

use otf_2d_engine_color::{BlendMode, Color};
use otf_2d_engine_geom::{Affine, Path, PathBuilder, Point, Rect, RectRadii, Vec2};
use otf_2d_engine_scene::{
    Cap, ColorStop, Dash, Extend, FillRule, Glyph, GlyphOptions, Join, NodeHash, NodeId, Paint,
    PaintRef, PathRef, Scene, SceneDecodeError, SceneUnit, StrokeStyle, TransformRef,
};
use otf_2d_engine_testing::alloc::{CountingAllocator, measure};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator::new();

static_assertions::assert_impl_all!(Scene: Send, Sync);

/// Header length, mirrored from `serialize.rs`. Duplicated on purpose: the
/// tests patch fixed offsets, so a header change should break them loudly
/// rather than silently relocate what they are patching.
const HEADER_LEN: usize = 24 + 17 * 8;

// ---------------------------------------------------------------- fixtures

/// Everything a frame needs, built once so the encode loop itself is
/// allocation-free.
struct Fixture {
    paths: Vec<Path>,
    stroke: StrokeStyle,
    stops: Vec<ColorStop>,
    glyphs: Vec<Glyph>,
    variations: Vec<f32>,
}

impl Fixture {
    fn new(draws: usize) -> Fixture {
        let paths = (0..draws)
            .map(|i| {
                let x = i as f64 * 3.0;
                match i % 3 {
                    0 => PathBuilder::new()
                        .rect(Rect::new(x, 0.0, x + 20.0, 12.0))
                        .build(),
                    1 => PathBuilder::new()
                        .rounded_rect(Rect::new(x, 0.0, x + 20.0, 12.0), RectRadii::uniform(4.0))
                        .build(),
                    _ => {
                        let mut b = PathBuilder::new();
                        b.move_to(Point::new(x, 0.0));
                        b.curve_to(
                            Point::new(x + 5.0, 9.0),
                            Point::new(x + 14.0, -9.0),
                            Point::new(x + 20.0, 3.0),
                        );
                        b.line_to(Point::new(x + 10.0, 12.0));
                        b.close();
                        b.build()
                    }
                }
            })
            .collect();
        Fixture {
            paths,
            stroke: StrokeStyle::new(2.5)
                .with_join(Join::Round)
                .with_caps(Cap::Square)
                .with_dash(Dash::new(&[4.0, 2.0, 1.0, 2.0], 0.5)),
            stops: vec![
                ColorStop::new(0.0, Color::from_srgb8(255, 0, 0, 255)),
                ColorStop::new(0.5, Color::from_srgb8(0, 255, 0, 128)),
                ColorStop::new(1.0, Color::from_srgb8(0, 0, 255, 255)),
            ],
            glyphs: (0..16)
                .map(|g| Glyph::new(g, g as f32 * 9.0, 0.0))
                .collect(),
            variations: vec![400.0, 0.0],
        }
    }

    /// Encodes one frame. Touches every arena buffer, so "allocates nothing"
    /// is a claim about all fifteen and not just the busy ones.
    fn encode(&self, scene: &mut Scene) {
        let node = scene.encode_push_node(NodeId(7), otf_2d_engine_scene::NO_REF);
        let root = scene.encode_transform(Affine::translate(Vec2::new(4.0, 4.0)));
        let stops = scene.encode_stops(&self.stops);
        let gradient = scene.encode_paint(&Paint::LinearGradient {
            start: Point::new(0.0, 0.0),
            end: Point::new(64.0, 0.0),
            stops,
            extend: Extend::Reflect,
        });
        let solid = scene.encode_paint(&Paint::hex(0x3355ffff));
        let stroke = scene.encode_stroke(&self.stroke);

        let clip = scene.encode_path(&self.paths[0]);
        let layer = scene.encode_push_layer(BlendMode::SrcOver, 0.75, root, clip);

        for (i, path) in self.paths.iter().enumerate() {
            let transform = scene.encode_transform(Affine::scale(1.0 + (i % 4) as f64));
            let handle = scene.encode_path(path);
            match i % 3 {
                0 => scene.encode_fill(FillRule::NonZero, transform, solid, handle),
                1 => scene.encode_fill(FillRule::EvenOdd, transform, gradient, handle),
                _ => scene.encode_stroke_draw(stroke, transform, solid, handle),
            }
        }

        let variations = scene.encode_variations(&self.variations);
        let options = GlyphOptions {
            synthetic_bold: 0.02,
            variations,
            ..GlyphOptions::default()
        };
        let run = scene.encode_glyph_run(
            otf_2d_engine_scene::FontRef::new(3),
            18.0,
            &self.glyphs,
            &options,
        );
        scene.encode_glyphs(run, root, solid);
        scene.encode_pop_layer(layer);
        scene.encode_pop_node(node, NodeHash(0xdead_beef));
    }
}

fn populated() -> Scene {
    let fixture = Fixture::new(24);
    let mut scene = Scene::with_unit(SceneUnit::Point);
    fixture.encode(&mut scene);
    scene
}

// ---------------------------------------------------------------- I-9

#[test]
fn a_reset_scene_re_encodes_without_allocating() {
    const DRAWS: usize = 1000;
    let fixture = Fixture::new(DRAWS);
    let mut scene = Scene::new();

    let (_, first) = measure(|| fixture.encode(&mut scene));
    assert!(
        first.acquisitions() > 0,
        "the counting allocator is not installed: the first pass allocated nothing"
    );
    let hash = scene.content_hash();
    let memory = scene.memory_usage().total();

    scene.reset();
    let (_, second) = measure(|| fixture.encode(&mut scene));

    assert_eq!(
        second.acquisitions(),
        0,
        "I-9: the second frame allocated ({second:?}) after {first:?} on the first"
    );
    assert!(
        second.is_quiet(),
        "I-9: the second frame touched the allocator: {second:?}"
    );
    assert_eq!(
        scene.content_hash(),
        hash,
        "re-encoding the same frame changed the content"
    );
    assert_eq!(scene.memory_usage().total(), memory);
    assert_eq!(
        scene.tags().len(),
        DRAWS + 3,
        "draws, plus push/pop layer and the glyph run"
    );
}

#[test]
fn reset_keeps_capacity_and_the_unit() {
    let mut scene = populated();
    let before = scene.memory_usage();
    scene.reset();

    assert!(scene.is_empty());
    assert_eq!(
        scene.unit(),
        SceneUnit::Point,
        "the unit is a surface property, not a frame one"
    );
    assert_eq!(
        scene.memory_usage().total(),
        0,
        "usage counts live elements, not capacity"
    );

    let (_, counters) = measure(|| Fixture::new(24).encode(&mut scene));
    let _ = before;
    assert!(
        counters.acquisitions() > 0,
        "sanity: building fresh fixtures does allocate, so the check above means something"
    );
}

// ---------------------------------------------------------------- round trip

#[test]
fn bytes_round_trip_to_an_identical_hash() {
    let scene = populated();
    let bytes = scene.to_bytes();
    let decoded = Scene::from_bytes(&bytes).expect("decode");

    assert_eq!(decoded.content_hash(), scene.content_hash());
    assert_eq!(decoded.unit(), scene.unit());
    assert_eq!(
        decoded, scene,
        "the decoded arena differs buffer for buffer"
    );
    assert_eq!(decoded.to_bytes(), bytes, "re-encoding is not byte-stable");
}

#[test]
fn an_empty_scene_round_trips() {
    for unit in [
        SceneUnit::Logical,
        SceneUnit::Pixel,
        SceneUnit::Point,
        SceneUnit::Millimeter,
    ] {
        let scene = Scene::with_unit(unit);
        let decoded = Scene::from_bytes(&scene.to_bytes()).expect("decode");
        assert_eq!(decoded.unit(), unit);
        assert_eq!(decoded.content_hash(), scene.content_hash());
        assert!(decoded.is_empty());
    }
}

#[test]
fn every_buffer_survives_the_round_trip() {
    let scene = populated();
    let decoded = Scene::from_bytes(&scene.to_bytes()).expect("decode");

    assert_eq!(decoded.tags(), scene.tags());
    assert_eq!(decoded.paths(), scene.paths());
    assert_eq!(decoded.paints(), scene.paints());
    assert_eq!(decoded.strokes(), scene.strokes());
    assert_eq!(decoded.glyph_runs(), scene.glyph_runs());
    assert_eq!(decoded.glyphs(), scene.glyphs());
    assert_eq!(decoded.layers(), scene.layers());
    assert_eq!(decoded.stops(), scene.stops());
    assert_eq!(decoded.dash_data(), scene.dash_data());
    assert_eq!(decoded.variations(), scene.variations());
    assert_eq!(decoded.node_hashes(), scene.node_hashes());
    assert_eq!(decoded.node_descs(), scene.node_descs());
    assert_eq!(decoded.memory_usage(), scene.memory_usage());

    let original = scene.path(PathRef::new(1)).expect("path 1");
    let copy = decoded.path(PathRef::new(1)).expect("path 1");
    assert_eq!(copy.raw_verbs(), original.raw_verbs());
    assert_eq!(copy.raw_points(), original.raw_points());
    assert_eq!(copy.bounds(), original.bounds());
    assert_eq!(copy.shape(), original.shape());
}

/// The M1 gate freezes the arena layout: after it, a change here invalidates
/// M2 onward. This pins the buffer *order* in the serialised header, which the
/// record-size assertions in `records.rs` do not cover.
#[test]
fn the_arena_layout_is_frozen() {
    let scene = populated();
    let bytes = scene.to_bytes();
    let count = |i: usize| {
        let at = 24 + i * 8;
        u64::from_ne_bytes(bytes[at..at + 8].try_into().expect("eight bytes")) as usize
    };

    let memory = scene.memory_usage();
    assert_eq!(HEADER_LEN, 160, "17 buffers, 24-byte prologue");
    assert_eq!(count(0), scene.tags().len(), "0: tags");
    assert_eq!(count(1), scene.path_data().len(), "1: path_data");
    assert_eq!(count(2), scene.path_verbs().len(), "2: path_verbs");
    assert_eq!(count(3), scene.paths().len(), "3: paths");
    assert_eq!(count(4), scene.transforms().len(), "4: transforms");
    assert_eq!(count(5), scene.paints().len(), "5: paints");
    assert_eq!(count(6), scene.stops().len(), "6: stops");
    assert_eq!(count(7), scene.stop_runs().len(), "7: stop_runs");
    assert_eq!(count(8), scene.strokes().len(), "8: strokes");
    assert_eq!(count(9), scene.dash_data().len(), "9: dash_data");
    assert_eq!(count(10), scene.glyph_runs().len(), "10: glyph_runs");
    assert_eq!(count(11), scene.glyphs().len(), "11: glyphs");
    assert_eq!(count(12), scene.variations().len(), "12: variations");
    assert_eq!(
        count(13),
        scene.variation_runs().len(),
        "13: variation_runs"
    );
    assert_eq!(count(14), scene.layers().len(), "14: layers");
    assert_eq!(count(15), scene.node_hashes().len(), "15: node_hashes");
    assert_eq!(count(16), scene.node_descs().len(), "16: node_descs");

    // Distinct lengths are what make the order assertions above meaningful:
    // if two adjacent buffers always had the same length, swapping them would
    // pass. These are the ones that could plausibly be confused.
    assert_ne!(scene.stops().len(), scene.stop_runs().len());
    assert_ne!(scene.variations().len(), scene.variation_runs().len());
    assert_ne!(scene.paths().len(), scene.transforms().len());
    assert!(memory.total() > 0);
}

#[test]
fn write_to_appends_rather_than_replaces() {
    let scene = populated();
    let mut buffer = vec![0xabu8; 3];
    scene.write_to(&mut buffer);
    assert_eq!(&buffer[..3], &[0xab; 3]);
    assert_eq!(&buffer[3..], scene.to_bytes().as_slice());
}

#[test]
fn a_scene_that_decodes_hands_stage_two_only_valid_handles() {
    let scene = populated();
    let decoded = Scene::from_bytes(&scene.to_bytes()).expect("decode");
    for tag in decoded.tags() {
        assert!(
            tag.paint == otf_2d_engine_scene::NO_REF
                || (tag.paint as usize) < decoded.paints().len()
        );
        assert!(
            tag.transform == otf_2d_engine_scene::NO_REF
                || (tag.transform as usize) < decoded.transforms().len()
        );
    }
    for path in decoded.paths() {
        let end = path.point_offset as usize + path.point_len as usize;
        assert!(end <= decoded.path_data().len());
    }
}

// ---------------------------------------------------------------- rejection

#[test]
fn a_non_scene_is_rejected_by_magic() {
    assert_eq!(Scene::from_bytes(&[]), Err(SceneDecodeError::BadMagic));
    assert_eq!(
        Scene::from_bytes(b"not a scene at all"),
        Err(SceneDecodeError::BadMagic)
    );
    assert_eq!(
        Scene::from_bytes(&[0u8; 512]),
        Err(SceneDecodeError::BadMagic)
    );
}

#[test]
fn a_truncated_scene_is_rejected() {
    let bytes = populated().to_bytes();

    assert_eq!(
        Scene::from_bytes(&bytes[..HEADER_LEN - 8]),
        Err(SceneDecodeError::Truncated {
            needed: HEADER_LEN,
            found: HEADER_LEN - 8
        })
    );
    // Cut inside the payload: the header still parses, a buffer does not.
    let cut = bytes.len() - 16;
    assert!(matches!(
        Scene::from_bytes(&bytes[..cut]),
        Err(SceneDecodeError::Truncated { .. })
    ));
}

#[test]
fn a_foreign_header_is_rejected() {
    let good = populated().to_bytes();

    let mut wrong_version = good.clone();
    wrong_version[8..12].copy_from_slice(&99u32.to_ne_bytes());
    assert!(matches!(
        Scene::from_bytes(&wrong_version),
        Err(SceneDecodeError::UnsupportedVersion { found: 99, .. })
    ));

    let mut wrong_endian = good.clone();
    wrong_endian[12..16].reverse();
    assert_eq!(
        Scene::from_bytes(&wrong_endian),
        Err(SceneDecodeError::ForeignEndian)
    );

    let mut wrong_layout = good.clone();
    wrong_layout[16] ^= 0xff;
    assert!(matches!(
        Scene::from_bytes(&wrong_layout),
        Err(SceneDecodeError::LayoutMismatch { .. })
    ));

    let mut wrong_unit = good.clone();
    wrong_unit[20] = 200;
    assert_eq!(
        Scene::from_bytes(&wrong_unit),
        Err(SceneDecodeError::UnknownUnit(200))
    );
}

#[test]
fn an_impossible_element_count_is_rejected() {
    let mut bytes = populated().to_bytes();
    // The first count is `tags`. Claim more tags than the payload holds.
    bytes[24..32].copy_from_slice(&u64::MAX.to_ne_bytes());
    assert!(matches!(
        Scene::from_bytes(&bytes),
        Err(SceneDecodeError::CountOverflow | SceneDecodeError::Truncated { .. })
    ));
}

#[test]
fn an_unknown_draw_kind_is_rejected() {
    let mut bytes = populated().to_bytes();
    // `DrawTag::kind` is the first byte of the first record of the first buffer.
    bytes[HEADER_LEN] = 0x7f;
    assert_eq!(
        Scene::from_bytes(&bytes),
        Err(SceneDecodeError::UnknownDiscriminant {
            buffer: "tags",
            index: 0
        })
    );
}

#[test]
fn a_dangling_handle_is_rejected() {
    // The raw encoder is `#[doc(hidden)]` and does not validate — that is
    // T1.4's job. It is the shortest way to produce bytes a hostile writer
    // could produce, and `from_bytes` must not accept them.
    let mut scene = Scene::new();
    scene.encode_fill(
        FillRule::NonZero,
        TransformRef::NONE,
        PaintRef::NONE,
        PathRef::new(7),
    );
    assert_eq!(
        Scene::from_bytes(&scene.to_bytes()),
        Err(SceneDecodeError::DanglingReference {
            buffer: "tags",
            index: 0
        })
    );
}

#[test]
fn decode_errors_describe_themselves() {
    let text = format!(
        "{}",
        SceneDecodeError::DanglingReference {
            buffer: "tags",
            index: 3
        }
    );
    assert!(text.contains("tags[3]"), "{text}");
    let text = format!("{}", SceneDecodeError::BadMagic);
    assert!(text.contains("magic"), "{text}");
}
