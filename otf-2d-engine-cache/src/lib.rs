//! Damage tracking, tile cache and glyph cache for 2D-Engine.
//!
//! See `docs/03-incrementality.md`. Every cache here is disableable and must
//! produce byte-identical output when bypassed (invariant I-6).
#![forbid(unsafe_code)]
