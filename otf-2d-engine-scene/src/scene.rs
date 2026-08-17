//! The scene arena.
//!
//! A `Scene` is not a tree of objects. It is a set of parallel buffers with
//! `u32` handles between them (Doc 02 §2). That is what makes it `Send + Sync`
//! without a lock, hashable in one pass, serialisable without pointer fixups,
//! and cheap to reuse across frames.
//!
//! # Invariants this file exists to hold
//!
//! * **I-1** — no interior mutability. Encoding takes `&mut Scene`; once
//!   encoding is over the scene is read-only data.
//! * **I-2** — no pointers. Every cross-reference is an index.
//! * **I-9** — `reset` clears without deallocating, so a steady-state frame
//!   allocates nothing.

use alloc::vec::Vec;
use core::hash::Hasher;

use otf_2d_engine_color::{BlendMode, Color, ColorSpace};
use otf_2d_engine_geom::{Affine, Path, PathShape, PathVerb, Point, Rect};
use rustc_hash::FxHasher;

use crate::handles::{
    FontRef, GlyphRunRef, ImageRef, NO_REF, NodeHash, NodeId, PaintRef, PathRef, StopsRef,
    StrokeRef, TransformRef, VariationsRef,
};
use crate::records::{
    ColorStopRec, DrawKind, DrawTag, FLAG_EVEN_ODD, GlyphRec, GlyphRunDesc, JOIN_BEVEL, JOIN_MITER,
    JOIN_ROUND, LayerDesc, NodeDesc, PAINT_FLAG_HAS_FOCAL, PaintDesc, PaintKind, PathDesc,
    ShapeKind, StrokeDesc, TransformRec,
};
use crate::style::{ColorStop, FillRule, Glyph, GlyphOptions, Join, Paint, Sampling, StrokeStyle};
use crate::unit::SceneUnit;

/// How far back interning looks for an identical entry.
///
/// A full hash map would dedupe more, but it would allocate every frame and
/// break I-9. Consumers overwhelmingly repeat the *most recent* transform and
/// paint — a UI draws a run of shapes under one transform — so a short linear
/// scan catches nearly all of it for no allocation and no state.
const INTERN_WINDOW: usize = 8;

/// An immutable, flat, `Send + Sync` scene description.
///
/// Build one with `SceneBuilder` (T1.4). Reuse it across frames with
/// [`Scene::reset`], which keeps every allocation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    /// One tag per draw command, in submission order.
    pub(crate) tags: Vec<DrawTag>,
    /// Path point coordinates, densely packed as `x, y` pairs.
    ///
    /// `f64` per D-21: this is scene space, whose magnitude is unbounded.
    pub(crate) path_data: Vec<f64>,
    /// Path verbs, densely packed. Parallel to runs of `path_data`.
    pub(crate) path_verbs: Vec<u8>,
    /// Path descriptors: offsets and lengths into the two buffers above.
    pub(crate) paths: Vec<PathDesc>,
    /// Affine transforms, deduplicated within a short window.
    pub(crate) transforms: Vec<TransformRec>,
    /// Paints: solid, gradient, image.
    pub(crate) paints: Vec<PaintDesc>,
    /// Gradient stop runs, densely packed.
    pub(crate) stops: Vec<ColorStopRec>,
    /// Stroke styles.
    pub(crate) strokes: Vec<StrokeDesc>,
    /// Dash patterns, densely packed.
    pub(crate) dash_data: Vec<f32>,
    /// Glyph runs.
    pub(crate) glyph_runs: Vec<GlyphRunDesc>,
    /// Glyphs, densely packed.
    pub(crate) glyphs: Vec<GlyphRec>,
    /// Variable-font axis coordinates, densely packed.
    pub(crate) variations: Vec<f32>,
    /// Layer push/pop records.
    pub(crate) layers: Vec<LayerDesc>,
    /// Content hashes for structural sharing (Doc 03 §3). Reserved until M6.
    pub(crate) node_hashes: Vec<u64>,
    /// Node identities and extents, parallel to `node_hashes`. Reserved.
    pub(crate) node_descs: Vec<NodeDesc>,
    /// The physical meaning of a coordinate value of 1.0.
    pub(crate) unit: SceneUnit,
}

/// Arena usage, for consumer budgeting (Doc 02 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SceneMemory {
    pub tags: usize,
    pub path_data: usize,
    pub path_verbs: usize,
    pub paths: usize,
    pub transforms: usize,
    pub paints: usize,
    pub stops: usize,
    pub strokes: usize,
    pub dash_data: usize,
    pub glyph_runs: usize,
    pub glyphs: usize,
    pub variations: usize,
    pub layers: usize,
    pub nodes: usize,
}

impl SceneMemory {
    /// Total bytes across every buffer.
    pub fn total(&self) -> usize {
        self.tags
            + self.path_data
            + self.path_verbs
            + self.paths
            + self.transforms
            + self.paints
            + self.stops
            + self.strokes
            + self.dash_data
            + self.glyph_runs
            + self.glyphs
            + self.variations
            + self.layers
            + self.nodes
    }
}

// ---------------------------------------------------------------- construction

impl Scene {
    /// A scene in [`SceneUnit::Logical`] units.
    ///
    /// Raster-only: a vector backend rejects a logical scene, because it
    /// cannot know whether 1.0 is a pixel, a point or a millimetre, and
    /// guessing produces a plausible document at the wrong physical size
    /// (D-19).
    pub fn new() -> Self {
        Self::with_unit(SceneUnit::Logical)
    }

    /// A scene whose coordinates carry the given physical meaning.
    ///
    /// The unit is fixed at construction. It cannot be changed afterwards,
    /// and [`Scene::reset`] preserves it — a surface does not change what its
    /// coordinates mean between frames.
    pub fn with_unit(unit: SceneUnit) -> Self {
        Self {
            unit,
            ..Self::default()
        }
    }

    /// The physical meaning of one coordinate unit.
    #[inline]
    pub fn unit(&self) -> SceneUnit {
        self.unit
    }

    /// Clears the contents, retaining every allocation.
    ///
    /// This is the steady-state frame boundary: one long-lived `Scene` per
    /// surface, `reset` per frame, so frame N+1 reuses frame N's memory and
    /// allocates nothing (I-9).
    pub fn reset(&mut self) {
        self.tags.clear();
        self.path_data.clear();
        self.path_verbs.clear();
        self.paths.clear();
        self.transforms.clear();
        self.paints.clear();
        self.stops.clear();
        self.strokes.clear();
        self.dash_data.clear();
        self.glyph_runs.clear();
        self.glyphs.clear();
        self.variations.clear();
        self.layers.clear();
        self.node_hashes.clear();
        self.node_descs.clear();
        // `unit` deliberately survives: it is a property of the surface, not
        // of the frame.
    }

    /// True when nothing has been encoded.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }

    /// Current arena usage in bytes, per buffer.
    pub fn memory_usage(&self) -> SceneMemory {
        fn bytes<T>(v: &[T]) -> usize {
            core::mem::size_of_val(v)
        }
        SceneMemory {
            tags: bytes(&self.tags),
            path_data: bytes(&self.path_data),
            path_verbs: bytes(&self.path_verbs),
            paths: bytes(&self.paths),
            transforms: bytes(&self.transforms),
            paints: bytes(&self.paints),
            stops: bytes(&self.stops),
            strokes: bytes(&self.strokes),
            dash_data: bytes(&self.dash_data),
            glyph_runs: bytes(&self.glyph_runs),
            glyphs: bytes(&self.glyphs),
            variations: bytes(&self.variations),
            layers: bytes(&self.layers),
            nodes: bytes(&self.node_hashes) + bytes(&self.node_descs),
        }
    }

    /// Reserves room for `draws` draw commands, so the first frame does not
    /// grow its buffers repeatedly.
    pub fn reserve(&mut self, draws: usize) {
        self.tags.reserve(draws);
        self.paths.reserve(draws);
        self.transforms.reserve(draws);
        self.paints.reserve(draws);
    }
}

// ---------------------------------------------------------------- reading

impl Scene {
    #[inline]
    pub fn tags(&self) -> &[DrawTag] {
        &self.tags
    }

    #[inline]
    pub fn paths(&self) -> &[PathDesc] {
        &self.paths
    }

    /// Every affine in the arena, in encode order. Stage 2 walks this once to
    /// resolve draws to device space.
    #[inline]
    pub fn transforms(&self) -> &[TransformRec] {
        &self.transforms
    }

    /// Every path coordinate, as `x, y` pairs. Indexed by
    /// [`PathDesc::point_offset`].
    #[inline]
    pub fn path_data(&self) -> &[f64] {
        &self.path_data
    }

    /// Every path verb. Indexed by [`PathDesc::verb_offset`].
    #[inline]
    pub fn path_verbs(&self) -> &[u8] {
        &self.path_verbs
    }

    #[inline]
    pub fn paints(&self) -> &[PaintDesc] {
        &self.paints
    }

    #[inline]
    pub fn strokes(&self) -> &[StrokeDesc] {
        &self.strokes
    }

    #[inline]
    pub fn glyph_runs(&self) -> &[GlyphRunDesc] {
        &self.glyph_runs
    }

    #[inline]
    pub fn layers(&self) -> &[LayerDesc] {
        &self.layers
    }

    #[inline]
    pub fn stops(&self) -> &[ColorStopRec] {
        &self.stops
    }

    #[inline]
    pub fn dash_data(&self) -> &[f32] {
        &self.dash_data
    }

    #[inline]
    pub fn variations(&self) -> &[f32] {
        &self.variations
    }

    #[inline]
    pub fn glyphs(&self) -> &[GlyphRec] {
        &self.glyphs
    }

    /// Node content hashes, as raw `u64`. Reserved for the M6 node cache
    /// (Doc 03 §3).
    #[inline]
    pub fn node_hashes(&self) -> &[u64] {
        &self.node_hashes
    }

    /// The content hash of one node.
    #[inline]
    pub fn node_hash(&self, node: u32) -> Option<NodeHash> {
        self.node_hashes.get(node as usize).copied().map(NodeHash)
    }

    /// Node identities and extents. Reserved for the M6 node cache.
    #[inline]
    pub fn node_descs(&self) -> &[NodeDesc] {
        &self.node_descs
    }

    /// The affine behind a handle, or the identity for [`TransformRef::NONE`].
    pub fn transform(&self, handle: TransformRef) -> Affine {
        match handle.get().and_then(|i| self.transforms.get(i)) {
            Some(rec) => Affine::new(rec.0),
            None => Affine::IDENTITY,
        }
    }

    /// A path's descriptor and geometry.
    pub fn path(&self, handle: PathRef) -> Option<PathView<'_>> {
        let desc = *self.paths.get(handle.get()?)?;
        let verbs = self
            .path_verbs
            .get(desc.verb_offset as usize..(desc.verb_offset + desc.verb_len) as usize)?;
        let data = self
            .path_data
            .get(desc.point_offset as usize..(desc.point_offset + desc.point_len) as usize)?;
        Some(PathView { desc, verbs, data })
    }

    /// The stops backing a gradient paint.
    pub fn paint_stops(&self, desc: &PaintDesc) -> &[ColorStopRec] {
        let start = desc.stops_offset as usize;
        let end = start.saturating_add(desc.stops_len as usize);
        self.stops.get(start..end).unwrap_or(&[])
    }

    /// The glyphs of a run.
    pub fn run_glyphs(&self, desc: &GlyphRunDesc) -> &[GlyphRec] {
        let start = desc.glyph_offset as usize;
        let end = start.saturating_add(desc.glyph_len as usize);
        self.glyphs.get(start..end).unwrap_or(&[])
    }

    /// The variation coordinates of a run.
    pub fn run_variations(&self, desc: &GlyphRunDesc) -> &[f32] {
        let start = desc.variations_offset as usize;
        let end = start.saturating_add(desc.variations_len as usize);
        self.variations.get(start..end).unwrap_or(&[])
    }

    /// The dash pattern of a stroke.
    pub fn stroke_dash(&self, desc: &StrokeDesc) -> &[f32] {
        let start = desc.dash_offset_index as usize;
        let end = start.saturating_add(desc.dash_len as usize);
        self.dash_data.get(start..end).unwrap_or(&[])
    }

    /// A 64-bit content hash over every buffer and the unit.
    ///
    /// Non-cryptographic and deliberately so (Doc 03 §3): this identifies
    /// content for caching, it does not defend against adversarial input.
    /// Two scenes with this hash equal are equal byte for byte in practice.
    pub fn content_hash(&self) -> u64 {
        let mut hasher = FxHasher::default();
        hasher.write_u8(self.unit.to_u8());
        hasher.write(bytemuck::cast_slice(&self.tags));
        hasher.write(bytemuck::cast_slice(&self.path_data));
        hasher.write(&self.path_verbs);
        hasher.write(bytemuck::cast_slice(&self.paths));
        hasher.write(bytemuck::cast_slice(&self.transforms));
        hasher.write(bytemuck::cast_slice(&self.paints));
        hasher.write(bytemuck::cast_slice(&self.stops));
        hasher.write(bytemuck::cast_slice(&self.strokes));
        hasher.write(bytemuck::cast_slice(&self.dash_data));
        hasher.write(bytemuck::cast_slice(&self.glyph_runs));
        hasher.write(bytemuck::cast_slice(&self.glyphs));
        hasher.write(bytemuck::cast_slice(&self.variations));
        hasher.write(bytemuck::cast_slice(&self.layers));
        hasher.write(bytemuck::cast_slice(&self.node_hashes));
        hasher.write(bytemuck::cast_slice(&self.node_descs));
        hasher.finish()
    }
}

/// A path's descriptor together with its geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathView<'a> {
    desc: PathDesc,
    verbs: &'a [u8],
    /// `x, y` pairs.
    data: &'a [f64],
}

impl<'a> PathView<'a> {
    #[inline]
    pub fn desc(&self) -> &PathDesc {
        &self.desc
    }

    #[inline]
    pub fn bounds(&self) -> Rect {
        let [x0, y0, x1, y1] = self.desc.bounds;
        Rect::new(x0, y0, x1, y1)
    }

    /// The recognised primitive this path is, if any (Doc 02 §3).
    pub fn shape(&self) -> PathShape {
        match ShapeKind::from_u32(self.desc.shape) {
            Some(ShapeKind::Rect) => PathShape::Rect(self.bounds()),
            Some(ShapeKind::RoundedRect) => {
                let [tl, tr, br, bl] = self.desc.radii;
                PathShape::RoundedRect(
                    self.bounds(),
                    otf_2d_engine_geom::RectRadii::new(tl, tr, br, bl),
                )
            }
            _ => PathShape::General,
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }

    /// Raw verb discriminants, densely packed.
    #[inline]
    pub fn raw_verbs(&self) -> &'a [u8] {
        self.verbs
    }

    /// Raw coordinates, as `x, y` pairs.
    #[inline]
    pub fn raw_points(&self) -> &'a [f64] {
        self.data
    }

    /// Decoded verbs. Unknown discriminants are impossible in a scene this
    /// crate encoded and are rejected at deserialisation, so this cannot fail.
    pub fn verbs(&self) -> impl Iterator<Item = PathVerb> + 'a {
        self.verbs.iter().map(|&v| decode_verb(v))
    }

    /// Decoded points.
    pub fn points(&self) -> impl Iterator<Item = Point> + 'a {
        self.data.chunks_exact(2).map(|c| Point::new(c[0], c[1]))
    }

    /// Rebuilds an owned [`Path`]. Allocates; stage 2 reads the buffers
    /// directly instead.
    pub fn to_path(&self) -> Path {
        let mut builder =
            otf_2d_engine_geom::PathBuilder::with_capacity(self.verbs.len(), self.data.len() / 2);
        let mut points = self.points();
        for verb in self.verbs() {
            match verb {
                PathVerb::MoveTo => {
                    builder.move_to(points.next().unwrap_or(Point::ORIGIN));
                }
                PathVerb::LineTo => {
                    builder.line_to(points.next().unwrap_or(Point::ORIGIN));
                }
                PathVerb::QuadTo => {
                    let c = points.next().unwrap_or(Point::ORIGIN);
                    let p = points.next().unwrap_or(Point::ORIGIN);
                    builder.quad_to(c, p);
                }
                PathVerb::CurveTo => {
                    let a = points.next().unwrap_or(Point::ORIGIN);
                    let b = points.next().unwrap_or(Point::ORIGIN);
                    let p = points.next().unwrap_or(Point::ORIGIN);
                    builder.curve_to(a, b, p);
                }
                PathVerb::ClosePath => {
                    builder.close();
                }
            }
        }
        builder.finish()
    }
}

#[inline]
fn encode_verb(verb: PathVerb) -> u8 {
    match verb {
        PathVerb::MoveTo => 0,
        PathVerb::LineTo => 1,
        PathVerb::QuadTo => 2,
        PathVerb::CurveTo => 3,
        PathVerb::ClosePath => 4,
    }
}

#[inline]
fn decode_verb(v: u8) -> PathVerb {
    match v {
        0 => PathVerb::MoveTo,
        1 => PathVerb::LineTo,
        2 => PathVerb::QuadTo,
        3 => PathVerb::CurveTo,
        _ => PathVerb::ClosePath,
    }
}

// ---------------------------------------------------------------- encoding

/// Raw, unvalidated arena appends.
///
/// `SceneBuilder` (T1.4) is the validating front door and the only thing
/// consumers should use. These are public because the raster and cache crates
/// build scenes in tests and benchmarks, but the encoding format is explicitly
/// not part of the stable API (Doc 02 §8).
#[doc(hidden)]
impl Scene {
    /// Appends an affine, reusing a recent identical entry when there is one.
    pub fn encode_transform(&mut self, affine: Affine) -> TransformRef {
        let rec = TransformRec(affine.as_coefficients());
        let start = self.transforms.len().saturating_sub(INTERN_WINDOW);
        for (offset, existing) in self.transforms[start..].iter().enumerate() {
            if *existing == rec {
                return TransformRef::new((start + offset) as u32);
            }
        }
        self.transforms.push(rec);
        TransformRef::new((self.transforms.len() - 1) as u32)
    }

    /// Appends a run of gradient stops.
    pub fn encode_stops(&mut self, stops: &[ColorStop]) -> StopsRef {
        let offset = self.stops.len() as u32;
        self.stops.extend(stops.iter().map(|s| ColorStopRec {
            offset: s.offset,
            color: s.color.to_premul(),
            color_space: color_space_to_u32(s.color.space),
        }));
        // The run's length rides in the paint that references it, so the
        // handle is just the start index.
        StopsRef::new(offset)
    }

    /// Appends variable-font axis coordinates.
    pub fn encode_variations(&mut self, coords: &[f32]) -> VariationsRef {
        if coords.is_empty() {
            return VariationsRef::NONE;
        }
        let offset = self.variations.len() as u32;
        self.variations.extend_from_slice(coords);
        VariationsRef::new(offset)
    }

    /// Appends a path's verbs, points, bounds and shape hint.
    pub fn encode_path(&mut self, path: &Path) -> PathRef {
        let verb_offset = self.path_verbs.len() as u32;
        self.path_verbs
            .extend(path.verbs().iter().map(|&v| encode_verb(v)));
        let point_offset = self.path_data.len() as u32;
        for p in path.points() {
            self.path_data.push(p.x);
            self.path_data.push(p.y);
        }

        let bounds = path.control_bounds();
        let (shape, radii) = match path.shape() {
            PathShape::General => (ShapeKind::General, [0.0; 4]),
            PathShape::Rect(_) => (ShapeKind::Rect, [0.0; 4]),
            PathShape::RoundedRect(_, r) => (
                ShapeKind::RoundedRect,
                [r.top_left, r.top_right, r.bottom_right, r.bottom_left],
            ),
        };

        self.paths.push(PathDesc {
            bounds: [bounds.x0, bounds.y0, bounds.x1, bounds.y1],
            radii,
            verb_offset,
            verb_len: self.path_verbs.len() as u32 - verb_offset,
            point_offset,
            point_len: self.path_data.len() as u32 - point_offset,
            shape: shape.to_u32(),
            reserved: 0,
        });
        PathRef::new((self.paths.len() - 1) as u32)
    }

    /// Appends a paint, reusing a recent identical entry when there is one.
    ///
    /// `stops_len` is supplied separately because [`StopsRef`] carries only
    /// the run's start.
    pub fn encode_paint(&mut self, paint: &Paint, stops_len: u32) -> PaintRef {
        let desc = match *paint {
            Paint::Solid(color) => PaintDesc {
                kind: PaintKind::Solid.to_u32(),
                mode: 0,
                color_space: color_space_to_u32(color.space),
                flags: 0,
                color: color.to_premul(),
                geometry: [0.0; 6],
                stops_offset: 0,
                stops_len: 0,
                image: NO_REF,
                transform: NO_REF,
            },
            Paint::LinearGradient {
                start,
                end,
                stops,
                extend,
            } => PaintDesc {
                kind: PaintKind::LinearGradient.to_u32(),
                mode: extend as u32,
                color_space: color_space_to_u32(ColorSpace::Srgb),
                flags: 0,
                color: [0.0; 4],
                geometry: [start.x, start.y, end.x, end.y, 0.0, 0.0],
                stops_offset: stops.index(),
                stops_len,
                image: NO_REF,
                transform: NO_REF,
            },
            Paint::RadialGradient {
                center,
                radius,
                focal,
                stops,
                extend,
            } => {
                let f = focal.unwrap_or(center);
                PaintDesc {
                    kind: PaintKind::RadialGradient.to_u32(),
                    mode: extend as u32,
                    color_space: color_space_to_u32(ColorSpace::Srgb),
                    flags: if focal.is_some() {
                        PAINT_FLAG_HAS_FOCAL
                    } else {
                        0
                    },
                    color: [0.0; 4],
                    geometry: [center.x, center.y, radius, f.x, f.y, 0.0],
                    stops_offset: stops.index(),
                    stops_len,
                    image: NO_REF,
                    transform: NO_REF,
                }
            }
            Paint::Image {
                image,
                sampling,
                transform,
            } => PaintDesc {
                kind: PaintKind::Image.to_u32(),
                mode: sampling as u32,
                color_space: color_space_to_u32(ColorSpace::Srgb),
                flags: 0,
                color: [0.0; 4],
                geometry: [0.0; 6],
                stops_offset: 0,
                stops_len: 0,
                image: image.index(),
                transform: transform.index(),
            },
        };

        let start = self.paints.len().saturating_sub(INTERN_WINDOW);
        for (offset, existing) in self.paints[start..].iter().enumerate() {
            if *existing == desc {
                return PaintRef::new((start + offset) as u32);
            }
        }
        self.paints.push(desc);
        PaintRef::new((self.paints.len() - 1) as u32)
    }

    /// Appends a stroke style and its dash pattern.
    pub fn encode_stroke(&mut self, style: &StrokeStyle) -> StrokeRef {
        let (dash_offset_index, dash_len, dash_offset) = match &style.dash {
            Some(dash) if !dash.is_degenerate() => {
                let start = self.dash_data.len() as u32;
                self.dash_data.extend_from_slice(&dash.pattern);
                (start, dash.pattern.len() as u32, dash.offset)
            }
            _ => (0, 0, 0.0),
        };
        let (join, miter_limit) = match style.join {
            Join::Miter { limit } => (JOIN_MITER, limit),
            Join::Round => (JOIN_ROUND, 0.0),
            Join::Bevel => (JOIN_BEVEL, 0.0),
        };
        self.strokes.push(StrokeDesc {
            width: style.width,
            miter_limit,
            join,
            start_cap: style.start_cap as u32,
            end_cap: style.end_cap as u32,
            dash_offset,
            dash_offset_index,
            dash_len,
        });
        StrokeRef::new((self.strokes.len() - 1) as u32)
    }

    /// Appends a glyph run.
    pub fn encode_glyph_run(
        &mut self,
        font: FontRef,
        size: f32,
        glyphs: &[Glyph],
        options: &GlyphOptions,
        variations_len: u32,
    ) -> GlyphRunRef {
        let glyph_offset = self.glyphs.len() as u32;
        self.glyphs.extend(glyphs.iter().map(|g| GlyphRec {
            id: g.id,
            x: g.x,
            y: g.y,
        }));
        let (variations_offset, variations_len) = match options.variations.get() {
            Some(offset) => (offset as u32, variations_len),
            None => (0, 0),
        };
        self.glyph_runs.push(GlyphRunDesc {
            font: font.index(),
            size,
            glyph_offset,
            glyph_len: glyphs.len() as u32,
            variations_offset,
            variations_len,
            synthetic_bold: options.synthetic_bold,
            synthetic_skew: options.synthetic_skew,
            hinting: options.hinting as u32,
            reserved: 0,
        });
        GlyphRunRef::new((self.glyph_runs.len() - 1) as u32)
    }

    /// Appends a fill command.
    pub fn encode_fill(
        &mut self,
        rule: FillRule,
        transform: TransformRef,
        paint: PaintRef,
        path: PathRef,
    ) {
        self.tags.push(DrawTag {
            kind: DrawKind::Fill.to_u8(),
            flags: if rule == FillRule::EvenOdd {
                FLAG_EVEN_ODD
            } else {
                0
            },
            reserved: 0,
            transform: transform.index(),
            paint: paint.index(),
            payload: path.index(),
            aux: NO_REF,
        });
    }

    /// Appends a stroke command.
    pub fn encode_stroke_draw(
        &mut self,
        style: StrokeRef,
        transform: TransformRef,
        paint: PaintRef,
        path: PathRef,
    ) {
        self.tags.push(DrawTag {
            kind: DrawKind::Stroke.to_u8(),
            flags: 0,
            reserved: 0,
            transform: transform.index(),
            paint: paint.index(),
            payload: path.index(),
            aux: style.index(),
        });
    }

    /// Appends a glyph-run command.
    pub fn encode_glyphs(&mut self, run: GlyphRunRef, transform: TransformRef, paint: PaintRef) {
        self.tags.push(DrawTag {
            kind: DrawKind::Glyphs.to_u8(),
            flags: 0,
            reserved: 0,
            transform: transform.index(),
            paint: paint.index(),
            payload: run.index(),
            aux: NO_REF,
        });
    }

    /// Appends an image command. `alpha` rides in the paint's colour slot.
    pub fn encode_image(
        &mut self,
        image: ImageRef,
        transform: TransformRef,
        sampling: Sampling,
        alpha: f32,
    ) {
        let paint = self.encode_paint(
            &Paint::Image {
                image,
                sampling,
                transform,
            },
            0,
        );
        // Alpha travels as a premultiplied white so the fine loop needs no
        // separate per-draw opacity field.
        self.paints[paint.index() as usize].color = [alpha, alpha, alpha, alpha];
        self.tags.push(DrawTag {
            kind: DrawKind::Image.to_u8(),
            flags: 0,
            reserved: 0,
            transform: transform.index(),
            paint: paint.index(),
            payload: image.index(),
            aux: NO_REF,
        });
    }

    /// Opens a layer, returning its index in the layer arena.
    pub fn encode_push_layer(
        &mut self,
        blend: BlendMode,
        alpha: f32,
        transform: TransformRef,
        clip: PathRef,
    ) -> u32 {
        let push_tag = self.tags.len() as u32;
        let layer = self.layers.len() as u32;
        self.layers.push(LayerDesc {
            blend: blend as u32,
            alpha,
            clip_path: clip.index(),
            transform: transform.index(),
            push_tag,
            pop_tag: NO_REF,
        });
        self.tags.push(DrawTag {
            kind: DrawKind::PushLayer.to_u8(),
            flags: 0,
            reserved: 0,
            transform: transform.index(),
            paint: NO_REF,
            payload: layer,
            aux: NO_REF,
        });
        layer
    }

    /// Closes the layer at `layer`, recording where its pop landed.
    pub fn encode_pop_layer(&mut self, layer: u32) {
        let pop_tag = self.tags.len() as u32;
        if let Some(desc) = self.layers.get_mut(layer as usize) {
            desc.pop_tag = pop_tag;
        }
        self.tags.push(DrawTag {
            kind: DrawKind::PopLayer.to_u8(),
            flags: 0,
            reserved: 0,
            transform: NO_REF,
            paint: NO_REF,
            payload: layer,
            aux: NO_REF,
        });
    }

    /// Opens a reusable subtree, returning its node index.
    ///
    /// Reserved: written from T1.3 so the arena layout can express it, read by
    /// the node cache in M6 (Doc 03 §9). Nothing consumes it before then.
    pub fn encode_push_node(&mut self, id: NodeId, parent: u32) -> u32 {
        self.node_descs.push(NodeDesc {
            id: id.0,
            tag_offset: self.tags.len() as u32,
            tag_len: 0,
            parent,
            reserved: 0,
        });
        self.node_hashes.push(NodeHash::UNSET.0);
        (self.node_descs.len() - 1) as u32
    }

    /// Closes a subtree, recording how many tags it contributed.
    pub fn encode_pop_node(&mut self, node: u32, hash: NodeHash) {
        let tags = self.tags.len() as u32;
        if let Some(desc) = self.node_descs.get_mut(node as usize) {
            desc.tag_len = tags.saturating_sub(desc.tag_offset);
        }
        if let Some(slot) = self.node_hashes.get_mut(node as usize) {
            *slot = hash.0;
        }
    }

    /// The number of layer records, so a builder can tell whether any are open.
    #[inline]
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    /// The number of stops encoded so far, so a builder can compute run lengths.
    #[inline]
    pub fn stop_count(&self) -> usize {
        self.stops.len()
    }

    /// The number of variation coordinates encoded so far.
    #[inline]
    pub fn variation_count(&self) -> usize {
        self.variations.len()
    }
}

/// `ColorSpace` is `#[non_exhaustive]`, so an unrecognised variant must have a
/// defined encoding rather than a compile error here. sRGB is the safe answer:
/// it is the default and cannot itself be the unrecognised one.
fn color_space_to_u32(space: ColorSpace) -> u32 {
    match space {
        ColorSpace::Srgb => 0,
        ColorSpace::DisplayP3 => 1,
        ColorSpace::Rec2020 => 2,
        _ => 0,
    }
}

/// Decodes `color_space_to_u32`, falling back to sRGB for unknown values.
pub fn color_space_from_u32(v: u32) -> ColorSpace {
    match v {
        1 => ColorSpace::DisplayP3,
        2 => ColorSpace::Rec2020,
        _ => ColorSpace::Srgb,
    }
}

/// Rebuilds a [`Color`] from a stored premultiplied record.
pub fn color_from_record(rgba: [f32; 4], space: u32) -> Color {
    Color::from_premul_f32(
        rgba[0],
        rgba[1],
        rgba[2],
        rgba[3],
        color_space_from_u32(space),
    )
}
