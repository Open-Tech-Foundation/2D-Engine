//! T2.1 acceptance tests for stage 4.
//!
//! Three properties from the plan: only touched tiles allocate, tile geometry
//! is a runtime parameter, and segments within a tile are deterministically
//! ordered.

use otf_2d_engine_raster::{Binner, Segment, SurfaceSize, TileGeometry};
use otf_2d_engine_testing::alloc::{CountingAllocator, measure};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator::new();

/// The tiles a bin produced, as `(x, y)` pairs in output order.
fn occupied(
    binner: &mut Binner,
    segments: &[Segment],
    g: TileGeometry,
    s: SurfaceSize,
) -> Vec<(u16, u16)> {
    binner
        .bin(segments, g, s)
        .tiles()
        .iter()
        .map(|t| (t.x, t.y))
        .collect()
}

/// A closed rectangle as four directed segments.
fn rect_segments(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<Segment> {
    vec![
        Segment::new(x0, y0, x1, y0),
        Segment::new(x1, y0, x1, y1),
        Segment::new(x1, y1, x0, y1),
        Segment::new(x0, y1, x0, y0),
    ]
}

// ------------------------------------------------------------ sparsity

#[test]
fn only_touched_tiles_are_stored() {
    let segments = rect_segments(4.0, 4.0, 20.0, 20.0);
    let geometry = TileGeometry::new(16, 4);

    let mut binner = Binner::new();
    let small = binner
        .bin(&segments, geometry, SurfaceSize::new(64, 64))
        .stats();
    let large = binner
        .bin(&segments, geometry, SurfaceSize::new(4096, 4096))
        .stats();

    assert_eq!(
        small, large,
        "binning the same geometry must not depend on how much empty surface surrounds it"
    );
    assert!(small.tiles > 0);
    // The rect spans x 4..20 (tile columns 0..1) and y 4..20 (tile rows 1..5).
    assert!(
        small.tiles <= 2 * 5,
        "tiles are assigned per crossing, not per bbox cell"
    );
}

#[test]
fn storage_scales_with_covered_area_not_surface_area() {
    let segments = rect_segments(2.0, 2.0, 10.0, 10.0);
    let geometry = TileGeometry::DEFAULT;
    let mut binner = Binner::new();

    // Warm the buffers so the measurement sees steady state, then bin into a
    // surface 4096× larger in area and assert nothing more is allocated.
    let _ = binner.bin(&segments, geometry, SurfaceSize::new(64, 64));
    let (small, _) = measure(|| {
        binner
            .bin(&segments, geometry, SurfaceSize::new(64, 64))
            .stats()
            .assignments
    });
    let (large, counters) = measure(|| {
        binner
            .bin(&segments, geometry, SurfaceSize::new(4096, 4096))
            .stats()
            .assignments
    });

    assert_eq!(small, large);
    assert_eq!(
        counters.acquisitions(),
        0,
        "a 4096× larger surface allocated more ({counters:?})"
    );
}

#[test]
fn an_empty_surface_costs_nothing() {
    let mut binner = Binner::new();
    let bins = binner.bin(&[], TileGeometry::DEFAULT, SurfaceSize::new(1920, 1080));
    assert_eq!(bins.tiles().len(), 0);
    assert_eq!(bins.indices().len(), 0);
    assert_eq!(bins.grid(), (8, 270));
}

// ------------------------------------------------------------ tightness

#[test]
fn a_diagonal_is_assigned_only_to_the_tiles_it_crosses() {
    // An 8×8 surface of 4×4 tiles is a 2×2 grid. The main diagonal passes
    // through two of the four cells; its bounding box covers all four.
    let segments = [Segment::new(0.0, 0.0, 8.0, 8.0)];
    let mut binner = Binner::new();
    let tiles = occupied(
        &mut binner,
        &segments,
        TileGeometry::new(4, 4),
        SurfaceSize::new(8, 8),
    );
    assert_eq!(tiles, vec![(0, 0), (1, 1)]);
}

#[test]
fn a_horizontal_segment_occupies_one_tile_row() {
    let segments = [Segment::new(1.0, 6.0, 30.0, 6.0)];
    let mut binner = Binner::new();
    let tiles = occupied(
        &mut binner,
        &segments,
        TileGeometry::new(16, 4),
        SurfaceSize::new(64, 64),
    );
    assert_eq!(tiles, vec![(0, 1), (1, 1)]);
}

#[test]
fn a_segment_that_ends_on_a_tile_boundary_does_not_enter_the_next_tile() {
    let segments = [Segment::new(2.0, 0.0, 2.0, 4.0)];
    let mut binner = Binner::new();
    let tiles = occupied(
        &mut binner,
        &segments,
        TileGeometry::new(4, 4),
        SurfaceSize::new(16, 16),
    );
    assert_eq!(
        tiles,
        vec![(0, 0)],
        "y = 4 opens row 1 but contributes no area to it"
    );
}

#[test]
fn off_surface_segments_are_dropped() {
    let segments = [
        Segment::new(-100.0, -100.0, -50.0, -50.0),
        Segment::new(500.0, 10.0, 600.0, 20.0),
        Segment::new(10.0, 500.0, 20.0, 600.0),
        Segment::new(f32::NAN, 0.0, 1.0, 1.0),
        Segment::new(1.0, 1.0, 2.0, 2.0),
    ];
    let mut binner = Binner::new();
    let bins = binner.bin(
        &segments,
        TileGeometry::new(16, 4),
        SurfaceSize::new(64, 64),
    );
    assert_eq!(bins.stats().segments_dropped, 4);
    assert_eq!(bins.tiles().len(), 1);
}

#[test]
fn a_segment_crossing_the_surface_edge_is_clipped_to_the_grid() {
    let segments = [Segment::new(-50.0, 2.0, 50.0, 2.0)];
    let mut binner = Binner::new();
    let bins = binner.bin(
        &segments,
        TileGeometry::new(16, 4),
        SurfaceSize::new(32, 32),
    );
    let tiles: Vec<_> = bins.tiles().iter().map(|t| (t.x, t.y)).collect();
    assert_eq!(
        tiles,
        vec![(0, 0), (1, 0)],
        "the grid is only two columns wide"
    );
}

// ------------------------------------------------------------ geometry

#[test]
fn tile_geometry_is_a_runtime_parameter() {
    let segments = rect_segments(0.0, 0.0, 32.0, 32.0);
    let surface = SurfaceSize::new(64, 64);
    let mut binner = Binner::new();

    let mut seen = Vec::new();
    for geometry in [
        TileGeometry::new(256, 4),
        TileGeometry::new(64, 8),
        TileGeometry::new(16, 16),
        TileGeometry::new(8, 2),
    ] {
        let bins = binner.bin(&segments, geometry, surface);
        assert_eq!(bins.geometry(), geometry);
        assert_eq!(bins.grid(), surface.tile_grid(geometry));
        // Every assignment is a real tile in the grid, and every segment it
        // names exists.
        for tile in bins.tiles() {
            assert!((tile.x as u32) < bins.grid().0);
            assert!((tile.y as u32) < bins.grid().1);
            assert!(tile.len > 0, "an empty tile has no business being stored");
            assert_eq!(bins.tile_segments(tile).count(), tile.len as usize);
        }
        seen.push((geometry, bins.stats().tiles));
    }

    // Smaller tiles mean more of them for the same geometry — the parameter
    // actually reaches the algorithm.
    assert!(
        seen[0].1 < seen[3].1,
        "tile counts did not change with geometry: {seen:?}"
    );
}

#[test]
fn zero_sized_tiles_are_clamped_rather_than_dividing_by_zero() {
    let geometry = TileGeometry::new(0, 0);
    assert_eq!(geometry.width, 1);
    assert_eq!(geometry.height, 1);
    let mut binner = Binner::new();
    let segments = [Segment::new(0.0, 0.0, 2.0, 2.0)];
    let bins = binner.bin(&segments, geometry, SurfaceSize::new(4, 4));
    assert!(bins.stats().tiles > 0);
}

// ------------------------------------------------------------ determinism

#[test]
fn segments_within_a_tile_are_deterministically_ordered() {
    // Many segments crossing the same tiles, deliberately not in sorted order.
    let segments: Vec<Segment> = (0..64)
        .map(|i| {
            let f = i as f32;
            Segment::new(
                (f * 7.0) % 31.0,
                (f * 13.0) % 29.0,
                (f * 3.0) % 31.0,
                (f * 11.0) % 29.0,
            )
        })
        .collect();
    let geometry = TileGeometry::new(16, 4);
    let surface = SurfaceSize::new(32, 32);

    let mut binner = Binner::new();
    let first: Vec<u32> = binner.bin(&segments, geometry, surface).indices().to_vec();
    let layout: Vec<(u16, u16, u32, u32)> = binner
        .bin(&segments, geometry, surface)
        .tiles()
        .iter()
        .map(|t| (t.x, t.y, t.offset, t.len))
        .collect();
    assert!(!first.is_empty());

    for run in 0..1000 {
        let bins = binner.bin(&segments, geometry, surface);
        assert_eq!(
            bins.indices(),
            first.as_slice(),
            "run {run} reordered segments"
        );
        let tiles: Vec<(u16, u16, u32, u32)> = bins
            .tiles()
            .iter()
            .map(|t| (t.x, t.y, t.offset, t.len))
            .collect();
        assert_eq!(tiles, layout, "run {run} reordered tiles");
    }
}

#[test]
fn tiles_come_out_in_row_major_order() {
    let segments = rect_segments(0.0, 0.0, 60.0, 60.0);
    let mut binner = Binner::new();
    let bins = binner.bin(
        &segments,
        TileGeometry::new(16, 16),
        SurfaceSize::new(64, 64),
    );
    let keys: Vec<u32> = bins
        .tiles()
        .iter()
        .map(|t| t.y as u32 * 4 + t.x as u32)
        .collect();
    assert!(keys.windows(2).all(|w| w[0] < w[1]), "{keys:?}");
}

#[test]
fn a_tiles_segments_are_the_ones_it_names() {
    let segments = rect_segments(1.0, 1.0, 30.0, 30.0);
    let mut binner = Binner::new();
    let bins = binner.bin(
        &segments,
        TileGeometry::new(16, 16),
        SurfaceSize::new(32, 32),
    );
    let mut total = 0;
    for tile in bins.tiles() {
        let named: Vec<Segment> = bins
            .tile_indices(tile)
            .iter()
            .map(|&i| segments[i as usize])
            .collect();
        let iterated: Vec<Segment> = bins.tile_segments(tile).collect();
        assert_eq!(named, iterated);
        total += tile.len as usize;
    }
    assert_eq!(total, bins.stats().assignments);
    assert_eq!(total, bins.indices().len());
}

// ------------------------------------------------------------ I-9

#[test]
fn binning_a_second_frame_allocates_nothing() {
    let segments: Vec<Segment> = (0..500)
        .map(|i| {
            let f = i as f32 * 0.7;
            Segment::new(f, f * 0.5, f + 9.0, f * 0.5 + 6.0)
        })
        .collect();
    let geometry = TileGeometry::DEFAULT;
    let surface = SurfaceSize::new(1920, 1080);

    let mut binner = Binner::new();
    let (first_count, first) = measure(|| binner.bin(&segments, geometry, surface).stats().tiles);
    assert!(
        first.acquisitions() > 0,
        "the counting allocator is not installed"
    );

    let (second_count, second) = measure(|| binner.bin(&segments, geometry, surface).stats().tiles);
    assert_eq!(first_count, second_count);
    assert_eq!(
        second.acquisitions(),
        0,
        "I-9: a steady-state bin allocated ({second:?})"
    );
}
