//! Affine transforms.

use core::ops::Mul;

use crate::math;
use crate::point::{Point, Vec2};
use crate::rect::Rect;

/// A row-major 2×3 affine transform, stored as `[a, b, c, d, e, f]`:
///
/// ```text
/// | a  c  e |     x' = a·x + c·y + e
/// | b  d  f |     y' = b·x + d·y + f
/// | 0  0  1 |
/// ```
///
/// This is the SVG and PostScript convention, so a matrix copied out of a
/// consumer's stylesheet means what it looks like it means.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine([f64; 6]);

impl Default for Affine {
    #[inline]
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Affine {
    pub const IDENTITY: Affine = Affine([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    #[inline]
    pub const fn new(coefficients: [f64; 6]) -> Self {
        Affine(coefficients)
    }

    #[inline]
    pub const fn as_coefficients(&self) -> [f64; 6] {
        self.0
    }

    #[inline]
    pub fn translate(v: Vec2) -> Affine {
        Affine([1.0, 0.0, 0.0, 1.0, v.x, v.y])
    }

    #[inline]
    pub fn scale(s: f64) -> Affine {
        Affine([s, 0.0, 0.0, s, 0.0, 0.0])
    }

    #[inline]
    pub fn scale_non_uniform(sx: f64, sy: f64) -> Affine {
        Affine([sx, 0.0, 0.0, sy, 0.0, 0.0])
    }

    /// Rotation about the origin. Positive angles turn from +x toward +y,
    /// which is clockwise on screen because y points down.
    #[inline]
    pub fn rotate(radians: f64) -> Affine {
        let (s, c) = (math::sin(radians), math::cos(radians));
        Affine([c, s, -s, c, 0.0, 0.0])
    }

    /// Rotation about `center` rather than the origin.
    pub fn rotate_about(radians: f64, center: Point) -> Affine {
        Affine::translate(-center.to_vec2())
            .then(Affine::rotate(radians))
            .then(Affine::translate(center.to_vec2()))
    }

    /// Shear by the given tangents. `skew(tan θ, 0)` is a synthetic oblique.
    #[inline]
    pub fn skew(kx: f64, ky: f64) -> Affine {
        Affine([1.0, ky, kx, 1.0, 0.0, 0.0])
    }

    /// Applies `self` first, then `other`.
    ///
    /// Reads left to right in the order the transforms happen, which is the
    /// opposite of matrix-product notation and the right way round for a
    /// builder API.
    #[inline]
    #[must_use]
    pub fn then(self, other: Affine) -> Affine {
        let [a1, b1, c1, d1, e1, f1] = self.0;
        let [a2, b2, c2, d2, e2, f2] = other.0;
        Affine([
            a2 * a1 + c2 * b1,
            b2 * a1 + d2 * b1,
            a2 * c1 + c2 * d1,
            b2 * c1 + d2 * d1,
            a2 * e1 + c2 * f1 + e2,
            b2 * e1 + d2 * f1 + f2,
        ])
    }

    /// Convenience for `self.then(Affine::translate(v))`.
    #[inline]
    #[must_use]
    pub fn then_translate(self, v: Vec2) -> Affine {
        let [a, b, c, d, e, f] = self.0;
        Affine([a, b, c, d, e + v.x, f + v.y])
    }

    /// The determinant of the linear part. Zero means the transform collapses
    /// the plane onto a line or a point and cannot be inverted.
    #[inline]
    pub fn determinant(self) -> f64 {
        let [a, b, c, d, _, _] = self.0;
        a * d - b * c
    }

    /// The inverse, or `None` when the transform is singular or non-finite.
    pub fn inverse(self) -> Option<Affine> {
        let [a, b, c, d, e, f] = self.0;
        let det = a * d - b * c;
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let inv_det = 1.0 / det;
        let out = Affine([
            d * inv_det,
            -b * inv_det,
            -c * inv_det,
            a * inv_det,
            (c * f - d * e) * inv_det,
            (b * e - a * f) * inv_det,
        ]);
        out.is_finite().then_some(out)
    }

    /// The largest singular value of the linear part.
    ///
    /// This is the factor by which the transform can stretch a unit vector, so
    /// it is what stage 3 uses to pick a flattening tolerance (Doc 01 §4). The
    /// closed form below avoids forming MᵀM and taking its eigenvalues.
    pub fn max_scale(self) -> f64 {
        let [a, b, c, d, _, _] = self.0;
        // For M = [[a, c], [b, d]] the singular values are |Q ± R| with
        // Q = ‖(mean of diagonal, mean of antidiagonal difference)‖ and
        // R = ‖(half difference of diagonal, mean of antidiagonal)‖.
        let e = (a + d) * 0.5;
        let f = (a - d) * 0.5;
        let g = (b + c) * 0.5;
        let h = (b - c) * 0.5;
        math::hypot(e, h) + math::hypot(f, g)
    }

    /// The smallest singular value: how much the transform can *compress* a
    /// unit vector. Zero for a degenerate transform.
    pub fn min_scale(self) -> f64 {
        let [a, b, c, d, _, _] = self.0;
        let e = (a + d) * 0.5;
        let f = (a - d) * 0.5;
        let g = (b + c) * 0.5;
        let h = (b - c) * 0.5;
        math::abs(math::hypot(e, h) - math::hypot(f, g))
    }

    /// The translation component.
    #[inline]
    pub fn translation(self) -> Vec2 {
        Vec2::new(self.0[4], self.0[5])
    }

    /// This transform with its translation replaced.
    #[inline]
    #[must_use]
    pub fn with_translation(self, v: Vec2) -> Affine {
        let [a, b, c, d, _, _] = self.0;
        Affine([a, b, c, d, v.x, v.y])
    }

    /// True when the linear part is the identity, so the transform is a pure
    /// translation. The scroll fast path in Doc 03 §4 keys off this.
    #[inline]
    pub fn is_translation(self) -> bool {
        let [a, b, c, d, _, _] = self.0;
        a == 1.0 && b == 0.0 && c == 0.0 && d == 1.0
    }

    /// True when the transform maps axis-aligned rects to axis-aligned rects,
    /// i.e. it is a scale/flip/translate or that composed with a quarter turn.
    /// Rect clips take a fast path only when this holds (Doc 01 §4).
    ///
    /// The comparison is exact, deliberately: a tolerance here would be an
    /// arbitrary threshold on a correctness-relevant fast path. One
    /// consequence is that `Affine::rotate(FRAC_PI_2)` does *not* qualify,
    /// because `cos(π/2)` is 6.1e-17 rather than zero. A consumer that wants
    /// the fast path for a right angle should write the matrix exactly.
    #[inline]
    pub fn preserves_axis_alignment(self) -> bool {
        let [a, b, c, d, _, _] = self.0;
        (b == 0.0 && c == 0.0) || (a == 0.0 && d == 0.0)
    }

    #[inline]
    pub fn is_finite(self) -> bool {
        self.0.iter().all(|v| v.is_finite())
    }

    #[inline]
    pub fn is_identity(self) -> bool {
        self.0 == Affine::IDENTITY.0
    }

    #[inline]
    pub fn transform_point(self, p: Point) -> Point {
        let [a, b, c, d, e, f] = self.0;
        Point::new(a * p.x + c * p.y + e, b * p.x + d * p.y + f)
    }

    /// Transforms a displacement, ignoring translation.
    #[inline]
    pub fn transform_vec2(self, v: Vec2) -> Vec2 {
        let [a, b, c, d, _, _] = self.0;
        Vec2::new(a * v.x + c * v.y, b * v.x + d * v.y)
    }

    /// The axis-aligned bounding box of the transformed rect.
    ///
    /// Exact only when the transform preserves axis alignment; otherwise it is
    /// the bounding box of the transformed corners, which is the tightest
    /// axis-aligned answer available.
    pub fn transform_rect_bbox(self, r: Rect) -> Rect {
        let corners = [
            self.transform_point(Point::new(r.x0, r.y0)),
            self.transform_point(Point::new(r.x1, r.y0)),
            self.transform_point(Point::new(r.x1, r.y1)),
            self.transform_point(Point::new(r.x0, r.y1)),
        ];
        let mut out = Rect::NOTHING;
        for p in corners {
            out = out.union_point(p);
        }
        out
    }
}

/// `a * b` applies `b` first, then `a`, matching matrix-product notation.
/// Prefer [`Affine::then`] in new code; this exists because the product form
/// is what transcribed maths looks like.
impl Mul for Affine {
    type Output = Affine;
    #[inline]
    fn mul(self, rhs: Affine) -> Affine {
        rhs.then(self)
    }
}

impl Mul<Point> for Affine {
    type Output = Point;
    #[inline]
    fn mul(self, p: Point) -> Point {
        self.transform_point(p)
    }
}
