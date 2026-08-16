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
