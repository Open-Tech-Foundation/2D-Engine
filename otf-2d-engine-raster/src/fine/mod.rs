//! Stage 6 — fine rasterization (Doc 01 §4).
//!
//! Strips in, pixels out. This is the hot loop, and it is the one place where
//! a scalar reference and a SIMD implementation must agree *bit for bit*
//! (I-5) — not "within tolerance", which would make every later optimisation
//! unverifiable.
//!
//! # How bit-identity is guaranteed rather than argued
//!
//! The blend is entirely **integer**: fixed-point linear levels, multiplies,
//! adds and shifts, with every transfer function a table lookup. There is no
//! floating-point rounding to diverge, no reciprocal approximation, no fused
//! multiply-add for a compiler to contract differently on one path. The SIMD
//! kernel performs the same operations in the same order on eight pixels at
//! once, and reads the same tables. Bit-identity is structural.
//!
//! # Why the blend is in linear light
//!
//! Doc 01 §7 makes linear-light premultiplied `f32` the model and the `u8`
//! path an optimisation that "must produce results within tolerance of the f32
//! path". Compositing sRGB bytes directly is not within tolerance of that — it
//! differs by tens of codes on any antialiased edge — so the u8 path decodes,
//! blends in linear, and re-encodes (D-31). What the fast path saves is
//! storage, bandwidth and float arithmetic, not the transfer function.

mod dispatch;
mod tables;

#[cfg(all(feature = "std", target_arch = "x86_64"))]
mod avx2;

use otf_2d_engine_color::{Color, ColorSpace};

pub use dispatch::Simd;
pub use tables::{FineTables, LINEAR_LEVELS, LINEAR_SCALE, LINEAR_SHIFT};

use crate::pixels::{PixelFormat, TargetMut, quantize};
use crate::strips::{StripKind, Strips};

/// Rounding constant for the fixed-point shift.
pub(crate) const LINEAR_HALF: u32 = LINEAR_SCALE / 2;

/// A solid paint, prepared for one target format.
///
/// Channels are stored in the target's byte order, as premultiplied linear
/// levels, so the inner loop never branches on format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolidPaint {
    /// Premultiplied linear levels, indexed by target byte position.
    pub(crate) levels: [u32; 4],
    /// The colour encoded for the target, for the full-coverage opaque case.
    pub(crate) bytes: [u8; 4],
    opaque: bool,
}

impl SolidPaint {
    /// Prepares a colour for a target.
    ///
    /// The colour is converted to sRGB first: the u8 pipeline is the sRGB fast
    /// path, and a wide-gamut colour has to be brought into it before it can
    /// be written as sRGB bytes.
    pub fn new(color: Color, format: PixelFormat, tables: &FineTables) -> SolidPaint {
        let premul = color.convert_to(ColorSpace::Srgb).to_premul();
        let mut levels = [0u32; 4];
        for (channel, &slot) in format.channel_order().iter().enumerate() {
            levels[slot] = quantize(premul[channel]);
        }
        levels[3] = quantize(premul[3]);
        let mut bytes = [0u8; 4];
        for slot in 0..3 {
            bytes[slot] = tables.encode(levels[slot]);
        }
        bytes[3] = tables.encode_alpha(levels[3]);
        SolidPaint {
            levels,
            bytes,
            opaque: levels[3] >= LINEAR_SCALE,
        }
    }

    /// True when the paint covers what is beneath it completely.
    #[inline]
    pub fn is_opaque(&self) -> bool {
        self.opaque
    }

    /// The colour as target bytes.
    #[inline]
    pub fn bytes(&self) -> [u8; 4] {
        self.bytes
    }
}

/// What stage 6 did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FineStats {
    /// Pixels written by the full-coverage store path.
    pub pixels_stored: usize,
    /// Pixels written by the blend path.
    pub pixels_blended: usize,
    /// Which implementation ran.
    pub simd: Simd,
}

/// Composites a solid paint through `strips` into `target`.
///
/// `simd` selects the implementation; [`Simd::detect`] picks the best the CPU
/// supports, and any choice the CPU cannot run falls back to scalar rather
/// than faulting.
pub fn render_solid(
    target: &mut TargetMut<'_>,
    strips: &Strips<'_>,
    color: Color,
    tables: &FineTables,
    simd: Simd,
) -> FineStats {
    let paint = SolidPaint::new(color, target.format(), tables);
    render_solid_paint(target, strips, &paint, tables, simd)
}

/// The same, with the paint already prepared.
pub fn render_solid_paint(
    target: &mut TargetMut<'_>,
    strips: &Strips<'_>,
    paint: &SolidPaint,
    tables: &FineTables,
    simd: Simd,
) -> FineStats {
    let simd = simd.resolve();
    let mut stats = FineStats {
        simd,
        ..FineStats::default()
    };
    let band_height = strips.geometry().height as u32;

    for strip in strips.strips() {
        let alphas = strips.strip_alphas(strip);
        let top = strip.band * band_height;
        let width = strip.width.min(target.width().saturating_sub(strip.x));
        if width == 0 {
            continue;
        }
        for row in 0..strip.rows as u32 {
            let y = top + row;
            let Some(pixels) = target.row_mut(y) else {
                continue;
            };
            let start = strip.x as usize * 4;
            let end = start + width as usize * 4;
            let Some(span) = pixels.get_mut(start..end) else {
                continue;
            };

            match strip.kind {
                StripKind::Uniform { .. } => {
                    let coverage = alphas.get(row as usize).copied().unwrap_or(0);
                    if coverage == 0 {
                        continue;
                    }
                    if coverage == 255 && paint.opaque {
                        fill(span, paint.bytes, simd);
                        stats.pixels_stored += width as usize;
                    } else {
                        blend_uniform(span, paint, tables, coverage, simd);
                        stats.pixels_blended += width as usize;
                    }
                }
                StripKind::Alpha { .. } => {
                    let row_start = row as usize * strip.width as usize;
                    let row_end = row_start + width as usize;
                    let Some(coverage) = alphas.get(row_start..row_end) else {
                        continue;
                    };
                    blend_coverage(span, paint, tables, coverage, simd);
                    stats.pixels_blended += width as usize;
                }
            }
        }
    }
    stats
}

// ------------------------------------------------------------ kernels

#[allow(
    unsafe_code,
    reason = "dispatches to the AVX2 kernel after runtime detection"
)]
fn fill(span: &mut [u8], bytes: [u8; 4], simd: Simd) {
    #[cfg(all(feature = "std", target_arch = "x86_64"))]
    if simd == Simd::Avx2 {
        // SAFETY: `resolve` only yields `Avx2` when the CPU reports it.
        unsafe { avx2::fill(span, bytes) };
        return;
    }
    let _ = simd;
    for pixel in span.chunks_exact_mut(4) {
        pixel.copy_from_slice(&bytes);
    }
}

#[allow(
    unsafe_code,
    reason = "dispatches to the AVX2 kernel after runtime detection"
)]
fn blend_uniform(
    span: &mut [u8],
    paint: &SolidPaint,
    tables: &FineTables,
    coverage: u8,
    simd: Simd,
) {
    #[cfg(all(feature = "std", target_arch = "x86_64"))]
    if simd == Simd::Avx2 {
        // SAFETY: `resolve` only yields `Avx2` when the CPU reports it.
        unsafe { avx2::blend_uniform(span, paint, tables, coverage) };
        return;
    }
    let _ = simd;
    let cov = tables.coverage(coverage);
    for pixel in span.chunks_exact_mut(4) {
        blend_pixel(pixel, paint, tables, cov);
    }
}

#[allow(
    unsafe_code,
    reason = "dispatches to the AVX2 kernel after runtime detection"
)]
fn blend_coverage(
    span: &mut [u8],
    paint: &SolidPaint,
    tables: &FineTables,
    coverage: &[u8],
    simd: Simd,
) {
    #[cfg(all(feature = "std", target_arch = "x86_64"))]
    if simd == Simd::Avx2 {
        // SAFETY: `resolve` only yields `Avx2` when the CPU reports it.
        unsafe { avx2::blend_coverage(span, paint, tables, coverage) };
        return;
    }
    let _ = simd;
    for (pixel, &alpha) in span.chunks_exact_mut(4).zip(coverage) {
        if alpha == 0 {
            continue;
        }
        blend_pixel(pixel, paint, tables, tables.coverage(alpha));
    }
}

/// One pixel of source-over, in fixed-point linear light.
///
/// This is the reference the SIMD kernel must reproduce exactly. Every
/// operation is integer, so there is nothing for the two paths to round
/// differently.
#[inline]
pub(crate) fn blend_pixel(pixel: &mut [u8], paint: &SolidPaint, tables: &FineTables, cov: u32) {
    let alpha = scale(paint.levels[3], cov);
    let inverse = LINEAR_SCALE - alpha;

    for (slot, byte) in pixel.iter_mut().enumerate().take(3) {
        let source = scale(paint.levels[slot], cov);
        let destination = tables.decode(*byte);
        let blended = source + scale(destination, inverse);
        *byte = tables.encode(blended.min(LINEAR_SCALE));
    }
    let destination = tables.decode_alpha(pixel[3]);
    let blended = alpha + scale(destination, inverse);
    pixel[3] = tables.encode_alpha(blended.min(LINEAR_SCALE));
}

/// `value * factor` in fixed point, rounded to nearest.
#[inline]
pub(crate) fn scale(value: u32, factor: u32) -> u32 {
    (value * factor + LINEAR_HALF) >> LINEAR_SHIFT
}
