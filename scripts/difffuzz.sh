#!/usr/bin/env bash
# Differential fuzzing runner: builds nula_difffuzz (feature `difffuzz`)
# and runs a campaign of generated programs through the VM, forced-JIT,
# and AOT backends, persisting crashers under fuzz/differential/crashers/.
#
# Usage:
#   scripts/difffuzz.sh [--seeds N] [--time SECONDS] [--seed-base N]
#                       [--release] [--extra ARGS...]
#
# Defaults: --seeds 10000, no time limit. Environment:
#   CARGO_TARGET_DIR   private target dir (default: /tmp/ct-bfuzz)
#   NULANG_STDLIB      stdlib path for the Nulang toolchain
set -euo pipefail

cd "$(dirname "$0")/.."

SEEDS=10000
TIME_ARG=""
SEED_BASE=0
PROFILE="debug"
EXTRA=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --seeds) SEEDS="$2"; shift 2 ;;
        --time) TIME_ARG="--time $2"; shift 2 ;;
        --seed-base) SEED_BASE="$2"; shift 2 ;;
        --release) PROFILE="release"; shift ;;
        *) EXTRA+=("$1"); shift ;;
    esac
done

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/ct-bfuzz}"
export NULANG_STDLIB="${NULANG_STDLIB:-/mnt/agents/nulang/src/stdlib}"

mkdir -p fuzz/differential/crashers

if [[ "$PROFILE" == "release" ]]; then
    BUILD_FLAGS="--release"
else
    BUILD_FLAGS=""
fi

cargo build $BUILD_FLAGS --no-default-features --features difffuzz --bin nula_difffuzz

BIN="${CARGO_TARGET_DIR}/${PROFILE}/nula_difffuzz"
exec "$BIN" --seeds "$SEEDS" --seed-base "$SEED_BASE" $TIME_ARG \
    --crashers fuzz/differential/crashers "${EXTRA[@]}"
