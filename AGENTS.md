# AGENTS.md

Operating rules for coding agents working on 2D-Engine. Read before any task.

---

## Documents

| File | Contains |
|---|---|
| `01-architecture.md` | Pipeline, stages, threading, removed legacy |
| `02-scene-ir-and-api.md` | Crate layout, memory layout, public API |
| `03-incrementality.md` | Caching design — the differentiating work |
| `04-implementation-plan.md` | Ordered tasks with completion criteria |

Specs are authoritative. If code and spec disagree, the spec is right and the code is a bug — unless the spec is demonstrably wrong, in which case **stop and report**. Do not silently diverge.

---

## Hard invariants

Violating any of these is a defect regardless of whether tests pass. Each has a grep or assertion gate in CI.

**I-1 — The scene is immutable after encoding.**
No interior mutability. No `Cell`, `RefCell`, `Mutex` in `otf-2d-engine-scene`. Breaking this breaks off-thread encoding, diffing, and every cache in Doc 03.

**I-2 — No pointers in the IR.** Only `u32` handles. No `Rc`, `Arc`, `Box`, or lifetimes in scene types. `Scene: Send + Sync` is compile-time asserted.

**I-3 — No stateful graphics context.** No `save`/`restore`, no current point, no current paint, no global state. Every draw call is fully specified by its arguments.

**I-4 — 2D-Engine never spawns a thread.** Pools are caller-supplied. `thread::spawn` and `rayon::` appear nowhere outside tests.

**I-5 — Scalar and SIMD paths are bit-identical.** Not "within tolerance". Identical. Asserted across the full golden corpus.

**I-6 — Every cache is disableable and produces identical output when bypassed.** `bypass_caches: true` vs `false` must be byte-equal. This is the prime invariant (Doc 03 §2).

**I-7 — Stages 1–4 contain no backend-specific concepts.** No CPU or GPU types, no `wgpu`, no pixel formats. This is the M7 seam.

**I-8 — Encode-time validation is total.** A `Scene` that encoded without error never panics downstream. Rasterization returns `Result`, never partial output.

**I-9 — Zero allocations in steady state.** After warm-up, `reset()` + re-encode + render allocates nothing on the hot path.

**I-10 — Every cache has a hard byte budget with eviction.** No unbounded growth. Ever.

---

## Removed by design

Do not add these back. Each was removed deliberately (Doc 01 §8). If a task seems to need one, **stop and report** — the task is wrong, or the spec is.

- Font hinting / TrueType bytecode interpretation
- Subpixel RGB (LCD/ClearType) antialiasing
- Supersampled antialiasing
- Fixed-point coordinates
- Stateful context API
- sRGB-only 8-bit compositing as the internal model
- Strings in the renderer API
- Polyline stroke approximation
- Dashing inside the rasterizer
- Global state, error codes, `errno` patterns
- Implicit device pixels or DPI

---

## Working rules

**Order is binding.** Tasks execute in the sequence in Doc 04. Ordering encodes constraints — arena layout, SIMD data layout — that cannot be retrofitted. Do not reorder to make something easier.

**Done means checks pass.** A task's "Done when" criteria are commands that exit 0. Not "looks correct". Not "compiles".

**Never weaken a test to make it pass.** If a golden image diverges, the renderer is wrong. Investigate before touching the fixture. Updating a reference image requires an explicit note explaining why the new output is correct.

**Never disable an invariant to unblock yourself.** Report instead.

**One task per change.** Except T2.3 + T2.4 (scalar + SIMD), which must land together.

**Benchmarks run on every commit.** A regression beyond threshold against the tracked JSON baseline fails the build. Do not raise the threshold to pass.

**Reserved fields stay.** `NodeId` and `NodeHash` exist in the IR from T1.3 but are unused until M6. Do not remove them as dead code — they cannot be added later without invalidating the arena layout.

---

## When to stop and ask

Stop, do not improvise, when:

- A task requires resolving an open question (Q-01…Q-08 in Doc 04)
- A task appears to require violating an invariant or a logged decision
- A spec is internally contradictory or contradicts another spec
- A performance gate cannot be met without a design change
- M5 is reached without real recorded scene traces
- A dependency not listed in Doc 02 §1 seems necessary

Report what you found, what you'd propose, and why. Then wait.

---

## Dependency policy

The v1 dependency list in Doc 02 §1 is closed. Adding to it requires justification against: is this on the hot path, is it a public-API type, could we write it in under 200 lines, does it pull a transitive tree.

Explicitly not dependencies in v1: `kurbo`, `peniko`, `lyon`, `tiny-skia`, `wgpu`, `winit`, or any windowing crate.

---

## Reporting format

On task completion:

```
TASK: T2.4
STATUS: complete
CHECKS: <commands run, exit codes>
BENCHMARK DELTA: <vs baseline>
NOTES: <anything surprising>
```

On blocking:

```
TASK: T5.1
STATUS: blocked
REASON: <what>
SPEC REF: <doc and section>
PROPOSED: <options, with tradeoffs>
```
