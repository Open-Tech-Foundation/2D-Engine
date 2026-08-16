//! Geometry primitives for 2D-Engine.
//!
//! See `docs/02-scene-ir-and-api.md` §3. Every type in this crate is public
//! API and therefore a stability commitment: coordinates are `f64` at the
//! surface, and `f32` only appears after transform resolution in stage 2,
//! once coordinates are device-local and bounded.
//!
//! The crate is `no_std`-capable. Enable the `libm` feature in place of `std`
//! on targets without a float math library.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod affine;
mod math;
mod path;
mod point;
mod rect;

pub use affine::Affine;
pub use path::{Path, PathBuilder, PathEl, PathSeg, PathShape, PathVerb, Segments};
pub use point::{Point, Size, Vec2};
pub use rect::{Rect, RectRadii};
