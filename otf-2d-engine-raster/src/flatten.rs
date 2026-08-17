//! Stage 3 — flatten (Doc 01 §4).
//!
//! # This is the M2 stopgap, not the shipping implementation
//!
//! Doc 01 §4 is explicit that flattening must use Euler-spiral / parallel-curve
//! subdivision (Levien's method), because segment count drives the cost of
//! every stage after it. That is **T3.1**, which also requires a comparative
//! test proving it emits fewer segments than recursive subdivision at equal
//! tolerance — and then deleting the subdivision version.
//!
//! This is that subdivision version. It exists so M2 can rasterize curves at
//! all, it is correct within tolerance, and T3.1 measures against it before
//! removing it. It is deliberately simple rather than clever: making a
//! throwaway fast would only make the comparison flattering.
//!
//! # Tolerance is in device space
//!
//! Points are transformed first and subdivided afterwards, so the tolerance is
//! directly in device pixels: a path scaled 10× gets more segments and one
//! scaled 0.1× gets fewer, without the caller doing anything (Doc 01 §4).

use alloc::vec::Vec;

use otf_2d_engine_geom::{Affine, PathVerb, Point};

use crate::math::{ceil, sqrt};
use crate::segment::Segment;

/// Default flattening error, in device pixels.
///
/// A quarter pixel is the usual choice: coverage is quantised to 1/255, and a
/// deviation this small moves an antialiased edge by well under one code.
pub const DEFAULT_TOLERANCE: f64 = 0.25;

/// Turns transformed path geometry into device-space line segments.
///
/// Reused across draws and frames so a steady-state flatten allocates nothing
/// (I-9).
#[derive(Debug, Clone, Default)]
pub struct Flattener {
    segments: Vec<Segment>,
    /// Where the current subpath began, for the implicit closing edge.
    start: Option<Point>,
    current: Point,
}

impl Flattener {
    pub fn new() -> Flattener {
        Flattener::default()
    }

    /// Discards the previous draw's segments, keeping the allocation.
    pub fn reset(&mut self) {
        self.segments.clear();
        self.start = None;
        self.current = Point::new(0.0, 0.0);
    }

    /// The segments accumulated so far.
    #[inline]
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Bytes currently held.
    pub fn memory_usage(&self) -> usize {
        core::mem::size_of_val(&self.segments[..])
    }

    /// Appends one path, transformed into device space.
    ///
    /// Fills are implicitly closed: an unclosed subpath still gets its closing
    /// edge, because a fill is defined by winding and an open outline has none.
    pub fn add_path(&mut self, verbs: &[u8], points: &[f64], transform: Affine, tolerance: f64) {
        let tolerance = if tolerance.is_finite() && tolerance > 0.0 {
            tolerance
        } else {
            DEFAULT_TOLERANCE
        };
        let mut cursor = 0usize;
        let point = |at: usize| -> Option<Point> {
            let x = *points.get(at * 2)?;
            let y = *points.get(at * 2 + 1)?;
            Some(transform.transform_point(Point::new(x, y)))
        };

        for &verb in verbs {
            match PathVerb::from_u8(verb) {
                Some(PathVerb::MoveTo) => {
                    self.close_subpath();
                    let Some(p) = point(cursor) else { return };
                    cursor += 1;
                    self.start = Some(p);
                    self.current = p;
                }
                Some(PathVerb::LineTo) => {
                    let Some(p) = point(cursor) else { return };
                    cursor += 1;
                    self.line_to(p);
                }
                Some(PathVerb::QuadTo) => {
                    let (Some(c), Some(p)) = (point(cursor), point(cursor + 1)) else {
                        return;
                    };
                    cursor += 2;
                    self.quad_to(c, p, tolerance);
                }
                Some(PathVerb::CurveTo) => {
                    let (Some(c0), Some(c1), Some(p)) =
                        (point(cursor), point(cursor + 1), point(cursor + 2))
                    else {
                        return;
                    };
                    cursor += 3;
                    self.curve_to(c0, c1, p, tolerance);
                }
                Some(PathVerb::ClosePath) => self.close_subpath(),
                None => return,
            }
        }
        self.close_subpath();
    }

    fn line_to(&mut self, to: Point) {
        self.emit(self.current, to);
        self.current = to;
    }

    fn close_subpath(&mut self) {
        if let Some(start) = self.start.take()
            && (start.x != self.current.x || start.y != self.current.y)
        {
            self.emit(self.current, start);
        }
        self.start = None;
    }

    fn emit(&mut self, from: Point, to: Point) {
        let segment = Segment::new(from.x as f32, from.y as f32, to.x as f32, to.y as f32);
        if segment.is_finite() && !segment.is_horizontal() {
            // A horizontal edge crosses no scanline and so carries no winding.
            // Dropping it here saves stage 4 the work of binning it.
            self.segments.push(segment);
        }
    }

    /// Subdivides a quadratic at uniform parameter steps.
    ///
    /// The error of a chord against a quadratic is `d / (8n²)` where `d` is the
    /// length of the second difference, so the step count follows directly.
    fn quad_to(&mut self, control: Point, to: Point, tolerance: f64) {
        let from = self.current;
        let dx = from.x - 2.0 * control.x + to.x;
        let dy = from.y - 2.0 * control.y + to.y;
        let deviation = sqrt(dx * dx + dy * dy);
        let steps = steps_for(deviation / (8.0 * tolerance));

        for step in 1..=steps {
            let t = step as f64 / steps as f64;
            let u = 1.0 - t;
            let x = u * u * from.x + 2.0 * u * t * control.x + t * t * to.x;
            let y = u * u * from.y + 2.0 * u * t * control.y + t * t * to.y;
            self.line_to(Point::new(x, y));
        }
    }

    /// Subdivides a cubic at uniform parameter steps.
    ///
    /// The bound is `(3/4)·d / n²` over the larger of the two second
    /// differences.
    fn curve_to(&mut self, first: Point, second: Point, to: Point, tolerance: f64) {
        let from = self.current;
        let ax = from.x - 2.0 * first.x + second.x;
        let ay = from.y - 2.0 * first.y + second.y;
        let bx = first.x - 2.0 * second.x + to.x;
        let by = first.y - 2.0 * second.y + to.y;
        let deviation = sqrt((ax * ax + ay * ay).max(bx * bx + by * by));
        let steps = steps_for(0.75 * deviation / tolerance);

        for step in 1..=steps {
            let t = step as f64 / steps as f64;
            let u = 1.0 - t;
            let x = u * u * u * from.x
                + 3.0 * u * u * t * first.x
                + 3.0 * u * t * t * second.x
                + t * t * t * to.x;
            let y = u * u * u * from.y
                + 3.0 * u * u * t * first.y
                + 3.0 * u * t * t * second.y
                + t * t * t * to.y;
            self.line_to(Point::new(x, y));
        }
    }
}

/// Steps needed so the squared-error budget `ratio` is met, clamped to
/// something a pathological transform cannot turn into a hang.
fn steps_for(ratio: f64) -> u32 {
    /// A curve needing more than this is either degenerate or scaled past any
    /// surface anyone will render onto.
    const MAX_STEPS: u32 = 4096;

    if !ratio.is_finite() || ratio <= 0.0 {
        return 1;
    }
    let steps = ceil(sqrt(ratio));
    if steps >= MAX_STEPS as f64 {
        MAX_STEPS
    } else {
        (steps as u32).max(1)
    }
}

/// Clips a segment to an axis-aligned rectangle, preserving winding.
///
/// The `y` range is truncated by parameter — the part outside crosses no
/// scanline inside the rect — and `x` is clamped to the sides, which folds the
/// off-rect part onto the boundary. Folding rather than dropping is what keeps
/// a shape that extends past the clip filled up to its edge: winding depends
/// only on where an edge crosses a scanline, not on how far left it started.
///
/// Returns `None` when nothing of the segment survives.
///
/// This is exact, so a rectangular clip antialiases its own edges for free and
/// stage 6 needs no notion of clipping at all.
pub fn clip_segment(segment: Segment, x0: f32, y0: f32, x1: f32, y1: f32) -> Option<Segment> {
    if !segment.is_finite() || x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (top, bottom) = (segment.min_y(), segment.max_y());
    if bottom <= y0 || top >= y1 {
        return None;
    }

    let enter = top.max(y0);
    let exit = bottom.min(y1);
    if exit <= enter {
        return None;
    }

    // `x_at` works on the infinite line, so evaluating at the clipped `y`
    // bounds gives the sub-segment's endpoints directly.
    let x_enter = segment.x_at(enter as f64) as f32;
    let x_exit = segment.x_at(exit as f64) as f32;
    let (start, end) = if segment.y0 <= segment.y1 {
        ((x_enter, enter), (x_exit, exit))
    } else {
        ((x_exit, exit), (x_enter, enter))
    };

    Some(Segment::new(
        start.0.clamp(x0, x1),
        start.1,
        end.0.clamp(x0, x1),
        end.1,
    ))
}

/// Clips every segment to a rectangle, appending the survivors to `out`.
pub fn clip_segments(
    segments: &[Segment],
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    out: &mut Vec<Segment>,
) {
    out.clear();
    for &segment in segments {
        if let Some(clipped) = clip_segment(segment, x0, y0, x1, y1) {
            out.push(clipped);
        }
    }
}
