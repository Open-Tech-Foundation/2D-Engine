//! Immutable scene IR and encoder for 2D-Engine.
//!
//! See `docs/02-scene-ir-and-api.md` §2 and §5. The scene is a set of parallel
//! arena buffers referenced by `u32` handles; it has no interior mutability and
//! is `Send + Sync` (invariants I-1 and I-2 in `AGENTS.md`).
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod builder;
mod handles;
mod records;
mod scene;
mod serialize;
mod style;
mod unit;

pub use builder::{
    EncodeError, MAX_GLYPHS_PER_RUN, MAX_LAYER_DEPTH, MAX_PATH_POINTS, NodeScope, SceneBuilder,
};
pub use handles::{
    FontRef, GlyphRunRef, ImageRef, LayerRef, NO_REF, NodeHash, NodeId, PaintRef, PathRef,
    StopsRef, StrokeRef, TransformRef, VariationsRef,
};
pub use records::{
    ColorStopRec, DrawKind, DrawTag, FLAG_EVEN_ODD, GlyphRec, GlyphRunDesc, JOIN_BEVEL, JOIN_MITER,
    JOIN_ROUND, LayerDesc, NodeDesc, PAINT_FLAG_HAS_FOCAL, PaintDesc, PaintKind, PathDesc, RunRec,
    ShapeKind, StrokeDesc, TransformRec,
};
pub use scene::{PathView, Scene, SceneMemory, color_from_record, color_space_from_u32};
pub use serialize::SceneDecodeError;
pub use style::{
    Cap, ColorStop, Dash, Extend, FillRule, Glyph, GlyphOptions, Hinting, Join, Paint, Sampling,
    StrokeStyle,
};
pub use unit::SceneUnit;
