//! Which stage 6 implementation runs.
//!
//! Selected at runtime, per render, never at compile time (Doc 01 §4): one
//! binary has to run correctly on a machine older than the one that built it.

/// A stage 6 implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Simd {
    /// The reference implementation. Always available, on every target.
    #[default]
    Scalar,
    /// AVX2, eight pixels at a time. x86-64 only (Q-02).
    Avx2,
}

impl Simd {
    /// The fastest implementation this CPU supports.
    pub fn detect() -> Simd {
        if Simd::Avx2.is_available() {
            Simd::Avx2
        } else {
            Simd::Scalar
        }
    }

    /// Whether this CPU can run the implementation.
    pub fn is_available(self) -> bool {
        match self {
            Simd::Scalar => true,
            Simd::Avx2 => avx2_available(),
        }
    }

    /// This implementation if the CPU supports it, scalar otherwise.
    ///
    /// Every entry point funnels through this, so asking for a path the
    /// machine cannot run is a slowdown rather than an illegal instruction.
    pub fn resolve(self) -> Simd {
        if self.is_available() {
            self
        } else {
            Simd::Scalar
        }
    }
}

#[cfg(all(feature = "std", target_arch = "x86_64"))]
fn avx2_available() -> bool {
    std::arch::is_x86_feature_detected!("avx2")
}

/// Feature detection needs `std`. Without it there is no way to ask the CPU
/// what it supports, so the portable path is the only safe answer.
#[cfg(not(all(feature = "std", target_arch = "x86_64")))]
fn avx2_available() -> bool {
    false
}
