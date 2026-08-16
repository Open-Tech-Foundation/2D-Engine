//! Float math that works with and without `std`.
//!
//! `otf-2d-engine-geom` is `no_std`-capable (Doc 02 §1), and `f64::sqrt` and
//! friends live in `std`. These thin wrappers pick `std` or `libm` so callers
//! never write a `cfg`. `libm` spells some of them differently, hence the
//! per-function name.

#[cfg(all(not(feature = "std"), not(feature = "libm")))]
compile_error!(
    "otf-2d-engine-geom needs float math: enable the `std` feature, or `libm` for no_std targets"
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
    abs => fabs;
    sin => sin;
    cos => cos;
    tan => tan;
    floor => floor;
    ceil => ceil;
    round => round;
}

binary! {
    atan2 => atan2;
    hypot => hypot;
}
