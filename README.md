# cargo-refresh

**Save a Rust file. See the change in your running program. Keep its state.**

`cargo-refresh` is Fast Refresh for Rust. Run your program through it instead of
`cargo run`. When you save a `.rs` file, the changed functions are compiled and
patched into the process that is already running. A successful live patch keeps
open connections and in-memory state. Changes that require a restart follow the
fallback ladder below.

> **Status: pre-release, integration in progress.** This README is a release
> skeleton, not a claim that every command and path below is qualified. Performance,
> memory and correctness results will come from the final release benchmark.

The M2 controller now builds and supervises applications, checks saves, and routes changes through live patching, restart, or stock Cargo. See [controller usage and current scope](docs/controller.md); run `./scripts/test.sh` for the hand-run checks.

<!-- BENCH:LIVE_SAVE_VISIBLE_HEADLINE -->
<!-- BENCH:DEMO_GIF_AND_DURATION -->

## Install and first run

Planned release workflow, once the package and toolchain artifacts are published:

```sh
cargo install cargo-refresh
cd your-project
cargo refresh run --bin my-server
```

For source builds and current checks, see [CONTRIBUTING.md](CONTRIBUTING.md).
Toolchain artifact downloads are a controller integration task; see the
[installation contract](docs/install-layout.md).

## What happens when you save

| Your edit | Path |
|---|---|
| Admitted function bodies, helpers, imports, comments and inserted lines inside the editing frontier | Live patch, state kept |
| Struct or enum layout, `async fn` changes | Automatic restart; in-memory state is lost |
| `Cargo.toml`, `build.rs`, out-of-frontier crates, unsupported compiler features | Normal Cargo build and restart |
| Syntax or type error | Diagnostic; keep the previous working program |
| Borrow checker error after an optimistic patch | Diagnostic and patch rollback; use strict mode to wait for checking |

Admission and compiler refusals may choose a slower path. Correctness takes
priority over live delivery. A valid save must eventually produce new behavior;
an invalid save must produce a diagnostic.

## Requirements

GNU/Linux on the supported Intel/AMD architecture. Toolchain support is tied to
the **exact rustc commit**, not just its version name. Unsupported toolchains must
use normal Cargo builds and restarts.

<!-- BENCH:SUPPORTED_TARGET_AND_RUSTC_VERSIONS -->
<!-- BENCH:RAM_GUIDANCE_FROM_FINAL_APP_RUNS -->
<!-- BENCH:GLIBC_AND_AGENT_RUNTIME_REQUIREMENTS -->

Current source-build dependencies and the libunwind runtime fallback are documented
in [install-layout.md](docs/install-layout.md). Oldest-distro compatibility still
needs qualification.

## Tips for speed

Keep frequently edited files small: the warm compiler's work can depend on the
edited file. The editing frontier is the set of crates owned by rustd; edits
outside it use Cargo. Use `--edit <crate>` to expand it when needed, accepting the
extra warm-up and memory cost.

<!-- BENCH:SMALL_FILE_COMPARISON -->

Nightly Cargo's checksum-freshness experiment may help identical-content rebuilds
on the Cargo path. Consult the exact toolchain's Cargo documentation before using
`CARGO_UNSTABLE_CHECKSUM_FRESHNESS` or `CARGO_BUILD_FINGERPRINT=content`; this is not a
reason to enable unqualified live patching on nightly.

## Apps that cache rendered output

A successful code patch does not automatically invalidate a template or page cache.
The planned after-patch hook from the prototype is:

```rust,ignore
fast_refresh::on_patch(|| cache.clear());
```

The hook is not yet shipped as a public helper crate here. Once integrated, use it
to clear Tera/MiniJinja template caches or trigger an SSG rebuild. A code patch alone
is insufficient evidence: probe the rendered page and compare it with stock.

## Known limitations

- Optimistic patches execute before the borrow checker confirms them. Strict mode
  is intended to wait for checking.
- Async edits restart. Layout changes restart and lose in-memory state.
- Patched functions use Cranelift; runtime performance differs from LLVM output.
- Retained old code supports old closures and pointers but consumes memory.
- Tracing callsite line numbers may lag after edits; release qualification must
  confirm the precise behavior.
- Download/install behavior, the cache hook and full live semantics qualification
  remain integration work. See contributor documentation before relying on them.

<!-- BENCH:BORROW_CHECK_WINDOW -->
<!-- BENCH:PATCHED_CODE_RUNTIME_RATIO -->
<!-- BENCH:FINAL_QUALIFIED_LIMITATIONS -->

## Troubleshooting

If an edit does not appear, check whether the app caches its output, then inspect
its selected path and diagnostic. If it fell back to Cargo, check the editing
frontier and refusal reason. If memory is high, narrow the frontier or use stock
mode. Proposed diagnostic commands for the completed controller:

```sh
cargo refresh status
cargo refresh run --events events.jsonl
```

Their implementation status and current alternatives are described in
[controller.md](docs/controller.md). A low-memory or unsupported-toolchain warning
must select stock mode before warming rustd.

## Release numbers

All cells below are deliberately unfilled. Use the final benchmark's generated
output and committed raw rows, not research-prototype figures.

| Project | Size | Cargo build + restart | Live patch | Paired ratio |
|---|---|---|---|---|
| axum web | <!-- BENCH:WEB_SIZE --> | <!-- BENCH:WEB_STOCK --> | <!-- BENCH:WEB_LIVE --> | <!-- BENCH:WEB_RATIO --> |
| mdBook | <!-- BENCH:MDBOOK_SIZE --> | <!-- BENCH:MDBOOK_STOCK --> | <!-- BENCH:MDBOOK_LIVE --> | <!-- BENCH:MDBOOK_RATIO --> |
| rust-analyzer | <!-- BENCH:RA_SIZE --> | <!-- BENCH:RA_STOCK --> | <!-- BENCH:RA_LIVE --> | <!-- BENCH:RA_RATIO --> |
| Bevy | <!-- BENCH:BEVY_SIZE --> | <!-- BENCH:BEVY_STOCK --> | <!-- BENCH:BEVY_LIVE --> | <!-- BENCH:BEVY_RATIO --> |
| Conduit | <!-- BENCH:CONDUIT_SIZE --> | <!-- BENCH:CONDUIT_STOCK --> | <!-- BENCH:CONDUIT_LIVE --> | <!-- BENCH:CONDUIT_RATIO --> |

### Real editing sessions

| App | Saves | Went live | Live median and tail | Valid saves delivered | Matched stock |
|---|---|---|---|---|---|
| axum web | <!-- BENCH:WEB_SAVES --> | <!-- BENCH:WEB_LIVE_SHARE --> | <!-- BENCH:WEB_SESSION_LATENCY --> | <!-- BENCH:WEB_DELIVERED --> | <!-- BENCH:WEB_STOCK_MATCH --> |
| mdBook | <!-- BENCH:MDBOOK_SAVES --> | <!-- BENCH:MDBOOK_LIVE_SHARE --> | <!-- BENCH:MDBOOK_SESSION_LATENCY --> | <!-- BENCH:MDBOOK_DELIVERED --> | <!-- BENCH:MDBOOK_STOCK_MATCH --> |
| rust-analyzer | <!-- BENCH:RA_SAVES --> | <!-- BENCH:RA_LIVE_SHARE --> | <!-- BENCH:RA_SESSION_LATENCY --> | <!-- BENCH:RA_DELIVERED --> | <!-- BENCH:RA_STOCK_MATCH --> |
| Bevy | <!-- BENCH:BEVY_SAVES --> | <!-- BENCH:BEVY_LIVE_SHARE --> | <!-- BENCH:BEVY_SESSION_LATENCY --> | <!-- BENCH:BEVY_DELIVERED --> | <!-- BENCH:BEVY_STOCK_MATCH --> |
| Conduit | <!-- BENCH:CONDUIT_SAVES --> | <!-- BENCH:CONDUIT_LIVE_SHARE --> | <!-- BENCH:CONDUIT_SESSION_LATENCY --> | <!-- BENCH:CONDUIT_DELIVERED --> | <!-- BENCH:CONDUIT_STOCK_MATCH --> |

<!-- BENCH:RESTART_LATENCY -->
<!-- BENCH:GENERATED_PROGRAM_STOCK_MATCHES -->
<!-- BENCH:PANIC_MATRIX_STOCK_MATCHES -->
<!-- BENCH:LONG_SESSION_DELIVERY_AND_MEMORY_GROWTH -->
<!-- BENCH:KEEP_ALIVE_REQUEST_COUNTS_AND_ERRORS -->
<!-- BENCH:MACHINE_CPU_RAM_OS_RUSTC_DATE_FLAGS_LOAD_AND_REPETITIONS -->

## Why rebuilds are slow

A body edit can repeat macro expansion, lowering and analysis for a crate and its
dependents. The warm compiler aims to avoid repeating that work for unrelated code.
Which phase dominates depends on the project and build profile.

<!-- BENCH:FRONTEND_LINKING_CODEGEN_BREAKDOWN -->
<!-- BENCH:DEPENDENT_CRATE_REBUILD_COMPARISON -->

See [Why Rust rebuilds are slow](docs/why-rust-rebuilds-are-slow.md) for research
context; its prototype figures are not release benchmark results.

## How it works

```text
save → controller → rustd: incremental rust-analyzer frontend → Cranelift
                         ↓ new code pages, relocations and entry jumps
                  librefresh.so in the running application
                         pause threads → install safely → resume

controller → background cargo check → diagnostic/rollback on rejection
           → restart or stock Cargo when a live patch is refused
```

Old code stays mapped so previously created closures and function pointers can
continue using it. The controller owns admission, supervision and fallback;
rustd and the agent must refuse changes they cannot safely support.

## Reproduce the numbers

The planned release benchmark applies scripted edits, probes the running program
until the expected behavior appears, interleaves refresh with stock rebuild and
restart, and compares complete observable outputs. Raw release rows belong in
`bench/results/`. The proposed `scripts/bench.sh` driver is not present yet; see
[CONTRIBUTING.md](CONTRIBUTING.md) for current hand-run checks and measurement rules.

<!-- BENCH:PROBE_INTERVAL_AND_REPETITION_POLICY -->
<!-- BENCH:RAW_RELEASE_RESULTS_LINK -->

## Credits and license

Built on [DioxusLabs/rustd](https://github.com/DioxusLabs/rustd),
[rust-analyzer](https://github.com/rust-lang/rust-analyzer), and Cranelift.

<!-- BENCH:RESEARCH_EXPERIMENT_COUNT -->

Dual licensed under MIT or the Apache License; see [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE). Release artifacts include rustd and rust-analyzer
license texts. Dependency attribution review remains a release gate.
