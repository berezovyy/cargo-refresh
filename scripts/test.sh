#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export RUSTUP_TOOLCHAIN=1.99.0
export CARGO_BUILD_JOBS=4
nice -n 19 cargo fmt --all --check
nice -n 19 cargo clippy --workspace --all-targets -j 4 -- -D warnings
nice -n 19 cargo test --workspace -j 4
