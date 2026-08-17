//! Stage 5 — strip generation (Doc 01 §4).
//!
//! Turns per-tile segments into sparse strips: spans of per-pixel alpha where
//! coverage varies, and spans of constant per-row alpha where it does not.
//! A large filled rectangle produces a handful of alpha spans at its edges and
//! one constant span across each band's interior, rather than millions of
//! coverage values.
//!
//! # Analytic antialiasing, never supersampling
//!
//! Coverage is the exact signed area a shape covers in each pixel, accumulated
//! from the geometry (Doc 01 §4, §8). Supersampling costs N× the work for
//! worse quality on near-horizontal edges, which is precisely the case UI text
//! and hairlines are made of. A grep gate in `ci/invariants.sh` fails the build
//! if a supersampling path appears here.
//!
//! # How the accumulation works
//!
//! For each scanline, every edge crossing it deposits a signed area delta into
//! the pixel cells it touches. A prefix sum along `x` then yields, at every
//! pixel, the exact signed area covered so far. Away from any edge the running
//! sum is the winding number, which is what makes constant spans free: they
//! need no per-pixel work at all, only the carried sum.
//!
//! The delta distribution is the accumulation method Levien's `font-rs`
//! popularised. It is exact for straight edges, which is why the coverage
//! tests can assert an analytic area rather than a tolerance band.

use alloc::vec::Vec;

use otf_2d_engine_scene::FillRule;

use crate::binning::{SurfaceSize, TileBins, TileGeometry};
use crate::segment::Segment;

/// What a strip carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripKind {
    /// `width * rows` alpha bytes at `offset`, row-major: row `r`, column `x`
    /// is at `offset + r * width + x`.
    Alpha { offset: u32 },
    /// `rows` alpha bytes at `offset`, constant across the span.
    ///
    /// A span with every byte 255 is the *solid run*: fully inside the shape,
    /// costing one byte per row however wide it is.
    Uniform { offset: u32 },
}

/// One span of coverage within a band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Strip {
    /// First pixel column.
    pub x: u32,
    /// Band index. The first pixel row is `band * geometry.height`.
    pub band: u32,
    /// Width in pixels.
    pub width: u32,
    /// Rows this strip covers. Less than the band height only in a final band
    /// that the surface cuts short.
    pub rows: u16,
    pub kind: StripKind,
}

impl Strip {
    /// True when the span has one alpha per row rather than per pixel.
    #[inline]
    pub fn is_uniform(&self) -> bool {
        matches!(self.kind, StripKind::Uniform { .. })
    }

    /// True when every row of the span is fully covered — a solid run.
    pub fn is_solid(&self, strips: &Strips<'_>) -> bool {
        self.is_uniform() && strips.strip_alphas(self).iter().all(|&a| a == 255)
    }
}

/// What stage 5 produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StripStats {
    /// Bands that produced at least one strip.
    pub bands: usize,
    /// Spans with per-pixel coverage.
    pub alpha_strips: usize,
    /// Spans with constant coverage, solid runs included.
    pub uniform_strips: usize,
    /// Alpha bytes stored for per-pixel spans. This is the number that must
    /// stay proportional to a shape's perimeter rather than its area.
    pub alpha_pixels: usize,
}

/// Sparse strips for one draw.
#[derive(Debug, Clone, Copy)]
pub struct Strips<'a> {
    geometry: TileGeometry,
    surface: SurfaceSize,
    strips: &'a [Strip],
    alphas: &'a [u8],
    stats: StripStats,
}

impl<'a> Strips<'a> {
    #[inline]
    pub fn geometry(&self) -> TileGeometry {
        self.geometry
    }

    #[inline]
    pub fn surface(&self) -> SurfaceSize {
        self.surface
    }

    #[inline]
    pub fn strips(&self) -> &'a [Strip] {
        self.strips
    }

    #[inline]
    pub fn alphas(&self) -> &'a [u8] {
        self.alphas
    }

    #[inline]
    pub fn stats(&self) -> StripStats {
        self.stats
    }

    /// The range of [`Strips::strips`] belonging to one band.
    ///
    /// Strips are emitted band by band, so this is a binary search rather than
    /// a scan — which matters once bands are handed to different workers.
    pub fn band_range(&self, band: u32) -> core::ops::Range<usize> {
        let start = self.strips.partition_point(|s| s.band < band);
        let end = self.strips.partition_point(|s| s.band <= band);
        start..end
    }

    /// The strips belonging to one band.
    pub fn band_strips(&self, band: u32) -> &'a [Strip] {
        let range = self.band_range(band);
        &self.strips[range]
    }

    /// The alpha bytes belonging to a strip.
    pub fn strip_alphas(&self, strip: &Strip) -> &'a [u8] {
        let (offset, len) = match strip.kind {
            StripKind::Alpha { offset } => (offset, strip.rows as usize * strip.width as usize),
            StripKind::Uniform { offset } => (offset, strip.rows as usize),
        };
        let start = offset as usize;
        self.alphas
            .get(start..start.saturating_add(len))
            .unwrap_or(&[])
    }

    /// Coverage at a pixel within a strip.
    pub fn coverage(&self, strip: &Strip, row: u16, column: u32) -> u8 {
        if row >= strip.rows || column >= strip.width {
            return 0;
        }
        let alphas = self.strip_alphas(strip);
        match strip.kind {
            StripKind::Alpha { .. } => alphas
                .get(row as usize * strip.width as usize + column as usize)
                .copied()
                .unwrap_or(0),
            StripKind::Uniform { .. } => alphas.get(row as usize).copied().unwrap_or(0),
        }
    }
}

/// Reusable stage 5 workspace.
///
/// Held across frames so a steady-state pass allocates nothing (I-9).
#[derive(Debug, Clone, Default)]
pub struct Striper {
    strips: Vec<Strip>,
    alphas: Vec<u8>,
    /// Signed-area deltas for one run of adjacent tiles: `rows` scanlines of
    /// `stride` cells.
    areas: Vec<f32>,
    /// Running prefix sum per row, carried across gaps within a band.
    running: Vec<f32>,
    /// Coverage of the column being classified.
    column: Vec<u8>,
    /// Coverage of the run of identical columns being counted.
    group: Vec<u8>,
    group_x: u32,
    group_count: u32,
    /// Per-column coverage of the per-pixel span being built. Column-major;
    /// transposed on flush.
    columns: Vec<u8>,
    alpha_x: u32,
    alpha_active: bool,
    /// Segment indices of the current run, deduplicated.
    run_segments: Vec<u32>,
    stats: StripStats,
}

/// How many identical adjacent columns are worth a constant span of their own.
///
/// A constant span costs `rows` bytes however wide it is, but committing one
/// mid-edge also ends the per-pixel span either side of it, and a strip record
/// costs more than a handful of alpha bytes. Eight is where the saving covers
/// the split; the interiors this exists to catch are hundreds of columns wide,
/// so the exact threshold does not matter to them.
const MIN_UNIFORM_RUN: u32 = 8;

impl Striper {
    pub fn new() -> Striper {
        Striper::default()
    }

    /// Bytes currently held.
    pub fn memory_usage(&self) -> usize {
        core::mem::size_of_val(&self.strips[..])
            + self.alphas.capacity()
            + core::mem::size_of_val(&self.areas[..])
            + self.columns.capacity()
            + self.group.capacity()
            + core::mem::size_of_val(&self.run_segments[..])
    }

    /// Builds a single band containing one per-pixel span of the given
    /// coverage.
    ///
    /// Stage 6 is the one place where a scalar and a SIMD kernel must agree
    /// bit for bit, and proving that needs every combination of coverage and
    /// destination byte — not just the ones a picture happens to contain.
    /// This is how a test hands stage 6 a chosen coverage run.
    ///
    /// Not part of the stable API: strip layout is an internal format
    /// (Doc 02 §8).
    #[doc(hidden)]
    pub fn from_coverage(&mut self, coverage: &[u8], width: u32) -> Strips<'_> {
        self.strips.clear();
        self.alphas.clear();
        self.stats = StripStats::default();

        let width = width.min(coverage.len() as u32);
        if width > 0 {
            self.alphas.extend_from_slice(&coverage[..width as usize]);
            self.strips.push(Strip {
                x: 0,
                band: 0,
                width,
                rows: 1,
                kind: StripKind::Alpha { offset: 0 },
            });
            self.stats.bands = 1;
            self.stats.alpha_strips = 1;
            self.stats.alpha_pixels = width as usize;
        }
        Strips {
            geometry: TileGeometry::new(width.max(1) as u16, 1),
            surface: SurfaceSize::new(width, 1),
            strips: &self.strips,
            alphas: &self.alphas,
            stats: self.stats,
        }
    }

    /// Generates strips for one draw's binned segments.
    pub fn generate<'a>(&'a mut self, bins: &TileBins<'_>, rule: FillRule) -> Strips<'a> {
        self.strips.clear();
        self.alphas.clear();
        self.stats = StripStats::default();

        let geometry = bins.geometry();
        let surface = bins.surface();
        let tile_height = geometry.height as u32;
        let tile_width = geometry.width as u32;

        let mut tile_index = 0;
        let tiles = bins.tiles();
        while tile_index < tiles.len() {
            let band = tiles[tile_index].y as u32;
            let band_start = tile_index;
            while tile_index < tiles.len() && tiles[tile_index].y as u32 == band {
                tile_index += 1;
            }
            let band_tiles = &tiles[band_start..tile_index];

            let band_top = band * tile_height;
            if band_top >= surface.height {
                continue;
            }
            let rows = (surface.height - band_top).min(tile_height) as u16;
            let before = self.strips.len();
            self.band(bins, band_tiles, band, rows, rule, tile_width, surface);
            if self.strips.len() > before {
                self.stats.bands += 1;
            }
        }
        Strips {
            geometry,
            surface,
            strips: &self.strips,
            alphas: &self.alphas,
            stats: self.stats,
        }
    }

    /// Emits one band's strips.
    #[allow(clippy::too_many_arguments)]
    fn band(
        &mut self,
        bins: &TileBins<'_>,
        band_tiles: &[crate::binning::TileEntry],
        band: u32,
        rows: u16,
        rule: FillRule,
        tile_width: u32,
        surface: SurfaceSize,
    ) {
        let rows_usize = rows as usize;
        self.running.clear();
        self.running.resize(rows_usize, 0.0);
        self.column.clear();
        self.column.resize(rows_usize, 0);
        self.group.clear();
        self.group.resize(rows_usize, 0);
        self.group_count = 0;
        self.alpha_active = false;

        let band_top = (band * bins.geometry().height as u32) as f32;
        let mut cursor = 0u32;
        let mut run_start = 0usize;

        while run_start < band_tiles.len() {
            // A maximal run of adjacent tile columns. Segments assigned to a
            // tile in the run cannot reach past it: if they did, the next tile
            // would have been touched and would be part of the run.
            let mut run_end = run_start + 1;
            while run_end < band_tiles.len()
                && band_tiles[run_end].x == band_tiles[run_end - 1].x + 1
            {
                run_end += 1;
            }
            let run = &band_tiles[run_start..run_end];
            let x_lo = run[0].x as u32 * tile_width;
            let x_hi = ((run[run.len() - 1].x as u32 + 1) * tile_width).min(surface.width);
            run_start = run_end;
            if x_hi <= x_lo {
                continue;
            }

            self.gap(band, rows, cursor, x_lo);
            self.run(bins, run, band_top, rows, rule, x_lo, x_hi, band);
            cursor = x_hi;
        }

        self.gap(band, rows, cursor, surface.width);
    }

    /// Emits a constant span for `[x0, x1)`, whose coverage is whatever the
    /// running sum carries. No segment crosses it, so there is nothing to
    /// accumulate — this is where a solid interior costs nothing.
    fn gap(&mut self, band: u32, rows: u16, x0: u32, x1: u32) {
        if x1 <= x0 {
            return;
        }
        let mut any = false;
        for row in 0..rows as usize {
            let alpha = to_alpha(self.running[row]);
            self.column[row] = alpha;
            any |= alpha != 0;
        }
        if !any {
            return;
        }
        let offset = self.alphas.len() as u32;
        self.alphas.extend_from_slice(&self.column[..rows as usize]);
        self.strips.push(Strip {
            x: x0,
            band,
            width: x1 - x0,
            rows,
            kind: StripKind::Uniform { offset },
        });
        self.stats.uniform_strips += 1;
    }

    /// Accumulates a run of adjacent tiles and emits its spans.
    #[allow(clippy::too_many_arguments)]
    fn run(
        &mut self,
        bins: &TileBins<'_>,
        run: &[crate::binning::TileEntry],
        band_top: f32,
        rows: u16,
        rule: FillRule,
        x_lo: u32,
        x_hi: u32,
        band: u32,
    ) {
        let width = (x_hi - x_lo) as usize;
        // Two spare cells: a delta can land on the column just past the run's
        // right edge, and the two-cell case can reach one further. Both are
        // carried into the running sum rather than dropped.
        let stride = width + 2;
        self.areas.clear();
        self.areas.resize(stride * rows as usize, 0.0);

        // Deduplicate: a segment crossing two tiles of the run appears in
        // both, and must be accumulated once. Sorting also fixes the order,
        // which matters because floating-point addition is not associative.
        self.run_segments.clear();
        for tile in run {
            self.run_segments.extend_from_slice(bins.tile_indices(tile));
        }
        self.run_segments.sort_unstable();
        self.run_segments.dedup();

        for i in 0..self.run_segments.len() {
            let index = self.run_segments[i] as usize;
            let Some(segment) = bins.segments().get(index).copied() else {
                continue;
            };
            accumulate(
                &mut self.areas,
                stride,
                rows,
                segment,
                band_top,
                x_lo as f32,
                x_hi as f32,
            );
        }

        self.group_count = 0;
        self.alpha_active = false;
        self.columns.clear();
        for x in 0..width {
            let mut any = false;
            for row in 0..rows as usize {
                self.running[row] += self.areas[row * stride + x];
                let alpha = to_alpha_with(self.running[row], rule);
                self.column[row] = alpha;
                any |= alpha != 0;
            }
            if !any {
                self.close_group(band, rows);
                self.flush_alpha(band, rows);
                continue;
            }
            self.take_column(x_lo + x as u32, rows, band);
        }
        self.close_group(band, rows);
        self.flush_alpha(band, rows);

        // Deltas past the run's right edge belong to the gap that follows.
        for row in 0..rows as usize {
            self.running[row] += self.areas[row * stride + width];
            self.running[row] += self.areas[row * stride + width + 1];
        }
    }

    /// Folds the classified column into the run of identical columns being
    /// counted, closing the previous run when it differs.
    fn take_column(&mut self, x: u32, rows: u16, band: u32) {
        let rows_usize = rows as usize;
        if self.group_count > 0 && self.column[..rows_usize] == self.group[..rows_usize] {
            self.group_count += 1;
            return;
        }
        self.close_group(band, rows);
        self.group[..rows_usize].copy_from_slice(&self.column[..rows_usize]);
        self.group_x = x;
        self.group_count = 1;
    }

    /// Commits the run of identical columns: as a constant span when it is
    /// long enough to be worth one, otherwise folded into the per-pixel span.
    fn close_group(&mut self, band: u32, rows: u16) {
        if self.group_count == 0 {
            return;
        }
        let rows_usize = rows as usize;
        let count = self.group_count;
        let x = self.group_x;
        self.group_count = 0;

        if count >= MIN_UNIFORM_RUN {
            self.flush_alpha(band, rows);
            let offset = self.alphas.len() as u32;
            self.alphas.extend_from_slice(&self.group[..rows_usize]);
            self.strips.push(Strip {
                x,
                band,
                width: count,
                rows,
                kind: StripKind::Uniform { offset },
            });
            self.stats.uniform_strips += 1;
            return;
        }

        if !self.alpha_active {
            self.alpha_active = true;
            self.alpha_x = x;
            self.columns.clear();
        }
        for _ in 0..count {
            self.columns.extend_from_slice(&self.group[..rows_usize]);
        }
    }

    /// Commits the per-pixel span.
    fn flush_alpha(&mut self, band: u32, rows: u16) {
        if !self.alpha_active {
            return;
        }
        self.alpha_active = false;
        let rows_usize = rows as usize;
        let width = self.columns.len() / rows_usize;
        if width == 0 {
            return;
        }
        let offset = self.alphas.len() as u32;
        // Column-major to row-major: the fine loop walks `x` within a row, so
        // that is the direction the bytes must be contiguous in.
        for row in 0..rows_usize {
            for column in 0..width {
                self.alphas.push(self.columns[column * rows_usize + row]);
            }
        }
        self.columns.clear();
        self.strips.push(Strip {
            x: self.alpha_x,
            band,
            width: width as u32,
            rows,
            kind: StripKind::Alpha { offset },
        });
        self.stats.alpha_strips += 1;
        self.stats.alpha_pixels += width * rows_usize;
    }
}

/// Signed area to coverage under the non-zero rule.
#[inline]
fn coverage_non_zero(area: f32) -> f32 {
    area.abs().min(1.0)
}

/// Signed area to coverage under the even-odd rule.
///
/// A triangle wave of period 2: winding 1 is inside, 2 is outside, 3 inside.
/// The integer cast rather than `%` keeps this buildable with no float-math
/// backend; it saturates, which is the right answer for a winding number large
/// enough to overflow `i32`.
#[inline]
fn coverage_even_odd(area: f32) -> f32 {
    let a = area.abs();
    let folded = a - 2.0 * ((a * 0.5) as i32 as f32);
    if folded > 1.0 { 2.0 - folded } else { folded }
}

#[inline]
fn to_alpha_with(area: f32, rule: FillRule) -> u8 {
    let coverage = match rule {
        FillRule::NonZero => coverage_non_zero(area),
        FillRule::EvenOdd => coverage_even_odd(area),
    };
    quantize(coverage)
}

/// Gap coverage, which is always a whole winding number, so the rule does not
/// change the answer for the cases a gap can be in.
#[inline]
fn to_alpha(area: f32) -> u8 {
    quantize(coverage_non_zero(area))
}

#[inline]
fn quantize(coverage: f32) -> u8 {
    if coverage.is_nan() || coverage <= 0.0 {
        return 0;
    }
    let scaled = coverage * 255.0 + 0.5;
    if scaled >= 255.0 { 255 } else { scaled as u8 }
}

/// `floor` for a non-negative value, without a float-math backend.
#[inline]
fn floor_nonneg(v: f32) -> f32 {
    (v as i32) as f32
}

/// `ceil` for a non-negative value, without a float-math backend.
#[inline]
fn ceil_nonneg(v: f32) -> f32 {
    let truncated = (v as i32) as f32;
    if truncated == v {
        truncated
    } else {
        truncated + 1.0
    }
}

/// Deposits one segment's signed-area deltas into `areas`.
///
/// `x` is clamped to the run's slab. Clamping preserves winding — it moves the
/// off-slab part onto the boundary, which is exactly what a shape extending
/// past the surface edge should contribute — while keeping every write in
/// range.
fn accumulate(
    areas: &mut [f32],
    stride: usize,
    rows: u16,
    segment: Segment,
    band_top: f32,
    x_lo: f32,
    x_hi: f32,
) {
    if !segment.is_finite() || segment.is_horizontal() {
        // A horizontal edge crosses no scanline, so it carries no winding.
        return;
    }
    let (direction, ax, ay, bx, by) = if segment.y0 < segment.y1 {
        (1.0f32, segment.x0, segment.y0, segment.x1, segment.y1)
    } else {
        (-1.0f32, segment.x1, segment.y1, segment.x0, segment.y0)
    };

    let band_bottom = band_top + rows as f32;
    let y_start = ay.max(band_top);
    let y_end = by.min(band_bottom);
    if y_end <= y_start {
        return;
    }
    let dxdy = (bx - ax) / (by - ay);
    let x_at = |y: f32| ax + (y - ay) * dxdy;

    let first_row = floor_nonneg(y_start - band_top) as usize;
    let last_row = (ceil_nonneg(y_end - band_top) as usize).min(rows as usize);

    for row in first_row..last_row {
        let top = y_start.max(band_top + row as f32);
        let bottom = y_end.min(band_top + row as f32 + 1.0);
        let dy = bottom - top;
        if dy <= 0.0 {
            continue;
        }
        let xa = x_at(top).clamp(x_lo, x_hi) - x_lo;
        let xb = x_at(bottom).clamp(x_lo, x_hi) - x_lo;
        let (x0, x1) = if xa < xb { (xa, xb) } else { (xb, xa) };
        let delta = dy * direction;
        let cells = &mut areas[row * stride..(row + 1) * stride];
        deposit(cells, x0, x1, delta);
    }
}

/// Distributes `delta` across the cells the sub-segment `[x0, x1]` covers.
///
/// This is the exact-area distribution: within one scanline the edge is a
/// straight line, so the area it cuts from each pixel column has a closed
/// form. No sampling, no approximation.
fn deposit(cells: &mut [f32], x0: f32, x1: f32, delta: f32) {
    let x0_floor = floor_nonneg(x0);
    let x0i = x0_floor as usize;
    let x1_ceil = ceil_nonneg(x1);
    let x1i = x1_ceil as usize;

    if x1i <= x0i + 1 {
        // The whole crossing sits inside one pixel column: split the delta
        // between that column and the next by the crossing's midpoint.
        let midpoint = 0.5 * (x0 + x1) - x0_floor;
        add(cells, x0i, delta - delta * midpoint);
        add(cells, x0i + 1, delta * midpoint);
        return;
    }

    let inverse = (x1 - x0).recip();
    let x0_frac = x0 - x0_floor;
    let first = 0.5 * inverse * (1.0 - x0_frac) * (1.0 - x0_frac);
    let x1_frac = x1 - x1_ceil + 1.0;
    let last = 0.5 * inverse * x1_frac * x1_frac;

    add(cells, x0i, delta * first);
    if x1i == x0i + 2 {
        add(cells, x0i + 1, delta * (1.0 - first - last));
    } else {
        let second = inverse * (1.5 - x0_frac);
        add(cells, x0i + 1, delta * (second - first));
        for cell in x0i + 2..x1i - 1 {
            add(cells, cell, delta * inverse);
        }
        let before_last = second + (x1i - x0i - 3) as f32 * inverse;
        add(cells, x1i - 1, delta * (1.0 - before_last - last));
    }
    add(cells, x1i, delta * last);
}

#[inline]
fn add(cells: &mut [f32], index: usize, value: f32) {
    if let Some(cell) = cells.get_mut(index) {
        *cell += value;
    }
}
