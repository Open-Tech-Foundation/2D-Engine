//! T2.5 acceptance tests for threaded dispatch.
//!
//! The engine spawns nothing (I-4, D-16): the caller supplies the pool. What
//! must hold is that the pixels do not depend on how many workers it has, and
//! that `None` is a genuine single-threaded path rather than a pool of one.

use std::sync::atomic::{AtomicUsize, Ordering};

use otf_2d_engine_color::Color;
use otf_2d_engine_raster::{
    Binner, ChunkTask, FineTables, PixelFormat, Segment, SerialPool, Simd, SurfaceSize, TargetMut,
    ThreadPool, TileGeometry, render_solid,
};
use otf_2d_engine_scene::FillRule;
use otf_2d_engine_testing::pool::ScopedPool;

fn polygon(points: &[(f32, f32)]) -> Vec<Segment> {
    (0..points.len())
        .map(|i| {
            let (x0, y0) = points[i];
            let (x1, y1) = points[(i + 1) % points.len()];
            Segment::new(x0, y0, x1, y1)
        })
        .collect()
}

/// A scene with work spread unevenly down the surface, so a scheduler that
/// splits it badly would show up.
fn scene(width: f32, height: f32) -> Vec<Segment> {
    let mut segments = Vec::new();
    for i in 0..64 {
        let f = i as f32;
        let x = (f * 37.3) % (width - 40.0);
        let y = (f * f * 3.1) % (height - 40.0);
        let size = 6.0 + (f * 1.7) % 30.0;
        segments.extend(polygon(&[
            (x + 0.25, y + 0.5),
            (x + size, y + 0.5),
            (x + size * 0.5, y + size),
        ]));
    }
    segments
}

fn render_with(
    segments: &[Segment],
    surface: SurfaceSize,
    pool: Option<&dyn ThreadPool>,
    simd: Simd,
    tables: &FineTables,
) -> Vec<u8> {
    let mut binner = Binner::new();
    let bins = binner.bin(segments, TileGeometry::DEFAULT, surface);
    let mut striper = otf_2d_engine_raster::Striper::new();
    let strips = striper.generate(&bins, FillRule::NonZero);

    let mut data = vec![0u8; (surface.width * surface.height) as usize * 4];
    let mut target = TargetMut::new(
        &mut data,
        surface.width,
        surface.height,
        PixelFormat::Rgba8Premul,
    )
    .expect("target");
    target.clear(Color::from_srgb8(255, 255, 255, 255), tables);
    render_solid(
        &mut target,
        &strips,
        Color::from_srgb8(20, 40, 200, 200),
        tables,
        simd,
        pool,
    );
    data
}

// ------------------------------------------------------------ bit-equality

#[test]
fn output_is_identical_at_one_two_four_and_eight_threads() {
    let tables = FineTables::new();
    let surface = SurfaceSize::new(321, 197);
    let segments = scene(321.0, 197.0);

    for simd in [Simd::Scalar, Simd::Avx2] {
        let single = render_with(&segments, surface, None, simd, &tables);
        for threads in [1usize, 2, 4, 8] {
            let pool = ScopedPool::new(threads);
            let many = render_with(&segments, surface, Some(&pool), simd, &tables);
            assert_eq!(
                many, single,
                "{simd:?} at {threads} threads differed from the single-threaded render"
            );
        }
    }
}

#[test]
fn a_surface_whose_height_is_not_a_whole_number_of_bands_still_agrees() {
    // The last band is short. It is the one a chunking bug hides in.
    let tables = FineTables::new();
    let segments = scene(64.0, 64.0);
    for height in [1u32, 2, 3, 5, 7, 13, 62, 63, 64, 65] {
        let surface = SurfaceSize::new(64, height);
        let single = render_with(&segments, surface, None, Simd::Scalar, &tables);
        let pool = ScopedPool::new(4);
        let many = render_with(&segments, surface, Some(&pool), Simd::Scalar, &tables);
        assert_eq!(many, single, "height {height}");
    }
}

#[test]
fn none_means_single_threaded_and_never_consults_a_pool() {
    /// Records every dispatch, then does the work serially.
    struct Counting {
        calls: AtomicUsize,
        chunks: AtomicUsize,
    }

    impl ThreadPool for Counting {
        fn dispatch_chunks(&self, data: &mut [u8], chunk: usize, task: &ChunkTask<'_>) {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.chunks
                .fetch_add(data.len().div_ceil(chunk.max(1)), Ordering::Relaxed);
            SerialPool.dispatch_chunks(data, chunk, task);
        }
    }

    let tables = FineTables::new();
    let surface = SurfaceSize::new(128, 64);
    let segments = scene(128.0, 64.0);

    let counting = Counting {
        calls: AtomicUsize::new(0),
        chunks: AtomicUsize::new(0),
    };
    let pooled = render_with(&segments, surface, Some(&counting), Simd::Scalar, &tables);
    assert_eq!(
        counting.calls.load(Ordering::Relaxed),
        1,
        "one dispatch per render"
    );
    assert_eq!(
        counting.chunks.load(Ordering::Relaxed),
        16,
        "64 rows in bands of 4 is 16 chunks"
    );

    let counting_again = Counting {
        calls: AtomicUsize::new(0),
        chunks: AtomicUsize::new(0),
    };
    let alone = render_with(&segments, surface, None, Simd::Scalar, &tables);
    assert_eq!(
        counting_again.calls.load(Ordering::Relaxed),
        0,
        "`None` must not reach a pool at all"
    );
    assert_eq!(alone, pooled, "the two paths must agree");
}

#[test]
fn a_pool_that_reorders_chunks_produces_the_same_pixels() {
    /// Runs the chunks back to front. A correct renderer cannot tell.
    struct Reversing;

    impl ThreadPool for Reversing {
        fn dispatch_chunks(&self, data: &mut [u8], chunk: usize, task: &ChunkTask<'_>) {
            if chunk == 0 {
                return;
            }
            let count = data.len().div_ceil(chunk);
            let mut pieces: Vec<(usize, &mut [u8])> = data.chunks_mut(chunk).enumerate().collect();
            pieces.reverse();
            assert_eq!(pieces.len(), count);
            for (index, slice) in pieces {
                task(index, slice);
            }
        }
    }

    let tables = FineTables::new();
    let surface = SurfaceSize::new(200, 100);
    let segments = scene(200.0, 100.0);
    let forwards = render_with(&segments, surface, None, Simd::Scalar, &tables);
    let backwards = render_with(&segments, surface, Some(&Reversing), Simd::Scalar, &tables);
    assert_eq!(forwards, backwards, "output depended on chunk order");
}

#[test]
fn each_band_is_written_by_exactly_one_worker() {
    /// Fails loudly if two chunks ever overlap.
    struct Checking;

    impl ThreadPool for Checking {
        fn dispatch_chunks(&self, data: &mut [u8], chunk: usize, task: &ChunkTask<'_>) {
            let total = data.len();
            let mut seen = 0usize;
            for (index, slice) in data.chunks_mut(chunk).enumerate() {
                seen += slice.len();
                task(index, slice);
            }
            assert_eq!(seen, total, "the chunks do not tile the buffer");
        }
    }

    let tables = FineTables::new();
    let surface = SurfaceSize::new(133, 71);
    let segments = scene(133.0, 71.0);
    let _ = render_with(&segments, surface, Some(&Checking), Simd::Scalar, &tables);
}

// ------------------------------------------------------------ scaling

/// Measured, not asserted in the ordinary run: wall-clock scaling on a shared
/// CI machine is a flaky test, and a flaky test on a throughput target teaches
/// people to ignore it. The nightly `exhaustive` job runs `--ignored`.
///
/// Only stage 6 is timed. It is the only stage a pool touches, and timing the
/// serial stages alongside it would measure Amdahl's law rather than the
/// dispatch.
#[test]
#[ignore = "wall-clock measurement; run with --ignored"]
fn throughput_scales_from_one_to_four_threads() {
    let tables = FineTables::new();
    let surface = SurfaceSize::new(3840, 2160);
    let simd = Simd::detect();
    println!(
        "cores: {}, simd: {simd:?}",
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0)
    );

    // Two fills, because they are bound by different things. The translucent
    // one runs the blend for every pixel; the opaque one is a store, and a
    // store across a 33 MiB surface is bound by memory bandwidth, which no
    // amount of threads invents more of.
    for (label, color) in [
        ("translucent fill", Color::from_srgb8(20, 40, 200, 160)),
        ("opaque fill", Color::from_srgb8(20, 40, 200, 255)),
    ] {
        let segments = polygon(&[(0.5, 0.5), (3839.5, 0.5), (3839.5, 2159.5), (0.5, 2159.5)]);
        let mut binner = Binner::new();
        let bins = binner.bin(&segments, TileGeometry::DEFAULT, surface);
        let mut striper = otf_2d_engine_raster::Striper::new();
        let strips = striper.generate(&bins, FillRule::NonZero);

        let mut data = vec![0u8; (surface.width * surface.height) as usize * 4];
        let mut target = TargetMut::new(
            &mut data,
            surface.width,
            surface.height,
            PixelFormat::Rgba8Premul,
        )
        .expect("target");

        let mut measure = |threads: usize| -> std::time::Duration {
            let pool = ScopedPool::new(threads);
            let run = |target: &mut TargetMut<'_>| {
                render_solid(target, &strips, color, &tables, simd, Some(&pool));
            };
            run(&mut target);
            (0..7)
                .map(|_| {
                    let start = std::time::Instant::now();
                    run(&mut target);
                    start.elapsed()
                })
                .min()
                .expect("seven runs")
        };

        let one = measure(1);
        let four = measure(4);
        let speedup = one.as_secs_f64() / four.as_secs_f64();
        println!("{label}: 1 thread {one:?}, 4 threads {four:?}, speedup {speedup:.2}x");

        if label == "translucent fill" {
            assert!(
                speedup >= 3.0,
                "expected >=3x from 1 to 4 threads, got {speedup:.2}x ({one:?} -> {four:?})"
            );
        }
    }
}
