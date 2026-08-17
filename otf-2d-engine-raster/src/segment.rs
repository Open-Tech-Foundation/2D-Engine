//! Device-space line segments — the currency between stages 3 and 4.
//!
//! `f32`, not `f64`: by the time geometry is a device-space segment it is
//! bounded by the surface, so the extra range buys nothing and costs half the
//! SIMD lanes (D-08, D-21). Scene space stays `f64`; this is where it narrows.

use bytemuck::{Pod, Zeroable};

/// A directed line segment in device pixels.
///
/// Direction matters: stage 5 accumulates *signed* area, so `p0 → p1` and
/// `p1 → p0` contribute opposite winding.
#[derive(Debug, Clone, Copy, PartialEq, Default, Zeroable, Pod)]
#[repr(C)]
pub struct Segment {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Segment {
    #[inline]
    pub const fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Segment {
        Segment { x0, y0, x1, y1 }
    }

    #[inline]
    pub fn is_finite(&self) -> bool {
        self.x0.is_finite() && self.y0.is_finite() && self.x1.is_finite() && self.y1.is_finite()
    }

    /// True when the segment covers no vertical extent, and so contributes no
    /// winding to a scanline.
    #[inline]
    pub fn is_horizontal(&self) -> bool {
        self.y0 == self.y1
    }

    #[inline]
    pub fn min_y(&self) -> f32 {
        self.y0.min(self.y1)
    }

    #[inline]
    pub fn max_y(&self) -> f32 {
        self.y0.max(self.y1)
    }

    #[inline]
    pub fn min_x(&self) -> f32 {
        self.x0.min(self.x1)
    }

    #[inline]
    pub fn max_x(&self) -> f32 {
        self.x0.max(self.x1)
    }

    /// The `x` where the segment crosses horizontal line `y`, assuming `y`
    /// lies within its vertical extent.
    ///
    /// Computed in `f64`: the division amplifies error on near-horizontal
    /// segments, which are exactly the ones binning must place accurately.
    #[inline]
    pub fn x_at(&self, y: f64) -> f64 {
        let dy = self.y1 as f64 - self.y0 as f64;
        if dy == 0.0 {
            return self.x0 as f64;
        }
        let t = (y - self.y0 as f64) / dy;
        self.x0 as f64 + t * (self.x1 as f64 - self.x0 as f64)
    }
}
