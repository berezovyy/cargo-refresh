# Toolchain support installation design

This is the M5 packaging contract for controller integration. Download/cache behavior
below is planned, not implemented by these scripts. Releases are built by hand;
no upload or CI/CD is configured.

The controller is installed separately with `cargo install cargo-refresh`. Toolchain
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

Proposed lookup/download/verification sequence:

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

Proposed fail-closed message:

```text
[refresh] cargo-refresh has no qualified support for rustc <version> (<commit>).
[refresh] Running normal cargo builds + restart (no live patching).
[refresh] Supported toolchain: 1.99.0, x86_64-unknown-linux-gnu, default CPU/features.
[refresh] See docs/install-layout.md for toolchain qualification.
```

For a corrupt or mismatched cached bundle, add the concrete reason and path, then
use the same stock fallback. Never launch rustd to discover whether a mismatch
works. An unavailable runtime dependency should say:

```text
[refresh] Live patching needs libunwind.so.8 and libunwind-x86_64.so.8.
[refresh] Install your distro's libunwind runtime package (Debian/Ubuntu: libunwind8).
[refresh] Running normal cargo builds + restart (no live patching).
```

Before warming, compare `/proc/meminfo` MemAvailable with the project's estimated
warm-up peak plus a 2 GiB reserve. A missing estimate is not evidence that the
project fits; use a conservative estimate. Proposed warning:

```text
[refresh] Live patching needs an estimated <peak> GiB plus 2 GiB reserve; <available> GiB is available.
[refresh] Continuing in cargo mode. Close other large processes and retry to enable live patching.
```

The development scripts independently enforce a 6 GiB available-memory floor,
stopping only their own process groups. Runtime controller budgeting is a separate
integration task. Do not describe the default line-table compiler as meeting a
6 GiB startup peak: M1 qualification exceeded that target.

## Build host compatibility and qualification

Run `scripts/release.sh <version>` locally. It builds rustd and both shims on the
`refresh/m1` branch using rustc 1.99.0, and the production agent via
`make -C agent release`. It attempts static libunwind only if local archives are
available and linkable; otherwise it uses the runtime libraries above. See the
artifact's `RUNTIME.txt` for measured GLIBC symbol versions and DT_NEEDED entries
for each executable/library. The whole package's glibc floor can exceed the
agent's. This host build does not establish an oldest-supported distro: rebuild
in an agreed older-glibc environment and test loading there before publishing.

`REFRESH_RESEARCH_ROOT`, `REFRESH_RUSTD_SOURCE` and `REFRESH_LANE` override local
source/lane locations. `.release-work/` holds warm build targets and qualification
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
