# 2D-Engine — Scene IR & Public API

**Status:** Draft for review
**Depends on:** Doc 01 (Architecture)

---

## 1. Crate layout

```
otf-2d-engine/
├── otf-2d-engine-geom/        Points, affines, rects, Béziers, path data
├── otf-2d-engine-color/       Colour spaces, conversion, blend math
├── otf-2d-engine-scene/       Scene IR, SceneBuilder, encoding, hashing
├── otf-2d-engine-raster/      Stages 3–6: flatten, bin, strips, fine raster
├── otf-2d-engine-cpu/         CPU backend (assembles the above)
├── otf-2d-engine-cache/       Damage tracking, tile cache, glyph cache (Doc 03)
├── otf-2d-engine-text/        OPTIONAL: shaping, fallback, bidi
├── otf-2d-engine-gpu/         POST-V1: wgpu backend
└── otf-2d-engine/      Facade: re-exports the common path
```

### Dependency rules

- `otf-2d-engine-scene` depends on `otf-2d-engine-geom` + `otf-2d-engine-color`. Nothing else.
- `otf-2d-engine-raster` never depends on `otf-2d-engine-cache`. Caching wraps rasterization; it is not woven through it. This enforces Doc 01 P5.
- `otf-2d-engine-text` depends on `otf-2d-engine-scene` but **nothing depends on `otf-2d-engine-text`**. It is a leaf.
- No crate in the v1 set depends on `wgpu`, `winit`, or any windowing crate.

### External dependencies (v1)

Keep this list short and defend additions.

| Crate | Purpose | Justification |
|---|---|---|
| `bytemuck` | POD casts for arena buffers | Avoids unsafe transmutes |
| `smallvec` | Small stack-allocated vecs | Hot-path allocation avoidance |
| `rustc-hash` | Fast non-cryptographic hashing | Cache keys |
| `skrifa` *(or equivalent)* | Font outline extraction | Do not write a font parser |
| `libm` *(optional)* | `no_std` float math | Embedded/wasm targets |

Explicitly **not** taken as dependencies in v1: `kurbo`, `peniko`, `lyon`, `tiny-skia`. Geometry types are ours, because they are the public API surface and must be stable independent of upstream churn.

`no_std` + `alloc` support is a target for `otf-2d-engine-geom`, `otf-2d-engine-color`, and `otf-2d-engine-scene`. `otf-2d-engine-raster` may require `std` when threading is enabled.

---

## 2. Memory layout

### Arena encoding

The `Scene` is not a tree of objects. It is a set of parallel buffers.

```rust
pub struct Scene {
    /// One tag per draw command, in submission order.
    tags:       Vec<DrawTag>,
    /// Path segment data, densely packed. Indexed by PathRef.
    path_data:  Vec<f32>,
    /// Path descriptors: offset + length into path_data.
    paths:      Vec<PathDesc>,
    /// Affine transforms, deduplicated.
    transforms: Vec<Affine>,
    /// Paints: solid, gradient, image.
    paints:     Vec<Paint>,
    /// Gradient stop runs, densely packed.
    stops:      Vec<ColorStop>,
    /// Glyph runs.
    glyph_runs: Vec<GlyphRunDesc>,
    /// Glyphs, densely packed. Indexed by GlyphRunDesc.
    glyphs:     Vec<Glyph>,
    /// Layer push/pop records.
    layers:     Vec<LayerDesc>,
    /// Content hashes for structural sharing (Doc 03).
    node_hashes: Vec<NodeHash>,
    /// Physical meaning of a coordinate value of 1.0.
    unit: SceneUnit,
}

/// What one coordinate unit means. Set once per scene, never per draw.
pub enum SceneUnit {
    /// Unitless. Consumer owns all scaling. Raster targets only.
    Logical,
    /// 1.0 = one device pixel.
    Pixel,
    /// 1.0 = one point (1/72 inch).
    Point,
    /// 1.0 = one millimetre.
    Millimeter,
}
```

### Why the unit lives on the scene

Raster backends ignore it entirely — the consumer has already baked scale into the transform, and output is pixels regardless.

Vector backends **require** it. PDF's coordinate system is physical: 1pt = 1/72 inch, an A4 page is 595 × 842 points. A vector backend given a `Logical` scene cannot know whether `1.0` means a pixel, a point, or a millimetre, and guessing produces silently wrong output on paper.

```rust
impl VectorBackend {
    /// Errors with `UnitRequired` when scene.unit == SceneUnit::Logical.
}
```

Erroring is deliberate. A vector backend that assumes a unit will produce a plausible-looking PDF at the wrong physical size, which is worse than a failure.

Inches and metres are conversions, not variants:

```rust
impl SceneUnit {
    pub fn inches(v: f64) -> f64 { v * 72.0 }      // → points
    pub fn meters(v: f64) -> f64 { v * 1000.0 }    // → millimetres
}
```

**Why structure-of-arrays:**

1. SIMD requires contiguous homogeneous data. AoS makes vectorisation impossible in stages 3–5.
2. Cache locality — stage 3 touches only `path_data`, never paint or glyph bytes.
3. Cheap hashing — hashing a contiguous byte range is a single pass.
4. Trivially serialisable — the whole scene is POD.

**Handles, not pointers:**

```rust
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct PathRef(u32);
pub struct PaintRef(u32);
pub struct TransformRef(u32);
pub struct GlyphRunRef(u32);
```

Consequence: `Scene: Send + Sync`, no lifetimes in the public API, no borrow-checker friction for consumers, and the scene can cross a thread boundary or be written to disk unchanged.

### Allocation policy

```rust
impl Scene {
    /// Clears contents, retains capacity. Zero allocations in steady state.
    pub fn reset(&mut self);
    /// Reports current arena usage, for consumer budgeting.
    pub fn memory_usage(&self) -> SceneMemory;
}
```

The intended consumer pattern is one long-lived `Scene` per surface, `reset()` per frame. Frame N+1 reuses frame N's allocations.

---

## 3. Geometry types

`otf-2d-engine-geom` is deliberately small. It is public API, so every type here is a stability commitment.

```rust
pub struct Point { pub x: f64, pub y: f64 }
pub struct Vec2  { pub x: f64, pub y: f64 }
pub struct Size  { pub width: f64, pub height: f64 }
pub struct Rect  { pub x0: f64, pub y0: f64, pub x1: f64, pub y1: f64 }

/// Row-major 2x3 affine: [a b c d e f]
pub struct Affine([f64; 6]);

impl Affine {
    pub const IDENTITY: Affine;
    pub fn translate(v: Vec2) -> Affine;
    pub fn scale(s: f64) -> Affine;
    pub fn scale_non_uniform(sx: f64, sy: f64) -> Affine;
    pub fn rotate(radians: f64) -> Affine;
    pub fn then(self, other: Affine) -> Affine;
    pub fn inverse(self) -> Option<Affine>;
    /// Largest singular value — drives flattening tolerance.
    pub fn max_scale(self) -> f64;
}
```

**`f64` in the public API, `f32` internally.** Consumers work in world coordinates where `f32` precision fails at scale (a browser page can be 100k logical pixels tall). Conversion to `f32` happens after transform resolution in stage 2, when coordinates are device-local and bounded.

### Path representation

```rust
pub enum PathEl {
    MoveTo(Point),
    LineTo(Point),
    QuadTo(Point, Point),
    CurveTo(Point, Point, Point),
    ClosePath,
}

pub struct PathBuilder { /* ... */ }

impl PathBuilder {
    pub fn move_to(&mut self, p: impl Into<Point>) -> &mut Self;
    pub fn line_to(&mut self, p: impl Into<Point>) -> &mut Self;
    pub fn quad_to(&mut self, c: impl Into<Point>, p: impl Into<Point>) -> &mut Self;
    pub fn curve_to(&mut self, c1: impl Into<Point>, c2: impl Into<Point>,
                    p: impl Into<Point>) -> &mut Self;
    pub fn close(&mut self) -> &mut Self;

    /// Convenience constructors — these are the UI hot path.
    pub fn rect(&mut self, r: Rect) -> &mut Self;
    pub fn rounded_rect(&mut self, r: Rect, radii: RectRadii) -> &mut Self;
    pub fn ellipse(&mut self, center: Point, radii: Vec2) -> &mut Self;
}
```

**No arcs, no conics in the IR.** Arcs are converted to cubics at build time. This keeps stages 3–5 handling exactly three primitive types. Conic sections are added later only if a consumer demonstrates need.

**`rounded_rect` is a first-class primitive**, not sugar. It is the single most common shape in application UI and gets a dedicated fast path in binning that skips general curve handling. Same for axis-aligned `rect`.

---

## 4. Paint

```rust
pub enum Paint {
    Solid(Color),
    LinearGradient { start: Point, end: Point, stops: StopsRef, extend: Extend },
    RadialGradient { center: Point, radius: f32, focal: Option<Point>,
                     stops: StopsRef, extend: Extend },
    Image { image: ImageRef, sampling: Sampling, transform: TransformRef },
}

pub enum Extend { Pad, Repeat, Reflect }

pub enum Sampling { Nearest, Bilinear }
```

Deferred to P5: conic/sweep gradients, mesh gradients, pattern paints, `Sampling::Bicubic`, mipmapped image sampling.

### Colour

```rust
pub struct Color {
    /// Linear-light, premultiplied.
    pub r: f32, pub g: f32, pub b: f32, pub a: f32,
    pub space: ColorSpace,
}

pub enum ColorSpace { Srgb, DisplayP3, Rec2020, /* extensible */ }

impl Color {
    /// Convenience for the common case. Converts to linear internally.
    pub fn from_srgb8(r: u8, g: u8, b: u8, a: u8) -> Color;
    pub fn from_rgba_f32(r: f32, g: f32, b: f32, a: f32) -> Color;
}
```

Colour space travels with the colour, per Doc 01 §7. There is no global colour context.

---

## 5. SceneBuilder

The encoding API. Note what it is *not*: there is no `save()`, no `restore()`, no current point, no current paint. Every call is self-contained.

```rust
pub struct SceneBuilder<'a> { scene: &'a mut Scene, /* ... */ }

impl<'a> SceneBuilder<'a> {
    pub fn new(scene: &'a mut Scene) -> Self;

    // ---- Shape drawing ----

    pub fn fill(
        &mut self,
        rule:      FillRule,
        transform: Affine,
        paint:     &Paint,
        path:      &Path,
    ) -> Result<(), EncodeError>;

    pub fn stroke(
        &mut self,
        style:     &StrokeStyle,
        transform: Affine,
        paint:     &Paint,
        path:      &Path,
    ) -> Result<(), EncodeError>;

    // ---- Text ----

    /// Glyph runs only. 2D-Engine does not accept strings — see §6.
    pub fn draw_glyphs(
        &mut self,
        font:      FontRef,
        size:      f32,
        transform: Affine,
        paint:     &Paint,
        glyphs:    &[Glyph],
        options:   GlyphOptions,
    ) -> Result<(), EncodeError>;

    // ---- Images ----

    pub fn draw_image(
        &mut self,
        image:     ImageRef,
        transform: Affine,
        sampling:  Sampling,
        alpha:     f32,
    ) -> Result<(), EncodeError>;

    // ---- Layers ----

    pub fn push_layer(
        &mut self,
        blend:     BlendMode,
        alpha:     f32,
        transform: Affine,
        clip:      Option<&Path>,
    ) -> Result<(), EncodeError>;

    pub fn pop_layer(&mut self) -> Result<(), EncodeError>;

    // ---- Structural sharing (Doc 03) ----

    /// Begin a reusable subtree with a caller-supplied stable identity.
    pub fn push_node(&mut self, id: NodeId) -> NodeScope;

    /// Reuse a previously encoded subtree without re-encoding it.
    pub fn reuse_node(&mut self, id: NodeId, transform: Affine) -> bool;
}
```

### Errors

```rust
pub enum EncodeError {
    NonFiniteCoordinate,
    UnbalancedLayer,
    LayerDepthExceeded { max: u32 },
    PathTooLarge { limit: usize },
    InvalidGlyphRun,
}
```

Validation happens at encode time. A `Scene` that encoded without error is guaranteed rasterizable — no mid-render failure, no partial output, no panic. This matters because consumers encode on a worker thread where a panic is expensive.

### Stroke style

```rust
pub struct StrokeStyle {
    pub width:       f32,
    pub join:        Join,      // Miter { limit } | Round | Bevel
    pub start_cap:   Cap,       // Butt | Round | Square
    pub end_cap:     Cap,
    pub dash:        Option<Dash>,
}

pub struct Dash { pub pattern: SmallVec<[f32; 4]>, pub offset: f32 }
```

Dashing is applied during stage 3 as a path transform before offset expansion, per Doc 01 §4.

---

## 6. Text interface

**2D-Engine accepts positioned glyphs. It does not accept strings.**

```rust
### Type ownership

| Type | Crate | Why |
|---|---|---|
| `Font` / `FontRef` | **core** | Outlines are needed to rasterize |
| `Glyph` | **core** | The draw primitive |
| `GlyphRun` | **core** | Batch sharing font, size, transform, paint |
| `GlyphOptions` | **core** | Affects outline extraction, not shaping |
| `TextStyle` | `-text` | Shaping policy: family, features, language |
| `TextLayout` | **consumer** | Line breaking interleaves with box layout |

`Font` is the arguable one, since both core and `-text` need it. The split: core owns a minimal handle plus outline access; `-text` extends it with shaping tables and metrics. Both reference the same underlying font data — core does not duplicate parsing.

```rust
// core
pub struct FontRef { /* handle into a caller-registered font */ }

impl FontRef {
    pub fn outline(&self, glyph: u32, coords: &[f32]) -> Option<Path>;
    pub fn units_per_em(&self) -> u16;
}
```

`TextLayout` is not ours because line breaking needs box constraints — available width, floats, exclusion zones, hyphenation policy. Those belong to a layout engine. Shipping `TextLayout` would require knowing the consumer's box model.

```rust
pub struct Glyph {
    pub id: u32,     // Glyph index within the font — NOT a codepoint
    pub x:  f32,     // Position in run-local space
    pub y:  f32,
}

pub struct GlyphOptions {
    pub hinting:        Hinting,        // v1: Hinting::None only
    pub synthetic_bold: f32,
    pub synthetic_skew: f32,            // Fake italic
    pub variations:     VariationsRef,  // Variable font axes
}
```

### Why this boundary

Three reasons, in order of importance:

1. **A browser-like consumer must shape before it can lay out.** Line breaking requires measuring runs. If shaping lives inside the renderer, the consumer either shapes twice or the renderer must expose measurement APIs that leak shaping internals anyway — at which point the boundary was fictional.

2. **Shaping is policy, not mechanism.** Font matching rules, `font-feature-settings`, `letter-spacing`, language-specific behaviour — these are consumer decisions. Baking them into a renderer means every consumer inherits ours.

3. **Dependency weight.** A consumer drawing charts should not compile a Unicode shaping stack, a bidi implementation, and system font enumeration.

### `otf-2d-engine-text`

The optional crate, for consumers that don't want to build a text stack:

```rust
// otf-2d-engine-text
pub struct Shaper { /* ... */ }

impl Shaper {
    pub fn shape(&mut self, text: &str, style: &TextStyle) -> ShapedRuns;
}

pub struct ShapedRuns { /* ... */ }

impl ShapedRuns {
    /// Measure before committing to line boxes.
    pub fn advance(&self) -> f32;
    /// Produce runs ready for SceneBuilder::draw_glyphs.
    pub fn runs(&self) -> impl Iterator<Item = ShapedRun<'_>>;
}
```

Responsibilities: script/bidi itemization, font fallback chains, shaping, cluster mapping for cursor positioning. **Not** line breaking or paragraph layout — that stays with the consumer, because it interleaves with box layout.

---

## 7. Rendering

```rust
pub struct Renderer<B: RasterBackend> { /* ... */ }

impl<B: RasterBackend> Renderer<B> {
    pub fn render(
        &mut self,
        scene:  &Scene,
        target: &mut B::Target,
        params: &RenderParams,
    ) -> Result<RenderStats, RenderError>;
}

pub struct RenderParams {
    pub width:      u32,
    pub height:     u32,
    pub base_color: Color,
    /// None = whole surface. Some = incremental redraw (Doc 03).
    pub damage:     Option<&[Rect]>,
    /// Caller-supplied. None = single-threaded.
    pub threads:    Option<&ThreadPool>,
    pub pipeline:   Pipeline,   // Auto | U8 | F32
    /// Doc 01 P5: disables all caching for reference output.
    pub bypass_caches: bool,
}

pub struct RenderStats {
    pub tiles_rasterized:  u32,
    pub tiles_from_cache:  u32,
    pub segments_flattened: u32,
    pub peak_memory:       usize,
    pub stage_timings:     [Duration; 7],
}
```

`RenderStats` is not optional telemetry — it is how the caching work in Doc 03 is validated and how performance regressions are caught. It ships in release builds.

### Backend traits

Two tiers, attaching at the two seams in Doc 01 §6.

```rust
/// Attaches after stage 4. Consumes binned geometry, emits pixels.
pub trait RasterBackend {
    type Target;
    type Error;

    fn rasterize(
        &mut self,
        resolved: &ResolvedScene,
        bins:     &TileBins,
        target:   &mut Self::Target,
        params:   &RenderParams,
    ) -> Result<RenderStats, Self::Error>;

    fn capabilities(&self) -> Capabilities;
}

/// Attaches after stage 2. Consumes unflattened geometry, emits
/// drawing operators. Curves stay curves.
pub trait VectorBackend {
    type Sink;
    type Error;

    fn emit(
        &mut self,
        resolved: &ResolvedScene,
        sink:     &mut Self::Sink,
        params:   &VectorParams,
    ) -> Result<VectorStats, Self::Error>;

    fn capabilities(&self) -> Capabilities;
}

pub struct VectorParams {
    /// Page or canvas extent, in the scene's unit.
    pub extent: Size,
    /// How to handle constructs the target cannot express.
    pub fallback: VectorFallback,
}

pub enum VectorFallback {
    /// Rasterize the offending subtree and embed it as an image.
    Rasterize { dpi: f32 },
    /// Fail instead, listing what could not be represented.
    Error,
}

pub enum VectorError {
    /// scene.unit == SceneUnit::Logical
    UnitRequired,
    /// Fallback was Error and something was unrepresentable.
    Unrepresentable(Vec<UnrepresentableFeature>),
    Sink(std::io::Error),
}
```

Neither trait abstracts over construction. A GPU backend needs a `wgpu::Device`; a PDF backend needs a sink and document metadata. Forcing a common constructor produces an abstraction that leaks immediately.

### Vector fidelity

These have no PDF/SVG equivalent and take the `VectorFallback` path:

| Construct | Reason |
|---|---|
| Blur, drop shadow | No target operator |
| Image filters | No target operator |
| Non-representable blend modes | Target supports a subset |
| Some layer effects | Depends on target's transparency-group support |

Preserved as real geometry: paths, strokes, transforms, clips, gradients, images, and glyph runs.

`VectorStats` reports what fell back, so a consumer can detect silent quality loss:

```rust
pub struct VectorStats {
    pub operators_emitted:   u32,
    pub subtrees_rasterized: u32,
    pub fallbacks:           Vec<UnrepresentableFeature>,
}
```

### CPU target

```rust
pub struct Pixmap {
    data:   Vec<u8>,        // or Vec<f32> for the f32 pipeline
    width:  u32,
    height: u32,
    format: PixelFormat,    // Rgba8Premul | Bgra8Premul | Rgba32FPremul
}

impl Pixmap {
    pub fn new(width: u32, height: u32, format: PixelFormat) -> Self;
    /// Borrow caller memory — no copy. Required for surface integration.
    pub fn from_borrowed(data: &mut [u8], width: u32, height: u32,
                         format: PixelFormat) -> Result<PixmapMut<'_>, Error>;
}
```

`from_borrowed` is how consumers render directly into a window surface, a shared memory buffer, or a texture staging area. 2D-Engine never allocates the final target unless asked.

---

## 8. Stability policy

| Crate | v1 commitment |
|---|---|
| `otf-2d-engine-geom`, `otf-2d-engine-color` | Stable at 1.0. Breaking changes are major-version events |
| `otf-2d-engine-scene` | Stable at 1.0 for the builder API; the encoding format is internal and may change freely |
| `otf-2d-engine-raster`, `otf-2d-engine-cpu` | Semver, but internals explicitly unstable |
| `otf-2d-engine-cache` | Unstable through v1 |
| `otf-2d-engine-text` | Independent versioning |

**The encoding format is not public API.** Consumers build scenes through `SceneBuilder` and never touch buffers directly. This preserves freedom to change layout for SIMD or caching without a major version.

**MSRV policy:** track stable minus two releases. MSRV bumps are minor, not major.

---

## 9. Worked example

```rust
use otf_2d_engine::*;

// Long-lived, reused across frames.
let mut scene = Scene::new();
let mut renderer = Renderer::new(CpuBackend::new());
let mut pixmap = Pixmap::new(1920, 1080, PixelFormat::Rgba8Premul);

// ---- Per frame ----
scene.reset();
let mut sb = SceneBuilder::new(&mut scene);

// A card with a rounded rect and a label.
let card = PathBuilder::new()
    .rounded_rect(Rect::new(40.0, 40.0, 360.0, 200.0), RectRadii::uniform(12.0))
    .build();

sb.fill(
    FillRule::NonZero,
    Affine::IDENTITY,
    &Paint::Solid(Color::from_srgb8(0x1e, 0x1e, 0x24, 0xff)),
    &card,
)?;

// Glyphs come pre-shaped from the consumer's text stack.
sb.draw_glyphs(
    font_ref,
    16.0,
    Affine::translate(Vec2 { x: 64.0, y: 96.0 }),
    &Paint::Solid(Color::from_srgb8(0xf0, 0xf0, 0xf4, 0xff)),
    &shaped_glyphs,
    GlyphOptions::default(),
)?;

let stats = renderer.render(&scene, &mut pixmap, &RenderParams {
    width: 1920,
    height: 1080,
    base_color: Color::from_srgb8(0x10, 0x10, 0x14, 0xff),
    damage: Some(&dirty_rects),
    threads: Some(&pool),
    pipeline: Pipeline::Auto,
    bypass_caches: false,
})?;
```

Note the properties this example demonstrates: no mutable graphics state, every call fully specified, scene reusable and `Send`, target memory caller-owned, damage explicit, threading caller-controlled.
