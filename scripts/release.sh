#!/usr/bin/env bash
# Build a local release candidate. Never publishes or downloads executables.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PORTABLE=1
ARGS=()
for arg in "$@"; do
  case "$arg" in
    --portable) PORTABLE=1 ;;
    --native) PORTABLE=0 ;;
    *) ARGS+=("$arg") ;;
  esac
done
set -- "${ARGS[@]}"
[[ $# == 1 && $1 =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || {
  echo 'Usage: scripts/release.sh [--portable|--native] <version> (portable is default)' >&2; exit 2;
}
VERSION=$1
RESEARCH=${REFRESH_RESEARCH_ROOT:-/path/to/research}
RUSTD=${REFRESH_RUSTD_SOURCE:-rustd}
LANE=${REFRESH_LANE:-$RESEARCH/tools/lane.sh}
[[ -z $(git -C "$ROOT" status --porcelain -- agent) ]] || { echo 'Commit agent changes before release' >&2; exit 1; }
export RUSTUP_TOOLCHAIN=1.99.0
export CARGO_BUILD_JOBS=4
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER CARGO_BUILD_TARGET
COMMIT=$(rustc -Vv | sed -n 's/^commit-hash: //p')
[[ $COMMIT == b940084d7eb6a299eb4bfeb8e34901bc051e7ac4 ]] || { echo 'Unqualified rustc commit; run manual qualification first' >&2; exit 1; }
[[ $(uname -sm) == 'Linux x86_64' ]] || { echo 'Only Linux x86_64 releases are supported' >&2; exit 1; }
for tool in zstd tar objdump nm sha256sum python3 make cc; do command -v "$tool" >/dev/null; done
[[ $(git -C "$RUSTD" branch --show-current) == refresh/m1 ]] || { echo 'rustd must be on refresh/m1' >&2; exit 1; }
# Check the compiler and RA together inside their resource lane, before compiling.
"$LANE" rustd -- bash -c 'test -z "$(git -C "$1" status --porcelain)" && test -z "$(git -C "$1/vendor/rust-analyzer" status --porcelain)"' _ "$RUSTD" || {
  echo 'Compiler/RA sources must be clean' >&2; exit 1;
}
RUSTD_COMMIT=$(git -C "$RUSTD" rev-parse HEAD)
RA_COMMIT=$(git -C "$RUSTD" rev-parse HEAD:vendor/rust-analyzer)
WORK=$ROOT/.release-work
mkdir -p "$WORK" "$ROOT/dist"
NAME=cargo-refresh-$VERSION-$COMMIT-x86_64-linux
STAGE=$(mktemp -d "$WORK/package.XXXXXXXX")
trap 'rm -rf "$STAGE"' EXIT
# Zig is installed by the caller; no implicit network setup in release builds.
# REFRESH_ZIG may point directly at the user-space Zig executable.
if (( PORTABLE )); then
  command -v cargo-zigbuild >/dev/null
  export ZIG=${REFRESH_ZIG:-zig}
  "$ZIG" version
  export CARGO_ZIGBUILD_ZIG_PATH="$ZIG"
  BUILD_TARGET=x86_64-unknown-linux-gnu.2.28
  BUILD_COMMAND=zigbuild
  BINARY_DIR="$WORK/rustd-portable-target/x86_64-unknown-linux-gnu/release"
  TARGET_DIR="$WORK/rustd-portable-target"
  TARGET_ARGS=(--target "$BUILD_TARGET")
else
  BUILD_COMMAND=build
  BINARY_DIR="$WORK/rustd-target/release"
  TARGET_DIR="$WORK/rustd-target"
  TARGET_ARGS=()
fi
# Build with bounded parallelism and RAM guard, private target, no daemon.
"$LANE" build -- python3 "$ROOT/scripts/run-guarded.py" env \
  CARGO_TARGET_DIR="$TARGET_DIR" CARGO_PROFILE_RELEASE_DEBUG=0 \
  nice -n 19 cargo "$BUILD_COMMAND" "${TARGET_ARGS[@]}" --manifest-path "$RUSTD/Cargo.toml" --release --locked --offline -p rustd --bins -j 4
for bin in rustd cargo-rustd rustd-rustc; do
  install -m 755 "$BINARY_DIR/$bin" "$STAGE/$bin"
done
STATIC=0
if (( PORTABLE )); then
  # The production hybrid agent uses libc + dlopen(libgcc_s), not test-only CFI
  # libunwind. Never pull host libraries into a portable link.
  nice -n 19 make -B -C "$ROOT/agent" BUILD=build-portable all \
    CC="$ZIG cc -target x86_64-linux-gnu.2.28" \
    LDLIBS='-Wl,-z,now -ldl -lpthread'
  install -m 755 "$ROOT/agent/build-portable/librefresh.so" "$STAGE/librefresh.so"
else
  nice -n 19 make -B -C "$ROOT/agent" release
  install -m 755 "$ROOT/agent/build-release/librefresh.so" "$STAGE/librefresh.so"
fi
"$ROOT/scripts/new-toolchain.sh" 1.99.0 --dry-run
QUAL=${REFRESH_QUALIFICATION_OUT:-$WORK/qualification/$COMMIT}
install -m 644 "$QUAL/abi-facts.bundle" "$STAGE/abi-facts.bundle"
python3 "$ROOT/scripts/qualification/runtime-requirements.py" "$STAGE" > "$STAGE/RUNTIME.txt"
cat "$STAGE/RUNTIME.txt"
if (( PORTABLE )); then
  python3 "$ROOT/scripts/check-glibc.py" "$STAGE" 2.28
fi
cat > "$STAGE/VERSION" <<METADATA
cargo_refresh=$VERSION
cargo_refresh_commit=$(git -C "$ROOT" rev-parse HEAD)
rustd_commit=$RUSTD_COMMIT
ra_commit=$RA_COMMIT
rustc_commit=$COMMIT
target=x86_64-unknown-linux-gnu
agent_static_libunwind=$STATIC
glibc_max=$(if (( PORTABLE )); then echo 2.28; else echo host; fi)
METADATA
mkdir "$STAGE/licenses"
"$LANE" rustd -- bash -c 'for file in LICENSE-MIT LICENSE-APACHE; do install -m 644 "$1/vendor/rust-analyzer/$file" "$2/licenses/rust-analyzer-$file"; done' _ "$RUSTD" "$STAGE"
for file in LICENSE-MIT LICENSE-APACHE; do
  install -m 644 "$ROOT/$file" "$STAGE/licenses/cargo-refresh-$file"
  install -m 644 "$RUSTD/$file" "$STAGE/licenses/rustd-$file"
done
# A concurrent source change would make the recorded source identity misleading.
[[ $(git -C "$RUSTD" rev-parse HEAD) == "$RUSTD_COMMIT" ]] || { echo 'Compiler changed during release' >&2; exit 1; }
"$LANE" rustd -- bash -c 'test -z "$(git -C "$1" status --porcelain)"' _ "$RUSTD"
(
  cd "$STAGE"
  find . -type f ! -name SHA256SUMS -print0 | LC_ALL=C sort -z | xargs -0 sha256sum > SHA256SUMS
  sha256sum -c SHA256SUMS
)
EPOCH=$(git -C "$RUSTD" show -s --format=%ct HEAD)
TEMP=$STAGE/$NAME.tar.zst
# Explicit members prevent inclusion of the temporary archive in itself.
tar --sort=name --mtime="@$EPOCH" --owner=0 --group=0 --numeric-owner \
  -C "$STAGE" -cf - rustd cargo-rustd rustd-rustc librefresh.so abi-facts.bundle VERSION RUNTIME.txt licenses SHA256SUMS \
  | zstd -T1 -q -o "$TEMP"
mv "$TEMP" "$ROOT/dist/$NAME.tar.zst"
(cd "$ROOT/dist" && sha256sum "$NAME.tar.zst" > "$NAME.tar.zst.sha256" && cat "$NAME.tar.zst.sha256")
stat --format='%n %s bytes' "$ROOT/dist/$NAME.tar.zst"
echo 'Local candidate only. Full qualification and oldest-distro validation remain release gates.'
