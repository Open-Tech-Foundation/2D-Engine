//! T2.3 + T2.4 acceptance tests for stage 6.
//!
//! The scalar reference and the AVX2 kernel must agree bit for bit (I-5), the
//! dispatch must never select a path the CPU cannot run, and the u8 pipeline
//! must stay within tolerance of the linear `f32` model it is a fast path for
//! (Doc 01 §7).

use otf_2d_engine_color::{Color, ColorSpace, linear_to_srgb8, srgb8_to_linear};
use otf_2d_engine_raster::{
    Binner, FineTables, PixelFormat, Segment, Simd, SolidPaint, Striper, SurfaceSize, TargetMut,
    TileGeometry, render_solid, render_solid_paint,
};
use otf_2d_engine_scene::FillRule;

fn polygon(points: &[(f32, f32)]) -> Vec<Segment> {
    (0..points.len())
        .map(|i| {
            let (x0, y0) = points[i];
            let (x1, y1) = points[(i + 1) % points.len()];
            Segment::new(x0, y0, x1, y1)
        })
        .collect()
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<Segment> {
    polygon(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
}

/// Renders one solid fill onto a background, returning the pixel bytes.
fn render(
    segments: &[Segment],
    color: Color,
    background: Color,
    surface: SurfaceSize,
    format: PixelFormat,
    simd: Simd,
    tables: &FineTables,
) -> Vec<u8> {
    let mut binner = Binner::new();
    let bins = binner.bin(segments, TileGeometry::new(16, 4), surface);
    let mut striper = Striper::new();
    let strips = striper.generate(&bins, FillRule::NonZero);

    let mut data = vec![0u8; (surface.width * surface.height) as usize * 4];
    let mut target =
        TargetMut::new(&mut data, surface.width, surface.height, format).expect("target");
    target.clear(background, tables);
    render_solid(&mut target, &strips, color, tables, simd, None);
    data
}

fn white() -> Color {
    Color::from_srgb8(255, 255, 255, 255)
}

fn black() -> Color {
    Color::from_srgb8(0, 0, 0, 255)
}

fn pixel(data: &[u8], surface: SurfaceSize, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * surface.width + x) * 4) as usize;
    [data[at], data[at + 1], data[at + 2], data[at + 3]]
}

/// True when this machine can run the AVX2 kernel. Tests that need it say so
/// rather than passing vacuously.
fn avx2() -> bool {
    Simd::Avx2.is_available()
}

// ------------------------------------------------------------ dispatch

#[test]
fn dispatch_never_selects_a_path_the_cpu_cannot_run() {
    let chosen = Simd::detect();
    assert!(
        chosen.is_available(),
        "detect() chose {chosen:?}, which this CPU cannot run"
    );
    assert!(
        Simd::Scalar.is_available(),
        "the reference path is always available"
    );
    assert_eq!(Simd::Scalar.resolve(), Simd::Scalar);
    assert_eq!(
        Simd::Avx2.resolve(),
        if avx2() { Simd::Avx2 } else { Simd::Scalar },
        "asking for an unsupported path must fall back, not fault"
    );
}

#[test]
fn the_reported_path_is_the_path_that_ran() {
    let tables = FineTables::new();
    let surface = SurfaceSize::new(16, 16);
    let segments = rect(2.0, 2.0, 14.0, 14.0);
    let mut binner = Binner::new();
    let bins = binner.bin(&segments, TileGeometry::new(16, 4), surface);
    let mut striper = Striper::new();
    let strips = striper.generate(&bins, FillRule::NonZero);

    let mut data = vec![0u8; 16 * 16 * 4];
    let mut target = TargetMut::new(&mut data, 16, 16, PixelFormat::Rgba8Premul).expect("target");
    let stats = render_solid(&mut target, &strips, black(), &tables, Simd::Avx2, None);
    assert_eq!(stats.simd, Simd::Avx2.resolve());
    assert!(stats.pixels_stored + stats.pixels_blended > 0);
}

// ------------------------------------------------------------ bit-identity

/// The core of I-5: the two kernels must agree on every combination of
/// destination byte and coverage, not merely on the pictures a corpus happens
/// to contain.
///
/// The sweep is exhaustive in both — 256 destinations × 256 coverages — because
/// the interesting divergences are exact ties in the fixed-point rounding, and
/// a sampled sweep walks straight past them.
#[test]
fn every_destination_and_coverage_blends_identically_on_both_paths() {
    if !avx2() {
        eprintln!("skipping: this CPU has no AVX2");
        return;
    }
    let tables = FineTables::new();
    let coverage: Vec<u8> = (0..=255u8).collect();

    for format in [PixelFormat::Rgba8Premul, PixelFormat::Bgra8Premul] {
        for paint_color in [
            Color::from_srgb8(0, 0, 0, 255),
            Color::from_srgb8(255, 255, 255, 255),
            Color::from_srgb8(255, 0, 0, 255),
            Color::from_srgb8(31, 97, 200, 128),
            Color::from_srgb8(200, 10, 90, 1),
            Color::from_srgb8(0, 0, 0, 0),
            Color::from_srgb8(188, 188, 188, 128),
        ] {
            let paint = SolidPaint::new(paint_color, format, &tables);
            for destination in 0..=255u8 {
                let mut scalar = vec![destination; coverage.len() * 4];
                let mut vector = scalar.clone();
                blend_span(
                    &mut scalar,
                    &paint,
                    &tables,
                    &coverage,
                    Simd::Scalar,
                    format,
                );
                blend_span(&mut vector, &paint, &tables, &coverage, Simd::Avx2, format);
                assert_eq!(
                    scalar, vector,
                    "dst {destination}, {format:?}, {paint_color:?}"
                );
            }
        }
    }
}

/// The vector kernel handles eight pixels at a time and hands the remainder to
/// the scalar one, so the seam between them needs its own sweep.
#[test]
fn the_simd_tail_agrees_with_the_scalar_path() {
    if !avx2() {
        eprintln!("skipping: this CPU has no AVX2");
        return;
    }
    let tables = FineTables::new();
    let format = PixelFormat::Rgba8Premul;
    let paint = SolidPaint::new(Color::from_srgb8(31, 97, 200, 200), format, &tables);

    for width in 1..40usize {
        let coverage: Vec<u8> = (0..width).map(|i| ((i * 37 + 11) % 256) as u8).collect();
        for destination in [0u8, 1, 17, 128, 200, 254, 255] {
            let mut scalar = vec![destination; width * 4];
            let mut vector = scalar.clone();
            blend_span(
                &mut scalar,
                &paint,
                &tables,
                &coverage,
                Simd::Scalar,
                format,
            );
            blend_span(&mut vector, &paint, &tables, &coverage, Simd::Avx2, format);
            assert_eq!(scalar, vector, "width {width}, dst {destination}");
        }
    }
}

/// Runs one coverage span through stage 6. The point is the kernel, not the
/// path that reached it.
fn blend_span(
    data: &mut [u8],
    paint: &SolidPaint,
    tables: &FineTables,
    coverage: &[u8],
    simd: Simd,
    format: PixelFormat,
) {
    let width = coverage.len() as u32;
    let mut striper = Striper::new();
    let strips = striper.from_coverage(coverage, width);
    let mut target = TargetMut::new(data, width, 1, format).expect("target");
    render_solid_paint(&mut target, &strips, paint, tables, simd, None);
}

// ------------------------------------------------------------ whole scenes

#[test]
fn whole_scenes_render_identically_on_both_paths() {
    if !avx2() {
        eprintln!("skipping: this CPU has no AVX2");
        return;
    }
    let tables = FineTables::new();
    let surface = SurfaceSize::new(97, 61);

    let cases: Vec<(&str, Vec<Segment>, Color)> = vec![
        ("aligned rect", rect(0.0, 0.0, 97.0, 61.0), black()),
        (
            "antialiased rect",
            rect(3.25, 2.5, 80.75, 50.5),
            Color::from_srgb8(20, 120, 200, 255),
        ),
        (
            "triangle",
            polygon(&[(5.0, 5.0), (90.0, 12.0), (40.0, 55.0)]),
            Color::from_srgb8(240, 30, 90, 255),
        ),
        (
            "translucent diamond",
            polygon(&[(48.5, 4.5), (92.5, 30.5), (48.5, 56.5), (4.5, 30.5)]),
            Color::from_srgb8(10, 200, 60, 96),
        ),
        (
            "hairline",
            rect(20.1, 10.0, 20.4, 50.0),
            Color::from_srgb8(0, 0, 0, 255),
        ),
        (
            "off-surface",
            rect(-30.0, -20.0, 40.5, 30.5),
            Color::from_srgb8(255, 200, 0, 200),
        ),
    ];

    for format in [PixelFormat::Rgba8Premul, PixelFormat::Bgra8Premul] {
        for (name, segments, color) in &cases {
            let scalar = render(
                segments,
                *color,
                white(),
                surface,
                format,
                Simd::Scalar,
                &tables,
            );
            let vector = render(
                segments,
                *color,
                white(),
                surface,
                format,
                Simd::Avx2,
                &tables,
            );
            assert_eq!(
                scalar, vector,
                "{name} at {format:?} differed between paths"
            );
        }
    }
}

// ------------------------------------------------------------ correctness

#[test]
fn full_coverage_by_an_opaque_paint_writes_the_paint_exactly() {
    let tables = FineTables::new();
    let surface = SurfaceSize::new(16, 16);
    let color = Color::from_srgb8(37, 200, 91, 255);
    for simd in [Simd::Scalar, Simd::Avx2] {
        let data = render(
            &rect(0.0, 0.0, 16.0, 16.0),
            color,
            white(),
            surface,
            PixelFormat::Rgba8Premul,
            simd,
            &tables,
        );
        for chunk in data.chunks_exact(4) {
            assert_eq!(chunk, [37, 200, 91, 255], "{simd:?}");
        }
    }
}

#[test]
fn zero_coverage_leaves_the_destination_bit_identical() {
    let tables = FineTables::new();
    let surface = SurfaceSize::new(32, 32);
    let background = Color::from_srgb8(17, 200, 43, 255);
    for simd in [Simd::Scalar, Simd::Avx2] {
        let data = render(
            &rect(40.0, 40.0, 50.0, 50.0),
            black(),
            background,
            surface,
            PixelFormat::Rgba8Premul,
            simd,
            &tables,
        );
        for chunk in data.chunks_exact(4) {
            assert_eq!(
                chunk,
                [17, 200, 43, 255],
                "{simd:?}: an untouched pixel changed"
            );
        }
    }
}

#[test]
fn the_channel_order_follows_the_format() {
    let tables = FineTables::new();
    let surface = SurfaceSize::new(8, 8);
    let color = Color::from_srgb8(10, 20, 30, 255);
    let rgba = render(
        &rect(0.0, 0.0, 8.0, 8.0),
        color,
        white(),
        surface,
        PixelFormat::Rgba8Premul,
        Simd::Scalar,
        &tables,
    );
    let bgra = render(
        &rect(0.0, 0.0, 8.0, 8.0),
        color,
        white(),
        surface,
        PixelFormat::Bgra8Premul,
        Simd::Scalar,
        &tables,
    );
    assert_eq!(pixel(&rgba, surface, 0, 0), [10, 20, 30, 255]);
    assert_eq!(pixel(&bgra, surface, 0, 0), [30, 20, 10, 255]);
}

#[test]
fn blending_happens_in_linear_light_not_in_srgb_space() {
    // Half-covered black over white. Compositing the bytes directly would give
    // 128; the linear-light answer is the sRGB encoding of 0.5, which is 188.
    // This single number is the difference D-31 exists to protect.
    let tables = FineTables::new();
    let surface = SurfaceSize::new(8, 8);
    let data = render(
        &rect(0.0, 0.0, 4.0, 8.0),
        black(),
        white(),
        surface,
        PixelFormat::Rgba8Premul,
        Simd::Scalar,
        &tables,
    );
    // Column 3 is fully covered, column 4 is not covered; use a half-covered
    // strip instead.
    let half = render(
        &rect(0.0, 0.0, 3.5, 8.0),
        black(),
        white(),
        surface,
        PixelFormat::Rgba8Premul,
        Simd::Scalar,
        &tables,
    );
    assert_eq!(pixel(&data, surface, 0, 0), [0, 0, 0, 255]);
    let edge = pixel(&half, surface, 3, 0);
    let expected = linear_to_srgb8(0.5);
    assert!(
        (edge[0] as i32 - expected as i32).abs() <= 1,
        "half coverage gave {edge:?}, linear light says {expected}"
    );
    assert!(edge[0] > 150, "this looks like an sRGB-space blend");
}

/// The u8 pipeline is a fast path for the linear `f32` model, so it has to
/// agree with it. This computes the reference in `f32` and compares.
#[test]
fn the_u8_pipeline_matches_an_f32_reference() {
    let tables = FineTables::new();
    let width = 256u32;

    for paint_color in [
        Color::from_srgb8(0, 0, 0, 255),
        Color::from_srgb8(255, 255, 255, 255),
        Color::from_srgb8(64, 160, 255, 255),
        Color::from_srgb8(200, 40, 10, 128),
        Color::from_srgb8(9, 240, 130, 40),
    ] {
        for background in [0u8, 1, 64, 128, 200, 255] {
            let paint = SolidPaint::new(paint_color, PixelFormat::Rgba8Premul, &tables);
            let coverage: Vec<u8> = (0..width).map(|i| i as u8).collect();
            let mut data = vec![background; width as usize * 4];
            let mut striper = Striper::new();
            let strips = striper.from_coverage(&coverage, width);
            let mut target =
                TargetMut::new(&mut data, width, 1, PixelFormat::Rgba8Premul).expect("target");
            render_solid_paint(&mut target, &strips, &paint, &tables, Simd::Scalar, None);

            let source = paint_color.convert_to(ColorSpace::Srgb).to_premul();
            for (index, chunk) in data.chunks_exact(4).enumerate() {
                let cov = coverage[index] as f32 / 255.0;
                let inverse = 1.0 - source[3] * cov;
                for channel in 0..3 {
                    let reference = source[channel] * cov + srgb8_to_linear(background) * inverse;
                    let expected = linear_to_srgb8(reference);
                    let error = (chunk[channel] as i32 - expected as i32).abs();
                    assert!(
                        error <= 1,
                        "{paint_color:?} over {background} at coverage {}: got {} want {expected}",
                        coverage[index],
                        chunk[channel]
                    );
                }
            }
        }
    }
}

#[test]
fn compositing_a_translucent_paint_accumulates_alpha() {
    let tables = FineTables::new();
    let surface = SurfaceSize::new(8, 8);
    let transparent = Color::from_rgba_f32(0.0, 0.0, 0.0, 0.0);
    let half = Color::from_srgb8(255, 255, 255, 128);
    for simd in [Simd::Scalar, Simd::Avx2] {
        let data = render(
            &rect(0.0, 0.0, 8.0, 8.0),
            half,
            transparent,
            surface,
            PixelFormat::Rgba8Premul,
            simd,
            &tables,
        );
        let got = pixel(&data, surface, 0, 0);
        assert_eq!(got[3], 128, "{simd:?}: alpha did not composite");
    }
}

#[test]
fn a_target_must_be_large_enough() {
    let mut data = vec![0u8; 16];
    assert!(TargetMut::new(&mut data, 8, 8, PixelFormat::Rgba8Premul).is_err());
    let mut data = vec![0u8; 8 * 8 * 4];
    assert!(TargetMut::new(&mut data, 8, 8, PixelFormat::Rgba8Premul).is_ok());
    let mut data = vec![0u8; 8 * 8 * 4];
    assert!(TargetMut::with_stride(&mut data, 8, 8, 4, PixelFormat::Rgba8Premul).is_err());
}

#[test]
fn a_strided_target_writes_only_its_own_rows() {
    let tables = FineTables::new();
    let stride = 8 * 4 + 16;
    let mut data = vec![0xccu8; stride * 4];
    {
        let mut target = TargetMut::with_stride(&mut data, 8, 4, stride, PixelFormat::Rgba8Premul)
            .expect("target");
        target.clear(black(), &tables);
    }
    for row in 0..4 {
        let start = row * stride;
        assert!(data[start..start + 32].iter().all(|&b| b != 0xcc));
        assert!(
            data[start + 32..start + stride].iter().all(|&b| b == 0xcc),
            "padding after row {row} was written"
        );
    }
}

/// Reports what the SIMD kernel actually buys. Not asserted: a throughput
/// threshold on a shared machine is a flaky test, and T2.4's criteria are
/// about correctness and dispatch, not speed. Recorded so the number is
/// visible rather than assumed.
#[test]
#[ignore = "wall-clock measurement; run with --ignored"]
fn report_kernel_throughput() {
    let tables = FineTables::new();
    let surface = SurfaceSize::new(1920, 1080);
    let segments = rect(0.5, 0.5, 1919.5, 1079.5);
    let mut binner = Binner::new();
    let bins = binner.bin(&segments, TileGeometry::DEFAULT, surface);
    let mut striper = Striper::new();
    let strips = striper.generate(&bins, FillRule::NonZero);

    for (label, color) in [
        ("translucent", Color::from_srgb8(20, 40, 200, 160)),
        ("opaque", Color::from_srgb8(20, 40, 200, 255)),
    ] {
        for simd in [Simd::Scalar, Simd::Avx2] {
            if !simd.is_available() {
                continue;
            }
            let paint = SolidPaint::new(color, PixelFormat::Rgba8Premul, &tables);
            let mut data = vec![0u8; (surface.width * surface.height) as usize * 4];
            let mut target = TargetMut::new(
                &mut data,
                surface.width,
                surface.height,
                PixelFormat::Rgba8Premul,
            )
            .expect("target");
            render_solid_paint(&mut target, &strips, &paint, &tables, simd, None);
            let best = (0..7)
                .map(|_| {
                    let start = std::time::Instant::now();
                    render_solid_paint(&mut target, &strips, &paint, &tables, simd, None);
                    start.elapsed()
                })
                .min()
                .expect("seven runs");
            let pixels = (surface.width * surface.height) as f64;
            println!(
                "{label} {simd:?}: {best:?} ({:.2} ns/pixel)",
                best.as_secs_f64() * 1e9 / pixels
            );
        }
    }

    // The per-pixel-coverage path, which no map can collapse: this is what an
    // antialiased edge costs.
    let width = 4096u32;
    let coverage: Vec<u8> = (0..width).map(|i| (i % 255) as u8 + 1).collect();
    for simd in [Simd::Scalar, Simd::Avx2] {
        if !simd.is_available() {
            continue;
        }
        let paint = SolidPaint::new(
            Color::from_srgb8(20, 40, 200, 160),
            PixelFormat::Rgba8Premul,
            &tables,
        );
        let mut striper = Striper::new();
        let strips = striper.from_coverage(&coverage, width);
        let mut data = vec![128u8; width as usize * 4];
        let mut target =
            TargetMut::new(&mut data, width, 1, PixelFormat::Rgba8Premul).expect("target");
        render_solid_paint(&mut target, &strips, &paint, &tables, simd, None);
        let best = (0..2000)
            .map(|_| {
                let start = std::time::Instant::now();
                render_solid_paint(&mut target, &strips, &paint, &tables, simd, None);
                start.elapsed()
            })
            .min()
            .expect("runs");
        println!(
            "varying coverage {simd:?}: {best:?} ({:.2} ns/pixel)",
            best.as_secs_f64() * 1e9 / width as f64
        );
    }
}
