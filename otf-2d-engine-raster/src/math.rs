//! Float math that works with and without `std`.
//!
//! The same shim `otf-2d-engine-geom` carries, for the same reason: `f64::sqrt`
//! and friends live in `std`, and this crate builds `no_std` (Doc 02 §1). Only
//! the functions the flattener needs are here — a shim grows a maintenance cost
//! per entry, and there is no point paying it for functions nobody calls.

#[cfg(all(not(feature = "std"), not(feature = "libm")))]
compile_error!(
    "otf-2d-engine-raster needs float math: enable the `std` feature, or `libm` for no_std targets"
);

/// `unary!(name => libm_name)` and `binary!(name => libm_name)`.
macro_rules! unary {
    ($($name:ident => $libm:ident;)*) => {
        $(
            #[inline]
            #[allow(unused)]
            pub fn $name(x: f64) -> f64 {
                #[cfg(feature = "std")]
                { f64::$name(x) }
                #[cfg(not(feature = "std"))]
                { libm::$libm(x) }
            }
        )*
    };
}

macro_rules! binary {
    ($($name:ident => $libm:ident;)*) => {
        $(
            #[inline]
            #[allow(unused)]
            pub fn $name(a: f64, b: f64) -> f64 {
                #[cfg(feature = "std")]
                { f64::$name(a, b) }
                #[cfg(not(feature = "std"))]
                { libm::$libm(a, b) }
            }
        )*
    };
}

unary! {
    sqrt => sqrt;
    cbrt => cbrt;
    abs => fabs;
    sin => sin;
    cos => cos;
    ceil => ceil;
}

binary! {
    atan2 => atan2;
    copysign => copysign;
}
