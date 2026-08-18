//! Stage 3 — flatten (Doc 01 §4).
//!
//! Curves in, line segments out, within a stated error. Segment count drives
//! the cost of binning, strip generation and fine rasterization, so this stage
//! spends real work to keep it low: it fits Euler spirals to each curve and
//! flattens those (D-11, T3.1), rather than subdividing until a flatness test
//! passes. The spiral machinery lives in [`crate::euler`]; this module is the
//! driver that decides where spirals go and where their line segments land.
//!
//! # The shape of one curve
//!
//! 1. **Split at inflections and cusps.** Between them the curvature keeps its
//!    sign, which is what makes a spiral fit well and what keeps a chord from
//!    spanning an S-bend it cannot represent. The split points are roots of a
//!    quadratic, so this costs almost nothing.
//! 2. **Fit spirals.** Halve until the spiral through a piece's endpoints and
//!    end tangents is within its share of the tolerance, measured by
//!    [`spiral_error`].
//! 3. **Count once, place once.** Each spiral reports how many segments it
//!    needs as a real number; the curve's total is rounded up once, and the
//!    segments are then distributed across the spirals by that budget. Rounding
//!    per spiral instead would cost up to one segment per spiral.
//!
//! # Tolerance is in device space
//!
//! The caller states the error in device pixels. Flattening happens in the
//! path's own coordinates — that is where stroke expansion and dashing will
//! also happen in T3.2 and T3.3 — so the tolerance is divided by
//! [`Affine::max_scale`], the most any direction can be stretched by. A path
//! scaled 10× gets more segments and one scaled 0.1× gets fewer, without the
//! caller doing anything (Doc 01 §4).

use alloc::vec::Vec;

use otf_2d_engine_geom::{Affine, PathVerb, Point, Vec2};

use crate::euler::{Density, EulerSeg};
use crate::math::{abs, atan2, ceil, sqrt};
use crate::segment::Segment;
use crate::stroke::{StrokeSpec, Stroker, Vertex};

/// Default flattening error, in device pixels.
///
/// A quarter pixel is the usual choice: coverage is quantised to 1/255, and a
/// deviation this small moves an antialiased edge by well under one code.
pub const DEFAULT_TOLERANCE: f64 = 0.25;

/// Largest angle, in radians, a spiral's end tangent may make with its chord.
///
/// A cap is needed because the fit is only sound while the curve stays a
/// well-behaved arc, and because [`crate::euler`]'s series has a range. This
/// value is just over π/4, which is what the cubic approximation of a quarter
/// circle asks for — the most common curve in the world, and one that would
/// otherwise be split in half for nothing.
pub(crate) const MAX_TANGENT_ANGLE: f64 = 0.9;

/// What [`spiral_error`] multiplies its measurement by.
///
/// A handful of samples cannot be relied on to land on the worst point of the
/// gap between two curves. `fit_quality` measures the shortfall against dense
/// sampling — over random Béziers and over arcs of every turn — and asserts it
/// stays inside this factor; the worst seen is a quarter, so this is that with
/// room over it.
const FIT_SAFETY: f64 = 1.4;

/// Share of the tolerance a spiral fit may take without further argument.
///
/// Whatever the fit spends, the line placement cannot, and segment count goes
/// as the inverse square root of what is left. Under a tenth that is worth
/// nothing to argue about, so the piece is kept as it stands; above it the
/// question is put to [`worth_halving`], which prices the split rather than
/// assuming it.
const FIT_BUDGET: f64 = 0.1;

/// The least of the tolerance line placement may be left with.
///
/// The floor stops a spiral that fits badly — one accepted at the depth limit,
/// with an error estimate of infinity — from asking for an unbounded number of
/// segments. Nothing else may reach it: [`worth_halving`] refuses any fit that
/// would, because a floor under the chord budget is a promise the sum cannot
/// keep.
const LINE_BUDGET_FLOOR: f64 = 0.4;

/// How far the halving in [`Flattener::fit_spirals`] may go before a piece is
/// accepted as it stands.
///
/// A curve that has not converged after ten halvings is degenerate — a cusp
/// sitting exactly on a split point, or coordinates far enough apart that the
/// subdivision is numerical noise. Accepting is right: the error there is
/// bounded by the piece being 1/1024 of the curve, and looping is not.
const MAX_SPLIT_DEPTH: u32 = 10;

/// One fitted spiral and the segment budget it asks for.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Spiral {
    pub(crate) seg: EulerSeg,
    density: Density,
    /// Segments this spiral needs, before the curve's total is rounded.
    value: f64,
    /// What the density was built to hold to, once the fit took its share.
    line_tolerance: f64,
}

/// Turns path geometry into device-space line segments.
///
/// Reused across draws and frames so a steady-state flatten allocates nothing
/// (I-9).
#[derive(Debug, Clone, Default)]
pub struct Flattener {
    segments: Vec<Segment>,
    /// Spirals fitted to the curve being flattened, in order.
    spirals: Vec<Spiral>,
    /// Where the current subpath began, for the implicit closing edge.
    start: Option<Point>,
    current: Point,
    current_device: Point,
    transform: Affine,
    /// Last, because a fill never touches it: the buffers a stroke needs are
    /// several times the size of everything above them, and in front of them
    /// they push the fields that *are* read on every emitted point along.
    stroker: Stroker,
}

impl Flattener {
    pub fn new() -> Flattener {
        Flattener {
            transform: Affine::IDENTITY,
            ..Flattener::default()
        }
    }

    /// Discards the previous draw's segments, keeping the allocation.
    pub fn reset(&mut self) {
        self.segments.clear();
        self.spirals.clear();
        self.start = None;
        self.current = Point::new(0.0, 0.0);
        self.current_device = Point::new(0.0, 0.0);
    }

    /// The segments accumulated so far.
    #[inline]
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Bytes currently held.
    pub fn memory_usage(&self) -> usize {
        self.stroker.memory_usage()
            + core::mem::size_of_val(&self.segments[..])
            + core::mem::size_of_val(&self.spirals[..])
    }

    /// Appends one path, transformed into device space.
    ///
    /// `tolerance` is the largest distance the output may stray from the true
    /// curve, in device pixels.
    ///
    /// Fills are implicitly closed: an unclosed subpath still gets its closing
    /// edge, because a fill is defined by winding and an open outline has none.
    /// Appends the outline of the stroke of one path.
    ///
    /// The outline is built in path coordinates and transformed on the way
    /// out, exactly as a fill is, so a stroke is as wide as the transform
    /// makes it: a path scaled twice over is stroked twice as thick, and one
    /// scaled unevenly is stroked by an ellipse. That is what a stroke *is* —
    /// the shape swept by a pen carried along the path — and it is what SVG
    /// and CSS mean by it too.
    pub fn add_stroke(
        &mut self,
        verbs: &[u8],
        points: &[f64],
        transform: Affine,
        tolerance: f64,
        spec: &StrokeSpec,
    ) {
        let local_tolerance = self.begin(transform, tolerance);
        // Out of `self` while the outline is being built, so that emitting can
        // borrow the flattener and the stroker's buffers at the same time.
        let mut stroker = core::mem::take(&mut self.stroker);
        stroker.expand(
            verbs,
            points,
            spec,
            local_tolerance,
            &mut |vertex| match vertex {
                Vertex::Start(point) => {
                    self.close_subpath();
                    self.start = Some(point);
                    self.move_to(point);
                }
                Vertex::Line(point) => self.line_to(point),
                Vertex::Close => self.close_subpath(),
            },
        );
        self.close_subpath();
        self.stroker = stroker;
    }

    /// Sets the transform and returns the tolerance in path coordinates.
    fn begin(&mut self, transform: Affine, tolerance: f64) -> f64 {
        let device_tolerance = if tolerance.is_finite() && tolerance > 0.0 {
            tolerance
        } else {
            DEFAULT_TOLERANCE
        };
        let scale = transform.max_scale();
        self.transform = transform;
        // A degenerate transform collapses the path to a point or a line, and
        // no amount of flattening changes what it looks like: one segment per
        // curve is exactly right, and an infinite tolerance says so.
        if scale.is_finite() && scale > 0.0 {
            device_tolerance / scale
        } else {
            f64::INFINITY
        }
    }

    pub fn add_path(&mut self, verbs: &[u8], points: &[f64], transform: Affine, tolerance: f64) {
        let local_tolerance = self.begin(transform, tolerance);
        let mut cursor = 0usize;
        let point = |at: usize| -> Option<Point> {
            let x = *points.get(at * 2)?;
            let y = *points.get(at * 2 + 1)?;
            Some(Point::new(x, y))
        };

        for &verb in verbs {
            match PathVerb::from_u8(verb) {
                Some(PathVerb::MoveTo) => {
                    self.close_subpath();
                    let Some(p) = point(cursor) else { return };
                    cursor += 1;
                    self.start = Some(p);
                    self.move_to(p);
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
                    // Degree elevation, so one curve path serves both. A
                    // quadratic is a cubic; keeping a second implementation of
                    // every step for it would buy nothing but drift.
                    let from = self.current;
                    let first = Point::new(
                        from.x + (2.0 / 3.0) * (c.x - from.x),
                        from.y + (2.0 / 3.0) * (c.y - from.y),
                    );
                    let second = Point::new(
                        p.x + (2.0 / 3.0) * (c.x - p.x),
                        p.y + (2.0 / 3.0) * (c.y - p.y),
                    );
                    self.curve_to(first, second, p, local_tolerance);
                }
                Some(PathVerb::CurveTo) => {
                    let (Some(c0), Some(c1), Some(p)) =
                        (point(cursor), point(cursor + 1), point(cursor + 2))
                    else {
                        return;
                    };
                    cursor += 3;
                    self.curve_to(c0, c1, p, local_tolerance);
                }
                Some(PathVerb::ClosePath) => self.close_subpath(),
                None => return,
            }
        }
        self.close_subpath();
    }

    fn move_to(&mut self, to: Point) {
        self.current = to;
        self.current_device = self.transform.transform_point(to);
    }

    fn line_to(&mut self, to: Point) {
        let device = self.transform.transform_point(to);
        self.emit(device);
        self.current = to;
    }

    fn close_subpath(&mut self) {
        if let Some(start) = self.start.take()
            && (start.x != self.current.x || start.y != self.current.y)
        {
            self.line_to(start);
        }
        self.start = None;
    }

    /// Appends one device-space segment from the current point to `device`.
    fn emit(&mut self, device: Point) {
        let from = self.current_device;
        let segment = Segment::new(
            from.x as f32,
            from.y as f32,
            device.x as f32,
            device.y as f32,
        );
        self.current_device = device;
        if segment.is_finite() && !segment.is_horizontal() {
            // A horizontal edge crosses no scanline and so carries no winding.
            // Dropping it here saves stage 4 the work of binning it.
            self.segments.push(segment);
        }
    }

    /// Flattens one cubic, in path coordinates, at a path-space `tolerance`.
    fn curve_to(&mut self, first: Point, second: Point, to: Point, tolerance: f64) {
        let curve = [self.current, first, second, to];
        if !tolerance.is_finite() || tolerance <= 0.0 {
            self.line_to(to);
            return;
        }

        // Between inflections the curvature keeps its sign. Both the spiral fit
        // and the chord-placement model rely on that, so the splits come first
        // and every piece boundary is a vertex of the output.
        let mut bounds = [0.0f64; 4];
        let piece_count = inflection_cuts(&curve, &mut bounds) - 1;

        // Moved out of `self` so that emitting — which needs the whole
        // flattener — and reading the spirals do not borrow it at once. The
        // allocation goes straight back at the end.
        let mut spirals = core::mem::take(&mut self.spirals);
        spirals.clear();
        let mut piece_ends = [0usize; 4];
        for piece in 0..piece_count {
            let sub = sub_cubic(&curve, bounds[piece], bounds[piece + 1]);
            fit_spirals(&sub, tolerance, &mut spirals);
            piece_ends[piece] = spirals.len();
        }

        let total: f64 = spirals.iter().map(|spiral| spiral.value).sum();
        if !total.is_finite() || total <= 0.0 {
            self.spirals = spirals;
            self.line_to(to);
            return;
        }
        // One rounding for the whole curve, not one per spiral.
        let step = total / ceil(total - COUNT_EPSILON).max(1.0);

        let mut first_of_piece = 0usize;
        for piece in 0..piece_count {
            let range = &spirals[first_of_piece..piece_ends[piece]];
            first_of_piece = piece_ends[piece];
            if !range.is_empty() {
                let piece_total: f64 = range.iter().map(|spiral| spiral.value).sum();
                let count = tighten(range, ceil(piece_total / step - COUNT_EPSILON).max(1.0));
                walk(range, count, |index, at| {
                    let point = range[index].seg.eval(at);
                    let device = self.transform.transform_point(point);
                    self.emit(device);
                });
            }
            // The piece's own endpoint, exactly, rather than whatever the
            // spiral evaluates to there.
            let end = eval_cubic(&curve, bounds[piece + 1]);
            let device = self.transform.transform_point(end);
            self.emit(device);
        }
        self.spirals = spirals;
        self.current = to;
    }
}

/// Cuts `[0, 1]` where the curvature vanishes, and returns how many boundaries
/// came out — one more than the number of pieces.
///
/// Between inflections the curvature keeps its sign. Both the spiral fit and
/// the chord-placement model rely on that, so the splits come first and every
/// piece boundary is a vertex of the output.
pub(crate) fn inflection_cuts(curve: &[Point; 4], out: &mut [f64; 4]) -> usize {
    let mut count = 1;
    out[0] = 0.0;
    for root in curvature_roots(curve) {
        if root > out[count - 1] + PIECE_EPSILON && root < 1.0 - PIECE_EPSILON && count < 3 {
            out[count] = root;
            count += 1;
        }
    }
    out[count] = 1.0;
    count + 1
}

/// Fits spirals to one inflection-free piece and appends them to `out`.
///
/// The search is the usual halving with a step back up, with two things on top
/// of it: a piece is kept when halving it would not be worth the fits
/// ([`worth_halving`]), and when it is not kept, the error just measured says
/// how many halvings it is short by rather than costing a rejected fit at
/// every level on the way down ([`shortfall_halvings`]).
pub(crate) fn fit_spirals(curve: &[Point; 4], tolerance: f64, out: &mut Vec<Spiral>) {
    let fit_tolerance = tolerance * FIT_BUDGET;
    let mut start_index = 0u32;
    let mut depth = 0u32;
    loop {
        let width = 1.0 / (1u32 << depth) as f64;
        let t0 = start_index as f64 * width;
        if t0 >= 1.0 - PIECE_EPSILON {
            break;
        }
        let t1 = (t0 + width).min(1.0);
        let sub = sub_cubic(curve, t0, t1);
        // A piece that has become straight enough is done, whatever a spiral
        // would say about it. Without this a cusp — where no spiral fits and
        // the error estimate keeps asking for another halving — costs the
        // depth limit in fits every time, and the pieces around it have long
        // since stopped being distinguishable from their chords.
        let bow = flatness(&sub) * FLATNESS_MARGIN;
        let fitted = if bow <= fit_tolerance {
            fit_spiral_only(&sub).map(|seg| (seg, bow))
        } else {
            fit_spiral(&sub)
        };
        let accept = match fitted {
            Some((seg, error)) => error <= fit_tolerance || !worth_halving(&seg, error, tolerance),
            // A piece whose two ends are the same point has no chord to fit a
            // spiral to, and yet it is not nothing: a cubic that closes on
            // itself is a loop, and the loop is the shape. Halving it is what
            // finds the two halves that do have chords. Only when the bow has
            // gone with the chord is the piece really a point.
            None => bow <= fit_tolerance,
        };
        if accept || depth >= MAX_SPLIT_DEPTH {
            if let Some((seg, error)) = fitted {
                out.push(Spiral::new(seg, tolerance, error));
            }
            start_index += 1;
            while depth > 0 && start_index.is_multiple_of(2) {
                start_index /= 2;
                depth -= 1;
            }
        } else {
            // How many halvings this piece is short by, rather than one at a
            // time. A spiral fit is a fourth-order approximation — halve the
            // piece and the gap falls by sixteen — so the error just measured
            // says how far down the answer is, and walking there one level at
            // a time pays for a rejected fit at every level on the way.
            let halvings =
                shortfall_halvings(fitted.map_or(f64::INFINITY, |(_, e)| e), fit_tolerance)
                    .min(MAX_SPLIT_DEPTH - depth);
            start_index <<= halvings;
            depth += halvings;
        }
    }
}

/// How many halvings it takes to bring `error` down to `target`, from the
/// fourth-order convergence of the fit — never fewer than one, and rounded
/// down so the search still walks the last level itself rather than
/// overshooting into pieces nobody asked for.
///
/// The logarithm is the exponent field of the ratio, which is what
/// `floor(log2)` means; no library call is involved.
fn shortfall_halvings(error: f64, target: f64) -> u32 {
    let ratio = error / target;
    if !ratio.is_finite() || ratio <= 1.0 {
        return 1;
    }
    let exponent = ((ratio.to_bits() >> 52) & 0x7ff) as i64 - 1023;
    ((exponent / 4) as u32).max(1)
}

/// The error left for chord placement once the fit has taken its share.
///
/// The two errors add in the worst case — a fit that bulges one way and a
/// chord that cuts the same way — so the line placement gets what the fit did
/// not spend, and no more.
pub(crate) fn line_budget(tolerance: f64, error: f64) -> f64 {
    (tolerance - error).max(tolerance * LINE_BUDGET_FLOOR)
}

/// Whether halving this piece can buy enough segments to be worth the fits.
///
/// A fit that spends more of the tolerance than [`FIT_BUDGET`] allows is not
/// wrong — the chord placement is told what is left and stays inside the
/// budget either way — it is only *more segments* than a tighter fit would
/// need. So the question the split turns on is not how large the error is but
/// what it costs, and that is a number this stage can read directly: the
/// density integral is the segment count, and halving cannot do better than
/// drive the fit error to nothing and hand the whole budget to the chords.
///
/// Asking it this way is what keeps large circular arcs from being torn up.
/// A quarter of a big circle is a cubic that no spiral matches to a tenth of a
/// pixel — the cubic is not a circular arc, and the gap grows with the radius —
/// so a fixed fraction of the tolerance splits it into four every time, at five
/// fits a cubic, and gets 2.6% of its segments back for them. Measured over 256
/// circles at 1080p, pricing the split rather than assuming it took this stage
/// from 5.2 ms a frame to 1.0 ms.
fn worth_halving(seg: &EulerSeg, error: f64, tolerance: f64) -> bool {
    // A fit that has eaten this much of the tolerance leaves the chords too
    // little to work with whatever the count says, and it is the only case
    // where [`LINE_BUDGET_FLOOR`] would bind — which it must not, because a
    // floor under the chord budget is a promise the sum cannot keep.
    if error > tolerance * MAX_FIT_SHARE {
        return true;
    }
    let now = Density::new(seg, line_budget(tolerance, error)).value();
    let best = Density::new(seg, tolerance).value();
    now > best * (1.0 + SPLIT_GAIN_FRACTION)
}

/// The largest share of the tolerance a fit may take and still be considered
/// on its segment count rather than halved outright.
pub(crate) const MAX_FIT_SHARE: f64 = 0.5;

/// What share of a piece's own segment count halving has to save before it is
/// worth the fits it costs.
///
/// Measured, not guessed: at a tenth the flattener keeps every split that pays
/// and drops the ones that do not, and both the segment count against
/// recursive subdivision and the number of individual curves it loses on are
/// better than at any tighter setting.
const SPLIT_GAIN_FRACTION: f64 = 0.10;

impl Spiral {
    /// A spiral to be cut into chords, with `error` already spent on getting
    /// it there.
    pub(crate) fn new(seg: EulerSeg, tolerance: f64, error: f64) -> Spiral {
        let line_tolerance = line_budget(tolerance, error);
        let density = Density::new(&seg, line_tolerance);
        Spiral {
            seg,
            density,
            value: density.value(),
            line_tolerance,
        }
    }
}

/// Cuts a run of spirals into chords, and calls `visit` with each interior
/// vertex. The ends are the caller's, who knows them exactly.
pub(crate) fn place_chords(spirals: &[Spiral], mut visit: impl FnMut(Point)) {
    let total: f64 = spirals.iter().map(|spiral| spiral.value).sum();
    if !total.is_finite() || total <= 0.0 {
        return;
    }
    let count = tighten(spirals, ceil(total - COUNT_EPSILON).max(1.0));
    walk(spirals, count, |index, at| {
        visit(spirals[index].seg.eval(at))
    });
}

/// Calls `visit` with the spiral and parameter of every interior vertex, given
/// that the run of spirals is to be cut into `count` line segments.
///
/// The budget is shared across the run rather than rounded per spiral, so a
/// chord may span a spiral boundary. That is what keeps the count near the
/// theoretical minimum — a run of twenty spirals worth two segments emits two,
/// not twenty.
fn walk(spirals: &[Spiral], count: f64, mut visit: impl FnMut(usize, f64)) {
    let total: f64 = spirals.iter().map(|spiral| spiral.value).sum();
    if !total.is_finite() || total <= 0.0 {
        return;
    }
    let step = total / count;
    let mut accumulated = 0.0;
    let mut index = 1.0;
    for (position, spiral) in spirals.iter().enumerate() {
        while index < count && index * step < accumulated + spiral.value {
            visit(position, spiral.density.invert(index * step - accumulated));
            index += 1.0;
        }
        accumulated += spiral.value;
    }
}

/// Where a vertex sits: which spiral, and where along it.
#[derive(Debug, Clone, Copy)]
struct Position {
    spiral: usize,
    at: f64,
}

/// Raises a run's segment count until no chord strays past the tolerance.
///
/// This is what lets the line budget be spent to the last of it. [`Density`]
/// is a *model* — a sagitta bound, local to the curvature under one chord —
/// and it comes up short two ways: by up to a tenth on a single spiral where
/// the turn per chord is large (`euler::tests` pins that number down), and
/// without limit where a chord spans several spirals of very different
/// curvature, since a long straight run ending in a tight bend integrates to
/// less than one segment's worth of density yet misses the bend by the whole
/// turn.
///
/// The alternative is to hold back a fixed share of the tolerance against
/// both, which costs segments on every curve for a shortfall almost none of
/// them have. So the placement is measured instead: [`chord_deviation`] reads
/// what the chords actually do, and a run that misses is cut more finely.
/// Curves whose curvature does not lurch — every arc, every rounded corner,
/// every glyph curve — pass first time and pay one check with no
/// transcendental functions in it.
fn tighten(spirals: &[Spiral], count: f64) -> f64 {
    let budget = spirals.iter().fold(f64::INFINITY, |least, spiral| {
        least.min(spiral.line_tolerance)
    });
    let mut count = count;
    for _ in 0..VERIFY_ROUNDS {
        let worst = worst_chord_deviation(spirals, count);
        if worst <= budget {
            break;
        }
        // Deviation falls as the square of the count.
        let grown = ceil(count * sqrt(worst / budget) - COUNT_EPSILON);
        if !grown.is_finite() || grown <= count || grown > MAX_SEGMENTS {
            break;
        }
        count = grown;
    }
    count
}

/// The furthest any of the chords strays from the spirals it spans.
fn worst_chord_deviation(spirals: &[Spiral], count: f64) -> f64 {
    let Some(last) = spirals.len().checked_sub(1) else {
        return 0.0;
    };
    let start = Position { spiral: 0, at: 0.0 };
    let finish = Position {
        spiral: last,
        at: 1.0,
    };
    // Inside one spiral the curvature runs monotonically from one end to the
    // other, and what the density model gets wrong is the turn a chord spans —
    // which grows with the curvature under it. So the chords that test the
    // model hardest are the two at the ends, and the ones between them cannot
    // fail where those pass. Their vertices come straight out of the density's
    // inverse, so the check costs two chords and no walk at all. A run of
    // several spirals has no such order to it, and every chord is measured.
    if spirals.len() == 1 {
        let spiral = &spirals[0];
        if !spiral.value.is_finite() || spiral.value <= 0.0 || count < 2.0 {
            return chord_deviation(spirals, start, finish);
        }
        let step = spiral.value / count;
        let first = Position {
            spiral: 0,
            at: spiral.density.invert(step),
        };
        let final_vertex = Position {
            spiral: 0,
            at: spiral.density.invert((count - 1.0) * step),
        };
        return chord_deviation(spirals, start, first).max(chord_deviation(
            spirals,
            final_vertex,
            finish,
        ));
    }
    let mut worst: f64 = 0.0;
    let mut previous = start;
    walk(spirals, count, |index, at| {
        let next = Position { spiral: index, at };
        worst = worst.max(chord_deviation(spirals, previous, next));
        previous = next;
    });
    worst.max(chord_deviation(spirals, previous, finish))
}

/// How far the spirals between two vertices stray from the chord joining them.
///
/// Exact to first order in the turn, and an over-estimate beyond it: the
/// perpendicular offset obeys `y' = sin φ`, and using `φ` in its place can only
/// make the answer larger. With `y'' = κ` and the ends pinned to the chord,
/// `y` is a cubic within each spiral, so its extremes are the ends and the
/// points where the tangent runs parallel to the chord — roots of a quadratic.
fn chord_deviation(spirals: &[Spiral], from: Position, to: Position) -> f64 {
    // First pass: the shape of the curve, measured from its starting tangent.
    let (mut length, mut angle, mut offset) = (0.0, 0.0, 0.0);
    for_each_span(spirals, from, to, |intercept, slope, span| {
        offset += angle * span + 0.5 * intercept * span * span + slope * span * span * span / 6.0;
        angle += intercept * span + 0.5 * slope * span * span;
        length += span;
    });
    if !length.is_finite() || length <= 0.0 || !offset.is_finite() {
        return 0.0;
    }
    // Tilting the reference by this much is what pins the far end to the chord.
    let mut angle = -offset / length;
    let mut offset = 0.0;
    let mut worst: f64 = 0.0;
    for_each_span(spirals, from, to, |intercept, slope, span| {
        for root in quadratic_span_roots(angle, intercept, 0.5 * slope, span) {
            let at = offset
                + angle * root
                + 0.5 * intercept * root * root
                + slope * root * root * root / 6.0;
            worst = worst.max(abs(at));
        }
        offset += angle * span + 0.5 * intercept * span * span + slope * span * span * span / 6.0;
        angle += intercept * span + 0.5 * slope * span * span;
        worst = worst.max(abs(offset));
    });
    worst
}

/// Calls `span` for each stretch of spiral between two vertices, with the
/// curvature there as `intercept + slope·σ` over `σ ∈ [0, length]` and `σ`
/// measured in arc length.
fn for_each_span(
    spirals: &[Spiral],
    from: Position,
    to: Position,
    mut span: impl FnMut(f64, f64, f64),
) {
    let last = to.spiral.min(spirals.len().saturating_sub(1));
    for (offset, spiral) in spirals[from.spiral..=last].iter().enumerate() {
        let index = from.spiral + offset;
        let start = if index == from.spiral { from.at } else { 0.0 };
        let end = if index == to.spiral { to.at } else { 1.0 };
        let length = spiral.seg.arc_len;
        if end <= start || !length.is_finite() || length <= 0.0 {
            continue;
        }
        let params = spiral.seg.params;
        span(
            (params.k0 + params.k1 * (start - 0.5)) / length,
            params.k1 / (length * length),
            (end - start) * length,
        );
    }
}

/// Roots of `c0 + c1·σ + c2·σ²` strictly inside `(0, span)`.
fn quadratic_span_roots(c0: f64, c1: f64, c2: f64, span: f64) -> Roots {
    let mut out = Roots::default();
    if abs(c2) <= abs(c1) * LINEAR_EPSILON {
        if c1 != 0.0 {
            out.push_in_span(-c0 / c1, span);
        }
        return out;
    }
    let discriminant = c1 * c1 - 4.0 * c2 * c0;
    if discriminant < 0.0 {
        return out;
    }
    let root = sqrt(discriminant);
    let q = -0.5 * (c1 + if c1 >= 0.0 { root } else { -root });
    if q != 0.0 {
        out.push_in_span(c0 / q, span);
    }
    out.push_in_span(q / c2, span);
    out
}

/// How many times a run's segment count may be raised before the placement is
/// taken as good enough.
///
/// Each round multiplies the count by the square root of how far the worst
/// chord overshot, which converges in one round for anything but a curve whose
/// curvature is concentrated in a spot the chords keep straddling.
const VERIFY_ROUNDS: usize = 4;

/// A ceiling on segments per run, so a pathological curve cannot turn into an
/// unbounded amount of work.
const MAX_SEGMENTS: f64 = 65536.0;

/// Parameter separation below which two split points count as one.
const PIECE_EPSILON: f64 = 1e-6;

/// Slack allowed when rounding a segment count up.
///
/// A curve whose budget lands on 12.000000001 through nothing but floating
/// point wants twelve segments, not thirteen.
const COUNT_EPSILON: f64 = 1e-9;

/// How far a cubic can stray from its own chord.
///
/// Three quarters of the furthest a control point sits from the chord, which is
/// the classical bound for a cubic. Costs two cross products and no
/// transcendental functions, which is what makes it worth asking before
/// anything else.
fn flatness(curve: &[Point; 4]) -> f64 {
    let chord = Vec2::new(curve[3].x - curve[0].x, curve[3].y - curve[0].y);
    let length = chord.length();
    let offset = |point: Point| {
        let leg = Vec2::new(point.x - curve[0].x, point.y - curve[0].y);
        if length > 0.0 {
            abs(leg.cross(chord)) / length
        } else {
            leg.length()
        }
    };
    0.75 * offset(curve[1]).max(offset(curve[2]))
}

/// What [`flatness`] is multiplied by before it stands in for the fit error.
///
/// The spiral shares the cubic's endpoints and end tangents, so it stays within
/// the same envelope and its own bow is of the same order; four times the
/// cubic's own bow covers the gap between them with room to spare, and where
/// this test fires the number is heading to zero as the square of the piece
/// anyway.
const FLATNESS_MARGIN: f64 = 4.0;

/// The spiral through a cubic's endpoints and end tangents, without measuring
/// how far it strays.
fn fit_spiral_only(curve: &[Point; 4]) -> Option<EulerSeg> {
    let chord = Vec2::new(curve[3].x - curve[0].x, curve[3].y - curve[0].y);
    if chord.length_squared() <= 0.0 {
        return None;
    }
    let (start, end) = end_tangents(curve, chord);
    let th0 = atan2(start.cross(chord), start.dot(chord));
    let th1 = atan2(chord.cross(end), chord.dot(end));
    Some(EulerSeg::new(curve[0], curve[3], th0, th1))
}

/// The spiral through a cubic's endpoints and end tangents, with an upper bound
/// on how far it strays from the cubic — or `None` when the cubic is
/// degenerate enough to have no tangents to match.
fn fit_spiral(curve: &[Point; 4]) -> Option<(EulerSeg, f64)> {
    let chord = Vec2::new(curve[3].x - curve[0].x, curve[3].y - curve[0].y);
    if chord.length_squared() <= 0.0 {
        return None;
    }
    let (start, end) = end_tangents(curve, chord);
    // One `atan2` per angle rather than three for the pair: the angle between
    // two vectors is the arctangent of their cross over their dot, and it
    // lands in (−π, π] already.
    let th0 = atan2(start.cross(chord), start.dot(chord));
    let th1 = atan2(chord.cross(end), chord.dot(end));
    if abs(th0) > MAX_TANGENT_ANGLE || abs(th1) > MAX_TANGENT_ANGLE {
        // Not a rejection of the curve, a request to halve it: a spiral this
        // bent is outside the range the fit is sound over.
        return Some((EulerSeg::new(curve[0], curve[3], th0, th1), f64::INFINITY));
    }
    let seg = EulerSeg::new(curve[0], curve[3], th0, th1);
    let error = spiral_error(&Cubic::new(curve), &seg);
    Some((seg, error))
}

/// Tangent directions at a cubic's ends, falling back through the control
/// polygon when control points coincide.
fn end_tangents(curve: &[Point; 4], chord: Vec2) -> (Vec2, Vec2) {
    let scale = chord.length_squared() * DEGENERATE_TANGENT;
    let mut start = Vec2::new(curve[1].x - curve[0].x, curve[1].y - curve[0].y);
    if start.length_squared() <= scale {
        start = Vec2::new(curve[2].x - curve[0].x, curve[2].y - curve[0].y);
    }
    if start.length_squared() <= scale {
        start = chord;
    }
    let mut end = Vec2::new(curve[3].x - curve[2].x, curve[3].y - curve[2].y);
    if end.length_squared() <= scale {
        end = Vec2::new(curve[3].x - curve[1].x, curve[3].y - curve[1].y);
    }
    if end.length_squared() <= scale {
        end = chord;
    }
    (start, end)
}

/// Relative squared length below which a control leg carries no direction.
const DEGENERATE_TANGENT: f64 = 1e-24;

/// A cubic with its difference tables built once.
///
/// The Bernstein forms of `C'` and `C''` are written in the *differences* of
/// the control points, and the error estimate evaluates them a dozen times per
/// fit. Taking those differences once per curve rather than once per
/// evaluation is most of the arithmetic in the estimator.
#[derive(Debug, Clone, Copy)]
struct Cubic {
    points: [Point; 4],
    /// First differences, `P(i+1) − P(i)`.
    first: [Vec2; 3],
    /// Second differences, `P(i+2) − 2·P(i+1) + P(i)`.
    second: [Vec2; 2],
}

impl Cubic {
    fn new(points: &[Point; 4]) -> Cubic {
        let difference = |a: Point, b: Point| Vec2::new(a.x - b.x, a.y - b.y);
        let first = [
            difference(points[1], points[0]),
            difference(points[2], points[1]),
            difference(points[3], points[2]),
        ];
        let second = [
            Vec2::new(first[1].x - first[0].x, first[1].y - first[0].y),
            Vec2::new(first[2].x - first[1].x, first[2].y - first[1].y),
        ];
        Cubic {
            points: *points,
            first,
            second,
        }
    }

    fn eval(&self, t: f64) -> Point {
        eval_cubic(&self.points, t)
    }

    /// `C'(t)`, up to the constant factor 3.
    fn tangent(&self, t: f64) -> Vec2 {
        let u = 1.0 - t;
        let (a, b, c) = (u * u, 2.0 * u * t, t * t);
        Vec2::new(
            a * self.first[0].x + b * self.first[1].x + c * self.first[2].x,
            a * self.first[0].y + b * self.first[1].y + c * self.first[2].y,
        )
    }

    /// `C''(t)`, up to the constant factor 6.
    fn bend(&self, t: f64) -> Vec2 {
        let u = 1.0 - t;
        Vec2::new(
            u * self.second[0].x + t * self.second[1].x,
            u * self.second[0].y + t * self.second[1].y,
        )
    }
}

/// How far the spiral strays from the cubic.
///
/// The difficulty is not measuring the gap but knowing which two points to
/// measure between. Equal parameters mean nothing across two
/// parameterisations — a cubic's speed varies along its length where a
/// spiral's never does — and comparing them anyway reads the slip, which on an
/// ordinary cubic is several times the deviation. Projecting each sample onto
/// the curve answers it properly but costs a quadratic solve and a Newton on
/// the distance, and this is the hot spot of the whole stage.
///
/// So the answer comes from the geometry the caller has already arranged. The
/// curve has been cut at every inflection and its end angles capped, so both
/// it and the spiral run one way along their shared chord — both are *graphs*
/// over it. The point to compare against is then the one directly across, and
/// finding it is a scalar problem: project the cubic onto the chord once, and
/// two Newton steps on four numbers land on it. What is left across the chord
/// is the gap, with the parameterisation gone from the answer.
///
/// Two or three samples of a smooth gap are an estimate, not a bound.
/// [`FIT_SAFETY`] is what covers the difference, and `fit_quality` measures it
/// against densely sampled truth rather than assuming it. Being tight matters
/// as much as being safe: an estimate that reads high does not make the output
/// wrong, it makes the flattener spend segments it did not need, or split
/// curves that did not need it.
fn spiral_error(cubic: &Cubic, seg: &EulerSeg) -> f64 {
    let chord = seg.chord;
    let length = chord.length();
    if !length.is_finite() || length <= 0.0 {
        return f64::INFINITY;
    }
    // The cubic's along-chord coordinate, and its speed: a scalar cubic and
    // the scalar quadratic under it, taken once for every sample.
    let along = |point: Point| (point.x - seg.p0.x) * chord.x + (point.y - seg.p0.y) * chord.y;
    let axis = [
        along(cubic.points[0]),
        along(cubic.points[1]),
        along(cubic.points[2]),
        along(cubic.points[3]),
    ];
    let slope = [
        3.0 * (axis[1] - axis[0]),
        3.0 * (axis[2] - axis[1]),
        3.0 * (axis[3] - axis[2]),
    ];
    // The graph reading is only valid while the cubic really does run one way
    // along the chord. A piece that doubles back has more than one point
    // across from the same place on the spiral, and the search below would
    // find whichever is nearest and report a gap where there is a chasm — the
    // curve can loop out and return while every sample reads clean. Where the
    // along-chord speed changes sign there is no answer to give, only a
    // request to halve.
    if !turning_points(slope).is_empty() {
        return f64::INFINITY;
    }
    // A symmetric fit has a symmetric gap, so one side of it says everything
    // the other would. Where to look on that side is not the middle: the usual
    // cubic approximation of a circular arc passes exactly *through* the arc's
    // midpoint, so a sample there alone reads no gap at all on the commonest
    // curve there is. A fifth along is where that gap actually peaks.
    let symmetric =
        abs(seg.params.k1) <= SYMMETRY_EPSILON * (abs(seg.params.k0) + abs(seg.params.k1));
    let samples: &[f64] = if symmetric {
        &[0.5, 0.2]
    } else {
        &[0.25, 0.5, 0.75]
    };
    let mut worst: f64 = 0.0;
    for &at in samples {
        let on_spiral = seg.eval(at);
        let target = along(on_spiral);
        let mut t = at;
        for _ in 0..CORRESPONDENCE_STEPS {
            let u = 1.0 - t;
            let here = u * u * u * axis[0]
                + 3.0 * u * u * t * axis[1]
                + 3.0 * u * t * t * axis[2]
                + t * t * t * axis[3];
            let rate = u * u * slope[0] + 2.0 * u * t * slope[1] + t * t * slope[2];
            if !rate.is_finite() || rate == 0.0 {
                break;
            }
            t = (t - (here - target) / rate).clamp(0.0, 1.0);
        }
        let on_curve = cubic.eval(t);
        let offset = Vec2::new(on_curve.x - on_spiral.x, on_curve.y - on_spiral.y);
        worst = worst.max(abs(offset.cross(chord)) / length);
    }
    let error = worst * FIT_SAFETY;
    if error.is_finite() {
        error
    } else {
        f64::INFINITY
    }
}

/// Parameters where the along-chord speed vanishes, from its three Bernstein
/// coefficients.
fn turning_points(slope: [f64; 3]) -> Roots {
    quadratic_roots(
        slope[0],
        0.25 * (slope[0] + 2.0 * slope[1] + slope[2]),
        slope[2],
    )
}

/// Share of a spiral's turning that its curvature may change by and still
/// count as symmetric.
const SYMMETRY_EPSILON: f64 = 1e-9;

/// Newton steps spent lining a cubic parameter up with a point on the spiral.
///
/// The guess is the spiral's own parameter, which is already close — both
/// curves cover the same chord — and the coordinate being solved for is
/// monotonic in `t`, so the step never has anywhere else to go.
const CORRESPONDENCE_STEPS: usize = 2;

/// Parameters where the curvature vanishes: inflections, and the cusps where
/// the derivative does too.
fn curvature_roots(curve: &[Point; 4]) -> Roots {
    let cubic = Cubic::new(curve);
    let at = |t: f64| cubic.tangent(t).cross(cubic.bend(t));
    quadratic_roots(at(0.0), at(0.5), at(1.0))
}

/// Roots in `[0, 1]` of the quadratic through `(0, f0)`, `(½, fh)`, `(1, f1)`.
fn quadratic_roots(f0: f64, fh: f64, f1: f64) -> Roots {
    let a = 2.0 * (f0 - 2.0 * fh + f1);
    let b = -3.0 * f0 + 4.0 * fh - f1;
    let c = f0;
    let scale = abs(f0).max(abs(fh)).max(abs(f1));
    let mut roots = Roots::default();
    if abs(a) <= scale * LINEAR_EPSILON {
        if abs(b) > scale * LINEAR_EPSILON {
            roots.push(-c / b);
        }
        return roots;
    }
    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 {
        return roots;
    }
    // The textbook form loses the small root to cancellation; this one does
    // not.
    let root = sqrt(discriminant);
    let q = -0.5 * (b + if b >= 0.0 { root } else { -root });
    roots.push(q / a);
    if q != 0.0 {
        roots.push(c / q);
    }
    roots.sort();
    roots
}

/// Relative size below which a quadratic's leading coefficient is noise.
const LINEAR_EPSILON: f64 = 1e-12;

/// Up to two roots, filtered to `[0, 1]`, without an allocation.
#[derive(Debug, Clone, Copy, Default)]
struct Roots {
    values: [f64; 2],
    len: usize,
}

impl Roots {
    fn push(&mut self, value: f64) {
        if value.is_finite() && (0.0..=1.0).contains(&value) && self.len < 2 {
            self.values[self.len] = value;
            self.len += 1;
        }
    }

    /// Pushes a root that must lie strictly inside `(0, span)`.
    fn push_in_span(&mut self, value: f64, span: f64) {
        if value.is_finite() && value > 0.0 && value < span && self.len < 2 {
            self.values[self.len] = value;
            self.len += 1;
        }
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn sort(&mut self) {
        if self.len == 2 && self.values[0] > self.values[1] {
            self.values.swap(0, 1);
        }
    }
}

impl IntoIterator for Roots {
    type Item = f64;
    type IntoIter = core::iter::Take<core::array::IntoIter<f64, 2>>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.into_iter().take(self.len)
    }
}

fn eval_cubic(curve: &[Point; 4], t: f64) -> Point {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Point::new(
        a * curve[0].x + b * curve[1].x + c * curve[2].x + d * curve[3].x,
        a * curve[0].y + b * curve[1].y + c * curve[2].y + d * curve[3].y,
    )
}

/// The part of `curve` over `[t0, t1]`, as a cubic in its own right.
pub(crate) fn sub_cubic(curve: &[Point; 4], t0: f64, t1: f64) -> [Point; 4] {
    // The whole curve is the commonest piece there is — a fit that is accepted
    // where it stands never asks for any other — and cutting it at both ends
    // for nothing is two de Casteljau passes that give the curve back.
    if t0 <= 0.0 && t1 >= 1.0 {
        return *curve;
    }
    let right = split_right(curve, t0);
    let width = 1.0 - t0;
    let at = if width > 0.0 { (t1 - t0) / width } else { 0.0 };
    split_left(&right, at.clamp(0.0, 1.0))
}

fn split_right(curve: &[Point; 4], t: f64) -> [Point; 4] {
    let (_, _, p23, _, p123, p) = de_casteljau(curve, t);
    [p, p123, p23, curve[3]]
}

fn split_left(curve: &[Point; 4], t: f64) -> [Point; 4] {
    let (p01, _, _, p012, _, p) = de_casteljau(curve, t);
    [curve[0], p01, p012, p]
}

fn de_casteljau(curve: &[Point; 4], t: f64) -> (Point, Point, Point, Point, Point, Point) {
    let p01 = curve[0].lerp(curve[1], t);
    let p12 = curve[1].lerp(curve[2], t);
    let p23 = curve[2].lerp(curve[3], t);
    let p012 = p01.lerp(p12, t);
    let p123 = p12.lerp(p23, t);
    (p01, p12, p23, p012, p123, p012.lerp(p123, t))
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

#[cfg(test)]
mod fit_quality {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// True max distance from the spiral to the cubic, by dense sampling.
    fn truth(curve: &[Point; 4], seg: &EulerSeg) -> f64 {
        let cubic = Cubic::new(curve);
        let mut worst: f64 = 0.0;
        for i in 0..=400 {
            let p = seg.eval(i as f64 / 400.0);
            let mut best = f64::INFINITY;
            let mut at = 0.0;
            for j in 0..=1000 {
                let t = j as f64 / 1000.0;
                let d = eval_cubic(curve, t).distance(p);
                if d < best {
                    best = d;
                    at = t;
                }
            }
            // Newton on the squared distance closes the gap the sampling
            // leaves, which is otherwise the size of the answer itself.
            for _ in 0..12 {
                let on_curve = cubic.eval(at);
                let offset = Vec2::new(on_curve.x - p.x, on_curve.y - p.y);
                let first = cubic.tangent(at) * 3.0;
                let second = cubic.bend(at) * 6.0;
                let slope = first.length_squared() + offset.dot(second);
                if !slope.is_finite() || slope <= 0.0 {
                    break;
                }
                at = (at - offset.dot(first) / slope).clamp(0.0, 1.0);
            }
            best = best.min(cubic.eval(at).distance(p));
            worst = worst.max(best);
        }
        worst
    }

    /// The estimator is a sampled reading, not a bound, so what has to hold is
    /// that [`FIT_SAFETY`] covers how far it can read low. This measures that
    /// directly: fit a spiral, dense-sample the real gap, and compare.
    ///
    /// The measurement is confined to gaps large enough to mean something —
    /// below a thousandth of a pixel the dense sampling is measuring itself.
    #[test]
    fn the_safety_factor_covers_what_the_samples_miss() {
        let mut rng = Rng(7);
        let mut worst: f64 = 0.0;
        let mut worst_curve = None;
        for _ in 0..300 {
            let mut curve = [Point::new(0.0, 0.0); 4];
            for point in curve.iter_mut() {
                *point = Point::new(rng.next() * 200.0 - 100.0, rng.next() * 200.0 - 100.0);
            }
            let mut cuts = vec![0.0];
            for root in curvature_roots(&curve) {
                cuts.push(root);
            }
            cuts.push(1.0);
            for window in cuts.windows(2) {
                if window[1] - window[0] < 1e-3 {
                    continue;
                }
                // Every depth the halving search would reach, so pieces of
                // every size are measured, not only whole curves.
                for depth in 0..3u32 {
                    let pieces = 1u32 << depth;
                    for index in 0..pieces {
                        let span = (window[1] - window[0]) / pieces as f64;
                        let from = window[0] + span * index as f64;
                        let piece = sub_cubic(&curve, from, from + span);
                        let Some((seg, estimate)) = fit_spiral(&piece) else {
                            continue;
                        };
                        if !estimate.is_finite() {
                            continue;
                        }
                        let chord =
                            Vec2::new(piece[3].x - piece[0].x, piece[3].y - piece[0].y).length();
                        let real = truth(&piece, &seg);
                        if real <= 1e-3 || chord <= 1.0 {
                            continue;
                        }
                        let ratio = real / (estimate / FIT_SAFETY);
                        if ratio > worst {
                            worst = ratio;
                            worst_curve = Some(piece);
                        }
                    }
                }
            }
        }
        assert!(
            worst <= FIT_SAFETY,
            "the estimate read {worst}× low on {worst_curve:?}, past the {FIT_SAFETY}× \
             safety factor that is supposed to cover it"
        );
    }

    /// A symmetric fit is read from two samples rather than three, on the
    /// grounds that a symmetric gap says the same thing on both sides. Arcs
    /// are the curve that makes it matter — and the trap: the usual cubic
    /// approximation of a circular arc passes exactly through the arc's
    /// midpoint, so a sample there alone reads no gap at all.
    #[test]
    fn a_symmetric_fit_is_sampled_where_its_gap_is() {
        let mut worst: f64 = 0.0;
        let mut worst_at = (0.0, 0.0);
        for step in 1..=36 {
            let turn = step as f64 * 0.05;
            for radius in [1.0f64, 5.0, 20.0, 100.0, 1000.0] {
                // The exact cubic approximation of an arc of this turn.
                let handle = 4.0 / 3.0 * (turn / 4.0).tan();
                let (sine, cosine) = (turn.sin(), turn.cos());
                let curve = [
                    Point::new(radius, 0.0),
                    Point::new(radius, radius * handle),
                    Point::new(
                        radius * (cosine + handle * sine),
                        radius * (sine - handle * cosine),
                    ),
                    Point::new(radius * cosine, radius * sine),
                ];
                let Some((seg, estimate)) = fit_spiral(&curve) else {
                    continue;
                };
                if !estimate.is_finite() {
                    continue;
                }
                let real = truth(&curve, &seg);
                if real <= 1e-9 * radius {
                    continue;
                }
                let ratio = real / (estimate / FIT_SAFETY);
                if ratio > worst {
                    worst = ratio;
                    worst_at = (turn, radius);
                }
            }
        }
        assert!(
            worst <= FIT_SAFETY,
            "an arc of turn {} and radius {} was read {worst}× low",
            worst_at.0,
            worst_at.1
        );
    }

    /// The estimate reads the cubic as a graph over the chord. A piece that
    /// doubles back has two points across from the same place and the reading
    /// is meaningless — it can loop out and return while every sample comes
    /// back clean. This is such a piece, found by the deviation test before
    /// the check that catches it existed.
    #[test]
    fn a_piece_that_doubles_back_is_not_read_at_all() {
        let curve = [
            Point::new(7.9897461556807325, 2.3114144716519345),
            Point::new(4.917725548529361, 11.741506072032001),
            Point::new(8.081984207353228, 3.703708083980322),
            Point::new(6.765715692727175, 5.967495934290175),
        ];
        assert!(curvature_roots(&curve).into_iter().next().is_none());
        let (_, error) = fit_spiral(&curve).expect("a chord to fit across");
        assert!(
            error.is_infinite(),
            "a piece that runs back over its own chord was given a finite fit \
             error of {error}, which the budget would then believe"
        );
    }
}
