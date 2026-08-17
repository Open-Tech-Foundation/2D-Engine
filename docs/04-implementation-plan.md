# 2D-Engine — Implementation Plan

**For:** autonomous coding agent
**Depends on:** Docs 01–03
**Read `AGENTS.md` before starting any task.**

No time estimates. No staffing. Work is ordered task units with machine-verifiable completion criteria. A task is done when its checks pass, not when it looks finished.

---

## 0. How to use this document

- Tasks execute **in listed order**. A task may not begin until every task it depends on has passing checks.
- Every task states **Done when** as commands that exit 0. If a criterion cannot be expressed as a command, it is not a criterion — it is a note.
- If a task cannot be completed as specified, **stop and report**. Do not substitute a simpler design.
- Do not skip ahead to make a later task easier. Ordering encodes hard constraints (SIMD, arena layout) that cannot be retrofitted.

---

## M0 — Repository & harness

### T0.1 Workspace skeleton
Create the crate layout from Doc 02 §1. Empty crates, correct dependency edges.

**Done when**
```
cargo build --workspace
cargo tree -p otf-2d-engine-scene | grep -qv wgpu
cargo tree -p otf-2d-engine-raster | grep -qv otf-2d-engine-cache
```

### T0.2 CI gates
Build matrix (x86-64, aarch64), clippy with `-D warnings`, `cargo fmt --check`, MSRV check, `no_std` check for `otf-2d-engine-geom`/`otf-2d-engine-color`/`otf-2d-engine-scene`.

**Done when** `./ci/check.sh` exits 0.

### T0.3 Golden-image harness
Test runner that renders a scene twice — `bypass_caches: true` and `false` — and asserts byte equality, then compares against a stored reference PNG.

**Done when** harness runs with zero test cases registered and exits 0. It must fail loudly if a registered case diverges; verify with a deliberately corrupted fixture, then remove it.

### T0.4 Benchmark harness
Criterion-based. Records per-stage timings from `RenderStats`. Writes results to a tracked JSON file for regression comparison.

**Done when** `cargo bench` runs and emits the JSON with zero benchmarks registered.

---

## M1 — Foundations (no rendering)

### T1.1 `otf-2d-engine-geom`
Doc 02 §3. `Point`, `Vec2`, `Size`, `Rect`, `Affine`, `PathEl`, `PathBuilder`. `f64` public, arcs converted to cubics at build time. `rect` and `rounded_rect` as distinct primitives.

**Done when**
- Property tests: `affine.then(affine.inverse()) ≈ IDENTITY` over random inputs
- `Affine::max_scale` matches largest singular value computed independently
- `rounded_rect` output bounds equal input rect
- `no_std` build passes

### T1.2 `otf-2d-engine-color`
Doc 01 §7. Linear premultiplied `f32` model, `ColorSpace` enum, sRGB ⇄ linear conversion, premultiply/unpremultiply.

**Done when**
- Round-trip `srgb8 → linear → srgb8` is identity for all 256³ values *(exhaustive test, mark `#[ignore]` for normal runs)*
- Premultiply round-trip within 1 ULP for alpha > 0
- `no_std` build passes

### T1.3 Scene arena
Doc 02 §2. Structure-of-arrays buffers, `u32` handles, `reset()` retaining capacity. `NodeId`/`NodeHash` fields present but unused (Doc 03 §9). **`SceneUnit` field present and set at construction** — cannot be retrofitted, same category as the node fields.

**Done when**
- `static_assertions` proves `Scene: Send + Sync`
- Encode 1000-draw scene, `reset()`, re-encode: allocation count is 0 on the second pass *(use a counting allocator in tests)*
- Scene serialises to bytes and deserialises to an identical hash

### T1.4 `SceneBuilder` + validation
Doc 02 §5. All encode methods. Every `EncodeError` variant reachable and tested.

**Done when**
- One test per `EncodeError` variant, each asserting the specific error
- Fuzz target: random method call sequences never panic, always return `Ok` or a typed error
- No public method can produce a `Scene` that later panics in stage 2

### T1.5 Stage 2 — resolve
Doc 01 §4. Transform/clip collapse, culling, flat draw list output.

**Done when**
- Output contains no tree structure *(type-level: `ResolvedScene` has no recursive fields)*
- Nested transforms produce mathematically correct absolute affines *(property test against manual composition)*
- Draws fully outside the target are absent from output
- **Output is resolution-independent**: curves are still curves, no flattening tolerance applied *(assert `ResolvedScene` contains cubic/quad segments, not only lines)*. This is what the vector seam depends on — see Doc 01 §6

**M1 gate:** T1.1–T1.5 all green. The IR is now frozen for arena layout. Changing it after this point invalidates M2+.

---

## M2 — Rasterizer core

> **Binding constraint:** T2.3 and T2.4 must land in the same change. A scalar-only rasterizer is not an acceptable intermediate state (Doc 01 R1).

### T2.1 Stage 4 — binning
Sparse tile assignment. Tile geometry as **runtime parameters**, not constants (Q-01).

**Done when**
- Only touched tiles allocate *(assert allocation count scales with covered area, not surface area)*
- Tile height and wide-tile width are configurable; tests run at ≥2 configurations
- Segments within a tile are deterministically ordered *(same input → same order, 1000 runs)*

### T2.2 Stage 5 — strip generation
Analytic AA via signed-area accumulation. Alpha strips + solid runs.

**Done when**
- A full-surface opaque rect produces solid runs, not per-pixel alpha *(assert alpha-strip count is O(perimeter), not O(area))*
- Coverage sums to exact analytic area for axis-aligned and 45° edges, within 1/255
- No supersampling code path exists *(grep gate)*

### T2.3 Stage 6 — fine raster, scalar
Reference implementation. Correctness over speed.

### T2.4 Stage 6 — fine raster, SIMD
One target: NEON on aarch64, AVX2 or SSE4.2 on x86-64 (Q-02). Runtime dispatch.

**Done when (T2.3 + T2.4 jointly)**
- SIMD output is **bit-identical** to scalar across the full golden corpus
- Both paths present in the same commit
- Runtime dispatch selects correctly and falls back on unsupported CPUs

### T2.5 Threaded dispatch
Caller-supplied pool. 2D-Engine spawns nothing (D-16).

**Done when**
- Output is identical at 1, 2, 4, 8 threads *(bit-equality across all four)*
- `threads: None` works and is single-threaded
- `grep -rn "thread::spawn\|rayon::" otf-2d-engine-*/src` returns nothing outside tests
- Throughput scales ≥3× from 1 to 4 threads on the fill benchmark

### T2.6 Solid fills end to end
`fill` with `Paint::Solid`, both fill rules, rectangular clip, `Pixmap` target, u8 pipeline.

**Done when**
- ≥20 golden-image cases registered and passing
- Reference-comparison test against an established renderer within stated tolerance
- Benchmark registered; baseline recorded in the tracked JSON

**M2 gate:** first pixels. Bit-equality invariants above are permanent — they run on every subsequent commit.

---

## M3 — Geometry & paint

### T3.1 Stage 3 — Euler-spiral flattening
Doc 01 §4. Not recursive midpoint subdivision.

**Done when**
- Max deviation from the true curve ≤ tolerance, verified by dense sampling over random Béziers
- Segment count is lower than recursive subdivision at equal tolerance *(comparative test, both implemented; delete the subdivision version after the test passes)*
- Tolerance derived from `Affine::max_scale`, not fixed

### T3.2 Stroke expansion
Offset curves, joins, caps. Not polyline approximation.

**Done when**
- Self-intersecting paths produce correct results *(golden cases)*
- Degenerate inputs handled without panic: zero-length segments, cusps, zero width, miter limit exceeded
- Fuzz target: random paths × random `StrokeStyle`, no panics, no non-finite output
- `grep -rn "polyline" otf-2d-engine-raster/src` returns nothing

### T3.3 Dashing
Path-to-path transform applied before expansion.

**Done when** dash application occurs in stage 3 and no dash logic exists in stages 5–6 *(grep gate)*.

### T3.4 Gradients
Linear and radial, all `Extend` modes, evaluated inline per span.

**Done when** no intermediate gradient buffer is allocated *(allocation-count test)*, and golden cases cover each extend mode.

### T3.5 Arbitrary path clipping
Mask-based.

**Done when** nested clips ≥4 deep produce correct intersection; rect clips still take the fast path *(assert via `RenderStats`)*.

### T3.6 Images
Nearest and bilinear sampling.

### T3.7 Layer stack
`push_layer`/`pop_layer`, `src-over`, group opacity. Layer buffers sized to resolved bounds, not full screen.

**Done when** a small layer on a 4K surface allocates proportional to the layer's bounds *(allocation-size assertion)*.

### T3.8 f32 pipeline + `Pipeline::Auto`
**Done when** u8 and f32 outputs agree within tolerance across the golden corpus, and `Auto` selects u8 for opaque-sRGB scenes *(assert via `RenderStats`)*.

**M3 gate:** SVG test corpus renders correctly. Benchmark regression check passes against the M2 baseline.

---

## M4 — Text → **v1.0**

### T4.1 Outline extraction
Via `skrifa` (Q-03). Do not write a font parser.

### T4.2 Glyph rasterization to A8 atlas
Coverage only. Colour applied at blit time from the paint.

**Done when** one cached glyph renders correctly in ≥3 different colours without re-rasterization *(assert via `CacheStats`)*.

### T4.3 Glyph cache — Bet 3
Doc 03 §5 in full: `GlyphKey`, subpixel buckets, size quantisation, shelf packing, LRU, compaction, large-glyph bypass.

**Done when**
- Hit rate ≥95% on the text benchmark corpus
- Atlas bytes never exceed budget over 10⁶ glyph draws *(soak test)*
- Compaction triggers and preserves correctness under forced fragmentation
- Glyphs above the size threshold bypass the atlas *(assert via `CacheStats`)*

### T4.4 Colour glyphs
COLRv1, separate RGBA atlas with its own budget.

### T4.5 Variable fonts, synthetic bold/oblique

### T4.6 `draw_glyphs` end to end
**Done when** ≥15 text golden cases pass at 1×, 2×, 3× scale, and the text benchmark meets its recorded gate.

**M4 gate — v1.0.** Public API documented. Three example programs build and run.

---

## M5 — Tile caching → v1.1

### T5.1 Tile content hashing
### T5.2 Tile cache with LRU and budget
### T5.3 Damage computation from changed bounds
### T5.4 Scroll fast path
### T5.5 Adaptive policy (Doc 03 §7)

### T5.6 Chaos test
Random mutation sequences; incremental render compared against full re-render every frame.

**Done when**
- 10⁶ random mutations, zero divergence
- Steady-state UI trace rasterizes <10% of tiles *(assert via `RenderStats`)*
- Scroll trace performs zero full-surface rasterizations
- Adaptive policy disables caching on the animation trace *(assert via `CacheStats`)*
- 24-hour soak: budgets never exceeded

> **Blocker:** M5 requires recorded real scene traces. Synthetic scenes lack frame-to-frame coherence and will produce meaningless hit rates. Record traces during M4. If unavailable, **stop and report** — do not proceed against synthetic data.

---

## M6 — Structural sharing & effects → v1.2

### T6.1 Node cache — Bet 1 (Doc 03 §3)
### T6.2 Full Porter-Duff operators
### T6.3 Separable blend modes
### T6.4 Blur and drop shadow
### T6.5 Conic and sweep gradients
### T6.6 `otf-2d-engine-text`: shaping, fallback, bidi

**Done when**
- Node cache hit rate ≥80% on a real consumer trace
- Encode time scales with changed nodes, not total nodes *(assert across trace with 1 vs 100 mutated nodes)*
- `otf-2d-engine-text` shapes Latin, Tamil, Devanagari, Arabic, CJK correctly against reference output

> **Conditional:** T6.1 returns nothing unless a consumer supplies stable `NodeId`s. Verify before implementing; if no consumer does, skip and report.

---

## MV — Vector backends

**May run any time after M3.** Independent of M5/M6 — it attaches at the stage-2 seam and touches no caching or rasterizer code. Schedule against consumer demand.

### TV.1 Trait split
Separate `RasterBackend` and `VectorBackend` per Doc 02 §7. Refactor existing CPU backend onto `RasterBackend`.

**Done when** existing golden corpus passes unchanged after the refactor.

### TV.2 `VectorBackend` scaffolding
`VectorParams`, `VectorFallback`, `VectorStats`, `VectorError`.

**Done when** a `Logical`-unit scene returns `VectorError::UnitRequired`, and every `UnrepresentableFeature` variant is reachable in a test.

### TV.3 SVG backend
Simpler target, do it first. Paths, transforms, clips, gradients, images, glyph runs as geometry.

**Done when**
- Emitted SVG, rendered by an independent renderer, matches our raster output within tolerance
- Output validates against the SVG schema
- Curves appear as `C`/`Q` commands, **not** flattened polylines *(grep gate on output)*

### TV.4 PDF backend
Points as the physical unit, transparency groups for layers, font embedding and subsetting.

**Done when**
- Output opens in ≥2 independent PDF readers
- Physical dimensions correct: a 210 × 297 mm scene measures A4
- Embedded fonts subset correctly; text is selectable and searchable
- Curves are native PDF operators, not flattened

### TV.5 Rasterization fallback
Blur, filters, and unrepresentable blend modes rasterized per-subtree and embedded.

**Done when**
- `VectorFallback::Error` reports every unrepresentable construct rather than silently degrading
- `VectorFallback::Rasterize` produces correct output at the requested DPI
- `VectorStats::fallbacks` is populated accurately

---

## M7 — GPU backend → v2.0

### T7.1 `otf-2d-engine-gpu` on wgpu
### T7.2 Hybrid mode: stage 5 CPU, stage 6 GPU
### T7.3 Backend selection and CPU fallback
### T7.4 Optional surface-integration crate

**Done when** visual parity with CPU within stated tolerance — **not bit-exact** (D-14) — and a measurable win on the large-scene benchmark.

---

## M8 — Hardening

Ongoing, no gate. HDR, wide gamut completeness, wasm SIMD, `no_std` targets, image decode, conformance expansion, fuzzing at scale, API stabilisation to 1.0.

---

## Decision log

Settled. Do not relitigate; if a task appears to require violating one, **stop and report**.

| # | Decision | Rationale |
|---|---|---|
| D-01 | Build from scratch, not adopt `vello_cpu` | Org requirement |
| D-02 | Rust, `no_std`-capable core | Target requirement |
| D-03 | CPU-first, GPU at M7 | Determinism, testability, no driver surface |
| D-04 | Headless; caller owns window and event loop | Embeddability |
| D-05 | No font hinting | HiDPI universal; large legacy surface |
| D-06 | No subpixel RGB antialiasing | Breaks on rotation, transparency, OLED |
| D-07 | Glyph runs in, not strings | Consumer must shape before layout |
| D-08 | Linear premultiplied f32; u8 fast path | Wide gamut and HDR by construction |
| D-09 | Immutable scene, no stateful context | Enables off-thread encode, diffing, caching |
| D-10 | Sparse strips + analytic AA | Current state of the art |
| D-11 | Euler-spiral flattening | Fewer segments per error bound |
| D-12 | SIMD + threading designed in from M2 | Retrofit is a rewrite |
| D-13 | Innovate in caching, copy in rasterization | Caching failure costs a feature; rasterizer failure costs the project |
| D-14 | Visual CPU/GPU parity, not bit-exact | Bit-exact forfeits GPU advantage |
| D-15 | Own geometry types, no `kurbo`/`peniko` | Public API; must be stable |
| D-16 | Engine never spawns threads | Composability; wasm |
| D-17 | Two backend seams: vector after stage 2, raster after stage 4 | Flattening destroys what vector targets need |
| D-18 | Stage 2 output is resolution-independent | Prerequisite for D-17 |
| D-19 | `SceneUnit` on the scene; vector backends reject `Logical` | Guessing a physical unit produces silently wrong output on paper |
| D-20 | Vector fidelity is partial, with rasterization fallback | Blur and filters have no PDF equivalent; Skia and Cairo do the same |
| D-21 | `Scene` stores coordinates as `f64`; narrowing to `f32` happens in stage 2 | Doc 02 §2 and §3 contradicted each other. §3 carries the rationale — world coordinates are unbounded, a page can be 100k logical pixels tall — and D-17/D-18 depend on stage 2's input being device-independent. Costs 2× on `path_data` only |
| D-22 | The arena carries `path_verbs`, `strokes`, `dash_data` and `variations` buffers beyond the Doc 02 §2 sketch | `StrokeStyle`, `Dash` and `VariationsRef` are in the documented public API (Doc 02 §5, §6) and have nowhere else to live. Same SoA rules apply |

---

## Open questions

Human decisions. **Stop and ask** if a task requires one that is unresolved.

| # | Question | Blocks |
|---|---|---|
| Q-01 | Tile strip height and wide-tile width | T2.1 — start 4/256, must remain parameters |
| ~~Q-02~~ | ~~x86-64 SIMD baseline: SSE4.2 or AVX2~~ | **Closed:** AVX2, runtime-dispatched, with the T2.3 scalar path as the fallback. T2.4 asks for one target; I-5 makes every extra SIMD path an extra bit-identity obligation. SSE4.2 may be added in M8 |
| Q-03 | `skrifa` or own font parsing | T4.1 — recommend `skrifa` |
| Q-04 | Vertical subpixel glyph buckets | T4.3 — 4× cache cost, measure first |
| ~~Q-05~~ | ~~Which consumer supplies scene traces~~ | **Closed:** `Web-App-Framework`. Confirm the recording harness lands during M4 |
| ~~Q-06~~ | ~~Licence~~ | **Closed:** MIT or Apache-2.0, matching org convention. Pick one at T0.1 |
| ~~Q-07~~ | ~~Public or internal~~ | **Closed:** public. Doc 02 §8 stability policy is a public commitment |
| ~~Q-08~~ | ~~Is wasm a target, and when~~ | **Closed:** not in v1; M8 as already planned. D-16's caller-supplied pool already covers single-threaded wasm, and SIMD128 is a compile-time feature rather than runtime dispatch, so nothing is retrofitted by deferring |
