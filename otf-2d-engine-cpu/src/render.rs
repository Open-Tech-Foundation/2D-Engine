//! The CPU backend: stages 2 through 6, assembled (Doc 01 §6).

use alloc::vec::Vec;
use core::fmt;

use otf_2d_engine_color::{Color, ColorSpace};
use otf_2d_engine_geom::{Affine, Rect};
use otf_2d_engine_raster::{
    Binner, DEFAULT_TOLERANCE, FineTables, Flattener, Segment, Simd, SolidPaint, Striper,
    SurfaceSize, TargetMut, ThreadPool, TileGeometry, clip_segments, render_solid_paint,
};
use otf_2d_engine_scene::{
    PaintKind, ResolveParams, ResolvedKind, Resolver, Scene, color_from_record,
};

/// Which precision the fine stage works in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Pipeline {
    /// Pick per render. Today that always means [`Pipeline::U8`]; the choice
    /// becomes real when the `f32` pipeline lands in T3.8.
    #[default]
    Auto,
    /// 8-bit sRGB storage, linear-light blending.
    U8,
    /// 32-bit float, for wide gamut and heavy compositing. T3.8.
    F32,
}

/// How a scene is rendered onto a target.
#[derive(Clone, Copy)]
pub struct RenderParams<'a> {
    pub width: u32,
    pub height: u32,
    /// What the surface is cleared to before drawing.
    pub base_color: Color,
    /// `None` renders the whole surface. `Some` is an incremental redraw.
    pub damage: Option<Rect>,
    /// Caller-supplied. `None` is single-threaded — the engine spawns nothing.
    pub threads: Option<&'a dyn ThreadPool>,
    pub pipeline: Pipeline,
    /// Doc 01 P5: disables every cache, for reference output.
    pub bypass_caches: bool,
    /// Scene-to-device transform: DPI scaling, canvas placement.
    pub transform: Affine,
    /// Which stage 6 kernel to use. `Simd::detect()` by default.
    pub simd: Simd,
    /// Tile geometry (Q-01), a parameter rather than a constant.
    pub tile: TileGeometry,
    /// Flattening error in device pixels.
    pub tolerance: f64,
}

impl fmt::Debug for RenderParams<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RenderParams")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pipeline", &self.pipeline)
            .field("threaded", &self.threads.is_some())
            .field("simd", &self.simd)
            .field("tile", &self.tile)
            .finish_non_exhaustive()
    }
}

impl RenderParams<'_> {
    /// Whole-surface defaults: transparent base, no damage, no threads.
    pub fn new(width: u32, height: u32) -> RenderParams<'static> {
        RenderParams {
            width,
            height,
            base_color: Color::TRANSPARENT,
            damage: None,
            threads: None,
            pipeline: Pipeline::Auto,
            bypass_caches: false,
            transform: Affine::IDENTITY,
            simd: Simd::detect(),
            tile: TileGeometry::DEFAULT,
            tolerance: DEFAULT_TOLERANCE,
        }
    }
}

/// Why a render could not be produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RenderError {
    /// The target is not the size the parameters describe.
    TargetSize {
        expected: (u32, u32),
        found: (u32, u32),
    },
    /// A pipeline this build does not implement yet.
    UnsupportedPipeline(Pipeline),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TargetSize { expected, found } => write!(
                f,
                "target is {}×{}, parameters say {}×{}",
                found.0, found.1, expected.0, expected.1
            ),
            Self::UnsupportedPipeline(pipeline) => {
                write!(f, "{pipeline:?} is not implemented in this build")
            }
        }
    }
}

impl core::error::Error for RenderError {}

/// What one render did.
///
/// Not optional telemetry: this is how the caching work in Doc 03 is validated
/// and how performance regressions are caught (Doc 02 §7). It ships in release
/// builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RenderStats {
    pub draws_resolved: usize,
    pub draws_culled: usize,
    pub draws_skipped: usize,
    pub segments_flattened: usize,
    pub tiles_rasterized: usize,
    pub tiles_from_cache: usize,
    pub alpha_pixels: usize,
    pub solid_pixels: usize,
    pub peak_memory: usize,
    pub simd: Simd,
}

/// The CPU backend, with every stage's workspace held for reuse.
///
/// One per rendering thread, kept alive across frames: that is what makes a
/// steady-state frame allocation-free (I-9).
#[derive(Debug, Default)]
pub struct CpuRenderer {
    resolver: Resolver,
    flattener: Flattener,
    clipped: Vec<Segment>,
    binner: Binner,
    striper: Striper,
    tables: FineTables,
}

impl CpuRenderer {
    pub fn new() -> CpuRenderer {
        CpuRenderer {
            tables: FineTables::new(),
            ..CpuRenderer::default()
        }
    }

    /// The transfer-function tables, for encoding colours the same way the
    /// renderer does.
    pub fn tables(&self) -> &FineTables {
        &self.tables
    }

    /// Renders `scene` onto `target`.
    pub fn render(
        &mut self,
        scene: &Scene,
        target: &mut TargetMut<'_>,
        params: &RenderParams<'_>,
    ) -> Result<RenderStats, RenderError> {
        if target.width() != params.width || target.height() != params.height {
            return Err(RenderError::TargetSize {
                expected: (params.width, params.height),
                found: (target.width(), target.height()),
            });
        }
        match params.pipeline {
            Pipeline::Auto | Pipeline::U8 => {}
            other => return Err(RenderError::UnsupportedPipeline(other)),
        }

        let simd = params.simd.resolve();
        let mut stats = RenderStats {
            simd,
            ..RenderStats::default()
        };

        let surface = SurfaceSize::new(params.width, params.height);
        let bounds = Rect::new(0.0, 0.0, params.width as f64, params.height as f64);
        target.clear(params.base_color, &self.tables);

        let mut resolve = ResolveParams::new(bounds).with_transform(params.transform);
        if let Some(damage) = params.damage {
            resolve = resolve.with_damage(damage);
        }
        let resolved = self.resolver.resolve(scene, &resolve);
        stats.draws_culled = resolved.stats().culled;

        for draw in resolved.draws() {
            let ResolvedKind::Fill { rule, path } = draw.kind else {
                // Strokes, glyphs, images and layers arrive in M3 and M4. A
                // draw this build cannot honour is counted, never guessed at.
                if !matches!(
                    draw.kind,
                    ResolvedKind::BeginLayer { .. } | ResolvedKind::EndLayer { .. }
                ) {
                    stats.draws_skipped += 1;
                }
                continue;
            };
            let Some(view) = resolved.scene().path(path) else {
                stats.draws_skipped += 1;
                continue;
            };
            let Some(paint) = solid_paint(&resolved, draw, target, &self.tables) else {
                stats.draws_skipped += 1;
                continue;
            };

            // Stage 3.
            self.flattener.reset();
            self.flattener.add_path(
                view.raw_verbs(),
                view.raw_points(),
                draw.transform,
                params.tolerance,
            );

            // The clip is a rectangle, so clipping is exact: fold the geometry
            // into the rect and the antialiasing of its edges falls out of the
            // same coverage the fill already computes. Stage 6 needs no notion
            // of clipping at all.
            let clip = resolved.clip(draw).rect.intersect(bounds);
            if clip.is_empty() {
                continue;
            }
            clip_segments(
                self.flattener.segments(),
                clip.x0 as f32,
                clip.y0 as f32,
                clip.x1 as f32,
                clip.y1 as f32,
                &mut self.clipped,
            );
            stats.segments_flattened += self.clipped.len();
            if self.clipped.is_empty() {
                continue;
            }

            // Stages 4 and 5.
            let bins = self.binner.bin(&self.clipped, params.tile, surface);
            stats.tiles_rasterized += bins.stats().tiles;
            let strips = self.striper.generate(&bins, rule);

            // Stage 6.
            let fine =
                render_solid_paint(target, &strips, &paint, &self.tables, simd, params.threads);
            stats.alpha_pixels += fine.pixels_blended;
            stats.solid_pixels += fine.pixels_stored;
            stats.draws_resolved += 1;
        }

        stats.peak_memory = self.memory_usage();
        Ok(stats)
    }

    /// Bytes held across frames by the stage workspaces.
    pub fn memory_usage(&self) -> usize {
        self.resolver.memory_usage()
            + self.flattener.memory_usage()
            + core::mem::size_of_val(&self.clipped[..])
            + self.binner.memory_usage()
            + self.striper.memory_usage()
    }
}

/// The solid paint a fill uses, or `None` when the paint is one this build
/// does not implement yet.
fn solid_paint(
    resolved: &otf_2d_engine_scene::ResolvedScene<'_>,
    draw: &otf_2d_engine_scene::ResolvedDraw,
    target: &TargetMut<'_>,
    tables: &FineTables,
) -> Option<SolidPaint> {
    let desc = draw
        .paint
        .get()
        .and_then(|index| resolved.scene().paints().get(index))?;
    if PaintKind::from_u32(desc.kind)? != PaintKind::Solid {
        // Gradients are T3.4, images T3.6.
        return None;
    }
    let color = color_from_record(desc.color, desc.color_space);
    Some(SolidPaint::new(
        color.convert_to(ColorSpace::Srgb),
        target.format(),
        tables,
    ))
}
