//! Rasterization stages 3–6 for 2D-Engine: flatten, bin, strips, fine raster.
//!
//! See `docs/01-architecture.md` §4. This crate never depends on
//! `otf-2d-engine-cache`: caching wraps rasterization, it is not woven through
//! it (`docs/02-scene-ir-and-api.md` §1).
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod binning;
mod segment;

pub use binning::{BinStats, Binner, SurfaceSize, TileBins, TileEntry, TileGeometry};
pub use segment::Segment;
