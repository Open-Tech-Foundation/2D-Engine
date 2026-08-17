//! Test harnesses for 2D-Engine.
//!
//! This crate is `publish = false` and appears only as a dev-dependency. It is
//! outside the closed v1 dependency list in Doc 02 §1, which governs what the
//! shipped crates link against.
#![deny(unsafe_code)]
// `alloc` implements `GlobalAlloc`, which is unsafe by definition. Scoped to
// that one module; every other module here is still `deny`.

pub mod alloc;
pub mod bench;
pub mod golden;
pub mod image;

/// Creates a unique scratch directory under `target/` and removes any previous
/// contents. Used by harness self-tests that need a private reference dir.
pub fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../target/test-scratch")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}
