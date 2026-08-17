//! Stage 6, AVX2: eight pixels per instruction.
//!
//! Every operation here mirrors [`super::blend_pixel`] exactly — the same
//! integer multiplies, the same rounding constant, the same shift, the same
//! tables — so the output is bit-identical to the scalar path by construction
//! rather than by measurement (I-5). Nothing here is approximate: no
//! reciprocal estimate, no fused multiply-add, no float at all.
#![allow(
    unsafe_code,
    reason = "SIMD intrinsics are unsafe by definition; every call site is guarded by runtime feature detection"
)]

use core::arch::x86_64::*;

use super::tables::{FineTables, LINEAR_SCALE, LINEAR_SHIFT};
use super::{LINEAR_HALF, SolidPaint, blend_pixel, scale};

/// Pixels per vector.
const LANES: usize = 8;

/// Stores a constant pixel across the span.
///
/// # Safety
/// The caller must have verified AVX2 support.
#[target_feature(enable = "avx2")]
pub unsafe fn fill(span: &mut [u8], bytes: [u8; 4]) {
    unsafe {
        let pixel = i32::from_ne_bytes(bytes);
        let wide = _mm256_set1_epi32(pixel);
        let mut chunks = span.chunks_exact_mut(LANES * 4);
        for chunk in &mut chunks {
            _mm256_storeu_si256(chunk.as_mut_ptr().cast(), wide);
        }
        for pixel in chunks.into_remainder().chunks_exact_mut(4) {
            pixel.copy_from_slice(&bytes);
        }
    }
}

/// Blends a constant coverage across the span.
///
/// # Safety
/// The caller must have verified AVX2 support.
#[target_feature(enable = "avx2")]
pub unsafe fn blend_uniform(
    span: &mut [u8],
    paint: &SolidPaint,
    tables: &FineTables,
    coverage: u8,
) {
    unsafe {
        let cov = tables.coverage(coverage);
        let alpha = scale(paint.levels[3], cov);
        let inverse = LINEAR_SCALE - alpha;
        let sources = [
            scale(paint.levels[0], cov),
            scale(paint.levels[1], cov),
            scale(paint.levels[2], cov),
            alpha,
        ];

        let source_vectors = [
            _mm256_set1_epi32(sources[0] as i32),
            _mm256_set1_epi32(sources[1] as i32),
            _mm256_set1_epi32(sources[2] as i32),
            _mm256_set1_epi32(sources[3] as i32),
        ];
        let inverse_vector = _mm256_set1_epi32(inverse as i32);

        let mut chunks = span.chunks_exact_mut(LANES * 4);
        for chunk in &mut chunks {
            blend_chunk(chunk, tables, source_vectors, inverse_vector);
        }
        for pixel in chunks.into_remainder().chunks_exact_mut(4) {
            blend_pixel(pixel, paint, tables, cov);
        }
    }
}

/// Blends per-pixel coverage across the span.
///
/// # Safety
/// The caller must have verified AVX2 support.
#[target_feature(enable = "avx2")]
pub unsafe fn blend_coverage(
    span: &mut [u8],
    paint: &SolidPaint,
    tables: &FineTables,
    coverage: &[u8],
) {
    unsafe {
        let (_, alpha_table, _) = tables.raw();
        let paint_vectors = [
            _mm256_set1_epi32(paint.levels[0] as i32),
            _mm256_set1_epi32(paint.levels[1] as i32),
            _mm256_set1_epi32(paint.levels[2] as i32),
            _mm256_set1_epi32(paint.levels[3] as i32),
        ];
        let scale_vector = _mm256_set1_epi32(LINEAR_SCALE as i32);

        let pixels = span.len() / 4;
        let vectors = pixels / LANES;
        for vector in 0..vectors {
            let offset = vector * LANES;
            let alphas = &coverage[offset..offset + LANES];
            // Coverage bytes to linear levels.
            let bytes = _mm_loadl_epi64(alphas.as_ptr().cast());
            let indices = _mm256_cvtepu8_epi32(bytes);
            let cov = gather(alpha_table.as_ptr(), indices);

            let source_alpha = scale_vector_lanes(paint_vectors[3], cov);
            let inverse = _mm256_sub_epi32(scale_vector, source_alpha);
            let sources = [
                scale_vector_lanes(paint_vectors[0], cov),
                scale_vector_lanes(paint_vectors[1], cov),
                scale_vector_lanes(paint_vectors[2], cov),
                source_alpha,
            ];
            let chunk = &mut span[offset * 4..(offset + LANES) * 4];
            blend_chunk(chunk, tables, sources, inverse);
        }

        for index in vectors * LANES..pixels {
            let alpha = coverage[index];
            if alpha == 0 {
                continue;
            }
            let pixel = &mut span[index * 4..index * 4 + 4];
            blend_pixel(pixel, paint, tables, tables.coverage(alpha));
        }
    }
}

/// `value * factor` in fixed point, rounded to nearest — the vector form of
/// [`super::scale`], operation for operation.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn scale_vector_lanes(value: __m256i, factor: __m256i) -> __m256i {
    let product = _mm256_mullo_epi32(value, factor);
    let rounded = _mm256_add_epi32(product, _mm256_set1_epi32(LINEAR_HALF as i32));
    _mm256_srli_epi32(rounded, LINEAR_SHIFT as i32)
}

/// Reads eight table entries.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn gather(table: *const u32, indices: __m256i) -> __m256i {
    // SAFETY: indices are masked to the table's length by the caller.
    unsafe { _mm256_i32gather_epi32(table.cast::<i32>(), indices, 4) }
}

/// Looks up eight packed encode-table bytes.
///
/// The table stores four bytes per word, so the index selects a word and the
/// low two bits select the byte within it. One gather and two shifts, against
/// a table a quarter the size — which is what keeps it in L1.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn encode_levels(table: *const u32, levels: __m256i) -> __m256i {
    unsafe {
        let words = gather(table, _mm256_srli_epi32(levels, 2));
        let shift = _mm256_slli_epi32(_mm256_and_si256(levels, _mm256_set1_epi32(3)), 3);
        _mm256_and_si256(_mm256_srlv_epi32(words, shift), _mm256_set1_epi32(0xff))
    }
}

/// Extracts one byte lane from each pixel. The shift must be a constant, hence
/// the const parameter rather than a loop.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn extract<const SHIFT: i32>(destination: __m256i, mask: __m256i) -> __m256i {
    _mm256_and_si256(_mm256_srli_epi32(destination, SHIFT), mask)
}

/// Puts a byte lane back.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn place<const SHIFT: i32>(value: __m256i) -> __m256i {
    _mm256_slli_epi32(value, SHIFT)
}

/// Blends eight pixels with per-channel source levels already scaled by
/// coverage.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn blend_chunk(
    chunk: &mut [u8],
    tables: &FineTables,
    sources: [__m256i; 4],
    inverse: __m256i,
) {
    unsafe {
        let (decode, alpha_table, encode) = tables.raw();
        let byte_mask = _mm256_set1_epi32(0xff);
        let ceiling = _mm256_set1_epi32(LINEAR_SCALE as i32);
        let destination = _mm256_loadu_si256(chunk.as_ptr().cast());

        // One colour channel: decode, blend, clamp, encode. Identical to the
        // scalar loop body, operation for operation.
        macro_rules! colour {
            ($shift:literal, $slot:expr) => {{
                let indices = extract::<$shift>(destination, byte_mask);
                let current = gather(decode.as_ptr(), indices);
                let blended =
                    _mm256_add_epi32(sources[$slot], scale_vector_lanes(current, inverse));
                let clamped = _mm256_min_epi32(blended, ceiling);
                place::<$shift>(encode_levels(encode.as_ptr(), clamped))
            }};
        }

        let red = colour!(0, 0);
        let green = colour!(8, 1);
        let blue = colour!(16, 2);

        // Alpha carries no transfer function: the same rounded scale by 255
        // the scalar path applies.
        let indices = extract::<24>(destination, byte_mask);
        let current = gather(alpha_table.as_ptr(), indices);
        let blended = _mm256_add_epi32(sources[3], scale_vector_lanes(current, inverse));
        let clamped = _mm256_min_epi32(blended, ceiling);
        let product = _mm256_mullo_epi32(clamped, _mm256_set1_epi32(255));
        let rounded = _mm256_add_epi32(product, _mm256_set1_epi32(LINEAR_HALF as i32));
        let alpha = place::<24>(_mm256_srli_epi32(rounded, LINEAR_SHIFT as i32));

        let result = _mm256_or_si256(_mm256_or_si256(red, green), _mm256_or_si256(blue, alpha));
        _mm256_storeu_si256(chunk.as_mut_ptr().cast(), result);
    }
}
