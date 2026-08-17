//! T2.2 acceptance tests for stage 5.
//!
//! Two properties from the plan: a full-surface opaque rect produces solid
//! runs rather than per-pixel alpha, and coverage equals the exact analytic
//! area for axis-aligned and 45° edges within 1/255. The third — that no
//! supersampling path exists — is a grep gate in `ci/invariants.sh`.

use otf_2d_engine_raster::{
    Binner, Segment, StripKind, Striper, Strips, SurfaceSize, TileGeometry,
};
use otf_2d_engine_scene::FillRule;
use otf_2d_engine_testing::alloc::{CountingAllocator, measure};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator::new();

/// Closes a polygon into directed segments.
fn polygon(points: &[(f32, f32)]) -> Vec<Segment> {
    let mut segments = Vec::with_capacity(points.len());
    for i in 0..points.len() {
        let (x0, y0) = points[i];
        let (x1, y1) = points[(i + 1) % points.len()];
        segments.push(Segment::new(x0, y0, x1, y1));
    }
    segments
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<Segment> {
    polygon(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
}

/// Expands strips back into a dense coverage image, which is what the fine
/// stage will do for real. Everything a strip does not cover is zero.
fn expand(strips: &Strips<'_>) -> Vec<u8> {
    let surface = strips.surface();
    let mut image = vec![0u8; (surface.width * surface.height) as usize];
    let band_height = strips.geometry().height as u32;
    for strip in strips.strips() {
        let top = strip.band * band_height;
        for row in 0..strip.rows as u32 {
            let y = top + row;
            if y >= surface.height {
                continue;
            }
            for column in 0..strip.width {
                let x = strip.x + column;
                if x >= surface.width {
                    continue;
                }
                let alpha = strips.coverage(strip, row as u16, column);
                let at = (y * surface.width + x) as usize;
                assert_eq!(image[at], 0, "strips overlap at ({x}, {y})");
                image[at] = alpha;
            }
        }
    }
    image
}

struct Rasterized {
    image: Vec<u8>,
    alpha_strips: usize,
    uniform_strips: usize,
    alpha_pixels: usize,
    solid_strips: usize,
}

fn rasterize(
    segments: &[Segment],
    rule: FillRule,
    surface: SurfaceSize,
    geometry: TileGeometry,
) -> Rasterized {
    let mut binner = Binner::new();
    let bins = binner.bin(segments, geometry, surface);
    let mut striper = Striper::new();
    let strips = striper.generate(&bins, rule);
    let stats = strips.stats();
    let solid_strips = strips
        .strips()
        .iter()
        .filter(|s| s.is_solid(&strips))
        .count();
    Rasterized {
        image: expand(&strips),
        alpha_strips: stats.alpha_strips,
        uniform_strips: stats.uniform_strips,
        alpha_pixels: stats.alpha_pixels,
        solid_strips,
    }
}

/// Total coverage in pixels, as an area.
fn covered_area(image: &[u8]) -> f64 {
    image.iter().map(|&a| a as f64 / 255.0).sum()
}

// ------------------------------------------------------------ solid runs

#[test]
fn a_pixel_aligned_full_surface_rect_is_entirely_solid() {
    let surface = SurfaceSize::new(64, 64);
    let out = rasterize(
        &rect(0.0, 0.0, 64.0, 64.0),
        FillRule::NonZero,
        surface,
        TileGeometry::new(16, 4),
    );

    assert!(
        out.image.iter().all(|&a| a == 255),
        "the surface is not fully covered"
    );
    assert_eq!(
        out.alpha_strips, 0,
        "an aligned rect needs no per-pixel coverage"
    );
    assert_eq!(out.alpha_pixels, 0);
    assert_eq!(
        out.uniform_strips, out.solid_strips,
        "every span is a solid run"
    );
    assert_eq!(
        out.uniform_strips, 32,
        "16 bands, each a tile-wide run plus the gap to the surface edge"
    );
}

#[test]
fn alpha_cost_follows_the_perimeter_not_the_area() {
    // A rect inset by a fraction of a pixel, so every edge is antialiased.
    // Quadrupling the area must not quadruple the per-pixel coverage stored.
    let mut measurements = Vec::new();
    for size in [64u32, 128, 256, 512] {
        let n = size as f32;
        let out = rasterize(
            &rect(0.25, 0.25, n - 0.25, n - 0.25),
            FillRule::NonZero,
            SurfaceSize::new(size, size),
            TileGeometry::DEFAULT,
        );
        measurements.push((size, out.alpha_pixels, out.alpha_strips));
    }

    for &(size, alpha_pixels, _) in &measurements {
        assert!(
            alpha_pixels <= 4 * size as usize,
            "alpha coverage is growing with area, not perimeter: {measurements:?}"
        );
    }
    // Doubling the edge length roughly doubles the cost; it would quadruple if
    // the interior were being stored per pixel.
    // 64 → 512 is 8× the edge length and 64× the area. Alpha cost must follow
    // the former; the slack is there for a different-but-still-linear split.
    let (_, small, _) = measurements[0];
    let (_, large, _) = measurements[3];
    assert!(
        large <= small * 12,
        "8× the perimeter cost {}× the alpha: {measurements:?}",
        large / small.max(1)
    );
}

#[test]
fn the_interior_of_an_antialiased_rect_is_still_solid() {
    let surface = SurfaceSize::new(32, 32);
    let out = rasterize(
        &rect(4.5, 4.5, 27.5, 27.5),
        FillRule::NonZero,
        surface,
        TileGeometry::new(16, 4),
    );
    assert!(out.solid_strips > 0, "no solid run in a 23×23 filled rect");
    // Interior pixels are fully covered, edges are half.
    assert_eq!(out.image[(10 * 32 + 10) as usize], 255);
    assert_eq!(
        out.image[(10 * 32 + 4) as usize],
        128,
        "a half-covered edge pixel"
    );
}

// ------------------------------------------------------------ exact area

/// Coverage must be the analytic area, so the sum over the image is the
/// polygon's area. One part in 255 per pixel is the quantisation floor; the
/// tolerance below is that, times the number of partially covered pixels.
fn assert_area(segments: &[Segment], expected: f64, partial_pixels: f64, label: &str) {
    for geometry in [
        TileGeometry::new(256, 4),
        TileGeometry::new(16, 4),
        TileGeometry::new(8, 8),
    ] {
        let out = rasterize(
            segments,
            FillRule::NonZero,
            SurfaceSize::new(64, 64),
            geometry,
        );
        let area = covered_area(&out.image);
        let tolerance = partial_pixels / 255.0 + 1e-6;
        assert!(
            (area - expected).abs() <= tolerance,
            "{label} at {geometry:?}: area {area} != {expected} (tolerance {tolerance})"
        );
    }
}

#[test]
fn an_axis_aligned_rect_covers_its_exact_area() {
    // 8.5 × 6.0 = 51.0, with 2 * (8.5 + 6.0) ≈ 29 partially covered pixels.
    assert_area(&rect(1.25, 2.5, 9.75, 8.5), 51.0, 40.0, "axis-aligned rect");
}

#[test]
fn a_pixel_aligned_rect_covers_its_exact_area_with_no_error_at_all() {
    let out = rasterize(
        &rect(8.0, 8.0, 24.0, 20.0),
        FillRule::NonZero,
        SurfaceSize::new(64, 64),
        TileGeometry::new(16, 4),
    );
    assert_eq!(covered_area(&out.image), 16.0 * 12.0);
}

#[test]
fn a_forty_five_degree_edge_covers_its_exact_area() {
    // A right triangle with legs of 20: area 200, hypotenuse at exactly 45°.
    assert_area(
        &polygon(&[(10.0, 10.0), (30.0, 10.0), (30.0, 30.0)]),
        200.0,
        60.0,
        "45° triangle",
    );
}

#[test]
fn a_forty_five_degree_diamond_covers_its_exact_area() {
    // Every edge at 45°, and none of them pixel-aligned.
    assert_area(
        &polygon(&[(20.5, 8.5), (32.5, 20.5), (20.5, 32.5), (8.5, 20.5)]),
        288.0,
        100.0,
        "45° diamond",
    );
}

#[test]
fn a_half_covered_pixel_reads_as_half() {
    // A 1-pixel-wide column split exactly down the middle.
    let out = rasterize(
        &rect(4.0, 4.0, 4.5, 8.0),
        FillRule::NonZero,
        SurfaceSize::new(16, 16),
        TileGeometry::new(16, 4),
    );
    for y in 4..8 {
        assert_eq!(out.image[y * 16 + 4], 128, "row {y}");
        assert_eq!(out.image[y * 16 + 5], 0);
    }
}

// ------------------------------------------------------------ fill rules

#[test]
fn the_two_fill_rules_disagree_where_they_should() {
    // Two overlapping squares wound the same way.
    let mut segments = rect(4.0, 4.0, 20.0, 20.0);
    segments.extend(rect(12.0, 12.0, 28.0, 28.0));
    let surface = SurfaceSize::new(32, 32);
    let geometry = TileGeometry::new(16, 4);

    let non_zero = rasterize(&segments, FillRule::NonZero, surface, geometry);
    let even_odd = rasterize(&segments, FillRule::EvenOdd, surface, geometry);

    let overlap = (16 * 32 + 16) as usize;
    let only_first = (6 * 32 + 6) as usize;
    assert_eq!(non_zero.image[overlap], 255, "non-zero fills the overlap");
    assert_eq!(even_odd.image[overlap], 0, "even-odd leaves a hole");
    assert_eq!(non_zero.image[only_first], 255);
    assert_eq!(even_odd.image[only_first], 255);
}

#[test]
fn a_reversed_subpath_cuts_a_hole_under_the_non_zero_rule() {
    let mut segments = rect(4.0, 4.0, 28.0, 28.0);
    // Wound the other way.
    segments.extend(polygon(&[
        (10.0, 10.0),
        (10.0, 22.0),
        (22.0, 22.0),
        (22.0, 10.0),
    ]));
    let out = rasterize(
        &segments,
        FillRule::NonZero,
        SurfaceSize::new(32, 32),
        TileGeometry::new(16, 4),
    );
    assert_eq!(out.image[(16 * 32 + 16) as usize], 0, "the hole is not cut");
    assert_eq!(out.image[(6 * 32 + 6) as usize], 255);
}

// ------------------------------------------------------------ clipping

#[test]
fn a_shape_extending_off_the_left_edge_is_still_filled() {
    let out = rasterize(
        &rect(-50.0, 4.0, 20.0, 12.0),
        FillRule::NonZero,
        SurfaceSize::new(32, 32),
        TileGeometry::new(16, 4),
    );
    for y in 4..12 {
        assert_eq!(out.image[y * 32], 255, "row {y} column 0");
        assert_eq!(out.image[y * 32 + 19], 255);
        assert_eq!(out.image[y * 32 + 20], 0);
    }
    assert_eq!(covered_area(&out.image), 20.0 * 8.0);
}

#[test]
fn a_shape_extending_past_every_edge_fills_the_surface() {
    let out = rasterize(
        &rect(-10.0, -10.0, 42.0, 42.0),
        FillRule::NonZero,
        SurfaceSize::new(32, 32),
        TileGeometry::new(16, 4),
    );
    assert!(out.image.iter().all(|&a| a == 255));
    assert_eq!(out.alpha_pixels, 0, "nothing on-surface is a partial edge");
}

#[test]
fn a_shape_entirely_off_surface_produces_nothing() {
    let out = rasterize(
        &rect(100.0, 100.0, 120.0, 120.0),
        FillRule::NonZero,
        SurfaceSize::new(32, 32),
        TileGeometry::new(16, 4),
    );
    assert_eq!(out.alpha_strips + out.uniform_strips, 0);
}

// ------------------------------------------------------------ determinism

#[test]
fn strip_generation_is_deterministic() {
    let segments: Vec<Segment> = (0..48)
        .map(|i| {
            let f = i as f32;
            Segment::new(
                (f * 7.3) % 31.0,
                (f * 13.1) % 29.0,
                (f * 3.7) % 31.0,
                (f * 11.9) % 29.0,
            )
        })
        .collect();
    let surface = SurfaceSize::new(32, 32);
    let geometry = TileGeometry::new(16, 4);

    let first = rasterize(&segments, FillRule::NonZero, surface, geometry).image;
    for run in 0..200 {
        let again = rasterize(&segments, FillRule::NonZero, surface, geometry).image;
        assert_eq!(again, first, "run {run} differed");
    }
}

#[test]
fn tile_geometry_does_not_change_the_result() {
    // Tile size is a performance parameter (Q-01). If it changed the pixels,
    // benchmarking it would be changing the picture.
    let segments = polygon(&[(3.5, 2.25), (28.75, 9.5), (17.0, 29.25), (6.0, 18.5)]);
    let surface = SurfaceSize::new(32, 32);
    let reference = rasterize(
        &segments,
        FillRule::NonZero,
        surface,
        TileGeometry::new(256, 4),
    )
    .image;

    for geometry in [
        TileGeometry::new(16, 4),
        TileGeometry::new(8, 8),
        TileGeometry::new(4, 2),
        TileGeometry::new(32, 1),
    ] {
        let image = rasterize(&segments, FillRule::NonZero, surface, geometry).image;
        let worst = image
            .iter()
            .zip(&reference)
            .map(|(a, b)| (*a as i32 - *b as i32).abs())
            .max()
            .unwrap_or(0);
        assert!(worst <= 1, "{geometry:?} differed by {worst}");
    }
}

// ------------------------------------------------------------ I-9

#[test]
fn a_second_pass_allocates_nothing() {
    let segments = rect(2.5, 2.5, 900.0, 500.0);
    let surface = SurfaceSize::new(1024, 512);
    let geometry = TileGeometry::DEFAULT;

    let mut binner = Binner::new();
    let mut striper = Striper::new();
    let bins = binner.bin(&segments, geometry, surface);
    let (first_count, first) =
        measure(|| striper.generate(&bins, FillRule::NonZero).strips().len());
    assert!(
        first.acquisitions() > 0,
        "the counting allocator is not installed"
    );

    let (second_count, second) =
        measure(|| striper.generate(&bins, FillRule::NonZero).strips().len());
    assert_eq!(first_count, second_count);
    assert_eq!(
        second.acquisitions(),
        0,
        "I-9: a steady-state strip pass allocated ({second:?})"
    );
}

#[test]
fn strip_kinds_carry_the_bytes_they_claim() {
    let segments = rect(1.5, 1.5, 30.5, 30.5);
    let surface = SurfaceSize::new(32, 32);
    let mut binner = Binner::new();
    let bins = binner.bin(&segments, TileGeometry::new(16, 4), surface);
    let mut striper = Striper::new();
    let strips = striper.generate(&bins, FillRule::NonZero);

    assert!(!strips.strips().is_empty());
    for strip in strips.strips() {
        let expected = match strip.kind {
            StripKind::Alpha { .. } => strip.rows as usize * strip.width as usize,
            StripKind::Uniform { .. } => strip.rows as usize,
        };
        assert_eq!(strips.strip_alphas(strip).len(), expected);
        assert!(strip.width > 0);
        assert!(strip.rows > 0);
    }
}
