//! Rasterization stages 3–6 for 2D-Engine: flatten, bin, strips, fine raster.
//!
//! See `docs/01-architecture.md` §4. This crate never depends on
//! `otf-2d-engine-cache`: caching wraps rasterization, it is not woven through
//! it (`docs/02-scene-ir-and-api.md` §1).
#![cfg_attr(not(feature = "std"), no_std)]
// `deny` rather than `forbid`: the AVX2 kernel in `fine::avx2` cannot be
// written without intrinsics, and `forbid` cannot be lifted per module. Every
// other module in this crate is still held to it.
#![deny(unsafe_code)]

extern crate alloc;

mod binning;
mod euler;
mod fine;
mod flatten;
mod math;
mod pixels;
mod segment;
mod strips;
mod stroke;
mod threads;

pub use binning::{BinStats, Binner, SurfaceSize, TileBins, TileEntry, TileGeometry};
pub use fine::{
    FineStats, FineTables, LINEAR_LEVELS, LINEAR_SCALE, LINEAR_SHIFT, Simd, SolidPaint,
    render_solid, render_solid_paint,
};
pub use flatten::{DEFAULT_TOLERANCE, Flattener, clip_segment, clip_segments};
pub use pixels::{PixelFormat, TargetError, TargetMut, encode_color};
pub use segment::Segment;
pub use strips::{Strip, StripKind, StripStats, Striper, Strips};
pub use stroke::StrokeSpec;
pub use threads::{ChunkTask, SerialPool, ThreadPool};
