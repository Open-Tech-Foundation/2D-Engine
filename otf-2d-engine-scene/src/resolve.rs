//! Stage 2 — resolve (Doc 01 §4).
//!
//! Turns a [`Scene`] into a flat, ordered draw list with absolute transforms,
//! collapsed clips and off-target draws removed.
//!
//! # What "no tree" means here
//!
//! After this stage nothing downstream walks a hierarchy. Layer nesting
//! survives only as *order*: a `BeginLayer` draw, the draws it contains, an
//! `EndLayer` draw — exactly the way the tag stream expresses it. Every record
//! this module produces is [`Copy`], which is the type-level proof: a recursive
//! field would need heap indirection, and nothing heap-indirected is `Copy`.
//!
//! # Resolution independence
//!
//! Stage 2's output feeds the vector seam (Doc 01 §6), so it must not bake in
//! anything that depends on device resolution. Curves stay curves — this module
//! never touches path geometry, it only computes bounding boxes from it — and
//! there is no tolerance anywhere. Flattening tolerance is derived in stage 3
//! from the resolved transform, which is why the transform is what stage 2
//! hands on.
//!
//! # Where the transform stack went
//!
//! Doc 01 §4 describes stage 2 as collapsing a transform stack. In this engine
//! there is no stack to collapse: [`crate::SceneBuilder`] has no current
//! transform (I-3), so every draw already carries its own absolute scene-space
//! affine. What stage 2 composes is that affine with the scene-to-device
//! transform from [`ResolveParams`]. The stack that does still exist is the
//! *clip* stack, and collapsing it is real work.

use alloc::vec::Vec;

use otf_2d_engine_color::BlendMode;
use otf_2d_engine_geom::{Affine, Rect};

use crate::handles::{GlyphRunRef, ImageRef, PaintRef, PathRef, StrokeRef};
use crate::records::{DrawKind, FLAG_EVEN_ODD};
use crate::scene::Scene;
use crate::style::{Cap, FillRule, Sampling};

/// What a resolved entry does.
///
/// Every payload is a handle or a value; nothing names a [`ResolvedScene`], so
/// the list cannot be a tree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResolvedKind {
    Fill {
        rule: FillRule,
        path: PathRef,
    },
    Stroke {
        style: StrokeRef,
        path: PathRef,
    },
    Glyphs {
        run: GlyphRunRef,
    },
    Image {
        image: ImageRef,
        sampling: Sampling,
        alpha: f32,
    },
    /// Opens the layer at this index in [`ResolvedScene::layers`].
    BeginLayer {
        layer: u32,
    },
    /// Closes it.
    EndLayer {
        layer: u32,
    },
}

/// One entry in the resolved draw list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedDraw {
    pub kind: ResolvedKind,
    /// Scene affine composed with the scene-to-device transform. Absolute.
    pub transform: Affine,
    /// The paint, or [`PaintRef::NONE`] for layer boundaries.
    pub paint: PaintRef,
    /// Index into [`ResolvedScene::clips`]. Always valid: entry 0 is the
    /// target rectangle, so there is no "unclipped" special case.
    pub clip: u32,
    /// Conservative device-space bounds. [`Rect::EVERYTHING`] when the extent
    /// is not knowable at this stage.
    pub bounds: Rect,
}

/// A layer, with the range of draws it spans.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedLayer {
    pub blend: BlendMode,
    pub alpha: f32,
    /// The clip in force inside the layer.
    pub clip: u32,
    /// Device-space extent of the layer's content, already intersected with
    /// its clip. This is what stage 7 sizes its offscreen buffer from — never
    /// the full surface (Doc 01 §4).
    pub bounds: Rect,
    /// Index of this layer's `BeginLayer` draw.
    pub first_draw: u32,
    /// Draws from `BeginLayer` through `EndLayer`, inclusive.
    pub draw_len: u32,
}

/// A fully collapsed clip.
///
/// The rectangular part is intersected down to a single rect, which is the
/// overwhelming majority of UI clipping. Anything not expressible that way
/// becomes a mask, and masks accumulate in a flat list rather than a chain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedClip {
    /// Intersection of the target, the damage region and every rectangular
    /// clip in scope.
    pub rect: Rect,
    /// Range into [`ResolvedScene::masks`].
    pub mask_offset: u32,
    pub mask_len: u32,
}

impl ResolvedClip {
    /// True when this clip is a plain rectangle, so stage 6 needs no mask.
    #[inline]
    pub fn is_rectangular(&self) -> bool {
        self.mask_len == 0
    }
}

/// One non-rectangular clip contributing to a [`ResolvedClip`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipMask {
    pub path: PathRef,
    /// Absolute device-space transform for the mask path.
    pub transform: Affine,
    /// Conservative device-space bounds of the mask.
    pub bounds: Rect,
}

/// How stage 2 maps a scene onto a device.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolveParams {
    /// Scene-to-device transform: DPI scaling, page placement, canvas offset.
    pub transform: Affine,
    /// Device-space bounds of the render target.
    pub target: Rect,
    /// Restrict output to this device-space region. `None` means the whole
    /// target. Damage is derived, not consumer-supplied (Doc 03 §5); this is
    /// where the derived rect enters the pipeline.
    pub damage: Option<Rect>,
    /// Whether to drop draws that cannot affect the target.
    ///
    /// Culling is an optimisation, so it is disableable (P5, I-6). With it off
    /// every draw survives and the rendered result must be identical — that
    /// equality is what makes the fast path testable.
    pub cull: bool,
}

impl ResolveParams {
    /// Identity transform, no damage restriction, culling on.
    pub fn new(target: Rect) -> ResolveParams {
        ResolveParams {
            transform: Affine::IDENTITY,
            target,
            damage: None,
            cull: true,
        }
    }

    /// Sets the scene-to-device transform.
    pub fn with_transform(mut self, transform: Affine) -> ResolveParams {
        self.transform = transform;
        self
    }

    /// Restricts output to a damage region.
    pub fn with_damage(mut self, damage: Rect) -> ResolveParams {
        self.damage = Some(damage);
        self
    }

    /// Turns culling off, for the reference path P5 requires.
    pub fn without_culling(mut self) -> ResolveParams {
        self.cull = false;
        self
    }

    /// The region a draw must touch to be visible.
    fn visible_region(&self) -> Rect {
        match self.damage {
            Some(damage) => self.target.intersect(damage),
            None => self.target,
        }
    }
}

/// What stage 2 did, for `RenderStats` and for tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResolveStats {
    /// Draw commands read from the scene.
    pub tags_in: usize,
    /// Entries emitted, including layer boundaries.
    pub draws_out: usize,
    /// Draws dropped because they could not affect the visible region.
    pub culled: usize,
    /// Draws dropped because a handle named nothing. Only reachable from
    /// hand-built scenes; `SceneBuilder` cannot produce one.
    pub dangling: usize,
    /// Layers resolved.
    pub layers: usize,
    /// Non-rectangular clip masks recorded.
    pub masks: usize,
    /// Layers left open by the scene and closed here.
    pub unclosed_layers: usize,
}

/// The output of stage 2.
///
/// Borrows both the scene it resolved and the buffers it was resolved into, so
/// no geometry is copied: a path in the resolved list is the same path in the
/// arena, curves and all.
#[derive(Debug, Clone, Copy)]
pub struct ResolvedScene<'a> {
    scene: &'a Scene,
    draws: &'a [ResolvedDraw],
    layers: &'a [ResolvedLayer],
    clips: &'a [ResolvedClip],
    masks: &'a [ClipMask],
    stats: ResolveStats,
}

impl<'a> ResolvedScene<'a> {
    /// The scene this was resolved from. Geometry and paints are read from it.
    #[inline]
    pub fn scene(&self) -> &'a Scene {
        self.scene
    }

    /// The draw list, in submission order.
    #[inline]
    pub fn draws(&self) -> &'a [ResolvedDraw] {
        self.draws
    }

    #[inline]
    pub fn layers(&self) -> &'a [ResolvedLayer] {
        self.layers
    }

    #[inline]
    pub fn clips(&self) -> &'a [ResolvedClip] {
        self.clips
    }

    #[inline]
    pub fn masks(&self) -> &'a [ClipMask] {
        self.masks
    }

    #[inline]
    pub fn stats(&self) -> ResolveStats {
        self.stats
    }

    /// The clip a draw is under. Never fails: index 0 is the target rect.
    #[inline]
    pub fn clip(&self, draw: &ResolvedDraw) -> ResolvedClip {
        self.clips[draw.clip as usize]
    }

    /// The masks contributing to a clip.
    pub fn clip_masks(&self, clip: &ResolvedClip) -> &'a [ClipMask] {
        let start = clip.mask_offset as usize;
        self.masks
            .get(start..start.saturating_add(clip.mask_len as usize))
            .unwrap_or(&[])
    }
}

/// One frame of resolve state.
///
/// Kept alive across frames so the buffers are reused: a steady-state resolve
/// allocates nothing (I-9).
#[derive(Debug, Clone, Default)]
pub struct Resolver {
    draws: Vec<ResolvedDraw>,
    layers: Vec<ResolvedLayer>,
    clips: Vec<ResolvedClip>,
    masks: Vec<ClipMask>,
    stack: Vec<Open>,
    stats: ResolveStats,
}

/// A layer that has been opened and not yet closed.
#[derive(Debug, Clone, Copy)]
struct Open {
    layer: u32,
    clip: u32,
    first_draw: u32,
    /// Union of the bounds of everything inside, before clipping.
    bounds: Rect,
}

impl Resolver {
    pub fn new() -> Resolver {
        Resolver::default()
    }

    /// Bytes currently held, so a consumer can budget stage 2 like the arena.
    pub fn memory_usage(&self) -> usize {
        core::mem::size_of_val(&self.draws[..])
            + core::mem::size_of_val(&self.layers[..])
            + core::mem::size_of_val(&self.clips[..])
            + core::mem::size_of_val(&self.masks[..])
            + core::mem::size_of_val(&self.stack[..])
    }

    /// Resolves `scene` for the given target.
    pub fn resolve<'a>(
        &'a mut self,
        scene: &'a Scene,
        params: &ResolveParams,
    ) -> ResolvedScene<'a> {
        self.draws.clear();
        self.layers.clear();
        self.clips.clear();
        self.masks.clear();
        self.stack.clear();
        self.stats = ResolveStats::default();

        // Clip 0 is the root: the visible region. Every draw indexes a real
        // clip, so nothing downstream needs an "unclipped" branch.
        self.clips.push(ResolvedClip {
            rect: params.visible_region(),
            mask_offset: 0,
            mask_len: 0,
        });

        self.stats.tags_in = scene.tags().len();
        for index in 0..scene.tags().len() {
            let tag = scene.tags()[index];
            let Some(kind) = DrawKind::from_u8(tag.kind) else {
                self.stats.dangling += 1;
                continue;
            };
            match kind {
                DrawKind::PushLayer => self.open_layer(scene, params, tag.payload),
                DrawKind::PopLayer => self.close_layer(),
                DrawKind::Fill | DrawKind::Stroke | DrawKind::Glyphs | DrawKind::Image => {
                    self.draw(scene, params, index, kind)
                }
            }
        }

        // A scene from `SceneBuilder` is always balanced, but a hand-built one
        // need not be. Closing here keeps stage 2 total: the draw list a
        // backend sees is balanced whatever it was handed.
        while !self.stack.is_empty() {
            self.stats.unclosed_layers += 1;
            self.close_layer();
        }

        self.stats.draws_out = self.draws.len();
        self.stats.layers = self.layers.len();
        self.stats.masks = self.masks.len();

        ResolvedScene {
            scene,
            draws: &self.draws,
            layers: &self.layers,
            clips: &self.clips,
            masks: &self.masks,
            stats: self.stats,
        }
    }

    /// The clip in force at the current nesting depth.
    fn current_clip(&self) -> u32 {
        self.stack.last().map_or(0, |open| open.clip)
    }

    fn open_layer(&mut self, scene: &Scene, params: &ResolveParams, layer_index: u32) {
        let Some(desc) = scene.layers().get(layer_index as usize).copied() else {
            self.stats.dangling += 1;
            return;
        };
        let parent_clip = self.current_clip();
        let transform = scene
            .transform(crate::handles::TransformRef::new(desc.transform))
            .then(params.transform);

        let clip = self.push_clip(scene, parent_clip, PathRef::new(desc.clip_path), transform);

        let layer = self.layers.len() as u32;
        let first_draw = self.draws.len() as u32;
        self.layers.push(ResolvedLayer {
            blend: blend_from_u32(desc.blend),
            alpha: desc.alpha,
            clip,
            bounds: Rect::NOTHING,
            first_draw,
            draw_len: 0,
        });
        self.draws.push(ResolvedDraw {
            kind: ResolvedKind::BeginLayer { layer },
            transform,
            paint: PaintRef::NONE,
            clip,
            bounds: self.clips[clip as usize].rect,
        });
        self.stack.push(Open {
            layer,
            clip,
            first_draw,
            bounds: Rect::NOTHING,
        });
    }

    fn close_layer(&mut self) {
        let Some(open) = self.stack.pop() else {
            // A pop with nothing open. Ignore it rather than panic: stage 2
            // must survive a hand-built scene (I-8).
            self.stats.dangling += 1;
            return;
        };
        let clip_rect = self.clips[open.clip as usize].rect;
        let bounds = open.bounds.intersect(clip_rect);

        self.draws.push(ResolvedDraw {
            kind: ResolvedKind::EndLayer { layer: open.layer },
            transform: Affine::IDENTITY,
            paint: PaintRef::NONE,
            clip: open.clip,
            bounds,
        });

        let draw_len = self.draws.len() as u32 - open.first_draw;
        let layer = &mut self.layers[open.layer as usize];
        layer.bounds = bounds;
        layer.draw_len = draw_len;

        // A nested layer contributes its extent to its parent.
        self.accumulate(bounds);
    }

    /// Adds `bounds` to the innermost open layer's extent.
    fn accumulate(&mut self, bounds: Rect) {
        if let Some(open) = self.stack.last_mut() {
            open.bounds = open.bounds.union(bounds);
        }
    }

    fn draw(&mut self, scene: &Scene, params: &ResolveParams, index: usize, kind: DrawKind) {
        let tag = scene.tags()[index];
        let transform = scene
            .transform(crate::handles::TransformRef::new(tag.transform))
            .then(params.transform);

        let (resolved, bounds) = match kind {
            DrawKind::Fill => {
                let path = PathRef::new(tag.payload);
                let Some(view) = scene.path(path) else {
                    self.stats.dangling += 1;
                    return;
                };
                let rule = if tag.flags & FLAG_EVEN_ODD != 0 {
                    FillRule::EvenOdd
                } else {
                    FillRule::NonZero
                };
                (
                    ResolvedKind::Fill { rule, path },
                    device_bounds(transform, view.bounds()),
                )
            }
            DrawKind::Stroke => {
                let path = PathRef::new(tag.payload);
                let style = StrokeRef::new(tag.aux);
                let (Some(view), Some(desc)) = (
                    scene.path(path),
                    style.get().and_then(|i| scene.strokes().get(i)).copied(),
                ) else {
                    self.stats.dangling += 1;
                    return;
                };
                let outset = stroke_outset(&desc, transform);
                (
                    ResolvedKind::Stroke { style, path },
                    device_bounds(transform, view.bounds()).inflate(outset),
                )
            }
            DrawKind::Glyphs => {
                let run = GlyphRunRef::new(tag.payload);
                if run.get().and_then(|i| scene.glyph_runs().get(i)).is_none() {
                    self.stats.dangling += 1;
                    return;
                }
                // Bounds need the outlines, which arrive with the glyph cache
                // in M4. Guessing an em box would risk culling a swash or an
                // accent that reaches outside it, and dropped text is a worse
                // failure than a draw that turns out to be off-screen.
                (ResolvedKind::Glyphs { run }, Rect::EVERYTHING)
            }
            DrawKind::Image => {
                let Some(paint) = PaintRef::new(tag.paint)
                    .get()
                    .and_then(|i| scene.paints().get(i))
                    .copied()
                else {
                    self.stats.dangling += 1;
                    return;
                };
                // The image's pixel extent lives in the caller's registry,
                // which the scene does not own, so its bounds are unknown
                // here.
                (
                    ResolvedKind::Image {
                        image: ImageRef::new(paint.image),
                        sampling: sampling_from_u32(paint.mode),
                        alpha: paint.color[3],
                    },
                    Rect::EVERYTHING,
                )
            }
            DrawKind::PushLayer | DrawKind::PopLayer => return,
        };

        let clip = self.current_clip();
        if params.cull && !bounds.intersects(self.clips[clip as usize].rect) {
            self.stats.culled += 1;
            return;
        }

        self.accumulate(bounds);
        self.draws.push(ResolvedDraw {
            kind: resolved,
            transform,
            paint: PaintRef::new(tag.paint),
            clip,
            bounds,
        });
    }

    /// Intersects `parent` with a clip path, returning the new clip's index.
    fn push_clip(&mut self, scene: &Scene, parent: u32, path: PathRef, transform: Affine) -> u32 {
        let parent_clip = self.clips[parent as usize];
        let Some(view) = scene.path(path) else {
            return parent;
        };
        let bounds = device_bounds(transform, view.bounds());

        // An axis-aligned rectangle under an axis-preserving transform is
        // exactly its bounding box, so it collapses into the rect and costs
        // stage 6 nothing. Anything else needs a mask.
        let rectangular = matches!(view.shape(), otf_2d_engine_geom::PathShape::Rect(_))
            && transform.preserves_axis_alignment();

        let (mask_offset, mask_len) = if rectangular {
            (parent_clip.mask_offset, parent_clip.mask_len)
        } else {
            // Copy the parent's masks so the child owns a contiguous range.
            // Siblings would otherwise interleave in the buffer and a range
            // would pick up a cousin's mask.
            let offset = self.masks.len() as u32;
            let start = parent_clip.mask_offset as usize;
            for i in start..start + parent_clip.mask_len as usize {
                let mask = self.masks[i];
                self.masks.push(mask);
            }
            self.masks.push(ClipMask {
                path,
                transform,
                bounds,
            });
            (offset, parent_clip.mask_len + 1)
        };

        self.clips.push(ResolvedClip {
            rect: parent_clip.rect.intersect(bounds),
            mask_offset,
            mask_len,
        });
        (self.clips.len() - 1) as u32
    }
}

/// Transforms `rect` into device space, keeping an unknown extent unknown.
///
/// `transform_rect_bbox` on an infinite rect produces `NaN` from `0 * inf`, so
/// the unbounded case is answered before the arithmetic runs.
fn device_bounds(transform: Affine, rect: Rect) -> Rect {
    if !rect.is_finite() {
        return if rect.is_empty() {
            Rect::NOTHING
        } else {
            Rect::EVERYTHING
        };
    }
    transform.transform_rect_bbox(rect)
}

/// How far a stroke can reach outside its path's control bounds, in device
/// space.
///
/// Deliberately generous: this feeds culling, where over-estimating costs a
/// draw that turns out to be invisible and under-estimating drops one that is
/// not.
fn stroke_outset(desc: &crate::records::StrokeDesc, transform: Affine) -> f64 {
    if !desc.width.is_finite() || desc.width <= 0.0 {
        return 0.0;
    }
    let half = desc.width as f64 * 0.5;
    // A miter join reaches `miter_limit` half-widths from the corner; round
    // and bevel reach one. A square cap reaches half a width past the endpoint
    // and √2 half-widths into the corner.
    let join = if desc.join == crate::records::JOIN_MITER {
        (desc.miter_limit as f64).max(1.0)
    } else {
        1.0
    };
    let square = desc.start_cap == Cap::Square as u32 || desc.end_cap == Cap::Square as u32;
    let cap = if square {
        core::f64::consts::SQRT_2
    } else {
        1.0
    };
    half * join.max(cap) * transform.max_scale()
}

/// `BlendMode` is `#[non_exhaustive]`; an unrecognised discriminant falls back
/// to the one mode v1 defines rather than refusing to resolve.
fn blend_from_u32(v: u32) -> BlendMode {
    match v {
        0 => BlendMode::SrcOver,
        _ => BlendMode::SrcOver,
    }
}

fn sampling_from_u32(v: u32) -> Sampling {
    match v {
        0 => Sampling::Nearest,
        _ => Sampling::Bilinear,
    }
}
