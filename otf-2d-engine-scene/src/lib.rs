//! Immutable scene IR and encoder for 2D-Engine.
//!
//! See `docs/02-scene-ir-and-api.md` §2 and §5. The scene is a set of parallel
//! arena buffers referenced by `u32` handles; it has no interior mutability and
//! is `Send + Sync` (invariants I-1 and I-2 in `AGENTS.md`).
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;
