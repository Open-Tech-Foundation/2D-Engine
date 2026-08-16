//! Colour spaces, conversion and blend math for 2D-Engine.
//!
//! See `docs/01-architecture.md` §7. The internal model is linear-light,
//! premultiplied `f32`, and `u8` sRGB is a fast path at the edges — never the
//! model. Colour space is data carried on each colour, so there is no global
//! colour context to get out of sync.
//!
//! The crate is `no_std`-capable. Enable the `libm` feature in place of `std`
//! on targets without a float math library.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod blend;
mod color;
mod math;
mod space;
mod srgb;

pub use blend::{BlendMode, apply_coverage, src_over, src_over_premul};
pub use color::Color;
pub use space::{ColorSpace, apply3, invert3};
pub use srgb::{
    SRGB8_TO_LINEAR, alpha8_to_f32, f32_to_alpha8, linear_to_srgb, linear_to_srgb8, srgb_to_linear,
    srgb8_to_linear,
};
