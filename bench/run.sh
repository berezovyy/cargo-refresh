#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
export RUSTUP_TOOLCHAIN=1.99.0 CARGO_BUILD_JOBS=4
mkdir -p "$ROOT/.bench-work/tmp"
export TMPDIR="$ROOT/.bench-work/tmp"
exec python3 "$ROOT/scripts/run-guarded.py" python3 "$ROOT/bench/measure.py" "$@"
