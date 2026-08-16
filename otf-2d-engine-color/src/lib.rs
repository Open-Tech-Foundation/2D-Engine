//! Colour spaces, conversion and blend math for 2D-Engine.
//!
//! See `docs/01-architecture.md` §7. The internal model is linear-light,
//! premultiplied `f32`; `u8` sRGB is a fast path, never the model.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
