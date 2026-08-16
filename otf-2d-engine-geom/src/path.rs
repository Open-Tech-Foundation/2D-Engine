//! Paths: the geometry a fill or stroke is built from.
//!
//! Doc 02 §3 fixes the primitive set at exactly three curve types — line,
//! quadratic and cubic — so stages 3 through 5 never grow a fourth case. Arcs
//! and ellipses are converted to cubics here, at build time, and conics are
//! not in the IR at all.

use alloc::vec::Vec;

use crate::affine::Affine;
use crate::math;
use crate::point::{Point, Vec2};
use crate::rect::{Rect, RectRadii};

/// One path element, in the form consumers write.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathEl {
    MoveTo(Point),
    LineTo(Point),
    QuadTo(Point, Point),
    CurveTo(Point, Point, Point),
    ClosePath,
}

/// The verb half of a path's structure-of-arrays storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PathVerb {
    MoveTo = 0,
    LineTo = 1,
    QuadTo = 2,
    CurveTo = 3,
    ClosePath = 4,
}

impl PathVerb {
    /// How many points this verb consumes from the point array.
    #[inline]
    pub const fn point_count(self) -> usize {
        match self {
            PathVerb::MoveTo | PathVerb::LineTo => 1,
            PathVerb::QuadTo => 2,
            PathVerb::CurveTo => 3,
            PathVerb::ClosePath => 0,
        }
    }
}

/// One curve segment with its start point resolved.
///
/// This is what stage 3 flattens. Unlike [`PathEl`] it is self-contained: no
/// current point has to be tracked to interpret it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathSeg {
    Line(Point, Point),
    Quad(Point, Point, Point),
    Cubic(Point, Point, Point, Point),
}

impl PathSeg {
    #[inline]
    pub fn start(&self) -> Point {
        match *self {
            PathSeg::Line(p0, _) | PathSeg::Quad(p0, _, _) | PathSeg::Cubic(p0, _, _, _) => p0,
        }
    }

    #[inline]
    pub fn end(&self) -> Point {
        match *self {
            PathSeg::Line(_, p) | PathSeg::Quad(_, _, p) | PathSeg::Cubic(_, _, _, p) => p,
        }
    }

    /// The bounding box of the control polygon. Conservative: it contains the
    /// curve but may be larger than its tight bounds.
    pub fn control_bounds(&self) -> Rect {
        let mut r = Rect::NOTHING;
        match *self {
            PathSeg::Line(a, b) => {
                r = r.union_point(a).union_point(b);
            }
            PathSeg::Quad(a, b, c) => {
                r = r.union_point(a).union_point(b).union_point(c);
            }
            PathSeg::Cubic(a, b, c, d) => {
                r = r
                    .union_point(a)
                    .union_point(b)
                    .union_point(c)
                    .union_point(d);
            }
        }
        r
    }
}

/// What a path is, when it is something binning can special-case.
///
/// Doc 02 §3 makes `rect` and `rounded_rect` first-class rather than sugar:
/// together they are roughly 80% of application UI draws (Doc 03 §1), and
/// knowing a path *is* one lets stage 4 skip general curve handling entirely.
/// A path only carries a shape when it consists of that primitive and nothing
/// else.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathShape {
    /// Arbitrary geometry. Takes the general path.
    General,
    /// A single axis-aligned rectangle, already normalised.
    Rect(Rect),
    /// A single rounded rectangle, with radii already clamped to the rect.
    RoundedRect(Rect, RectRadii),
}

/// An immutable path, stored as parallel verb and point arrays.
///
/// Structure-of-arrays for the same reason the scene arena is (Doc 02 §2):
/// stage 3 walks points and never touches verbs on the hot inner loop, and a
/// contiguous point array hashes and copies in one pass.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path {
    verbs: Vec<PathVerb>,
    points: Vec<Point>,
    shape: Option<PathShape>,
    bounds: Rect,
}

impl Path {
    /// An empty path. Filling or stroking it draws nothing.
    pub fn new() -> Self {
        Self {
            verbs: Vec::new(),
            points: Vec::new(),
            shape: None,
            bounds: Rect::NOTHING,
        }
    }

    pub fn builder() -> PathBuilder {
        PathBuilder::new()
    }

    #[inline]
    pub fn verbs(&self) -> &[PathVerb] {
        &self.verbs
    }

    #[inline]
    pub fn points(&self) -> &[Point] {
        &self.points
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }

    /// The special shape this path is, if any.
    #[inline]
    pub fn shape(&self) -> PathShape {
        self.shape.unwrap_or(PathShape::General)
    }

    /// The bounding box of every control point.
    ///
    /// Conservative for curves — a cubic lies inside its control hull, which
    /// may be larger than the curve's tight bounds. Culling wants a bound that
    /// is cheap and never too small, which this is.
    #[inline]
    pub fn control_bounds(&self) -> Rect {
        self.bounds
    }

    /// True when every coordinate is finite. `SceneBuilder` checks this before
    /// encoding (Doc 02 §5).
    pub fn is_finite(&self) -> bool {
        self.points.iter().all(|p| p.is_finite())
    }

    /// The path as [`PathEl`]s, in submission order.
    pub fn elements(&self) -> impl Iterator<Item = PathEl> + '_ {
        let mut i = 0;
        self.verbs.iter().map(move |&verb| {
            let p = &self.points[i..];
            i += verb.point_count();
            match verb {
                PathVerb::MoveTo => PathEl::MoveTo(p[0]),
                PathVerb::LineTo => PathEl::LineTo(p[0]),
                PathVerb::QuadTo => PathEl::QuadTo(p[0], p[1]),
                PathVerb::CurveTo => PathEl::CurveTo(p[0], p[1], p[2]),
                PathVerb::ClosePath => PathEl::ClosePath,
            }
        })
    }

    /// The path as self-contained segments, with closing lines made explicit.
    ///
    /// A `ClosePath` yields a `Line` back to the subpath start unless the
    /// subpath is already closed geometrically, in which case it yields
    /// nothing — an explicit zero-length closing line would create a spurious
    /// join during stroke expansion.
    pub fn segments(&self) -> Segments<'_> {
        Segments {
            path: self,
            verb: 0,
            point: 0,
            current: Point::ORIGIN,
            subpath_start: Point::ORIGIN,
        }
    }

    /// This path with `affine` applied to every point.
    ///
    /// The shape hint survives only when the transform preserves axis
    /// alignment; a rotated rounded rect is not a rounded rect any more.
    pub fn transformed(&self, affine: Affine) -> Path {
        let points: Vec<Point> = self
            .points
            .iter()
            .map(|&p| affine.transform_point(p))
            .collect();
        let shape = match self.shape {
            Some(PathShape::Rect(r)) if affine.preserves_axis_alignment() => {
                Some(PathShape::Rect(affine.transform_rect_bbox(r)))
            }
            // Radii would have to scale uniformly to stay circular, so only a
            // uniform scale (or none) keeps the fast path.
            Some(PathShape::RoundedRect(r, radii))
                if affine.preserves_axis_alignment()
                    && (affine.max_scale() - affine.min_scale()).abs() < 1e-12 =>
            {
                let s = affine.max_scale();
                Some(PathShape::RoundedRect(
                    affine.transform_rect_bbox(r),
                    RectRadii::new(
                        radii.top_left * s,
                        radii.top_right * s,
                        radii.bottom_right * s,
                        radii.bottom_left * s,
                    ),
                ))
            }
            _ => None,
        };
        let mut bounds = Rect::NOTHING;
        for &p in &points {
            bounds = bounds.union_point(p);
        }
        Path {
            verbs: self.verbs.clone(),
            points,
            shape,
            bounds,
        }
    }
}

/// Iterator over a path's [`PathSeg`]s. See [`Path::segments`].
pub struct Segments<'a> {
    path: &'a Path,
    verb: usize,
    point: usize,
    current: Point,
    subpath_start: Point,
}

impl Iterator for Segments<'_> {
    type Item = PathSeg;

    fn next(&mut self) -> Option<PathSeg> {
        loop {
            let verb = *self.path.verbs.get(self.verb)?;
            let pts = &self.path.points[self.point..];
            self.verb += 1;
            self.point += verb.point_count();

            match verb {
                PathVerb::MoveTo => {
                    self.current = pts[0];
                    self.subpath_start = pts[0];
                }
                PathVerb::LineTo => {
                    let seg = PathSeg::Line(self.current, pts[0]);
                    self.current = pts[0];
                    return Some(seg);
                }
                PathVerb::QuadTo => {
                    let seg = PathSeg::Quad(self.current, pts[0], pts[1]);
                    self.current = pts[1];
                    return Some(seg);
                }
                PathVerb::CurveTo => {
                    let seg = PathSeg::Cubic(self.current, pts[0], pts[1], pts[2]);
                    self.current = pts[2];
                    return Some(seg);
                }
                PathVerb::ClosePath => {
                    let start = self.subpath_start;
                    let seg = PathSeg::Line(self.current, start);
                    self.current = start;
                    if seg.start() != start {
                        return Some(seg);
                    }
                }
            }
        }
    }
}

/// Builds a [`Path`].
///
/// There is no current-point state visible to callers beyond what the path
/// itself implies: every method takes absolute coordinates, and there is no
/// `save`/`restore` (invariant I-3).
#[derive(Debug, Clone, Default)]
pub struct PathBuilder {
    verbs: Vec<PathVerb>,
    points: Vec<Point>,
    bounds: Rect,
    /// `Some` while the path is exactly one recognised primitive.
    shape: Option<PathShape>,
    /// Set once anything is appended, so a second primitive clears the hint.
    touched: bool,
}

impl PathBuilder {
    pub fn new() -> Self {
        Self {
            verbs: Vec::new(),
            points: Vec::new(),
            bounds: Rect::NOTHING,
            shape: None,
            touched: false,
        }
    }

    /// Pre-allocates room for `verbs` verbs and `points` points.
    pub fn with_capacity(verbs: usize, points: usize) -> Self {
        Self {
            verbs: Vec::with_capacity(verbs),
            points: Vec::with_capacity(points),
            bounds: Rect::NOTHING,
            shape: None,
            touched: false,
        }
    }

    /// Clears the builder, keeping its allocations for the next path.
    pub fn reset(&mut self) -> &mut Self {
        self.verbs.clear();
        self.points.clear();
        self.bounds = Rect::NOTHING;
        self.shape = None;
        self.touched = false;
        self
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }

    /// The point the next segment starts from, or `None` before any `move_to`.
    #[inline]
    pub fn current_point(&self) -> Option<Point> {
        self.points.last().copied()
    }

    fn push(&mut self, verb: PathVerb, pts: &[Point]) {
        debug_assert_eq!(verb.point_count(), pts.len());
        self.verbs.push(verb);
        for &p in pts {
            self.points.push(p);
            self.bounds = self.bounds.union_point(p);
        }
        self.touched = true;
    }

    /// Records that the path is no longer a single recognised primitive.
    fn clear_shape(&mut self) {
        self.shape = None;
    }

    pub fn move_to(&mut self, p: impl Into<Point>) -> &mut Self {
        self.clear_shape();
        self.push(PathVerb::MoveTo, &[p.into()]);
        self
    }

    pub fn line_to(&mut self, p: impl Into<Point>) -> &mut Self {
        self.clear_shape();
        self.push(PathVerb::LineTo, &[p.into()]);
        self
    }

    pub fn quad_to(&mut self, c: impl Into<Point>, p: impl Into<Point>) -> &mut Self {
        self.clear_shape();
        self.push(PathVerb::QuadTo, &[c.into(), p.into()]);
        self
    }

    pub fn curve_to(
        &mut self,
        c1: impl Into<Point>,
        c2: impl Into<Point>,
        p: impl Into<Point>,
    ) -> &mut Self {
        self.clear_shape();
        self.push(PathVerb::CurveTo, &[c1.into(), c2.into(), p.into()]);
        self
    }

    pub fn close(&mut self) -> &mut Self {
        self.clear_shape();
        self.push(PathVerb::ClosePath, &[]);
        self
    }

    /// Appends every element of `path`.
    pub fn extend_from_path(&mut self, path: &Path) -> &mut Self {
        self.clear_shape();
        for el in path.elements() {
            match el {
                PathEl::MoveTo(p) => self.push(PathVerb::MoveTo, &[p]),
                PathEl::LineTo(p) => self.push(PathVerb::LineTo, &[p]),
                PathEl::QuadTo(c, p) => self.push(PathVerb::QuadTo, &[c, p]),
                PathEl::CurveTo(a, b, p) => self.push(PathVerb::CurveTo, &[a, b, p]),
                PathEl::ClosePath => self.push(PathVerb::ClosePath, &[]),
            }
        }
        self
    }

    // ---- primitives ----------------------------------------------------

    /// An axis-aligned rectangle as a closed subpath, wound clockwise in a
    /// y-down coordinate system.
    pub fn rect(&mut self, r: Rect) -> &mut Self {
        let was_empty = !self.touched;
        let n = r.normalized();
        self.push(PathVerb::MoveTo, &[Point::new(n.x0, n.y0)]);
        self.push(PathVerb::LineTo, &[Point::new(n.x1, n.y0)]);
        self.push(PathVerb::LineTo, &[Point::new(n.x1, n.y1)]);
        self.push(PathVerb::LineTo, &[Point::new(n.x0, n.y1)]);
        self.push(PathVerb::ClosePath, &[]);
        self.shape = was_empty.then_some(PathShape::Rect(n));
        self
    }

    /// A rounded rectangle as a closed subpath.
    ///
    /// Radii are clamped into the rect the way CSS clamps `border-radius`, so
    /// the result never bulges outside `r`: the path's control bounds are
    /// exactly `r`.
    pub fn rounded_rect(&mut self, r: Rect, radii: impl Into<RectRadii>) -> &mut Self {
        let was_empty = !self.touched;
        let n = r.normalized();
        let radii = radii.into().clamped_to(n);

        if radii.is_zero() {
            self.rect(n);
            self.shape = was_empty.then_some(PathShape::RoundedRect(n, radii));
            return self;
        }

        let (tl, tr, br, bl) = (
            radii.top_left,
            radii.top_right,
            radii.bottom_right,
            radii.bottom_left,
        );

        self.push(PathVerb::MoveTo, &[Point::new(n.x0 + tl, n.y0)]);
        self.push(PathVerb::LineTo, &[Point::new(n.x1 - tr, n.y0)]);
        self.corner(
            Point::new(n.x1 - tr, n.y0 + tr),
            tr,
            -core::f64::consts::FRAC_PI_2,
        );
        self.push(PathVerb::LineTo, &[Point::new(n.x1, n.y1 - br)]);
        self.corner(Point::new(n.x1 - br, n.y1 - br), br, 0.0);
        self.push(PathVerb::LineTo, &[Point::new(n.x0 + bl, n.y1)]);
        self.corner(
            Point::new(n.x0 + bl, n.y1 - bl),
            bl,
            core::f64::consts::FRAC_PI_2,
        );
        self.push(PathVerb::LineTo, &[Point::new(n.x0, n.y0 + tl)]);
        self.corner(Point::new(n.x0 + tl, n.y0 + tl), tl, core::f64::consts::PI);
        self.push(PathVerb::ClosePath, &[]);

        self.shape = was_empty.then_some(PathShape::RoundedRect(n, radii));
        self
    }

    /// One 90° corner arc of a rounded rect, as a single cubic.
    fn corner(&mut self, center: Point, radius: f64, start_angle: f64) {
        if radius <= 0.0 {
            return;
        }
        append_arc_cubics(
            self,
            center,
            Vec2::new(radius, radius),
            0.0,
            start_angle,
            core::f64::consts::FRAC_PI_2,
        );
    }

    /// A closed ellipse as its own subpath, built from four cubics.
    pub fn ellipse(&mut self, center: impl Into<Point>, radii: impl Into<Vec2>) -> &mut Self {
        self.ellipse_rotated(center, radii, 0.0)
    }

    /// A closed ellipse whose axes are rotated by `x_rotation` radians.
    pub fn ellipse_rotated(
        &mut self,
        center: impl Into<Point>,
        radii: impl Into<Vec2>,
        x_rotation: f64,
    ) -> &mut Self {
        self.clear_shape();
        let (center, radii) = (center.into(), radii.into());
        let start = arc_point(center, radii, x_rotation, 0.0);
        self.push(PathVerb::MoveTo, &[start]);
        append_arc_cubics(
            self,
            center,
            radii,
            x_rotation,
            0.0,
            core::f64::consts::PI * 2.0,
        );
        self.push(PathVerb::ClosePath, &[]);
        self
    }

    /// A closed circle as its own subpath.
    pub fn circle(&mut self, center: impl Into<Point>, radius: f64) -> &mut Self {
        self.ellipse(center, Vec2::new(radius, radius))
    }

    /// An elliptical arc, appended to the current subpath.
    ///
    /// The arc runs `sweep_angle` radians from `start_angle`, measured in the
    /// ellipse's own frame. A line is inserted first if the current point is
    /// not already at the arc's start, so the result is always connected. The
    /// arc becomes cubics here — the IR has no arc primitive (Doc 02 §3).
    pub fn arc_to(
        &mut self,
        center: impl Into<Point>,
        radii: impl Into<Vec2>,
        x_rotation: f64,
        start_angle: f64,
        sweep_angle: f64,
    ) -> &mut Self {
        self.clear_shape();
        let (center, radii) = (center.into(), radii.into());
        let start = arc_point(center, radii, x_rotation, start_angle);
        match self.current_point() {
            None => self.push(PathVerb::MoveTo, &[start]),
            Some(p) if p != start => self.push(PathVerb::LineTo, &[start]),
            Some(_) => {}
        }
        append_arc_cubics(self, center, radii, x_rotation, start_angle, sweep_angle);
        self
    }

    // ---- finishing -----------------------------------------------------

    /// Produces a [`Path`], leaving the builder usable.
    ///
    /// Copies the buffers. Use [`PathBuilder::finish`] when the builder is
    /// done with, or [`PathBuilder::reset`] to reuse its allocations.
    pub fn build(&self) -> Path {
        Path {
            verbs: self.verbs.clone(),
            points: self.points.clone(),
            shape: self.shape,
            bounds: self.bounds,
        }
    }

    /// Consumes the builder and produces a [`Path`] without copying.
    pub fn finish(self) -> Path {
        Path {
            verbs: self.verbs,
            points: self.points,
            shape: self.shape,
            bounds: self.bounds,
        }
    }
}

/// A point on an ellipse at parameter `angle`.
fn arc_point(center: Point, radii: Vec2, x_rotation: f64, angle: f64) -> Point {
    let (sin_a, cos_a) = (math::sin(angle), math::cos(angle));
    let (x, y) = (radii.x * cos_a, radii.y * sin_a);
    let (sin_r, cos_r) = (math::sin(x_rotation), math::cos(x_rotation));
    Point::new(
        center.x + x * cos_r - y * sin_r,
        center.y + x * sin_r + y * cos_r,
    )
}

/// The derivative of [`arc_point`] with respect to `angle`.
fn arc_derivative(radii: Vec2, x_rotation: f64, angle: f64) -> Vec2 {
    let (sin_a, cos_a) = (math::sin(angle), math::cos(angle));
    let (dx, dy) = (-radii.x * sin_a, radii.y * cos_a);
    let (sin_r, cos_r) = (math::sin(x_rotation), math::cos(x_rotation));
    Vec2::new(dx * cos_r - dy * sin_r, dx * sin_r + dy * cos_r)
}

/// Appends the cubics approximating an elliptical arc.
///
/// The arc is split so no piece exceeds 90°, and each piece uses the standard
/// tangent-matching construction whose control handles are
/// `(4/3)·tan(δ/4)` long. At 90° that is the familiar 0.5522847 circle
/// constant; the maximum radial error is under 0.02% of the radius, which is
/// far below any device tolerance stage 3 will apply.
fn append_arc_cubics(
    builder: &mut PathBuilder,
    center: Point,
    radii: Vec2,
    x_rotation: f64,
    start_angle: f64,
    sweep_angle: f64,
) {
    if radii.x == 0.0 || radii.y == 0.0 || sweep_angle == 0.0 || !sweep_angle.is_finite() {
        return;
    }
    let segments = {
        let quarters = math::abs(sweep_angle) / core::f64::consts::FRAC_PI_2;
        (math::ceil(quarters) as usize).max(1)
    };
    let delta = sweep_angle / segments as f64;
    // (4/3)·tan(δ/4), the handle length that makes the cubic meet the arc at
    // both ends with matching tangents.
    let alpha = (4.0 / 3.0) * math::tan(delta * 0.25);

    let mut angle = start_angle;
    for _ in 0..segments {
        let next = angle + delta;
        let p0 = arc_point(center, radii, x_rotation, angle);
        let p3 = arc_point(center, radii, x_rotation, next);
        let d0 = arc_derivative(radii, x_rotation, angle);
        let d3 = arc_derivative(radii, x_rotation, next);
        builder.push(PathVerb::CurveTo, &[p0 + d0 * alpha, p3 - d3 * alpha, p3]);
        angle = next;
    }
}
