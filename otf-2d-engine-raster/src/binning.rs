//! Stage 4 — bin (Doc 01 §4).
//!
//! Assigns each device-space segment to every tile it touches. Two properties
//! matter and both are load-bearing downstream:
//!
//! * **Sparse.** Storage is proportional to the area the geometry covers, not
//!   to the surface. A 4K surface is ~2M pixels; a UI frame touches a small
//!   fraction, and a full-surface tile grid would dominate the frame budget
//!   before any pixel was written.
//! * **Deterministic.** Segments within a tile always come out in the same
//!   order, because stage 5 accumulates signed area and floating-point
//!   addition is not associative. Non-deterministic order would make output
//!   depend on iteration order, which would break I-5 and I-6 outright.
//!
//! Tile geometry is a runtime parameter, never a constant (Q-01). The starting
//! point is 256×4 — height 4 aligns with 128-bit SIMD lanes — but every
//! algorithm here reads it from [`TileGeometry`] so benchmarking can move it.

use alloc::vec::Vec;

use crate::segment::Segment;

/// Tile dimensions in device pixels.
///
/// A "wide tile" is short and wide: stage 6 walks it as `height` scanlines of
/// `width` pixels, which is the shape SIMD wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileGeometry {
    pub width: u16,
    pub height: u16,
}

impl TileGeometry {
    /// The starting point from Doc 01 §4: 256 wide, 4 tall.
    pub const DEFAULT: TileGeometry = TileGeometry {
        width: 256,
        height: 4,
    };

    /// Both dimensions clamped to at least 1, so a zero cannot divide.
    pub const fn new(width: u16, height: u16) -> TileGeometry {
        TileGeometry {
            width: if width == 0 { 1 } else { width },
            height: if height == 0 { 1 } else { height },
        }
    }
}

impl Default for TileGeometry {
    fn default() -> TileGeometry {
        TileGeometry::DEFAULT
    }
}

/// The render target's pixel extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SurfaceSize {
    pub width: u32,
    pub height: u32,
}

impl SurfaceSize {
    pub const fn new(width: u32, height: u32) -> SurfaceSize {
        SurfaceSize { width, height }
    }

    /// How many tiles across and down cover this surface.
    pub const fn tile_grid(&self, geometry: TileGeometry) -> (u32, u32) {
        let w = geometry.width as u32;
        let h = geometry.height as u32;
        (self.width.div_ceil(w), self.height.div_ceil(h))
    }
}

/// One tile that at least one segment touches.
///
/// Tiles nothing touches have no entry at all — that is what "sparse" means
/// here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileEntry {
    /// Tile column.
    pub x: u16,
    /// Tile row.
    pub y: u16,
    /// Start of this tile's run in [`TileBins::indices`].
    pub offset: u32,
    /// Segments in the run.
    pub len: u32,
}

/// What binning did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BinStats {
    /// Segments offered.
    pub segments_in: usize,
    /// Segments dropped as non-finite or entirely off-surface.
    pub segments_dropped: usize,
    /// Tile assignments made. The sum of every tile's `len`.
    pub assignments: usize,
    /// Tiles with at least one segment.
    pub tiles: usize,
}

/// Per-tile segment lists.
#[derive(Debug, Clone, Copy)]
pub struct TileBins<'a> {
    geometry: TileGeometry,
    surface: SurfaceSize,
    columns: u32,
    rows: u32,
    segments: &'a [Segment],
    tiles: &'a [TileEntry],
    indices: &'a [u32],
    stats: BinStats,
}

impl<'a> TileBins<'a> {
    #[inline]
    pub fn geometry(&self) -> TileGeometry {
        self.geometry
    }

    #[inline]
    pub fn surface(&self) -> SurfaceSize {
        self.surface
    }

    /// Tiles across and down. Note this is the *grid* size; only the tiles in
    /// [`TileBins::tiles`] have segments.
    #[inline]
    pub fn grid(&self) -> (u32, u32) {
        (self.columns, self.rows)
    }

    /// Every segment offered to the binner, indexed by [`TileBins::indices`].
    #[inline]
    pub fn segments(&self) -> &'a [Segment] {
        self.segments
    }

    /// Occupied tiles, ordered by row then column.
    #[inline]
    pub fn tiles(&self) -> &'a [TileEntry] {
        self.tiles
    }

    /// Segment indices, grouped into per-tile runs.
    #[inline]
    pub fn indices(&self) -> &'a [u32] {
        self.indices
    }

    #[inline]
    pub fn stats(&self) -> BinStats {
        self.stats
    }

    /// The segment indices belonging to a tile.
    pub fn tile_indices(&self, tile: &TileEntry) -> &'a [u32] {
        let start = tile.offset as usize;
        self.indices
            .get(start..start.saturating_add(tile.len as usize))
            .unwrap_or(&[])
    }

    /// The segments belonging to a tile, in accumulation order.
    pub fn tile_segments(&self, tile: &TileEntry) -> impl Iterator<Item = Segment> + 'a {
        let segments = self.segments;
        self.tile_indices(tile)
            .iter()
            .filter_map(move |&i| segments.get(i as usize).copied())
    }

    /// The device-space origin of a tile, in pixels.
    pub fn tile_origin(&self, tile: &TileEntry) -> (u32, u32) {
        (
            tile.x as u32 * self.geometry.width as u32,
            tile.y as u32 * self.geometry.height as u32,
        )
    }
}

/// Reusable binning workspace.
///
/// Held across frames so a steady-state bin allocates nothing (I-9).
#[derive(Debug, Clone, Default)]
pub struct Binner {
    /// `(tile_index, segment_index)` packed into one `u64` so the sort is a
    /// single integer comparison and the result is totally ordered — which is
    /// what makes the output deterministic rather than merely reproducible on
    /// one machine.
    pairs: Vec<u64>,
    tiles: Vec<TileEntry>,
    indices: Vec<u32>,
    stats: BinStats,
}

impl Binner {
    pub fn new() -> Binner {
        Binner::default()
    }

    /// Bytes currently held.
    pub fn memory_usage(&self) -> usize {
        core::mem::size_of_val(&self.pairs[..])
            + core::mem::size_of_val(&self.tiles[..])
            + core::mem::size_of_val(&self.indices[..])
    }

    /// Assigns `segments` to tiles.
    pub fn bin<'a>(
        &'a mut self,
        segments: &'a [Segment],
        geometry: TileGeometry,
        surface: SurfaceSize,
    ) -> TileBins<'a> {
        self.pairs.clear();
        self.tiles.clear();
        self.indices.clear();
        self.stats = BinStats::default();
        self.stats.segments_in = segments.len();

        let (columns, rows) = surface.tile_grid(geometry);
        if columns == 0 || rows == 0 {
            self.stats.segments_dropped = segments.len();
            return self.finish(segments, geometry, surface, columns, rows);
        }

        for (index, segment) in segments.iter().enumerate() {
            if !self.assign(segment, index as u32, geometry, surface, columns, rows) {
                self.stats.segments_dropped += 1;
            }
        }

        // A total order on `(tile, segment)`. `sort_unstable` allocates
        // nothing, and the keys are unique — a segment lands in a given tile
        // at most once — so "unstable" costs no determinism.
        self.pairs.sort_unstable();

        self.indices.reserve(self.pairs.len());
        let mut current: Option<u32> = None;
        for &pair in &self.pairs {
            let tile = (pair >> 32) as u32;
            let segment = pair as u32;
            if current != Some(tile) {
                self.tiles.push(TileEntry {
                    x: (tile % columns) as u16,
                    y: (tile / columns) as u16,
                    offset: self.indices.len() as u32,
                    len: 0,
                });
                current = Some(tile);
            }
            self.indices.push(segment);
            if let Some(entry) = self.tiles.last_mut() {
                entry.len += 1;
            }
        }

        self.stats.assignments = self.indices.len();
        self.stats.tiles = self.tiles.len();
        self.finish(segments, geometry, surface, columns, rows)
    }

    fn finish<'a>(
        &'a self,
        segments: &'a [Segment],
        geometry: TileGeometry,
        surface: SurfaceSize,
        columns: u32,
        rows: u32,
    ) -> TileBins<'a> {
        TileBins {
            geometry,
            surface,
            columns,
            rows,
            segments,
            tiles: &self.tiles,
            indices: &self.indices,
            stats: self.stats,
        }
    }

    /// Emits one pair per tile the segment touches. Returns false when the
    /// segment contributes nothing.
    fn assign(
        &mut self,
        segment: &Segment,
        index: u32,
        geometry: TileGeometry,
        surface: SurfaceSize,
        columns: u32,
        rows: u32,
    ) -> bool {
        if !segment.is_finite() {
            return false;
        }
        let tile_h = geometry.height as f64;
        let tile_w = geometry.width as f64;
        let min_y = segment.min_y() as f64;
        let max_y = segment.max_y() as f64;
        if max_y <= 0.0 || min_y >= surface.height as f64 {
            return false;
        }

        let (first_row, last_row) = band_range(min_y, max_y, tile_h, rows);
        let mut touched = false;

        for row in first_row..=last_row {
            // Clip the segment to this band. Because a line is monotone in `x`
            // over any `y` interval, the two ends of the clipped piece bracket
            // its whole horizontal extent — no sampling needed.
            let band_top = (row as f64) * tile_h;
            let band_bottom = band_top + tile_h;
            let (lo, hi) = if segment.is_horizontal() {
                (segment.min_x() as f64, segment.max_x() as f64)
            } else {
                let y_enter = min_y.max(band_top);
                let y_exit = max_y.min(band_bottom);
                let a = segment.x_at(y_enter);
                let b = segment.x_at(y_exit);
                (a.min(b), a.max(b))
            };

            // Only the right edge drops a band. A segment left of the surface
            // still carries winding onto it — dropping it would leave the
            // on-surface part of a shape that extends off the left edge
            // unfilled — so it clamps into column 0 instead.
            if lo >= surface.width as f64 {
                continue;
            }
            let (first_col, last_col) = band_range(lo, hi, tile_w, columns);
            for col in first_col..=last_col {
                let tile = row * columns + col;
                self.pairs.push(((tile as u64) << 32) | index as u64);
                touched = true;
            }
        }
        touched
    }
}

/// The inclusive range of bands of width `size` spanned by `[lo, hi]`.
///
/// Bands are half-open: a coordinate on a boundary opens the band above it. So
/// an extent that *ends* exactly on a boundary does not reach into the next
/// band — it touches it along a zero-width line, which contributes no area.
/// Getting this right is what keeps a 45° diagonal in the two tiles it crosses
/// instead of the four its bounding box covers.
///
/// Negative coordinates clamp to band 0. The caller has already established
/// that part of the extent is on-surface, so clamping widens the range rather
/// than misplacing it.
#[inline]
fn band_range(lo: f64, hi: f64, size: f64, count: u32) -> (u32, u32) {
    let last_band = count.saturating_sub(1) as u64;
    let floor = |v: f64| -> u64 { if v <= 0.0 { 0 } else { (v / size) as u64 } };

    let first = floor(lo).min(last_band);
    let mut raw = floor(hi);
    // The adjustment happens before clamping: clamping first would collapse an
    // off-surface upper bound onto the last band and then wrongly retreat from
    // it.
    // `raw as f64 * size == hi` rather than `(hi / size).fract() == 0.0`:
    // `fract` needs a float-math backend, and a multiply-and-compare needs
    // nothing, which keeps this file buildable on a bare `no_std` target.
    if hi > lo && hi > 0.0 && raw > 0 && (raw as f64) * size == hi {
        raw -= 1;
    }
    let last = raw.min(last_band).max(first);
    (first as u32, last as u32)
}
