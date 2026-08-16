//! Compositing.
//!
//! v1 supports `src-over` and group opacity (Doc 01 §4, stage 7). The full
//! Porter-Duff set and the separable blend modes land in M6; [`BlendMode`] is
//! `#[non_exhaustive]` so they can be added without a breaking change, and
//! nothing here accepts a mode it cannot execute.

use crate::color::Color;

/// How a source is combined with its destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum BlendMode {
    /// Source over destination — the ordinary painter's model.
    #[default]
    SrcOver,
}

/// Composites premultiplied `src` over premultiplied `dst`.
///
/// Both must be in the same colour space; converting per pixel in the fine
/// loop is not affordable, so stage 2 normalises paints to the target space
/// instead.
#[inline]
pub fn src_over_premul(src: [f32; 4], dst: [f32; 4]) -> [f32; 4] {
    let inv_a = 1.0 - src[3];
    [
        src[0] + dst[0] * inv_a,
        src[1] + dst[1] * inv_a,
        src[2] + dst[2] * inv_a,
        src[3] + dst[3] * inv_a,
    ]
}

/// [`src_over_premul`] for [`Color`], converting `src` into `dst`'s space.
pub fn src_over(src: Color, dst: Color) -> Color {
    let src = src.convert_to(dst.space);
    let [r, g, b, a] = src_over_premul(src.to_premul(), dst.to_premul());
    Color::from_premul_f32(r, g, b, a, dst.space)
}

/// Applies coverage to a premultiplied source. Coverage scales all four
/// channels because the source is premultiplied — this is the operation the
/// fine rasterizer performs per pixel.
#[inline]
pub fn apply_coverage(src: [f32; 4], coverage: f32) -> [f32; 4] {
    [
        src[0] * coverage,
        src[1] * coverage,
        src[2] * coverage,
        src[3] * coverage,
    ]
}
