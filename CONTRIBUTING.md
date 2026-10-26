# Contributing

Use Rust 1.99.0. The project has hand-run checks and no CI/CD. Work on a branch,
keep changes scoped, and compare behavior with normal Cargo before widening live
admission. An uncertain patch must restart or use Cargo; every save needs a
result or diagnostic.

```sh
scripts/test.sh
make -C agent test
make -C agent test-robustness test-panic test-pressure
python3 tests/semantics/run.py --help
```

`scripts/test.sh` runs formatting, strict Clippy and Rust workspace tests. It
serializes tests that spawn/fork processes to avoid executable-copy races. It
does not run agent or semantics suites. Agent tests need libunwind development
headers/libraries; the production agent only needs libc and libgcc_s.

The semantics suite compares recorded cases against stock Cargo. Use
`--mode stock`, then `--mode refresh --archive /absolute/path/to/bundle.tar.zst`
and a controller on PATH (or `--controller`). `--case` selects cases; `--results`
writes evidence. `--record` changes the golden contract and needs deliberate
review. See [suite instructions](tests/semantics/README.md). Some historical
extractor compatibility goldens require external captured objects and remain
ignored by default; they are not implied to ship in this clone.

`./try.sh --smoke` provides a self-contained live/restart/error/stock check.
[Recorded qualification](results/correctness.md) states the scope of broader app
and history checks. Their original large application workspaces are not bundled;
the compact evidence does not substitute for rerunning upstream app test suites.

For performance work, use [bench/run.sh](bench/README.md). Wait for load1 ≤ 2,
interleave stock/refresh per repetition, record loadavg and MemAvailable, and
report per-pair ratios as median [min, max] with sample count and machine state.
Do not call loaded correctness-run latency a benchmark. Final-build speed is
UNMEASURED until these measurements are run separately.

Use four Cargo build jobs. On a shared machine an optional external resource
wrapper may be passed through `REFRESH_LANE`; no wrapper is required. Run one
compiler session at a time, use on-disk TMPDIR/work directories, and keep the
6 GiB memory guard (`scripts/run-guarded.py`) around heavy work. Stop only your
own processes and remove only your own scratch/targets.

Support builds and the exact toolchain contract are described in
[installation](docs/install-layout.md). Source is included under `rustd/`; no
other repository is needed. Packaging only creates local archives.
