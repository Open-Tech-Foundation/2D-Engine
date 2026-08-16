# 2D-Engine — Architecture & Rendering Pipeline

**Status:** Draft for review
**Project:** 2D-Engine (Open Tech Foundation)

---

## 1. Scope

2D-Engine is a **generic, headless, 2D vector rendering engine** written in Rust.

It converts a declarative scene description into pixels. It does not decide *what* to draw. Consumers — a UI toolkit, a browser-like view layer, a charting library — build scenes and hand them to 2D-Engine.

### In scope

- Path filling and stroking with antialiasing
- Affine transforms, clipping, layer compositing
- Gradients, images, glyph run rasterization
- CPU rasterization (v1), GPU backend (post-v1)
- Rendering into a caller-provided buffer or surface
- **Vector output** (PDF, SVG) from the same scene, with partial fidelity — see §6
- **Unit-aware scenes** — logical, pixel, point, or millimetre — see Doc 02 §2

### Explicitly out of scope

| Not our problem | Belongs to |
|---|---|
| HTML/CSS parsing | Consumer |
| Layout (flexbox, grid, line breaking) | Consumer |
| Text shaping | `otf-2d-engine-text` (separate crate) |
| Widgets, event handling, hit testing | Consumer |
| Windowing, event loop, input | Consumer |
| Animation and timing | Consumer |
| Accessibility | Consumer |
| Font enumeration and matching | `otf-2d-engine-text` |

2D-Engine never owns a window, never opens a file, never blocks, and never spawns a thread the caller didn't authorise.

---

## 2. Design principles

These are load-bearing. Violating any one of them breaks something downstream.

**P1 — The scene is immutable data, not a command stream.**
Once encoded, a scene is a plain buffer with no interior mutability. This is what makes off-thread encoding, structural diffing, caching, and serialisation possible. Every stateful-context renderer (Cairo, Canvas2D, Skia's `SkCanvas`) gave this up and can never get it back.

**P2 — Free-threaded from commit one.**
All public types are `Send`. Scene encoding happens on any thread. Rasterization is parallel over tiles. Retrofitting threading into a renderer is a rewrite; every renderer that tried has learned this.

**P3 — SIMD is a design constraint, not an optimisation.**
Data layout in stages 4–6 assumes vectorised lanes. Structure-of-arrays everywhere in the hot path. A scalar rasterizer cannot be vectorised later without being rebuilt.

**P4 — Backend-agnostic front half.**
Stages 1–4 produce a representation with no CPU-specific or GPU-specific concepts. Anything that leaks device knowledge into stages 1–4 is a bug.

Stage 2's output is additionally **resolution-independent**: curves are not flattened and no device tolerance is applied. This is what lets vector backends attach after stage 2 while raster backends continue through stages 3–4. See §6.

**P5 — Correctness has a reference path.**
Every optimisation (caching, damage tracking, fast paths) must be disableable by a flag, and output with it disabled must match output with it enabled. This is enforced in CI. Without this invariant the caching work in Doc 03 is unshippable.

**P6 — Linear premultiplied f32 is the model; u8 sRGB is a fast path.**
Not the other way round. See §7.

---

## 3. Pipeline overview

```
  ┌─────────┐   ┌─────────┐   ┌─────────┐   ┌─────────┐
  │ 1 ENCODE│──▶│2 RESOLVE│──▶│3 FLATTEN│──▶│ 4 BIN   │
  └─────────┘   └─────────┘   └─────────┘   └─────────┘
   Scene IR      Transforms     Curves →      Segments →
   built by      + clips        segments      sparse tiles
   consumer      resolved
  ─────────────── backend-agnostic ────────────────────┤
                                                       │
  ┌─────────┐   ┌─────────┐   ┌───────────┐            │
  │7 COMPOSE│◀──│6 FINE   │◀──│ 5 STRIPS  │◀───────────┘
  └─────────┘   └─────────┘   └───────────┘
   Layers,       Strips →       Coverage →
   blending      pixels         sparse strips
```

Stages 1–2 are shared by **all** backends, vector and raster. Stages 3–4 are raster-only. Stages 5–6 are raster-backend-specific. Stage 7 is shared in structure, backend-specific in execution. See §6 for the two seams.

---

## 4. Stage detail

### Stage 1 — Encode

**Input:** consumer API calls.
**Output:** `Scene` — flat arena buffers.

The consumer builds a scene through `SceneBuilder`. Internally this appends to parallel arrays: a tag stream, a path-data stream, a transform stream, a paint stream, a glyph-run stream.

Key properties:

- **Index handles, never pointers.** Every reference within the scene is a `u32` index. No `Rc`, no `Box`, no lifetimes threaded through the IR. This makes `Scene: Send + Sync`, serialisable, and cheap to hash.
- **Arena reuse.** `Scene::reset()` truncates without deallocating. Steady-state frames perform zero allocations.
- **No validation deferred.** Malformed input (non-finite coordinates, unbalanced layer push/pop) is rejected at encode time with a `Result`, not discovered mid-raster.

Encoding is pure CPU work with no device dependency, so it can happen on a worker thread while the previous frame rasterizes.

### Stage 2 — Resolve

**Input:** `Scene`.
**Output:** `ResolvedScene` — flattened draw list.

- Collapse the transform stack: every draw carries its final absolute affine.
- Collapse the clip stack: each draw carries a clip reference. Rectangular clips (the overwhelming majority in UI) become a simple bounding rect. Non-rectangular clips become a mask allocation.
- Cull draws whose bounds fall outside the target or outside the current damage region.
- Resolve layer boundaries into an explicit begin/end structure with known bounds.

After this stage there is no tree. There is a flat, ordered list. Nothing downstream walks a hierarchy.

### Stage 3 — Flatten

**Input:** curve segments.
**Output:** line segments within a specified error tolerance.

Use **Euler spiral / parallel-curve flattening** (Levien's method), not recursive midpoint subdivision. It produces measurably fewer segments for the same error bound, and segment count drives every subsequent stage's cost.

Stroke expansion happens here too: strokes become filled outlines via offset-curve fitting, with proper joins (miter/round/bevel) and caps. **Not** by emitting thick polylines — that approach produces visible artifacts at joins and cannot handle self-intersection correctly.

Dashing is a *path-to-path transform* applied before expansion, not a rasterizer feature.

**Tolerance is in device space**, computed from the resolved transform. A path scaled to 10× needs more segments; a path scaled to 0.1× needs fewer.

### Stage 4 — Bin

**Input:** line segments.
**Output:** per-tile segment lists.

Space is divided into tiles. Segments are assigned to every tile they touch.

- **Sparse:** only tiles actually touched allocate storage. A 4K target is ~2M pixels; a typical UI frame touches a small fraction.
- **Sorted:** segments within a tile are ordered for deterministic accumulation.
- **Tile geometry is tunable.** Start with strip height 4 and wide-tile width 256, then benchmark. Height 4 aligns with 128-bit SIMD lanes; treat these as parameters, not constants baked into algorithms.

Binning is embarrassingly parallel over the segment list, with a merge step.

### Stage 5 — Strip generation

**Input:** per-tile segments.
**Output:** sparse strips (alpha runs + winding deltas).

This is the sparse-strips technique. For each tile column, accumulate signed area analytically to produce exact coverage, then emit:

- **Alpha strips** — spans with partial coverage, carrying per-pixel alpha
- **Solid runs** — spans fully inside the shape, carrying only a winding delta

The second is the win. A large filled rectangle produces a handful of alpha strips at the edges and one solid run across the interior, rather than millions of coverage values.

**Analytic antialiasing** — signed-area accumulation, not supersampling. Supersampling costs N× the work for worse quality on near-horizontal edges.

### Stage 6 — Fine rasterization

**Input:** strips.
**Output:** pixels in the target buffer.

The hot loop. Design constraints:

- **SIMD across x.** Process 4/8/16 pixels per instruction depending on target. Runtime dispatch: SSE4.2 / AVX2 baseline on x86-64, NEON on aarch64, WASM SIMD128 for wasm.
- **Threads across tiles.** Each wide tile is independent — no locking, no false sharing. Tiles are the parallelism unit.
- **Two precision pipelines.** A `u8` pipeline for speed (the default for opaque sRGB UI content) and an `f32` pipeline for accuracy (wide gamut, HDR, heavy compositing). Selected at runtime per render, not at compile time.
- **Paint evaluated inline.** Gradients and image sampling are computed per-span within the fine loop, not materialised into temporary buffers.

### Stage 7 — Compose

**Input:** rasterized layers.
**Output:** final target.

An explicit layer stack. `push_layer(clip, blend_mode, alpha)` allocates an offscreen region sized to the layer's resolved bounds — *not* full-screen. `pop_layer` composites it down.

v1 supports `src-over` and group opacity. The layer machinery is designed for the full Porter-Duff set, separable blend modes, and filters — those land in P5 without structural change.

---

## 5. Threading model

Three thread classes:

| Class | Owns | Count |
|---|---|---|
| **Encode** | Building `Scene` | Any caller thread |
| **Coordinator** | Stages 2–4, dispatch | One per render call |
| **Worker** | Stages 5–6 per tile | Caller-provided pool |

2D-Engine **does not spawn threads**. The caller supplies a pool (or `None` for single-threaded). This keeps us composable inside consumers that already manage their own scheduling, and makes the engine usable in wasm where threading may be unavailable.

Known ceiling: parallel efficiency degrades past roughly 4–8 workers when a scene makes heavy use of layers and clips, because layers introduce ordering dependencies. Measure before promising scaling.

---

## 6. Backend abstraction

There are **two seams**, because vector and raster backends diverge at different points.

```
Stage 1 encode → Stage 2 resolve
                      │
                      ├──▶ VECTOR SEAM: consumes ResolvedScene
                      │      ├──▶ PdfBackend      geometry preserved
                      │      └──▶ SvgBackend      geometry preserved
                      │
                      └──▶ Stage 3 flatten → Stage 4 bin
                                 │
                                 └──▶ RASTER SEAM: ResolvedScene + TileBins
                                        ├──▶ CpuBackend      stages 5–6 CPU, SIMD + threads
                                        ├──▶ HybridBackend   stage 5 CPU, stage 6 GPU
                                        └──▶ GpuBackend      stages 5–6 compute shaders
```

Vector backends attach **before** stage 3. Flattening converts Béziers to line segments, which is precisely the information a vector target needs to keep. Binning is inherently raster — it assigns segments to pixel tiles — so neither stage may run on the vector path.

**Consequence for stage 2:** resolve must not bake device-resolution-dependent values into its output. Curves stay curves. Tolerances are computed in stage 3, not before it.

Two trait tiers, sketched in Doc 02 §7:

- `VectorBackend` — consumes `ResolvedScene`, emits drawing operators
- `RasterBackend` — consumes `ResolvedScene + TileBins`, emits pixels

Neither trait abstracts over device or file creation — a GPU backend needs a `wgpu::Device`, a PDF backend needs a sink. Construction is backend-specific; rendering is generic.

**Vector fidelity is partial by nature.** Blur, image filters, and blend modes with no target equivalent are rasterized into an embedded image for that subtree. Analytic AA is meaningless on the vector path — the consuming renderer supplies its own. Skia and Cairo handle this the same way.

**Parity policy:** backends guarantee *visual* equivalence within a stated tolerance, **not** bit-identical output. Pixel-exact CPU/GPU parity is unsolved industry-wide and pursuing it forfeits most of the GPU's advantage. The conformance suite asserts perceptual difference below threshold, not equality.

---

## 7. Colour pipeline

- **Internal model:** linear-light, premultiplied alpha, `f32` per channel.
- **Colour space is data**, carried on the scene and on paints. Not a global.
- **u8 sRGB fast path** is an optimisation applied when the whole render is provably sRGB, opaque-dominant, and within gamut. It must produce results within tolerance of the f32 path — asserted in CI.
- **Display P3 and HDR** fall out of this design rather than requiring a rewrite. This is the single highest-leverage "get it right by construction" decision in the spec.

---

## 8. Removed legacy

Each of these exists in older renderers for reasons that no longer hold. Excluding them is a deliberate, documented choice — not an oversight.

| Removed | Why it existed | Why it's gone |
|---|---|---|
| Stateful context (`save`/`restore`, current point) | PostScript-era interpreter model | Prevents immutability, off-thread encoding, and diffing (P1) |
| Fixed-point coordinates (24.8) | 1990s CPUs had slow FP | `f32` vectorises; SIMD makes it free |
| TrueType bytecode hinting | 96 DPI screens needed stem snapping | HiDPI is universal. Thousands of lines of interpreter for no visible benefit |
| Subpixel RGB AA (ClearType/LCD) | Same 96 DPI problem | Breaks under rotation, transparency, and non-RGB-stripe panels (OLED). Removes per-glyph gamma tables and RGB filter kernels — large simplification |
| sRGB-only 8-bit compositing | Memory was expensive | Blocks wide gamut and HDR permanently |
| Bitmap font strikes, legacy colour font formats | Predate scalable colour fonts | Variable fonts + COLRv1 natively; legacy formats via fallback only |
| Strings in the renderer API | Convenience | Shaping must live above layout — see Doc 02 §6 |
| Polyline stroke approximation | Cheap | Artifacts at joins, wrong under self-intersection |
| Dashing as a rasterizer feature | Historical | It's a path transform |
| Global state, error codes, `errno` patterns | C ABI constraints | `Result` + no statics; required for P2 |
| Implicit device pixels / DPI | Single-density displays | Scale is just a transform; API is logical coordinates throughout |
| Supersampled AA | Simple to implement | Analytic AA is faster and better |

---

## 9. Principal risks

**R1 — SIMD/threading retrofit.** *Severity: project-ending.* Stage 5's strip format and stage 6's memory layout must assume vectorised lanes and tile independence from the first commit. Mitigation: write the scalar fallback and one SIMD path (NEON or AVX2) simultaneously in P1, never scalar-only.

**R2 — Conflation artifacts.** Adjacent shapes sharing an edge can show seams under analytic AA with independent compositing. This is an open problem that current renderers still list as unresolved. Mitigation: accept in v1, document it, track upstream research.

**R3 — Cache correctness.** The incrementality work (Doc 03) is where subtle, intermittent, hard-to-reproduce bugs live. Mitigation: P5 invariant — every cache disableable, CI asserts identical output.

**R4 — Scope creep via consumers.** Consumers will ask for layout, hit testing, and text convenience APIs. Every one of them breaks §1. Mitigation: this document is the boundary; changes to §1 require explicit sign-off.

**R5 — Rasterizer performance shortfall.** A first-attempt rasterizer may land slower than mature alternatives. Mitigation: benchmark against a known harness from P1 onward, not at the end. Set a target (within 2× of the reference at P3, within 1.2× at P7) and track it per-commit.
