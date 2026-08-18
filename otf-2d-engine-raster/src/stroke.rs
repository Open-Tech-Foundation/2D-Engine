//! Stage 3 stroke expansion (T3.2).
//!
//! A stroke is the set of points within half the width of a path. Its boundary
//! is the path's *parallel curve* on either side, joined at corners and closed
//! by caps — so expanding a stroke means constructing that boundary as a
//! filled outline, and the fill rule does the rest (Doc 01 §4).
//!
//! # Why not a thick polyline
//!
//! The cheap way is to flatten the centre line and emit a quadrilateral per
//! chord. It is wrong twice over. The quadrilaterals meet at an angle, so the
//! outer edge of every bend is a visible chain of notches whose depth does not
//! shrink with the tolerance — it is set by the chord's turn, which the
//! flattener chose for the *centre* line and which the outer edge magnifies by
//! `1 + r·κ`. And overlapping quadrilaterals count their winding twice, so a
//! path that crosses itself, or bends tighter than its own half-width, fills
//! the overlap as though it were two strokes. `ci/invariants.sh` greps for the
//! word.
//!
//! # What this does instead
//!
//! Stage 3 has already fitted Euler spirals to the centre line, and the
//! parallel curve of an Euler spiral is an easy curve to know things about:
//! it has the same tangent direction everywhere, its arc length runs at
//! `1 − r·κ` of the centre line's, and its curvature is `κ/(1 − r·κ)`. Both
//! are closed forms in a curvature that is linear by construction, so where
//! the parallel curve turns over — an inflection, or the cusp where the
//! offset reaches the centre of curvature — is the root of a linear function
//! rather than a search.
//!
//! So each centre-line spiral is cut at those points, and each piece has its
//! own Euler spiral fitted to it by exactly the argument stage 3 uses for a
//! cubic: match the ends and their tangents, measure the gap as a graph over
//! the shared chord, halve if it is too wide. What comes out is a run of
//! spirals — the same thing the flattener produces — and the same chord
//! placement cuts it into segments.
//!
//! # The error budget
//!
//! Three approximations stack up, and each is given its share of the
//! tolerance: the centre-line fit takes [`CENTRE_SHARE`], the parallel-curve
//! fit takes what it measures, and the chords get the rest.

use alloc::vec::Vec;

use otf_2d_engine_geom::{PathVerb, Point, Vec2};
use otf_2d_engine_scene::{Cap, Join};

use crate::euler::EulerSeg;
use crate::flatten::{
    MAX_FIT_SHARE, MAX_TANGENT_ANGLE, Spiral, fit_spirals, inflection_cuts, place_chords, sub_cubic,
};
use crate::math::{abs, atan2, ceil, cos, sin};

/// Share of the tolerance the centre-line fit may take.
///
/// A parallel curve is exactly as far from the true parallel curve as its
/// centre line is from the true centre line, so this error carries through the
/// offset unchanged and has to be paid for once, here.
const CENTRE_SHARE: f64 = 0.25;

/// How a path is stroked, in the terms stage 3 works in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeSpec {
    /// Full width, in path units.
    pub width: f64,
    pub join: Join,
    pub start_cap: Cap,
    pub end_cap: Cap,
}

impl StrokeSpec {
    /// True when the stroke would paint nothing at all.
    pub fn is_degenerate(&self) -> bool {
        !self.width.is_finite() || self.width <= 0.0
    }
}

/// What the expansion hands back, in path coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Vertex {
    /// Starts a new closed contour at this point.
    Start(Point),
    Line(Point),
    /// Closes the contour back to its start.
    Close,
}

/// Turns paths into the outlines of their strokes.
///
/// Held across calls for its buffers: expansion is per subpath, and a frame of
/// strokes should not be a frame of allocations (I-9).
#[derive(Debug, Clone, Default)]
pub(crate) struct Stroker {
    /// The centre line of the subpath being expanded.
    centre: Vec<EulerSeg>,
    /// Parallel-curve spirals of the piece being emitted.
    run: Vec<Spiral>,
    /// Scratch for the centre-line fit.
    fitted: Vec<Spiral>,
    /// One closed contour, held back until it is known to be one.
    buffer: Vec<Vertex>,
}

impl Stroker {
    pub(crate) fn memory_usage(&self) -> usize {
        self.centre.capacity() * core::mem::size_of::<EulerSeg>()
            + (self.run.capacity() + self.fitted.capacity()) * core::mem::size_of::<Spiral>()
            + self.buffer.capacity() * core::mem::size_of::<Vertex>()
    }

    /// Expands `verbs`/`points` into the outline of its stroke, in path
    /// coordinates, and hands it to `emit` one vertex at a time.
    pub(crate) fn expand(
        &mut self,
        verbs: &[u8],
        points: &[f64],
        spec: &StrokeSpec,
        tolerance: f64,
        emit: &mut impl FnMut(Vertex),
    ) {
        if spec.is_degenerate() {
            return;
        }
        let centre_tolerance = tolerance * CENTRE_SHARE / MAX_FIT_SHARE;
        let mut cursor = 0usize;
        let point = |at: usize| -> Option<Point> {
            Some(Point::new(*points.get(at * 2)?, *points.get(at * 2 + 1)?))
        };
        let mut start = Point::new(0.0, 0.0);
        let mut current = start;
        let mut open = false;
        self.centre.clear();

        for &verb in verbs {
            match PathVerb::from_u8(verb) {
                Some(PathVerb::MoveTo) => {
                    if open {
                        self.finish_subpath(false, start, spec, tolerance, emit);
                    }
                    let Some(p) = point(cursor) else { return };
                    cursor += 1;
                    start = p;
                    current = p;
                    open = true;
                }
                Some(PathVerb::LineTo) => {
                    let Some(p) = point(cursor) else { return };
                    cursor += 1;
                    self.push_line(current, p);
                    current = p;
                }
                Some(PathVerb::QuadTo) => {
                    let (Some(c), Some(p)) = (point(cursor), point(cursor + 1)) else {
                        return;
                    };
                    cursor += 2;
                    let first = Point::new(
                        current.x + (2.0 / 3.0) * (c.x - current.x),
                        current.y + (2.0 / 3.0) * (c.y - current.y),
                    );
                    let second = Point::new(
                        p.x + (2.0 / 3.0) * (c.x - p.x),
                        p.y + (2.0 / 3.0) * (c.y - p.y),
                    );
                    self.push_curve([current, first, second, p], centre_tolerance);
                    current = p;
                }
                Some(PathVerb::CurveTo) => {
                    let (Some(c0), Some(c1), Some(p)) =
                        (point(cursor), point(cursor + 1), point(cursor + 2))
                    else {
                        return;
                    };
                    cursor += 3;
                    self.push_curve([current, c0, c1, p], centre_tolerance);
                    current = p;
                }
                Some(PathVerb::ClosePath) => {
                    if open {
                        self.push_line(current, start);
                        self.finish_subpath(true, start, spec, tolerance, emit);
                        current = start;
                        open = false;
                    }
                }
                None => return,
            }
        }
        if open {
            self.finish_subpath(false, start, spec, tolerance, emit);
        }
    }

    /// Appends one straight run of the centre line, if it has any length.
    ///
    /// A line is an Euler spiral that never turns, so there is one kind of
    /// element and not two.
    fn push_line(&mut self, from: Point, to: Point) {
        if from.x == to.x && from.y == to.y {
            return;
        }
        self.centre.push(EulerSeg::new(from, to, 0.0, 0.0));
    }

    /// Appends the spirals stage 3 fits to one cubic.
    fn push_curve(&mut self, curve: [Point; 4], tolerance: f64) {
        self.fitted.clear();
        let mut cuts = [0.0f64; 4];
        let pieces = inflection_cuts(&curve, &mut cuts);
        for window in 0..pieces - 1 {
            let piece = sub_cubic(&curve, cuts[window], cuts[window + 1]);
            fit_spirals(&piece, tolerance, &mut self.fitted);
        }
        for spiral in &self.fitted {
            if spiral.seg.arc_len > 0.0 {
                self.centre.push(spiral.seg);
            }
        }
    }

    /// Expands whatever centre line has been collected, then clears it.
    fn finish_subpath(
        &mut self,
        closed: bool,
        start: Point,
        spec: &StrokeSpec,
        tolerance: f64,
        emit: &mut impl FnMut(Vertex),
    ) {
        if self.centre.is_empty() {
            // A subpath with no length still paints, if the caps say so: the
            // stroke of a point is what the two caps enclose between them.
            dot(start, 0.5 * spec.width, spec, tolerance, emit);
        } else {
            self.expand_subpath(closed, spec, tolerance, emit);
        }
        self.centre.clear();
    }

    /// Expands one subpath's centre line, already collected in `centre`, and
    /// hands the outline to `emit`.
    ///
    /// `closed` says whether the subpath returns to its start, which decides
    /// whether the two sides are joined by caps into one contour or stand as
    /// two.
    fn expand_subpath(
        &mut self,
        closed: bool,
        spec: &StrokeSpec,
        tolerance: f64,
        emit: &mut impl FnMut(Vertex),
    ) {
        let radius = 0.5 * spec.width;
        if self.centre.is_empty() {
            return;
        }
        // The offset fit and the chords share what the centre-line fit left.
        let budget = tolerance * (1.0 - CENTRE_SHARE);
        if closed {
            // Two contours: the left side of the path as it runs, and the left
            // side of it run backwards, which is the right side. Non-zero
            // winding turns the pair into the ring between them.
            self.side(false, radius, budget, spec, emit);
            self.side(true, radius, budget, spec, emit);
        } else {
            // One contour: out along the left, round the end, back along what
            // is now the left of the reversed path, and round the start.
            let first = self.centre[0];
            let last = self.centre[self.centre.len() - 1];
            emit(Vertex::Start(offset_start(&first, radius)));
            self.walk_side(false, radius, budget, spec, emit);
            cap(
                last.eval(1.0),
                last.tangent(1.0),
                radius,
                spec.end_cap,
                budget,
                emit,
            );
            self.walk_side(true, radius, budget, spec, emit);
            cap(
                first.eval(0.0),
                Vec2::new(-first.tangent(0.0).x, -first.tangent(0.0).y),
                radius,
                spec.start_cap,
                budget,
                emit,
            );
            emit(Vertex::Close);
        }
    }

    /// Emits one closed side of a closed subpath.
    ///
    /// Held back until the whole side is known, because a side of which no
    /// part is boundary — the inside of a ring stroked wider than twice its
    /// own radius — is not a contour at all, and emitting one would punch a
    /// hole in the middle of a solid disc.
    fn side(
        &mut self,
        reverse: bool,
        radius: f64,
        budget: f64,
        spec: &StrokeSpec,
        emit: &mut impl FnMut(Vertex),
    ) {
        let first = if reverse {
            self.centre[self.centre.len() - 1].reversed()
        } else {
            self.centre[0]
        };
        let last = if reverse {
            self.centre[0].reversed()
        } else {
            self.centre[self.centre.len() - 1]
        };
        let mut buffer = core::mem::take(&mut self.buffer);
        buffer.clear();
        buffer.push(Vertex::Start(offset_start(&first, radius)));
        let any = self.walk_side(reverse, radius, budget, spec, &mut |vertex| {
            buffer.push(vertex)
        });
        // The corner where the subpath meets itself.
        join(
            last.eval(1.0),
            last.tangent(1.0),
            first.tangent(0.0),
            radius,
            spec.join,
            budget,
            &mut |vertex| buffer.push(vertex),
        );
        buffer.push(Vertex::Close);
        if any {
            for vertex in &buffer {
                emit(*vertex);
            }
        }
        self.buffer = buffer;
    }

    /// Emits the left parallel curve of the centre line, forwards or
    /// backwards, without opening or closing a contour.
    fn walk_side(
        &mut self,
        reverse: bool,
        radius: f64,
        budget: f64,
        spec: &StrokeSpec,
        emit: &mut impl FnMut(Vertex),
    ) -> bool {
        let count = self.centre.len();
        let mut any = false;
        let mut previous: Option<EulerSeg> = None;
        for index in 0..count {
            let element = if reverse {
                self.centre[count - 1 - index].reversed()
            } else {
                self.centre[index]
            };
            if let Some(before) = previous {
                join(
                    before.eval(1.0),
                    before.tangent(1.0),
                    element.tangent(0.0),
                    radius,
                    spec.join,
                    budget,
                    emit,
                );
            }
            any |= self.emit_offset(&element, radius, budget, emit);
            previous = Some(element);
        }
        any
    }

    /// Emits the left parallel curve of one spiral, ending on its far end.
    ///
    /// Returns whether any of it was boundary at all — see [`is_boundary`].
    fn emit_offset(
        &mut self,
        seg: &EulerSeg,
        radius: f64,
        budget: f64,
        emit: &mut impl FnMut(Vertex),
    ) -> bool {
        let mut cuts = [0.0f64; 4];
        let count = offset_cuts(seg, radius, &mut cuts);
        let mut any = false;
        for window in 0..count - 1 {
            let (from, to) = (cuts[window], cuts[window + 1]);
            if !is_boundary(seg, radius, from, to) {
                corner(
                    seg.offset_point(from, radius),
                    seg.tangent(from),
                    seg.offset_point(to, radius),
                    seg.tangent(to),
                    radius,
                    emit,
                );
                continue;
            }
            any = true;
            self.run.clear();
            fit_offset_run(seg, radius, from, to, budget, &mut self.run);
            place_chords(&self.run, |point| emit(Vertex::Line(point)));
            emit(Vertex::Line(seg.offset_point(to, radius)));
        }
        any
    }
}

/// Whether the parallel curve over `[from, to]` is part of the stroke's
/// boundary at all.
///
/// Past the centre of curvature — where `1 − radius·κ` turns negative — it is
/// not. The pen has already swept that ground from the other side of the bend,
/// so what looks like an edge there is inside the stroke, and following it
/// leaves a hole where two windings cancel. What is boundary instead is the
/// corner the two neighbouring pieces make when they run into each other,
/// which is what [`corner`] emits in its place: erode a rounded rectangle by
/// more than its corner radius and the corner comes back square, and this is
/// that, in general.
fn is_boundary(seg: &EulerSeg, radius: f64, from: f64, to: f64) -> bool {
    1.0 - radius * seg.curvature(0.5 * (from + to)) > 0.0
}

/// Emits the corner where two pieces of parallel curve meet across a stretch
/// that is not boundary, and then its far end.
///
/// The two run along the tangents at the ends of what was skipped, so where
/// they meet is where those lines cross. A limit stands behind it for the same
/// reason a miter join has one: as the two directions come into line the
/// crossing runs away to infinity, and a spike that long is not a corner.
fn corner(
    from: Point,
    entry: Vec2,
    to: Point,
    exit: Vec2,
    radius: f64,
    emit: &mut impl FnMut(Vertex),
) {
    let turn = entry.cross(exit);
    if turn != 0.0 {
        let gap = Vec2::new(to.x - from.x, to.y - from.y);
        let reach = gap.cross(exit) / turn;
        let limit = CORNER_LIMIT * (radius + gap.length());
        if reach.is_finite() && abs(reach) <= limit {
            let tip = Point::new(from.x + reach * entry.x, from.y + reach * entry.y);
            emit(Vertex::Line(tip));
        }
    }
    emit(Vertex::Line(to));
}

/// How far a [`corner`] may reach, in half-widths plus the gap it spans.
const CORNER_LIMIT: f64 = 16.0;

/// Where the left parallel curve of `seg` at `radius` turns over.
///
/// Two places, at most, and both are roots of a linear function because the
/// curvature is linear: where the curvature vanishes, which is an inflection
/// of the offset as much as of the centre line, and where `1 − radius·κ`
/// vanishes, which is the cusp at the centre of curvature. A spiral fit is
/// sound on neither, so both become ends of pieces.
fn offset_cuts(seg: &EulerSeg, radius: f64, out: &mut [f64; 4]) -> usize {
    let (start, end) = (seg.curvature(0.0), seg.curvature(1.0));
    let mut count = 1;
    out[0] = 0.0;
    let push = |at: f64, out: &mut [f64; 4], count: &mut usize| {
        if at > out[*count - 1] + CUT_EPSILON && at < 1.0 - CUT_EPSILON && *count < 3 {
            out[*count] = at;
            *count += 1;
        }
    };
    let mut roots = [f64::NAN; 2];
    roots[0] = crossing(start, end, 0.0);
    roots[1] = crossing(start, end, 1.0 / radius);
    if roots[0] > roots[1] {
        roots.swap(0, 1);
    }
    for root in roots {
        if root.is_finite() {
            push(root, out, &mut count);
        }
    }
    out[count] = 1.0;
    count + 1
}

/// Where a linear function running from `start` to `end` over `[0, 1]` takes
/// the value `target`, or `NaN` when it does not.
fn crossing(start: f64, end: f64, target: f64) -> f64 {
    if !target.is_finite() {
        return f64::NAN;
    }
    let slope = end - start;
    if slope == 0.0 {
        return f64::NAN;
    }
    (target - start) / slope
}

/// Parameter distance below which two cuts are the same cut.
const CUT_EPSILON: f64 = 1e-9;

/// Fits spirals to the left parallel curve of `seg` over `[from, to]`.
///
/// The same halving search stage 3 runs over a cubic, over a curve whose
/// tangents and curvature are known in closed form instead of computed from
/// control points.
fn fit_offset_run(
    seg: &EulerSeg,
    radius: f64,
    from: f64,
    to: f64,
    budget: f64,
    out: &mut Vec<Spiral>,
) {
    let mut start_index = 0u32;
    let mut depth = 0u32;
    loop {
        let width = 1.0 / (1u32 << depth) as f64;
        let at = start_index as f64 * width;
        if at >= 1.0 - CUT_EPSILON {
            break;
        }
        let a = from + (to - from) * at;
        let b = from + (to - from) * (at + width).min(1.0);
        let fitted = fit_offset(seg, radius, a, b);
        let accept = fitted.is_none_or(|(_, error)| error <= budget * OFFSET_BUDGET);
        if accept || depth >= MAX_OFFSET_DEPTH {
            if let Some((spiral, error)) = fitted {
                out.push(Spiral::new(spiral, budget, error));
            }
            start_index += 1;
            while depth > 0 && start_index.is_multiple_of(2) {
                start_index /= 2;
                depth -= 1;
            }
        } else {
            start_index *= 2;
            depth += 1;
        }
    }
}

/// Share of the budget a parallel-curve fit may take before the piece is
/// halved.
///
/// Lower than stage 3's, and not priced against the segments it would save:
/// what a stroke costs in segments is set by its two sides and its joins, and
/// a piece here is short enough already that a halving is cheap next to
/// getting the outline's shape wrong.
const OFFSET_BUDGET: f64 = 0.25;

/// How far the halving may go before a piece is taken as it stands.
const MAX_OFFSET_DEPTH: u32 = 8;

/// One spiral fitted to the left parallel curve over `[a, b]`, with how far it
/// strays from it.
fn fit_offset(seg: &EulerSeg, radius: f64, a: f64, b: f64) -> Option<(EulerSeg, f64)> {
    let start = seg.offset_point(a, radius);
    let end = seg.offset_point(b, radius);
    let chord = Vec2::new(end.x - start.x, end.y - start.y);
    let extent = chord.length_squared();
    if !extent.is_finite() || extent <= 0.0 {
        return None;
    }
    // Past the cusp the parallel curve runs backwards along the tangent. The
    // cuts put the cusp on a boundary, so the sign holds over the whole piece.
    let direction = if 1.0 - radius * seg.curvature(0.5 * (a + b)) < 0.0 {
        -1.0
    } else {
        1.0
    };
    let entry = scaled(seg.tangent(a), direction);
    let exit = scaled(seg.tangent(b), direction);
    let th0 = atan2(entry.cross(chord), entry.dot(chord));
    let th1 = atan2(chord.cross(exit), chord.dot(exit));
    if abs(th0) > MAX_TANGENT_ANGLE || abs(th1) > MAX_TANGENT_ANGLE {
        return Some((EulerSeg::new(start, end, th0, th1), f64::INFINITY));
    }
    let spiral = EulerSeg::new(start, end, th0, th1);
    let error = offset_error(seg, radius, a, b, &spiral, th0);
    Some((spiral, error))
}

fn scaled(v: Vec2, by: f64) -> Vec2 {
    Vec2::new(v.x * by, v.y * by)
}

/// How far `spiral` strays from the parallel curve it was fitted to.
///
/// Read the same way stage 3 reads a cubic: both curves run one way along
/// their shared chord, so the point to compare against is the one directly
/// across, and the gap is what is left after the chord direction is taken out.
/// Here the crossing is found by Newton on the centre line's own parameter,
/// whose derivative along the chord is `arc·(1 − radius·κ)·(T·chord)` — every
/// term already to hand.
fn offset_error(seg: &EulerSeg, radius: f64, a: f64, b: f64, spiral: &EulerSeg, th0: f64) -> f64 {
    const SAMPLES: [f64; 3] = [0.25, 0.5, 0.75];
    let chord = spiral.chord;
    let length = chord.length();
    if !length.is_finite() || length <= 0.0 {
        return f64::INFINITY;
    }
    // The tangent has to stay on one side of the chord for the crossing to be
    // unique, and it is a quadratic in the centre line's parameter, so its
    // extreme is one evaluation rather than a search.
    if turn_beyond(seg, a, b, th0) {
        return f64::INFINITY;
    }
    let origin = spiral.p0;
    let along = |p: Point| (p.x - origin.x) * chord.x + (p.y - origin.y) * chord.y;
    let mut worst: f64 = 0.0;
    for at in SAMPLES {
        let on_spiral = spiral.eval(at);
        let target = along(on_spiral);
        let mut s = a + (b - a) * at;
        for _ in 0..OFFSET_STEPS {
            let here = along(seg.offset_point(s, radius));
            let rate = seg.arc_len * (1.0 - radius * seg.curvature(s)) * seg.tangent(s).dot(chord);
            if !rate.is_finite() || rate == 0.0 {
                break;
            }
            s = (s - (here - target) / rate).clamp(a, b);
        }
        let on_curve = seg.offset_point(s, radius);
        let offset = Vec2::new(on_curve.x - on_spiral.x, on_curve.y - on_spiral.y);
        worst = worst.max(abs(offset.cross(chord)) / length);
    }
    let error = worst * OFFSET_SAFETY;
    if error.is_finite() {
        error
    } else {
        f64::INFINITY
    }
}

/// Newton steps spent crossing from a point on the fitted spiral to the point
/// of the parallel curve across from it.
const OFFSET_STEPS: usize = 2;

/// What [`offset_error`] multiplies its measurement by, for the same reason
/// stage 3's estimate is scaled: three samples of a smooth gap are an
/// estimate, not a bound.
const OFFSET_SAFETY: f64 = 1.4;

/// Whether the tangent leaves the half-turn either side of the chord anywhere
/// on the piece, which is where the reading above stops meaning anything.
fn turn_beyond(seg: &EulerSeg, a: f64, b: f64, th0: f64) -> bool {
    // `theta` is the tangent angle against the centre line's chord; the offset
    // chord is a fixed angle away from that, and `th0` says how far.
    let base = -th0 - seg.params.theta(a);
    let mut worst = abs(seg.params.theta(b) + base);
    let (k0, k1) = (seg.params.k0, seg.params.k1);
    if k1 != 0.0 {
        let vertex = 0.5 - k0 / k1;
        if vertex > a && vertex < b {
            worst = worst.max(abs(seg.params.theta(vertex) + base));
        }
    }
    worst > MAX_TANGENT_ANGLE
}

/// The stroke of a subpath with no length: what the two caps enclose between
/// them, around a point with no direction of its own.
///
/// Round caps make a disc, square caps a square, and butt caps nothing at all
/// — which is what every other renderer does, because a butt cap has no width
/// to give.
fn dot(at: Point, radius: f64, spec: &StrokeSpec, tolerance: f64, emit: &mut impl FnMut(Vertex)) {
    if !radius.is_finite() || radius <= 0.0 {
        return;
    }
    // The path has no tangent, so one is chosen; every cap shape here is
    // symmetric enough that the choice cannot be seen.
    let forward = Vec2::new(1.0, 0.0);
    match (spec.start_cap, spec.end_cap) {
        (Cap::Butt, Cap::Butt) => {}
        (start, end) => {
            let left = Point::new(at.x, at.y + radius);
            emit(Vertex::Start(left));
            cap(at, forward, radius, end, tolerance, emit);
            cap(
                at,
                Vec2::new(-forward.x, -forward.y),
                radius,
                start,
                tolerance,
                emit,
            );
            emit(Vertex::Close);
        }
    }
}

/// The point the left parallel curve of `seg` starts at.
fn offset_start(seg: &EulerSeg, radius: f64) -> Point {
    seg.offset_point(0.0, radius)
}

/// Emits the join carrying the outline from one piece's parallel curve to the
/// next's, around the point where they meet.
///
/// The join is emitted whichever way the path turns. On the outside of a bend
/// it is the shape the stroke actually has; on the inside it is a small loop
/// the two parallel curves have already covered, and non-zero winding counts
/// it as covered either way. Deciding which side it is on, and leaving the
/// inside out, would cost a branch per corner to save geometry the fill does
/// not charge for.
fn join(
    corner: Point,
    entry: Vec2,
    exit: Vec2,
    radius: f64,
    style: Join,
    budget: f64,
    emit: &mut impl FnMut(Vertex),
) {
    let turn = atan2(entry.cross(exit), entry.dot(exit));
    if abs(turn) <= SMOOTH_TURN {
        return;
    }
    let from = Point::new(corner.x - radius * entry.y, corner.y + radius * entry.x);
    let to = Point::new(corner.x - radius * exit.y, corner.y + radius * exit.x);
    match style {
        Join::Bevel => emit(Vertex::Line(to)),
        Join::Round => {
            arc(corner, from, radius, turn, budget, emit);
            emit(Vertex::Line(to));
        }
        Join::Miter { limit } => {
            // The spike is `1/sin(θ/2)` half-widths long, where `θ` is the
            // angle the two directions leave between them.
            let half = 0.5 * (core::f64::consts::PI - abs(turn));
            let sine = sin(half);
            let spike = if sine > 0.0 {
                1.0 / sine
            } else {
                f64::INFINITY
            };
            if spike <= limit as f64 {
                let tip = Point::new(
                    corner.x - radius * spike * (entry.y + exit.y) / (2.0 * cos(0.5 * turn)),
                    corner.y + radius * spike * (entry.x + exit.x) / (2.0 * cos(0.5 * turn)),
                );
                if tip.x.is_finite() && tip.y.is_finite() {
                    emit(Vertex::Line(tip));
                }
            }
            emit(Vertex::Line(to));
        }
    }
}

/// Turn below which a corner is not a corner.
///
/// Consecutive spirals within one curve share a tangent exactly, so this only
/// has to catch what rounding leaves behind.
const SMOOTH_TURN: f64 = 1e-12;

/// Emits the cap closing one end of an open stroke, from the left side to the
/// right.
///
/// `forward` points the way the path is going at that end, so the cap is
/// always built as though it were the far end.
fn cap(
    end: Point,
    forward: Vec2,
    radius: f64,
    style: Cap,
    budget: f64,
    emit: &mut impl FnMut(Vertex),
) {
    let left = Point::new(end.x - radius * forward.y, end.y + radius * forward.x);
    let right = Point::new(end.x + radius * forward.y, end.y - radius * forward.x);
    match style {
        Cap::Butt => emit(Vertex::Line(right)),
        Cap::Square => {
            emit(Vertex::Line(Point::new(
                left.x + radius * forward.x,
                left.y + radius * forward.y,
            )));
            emit(Vertex::Line(Point::new(
                right.x + radius * forward.x,
                right.y + radius * forward.y,
            )));
            emit(Vertex::Line(right));
        }
        Cap::Round => {
            arc(end, left, radius, -core::f64::consts::PI, budget, emit);
            emit(Vertex::Line(right));
        }
    }
}

/// Emits the interior of a circular arc about `centre`, starting at `from` and
/// sweeping by `turn`, without its final point.
///
/// A circular arc is an Euler spiral whose curvature does not change, so the
/// chord placement that cuts every other curve cuts this one too.
fn arc(
    centre: Point,
    from: Point,
    radius: f64,
    turn: f64,
    budget: f64,
    emit: &mut impl FnMut(Vertex),
) {
    if !radius.is_finite() || radius <= 0.0 || !turn.is_finite() {
        return;
    }
    // A sweep of more than a right angle is outside the range a single spiral
    // fit is sound over, so it is taken in equal bites that are not.
    let pieces = ceil(abs(turn) / core::f64::consts::FRAC_PI_2).max(1.0);
    let step = turn / pieces;
    let (mut sx, mut sy) = (from.x - centre.x, from.y - centre.y);
    for _ in 0..pieces as usize {
        let (sn, cs) = (sin(step), cos(step));
        let (nx, ny) = (sx * cs - sy * sn, sx * sn + sy * cs);
        let to = Point::new(centre.x + nx, centre.y + ny);
        let seg = EulerSeg::new(
            Point::new(centre.x + sx, centre.y + sy),
            to,
            0.5 * step,
            0.5 * step,
        );
        let spiral = [Spiral::new(seg, budget, 0.0)];
        place_chords(&spiral, |point| emit(Vertex::Line(point)));
        emit(Vertex::Line(to));
        (sx, sy) = (nx, ny);
    }
}
