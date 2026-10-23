# M4 runtime semantics fixtures

These 20 standalone packages implement the M4 §5 matrix. They specify desired
live behavior and verify stock Rust behavior. They do **not** qualify live
patching yet. Each case has Rust 1.99.0, `[profile.dev] debug = 0`, a lockfile,
`edits.toml`, recorded `expected.json`, a README, and an executable `stock.sh`.

Run from the product repository:

```sh
python3 tests/semantics/run.py --mode stock
python3 tests/semantics/run.py --mode stock --case 03 --case 17
python3 tests/semantics/run.py --mode stock --offline --results /tmp/semantics.json
# Intentionally update recorded observations after changing a fixture:
python3 tests/semantics/run.py --mode stock --record --case 03
# Equivalent per-case wrapper, usable from any directory:
tests/semantics/03-new-static/stock.sh
```

Python 3.11+, Rust/Cargo 1.99.0, and Linux are required. Case 20 also needs `cc`
and `ar`. First use downloads locked dependencies and compiles SQLx, Tokio,
tracing, and futures. `--offline` requires those dependencies to be cached.
SQLite is bundled by SQLx; no running database server or SQLx CLI is needed.
The SQLx fixture uses real `query!` expansions with both query versions'
offline metadata and executes each query against in-memory SQLite.

The suite remains a manual command in this phase: first-use dependency builds
are not guaranteed to fit the two-minute `scripts/test.sh` budget. No benchmark
or refresh latency claim is made by this suite.

## Probe protocol and edit contract

The app stays running, reading lines on stdin. `probe` returns exactly one line
starting `probe=`; `quit` exits. Other commands fail. The runner sends each
probe separately, consumes the complete response, and compares complete lines.
It does not use periodic output, sleep for startup, or accept matching substrings.

`edits.toml` schema 1 contains:

- `base_probes`: ordered responses from the initial process. These probes also
  initialize/advance state for a later live run.
- `v0_1`: allowed outcomes from M4. `works` means live, `restart` means explicit
  refusal followed by restart, and `stock` means stock build plus restart.
  Unknown/conditional rows retain their alternatives and explain them locally.
- `state_rule`: how retained live state differs from a fresh process.
- `environment` and `unset_environment`: deterministic initial build inputs.
- `steps`: cumulative edits, each with a name, ordered literal `edits` (`file`,
  `find`, `replace`), optional compiler `environment` updates, and ordered
  `stock_after_restart` and `live_expected` probe responses.
- Case 10 also has `frontiers`: two required future refresh configurations,
  one covering all callers and one with a caller outside the frontier.

Every find must match exactly once. All edits in one step form one logical
save; a future watcher driver should batch them before waiting for an outcome.
Paths cannot escape the copied case. Each step starts from the previous step's
source, not from the original. New-static case 03 therefore tests introduction
**and a later edit of the same storage**.

The stock oracle builds and starts the base, probes it twice, then stops it.
After each edit it does a stock build and starts a new process, probes twice,
and stops it. The live contract instead keeps the original process through all
edits and probes, preserving all earlier side effects. For example, case 01 is
`11,12` before the edit, `20,30` after a stock restart, but `22,32` after a live
patch. A restart may never be counted as live success.

`expected.json` separates `stock-after-restart` observations from
`live-expected` assertions. Normal runs require it to match the TOML contract,
then reproduce all stock observations. `--record` first verifies actual stock
output against the TOML declarations; only a successful case updates its JSON
and lockfile. This prevents accidentally blessing arbitrary output. Live
expectations are specifications, always labeled UNMEASURED.

## Refresh adapter contract (not implemented)

`--mode refresh` currently exits unsuccessfully with an explicit message; it
never invokes stock and reports that as refresh success. The future adapter
must:

1. Copy the case as the stock runner does and invoke `cargo refresh run` from
   that package, preserving separate app stdin/stdout and controller events.
2. Consume base probes before applying edits. Keep the process and its workers,
   closures, pointers, initialized lazy cells, and live Drop values alive.
3. Apply each cumulative step, wait for its explicit outcome event, and assert
   that outcome is allowed by `v0_1` (and the selected frontier configuration).
   Wait for applied-generation completion, not a timer or source-write alone.
4. For a live event assert every `live-expected` line, unchanged process identity,
   and no hidden restart. For restart or stock assert every
   `stock-after-restart` line and a real restart. An unannounced refusal, wrong
   output, crash, timeout, or unsupported input silently ignored is a failure.
5. For stock + build.rs assert build metadata was recaptured. For include_str!
   ensure the non-Rust file save actually triggers delivery. For env!/option_env!
   supply the new environment through a controller/session reconfiguration
   mechanism; changing the runner's environment cannot change a running
   controller's environment. This mechanism is pending integration.
6. Run case 10 twice: all three crates inside the frontier => live and all four
   instantiations updated; caller outside => stock rebuild and restart.
7. Record actual vtable routing from case 08, including retained and fresh trait
   objects, saved method pointer, and direct call. Current target is method-entry
   redirects reaching new code in all four paths. Old vtable behavior must be
   documented and refused if that contract cannot be met; it is not an implicit
   passing alternative.
8. Record raw and formatted tracing locations from case 13. M3 §6 permits a
   documented cosmetic cached-line limitation under a permissive policy; this
   fixture specifies correct lines. If qualification finds stale formatting,
   record it explicitly and decide the policy, rather than silently changing
   expectations to accept it.

Rows marked works/refuse require qualification to determine which outcome is
supported. Stock success alone settles none of TLS support, retained vtables,
closure retention, live unwinding, generation static allocation, or the inline
recompilation dependency graph.

## Workspace, resources, and evidence

Cases are independent Cargo workspaces, not members of the product workspace.
Case 11 uses dev `opt-level = 1` plus `#[inline(always)]` to exercise inlined
callers; all other cases use ordinary unoptimized dev builds.

The runner builds disposable source copies beneath `.work/sources/` and uses a
shared warm `.work/target/`. Original case sources stay pristine. Staged files get fresh mtimes so Cargo
cannot reuse the prior edited binary when restoring a base source. All builds run
as `nice -n 19 cargo build -j 4` with an explicit toolchain and normal runs use
`--locked`. Runs take a nonblocking exclusive lock so they cannot corrupt each
other's source copies or targets. Build/probe timeouts fail the case. A memory
guard stops owned child process groups below 6 GiB MemAvailable. No rustd is
started by stock mode; future refresh mode must use the rustd lane.

Build and stderr logs remain in `.work/logs/<case>/`. `--results` emits a compact
JSON summary with observations and each final edited binary's SHA256. This is
correctness evidence, not performance data. To reclaim only suite-created build
artifacts, remove `tests/semantics/.work/` after the runner has exited.
