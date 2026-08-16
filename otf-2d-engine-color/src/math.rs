//! Float math that works with and without `std`.
//!
//! `otf-2d-engine-color` is `no_std`-capable (Doc 02 §1), and the float
//! methods live in `std`. These wrappers pick `std` or `libm`.

#[cfg(all(not(feature = "std"), not(feature = "libm")))]
compile_error!(
    "otf-2d-engine-color needs float math: enable the `std` feature, or `libm` for no_std targets"
);

#[inline]
pub fn powf(x: f32, y: f32) -> f32 {
    #[cfg(feature = "std")]
    {
        f32::powf(x, y)
    }
    #[cfg(not(feature = "std"))]
    {
        libm::powf(x, y)
    }
}

#[inline]
#[allow(unused)]
pub fn abs(x: f32) -> f32 {
    #[cfg(feature = "std")]
    {
        f32::abs(x)
    }
    #[cfg(not(feature = "std"))]
    {
        libm::fabsf(x)
    }
}
