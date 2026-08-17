#!/usr/bin/env bash
#
# ci/invariants.sh — mechanical gates for the hard invariants in AGENTS.md.
#
# Each rule is a pattern that must NOT appear in the given paths. A line may
# opt out by ending with a marker comment, e.g.
#
#     let pool = thread::spawn(...); // ci-allow: I-4 test-only harness
#
# Opt-outs are deliberately ugly and greppable. Adding one is a review event,
# not a fix.

set -uo pipefail

cd "$(dirname "$0")/.."

RED=$'\033[31m'; OFF=$'\033[0m'
if [ ! -t 1 ]; then RED=; OFF=; fi

status=0

# check <id> <message> <extended-regex> <path>...
check() {
    local id=$1 msg=$2 pat=$3; shift 3
    local paths=("$@") existing=()
    local p
    for p in "${paths[@]}"; do [ -e "$p" ] && existing+=("$p"); done
    [ ${#existing[@]} -eq 0 ] && return 0

    # Comment lines are excluded: these gates are about what the code does,
    # and a rule whose own explanation trips it is a rule nobody can document.
    local hits
    hits=$(grep -rHnE --include='*.rs' "$pat" "${existing[@]}" 2>/dev/null \
           | grep -v 'ci-allow:' \
           | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)
    if [ -n "$hits" ]; then
        printf '%s%s violated%s — %s\n' "$RED" "$id" "$OFF" "$msg"
        printf '%s\n' "$hits" | sed 's/^/    /'
        status=1
    fi
}

SCENE=(otf-2d-engine-scene/src)
GEOM=(otf-2d-engine-geom/src)
COLOR=(otf-2d-engine-color/src)
RASTER=(otf-2d-engine-raster/src)
# The shipped crates. Deliberately enumerated rather than globbed, so the
# test-only crates (-testing, -bench) are not held to invariants that describe
# the engine's public behaviour.
ALL_SRC=(
    otf-2d-engine-geom/src
    otf-2d-engine-color/src
    otf-2d-engine-scene/src
    otf-2d-engine-raster/src
    otf-2d-engine-cpu/src
    otf-2d-engine-cache/src
    otf-2d-engine-text/src
    otf-2d-engine/src
)

# I-1 — the scene is immutable after encoding. No interior mutability.
check I-1 'no interior mutability in otf-2d-engine-scene' \
    '\b(Cell|RefCell|UnsafeCell|OnceCell|Mutex|RwLock|AtomicU?[0-9]+|AtomicBool)\b' \
    "${SCENE[@]}"

# I-2 — no pointers in the IR. Only u32 handles.
check I-2 'no owning pointers in the scene IR' \
    '\b(Rc|Arc|Box)\s*<' \
    "${SCENE[@]}" "${GEOM[@]}" "${COLOR[@]}"

# I-3 — no stateful graphics context.
# A stateful context's save/restore mutate the context, so match on the
# receiver rather than the bare name — file-I/O helpers called `save` are not
# what this invariant is about.
check I-3 'no save/restore stateful context API' \
    'fn (save|restore)[a-z_]*\s*\(\s*&mut self' \
    "${ALL_SRC[@]}"

# I-4 — 2D-Engine never spawns a thread.
check I-4 'the engine never spawns threads; pools are caller-supplied' \
    '(thread::spawn|std::thread::spawn|rayon::|spawn_blocking)' \
    "${ALL_SRC[@]}"

# Doc 01 §8 / T2.2 — analytic AA only, never supersampling.
check T2.2 'no supersampling path; antialiasing is analytic' \
    '\b(supersampl|super_sampl|msaa|ssaa|SampleGrid)' \
    "${RASTER[@]}"

# AGENTS.md I-5 / T2.4 — the scalar and SIMD paths must be bit-identical.
# Fused multiply-add and reciprocal estimates are the two things that silently
# make one path disagree with the other: FMA keeps an intermediate at higher
# precision than the scalar mul-then-add, and rcp/rsqrt are approximations
# whose results are not even specified exactly.
check I-5 'no FMA or reciprocal estimates; they break scalar/SIMD bit-identity' \
    '(_mm[0-9_]*_fm(add|sub)|_mm[0-9_]*_r(cp|sqrt)_(ps|ss)|\bmul_add\b|\bto_int_unchecked\b)' \
    "${RASTER[@]}"

# Doc 01 §4 / T3.2 — strokes are offset curves, never thick polylines.
check T3.2 'strokes are expanded as offset curves, not polylines' \
    '\bpolyline\b' \
    "${RASTER[@]}"

# Doc 01 §6 / T1.5 — stage 2 feeds the vector seam, so its output must stay
# resolution-independent. Flattening and its tolerance belong to stage 3.
check T1.5 'stage 2 is resolution-independent; no flattening, no tolerance' \
    '(tolerance|flatten|subdivid|segment_count)' \
    otf-2d-engine-scene/src/resolve.rs

# Doc 01 §8 — fixed-point coordinates were removed by design.
check D-01 'no fixed-point coordinates; f32 vectorises' \
    '\b(Fixed16_16|Fixed24_8|F26Dot6|FixedPoint)\b' \
    "${ALL_SRC[@]}"

if [ $status -eq 0 ]; then
    echo "     all invariant gates clean"
fi
exit $status
