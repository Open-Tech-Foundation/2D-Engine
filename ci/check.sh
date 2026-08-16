#!/usr/bin/env bash
#
# ci/check.sh — the T0.2 gate. Every check here runs on every commit.
#
# Usage:
#   ci/check.sh              run everything
#   ci/check.sh fmt clippy   run a subset (see `steps` below)
#
# Environment:
#   OTF_MSRV        MSRV toolchain to check against (default: read from Cargo.toml)
#   OTF_SKIP_CROSS  set to 1 to skip cross-target builds

set -euo pipefail

cd "$(dirname "$0")/.."
ROOT=$(pwd)

RED=$'\033[31m'; GREEN=$'\033[32m'; YELLOW=$'\033[33m'; BOLD=$'\033[1m'; OFF=$'\033[0m'
if [ ! -t 1 ]; then RED=; GREEN=; YELLOW=; BOLD=; OFF=; fi

FAILURES=()

run() {
    local name=$1; shift
    printf '%s==>%s %s\n' "$BOLD" "$OFF" "$name"
    if "$@"; then
        printf '  %sok%s     %s\n' "$GREEN" "$OFF" "$name"
    else
        printf '  %sFAILED%s %s\n' "$RED" "$OFF" "$name"
        FAILURES+=("$name")
    fi
}

skip() {
    printf '  %sskip%s   %s\n' "$YELLOW" "$OFF" "$1"
}

# Crates that must compile without `std`. Doc 02 §1.
NO_STD_CRATES=(otf-2d-engine-geom otf-2d-engine-color otf-2d-engine-scene)
# A target with no `std` at all, so a stray `use std::` is a hard error.
NO_STD_TARGET=thumbv7em-none-eabi
# Doc 01 §5 / D-12: the two architectures the rasterizer must build for.
CROSS_TARGETS=(aarch64-unknown-linux-gnu)

msrv() {
    grep -m1 '^rust-version' Cargo.toml | sed 's/[^0-9.]//g'
}

# ---------------------------------------------------------------- steps

step_fmt() {
    cargo fmt --all --check
}

step_clippy() {
    cargo clippy --workspace --all-targets --all-features -- -D warnings
}

step_build() {
    cargo build --workspace --all-targets
}

step_test() {
    cargo test --workspace
}

step_docs() {
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
}

step_no_std() {
    if ! rustup target list --installed | grep -qx "$NO_STD_TARGET"; then
        skip "no_std ($NO_STD_TARGET not installed; rustup target add $NO_STD_TARGET)"
        return 0
    fi
    local crate
    for crate in "${NO_STD_CRATES[@]}"; do
        printf '     %s\n' "$crate --no-default-features --features libm"
        cargo build -p "$crate" --no-default-features --features libm \
            --target "$NO_STD_TARGET"
    done
}

step_cross() {
    if [ "${OTF_SKIP_CROSS:-0}" = 1 ]; then skip "cross (OTF_SKIP_CROSS=1)"; return 0; fi
    local target
    for target in "${CROSS_TARGETS[@]}"; do
        if ! rustup target list --installed | grep -qx "$target"; then
            skip "cross $target (not installed; rustup target add $target)"
            continue
        fi
        printf '     %s\n' "$target"
        cargo check --workspace --target "$target"
    done
}

# AGENTS.md: benchmarks run on every commit, compared against the tracked
# baseline. A regression beyond threshold fails the build.
step_bench() {
    if [ "${OTF_SKIP_BENCH:-0}" = 1 ]; then skip "bench (OTF_SKIP_BENCH=1)"; return 0; fi
    cargo bench -p otf-2d-engine-bench
}

step_msrv() {
    local v=${OTF_MSRV:-$(msrv)}
    if ! rustup toolchain list | grep -q "^$v"; then
        skip "msrv $v (toolchain not installed; rustup toolchain install $v)"
        return 0
    fi
    printf '     rust %s\n' "$v"
    cargo "+$v" check --workspace --all-targets
}

# Invariant gates from AGENTS.md. Each is a grep that must find nothing.
#
# Format: <invariant>|<message>|<pattern>|<path glob...>
step_invariants() {
    "$ROOT/ci/invariants.sh"
}

# ---------------------------------------------------------------- driver

steps=(fmt clippy build test docs no_std cross msrv bench invariants)
if [ $# -gt 0 ]; then steps=("$@"); fi

for s in "${steps[@]}"; do
    run "$s" "step_$s"
done

echo
if [ ${#FAILURES[@]} -eq 0 ]; then
    printf '%sall checks passed%s\n' "$GREEN" "$OFF"
    exit 0
fi
printf '%s%d check(s) failed:%s\n' "$RED" "${#FAILURES[@]}" "$OFF"
printf '  %s\n' "${FAILURES[@]}"
exit 1
