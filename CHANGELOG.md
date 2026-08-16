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
