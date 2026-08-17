//! CPU raster backend for 2D-Engine.
//!
//! Assembles the stages in `otf-2d-engine-raster` behind the `RasterBackend`
//! seam described in `docs/01-architecture.md` §6.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod pixmap;
mod render;

pub use otf_2d_engine_raster::{FineTables, PixelFormat, Simd, TargetError, TargetMut, ThreadPool};
pub use pixmap::Pixmap;
pub use render::{CpuRenderer, Pipeline, RenderError, RenderParams, RenderStats};
