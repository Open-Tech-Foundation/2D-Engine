# 2D-Engine — Incrementality & Caching

**Status:** Draft for review
**Depends on:** Doc 01 (Architecture), Doc 02 (Scene IR)

---

## 1. Why this document exists

Docs 01 and 02 describe a competent modern renderer. Built well, it lands somewhere near existing alternatives — which, for a two-year investment, is not a good outcome.

This document contains the part that is actually differentiated.

### The asymmetry

General-purpose renderers cannot assume anything about their workload. **2D-Engine can.** The target consumer is application UI and browser-like views, where the following hold overwhelmingly:

| Property | Typical figure |
|---|---|
| Screen unchanged between frames | 90–98% |
| Geometry that is axis-aligned rect or rounded rect | ~80% of draws |
| Glyph reuse (same font/size/glyph) | ~200 unique glyphs, thousands of draws |
| Scene structure stable frame to frame | Only leaves change |
| Damage shape | Small, rectangular, few regions |

A general renderer must redraw everything correctly every frame. 2D-Engine can *know* a panel didn't change.

**This is worth more than a faster inner loop.** A 20% faster rasterizer saves 20% of raster time. Not rasterizing saves 100%. For a small team, exploiting workload structure is the only durable edge — the rasterizer core is a solved research problem where we will not beat funded specialists.

### The three bets

1. **Structural sharing** — unchanged subtrees cost a hash comparison, not a re-encode
2. **Damage-driven tile caching** — unchanged pixels are memcpy, not rasterization
3. **Text as a first-class primitive** — glyphs never enter the path pipeline

All three live in stages 1–2 and 7. **None touch stages 3–6.** This is deliberate: innovate where failure costs a feature, copy where failure costs the project. If all three bets fail, we still have a working renderer.

---

## 2. Prime invariant

> **Every cache must be disableable, and output with caches disabled must be identical to output with caches enabled.**

This is Doc 01 P5, restated because everything below depends on it.

Enforcement:

- `RenderParams::bypass_caches` disables all three caches at runtime
- CI runs the full golden-image suite twice, cached and bypassed, and asserts bit-identical results
- A randomised "chaos" test drives random mutation sequences and compares incremental rendering against full re-render every frame
- Any divergence is a release blocker, not a bug report

Without this, incrementality produces intermittent, unreproducible, user-visible corruption — the worst class of bug in a rendering engine, and the reason many projects abandon caching after shipping it.

---

## 3. Bet 1 — Structural sharing

### Problem

A browser-like consumer re-renders its entire tree on every DOM mutation. Changing one text node produces a full scene rebuild. Encoding a complex UI is not free — it is often comparable to rasterizing it.

### Design

Scene subtrees become **content-addressed persistent nodes**.

```rust
/// Caller-supplied stable identity. Typically a DOM node id,
/// widget id, or layout box id.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct NodeId(pub u64);

/// Hash of a node's encoded content, including descendants.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct NodeHash(pub u64);

pub struct NodeScope<'a> { /* RAII; computes hash on drop */ }
```

Consumer usage:

```rust
// Naive rebuild — the consumer does NOT track dirtiness.
for panel in &ui.panels {
    if sb.reuse_node(panel.id, panel.transform) {
        continue;                       // Cache hit: nothing encoded
    }
    let _scope = sb.push_node(panel.id);
    encode_panel(&mut sb, panel);       // Cache miss: encode normally
}
```

The consumer stays simple. 2D-Engine does the work.

### Hashing

Hash computed on `NodeScope` drop, over the byte ranges the node contributed to each arena buffer.

- **Non-cryptographic, 64-bit** (`rustc-hash` or `xxhash`). Collision probability at realistic node counts (~10⁵) is negligible; we are not defending against adversarial input.
- **Incremental**: parent hash combines child hashes with its own local content. Nested reuse works without rehashing descendants.
- **Transform-independent**: the transform is *excluded* from the hash and applied at reuse. A panel that only moved is a cache hit with a different transform — a very common case during scroll.

### Storage

```rust
struct NodeCache {
    /// NodeId → (hash, encoded byte ranges, bounds)
    entries: FxHashMap<NodeId, NodeEntry>,
    /// Retained arena holding encoded content of cached nodes.
    arena:   SceneArena,
    budget:  usize,
    lru:     LruList,
}
```

On `reuse_node`, matching entries are copied — a `memcpy` of contiguous bytes with index fixups — instead of re-executing consumer encoding logic.

### Expected win

Encoding time proportional to *changed* nodes rather than total nodes. For a typical UI mutation this is a 10–100× reduction in encode cost. It also directly feeds Bet 2: the diff tells us exactly what changed, which is where damage rectangles come from.

### Failure modes

| Risk | Mitigation |
|---|---|
| Consumer supplies unstable `NodeId`s | Metric: hit rate in `RenderStats`. Below ~50% signals consumer misuse; document loudly |
| Cache larger than the work it saves | Hard byte budget, LRU eviction, `memory_usage()` exposed |
| Hash collision | 64-bit is sufficient; optionally verify by content comparison in debug builds |
| Nodes that change every frame | Adaptive: track per-node hit rate, stop caching nodes that always miss |

---

## 4. Bet 2 — Damage-driven tile caching

### Problem

Rasterizing a 4K frame on CPU is expensive regardless of how good the rasterizer is. But typing a character changes a few hundred pixels, and scrolling a list changes a strip at one edge.

### Design

Cache **rasterized tile contents**, keyed by what was drawn into them.

```rust
struct TileKey {
    tile_x: u16,
    tile_y: u16,
    /// Hash of the ordered set of draws intersecting this tile,
    /// including their resolved transforms and paints.
    content: u64,
}

struct TileCache {
    entries: FxHashMap<TileKey, TilePixels>,
    budget:  usize,
    lru:     LruList,
    stats:   TileCacheStats,
}
```

### Pipeline integration

Between stage 4 (bin) and stage 5 (strips):

```
Stage 4: bin  ─┬─▶ compute per-tile content hash
               │
               ├─▶ HIT:  blit cached pixels, skip stages 5–6 entirely
               └─▶ MISS: stages 5–6 normally, then insert
```

### Damage computation

Damage is *derived*, not consumer-supplied. Bet 1's node diff yields changed nodes; changed node bounds (old ∪ new) yield dirty rects; dirty rects yield the tile set to re-rasterize.

```rust
fn compute_damage(prev: &NodeCache, curr: &Scene) -> DamageRegion {
    // Union of bounds of nodes whose hash changed,
    // in both their previous and current positions.
}
```

Consumers may still pass explicit damage via `RenderParams::damage`, which intersects with computed damage. When neither is available, fall back to full redraw.

**Damage coalescing:** more than ~8 rects, or union area exceeding ~60% of the surface, collapses to a single bounding rect. Tracking many small regions costs more in bookkeeping than it saves.

### Scroll fast path

Scrolling is the single most common expensive interaction in application UI, and it has special structure: content is unchanged, only translated.

Detection: all visible node hashes unchanged, transforms differ by pure integer translation.
Action: `memmove` the shared region within the target, rasterize only the newly exposed strip.

This turns a full-surface rasterization into a memmove plus a few hundred rows. Worth implementing as an explicit special case even though the general tile cache would partially handle it.

### Expected win

Steady-state UI frames rasterize 2–10% of tiles. Combined with Bet 1, a typical mutation frame becomes single-digit milliseconds at 4K on CPU — which is what makes the CPU-first decision viable rather than a compromise.

### Failure modes

| Risk | Mitigation |
|---|---|
| Cache thrash on animation | Adaptive: detect high miss rate, disable per-region, re-enable when stable |
| Memory blowup at 4K | Hard budget (default 64 MB, configurable), LRU, stats exposed |
| Stale tiles after a missed invalidation | Prime invariant §2 — chaos testing is the primary defence |
| Tile boundary seams | Tiles cache *composited* output; layer boundaries force cache bypass for affected tiles |

---

## 5. Bet 3 — Text as a first-class primitive

### Problem

Most renderers treat glyphs as generic paths: extract outline, flatten, bin, rasterize. For a workload that is predominantly text, this runs the entire path pipeline thousands of times per frame for perhaps 200 unique shapes.

### Design

Glyphs bypass stages 3–5 entirely and become coverage blits.

```rust
#[derive(PartialEq, Eq, Hash)]
struct GlyphKey {
    font:        FontId,
    glyph_id:    u32,
    /// Quantised size — 1/4 px buckets.
    size_q:      u16,
    /// Quantised subpixel offset — typically 4x1 or 4x4 buckets.
    subpixel:    SubpixelOffset,
    /// Non-translation component of the transform, quantised.
    /// Identity for the overwhelming majority of UI text.
    skew_q:      SkewKey,
    variations:  VariationHash,
    synth_bold:  u16,
}

struct GlyphCache {
    /// Key → location in a coverage atlas (A8, not RGBA).
    entries: FxHashMap<GlyphKey, AtlasSlot>,
    atlas:   CoverageAtlas,
    budget:  usize,
    lru:     LruList,
}
```

### Path

```
draw_glyphs
   └─▶ per glyph: look up GlyphKey
         ├─ HIT:  blit A8 coverage × paint → target
         └─ MISS: extract outline → flatten → rasterize to atlas → blit
```

The miss path uses the normal rasterizer, so there is no duplicated rasterization logic. The hit path never touches it.

### Details that matter

- **A8 coverage, not colour.** Colour comes from the paint at blit time. One cached glyph serves every colour it is drawn in — a large multiplier, since UI text is the same glyphs in a handful of colours.
- **Subpixel positioning, not hinting.** Doc 01 removes hinting. Quality comes from caching at quantised subpixel offsets (4 horizontal buckets is the standard trade; measure whether vertical buckets are worth the 4× cache pressure).
- **Size quantisation** at 1/4 px keeps the cache bounded under animated size changes without visible stepping.
- **Large glyphs bypass the cache.** Above a threshold (default 128 px), atlas pressure exceeds the benefit — rasterize directly.
- **Colour glyphs (COLRv1, embedded bitmaps)** take a separate RGBA atlas path. Emoji are a small fraction of glyphs but a large fraction of atlas bytes; budget them separately.

### Atlas management

Shelf-packing allocator. On full: evict LRU, and if fragmentation exceeds threshold, compact by repacking live entries. **Never grow unboundedly** — an unbounded glyph atlas is a classic memory leak in renderers, and eviction policy is where most implementations are careless.

### Expected win

Text rendering becomes memory-bandwidth-bound rather than compute-bound. For text-heavy frames — which is most application UI — this is the dominant cost reduction, plausibly larger than Bets 1 and 2 combined.

---

## 6. Memory budgets

Every cache is explicitly bounded. Defaults, all configurable:

| Cache | Default budget | Eviction | Notes |
|---|---|---|---|
| Node cache (Bet 1) | 32 MB | LRU + adaptive hit-rate | Scales with UI complexity |
| Tile cache (Bet 2) | 64 MB | LRU | Scales with surface area |
| Glyph atlas — coverage (Bet 3) | 16 MB | LRU + compaction | ~1024² A8 pages |
| Glyph atlas — colour (Bet 3) | 8 MB | LRU + compaction | Emoji, COLRv1 |
| Scene arena | Unbounded, caller-reset | — | Caller controls via `reset()` |

```rust
pub struct CacheBudgets {
    pub node_bytes:         usize,
    pub tile_bytes:         usize,
    pub glyph_coverage_bytes: usize,
    pub glyph_color_bytes:  usize,
}

pub struct CacheStats {
    pub node_hits:  u64, pub node_misses:  u64,
    pub tile_hits:  u64, pub tile_misses:  u64,
    pub glyph_hits: u64, pub glyph_misses: u64,
    pub bytes_used: CacheBudgets,
    pub evictions:  u64,
    pub compactions: u64,
}
```

Statistics ship in release builds. They are the only way to diagnose a consumer whose usage pattern defeats the caches, and the only way to catch a regression that silently drops hit rate from 95% to 40%.

---

## 7. Adaptive behaviour

Caches that always run are worse than no caches for adversarial workloads (continuous animation, procedural content, video).

```rust
struct AdaptivePolicy {
    /// Sliding window of recent hit rate.
    window:          RingBuffer<f32>,
    /// Below this, stop attempting this cache.
    disable_below:   f32,   // default 0.25
    /// Re-probe periodically to detect workload change.
    reprobe_frames:  u32,   // default 120
}
```

Applied independently per cache and, for the tile cache, per screen region — an animating video area can bypass caching while surrounding static UI continues to benefit.

---

## 8. Testing strategy

This subsystem is where correctness bugs hide. Testing is not optional scope.

| Layer | What it does | Frequency |
|---|---|---|
| **Golden images** | Reference PNGs, cached vs bypassed, bit-identical assertion | Every commit |
| **Chaos test** | Random mutation sequences; incremental vs full re-render compared every frame | Every commit |
| **Cache-state fuzzing** | Random eviction, budget exhaustion, compaction under load | Nightly |
| **Property tests** | Damage region always ⊇ actually-changed pixels | Every commit |
| **Benchmark suite** | Hit rates and frame times on recorded real-world scene traces | Every commit, tracked over time |
| **Memory tests** | Sustained run asserting budgets are never exceeded | Nightly |

**Scene traces** deserve emphasis: record real scene sequences from a consumer application early, and use them as the benchmark corpus. Synthetic benchmarks will mislead about cache hit rates, because synthetic scenes lack the frame-to-frame coherence that these bets exploit.

---

## 9. Implementation ordering

These are **not** all v1. Ordering by (value ÷ risk):

| Bet | Phase | Rationale |
|---|---|---|
| **Bet 3** (glyph cache) | **P3, in v1** | Lowest risk, highest certain value, self-contained. Required for usable text performance regardless |
| **Bet 2** (tile cache) | P4, post-v1 | Medium risk. Needs the prime invariant test infrastructure first |
| Scroll fast path | P4 | Subset of Bet 2, ship as soon as Bet 2 lands |
| **Bet 1** (structural sharing) | P5, post-v1 | Highest risk, requires consumer cooperation to prove value |

**Design for all three in P0.** The scene IR must carry `NodeId` and `NodeHash` fields from the first commit even though nothing consumes them until P5 — the arena layout and handle scheme cannot be retrofitted, exactly as with SIMD.

Ship the reserved fields unused. Do not ship a scene IR that cannot express them.
