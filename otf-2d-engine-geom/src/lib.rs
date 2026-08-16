//! Geometry primitives for 2D-Engine.
//!
//! See `docs/02-scene-ir-and-api.md` §3. Every type in this crate is public API
//! and therefore a stability commitment: coordinates are `f64` at the surface,
//! `f32` only appears after transform resolution in stage 2.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
