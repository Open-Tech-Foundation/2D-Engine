//! Float math that works with and without `std`.
//!
//! The same shim `otf-2d-engine-geom` carries, for the same reason: `f64::sqrt`
//! lives in `std`, and this crate builds `no_std` (Doc 02 §1). Only the two
//! functions the flattener needs are here — a shim grows a maintenance cost
//! per entry, and there is no point paying it for functions nobody calls.

#[cfg(all(not(feature = "std"), not(feature = "libm")))]
compile_error!(
    "otf-2d-engine-raster needs float math: enable the `std` feature, or `libm` for no_std targets"
);

#[inline]
pub fn sqrt(x: f64) -> f64 {
    #[cfg(feature = "std")]
    {
        f64::sqrt(x)
    }
    #[cfg(not(feature = "std"))]
    {
        libm::sqrt(x)
    }
}

#[inline]
pub fn ceil(x: f64) -> f64 {
    #[cfg(feature = "std")]
    {
        f64::ceil(x)
    }
    #[cfg(not(feature = "std"))]
    {
        libm::ceil(x)
    }
}
