//! Points, vectors and sizes.
//!
//! Coordinates are `f64` because this is public API and consumers work in
//! world space, where `f32` runs out of precision at document scale — a
//! browser page can be 100k logical pixels tall (Doc 02 §3). The narrowing to
//! `f32` happens after stage 2, once coordinates are device-local and bounded.

use core::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

use crate::math;

/// A position in 2D space.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// A displacement in 2D space.
///
/// Distinct from [`Point`] because affines translate points but not vectors,
/// and confusing the two is a classic source of off-by-a-translation bugs.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

/// A width and height.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Point {
    pub const ORIGIN: Point = Point { x: 0.0, y: 0.0 };

    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// The vector from the origin to this point.
    #[inline]
    pub const fn to_vec2(self) -> Vec2 {
        Vec2 {
            x: self.x,
            y: self.y,
        }
    }

    #[inline]
    pub fn distance(self, other: Point) -> f64 {
        math::hypot(self.x - other.x, self.y - other.y)
    }

    /// Linear interpolation; `t = 0` is `self`, `t = 1` is `other`.
    #[inline]
    pub fn lerp(self, other: Point, t: f64) -> Point {
        Point::new(
            self.x + (other.x - self.x) * t,
            self.y + (other.y - self.y) * t,
        )
    }

    /// True when both coordinates are finite. Encode-time validation rejects
    /// anything else (Doc 02 §5, `EncodeError::NonFiniteCoordinate`).
    #[inline]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    #[inline]
    pub fn midpoint(self, other: Point) -> Point {
        Point::new((self.x + other.x) * 0.5, (self.y + other.y) * 0.5)
    }
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    #[inline]
    pub const fn to_point(self) -> Point {
        Point {
            x: self.x,
            y: self.y,
        }
    }

    #[inline]
    pub fn length(self) -> f64 {
        math::hypot(self.x, self.y)
    }

    #[inline]
    pub fn length_squared(self) -> f64 {
        self.x * self.x + self.y * self.y
    }

    /// Returns the unit vector in this direction, or `None` for the zero
    /// vector, which has no direction.
    #[inline]
    pub fn normalize(self) -> Option<Vec2> {
        let len = self.length();
        if len > 0.0 && len.is_finite() {
            Some(self / len)
        } else {
            None
        }
    }

    #[inline]
    pub fn dot(self, other: Vec2) -> f64 {
        self.x * other.x + self.y * other.y
    }

    /// The 2D cross product, i.e. the z component of the 3D cross product.
    /// Its sign gives the turn direction, which joins and winding both need.
    #[inline]
    pub fn cross(self, other: Vec2) -> f64 {
        self.x * other.y - self.y * other.x
    }

    /// Rotated a quarter turn. Cheap and exact, unlike a general rotation.
    #[inline]
    pub const fn perpendicular(self) -> Vec2 {
        Vec2 {
            x: -self.y,
            y: self.x,
        }
    }

    #[inline]
    pub fn atan2(self) -> f64 {
        math::atan2(self.y, self.x)
    }

    #[inline]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

impl Size {
    pub const ZERO: Size = Size {
        width: 0.0,
        height: 0.0,
    };

    #[inline]
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }

    #[inline]
    pub const fn to_vec2(self) -> Vec2 {
        Vec2 {
            x: self.width,
            y: self.height,
        }
    }

    #[inline]
    pub fn area(self) -> f64 {
        self.width * self.height
    }

    /// True when either dimension is zero or negative, i.e. nothing to draw.
    #[inline]
    pub fn is_empty(self) -> bool {
        !(self.width > 0.0 && self.height > 0.0)
    }

    #[inline]
    pub fn is_finite(self) -> bool {
        self.width.is_finite() && self.height.is_finite()
    }
}

// ---- conversions -------------------------------------------------------

impl From<(f64, f64)> for Point {
    #[inline]
    fn from((x, y): (f64, f64)) -> Self {
        Point::new(x, y)
    }
}

impl From<(f64, f64)> for Vec2 {
    #[inline]
    fn from((x, y): (f64, f64)) -> Self {
        Vec2::new(x, y)
    }
}

impl From<(f64, f64)> for Size {
    #[inline]
    fn from((width, height): (f64, f64)) -> Self {
        Size::new(width, height)
    }
}

impl From<Point> for Vec2 {
    #[inline]
    fn from(p: Point) -> Self {
        p.to_vec2()
    }
}

impl From<Vec2> for Point {
    #[inline]
    fn from(v: Vec2) -> Self {
        v.to_point()
    }
}

// ---- arithmetic --------------------------------------------------------

impl Add<Vec2> for Point {
    type Output = Point;
    #[inline]
    fn add(self, v: Vec2) -> Point {
        Point::new(self.x + v.x, self.y + v.y)
    }
}

impl AddAssign<Vec2> for Point {
    #[inline]
    fn add_assign(&mut self, v: Vec2) {
        *self = *self + v;
    }
}

impl Sub<Vec2> for Point {
    type Output = Point;
    #[inline]
    fn sub(self, v: Vec2) -> Point {
        Point::new(self.x - v.x, self.y - v.y)
    }
}

impl SubAssign<Vec2> for Point {
    #[inline]
    fn sub_assign(&mut self, v: Vec2) {
        *self = *self - v;
    }
}

/// The difference of two points is a displacement, not a point.
impl Sub<Point> for Point {
    type Output = Vec2;
    #[inline]
    fn sub(self, other: Point) -> Vec2 {
        Vec2::new(self.x - other.x, self.y - other.y)
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    #[inline]
    fn add(self, other: Vec2) -> Vec2 {
        Vec2::new(self.x + other.x, self.y + other.y)
    }
}

impl AddAssign for Vec2 {
    #[inline]
    fn add_assign(&mut self, other: Vec2) {
        *self = *self + other;
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    #[inline]
    fn sub(self, other: Vec2) -> Vec2 {
        Vec2::new(self.x - other.x, self.y - other.y)
    }
}

impl SubAssign for Vec2 {
    #[inline]
    fn sub_assign(&mut self, other: Vec2) {
        *self = *self - other;
    }
}

impl Neg for Vec2 {
    type Output = Vec2;
    #[inline]
    fn neg(self) -> Vec2 {
        Vec2::new(-self.x, -self.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn mul(self, s: f64) -> Vec2 {
        Vec2::new(self.x * s, self.y * s)
    }
}

impl Mul<Vec2> for f64 {
    type Output = Vec2;
    #[inline]
    fn mul(self, v: Vec2) -> Vec2 {
        v * self
    }
}

impl Div<f64> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn div(self, s: f64) -> Vec2 {
        Vec2::new(self.x / s, self.y / s)
    }
}

impl Mul<f64> for Size {
    type Output = Size;
    #[inline]
    fn mul(self, s: f64) -> Size {
        Size::new(self.width * s, self.height * s)
    }
}
