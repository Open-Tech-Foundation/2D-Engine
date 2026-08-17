//! The encoding API (Doc 02 §5).
//!
//! Note what is missing: no `save`, no `restore`, no current point, no current
//! paint, no current transform. Every call carries everything it needs (I-3).
//! A builder holds a mutable borrow of one [`Scene`] and a small fixed-size
//! stack; it never allocates, so a re-encoded frame stays inside I-9.
//!
//! # Validation is total (I-8)
//!
//! Every method returns `Result<(), EncodeError>`, and a scene that encoded
//! without error is guaranteed rasterizable: no mid-render failure, no partial
//! output, no panic. Consumers encode on worker threads, where a panic is
//! expensive and a poisoned scene is worse.
//!
//! Two structural guarantees back that up beyond the per-argument checks:
//!
//! * Layers are balanced unconditionally — dropping a builder with layers
//!   still open closes them. [`SceneBuilder::finish`] reports that it had to,
//!   for consumers that want to know.
//! * Nodes are balanced by the borrow checker: [`NodeScope`] holds the builder
//!   for the length of the subtree and closes the node when it drops.

use core::fmt;
use core::ops::{Deref, DerefMut};

use otf_2d_engine_color::{BlendMode, Color};
use otf_2d_engine_geom::{Affine, Path};

use crate::handles::{FontRef, ImageRef, NO_REF, NodeHash, NodeId, StopsRef, VariationsRef};
use crate::scene::Scene;
use crate::style::{
    ColorStop, Dash, FillRule, Glyph, GlyphOptions, Join, Paint, Sampling, StrokeStyle,
};

/// The deepest layer nesting a scene may contain.
///
/// Stage 6 composites layers with a stack of scratch buffers, so depth is a
/// memory cost, not just a counter. Real content nests a handful deep; 256 is
/// far past anything an authoring tool emits and keeps the builder's own stack
/// at a kilobyte, which is what lets it be a fixed-size array and allocate
/// nothing.
pub const MAX_LAYER_DEPTH: u32 = 256;

/// The most points one path may contribute.
///
/// Arena offsets are `u32` (I-2), so the hard ceiling is `u32::MAX` elements
/// across the whole scene. This per-path limit sits far below it, so a single
/// runaway path is rejected with a clear error long before the arena itself
/// runs out of addressing.
///
/// A megapoint is roughly ten times the largest path real content produces —
/// a detailed map coastline runs to a few tens of thousands — and it is small
/// enough that hitting the limit is cheap to test, which matters more than the
/// exact figure: a limit no test can reach is a limit nothing verifies.
pub const MAX_PATH_POINTS: usize = 1 << 20;

/// The most glyphs one run may contain.
pub const MAX_GLYPHS_PER_RUN: usize = 1 << 20;

/// Why an encode call was refused.
///
/// `#[non_exhaustive]`: adding a rejection reason is not a breaking change,
/// and a consumer that matches exhaustively today would otherwise pin the set
/// forever (Doc 02 §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EncodeError {
    /// A coordinate, transform coefficient, colour component, dimension or
    /// alpha was `NaN` or infinite.
    ///
    /// These are rejected rather than clamped because there is no correct
    /// clamp: an infinite coordinate is a bug in the consumer, and silently
    /// picking a finite stand-in hides it until the output looks wrong.
    NonFiniteCoordinate,
    /// [`SceneBuilder::pop_layer`] with no layer open, or
    /// [`SceneBuilder::finish`] with one still open.
    UnbalancedLayer,
    /// Layer nesting exceeded [`MAX_LAYER_DEPTH`].
    LayerDepthExceeded {
        /// The limit that was hit.
        max: u32,
    },
    /// A path exceeded [`MAX_PATH_POINTS`], or a run would push an arena
    /// buffer past the `u32` addressing every handle depends on.
    PathTooLarge {
        /// The number of elements that would still have fit.
        limit: usize,
    },
    /// A glyph run was empty, too long, or carried a non-finite size or
    /// position.
    InvalidGlyphRun,
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFiniteCoordinate => write!(f, "coordinate is not finite"),
            Self::UnbalancedLayer => write!(f, "layer push and pop are unbalanced"),
            Self::LayerDepthExceeded { max } => write!(f, "layer nesting exceeds {max}"),
            Self::PathTooLarge { limit } => write!(f, "path exceeds the limit of {limit} elements"),
            Self::InvalidGlyphRun => write!(f, "glyph run is empty or malformed"),
        }
    }
}

impl core::error::Error for EncodeError {}

/// Encodes draw commands into a [`Scene`].
///
/// ```
/// use otf_2d_engine_color::Color;
/// use otf_2d_engine_geom::{Affine, PathBuilder, Rect};
/// use otf_2d_engine_scene::{FillRule, Paint, Scene, SceneBuilder};
///
/// let mut scene = Scene::new();
/// let mut builder = SceneBuilder::new(&mut scene);
/// let square = PathBuilder::new().rect(Rect::new(0.0, 0.0, 16.0, 16.0)).build();
/// builder.fill(
///     FillRule::NonZero,
///     Affine::IDENTITY,
///     &Paint::Solid(Color::from_srgb8(0, 0, 0, 255)),
///     &square,
/// )?;
/// builder.finish()?;
/// # Ok::<(), otf_2d_engine_scene::EncodeError>(())
/// ```
pub struct SceneBuilder<'a> {
    scene: &'a mut Scene,
    /// Indices of the layers currently open, innermost last. A fixed array
    /// rather than a `Vec`: the builder must not allocate (I-9), and
    /// [`MAX_LAYER_DEPTH`] makes the bound structural.
    open_layers: [u32; MAX_LAYER_DEPTH as usize],
    depth: usize,
    /// The innermost open node, or [`NO_REF`].
    current_node: u32,
}

impl fmt::Debug for SceneBuilder<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SceneBuilder")
            .field("depth", &self.depth)
            .field("current_node", &self.current_node)
            .finish_non_exhaustive()
    }
}

impl<'a> SceneBuilder<'a> {
    /// Starts encoding into `scene`, appending to whatever is already there.
    pub fn new(scene: &'a mut Scene) -> SceneBuilder<'a> {
        SceneBuilder {
            scene,
            open_layers: [NO_REF; MAX_LAYER_DEPTH as usize],
            depth: 0,
            current_node: NO_REF,
        }
    }

    /// The scene being built, for inspection mid-encode.
    #[inline]
    pub fn scene(&self) -> &Scene {
        self.scene
    }

    /// How many layers are currently open.
    #[inline]
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Closes any layers still open and reports whether it had to.
    ///
    /// Calling this is optional — dropping the builder does the same closing —
    /// but it is the only way to learn that the consumer's pushes and pops did
    /// not match.
    pub fn finish(mut self) -> Result<(), EncodeError> {
        if self.close_open_layers() {
            return Err(EncodeError::UnbalancedLayer);
        }
        Ok(())
    }

    /// Pops every open layer. Returns true if there was anything to pop.
    fn close_open_layers(&mut self) -> bool {
        let had_open = self.depth > 0;
        while self.depth > 0 {
            self.depth -= 1;
            let layer = self.open_layers[self.depth];
            self.scene.encode_pop_layer(layer);
        }
        had_open
    }

    // ------------------------------------------------------------ interning

    /// Interns a run of gradient stops.
    ///
    /// Gradients name their stops by handle rather than carrying a slice,
    /// because the IR holds no pointers (Doc 02 §4, I-2). Stops must therefore
    /// be interned before the paint that uses them.
    pub fn intern_stops(&mut self, stops: &[ColorStop]) -> Result<StopsRef, EncodeError> {
        for stop in stops {
            if !stop.offset.is_finite() || !color_is_finite(stop.color) {
                return Err(EncodeError::NonFiniteCoordinate);
            }
        }
        check_room(self.scene.stops().len(), stops.len())?;
        Ok(self.scene.encode_stops(stops))
    }

    /// Interns variable-font axis coordinates for a glyph run.
    pub fn intern_variations(&mut self, coords: &[f32]) -> Result<VariationsRef, EncodeError> {
        if coords.iter().any(|c| !c.is_finite()) {
            return Err(EncodeError::NonFiniteCoordinate);
        }
        check_room(self.scene.variations().len(), coords.len())?;
        Ok(self.scene.encode_variations(coords))
    }

    // ------------------------------------------------------------ drawing

    /// Fills `path` with `paint`.
    pub fn fill(
        &mut self,
        rule: FillRule,
        transform: Affine,
        paint: &Paint,
        path: &Path,
    ) -> Result<(), EncodeError> {
        check_transform(transform)?;
        check_paint(paint)?;
        self.check_path(path)?;

        let transform = self.scene.encode_transform(transform);
        let paint = self.scene.encode_paint(paint);
        let path = self.scene.encode_path(path);
        self.scene.encode_fill(rule, transform, paint, path);
        Ok(())
    }

    /// Strokes `path` with `paint`.
    ///
    /// A non-positive width is accepted and draws nothing: it is a legitimate
    /// way to switch a stroke off from data, unlike a non-finite width, which
    /// is always a bug.
    pub fn stroke(
        &mut self,
        style: &StrokeStyle,
        transform: Affine,
        paint: &Paint,
        path: &Path,
    ) -> Result<(), EncodeError> {
        check_transform(transform)?;
        check_paint(paint)?;
        check_stroke(style)?;
        self.check_path(path)?;
        if let Some(dash) = &style.dash {
            check_room(self.scene.dash_data().len(), dash.pattern.len())?;
        }

        let style = self.scene.encode_stroke(style);
        let transform = self.scene.encode_transform(transform);
        let paint = self.scene.encode_paint(paint);
        let path = self.scene.encode_path(path);
        self.scene.encode_stroke_draw(style, transform, paint, path);
        Ok(())
    }

    /// Draws a shaped glyph run.
    ///
    /// 2D-Engine takes glyph indices and positions, never strings: shaping is
    /// the consumer's job (Doc 02 §6).
    pub fn draw_glyphs(
        &mut self,
        font: FontRef,
        size: f32,
        transform: Affine,
        paint: &Paint,
        glyphs: &[Glyph],
        options: GlyphOptions,
    ) -> Result<(), EncodeError> {
        check_transform(transform)?;
        check_paint(paint)?;
        check_glyph_run(size, glyphs, &options)?;
        if self.scene.glyphs().len().saturating_add(glyphs.len()) > u32::MAX as usize {
            return Err(EncodeError::InvalidGlyphRun);
        }

        let transform = self.scene.encode_transform(transform);
        let paint = self.scene.encode_paint(paint);
        let run = self.scene.encode_glyph_run(font, size, glyphs, &options);
        self.scene.encode_glyphs(run, transform, paint);
        Ok(())
    }

    /// Draws an image from the caller's registry.
    pub fn draw_image(
        &mut self,
        image: ImageRef,
        transform: Affine,
        sampling: Sampling,
        alpha: f32,
    ) -> Result<(), EncodeError> {
        check_transform(transform)?;
        if !alpha.is_finite() {
            return Err(EncodeError::NonFiniteCoordinate);
        }
        let transform = self.scene.encode_transform(transform);
        self.scene.encode_image(image, transform, sampling, alpha);
        Ok(())
    }

    // ------------------------------------------------------------ layers

    /// Opens a layer. Everything drawn until the matching
    /// [`SceneBuilder::pop_layer`] is composited as a group.
    pub fn push_layer(
        &mut self,
        blend: BlendMode,
        alpha: f32,
        transform: Affine,
        clip: Option<&Path>,
    ) -> Result<(), EncodeError> {
        if self.depth >= MAX_LAYER_DEPTH as usize {
            return Err(EncodeError::LayerDepthExceeded {
                max: MAX_LAYER_DEPTH,
            });
        }
        check_transform(transform)?;
        if !alpha.is_finite() {
            return Err(EncodeError::NonFiniteCoordinate);
        }
        if let Some(path) = clip {
            self.check_path(path)?;
        }

        let transform_ref = self.scene.encode_transform(transform);
        let clip_ref = match clip {
            Some(path) => self.scene.encode_path(path),
            None => crate::handles::PathRef::NONE,
        };
        let layer = self
            .scene
            .encode_push_layer(blend, alpha, transform_ref, clip_ref);
        self.open_layers[self.depth] = layer;
        self.depth += 1;
        Ok(())
    }

    /// Closes the innermost open layer.
    pub fn pop_layer(&mut self) -> Result<(), EncodeError> {
        if self.depth == 0 {
            return Err(EncodeError::UnbalancedLayer);
        }
        self.depth -= 1;
        self.scene.encode_pop_layer(self.open_layers[self.depth]);
        Ok(())
    }

    // ------------------------------------------------------------ nodes

    /// Opens a reusable subtree with a caller-supplied stable identity
    /// (Doc 03 §3).
    ///
    /// The returned scope *is* the builder for the length of the subtree — it
    /// derefs to one — and closes the node when it drops. That is what makes
    /// an unbalanced node unrepresentable rather than merely discouraged.
    ///
    /// ```
    /// # use otf_2d_engine_geom::{Affine, PathBuilder, Rect};
    /// # use otf_2d_engine_color::Color;
    /// # use otf_2d_engine_scene::{FillRule, NodeId, Paint, Scene, SceneBuilder};
    /// # let mut scene = Scene::new();
    /// # let mut sb = SceneBuilder::new(&mut scene);
    /// # let path = PathBuilder::new().rect(Rect::new(0.0, 0.0, 1.0, 1.0)).build();
    /// if !sb.reuse_node(NodeId(7), Affine::IDENTITY) {
    ///     let mut panel = sb.push_node(NodeId(7));
    ///     panel.fill(FillRule::NonZero, Affine::IDENTITY, &Paint::hex(0xff0000ff), &path)?;
    /// }
    /// # Ok::<(), otf_2d_engine_scene::EncodeError>(())
    /// ```
    pub fn push_node(&mut self, id: NodeId) -> NodeScope<'_, 'a> {
        let parent = self.current_node;
        let node = self.scene.encode_push_node(id, parent);
        self.current_node = node;
        NodeScope {
            builder: self,
            node,
            parent,
        }
    }

    /// Reuses a previously encoded subtree instead of re-encoding it.
    ///
    /// Always false until the node cache lands in M6 (Doc 03 §3, §9): the IR
    /// carries the identities and extents from T1.3, but nothing stores the
    /// encoded bytes to copy back yet. Consumers can write the cache-aware
    /// shape now and get the win when the cache arrives.
    pub fn reuse_node(&mut self, _id: NodeId, _transform: Affine) -> bool {
        false
    }

    // ------------------------------------------------------------ checks

    fn check_path(&self, path: &Path) -> Result<(), EncodeError> {
        if !path.is_finite() {
            return Err(EncodeError::NonFiniteCoordinate);
        }
        let points = path.points().len();
        if points > MAX_PATH_POINTS {
            return Err(EncodeError::PathTooLarge {
                limit: MAX_PATH_POINTS,
            });
        }
        check_room(self.scene.path_data().len(), points * 2)?;
        check_room(self.scene.path_verbs().len(), path.verbs().len())
    }
}

impl Drop for SceneBuilder<'_> {
    /// Closes anything left open, so a `Scene` is balanced even if the
    /// consumer unwound past its `pop_layer` calls. An unbalanced layer stack
    /// reaching stage 6 would be a panic or a leaked scratch buffer; neither
    /// is acceptable under I-8.
    fn drop(&mut self) {
        self.close_open_layers();
    }
}

/// True when the arena can take `extra` more elements without exceeding the
/// `u32` range every handle is expressed in.
fn check_room(current: usize, extra: usize) -> Result<(), EncodeError> {
    let limit = u32::MAX as usize;
    if current.saturating_add(extra) > limit {
        return Err(EncodeError::PathTooLarge {
            limit: limit - current.min(limit),
        });
    }
    Ok(())
}

fn check_transform(affine: Affine) -> Result<(), EncodeError> {
    if affine.is_finite() {
        Ok(())
    } else {
        Err(EncodeError::NonFiniteCoordinate)
    }
}

fn color_is_finite(color: Color) -> bool {
    color.r.is_finite() && color.g.is_finite() && color.b.is_finite() && color.a.is_finite()
}

fn check_paint(paint: &Paint) -> Result<(), EncodeError> {
    let ok = match *paint {
        Paint::Solid(color) => color_is_finite(color),
        Paint::LinearGradient { start, end, .. } => start.is_finite() && end.is_finite(),
        Paint::RadialGradient {
            center,
            radius,
            focal,
            ..
        } => center.is_finite() && radius.is_finite() && focal.is_none_or(|f| f.is_finite()),
        Paint::Image { .. } => true,
    };
    if ok {
        Ok(())
    } else {
        Err(EncodeError::NonFiniteCoordinate)
    }
}

fn check_stroke(style: &StrokeStyle) -> Result<(), EncodeError> {
    let joins = match style.join {
        Join::Miter { limit } => limit.is_finite(),
        Join::Round | Join::Bevel => true,
    };
    let dashes = match &style.dash {
        Some(Dash { pattern, offset }) => {
            offset.is_finite() && pattern.iter().all(|d| d.is_finite())
        }
        None => true,
    };
    if style.width.is_finite() && joins && dashes {
        Ok(())
    } else {
        Err(EncodeError::NonFiniteCoordinate)
    }
}

fn check_glyph_run(size: f32, glyphs: &[Glyph], options: &GlyphOptions) -> Result<(), EncodeError> {
    if glyphs.is_empty() || glyphs.len() > MAX_GLYPHS_PER_RUN {
        return Err(EncodeError::InvalidGlyphRun);
    }
    if !size.is_finite() || size <= 0.0 {
        return Err(EncodeError::InvalidGlyphRun);
    }
    if !options.synthetic_bold.is_finite() || !options.synthetic_skew.is_finite() {
        return Err(EncodeError::InvalidGlyphRun);
    }
    if glyphs.iter().any(|g| !g.x.is_finite() || !g.y.is_finite()) {
        return Err(EncodeError::InvalidGlyphRun);
    }
    Ok(())
}

/// An open node. Derefs to the builder that created it, and closes the node
/// when dropped (Doc 03 §3).
#[must_use = "a node closes when its scope drops; dropping it immediately encodes an empty node"]
pub struct NodeScope<'b, 'a> {
    builder: &'b mut SceneBuilder<'a>,
    node: u32,
    parent: u32,
}

impl NodeScope<'_, '_> {
    /// This node's index in the arena.
    #[inline]
    pub fn node(&self) -> u32 {
        self.node
    }
}

impl<'a> Deref for NodeScope<'_, 'a> {
    type Target = SceneBuilder<'a>;

    fn deref(&self) -> &SceneBuilder<'a> {
        self.builder
    }
}

impl<'a> DerefMut for NodeScope<'_, 'a> {
    fn deref_mut(&mut self) -> &mut SceneBuilder<'a> {
        self.builder
    }
}

impl Drop for NodeScope<'_, '_> {
    /// Records the node's extent. The content hash stays
    /// [`NodeHash::UNSET`] until M6: writing a hash the node cache does not
    /// yet define would be a value nothing validates, and a wrong hash in a
    /// cache is a correctness bug rather than a slow path.
    fn drop(&mut self) {
        self.builder
            .scene
            .encode_pop_node(self.node, NodeHash::UNSET);
        self.builder.current_node = self.parent;
    }
}
