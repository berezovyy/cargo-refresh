# Toolchain support installation and portability

This is the M5 packaging contract. Verified local installation and cache selection
are implemented; public downloading remains planned. Releases are built by hand;
no upload or CI/CD is configured. See the implemented first-run section below.

## Source install

Public package, fork and artifact URLs are not published yet. In the commands
below, replace the two angle-bracket placeholders with the repositories supplied
by the release maintainer; the rustd URL must contain the qualified `refresh/m1`
branch and its pinned submodule commits. The fork's `.gitmodules` must point to
a repository that actually serves the patched rust-analyzer commit; upstream's
URL alone does not publish local submodule patches. Do not substitute upstream's
default branch. For an offline local clone test, local repository paths can replace those
URLs and the submodule URL can be overridden with `git -c
submodule.vendor/rust-analyzer.url=/path/to/local/submodule ...`.

Install system prerequisites first: Git, curl and CA certificates, a C/C++ build
toolchain and linker, make, Python 3, binutils (`objdump`, `nm`), GNU tar, and zstd.
Use your distribution's packages or user-space tools. No sudo is invoked by these
scripts. Rustup is installed into your own HOME:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o rustup-init.sh
sh rustup-init.sh -y --no-modify-path --profile minimal --default-toolchain 1.99.0
export PATH="$HOME/.cargo/bin:$PATH"
export RUSTUP_TOOLCHAIN=1.99.0
rustup component add rustc-dev rust-src --toolchain 1.99.0

git clone <cargo-refresh-repository-url> cargo-refresh
git clone --branch refresh/m1 --recurse-submodules <qualified-rustd-fork-url> rustd
cd cargo-refresh
cargo install --locked --path crates/cargo-refresh -j 4
scripts/release.sh --native 0.1.0-dev
export CARGO_REFRESH_DIST="$(pwd)/dist/cargo-refresh-0.1.0-dev-b940084d7eb6a299eb4bfeb8e34901bc051e7ac4-x86_64-linux.tar.zst"
cd /path/to/your-project
cargo refresh run --bin your-binary
```

A plain `cargo new` app prints once and exits: replace its main with a long-running
loop or server to observe live changes. Save a non-inlined function body while the
program runs. Struct layout edits restart the process and lose its in-memory state.
Invalid saves report diagnostics while keeping the last working program; fixing
the source resumes processing. Workspaces with multiple default packages also
need `--package <package-name>` to select the package containing the binary.

The source release build downloads locked dependencies as needed. It includes the
ABI helper, qualification fixture, historical qualified bundle and raw reference
facts in this repository; it does not need a research checkout or developer cache.
The native artifact supports the build host's glibc. Producing a distributable
old-glibc artifact uses the portable recipe below and still requires qualification.
`rustc-dev` is needed to build the ABI helper; `rust-src` is needed by rustd when
loading the standard library. Both must match the selected toolchain. Contributor
formatting/tests additionally use `rustfmt` and `clippy`.

`REFRESH_RUSTD_SOURCE` overrides the default sibling `../rustd` source directory.
`REFRESH_LANE` optionally supplies a shared-machine resource-lane script; no lane
script is needed on an ordinary user's machine. Builds use four jobs and a RAM
guard. On shared research machines, set the lane override or run the complete
build under the assigned rustd/build lanes. Scratch and output live in
`.release-work/` and `dist/`; set TMPDIR to an on-disk private directory when /tmp
is RAM-backed. A cold source build is not a claim of an under-five-minute install.

The controller is installed separately from source as above (a future published
package will use `cargo install cargo-refresh`). Toolchain
support is cached beneath `${XDG_CACHE_HOME:-$HOME/.cache}/cargo-refresh/<rustc-commit>/`:

```text
rustd
cargo-rustd
rustd-rustc
librefresh.so
abi-facts.bundle
VERSION
SHA256SUMS
RUNTIME.txt
licenses/
```

The two compiler shims are siblings of rustd. Preserve their executable permissions
and use this absolute directory when launching compiler capture; do not mix shims
from PATH with a different rustd release. `abi-facts.bundle` is schema-1 JSON, named
independently of its internal `commit_hash`. Current rustd reads
`$RUSTD_ABI_FACTS_DIR/<commit>.json`; the controller must stage a verified copy of
`abi-facts.bundle` under that name in a private per-session bundle directory and
set `RUSTD_ABI_FACTS_DIR` to its absolute path. The archive/cache file remains
`abi-facts.bundle`, so this adapter is required before compiler startup.

`VERSION` is a newline-separated key/value file: `cargo_refresh`,
`cargo_refresh_commit`, `rustd_commit`, `ra_commit`, `rustc_commit`, `target`, and
`agent_static_libunwind`. Toolchain commits are full hashes. The release archive is
`cargo-refresh-<version>-<rustc-commit>-x86_64-linux.tar.zst`. Its contents are flat,
ready for extraction into a private staging directory. `SHA256SUMS` covers every
payload file except itself; the adjacent `.tar.zst.sha256` covers the archive.

On every start, the controller should run `rustc -vV` in the target workspace with
the same toolchain environment used for Cargo. Parse `commit-hash`, `release`, and
`host`; compare the full commit and target with both VERSION and the bundle. A
matching version label alone is insufficient. Only the qualified default GNU
Linux x86-64 target/CPU/features and supported packaging flags may use rustd.
Unknown target/ABI overrides must select stock mode.

Cache/install contract and future download sequence:

1. Check an explicit local support directory, then the commit-keyed cache. Validate
   identities, required members, permissions and all payload hashes before use.
2. If absent, look up the exact `(controller version, rustc commit, target)` in a
   trusted release index. The public hosting location is still a release decision;
   do not invent a production URL or substitute the newest available bundle.
3. Fetch the archive and its expected SHA-256 over authenticated HTTPS. A checksum
   from the same compromised origin is not a signature; the release index is a
   trust boundary. Verify before extracting. Reject absolute/parent paths,
   symlinks, unexpected members and excessive expansion; extract into a private
   directory beside the final cache entry. Verify internal sums and identities.
4. Check shared-library dependencies and memory before launching any compiler or
   loading the agent. Rename a completely verified staging directory atomically
   under a per-commit install lock; never expose a partial installation. Keep failed
   downloads out of the active cache. Offline/missing/corrupt support selects cargo.

Fail-closed message:

```text
[refresh] cargo-refresh has no qualified support for rustc <version> (<commit>).
[refresh] Running normal cargo builds + restart (no live patching).
[refresh] Supported toolchain: 1.99.0, x86_64-unknown-linux-gnu, default CPU/features.
[refresh] See docs/install-layout.md for toolchain qualification.
```

For a corrupt or mismatched cached bundle, add the concrete reason and path, then
use the same stock fallback. Never launch rustd to discover whether a mismatch
works. Check the actual artifact requirements rather than assuming the research
agent's dependencies: the current production hybrid agent compiles the libunwind
CFI walker only in testing builds, and this host's as-needed linker drops
libunwind from its DT_NEEDED list. It dynamically opens `libgcc_s.so.1` for frame
registration, which must be available for patch unwinding. If an artifact does
require libunwind and it is unavailable, the diagnostic should say:

```text
[refresh] Live patching needs libunwind.so.8 and libunwind-x86_64.so.8.
[refresh] Install your distro's libunwind runtime package (Debian/Ubuntu: libunwind8).
[refresh] Running normal cargo builds + restart (no live patching).
```

Before warming, compare the project estimate plus a **1 GiB reserve** with the
minimum of host MemAvailable and cgroup-v2 ancestor headroom. The controller
reports the estimate and effective available memory on startup, selects stock
Cargo when the compiler does not fit, and retries after stable recovery.
See [memory.md](memory.md) for the project model, calibration limitations, runtime
pressure guard, hysteresis and memory-warning text. Research session guards
independently retain their 6 GiB host safety floor.

## Build host compatibility and qualification

Run `scripts/release.sh <version>` locally. The default portable build uses
cargo-zigbuild targeting GNU x86-64/glibc 2.28 with rustc 1.99.0 and builds the
production agent with Zig's old-glibc C headers/libraries. `--native` is an explicit
host-only diagnostic alternative. The release rejects symbol imports above 2.28
in portable mode, including weak imports. `RUNTIME.txt` records measured GLIBC
symbol versions and DT_NEEDED entries for every artifact.

The production hybrid agent does not link its test-only libunwind CFI walker.
It dynamically opens `libgcc_s.so.1` for frame registration. Local Rocky Linux 8
(glibc 2.28) checks ran the portable compiler, loaded the production agent, and
completed archive installation plus one live save with PID/counter continuity
and a matching stock oracle. This is container evidence on the host kernel;
full release/app qualification remains a manual gate.

`REFRESH_RUSTD_SOURCE` and optional `REFRESH_LANE` override source/lane locations. `.release-work/` holds warm build targets and qualification
logs; `dist/` holds local archives. They are git-ignored. Do not remove another
worker's targets. Requires local zstd, GNU tar, binutils, C compiler, Python, make,
Rust 1.99.0 and matching rustc-dev. No system/global installation is performed.

Run `scripts/new-toolchain.sh <toolchain>` by hand for a candidate compiler.
`--dry-run` still rebuilds the helper, extracts facts, checks SIMD/personality and
runs a preliminary stock TypeId matrix, but skips the product tests. Full mode
runs `scripts/test.sh` with the candidate toolchain. Neither mode grants support;
review the printed manual checklist, especially live-vs-stock TypeIds, intrinsic
refusals, actual live edits, layout restarts and differential tests. Future rustc
helper API changes may require adapter work; compilation failure stops the script.

For a retained reference bundle, extraction must match all ABI fields and raw
facts. Only a fixed allowlist of fixture-local `domain::` TypeIds and reference types
whose bound-region identity belongs to the probe crate may differ. Cargo changes
local crate identities with source location. The script verifies the
historical raw stream's SHA-256 and every other field, retains the rebuilt raw
stream and truthful candidate digest, and packages the *unchanged historical
qualified bundle*. `reference-comparison.json` records that distinction. It never
rewrites the raw hash to imply a byte-identical re-extraction. Unknown differences
fail closed. A new compiler's generated bundle stays a candidate until the manual
matrix is complete.

## Implemented local first run (M5 steps 1–3)

The controller now detects `rustc -vV`, checks the full qualified identity, and
validates every cached payload before compiler startup. `REFRESH_RUSTD` and a
`rustd` found on PATH do not bypass qualification. The sibling shims come from the
same verified support directory. `librefresh.so` is selected from that directory;
stock-only mode does not inject an agent or start/reset rustd.

Install a locally built candidate with either:

```sh
cargo refresh run --from-archive /absolute/path/cargo-refresh-0.1.0-dev-<commit>-x86_64-linux.tar.zst
CARGO_REFRESH_DIST=/absolute/path/cargo-refresh-0.1.0-dev-<commit>-x86_64-linux.tar.zst cargo refresh run
```

Keep the adjacent `.tar.zst.sha256` file. It must name the archive basename and
contain one SHA-256 line. These are trusted local inputs; this is integrity
checking, not a signature or public distribution trust system. URL downloads are
not implemented. `CARGO_REFRESH_SUPPORT` can select an explicit unpacked support
directory, with the same mandatory validation as the cache.

Installation rejects unknown members, links, traversal, duplicates, excessive
compressed/expanded size, incomplete or incorrect checksums, and mismatched
VERSION/ABI identities. A per-commit lock protects a private staging directory
and atomic rename. Existing corrupt entries select stock mode; remove an entry
explicitly before retrying installation. Cache hits are rehashed on every start.
The controller stages `<commit>.json` in an owned private temporary directory.

Unknown toolchains, target/CPU/profile/compiler-wrapper overrides, nonempty Cargo
configuration files, unavailable runtime libraries, and insufficient memory select
stock Cargo. Nonempty Cargo config is conservatively refused even when it contains
only harmless settings; qualification of individual settings is future work. Stock
builds preserve the active toolchain and user Rust flags. Startup uses an estimate
from owned Rust LOC and dependency count, plus 1 GiB reserve; the estimate is
shown in first-run output. An operator with a measured whole-session peak can set
`CARGO_REFRESH_WARM_PEAK_BYTES`. [Memory budgeting](memory.md) describes cgroup
accounting, runtime degradation and automatic recovery.

Portable release builds are now the default:

```sh
# Install user-space prerequisites; do not change global toolchain/config.
RUSTUP_TOOLCHAIN=1.99.0 cargo install cargo-zigbuild --locked -j 4
# Put Zig on PATH or set REFRESH_ZIG to its absolute executable path.
scripts/release.sh --portable 0.1.0-dev
# Host-only diagnostic build (does not assert old-glibc compatibility):
scripts/release.sh --native 0.1.0-dev
```

Portable builds target `x86_64-unknown-linux-gnu.2.28` through cargo-zigbuild and
compile the production agent with `zig cc -target x86_64-linux-gnu.2.28`. The
release fails if any artifact imports a GLIBC symbol newer than 2.28, including
weak imports. The production agent needs libc and dynamically opens libgcc_s;
it does not link the test-only libunwind walker. `RUNTIME.txt` reports actual
imports; a symbol check alone does not establish runtime qualification. Run the
release payload on an old-glibc distro before accepting it.

Hand-run integration (requires the research resource lane):

```sh
CARGO_REFRESH_TEST_ARCHIVE=/absolute/path/release.tar.zst \
  RUSTUP_TOOLCHAIN=1.99.0 cargo test -p cargo-refresh --test install -- --ignored --nocapture
```

Or use `scripts/integration-install.py` with `--controller`, `--archive`, and a new
`--work` directory under the rustd lane. It installs into a temporary HOME, applies
one live save, checks PID and counter continuity, and compares the edited value
with an independent stock build. Its `unsupported`, `corrupt`, `low-memory`, and
`override` modes exercise stock decisions without starting rustd.

Development profiles may set `debug`, `split-debuginfo`, `strip`, `incremental`,
`codegen-units`, `debug-assertions`, and `overflow-checks`, including package/build
overrides and inline/quoted TOML spellings. The first five affect artifacts or
compilation strategy. Rustd propagates the assertion/overflow flags into crate
configuration and codegen; the four-combination stock/live regression is in
`tests/profile-admission/`. Other keys select stock mode with the exact key in
the reason, including `opt-level`, `panic`, and `lto`. Release-only profiles are
allowed. Legacy `REFRESH_RUSTD_ENV` and inherited
`RUSTD_*` tuning select stock mode; they cannot replace the verified compiler,
ABI directory, or target behind the detector's back.


The controller watches rustc dep-info inputs as well as source roots. Changes to
embedded text/bytes, including root-level or external inputs, use stock Cargo
and recapture. Changing an include invocation also recaptures its new inputs.
Edits to an owned crate with a transitive caller outside the compiler frontier
use stock Cargo: restarting a partial binary cannot update excluded generic,
inlined, or const-evaluated copies. Add the callers with `--edit` to own the
complete dependency closure.

## Troubleshooting

- **Missing rustc-dev:** install `rustup component add rustc-dev --toolchain 1.99.0`.
  Setting a different global default does not supply the matching compiler library.
- **Compiler cannot load std:** install `rustup component add rust-src --toolchain 1.99.0`.
- **Missing source checkout or submodule:** clone the qualified fork beside this repo
  as `rustd`, or set `REFRESH_RUSTD_SOURCE`; initialize its pinned submodules.
- **Missing release prerequisite:** install the named system tool. `--native` avoids
  the extra Zig/cargo-zigbuild dependencies needed for portable artifacts.
- **Dependency unavailable:** allow network access for the initial build; scripts
  no longer force offline mode. Cargo's explicit offline/proxy/registry settings
  still apply. Refresh currently refuses nonempty Cargo config for live mode.
- **Support unavailable:** keep the archive plus its adjacent `.sha256`, and pass
  `--from-archive /absolute/path/to/archive.tar.zst`. Read the concrete rejection
  reason; mismatched/corrupt support deliberately selects stock mode.
- **Low memory:** close other heavy programs, reduce the editing frontier or use
  stock mode. Do not override estimates just to force a compiler into low RAM.
