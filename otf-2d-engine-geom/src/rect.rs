//! Axis-aligned rectangles.

use crate::math;
use crate::point::{Point, Size, Vec2};

/// An axis-aligned rectangle, stored as two corners.
///
/// A rect is *not* required to be normalised (`x0 <= x1`). Consumers hand us
/// whatever their layout produced, and silently reordering coordinates hides
/// bugs; call [`Rect::normalized`] when you need the ordering guarantee.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    /// The empty rect at the origin.
    pub const ZERO: Rect = Rect {
        x0: 0.0,
        y0: 0.0,
        x1: 0.0,
        y1: 0.0,
    };

    /// A rect that contains nothing and is the identity for [`Rect::union`].
    pub const NOTHING: Rect = Rect {
        x0: f64::INFINITY,
        y0: f64::INFINITY,
        x1: f64::NEG_INFINITY,
        y1: f64::NEG_INFINITY,
    };

    #[inline]
    pub const fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Self { x0, y0, x1, y1 }
    }

    #[inline]
    pub fn from_origin_size(origin: Point, size: Size) -> Self {
        Self::new(
            origin.x,
            origin.y,
            origin.x + size.width,
            origin.y + size.height,
        )
    }

    #[inline]
    pub fn from_points(a: Point, b: Point) -> Self {
        Self::new(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
    }

    #[inline]
    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }

    #[inline]
    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }

    #[inline]
    pub fn size(&self) -> Size {
        Size::new(self.width(), self.height())
    }

    #[inline]
    pub fn origin(&self) -> Point {
        Point::new(self.x0, self.y0)
    }

    #[inline]
    pub fn center(&self) -> Point {
        Point::new((self.x0 + self.x1) * 0.5, (self.y0 + self.y1) * 0.5)
    }

    /// The enclosed area, or zero for an empty rect. Checking emptiness first
    /// matters: `NOTHING` has infinite extents whose product is `+inf`, and a
    /// rect inverted on both axes has a positive product.
    #[inline]
    pub fn area(&self) -> f64 {
        if self.is_empty() {
            0.0
        } else {
            self.width() * self.height()
        }
    }

    /// True when the rect encloses no area, so nothing inside it can be drawn.
    #[inline]
    pub fn is_empty(&self) -> bool {
        !(self.x1 > self.x0 && self.y1 > self.y0)
    }

    #[inline]
    pub fn is_finite(&self) -> bool {
        self.x0.is_finite() && self.y0.is_finite() && self.x1.is_finite() && self.y1.is_finite()
    }

    /// Reorders the corners so `x0 <= x1` and `y0 <= y1`.
    #[inline]
    pub fn normalized(self) -> Rect {
        Rect::new(
            self.x0.min(self.x1),
            self.y0.min(self.y1),
            self.x0.max(self.x1),
            self.y0.max(self.y1),
        )
    }

    /// The smallest rect containing both. [`Rect::NOTHING`] is the identity.
    #[inline]
    pub fn union(self, other: Rect) -> Rect {
        Rect::new(
            self.x0.min(other.x0),
            self.y0.min(other.y0),
            self.x1.max(other.x1),
            self.y1.max(other.y1),
        )
    }

    /// Extends the rect to contain `p`.
    #[inline]
    pub fn union_point(self, p: Point) -> Rect {
        Rect::new(
            self.x0.min(p.x),
            self.y0.min(p.y),
            self.x1.max(p.x),
            self.y1.max(p.y),
        )
    }

    /// The overlap of two rects. May be empty; check with [`Rect::is_empty`].
    #[inline]
    pub fn intersect(self, other: Rect) -> Rect {
        Rect::new(
            self.x0.max(other.x0),
            self.y0.max(other.y0),
            self.x1.min(other.x1),
            self.y1.min(other.y1),
        )
    }

    /// True when the two rects share any area. Touching edges do not count.
    #[inline]
    pub fn intersects(self, other: Rect) -> bool {
        !self.intersect(other).is_empty()
    }

    /// True when `other` lies entirely within `self`.
    #[inline]
    pub fn contains_rect(self, other: Rect) -> bool {
        other.x0 >= self.x0 && other.y0 >= self.y0 && other.x1 <= self.x1 && other.y1 <= self.y1
    }

    /// Half-open containment: the low edges are inside, the high edges are not.
    /// This is the convention that tiles pixels without double-counting.
    #[inline]
    pub fn contains(self, p: Point) -> bool {
        p.x >= self.x0 && p.x < self.x1 && p.y >= self.y0 && p.y < self.y1
    }

    /// Grows the rect by `d` on every side; a negative `d` shrinks it.
    #[inline]
    pub fn inflate(self, d: f64) -> Rect {
        Rect::new(self.x0 - d, self.y0 - d, self.x1 + d, self.y1 + d)
    }

    #[inline]
    pub fn translate(self, v: Vec2) -> Rect {
        Rect::new(self.x0 + v.x, self.y0 + v.y, self.x1 + v.x, self.y1 + v.y)
    }

    /// The smallest rect on integer boundaries that contains this one. Used
    /// when mapping device-space bounds onto pixels and tiles.
    #[inline]
    pub fn round_out(self) -> Rect {
        Rect::new(
            math::floor(self.x0),
            math::floor(self.y0),
            math::ceil(self.x1),
            math::ceil(self.y1),
        )
    }

    /// The largest rect on integer boundaries contained by this one.
    #[inline]
    pub fn round_in(self) -> Rect {
        Rect::new(
            math::ceil(self.x0),
            math::ceil(self.y0),
            math::floor(self.x1),
            math::floor(self.y1),
        )
    }
}

/// Per-corner radii for a rounded rectangle.
///
/// Corner order is clockwise from the top left in a y-down coordinate system,
/// matching CSS `border-radius`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RectRadii {
    pub top_left: f64,
    pub top_right: f64,
    pub bottom_right: f64,
    pub bottom_left: f64,
}

impl RectRadii {
    pub const ZERO: RectRadii = RectRadii::uniform(0.0);

    #[inline]
    pub const fn new(top_left: f64, top_right: f64, bottom_right: f64, bottom_left: f64) -> Self {
        Self {
            top_left,
            top_right,
            bottom_right,
            bottom_left,
        }
    }

    #[inline]
    pub const fn uniform(r: f64) -> Self {
        Self::new(r, r, r, r)
    }

    #[inline]
    pub fn is_zero(&self) -> bool {
        self.top_left <= 0.0
            && self.top_right <= 0.0
            && self.bottom_right <= 0.0
            && self.bottom_left <= 0.0
    }

    #[inline]
    pub fn is_finite(&self) -> bool {
        self.top_left.is_finite()
            && self.top_right.is_finite()
            && self.bottom_right.is_finite()
            && self.bottom_left.is_finite()
    }

    /// Clamps radii into `rect` the way CSS does: negatives become zero, then
    /// if any edge's two radii overshoot that edge, every radius is scaled by
    /// the same factor so the corners stay circular rather than distorting.
    pub fn clamped_to(self, rect: Rect) -> RectRadii {
        let rect = rect.normalized();
        let (w, h) = (rect.width(), rect.height());
        let mut r = RectRadii::new(
            self.top_left.max(0.0),
            self.top_right.max(0.0),
            self.bottom_right.max(0.0),
            self.bottom_left.max(0.0),
        );

        let mut scale: f64 = 1.0;
        let mut limit = |sum: f64, extent: f64| {
            if sum > 0.0 && sum > extent {
                scale = scale.min(extent / sum);
            }
        };
        limit(r.top_left + r.top_right, w);
        limit(r.bottom_left + r.bottom_right, w);
        limit(r.top_left + r.bottom_left, h);
        limit(r.top_right + r.bottom_right, h);

        if scale < 1.0 {
            r = RectRadii::new(
                r.top_left * scale,
                r.top_right * scale,
                r.bottom_right * scale,
                r.bottom_left * scale,
            );
        }
        r
    }
}

impl From<f64> for RectRadii {
    #[inline]
    fn from(r: f64) -> Self {
        RectRadii::uniform(r)
    }
}
