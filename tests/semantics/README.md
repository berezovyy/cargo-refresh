# M4 runtime semantics fixtures

These 20 standalone packages implement the M4 §5 matrix. They specify desired
live behavior, verify stock Rust behavior, and exercise the real refresh
controller. Qualification observations are written to the selected results file. Each case has Rust 1.99.0, `[profile.dev] debug = 0`, a lockfile,
`edits.toml`, recorded `expected.json`, a README, and an executable `stock.sh`.

Run from the product repository:

```sh
python3 tests/semantics/test_runner.py
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
expectations remain specifications; actual refresh observations are recorded separately
in the selected results file, without rewriting the stock contract. The legacy
`live-verification` text in `expected.json` describes its original stock-only
provenance; it is not a verdict for a new refresh run.

## Real refresh adapter

Run the real controller with qualified installed support:

```sh
python3 tests/semantics/run.py --mode refresh --offline \
  --controller /absolute/path/to/cargo-refresh \
  --support /absolute/path/to/verified/support \
  --results /tmp/semantics-refresh.json
# Or use --archive /path/to/release.tar.zst (with its adjacent checksum).
# With Cargo and support installed normally, no controller/support flags are needed.
```

The committed fixtures intentionally contain `[profile.dev] debug = 0` (case 11
also sets optimization level 1). Current installation policy rejects any custom
dev profile and chooses stock, which is recorded as an admission/path finding.
For an explicit additional runtime sweep, pass `--default-profile` to both the
stock and refresh modes, using separate `--work` and `--results` paths. This
removes only a sole `debug = 0` override from disposable manifests; it leaves the
optimized inline fixture unchanged. The result records every normalized manifest
and labels its profile mode. Original manifests and output expectations stay
pristine. Do not merge fixture-profile and default-profile results silently.

The driver invokes `cargo refresh run` in disposable copies, retaining app stdin
and stdout separately from controller stderr and JSONL events. It waits for
`ready`, consumes the base probes exactly once, writes each cumulative edit,
then waits for an explicit terminal `save` event. It never polls a stateful probe
until a desired value appears. Every complete output line must match
`live-expected` for live, or `stock-after-restart` for restart/stock. It also checks
owned app PID plus Linux start ticks before and after every save and probe batch:
live must preserve identity, and restart/stock must replace it. It does not
substitute a stock run when a live probe fails. Well-formed but wrong base
responses mark the configuration failed while still allowing the edit steps to
run, so a baseline issue cannot hide subsequent edit behavior.

Output correctness and path expectations are separate fields. A safe fallback
with correct fresh-process output passes correctness and reports a path mismatch;
`--require-paths` makes these mismatches affect the exit status too. Timeouts,
crashes, error outcomes and wrong outputs fail. Timeout results retain all
intervening events and one diagnostic probe (which cannot count as success).
`--timeout` bounds ready/save waits; the default is 180 seconds. The results
contain actual controller reasons and probe responses for each edit.

Case 10 runs twice using its declared `--edit` frontier lists. Case 08 checks
retained/fresh vtables, a saved pointer and a direct call in a single response;
case 13 checks both raw and formatted tracing line metadata. Case 14 saves only
`message.txt`, so a missing watcher notification fails rather than being masked
by an extra Rust edit. Case 16 saves `build.rs` and records the controller's
stock/recapture events. When qualified compiler support was active initially,
it waits for `worker_reset` or reports a recapture failure; stock-only installation
mode explicitly records recapture as not applicable.

Case 15 is an explicit coverage limitation: a running controller has no
in-session environment update API. The driver stops the session, supplies the new
environment, launches `cargo refresh run` again, and checks fresh-process output
and changed identity. Its actual route is recorded as `session-relaunch` with a
path mismatch, **never** as a controller stock save event. This proves tracked
environment inputs on relaunch only; automatic environment reconfiguration
remains unqualified. Its configuration result is `PARTIAL` and makes the command
exit nonzero, even when relaunch output passes. No artificial source edit or
forced support failure is used.

Rows marked works/refuse require qualification to determine which outcome is
supported. Stock success alone settles none of TLS support, retained vtables,
closure retention, live unwinding, generation static allocation, or the inline
recompilation dependency graph.

## Workspace, resources, and evidence

Cases are independent Cargo workspaces, not members of the product workspace.
Case 11 uses dev `opt-level = 1` plus `#[inline(always)]` to exercise inlined
callers; all other cases use ordinary unoptimized dev builds.

Stock mode builds disposable source copies beneath `.work/sources/` and uses a
shared warm `.work/target/`. Original case sources stay pristine. Staged files get fresh mtimes so Cargo
cannot reuse the prior edited binary when restoring a base source. All builds run
as `nice -n 19 cargo build -j 4` with an explicit toolchain and normal runs use
`--locked`. Runs take a nonblocking exclusive lock so they cannot corrupt each
other's source copies or targets. Build/probe timeouts fail the case. A memory
guard stops owned child process groups below 6 GiB MemAvailable. No rustd is
started by stock mode. Refresh mode automatically re-executes under the research
rustd lane when `--lane` exists; do not wrap it in another rustd lane. Controller
capture/fallback builds and the stock oracle acquire the build lane separately,
using four jobs. Outside the research machine, set `--lane` to a suitable lane
script, or a nonexistent path when no shared lane service is required.

Refresh uses `.work-refresh/` by default; `--work` selects another private warm
root. Its Cargo builds use each copied case's default `target/`, preserved across
reruns; setting `CARGO_TARGET_DIR` would select the installer's conservative stock
mode. The driver omits verbose controller config output to keep app stdout clean. Case 10 configurations have separate source/log directories. Controller
stderr and events remain in `logs/<case>/session-*.{log,jsonl}` within that root.
Stock build and stderr logs remain in `.work/logs/<case>/`. `--results` emits a compact
JSON summary with observations and each final edited binary's SHA256. This is
correctness evidence, not performance data. To reclaim only suite-created build
artifacts, remove `tests/semantics/.work/` after the runner has exited.
