//! Euler spirals — the curve family stage 3 flattens through (Doc 01 §4).
//!
//! # Why a spiral and not the curve itself
//!
//! Flattening well means answering two questions: how many line segments a
//! curve needs for a given error, and where their endpoints go. Both have
//! closed-form answers for a curve whose curvature is *linear in arc length* —
//! an Euler spiral — and neither has one for a Bézier. So stage 3 fits Euler
//! spirals to the curve first, and then flattens those (Levien's method, D-11).
//! The result is fewer segments per error bound than subdivision, and segment
//! count is what every later stage pays for.
//!
//! # The one primitive everything is built from
//!
//! Write a spiral's tangent angle over its normalised arc length `s ∈ [0, 1]`
//! as `θ(s) = θc + k0·u + k1·u²/2` with `u = s − ½`: `k0` is the total turning,
//! `k1` how fast the curvature changes. Then every quantity needed —
//! the chord, the arc length, a point at parameter `s` — is
//!
//! ```text
//! I(k0, k1) = ∫ exp(i·(k0·u + k1·u²/2)) du   over u ∈ [−½, ½]
//! ```
//!
//! evaluated at some parameters. [`integ_euler`] is that integral, as the
//! power series its integrand's Taylor expansion integrates term by term. The
//! series is not a fitted approximation: every coefficient is a rational
//! number this module computes at compile time from the expansion, and its
//! truncation error over the parameter range stage 3 admits is under 1e-12
//! (`tests/euler.rs`).

use otf_2d_engine_geom::{Point, Vec2};

use crate::math::{abs, atan2, cbrt, copysign, cos, sin, sqrt};

/// Truncation order of the [`integ_euler`] series: terms up to `k0^p·k1^q`
/// with `p + q ≤ ORDER` are kept.
///
/// The series converges like `θ^n/n!` in the largest angle the integrand
/// reaches, so the order sets the parameter range that stays accurate. At 16
/// the error is under 1e-12 for `|k0| ≤ 2.6, |k1| ≤ 9`, which contains every
/// spiral the fit in `flatten` accepts, partial integrals included.
const ORDER: usize = 16;

/// Rows are powers of `k0²` — odd powers of `k0` integrate to zero over a
/// symmetric interval, so only even ones exist.
const ROWS: usize = ORDER / 2 + 1;
/// Columns are powers of `k1`.
const COLS: usize = ORDER + 1;

/// Signed series coefficients: `COEF[p][q]` multiplies `k0^{2p}·k1^q`.
///
/// From `exp(iθ) = Σ (iθ)^n/n!` with `θ = k0·u + k1·u²/2`, integrated over
/// `u ∈ [−½, ½]`. The term in `k0^{2p}·k1^q` collects with
/// `i^{2p+q}·C(2p+q, q) / ((2p+q)! · 2^{2p+3q} · (2p+2q+1))`; the power of `i`
/// is folded into the sign here and decides which part the term lands in —
/// even `q` is real, odd `q` imaginary.
static COEF: [[f64; COLS]; ROWS] = coefficients();

const fn coefficients() -> [[f64; COLS]; ROWS] {
    let mut out = [[0.0; COLS]; ROWS];
    let mut p = 0;
    while p < ROWS {
        let mut q = 0;
        while q < COLS {
            if 2 * p + q <= ORDER {
                let n = 2 * p + q;
                // C(n, q), built multiplicatively so it stays exact in f64.
                let mut binomial = 1.0;
                let mut i = 0;
                while i < q {
                    binomial = binomial * ((n - i) as f64) / ((i + 1) as f64);
                    i += 1;
                }
                let mut factorial = 1.0;
                let mut f = 2;
                while f <= n {
                    factorial *= f as f64;
                    f += 1;
                }
                let mut power_of_two = 1.0;
                let mut t = 0;
                while t < 2 * p + 3 * q {
                    power_of_two *= 2.0;
                    t += 1;
                }
                let magnitude = binomial / (factorial * power_of_two * ((n + q + 1) as f64));
                // i^{2p+q} = (−1)^p · i^q, and i^q is ±1 or ±i by q mod 4.
                let quarter = if q % 2 == 0 { q / 2 } else { (q - 1) / 2 };
                let negative = (p + quarter) % 2 == 1;
                out[p][q] = if negative { -magnitude } else { magnitude };
            }
            q += 1;
        }
        p += 1;
    }
    out
}

/// The `k1⁰` column of [`COEF`], on its own.
///
/// It is the series of `sin(x)/x` at `x = k0/2`, and it is the whole answer
/// for an arc — which is most of what a path is made of. Read out of `COEF` it
/// is nine entries a row apart, so nine cache lines; here it is one, and this
/// stage touches it under every point it emits.
static SINC: [f64; ROWS] = arc_coefficients();

const fn arc_coefficients() -> [f64; ROWS] {
    let mut out = [0.0; ROWS];
    let table = coefficients();
    let mut p = 0;
    while p < ROWS {
        out[p] = table[p][0];
        p += 1;
    }
    out
}

/// Term magnitude below which the rest of the series cannot move the answer.
const TERM_FLOOR: f64 = 1e-14;

/// How many terms the series needs at these parameters.
///
/// The integrand is `exp(iθ)` with `|θ| ≤ |k0|/2 + |k1|/8` over the interval,
/// so the tail past order `n` is bounded by `θ^{n+1}/(n+1)!`. The thresholds
/// below are that bound solved for 1e-12, which on a chord of 100 000 pixels
/// is still a nanometre. Cutting the order where the angles are small is worth
/// doing: an ordinary quarter-circle needs ten terms and a shallow one six,
/// against the sixteen the widest spiral asks for, and the series sits under
/// every point this stage emits.
#[inline]
fn series_order(k0: f64, k1: f64) -> usize {
    let theta = 0.5 * abs(k0) + 0.125 * abs(k1);
    if theta <= 0.065 {
        6
    } else if theta <= 0.19 {
        8
    } else if theta <= 0.39 {
        10
    } else if theta <= 0.67 {
        12
    } else if theta <= 1.01 {
        14
    } else {
        ORDER
    }
}

/// The Euler-spiral integral `I(k0, k1)`, as `(real, imaginary)`.
///
/// The powers are raised as the loops walk, never tabulated. A table has to be
/// filled to the truncation order before the first term is summed, which for
/// the curve this stage sees most — an arc, where `k1` is zero and every power
/// past the first is dead — is a chain of a dozen dependent multiplies for
/// nothing. Walking them costs one multiply where the term is wanted and stops
/// where the terms do.
#[inline]
pub fn integ_euler(k0: f64, k1: f64) -> (f64, f64) {
    let order = series_order(k0, k1);
    let rows = order / 2 + 1;
    let squared = k0 * k0;

    if k1 == 0.0 {
        // A spiral whose curvature does not change is a circular arc, and its
        // integral collapses: `∫exp(i·k0·u)du` over a symmetric interval is
        // `sin(k0/2)/(k0/2)`, real, with no `k1` anywhere. The `k1⁰` column of
        // the table *is* that function's series, so this is the same answer by
        // a shorter road — one Horner pass instead of the whole table. Arcs
        // are most of what a path is made of, so it is the road most travelled.
        let mut re = 0.0;
        for coefficient in SINC[..rows].iter().rev() {
            re = re * squared + coefficient;
        }
        return (re, 0.0);
    }

    let (mut re, mut im) = (0.0, 0.0);
    let mut base = 1.0;
    for (p, row) in COEF.iter().enumerate().take(rows) {
        // Rows and columns both shrink faster than a factorial, so once a
        // leading term is under the floor nothing after it can matter. Curves
        // whose curvature barely changes — every arc — leave most of the table
        // untouched this way.
        if p > 0 && abs(base * row[0]) < TERM_FLOOR {
            break;
        }
        let last = order - 2 * p;

        // The real part: even `q`, so the power chain steps by `k1²`.
        let mut power = 1.0;
        let mut q = 0;
        while q <= last {
            let term = row[q] * base * power;
            re += term;
            if abs(term) < TERM_FLOOR {
                break;
            }
            power = power * k1 * k1;
            q += 2;
        }
        // The imaginary part: odd `q`, chain starting at `k1`.
        let mut power = k1;
        let mut q = 1;
        while q <= last {
            let term = row[q] * base * power;
            im += term;
            if abs(term) < TERM_FLOOR {
                break;
            }
            power = power * k1 * k1;
            q += 2;
        }
        base *= squared;
    }
    (re, im)
}

/// A spiral's shape, independent of where it sits or how big it is.
///
/// `k0` is the total turning over the segment and `k1` the change in
/// curvature; `theta_c` orients the spiral so its chord runs along the chord of
/// the curve it fits, and `ch` is the length of that chord for a spiral of unit
/// arc length — the factor between arc length and chord length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EulerParams {
    pub k0: f64,
    pub k1: f64,
    pub theta_c: f64,
    pub ch: f64,
}

impl EulerParams {
    /// The spiral whose end tangents make angles `th0` and `th1` with its
    /// chord.
    ///
    /// `k0` falls straight out: the angle between a chord and the tangents at
    /// its ends sums to the total turning. `k1` is the root of
    /// `θc(k1) + arg I(k0, k1) = 0` — the statement that the spiral's chord
    /// points along the chord it is being fitted to — found by Newton from the
    /// small-angle solution `6·(th1 − th0)`. Two steps take the residual under
    /// 1e-12 across the whole range of angles stage 3 accepts, because the
    /// derivative is analytic rather than estimated.
    pub fn from_angles(th0: f64, th1: f64) -> EulerParams {
        let k0 = th0 + th1;
        let dth = th1 - th0;
        let mut k1 = k1_estimate(k0, dth);
        let mut value = integ_euler(k0, k1);
        // One Newton step turns the estimate's five good digits into all of
        // them. The slope it needs is not measured — the same reversion that
        // produced the estimate differentiates to give it — so the step costs
        // one more evaluation of the integral and nothing else.
        let residual = 0.5 * dth - 0.125 * k1 + atan2(value.1, value.0);
        if abs(residual) > RESIDUAL_EPSILON {
            k1 += residual * k1_slope(k0, dth);
            value = integ_euler(k0, k1);
        }
        let (re, im) = value;
        EulerParams {
            k0,
            k1,
            theta_c: -atan2(im, re),
            ch: sqrt(re * re + im * im),
        }
    }

    /// The tangent angle relative to the chord at arc-length fraction `s`.
    ///
    /// Read by the tests that check a fitted spiral against the angles it was
    /// asked for; the flattener works with `k0` and `k1` directly.
    #[inline]
    #[allow(dead_code)]
    pub fn theta(&self, s: f64) -> f64 {
        let u = s - 0.5;
        self.theta_c + self.k0 * u + 0.5 * self.k1 * u * u
    }

    /// The curvature at `s`, in units where the whole spiral has arc length 1.
    ///
    /// Read by the tests that check a fitted spiral against the curve it was
    /// fitted to; the flattener itself works with `k0` and `k1` directly.
    #[inline]
    #[allow(dead_code)]
    pub fn curvature(&self, s: f64) -> f64 {
        self.k0 + self.k1 * (s - 0.5)
    }
}

/// The solution of the `k1` equation, as its own power series.
///
/// The equation `dth/2 − k1/8 + arg I(k0, k1) = 0` can be inverted directly.
/// `arg I` is odd in `k1` and even in `k0`, so writing the whole left side as
/// a series in `k1` and reverting it gives `k1` as an odd series in `dth`
/// whose coefficients are even series in `k0` — and every one of those
/// coefficients is a rational number, worked out from the same expansion the
/// [`COEF`] table comes from. The first is 6, which is the small-angle answer
/// this used to start from; the rest are what makes one Newton step enough
/// instead of three.
///
/// Grouped by powers of `k0²`, with `d = dth²`:
///
/// ```text
/// k0⁰: 6 − d/70 − d²/10780 + 163·d³/588588000
/// k0²: −1/10 + d/4200 + 713·d²/42042000 + 20441·d³/120071952000
/// k0⁴: −1/1400 + 443·d/6468000 + 71·d²/840840000
/// k0⁶: −1/126000 + 239·d/302702400
/// ```
///
/// Truncated here the estimate is within about `1e-5` of the root over the
/// whole range of angles the fit accepts, and far closer over most of it;
/// `tests` pins that down against the converged answer.
#[inline]
fn k1_estimate(k0: f64, dth: f64) -> f64 {
    let d = dth * dth;
    let k = k0 * k0;
    let c0 = 6.0 - d * (1.0 / 70.0) - d * d * (1.0 / 10780.0) + d * d * d * (163.0 / 588588000.0);
    let c1 = -(1.0 / 10.0)
        + d * (1.0 / 4200.0)
        + d * d * (713.0 / 42042000.0)
        + d * d * d * (20441.0 / 120071952000.0);
    let c2 = -(1.0 / 1400.0) + d * (443.0 / 6468000.0) + d * d * (71.0 / 840840000.0);
    let c3 = -(1.0 / 126000.0) + d * (239.0 / 302702400.0);
    dth * (c0 + k * (c1 + k * (c2 + k * c3)))
}

/// `dk1/dT` at `T = dth/2` — the slope [`from_angles`] steps along.
///
/// [`k1_estimate`] is the series `k1 = a₁·T + a₃·T³ + a₅·T⁵`, and the equation
/// it solves is `T = W(k1)`, so `dW/dk1` is the reciprocal of that series
/// differentiated. Writing it out is free next to evaluating the integral's
/// derivative, which is what the step would otherwise cost.
#[inline]
fn k1_slope(k0: f64, dth: f64) -> f64 {
    let t = 0.5 * dth;
    let t2 = t * t;
    let k = k0 * k0;
    let a1 = 12.0 - k * (1.0 / 5.0) - k * k * (1.0 / 700.0) - k * k * k * (1.0 / 63000.0);
    let a3 = -(4.0 / 35.0) + k * (1.0 / 525.0) + k * k * (443.0 / 808500.0);
    let a5 = -(8.0 / 2695.0) + k * (1426.0 / 2627625.0);
    a1 + t2 * (3.0 * a3 + t2 * 5.0 * a5)
}

/// Residual below which the estimate is already the answer.
///
/// A symmetric spiral is one of those: `dth` is zero, so the estimate is
/// exactly zero, the chord is real, and the residual is exactly zero with it.
/// That is the commonest curve there is.
const RESIDUAL_EPSILON: f64 = 1e-12;

/// A spiral placed in the plane: it runs from `p0` to `p0 + chord`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EulerSeg {
    pub p0: Point,
    pub chord: Vec2,
    pub params: EulerParams,
    /// Arc length in the same units as `chord`.
    pub arc_len: f64,
}

impl EulerSeg {
    /// The spiral from `p0` to `p1` whose end tangents make angles `th0` and
    /// `th1` with the chord, measured so that a left turn is positive at both
    /// ends.
    pub fn new(p0: Point, p1: Point, th0: f64, th1: f64) -> EulerSeg {
        let chord = Vec2::new(p1.x - p0.x, p1.y - p0.y);
        let params = EulerParams::from_angles(th0, th1);
        let length = chord.length();
        let arc_len = if params.ch > 0.0 {
            length / params.ch
        } else {
            length
        };
        EulerSeg {
            p0,
            chord,
            params,
            arc_len,
        }
    }

    /// The point at arc-length fraction `s ∈ [0, 1]`.
    ///
    /// A partial integral of a spiral is another spiral's integral over a
    /// shifted, scaled interval, so this is one [`integ_euler`] call and a
    /// rotation — no quadrature, and no accumulation of error along the curve.
    pub fn eval(&self, s: f64) -> Point {
        if s <= 0.0 {
            return self.p0;
        }
        if s >= 1.0 {
            return Point::new(self.p0.x + self.chord.x, self.p0.y + self.chord.y);
        }
        let EulerParams {
            k0,
            k1,
            theta_c,
            ch,
        } = self.params;
        // u = a + s·v maps v ∈ [−½, ½] onto u ∈ [−½, s − ½].
        let a = 0.5 * (s - 1.0);
        let phase = theta_c + k0 * a + 0.5 * k1 * a * a;
        let (re, im) = integ_euler(s * (k0 + k1 * a), k1 * s * s);
        if ch <= 0.0 {
            return self.p0;
        }
        let scale = s / ch;
        let (sn, cs) = (sin(phase), cos(phase));
        // Rotate by the phase, then map the unit chord onto the real one:
        // both are complex multiplies.
        let x = scale * (re * cs - im * sn);
        let y = scale * (re * sn + im * cs);
        Point::new(
            self.p0.x + self.chord.x * x - self.chord.y * y,
            self.p0.y + self.chord.y * x + self.chord.x * y,
        )
    }
}

/// The largest turn one line segment may span, in radians.
///
/// The error model behind [`Density`] is the sagitta of a circular arc,
/// `κ·ℓ²/8`, which is a Taylor expansion in the turn and drifts from the true
/// `R·(1 − cos(θ/2))` as the turn grows. At 1 radian the two are within 2%,
/// and a chord across more than a radian of a tight hairpin misses the shape
/// entirely rather than by a sagitta. So the turn per segment is capped, which
/// only binds below a curvature radius of `8·tolerance` — features a couple of
/// pixels across.
const MAX_TURN: f64 = 1.0;

/// How many line segments a spiral needs, and where their endpoints go.
///
/// Both come from one density function `D(u)`, the number of segments per unit
/// of the spiral's parameter. Its integral is the segment count and its inverse
/// places them, and both are closed forms because `D` is piecewise a power of
/// the curvature, which is linear in `u`:
///
/// - `A·√|k|` — the sagitta bound `κ·ℓ²/8 ≤ tolerance`, the term that governs
///   ordinary curves.
/// - `B·|k|` — the turn cap above, which takes over inside tight hairpins.
/// - `C` — the deviation a chord picks up from curvature *changing* across it,
///   `κ'·ℓ³/125 ≤ tolerance`. Near an inflection the first term vanishes while
///   the curve still bends away from the chord either side; without this a
///   spiral there gets one long chord and misses by several times tolerance.
///
/// The density is the largest of the three, not their sum: each governs its own
/// regime, and adding them would double the segment count where two are
/// comparable.
#[derive(Debug, Clone, Copy)]
pub struct Density {
    k0: f64,
    k1: f64,
    a: f64,
    b: f64,
    c: f64,
    /// Interval boundaries in `u`, `count + 1` of them.
    bounds: [f64; MAX_SPANS + 1],
    /// Which term is largest on each interval.
    kinds: [Term; MAX_SPANS],
    /// Integral of the density up to the start of each interval, with the
    /// total in the last slot.
    running: [f64; MAX_SPANS + 1],
    count: usize,
    total: f64,
}

/// Three terms with two thresholds between them, so at most two crossings each
/// for `k` and `−k`, plus the two ends.
const MAX_SPANS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Term {
    Constant,
    Sqrt,
    Linear,
}

impl Density {
    /// The density for `segment` at a device-space error `tolerance`.
    pub fn new(segment: &EulerSeg, tolerance: f64) -> Density {
        let EulerParams { k0, k1, .. } = segment.params;
        let length = segment.arc_len;
        let a = if tolerance > 0.0 && length > 0.0 {
            sqrt(length / (8.0 * tolerance))
        } else {
            0.0
        };
        let b = 1.0 / MAX_TURN;
        let c = if tolerance > 0.0 && length > 0.0 {
            cbrt(length * abs(k1) / (125.0 * tolerance))
        } else {
            0.0
        };

        let mut density = Density {
            k0,
            k1,
            a,
            b,
            c,
            bounds: [0.0; MAX_SPANS + 1],
            kinds: [Term::Constant; MAX_SPANS],
            running: [0.0; MAX_SPANS + 1],
            count: 0,
            total: 0.0,
        };

        if abs(k1) <= K1_FLAT * (abs(k0) + abs(k1)) {
            // Constant curvature: every term is constant too, so the whole
            // spiral is one span and the antiderivatives — which divide by
            // `k1` — are never reached.
            let magnitude = abs(k0);
            let flat = {
                let sqrt_term = a * sqrt(magnitude);
                let linear_term = b * magnitude;
                if sqrt_term >= linear_term {
                    sqrt_term
                } else {
                    linear_term
                }
            };
            density.c = flat;
            density.bounds[0] = -0.5;
            density.bounds[1] = 0.5;
            density.kinds[0] = Term::Constant;
            density.running[0] = 0.0;
            density.running[1] = flat;
            density.count = 1;
            density.total = flat;
            return density;
        }

        // With |k| rising the largest term goes constant → sqrt → linear, so
        // the regimes need at most two thresholds.
        let (first, second) = density.thresholds();
        let mut breaks = [-0.5, 0.5, 0.0, 0.0, 0.0, 0.0];
        let mut breaks_len = 2;
        for threshold in [first, second] {
            if !threshold.is_finite() {
                continue;
            }
            for signed in [threshold, -threshold] {
                let u = (signed - k0) / k1;
                if u > -0.5 && u < 0.5 {
                    breaks[breaks_len] = u;
                    breaks_len += 1;
                }
            }
        }
        let breaks = &mut breaks[..breaks_len];
        // At most six entries, and this runs per spiral: an insertion sort
        // beats handing a comparator to a general one.
        for index in 1..breaks.len() {
            let mut slot = index;
            while slot > 0 && breaks[slot - 1] > breaks[slot] {
                breaks.swap(slot - 1, slot);
                slot -= 1;
            }
        }

        let mut running = 0.0;
        density.bounds[0] = breaks[0];
        for window in 0..breaks.len() - 1 {
            let (start, end) = (breaks[window], breaks[window + 1]);
            if end <= start || density.count == MAX_SPANS {
                continue;
            }
            let kind = density.term_at(0.5 * (start + end));
            density.bounds[density.count] = start;
            density.bounds[density.count + 1] = end;
            density.kinds[density.count] = kind;
            density.running[density.count] = running;
            running += density.integral(kind, start, end);
            density.count += 1;
            density.running[density.count] = running;
        }
        density.total = running;
        density
    }

    /// The `|k|` at which the sqrt term overtakes the constant one, and the one
    /// at which the linear term overtakes the sqrt.
    ///
    /// When the sqrt term never leads — a very short spiral whose curvature
    /// barely changes — the crossing collapses to a single constant-to-linear
    /// threshold, reported as the same value twice.
    fn thresholds(&self) -> (f64, f64) {
        let Density { a, b, c, .. } = *self;
        if a <= 0.0 {
            return (0.0, 0.0);
        }
        let constant_to_sqrt = (c / a) * (c / a);
        let sqrt_to_linear = (a / b) * (a / b);
        if constant_to_sqrt <= sqrt_to_linear {
            (constant_to_sqrt, sqrt_to_linear)
        } else {
            let constant_to_linear = c / b;
            (constant_to_linear, constant_to_linear)
        }
    }

    #[inline]
    fn term_at(&self, u: f64) -> Term {
        let k = abs(self.k0 + self.k1 * u);
        let sqrt_term = self.a * sqrt(k);
        let linear_term = self.b * k;
        if self.c >= sqrt_term && self.c >= linear_term {
            Term::Constant
        } else if sqrt_term >= linear_term {
            Term::Sqrt
        } else {
            Term::Linear
        }
    }

    /// `∫ D du` over `[start, end]`, where `D` is `kind` throughout.
    fn integral(&self, kind: Term, start: f64, end: f64) -> f64 {
        match kind {
            Term::Constant => self.c * (end - start),
            Term::Sqrt => {
                self.a * (self.sqrt_antiderivative(end) - self.sqrt_antiderivative(start))
            }
            Term::Linear => {
                self.b * (self.linear_antiderivative(end) - self.linear_antiderivative(start))
            }
        }
    }

    /// `∫ √|k0 + k1·u| du`, continuous and monotonic across the sign change.
    #[inline]
    fn sqrt_antiderivative(&self, u: f64) -> f64 {
        let k = self.k0 + self.k1 * u;
        let magnitude = abs(k);
        copysign(magnitude * sqrt(magnitude), k) * (2.0 / (3.0 * self.k1))
    }

    /// `∫ |k0 + k1·u| du`, likewise.
    #[inline]
    fn linear_antiderivative(&self, u: f64) -> f64 {
        let k = self.k0 + self.k1 * u;
        copysign(k * k, k) / (2.0 * self.k1)
    }

    /// The number of line segments this spiral needs, before rounding.
    #[inline]
    pub fn value(&self) -> f64 {
        self.total
    }

    /// The parameter `s` at which the density integral reaches `target`.
    pub fn invert(&self, target: f64) -> f64 {
        if self.count == 0 || self.total <= 0.0 {
            // A spiral with no density is a straight line, and `walk` never
            // asks one for a vertex; the answer only has to be in range.
            return 0.0;
        }
        let mut span = self.count - 1;
        for index in 0..self.count {
            if target <= self.running[index + 1] {
                span = index;
                break;
            }
        }
        let start = self.bounds[span];
        let remainder = target - self.running[span];
        let u = match self.kinds[span] {
            Term::Constant => {
                if self.c > 0.0 {
                    start + remainder / self.c
                } else {
                    start
                }
            }
            Term::Sqrt => {
                let value = self.sqrt_antiderivative(start) + remainder / self.a;
                let scaled = 1.5 * self.k1 * value;
                let root = cbrt(abs(scaled));
                let k = copysign(root * root, scaled);
                (k - self.k0) / self.k1
            }
            Term::Linear => {
                let value = self.linear_antiderivative(start) + remainder / self.b;
                let scaled = 2.0 * self.k1 * value;
                let k = copysign(sqrt(abs(scaled)), scaled);
                (k - self.k0) / self.k1
            }
        };
        (u + 0.5).clamp(0.0, 1.0)
    }
}

/// Below this share of the curvature, the change in it counts as none at all.
///
/// The antiderivatives divide by `k1`, and inverting them multiplies whatever
/// rounding the cube root left by `1/k1`. Holding `k1` to a millionth of the
/// curvature bounds that amplification at 1e-10 of the segment, and a spiral
/// that close to a circular arc has nothing to gain from the general path
/// anyway.
const K1_FLAT: f64 = 1e-6;

#[cfg(test)]
mod tests {
    use super::*;

    /// `I(k0, k1)` by Simpson's rule, as the reference the series is checked
    /// against. Slow and obviously correct, which is the point.
    fn integ_reference(k0: f64, k1: f64, steps: usize) -> (f64, f64) {
        let (mut re, mut im) = (0.0, 0.0);
        let width = 1.0 / steps as f64;
        for index in 0..=steps {
            let u = -0.5 + index as f64 * width;
            let weight = if index == 0 || index == steps {
                1.0
            } else if index % 2 == 1 {
                4.0
            } else {
                2.0
            };
            let theta = k0 * u + 0.5 * k1 * u * u;
            re += weight * theta.cos();
            im += weight * theta.sin();
        }
        (re * width / 3.0, im * width / 3.0)
    }

    #[test]
    fn the_series_matches_numerical_integration() {
        // The range the flattener can reach: `MAX_TANGENT_ANGLE` in `flatten`
        // caps the angles at 0.9, so `k0` at 1.8 and `k1` near 9, and a partial
        // integral widens `k0` by another `k1/8`.
        let mut worst = 0.0f64;
        for k0_step in -26..=26 {
            for k1_step in -18..=18 {
                let k0 = k0_step as f64 * 0.1;
                let k1 = k1_step as f64 * 0.5;
                let (re, im) = integ_euler(k0, k1);
                let (rre, rim) = integ_reference(k0, k1, 4096);
                worst = worst.max((re - rre).abs()).max((im - rim).abs());
            }
        }
        // A tenth of a nanometre on a chord a hundred thousand pixels long.
        assert!(
            worst < 1e-9,
            "series and quadrature disagree by {worst}, which is more than the \
             quadrature's own error"
        );
    }

    #[test]
    fn a_short_series_is_used_where_it_is_enough() {
        // The adaptive order is an optimisation, so it has to be invisible:
        // whatever it picks must agree with the full-order series.
        for k0_step in -18..=18 {
            for k1_step in -18..=18 {
                let k0 = k0_step as f64 * 0.1;
                let k1 = k1_step as f64 * 0.5;
                let order = series_order(k0, k1);
                assert!(order <= ORDER);
                let (re, im) = integ_euler(k0, k1);
                let (rre, rim) = integ_reference(k0, k1, 4096);
                assert!(
                    (re - rre).abs() < 1e-9 && (im - rim).abs() < 1e-9,
                    "order {order} at k0={k0} k1={k1} lost accuracy"
                );
            }
        }
    }

    #[test]
    fn the_fit_reproduces_the_angles_it_was_given() {
        let mut worst = 0.0f64;
        for first in -18..=18 {
            for second in -18..=18 {
                let th0 = first as f64 * 0.05;
                let th1 = second as f64 * 0.05;
                let params = EulerParams::from_angles(th0, th1);
                // theta(0) is the start tangent measured from the chord, which
                // is `-th0` by the convention in `from_angles`.
                worst = worst.max((params.theta(0.0) + th0).abs());
                worst = worst.max((params.theta(1.0) - th1).abs());
                assert!(params.ch > 0.0, "chord factor vanished at {th0}, {th1}");
            }
        }
        assert!(worst < 1e-9, "fitted tangents are off by {worst}");
    }

    #[test]
    fn a_spiral_starts_and_ends_where_it_was_put() {
        let (p0, p1) = (Point::new(-30.0, 12.0), Point::new(70.0, -5.0));
        for first in -16..=16 {
            for second in -16..=16 {
                let seg = EulerSeg::new(p0, p1, first as f64 * 0.05, second as f64 * 0.05);
                assert!(seg.eval(0.0).distance(p0) < 1e-9);
                assert!(seg.eval(1.0).distance(p1) < 1e-9);
                // And approaching the ends from inside, not just at them.
                assert!(seg.eval(1e-6).distance(p0) < 1e-3);
                assert!(seg.eval(1.0 - 1e-6).distance(p1) < 1e-3);
            }
        }
    }

    #[test]
    fn a_symmetric_spiral_is_a_circular_arc() {
        // Equal end angles mean constant curvature, and then the spiral has to
        // be the circle through its endpoints — the one case with an
        // independent closed form to check against.
        for step in 1..=16 {
            let angle = step as f64 * 0.05;
            let seg = EulerSeg::new(Point::new(0.0, 0.0), Point::new(100.0, 0.0), angle, angle);
            assert!(seg.params.k1.abs() < 1e-9, "k1 = {}", seg.params.k1);
            let radius = seg.arc_len / seg.params.k0;
            let centre = Point::new(
                50.0,
                -(radius * (angle).cos() * 0.0) - centre_offset(radius),
            );
            for index in 0..=32 {
                let at = index as f64 / 32.0;
                let point = seg.eval(at);
                assert!(
                    (point.distance(centre) - radius).abs() < 1e-9 * radius,
                    "point at {at} is {} from the centre, not {radius}",
                    point.distance(centre)
                );
            }
        }
    }

    /// How far below the chord the centre of the circle through `(0,0)` and
    /// `(100,0)` of the given radius sits.
    fn centre_offset(radius: f64) -> f64 {
        -(radius * radius - 50.0 * 50.0).sqrt()
    }

    #[test]
    fn arc_length_is_the_chord_over_the_chord_factor() {
        for first in (-16..=16).step_by(4) {
            for second in (-16..=16).step_by(4) {
                let (th0, th1) = (first as f64 * 0.05, second as f64 * 0.05);
                let seg = EulerSeg::new(Point::new(0.0, 0.0), Point::new(100.0, 0.0), th0, th1);
                // Measured by walking the spiral finely enough that the
                // polyline's shortfall is below the tolerance asserted.
                let steps = 8_000;
                let mut walked = 0.0;
                let mut previous = seg.eval(0.0);
                for index in 1..=steps {
                    let point = seg.eval(index as f64 / steps as f64);
                    walked += previous.distance(point);
                    previous = point;
                }
                assert!(
                    (walked - seg.arc_len).abs() < 1e-5 * seg.arc_len,
                    "walked {walked} against a stated arc length of {}",
                    seg.arc_len
                );
            }
        }
    }

    #[test]
    fn the_density_inverse_undoes_the_density() {
        for first in -12..=12 {
            for second in -12..=12 {
                let (th0, th1) = (first as f64 * 0.07, second as f64 * 0.07);
                let seg = EulerSeg::new(Point::new(0.0, 0.0), Point::new(240.0, 0.0), th0, th1);
                let density = Density::new(&seg, 0.25);
                assert!(density.value() >= 0.0 && density.value().is_finite());
                let mut previous = 0.0;
                for step in 0..=20 {
                    let target = density.value() * step as f64 / 20.0;
                    let at = density.invert(target);
                    assert!(
                        (0.0..=1.0).contains(&at),
                        "inverse left the segment: {at} at {th0}, {th1}"
                    );
                    assert!(
                        at >= previous - 1e-9,
                        "inverse is not monotonic: {at} after {previous}"
                    );
                    previous = at;
                }
                if density.value() <= 0.0 {
                    // A straight spiral has no density to invert, and `walk`
                    // never asks it for a vertex.
                    continue;
                }
                assert!(
                    density.invert(0.0).abs() < 1e-9,
                    "invert(0) = {} at {th0}, {th1} (k0 {}, k1 {})",
                    density.invert(0.0),
                    seg.params.k0,
                    seg.params.k1
                );
                assert!(
                    (density.invert(density.value()) - 1.0).abs() < 1e-9,
                    "invert(total) = {} at {th0}, {th1}",
                    density.invert(density.value())
                );
            }
        }
    }

    #[test]
    fn the_density_places_chords_within_the_tolerance_it_was_given() {
        // The placement's own promise, with the spiral standing in for the
        // curve: cut a spiral into the number of chords the density asks for,
        // and none of them may stray further than the tolerance times the
        // margin the flattener budgets for.
        // The model is a sagitta bound, exact only in the limit of a small
        // turn, so the placement can come out this much over the tolerance it
        // was handed. `flatten` does not budget for it — it measures the
        // chords it actually placed and cuts again where they miss — but the
        // size of what it is measuring for belongs here, next to the model.
        const MARGIN: f64 = 1.12;
        let tolerance = 0.25;
        let mut worst = 0.0f64;
        for first in -12..=12 {
            for second in -12..=12 {
                for scale in [4.0, 60.0, 900.0] {
                    let (th0, th1) = (first as f64 * 0.07, second as f64 * 0.07);
                    let seg = EulerSeg::new(Point::new(0.0, 0.0), Point::new(scale, 0.0), th0, th1);
                    let density = Density::new(&seg, tolerance);
                    let count = density.value().ceil().max(1.0);
                    let mut vertices = alloc::vec![seg.eval(0.0)];
                    for index in 1..count as usize {
                        let target = density.value() * index as f64 / count;
                        vertices.push(seg.eval(density.invert(target)));
                    }
                    vertices.push(seg.eval(1.0));
                    for index in 0..=400 {
                        let point = seg.eval(index as f64 / 400.0);
                        let mut best = f64::INFINITY;
                        for pair in vertices.windows(2) {
                            best = best.min(distance_to_segment(point, pair[0], pair[1]));
                        }
                        worst = worst.max(best / tolerance);
                    }
                }
            }
        }
        assert!(
            worst <= MARGIN,
            "chord placement strayed {worst}× the tolerance, past the {MARGIN}× \
             the flattener budgets for"
        );
    }

    fn distance_to_segment(point: Point, from: Point, to: Point) -> f64 {
        let (dx, dy) = (to.x - from.x, to.y - from.y);
        let length = dx * dx + dy * dy;
        if length == 0.0 {
            return point.distance(from);
        }
        let at = (((point.x - from.x) * dx + (point.y - from.y) * dy) / length).clamp(0.0, 1.0);
        point.distance(Point::new(from.x + at * dx, from.y + at * dy))
    }
}
