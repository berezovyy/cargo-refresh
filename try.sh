#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")" && pwd)
case ${1:-} in
  '') SMOKE=0 ;;
  --smoke) SMOKE=1 ;;
  --help|-h) echo 'Usage: ./try.sh [--smoke]'; exit 0 ;;
  *) echo 'Usage: ./try.sh [--smoke]' >&2; exit 2 ;;
esac
[[ $(uname -sm) == 'Linux x86_64' ]] || { echo 'This MVP requires Linux x86_64 with glibc.' >&2; exit 1; }
mkdir -p "$ROOT/.try/tmp"
export TMPDIR="$ROOT/.try/tmp"
for tool in python3 make cc tar zstd; do
  command -v "$tool" >/dev/null || { echo "Missing prerequisite: $tool (see docs/install-layout.md)" >&2; exit 1; }
done
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
if ! command -v rustup >/dev/null; then
  command -v curl >/dev/null || { echo 'Install curl to download rustup.' >&2; exit 1; }
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o "$TMPDIR/rustup-init.sh"
  sh "$TMPDIR/rustup-init.sh" -y --profile minimal --default-toolchain none --no-modify-path
  rm -f "$TMPDIR/rustup-init.sh"
fi
export RUSTUP_TOOLCHAIN=1.99.0 CARGO_BUILD_JOBS=4
if ! rustup run 1.99.0 rustc --version >/dev/null 2>&1; then
  rustup toolchain install 1.99.0 --profile minimal --no-self-update
fi
rustup component add rust-src --toolchain 1.99.0
# The support archive is already built; rustc-dev is only needed to rebuild it.
export CARGO_TARGET_DIR="$ROOT/.try/controller-target"
nice -n 19 cargo install --locked --path "$ROOT/crates/cargo-refresh" --root "$ROOT/.try/install" -j 4
unset CARGO_TARGET_DIR
ARCHIVE="$ROOT/bundle/cargo-refresh-0.1.0-dev-b940084d7eb6a299eb4bfeb8e34901bc051e7ac4-x86_64-linux.tar.zst"
CONTROLLER="$ROOT/.try/install/bin/cargo-refresh"
if (( SMOKE )); then
  exec python3 "$ROOT/scripts/run-guarded.py" python3 "$ROOT/scripts/demo-smoke.py" --controller "$CONTROLLER" --archive "$ARCHIVE"
fi
printf '\nEdit %s/examples/demo/src/main.rs\n' "$ROOT"
echo 'Try changing message() and step(); watch the PID and tick counter stay the same.'
echo 'Ctrl-C stops the demo and its compiler. See examples/demo/README.md for more edits.'
cd "$ROOT/examples/demo"
exec python3 "$ROOT/scripts/run-guarded.py" "$CONTROLLER" refresh run --bin demo --from-archive "$ARCHIVE"
