# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **T0.1** — Cargo workspace skeleton with the crate layout from Doc 02 §1:
  `otf-2d-engine-geom`, `-color`, `-scene`, `-raster`, `-cpu`, `-cache`,
  `-text`, and the `otf-2d-engine` facade. Dependency edges enforce the rules
  in Doc 02 §1: `-scene` sees only `-geom` and `-color`, `-raster` never sees
  `-cache`, and `-text` is a leaf. `-geom`, `-color` and `-scene` are `no_std`.
  Licence pinned to Apache-2.0, closing Q-06.
- **T0.2** — `ci/check.sh`, the single gate that must exit 0 on every commit:
  `cargo fmt --check`, clippy with `-D warnings`, workspace build and test,
  rustdoc with `-D warnings`, a real `no_std` build of `-geom`/`-color`/`-scene`
  against `thumbv7em-none-eabi`, an aarch64 cross-check, an MSRV check against
  the `rust-version` in `Cargo.toml`, and `ci/invariants.sh`. The latter greps
  for violations of the hard invariants in `AGENTS.md` (I-1 through I-4, plus
  the supersampling, polyline and fixed-point gates); opt-outs require a
  `// ci-allow:` marker. A GitHub Actions matrix runs it on x86-64 and aarch64.
- **T0.3** — Golden-image harness in the test-only `otf-2d-engine-testing`
  crate. Each case renders twice, with `bypass_caches` false and true, and the
  two must be byte-equal before the reference is even consulted, so an I-6
  violation is never misreported as a rasterizer bug. Comparison against the
  stored reference PNG is exact; on failure the harness writes actual,
  expected and a magenta diff overlay to `target/golden-failures/`.
  `OTF_BLESS=1` rewrites references and prints a banner restating that
  `AGENTS.md` requires a written justification for doing so. Harness
  self-tests cover every failure mode, including a deliberately corrupted
  one-pixel fixture, so the proof that it fails loudly is permanent rather
  than a one-off manual check. The corpus is registered explicitly in
  `tests/golden.rs` and is currently empty and passing.
- **T0.4** — Criterion-based benchmark harness in `otf-2d-engine-bench`.
  `cargo bench` runs the corpus, folds Criterion's mean estimates plus the
  per-stage timings from `RenderStats` into `target/bench-results.json`, and
  compares it against the tracked `benchmarks/baseline.json`. A slowdown past
  `OTF_BENCH_THRESHOLD` (default 5%) exits non-zero, as does a benchmark that
  silently stopped being measured; a new benchmark is reported but does not
  fail. `OTF_BLESS_BENCH=1` re-records the baseline behind the same
  justify-it-in-the-commit banner the golden harness uses. The corpus is
  currently empty and the run emits the JSON, which is the T0.4 gate.
  `ci/check.sh` gained a `bench` step so this runs on every commit.
- **T1.1** — `otf-2d-engine-geom`: `Point`, `Vec2`, `Size`, `Rect`,
  `RectRadii`, `Affine`, `PathEl`, `PathSeg`, `PathVerb`, `Path` and
  `PathBuilder`. `f64` throughout the public surface per Doc 02 §3. `Affine`
  is the SVG/PostScript `[a b c d e f]` layout with `then` reading in the
  order points travel; `max_scale` is the largest singular value in closed
  form, which is what stage 3 will derive its tolerance from. Paths are
  structure-of-arrays verb and point buffers, and arcs, ellipses and circles
  become cubics in `PathBuilder`, so the IR carries exactly three curve types.
  `rect` and `rounded_rect` are recognised primitives that tag the path with a
  `PathShape` for the stage 4 fast path; radii are clamped CSS-style so a
  rounded rect's bounds are exactly the rect it was built from. Builds
  `no_std` with `libm`.
- **T1.2** — `otf-2d-engine-color`: `Color` as linear-light premultiplied
  `f32` carrying its own `ColorSpace`, per Doc 01 §7. sRGB transfer functions
  in analytic and 8-bit table form, the table being what makes
  `srgb8 -> linear -> srgb8` exact for all 256³ colours (exhaustive test,
  `#[ignore]`d and run nightly). Out-of-range components are mirrored rather
  than clamped, so wide-gamut conversions keep the information an HDR target
  can use. `ColorSpace` conversion goes through CIE XYZ D65 with `from_xyz`
  inverted from `to_xyz` rather than transcribed. `BlendMode` is
  `#[non_exhaustive]` with only the `SrcOver` v1 supports, so M6 can add the
  Porter-Duff set without a breaking change. Builds `no_std` with `libm`.
- **T1.3** — `otf-2d-engine-scene`: the scene arena from Doc 02 §2. Fifteen
  structure-of-arrays buffers joined by `u32` handles and no pointers (I-2),
  no interior mutability (I-1), and `Send + Sync` proved with
  `static_assertions` rather than by running it on a thread. `reset` clears
  every buffer while retaining its allocation and preserves the `SceneUnit`,
  which is a property of the surface and not of the frame; a 1000-draw scene
  re-encoded after `reset` performs zero allocations and zero reallocations,
  measured by a counting global allocator now living in
  `otf-2d-engine-testing`. `NodeId`, `NodeHash` and the `node_descs` buffer
  are written from here but unread until the node cache in M6 (Doc 03 §9) —
  they cannot be retrofitted without changing the layout. Public value types
  (`Paint`, `StrokeStyle`, `Dash`, `Glyph`, `GlyphOptions`, `FillRule`,
  `Extend`, `Sampling`, `Join`, `Cap`, `Hinting`) accompany the `#[repr(C)]`
  `Pod` records they encode into, whose sizes are pinned by test.
  `content_hash` folds every buffer and the unit through `FxHasher` in one
  pass. `to_bytes`/`from_bytes` serialise the arena as a header plus one
  `memcpy` per buffer; the round trip preserves `content_hash` exactly.
  Decoding is total: the header carries a format version, an endianness
  sentinel and a record-layout fingerprint, and every handle in the payload is
  bounds-checked, so a corrupted cache entry produces a `SceneDecodeError`
  rather than a scene that faults in stage 2. Property tests corrupt, truncate
  and forge bytes to hold that line. Decided D-21 (`Scene` stores coordinates
  as `f64`; stage 2 narrows) and D-22 (the arena carries `path_verbs`,
  `strokes`, `dash_data` and `variations` beyond the Doc 02 §2 sketch),
  resolving a contradiction between Doc 02 §2 and §3. Closed Q-02 (AVX2,
  runtime-dispatched, scalar fallback) and Q-08 (wasm is not a v1 target).
- **T1.4** — `SceneBuilder` and encode-time validation, Doc 02 §5. `fill`,
  `stroke`, `draw_glyphs`, `draw_image`, `push_layer`/`pop_layer`,
  `push_node`/`reuse_node`, plus `intern_stops` and `intern_variations` for the
  runs gradients and variable fonts name by handle. No `save`, no `restore`, no
  current point, no current paint: every call carries everything it needs
  (I-3). The builder holds a fixed-size layer stack rather than a `Vec`, so
  encoding a frame allocates nothing (I-9) — tested through the public API, not
  just the raw encoder. Every `EncodeError` variant is reachable and has a test
  asserting that specific error. Two structural guarantees back I-8 beyond the
  per-argument checks: dropping a builder closes any layers left open, and
  `NodeScope` derefs to the builder so an unbalanced node cannot be written.
  The fuzz target drives random sequences of builder calls, including handles
  that name nothing and coordinates that are `NaN` or infinite, and asserts
  that nothing panics and that the resulting scene passes full structural
  validation — across `reset` boundaries too. `reuse_node` returns false until
  the node cache lands in M6, so consumers can write the cache-aware shape now.
  Decided D-23 (`StopsRef`/`VariationsRef` index run tables so a handle carries
  its own length), D-24 (`NodeScope` derefs to the builder, resolving a
  borrow-check contradiction in Doc 03 §3's sketch) and D-25 (a dropped builder
  closes open layers).
- **T1.5** — Stage 2 resolve, Doc 01 §4. `Resolver` turns a `Scene` into a
  `ResolvedScene`: a flat, ordered draw list with absolute transforms,
  collapsed clips, layer extents and off-target draws removed. There is no tree
  in the output and the type system says so — every record is `Copy`, and a
  recursive field would need heap indirection, which is not. Layer nesting
  survives only as order, exactly as the tag stream expresses it. Rectangular
  clips collapse into a single rect, which is nearly all UI clipping; anything
  else becomes a mask in a flat list, and each clip owns a contiguous copy of
  its masks so siblings cannot pick up each other's. Output is
  resolution-independent: stage 2 never touches path geometry, only its
  bounding boxes, so curves reach the vector seam as curves — enforced by a new
  `ci/invariants.sh` gate that fails if `resolve.rs` ever mentions flattening or
  a tolerance. Culling is disableable per P5, and a property test asserts it
  removes exactly the draws that cannot touch the visible region. Stroke bounds
  account for join and cap outset, so a fat stroke whose path lies off-screen
  still survives. A steady-state resolve allocates nothing. Stage 2 is total: a
  hand-built scene with unbalanced layers still resolves to a balanced list.
  Decided D-26 (stage 2 lives in `-scene`, since vector backends must not
  depend on the rasterizer), D-27 (`Resolver` owns the buffers,
  `ResolvedScene<'a>` borrows) and D-28 (glyph runs and images are not culled
  until their extents are knowable).
- The arena layout is frozen at the M1 gate, pinned by a test asserting the
  serialised buffer order, alongside the existing record-size assertions.
- **T2.1** — Stage 4 binning in `otf-2d-engine-raster`. `Binner` assigns
  device-space `Segment`s to every tile they touch, sparsely: storage is
  proportional to covered area, and binning the same geometry into a 4096×
  larger surface allocates nothing extra. Assignment is per crossing, not per
  bounding-box cell — a 45° diagonal lands in the two tiles it passes through
  rather than the four its bbox covers — because bands are half-open, so an
  extent ending exactly on a boundary does not reach into the next tile.
  Segments within a tile are deterministically ordered: assignments are packed
  into `(tile, segment)` `u64` keys and sorted, which is a total order rather
  than an artefact of iteration, and 1000 repeat runs are asserted identical.
  Tile geometry is a runtime parameter (Q-01), defaulting to 256×4 and tested
  at four configurations. Segments become `f32` here — device space is bounded
  by the surface, so the range buys nothing and costs half the SIMD lanes. A
  steady-state bin allocates nothing, and `otf-2d-engine-raster` now builds
  `no_std` and is part of the CI no_std matrix.
- **T2.2** — Stage 5 strip generation. `Striper` turns binned segments into
  sparse strips: per-pixel alpha where coverage varies, constant per-row alpha
  where it does not. Antialiasing is analytic — exact signed area accumulated
  from the geometry, distributed with the closed-form method Levien's `font-rs`
  popularised — so the tests assert an exact area rather than a tolerance band:
  an axis-aligned rect, a 45° triangle and a 45° diamond all integrate to their
  analytic area, and a pixel-aligned rect to its area with no error at all. A
  full-surface opaque rect produces solid runs and no per-pixel coverage; an
  antialiased one costs exactly two alpha columns per band, so alpha storage
  tracks the perimeter and not the area across a 64× area range. Away from any
  edge the running sum is the winding number, which is what makes a solid
  interior free. Both fill rules are implemented and tested against each other
  on overlapping and reversed subpaths. Shapes extending past the surface edge
  still fill: segments left of the surface clamp into column 0 rather than
  being dropped, because they carry winding onto it. Tile geometry does not
  change the pixels, which is what makes it safe to benchmark. A steady-state
  pass allocates nothing. Decided D-29 (coverage is `u8`) and D-30 (a constant
  span needs eight identical columns to be worth emitting).
- **T2.3 + T2.4** — Stage 6 fine rasterization, scalar and AVX2, landing
  together as Doc 01 R1 requires. Strips and a solid paint in, pixels out, into
  a borrowed strided `TargetMut` in `Rgba8Premul` or `Bgra8Premul`. The blend
  is source-over in linear light: the u8 pipeline decodes each destination
  byte, composites, and re-encodes, because compositing sRGB bytes directly is
  nowhere near the `f32` model the spec makes authoritative — half-covered
  black over white is 128 that way and 188 this way. A test asserts the u8
  result stays within one code of an `f32` linear reference across five paints,
  six backgrounds and all 256 coverages.
  Bit-identity between the two kernels is structural rather than measured: the
  blend is entirely integer fixed point with every transfer function a table
  lookup, so there is no float rounding, no reciprocal estimate and no FMA
  contraction to diverge. The AVX2 kernel performs the same operations in the
  same order on eight pixels at once and reads the same tables. The sweep is
  exhaustive — 256 destination bytes × 256 coverages × seven paints × two
  formats — which catches a one-ULP change to the rounding constant that a
  sampled sweep walks straight past; whole-scene renders are compared too. A
  new invariant gate keeps FMA and `rcp`/`rsqrt` out of the crate.
  Dispatch is runtime and per render: `Simd::detect` picks the best the CPU
  supports, and asking for a path the machine cannot run resolves to scalar
  rather than faulting. Full coverage by an opaque paint stores the paint bytes
  without touching a transfer function at all, which is the case a large fill
  spends its time in. Decided D-31 (linear-light blending in the u8 path), D-32
  (a target byte is the encoding of the premultiplied linear value) and D-33
  (the blend is integer, so I-5 is structural).
- **T2.5** — Threaded dispatch. `ThreadPool` is a caller-supplied trait; the
  engine still spawns nothing (I-4, D-16). It hands the pool the target buffer
  and a chunk size rather than an index range, because that is the only shape
  that gives a worker exclusive access to part of the target without a
  per-frame allocation or unsafe aliasing — and it is exactly what a `rayon`
  `par_chunks_mut` adapter wants. Chunks are bands: runs of whole scanlines,
  each written by one worker, so bit equality across thread counts is
  structural rather than tested into existence. Verified anyway at 1, 2, 4 and
  8 threads on both kernels, at ten surface heights that cut the last band
  short, against a pool that runs chunks in reverse, and against one that
  asserts the chunks tile the buffer exactly. `threads: None` never reaches a
  pool at all. Decided D-34 and D-35.

### Fixed

- Stage 6 collapses a constant-coverage span wider than 96 pixels into a
  byte-to-byte map, so the transfer functions leave the per-pixel loop
  entirely. Both kernels take this branch and build the same map, so it is a
  change of work and not of answer. On a Skylake-class core a full-HD
  translucent fill went from 3.24 to 2.98 ns/pixel scalar, and the AVX2 path —
  which had been 2.4× *slower* than scalar on that case, because it is
  gather-bound and `vpgatherdd` is no faster than scalar loads — now matches it
  exactly. The per-pixel-coverage path is still 9.19 vs 4.93 ns/pixel in AVX2's
  disfavour; it is O(perimeter) rather than O(area), so the net effect of
  selecting AVX2 is neutral to positive, and the measurements are recorded in
  D-36 and D-37 rather than left to be rediscovered.

### Changed

- `ci/invariants.sh` no longer greps comment lines. A rule whose own
  explanation trips it is a rule nobody can document.

### Fixed

- `otf-2d-engine-raster`, `-cpu`, `-cache`, `-text` and the `otf-2d-engine`
  facade now forward `std` and `libm` to their dependencies. Without it, the
  workspace only built because feature unification across `--workspace` turned
  `std` on for `-geom` and `-color`; building any one of those crates on its
  own — which is what `cargo bench -p otf-2d-engine-bench` does — hit the
  "needs float math" `compile_error!`.
