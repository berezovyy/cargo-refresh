# Contributing

The MVP uses hand-run checks, with no CI/CD. Use the pinned Rust 1.99.0 toolchain;
never change the global rustup default or Cargo config. Work on an isolated branch
and keep changes scoped. Packaging and cache contracts are in
[docs/install-layout.md](docs/install-layout.md).

From the repository root:

```sh
RUSTUP_TOOLCHAIN=1.99.0 scripts/test.sh
make -C agent test
make -C agent test-robustness test-panic test-pressure
python3 tests/semantics/run.py --help
python3 tests/semantics/run.py --mode stock --offline
python3 tests/semantics/run.py --mode refresh
```

`scripts/test.sh` runs formatting, Clippy and Rust workspace tests, stopping at the
first failure. It currently does not run agent, semantics or differential suites;
run those separately when relevant. It honors RUSTUP_TOOLCHAIN for candidate
qualification and defaults to the pinned baseline. Agent tests include a stock
compiled C host oracle. The semantics runner compares each recorded case with a
normal Cargo build. `--case <exact-name-or-prefix>` selects a case; `--results <path>`
writes evidence. `--record` changes golden outputs: use it only when deliberately
reviewing and updating the stock contract. Refresh mode invokes the real Cargo
subcommand, checks controller events, process identity and exact probe output, and
reports safe path mismatches separately. It needs installed qualified toolchain
support (or `--support` / `--archive`) and `cargo-refresh` on PATH (or
`--controller /path/to/cargo-refresh`). See [the suite README](tests/semantics/README.md)
for resource lanes, the environment-relaunch limitation and result interpretation.

Real application sessions live outside this repository, at
`/path/to/research/work/m4`. Read its README before running
anything: it contains pinned apps, private service locations, source restoration
rules and verification status. Example stock smoke (no latency claim):

```sh
cd /path/to/research/work/m4
./projects/mdbook/setup.sh
../../tools/lane.sh rustd -- python3 scripts/guard.py python3 scripts/stock_oracle.py mdbook --baseline-only
# Complete cumulative session, comparing every accepted save with stock:
../../tools/lane.sh rustd -- python3 scripts/guard.py python3 scripts/stock_oracle.py mdbook
```

Other sessions are `zola`, `atuin`, `meilisearch`, and `conduit`. Conduit requires its
private PostgreSQL setup; follow that README and stop only the service you started.
Use web and mdBook for iteration; run rust-analyzer only for a task that needs it.
Copy research fixtures into private scratch rather than editing experiments,
fixtures or benchmark inputs in place. Reuse your warm targets within a task.

## Shared machine resources

Use the research repository's `tools/lane.sh` for coordination:

```sh
# Any rustd daemon/session or rust-analyzer work:
tools/lane.sh rustd -- <command>
# Heavy Cargo builds; always bounded, low priority for setup:
tools/lane.sh build -- nice -n 19 cargo build --locked -j 4
# Reported timing runs, exclusive of rustd work:
tools/lane.sh quiet -- <benchmark-command>
```

These examples are run from the research root; use the absolute lane path from
other directories. Run at most one rustd daemon per task; a second shared slot
requires at least 14 GiB free and does not authorize concurrent daemons in your own
session. Monitor MemAvailable continuously and stop your own heavy process groups
below 6 GiB. `scripts/run-guarded.py <command...>` implements that guard. Never signal
someone else's processes. While a lane is busy, do code-only work. Keep disk use
within the task budget; clean only the target directories you created.

## Correctness and numbers

Fail closed: uncertain live patches must restart or use stock Cargo. Every save
must produce a result or a clear diagnostic. Compare observable output with stock
from the same toolchain and edited source; successful compilation alone is not an
oracle. A stale response is a failure even if a daemon reports success.

There is currently no `scripts/bench.sh` release driver in this checkout. Do not
claim the proposed benchmark commands already work. Follow the research plan in
`mvp-plan/testing-and-numbers.md` when implementing/running it; commit raw release
rows to `bench/results/` before filling README `BENCH` tokens. No headline
performance claim is qualified by this packaging work.

For reported numbers acquire the quiet lane and wait for load1 ≤ 2 (prefer ≤ 1 for
public release measurements), using the research EXP-101 settle helper. Recheck
after acquiring a queued lane. Interleave stock/refresh A/B per repetition; do not
measure all stock samples first. Record `/proc/loadavg` and MemAvailable for every
timing block; report per-pair ratios as median [min, max], sample count, warm/cold
state and machine details. Use the release repetition policy (at least ten per
scenario, discarded warm-up, median and tail) for headline numbers; small gains
need more repetitions. Mark anything not run UNMEASURED. Ordinary code edits
need relevant checks and a stock oracle, not a fresh performance campaign.

## Packaging

```sh
scripts/new-toolchain.sh 1.99.0 --dry-run
scripts/release.sh 0.1.0-dev
```

These create local candidates and logs only. Full qualification, older-glibc
validation, license review and manual publication are release gates. Never push
or publish from an implementation task.
