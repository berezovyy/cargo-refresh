# Installation and support bundles

The simplest route is `./try.sh`. It installs the controller locally, ensures
Rust **1.99.0** plus **rust-src**, and uses `bundle/*.tar.zst`. No published
package, second repository, submodule initialization, or release download is needed.

## Requirements

Linux x86_64 with glibc; the qualified compiler is Rust 1.99.0 commit
`b940084d7eb6a299eb4bfeb8e34901bc051e7ac4`. Runtime needs a C compiler/linker for
Cargo, make, Python 3, GNU tar, zstd, libc, libgcc_s and the toolchain libraries. The
committed portable bundle needs **glibc 2.28 or newer**. Packaging checks every
ELF payload's GLIBC imports against that floor. Rust and Cargo also need to run
on the host distribution.

The warm compiler needs several GiB of RAM; large projects need much more.
Startup estimates memory and uses Cargo if the estimate plus a 1 GiB reserve
exceeds available host/cgroup headroom. The runtime guard degrades to Cargo under
pressure.

## Build support from source

All compiler and frontend source is in `rustd/`, with licenses and source pins in
[rustd/README-FORK.md](../rustd/README-FORK.md). Install Git, make, a C compiler,
linker, binutils (`nm`, `objdump`), Python 3, GNU tar and zstd, then:

```sh
export RUSTUP_TOOLCHAIN=1.99.0
rustup toolchain install 1.99.0 --profile minimal
rustup component add rustc-dev rust-src llvm-tools-preview --toolchain 1.99.0
scripts/build-bundle.sh --native
```

This builds the included compiler with four jobs and a 6 GiB MemAvailable guard,
rebuilds the agent, extracts/checks ABI facts, and writes the archive and adjacent
checksum under `dist/`. No daemon is started. The first compiler build can take
several minutes. Matching llvm-tools-preview supplies the LLVM linker library
needed by the embedded assertion printer. `REFRESH_RUSTD_SOURCE` overrides the source directory; a qualified
external checkout is also accepted. Warm targets/logs live in `.release-work/`.
The default version is `0.1.0-dev`; an explicit version may follow `--native`.

Use the resulting archive with the controller:

```sh
cargo install --locked --path crates/cargo-refresh -j 4
export CARGO_REFRESH_DIST="$PWD/dist/cargo-refresh-0.1.0-dev-b940084d7eb6a299eb4bfeb8e34901bc051e7ac4-x86_64-linux.tar.zst"
cd /path/to/application
cargo refresh run --bin your-binary
```

Keep the adjacent `.sha256` file. To replace the committed portable bundle after
reviewing and testing a portable source build, copy both files from `dist/` into `bundle/`.
The bundle is intentionally committed so trying the project does not require
rustc-dev or a full compiler build.

## Portable builds

`scripts/build-bundle.sh --portable` (also the default mode without `--native`) needs
user-installed cargo-zigbuild and Zig. It targets GNU x86_64/glibc 2.28 and rejects
newer GLIBC symbol imports. Set `REFRESH_ZIG` to a Zig executable if necessary.
Run the resulting archive on the intended older distribution before claiming
compatibility. The committed archive was built with this portable mode.

```sh
RUSTUP_TOOLCHAIN=1.99.0 cargo install cargo-zigbuild --locked -j 4
scripts/build-bundle.sh --portable 0.1.0-dev
```

## Cache and integrity

Support is cached under
`${XDG_CACHE_HOME:-$HOME/.cache}/cargo-refresh/<rustc-commit>-<archive-sha256>`.
`--from-archive /absolute/path/file.tar.zst` or `CARGO_REFRESH_DIST` always selects
that exact archive, even if another bundle for the same rustc is already cached.
Different bundles coexist; a private atomic `.<rustc-commit>.current` file remembers
the last selection for starts without an archive. Existing rustc-only caches are
preserved and checked when no newer selection exists.
`CARGO_REFRESH_SUPPORT` selects a complete unpacked directory when no archive is supplied.
The controller verifies the archive checksum, safe member names, payload hashes,
VERSION identities, ABI facts, executable permissions, and runtime dependencies
before starting a compiler session. VERSION must declare the controller's
`support_protocol`; startup also queries the actual compiler and agent and checks
the application's agent PING. Missing or mismatched protocols produce an actionable
reinstall diagnostic and select normal Cargo before injecting the agent.
A per-commit lock and atomic rename protect installation. Cache hits are rehashed.
These checks establish integrity of trusted local inputs, not publisher signatures.

An archive contains `rustd`, `cargo-rustd`, `rustd-rustc`, `librefresh.so`,
`abi-facts.bundle`, `VERSION`, `RUNTIME.txt`, licenses and `SHA256SUMS`.
`RUNTIME.txt` records actual shared-library and GLIBC symbol requirements.
`VERSION` records the compiler/frontend source pins, toolchain identity and support
protocol. Bump the support protocol in controller, agent, compiler capabilities and
release metadata together whenever that contract changes.
Its product revision is the archive build revision and can precede the commit
that adds the finished archive.
The controller stages the ABI bundle under the compiler's expected commit filename.

## Runtime sockets

Agent, daemon and worker sockets use unique private session paths under
`$XDG_RUNTIME_DIR/cargo-refresh` when the runtime path is short enough, otherwise
`/tmp/cargo-refresh-<uid>`. Runtime directories must belong to the current user
with mode `0700`; unsafe directories and symlinks are refused. Socket paths are
checked against the Unix socket byte limit before startup, including the worker's
temporary bind suffix. Project paths and `TMPDIR` can be long.

Sessions remove their namespaces on shutdown; later starts clean namespaces left
by dead controller PIDs. Compiler state stays on disk through a short private
parent alias, and executable snapshots still follow `TMPDIR`. Agent startup errors
are reported immediately to the controller as well as stderr.

## Troubleshooting

- Missing rustc-dev during a source build: `rustup component add rustc-dev --toolchain 1.99.0`.
- Missing LLVM link library during a source build: `rustup component add llvm-tools-preview --toolchain 1.99.0`.
- Compiler cannot load standard library source: `rustup component add rust-src --toolchain 1.99.0`.
- glibc older than 2.28: build and qualify support for that host.
- Incompatible cached support: pass `--from-archive /path/to/matching-bundle.tar.zst`
  or set `CARGO_REFRESH_DIST`; a different archive gets its own cache entry.
- Missing archive: restore `bundle/`, or build a new archive from `rustd/`.
- Unsupported toolchain, unqualified Cargo config, wrapper or CPU/target/profile flags:
  read the refusal reason; Cargo mode is the conservative fallback.
- Low memory: close heavy programs or narrow ownership with `--edit`; do not force
  estimates down merely to make admission succeed.
- A workspace with several packages needs `--package name` as well as `--bin name`.

`scripts/new-toolchain.sh <toolchain>` is a manual candidate ABI check. Its
`--dry-run` still extracts facts and checks TypeIds/personality but skips product
tests. Neither form qualifies a new toolchain automatically.
