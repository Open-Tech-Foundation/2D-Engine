//! 2D-Engine — a 2D vector graphics engine with immutable scenes, rendering to
//! raster or vector targets.
//!
//! This crate is a facade: it re-exports the common path from the workspace
//! crates so consumers depend on one name.
#![forbid(unsafe_code)]

pub use otf_2d_engine_cache as cache;
pub use otf_2d_engine_color as color;
pub use otf_2d_engine_cpu as cpu;
pub use otf_2d_engine_geom as geom;
pub use otf_2d_engine_raster as raster;
pub use otf_2d_engine_scene as scene;
