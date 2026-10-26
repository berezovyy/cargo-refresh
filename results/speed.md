# Speed: earlier-build measurements

**The final build has NOT been benchmarked.** These warm-cache measurements were
recorded on an earlier build on October 20, 2026 (UTC), Linux x86_64, Rust 1.99.0,
debug information disabled. Re-run the
current demo and web comparison with `bench/run.sh`; do not treat these numbers
as a speed guarantee for this checkout.

Each cell is median [min, max] in milliseconds, except the dimensionless paired
ratio. Stock save/build/restart/visible was followed by the identical source edit
through refresh in every repetition (ABAB). Setup, capture and checker warm-up are
excluded; watcher, compile, patch and observable response are included. Five pairs
per game/web/mdBook row; rust-analyzer has five apply/revert repetitions, ten paired
transitions. Ratios are computed per pair, not from the displayed medians.

| App | Edit | Cargo + restart ms | Live ms | Paired Cargo/live | n |
|---|---|---:|---:|---:|---:|
| game | S1 | 490.9 [483.8, 513.2] | 55.6 [45.9, 93.8] | 8.8 [5.2, 11.2] | 5 |
| game | S2 | 494.5 [485.6, 501.4] | 63.0 [53.2, 68.9] | 7.8 [7.1, 9.4] | 5 |
| game | S3 | 665.4 [652.5, 687.0] | 72.2 [60.5, 73.8] | 9.4 [9.0, 11.3] | 5 |
| game | S8 | 505.8 [502.7, 525.7] | 76.7 [60.0, 79.2] | 6.7 [6.4, 8.4] | 5 |
| game | LINE | 519.8 [513.1, 529.8] | 64.4 [56.5, 79.1] | 8.1 [6.6, 9.4] | 5 |
| game | S4 | 1195.6 [1177.2, 1213.7] | 647.9 [618.0, 998.6] | 1.9 [1.2, 1.9] | 5 |
| web | S1 | 611.4 [604.0, 618.0] | 157.1 [140.9, 164.1] | 3.9 [3.7, 4.4] | 5 |
| web | S2 | 623.6 [612.0, 627.6] | 155.3 [150.0, 177.9] | 4.0 [3.5, 4.2] | 5 |
| web | S3 | 763.7 [754.9, 784.8] | 88.9 [78.4, 288.6] | 8.6 [2.6, 9.6] | 5 |
| web | S8 | 644.6 [627.7, 1507.6] | 162.6 [155.3, 334.5] | 4.1 [1.9, 9.1] | 5 |
| mdBook | renderer | 940.7 [931.4, 1867.7] | 189.1 [164.6, 469.8] | 5.0 [4.0, 5.7] | 5 |
| mdBook | preprocessor | 801.0 [770.8, 2084.4] | 210.1 [195.1, 231.5] | 3.8 [3.5, 10.4] | 5 |
| mdBook | search | 939.4 [933.5, 1673.2] | 182.7 [177.9, 209.6] | 5.3 [4.5, 9.1] | 5 |
| rust-analyzer | S1 | 3260.7 [2458.8, 3639.5] | 251.9 [216.4, 468.4] | 13.1 [5.5, 15.2] | 10 |
| rust-analyzer | S2 | 3603.8 [3473.7, 4016.7] | 194.5 [181.9, 216.2] | 18.8 [16.3, 20.9] | 10 |
| rust-analyzer | S3 | 17948.9 [15409.9, 19652.1] | 218.4 [195.1, 367.3] | 77.0 [53.5, 88.4] | 10 |
| rust-analyzer | S8 | 3894.1 [3465.8, 4368.4] | 246.2 [240.4, 477.5] | 15.2 [9.0, 17.7] | 10 |

[Raw paired observations with load/RAM](data/earlier-speed-pairs.jsonl) and
[full-precision summary](data/earlier-speed-summary.json).
[Toolchain output](data/earlier-speed-environment.json),
[resource samples](data/earlier-speed-resources.jsonl),
[resource summary](data/earlier-speed-resource-summary.json), and
[prototype binary identities](data/earlier-speed-provenance.json) are also retained.
Game S4 is a **restart with prototype state restoration**, not a live update.
State restoration across restarts is not a feature of this MVP.
mdBook used a cache invalidation hook and browser reload notification;
rust-analyzer used an explicit application safe point. Each app used its qualified
prototype configuration, not one unified compiler binary. The ordinary body's
cadence and network polling are included; browser DOM painting is not timed.

The runs waited for quiet setup, but measured load1 still ranged from 1.42 to
4.39 as their own work ran. Web load1 was 3.39–3.68, mdBook 3.15–4.06. Desktop
activity remained; these are not zero-load or hardware-independent constants.
Minimum sampled MemAvailable over setup/timing/cleanup was 9.59 GiB. A separate
mixed game run found slower body edits after layout churn, so the grouped body
figures must not be generalized to arbitrary edit histories. No claim about
complete application correctness follows from timing probes alone.

## Loaded correctness-run diagnostics

These are **not benchmark results**: 46 live web saves recorded during a loaded
correctness run on October 24, 2026, with the preceding assertion-printer compiler
build. The controller's notice→trigger latency median was 144 ms; compile accounted
for 114 ms. Observed save→visible median was 162 ms. Final-build performance remains
UNMEASURED. The phase timestamps, per-save loadavg, MemAvailable and stock-match
verdicts are retained [here](data/loaded-web-phases.json).

| Diagnostic interval | Median [min, max] ms | n |
|---|---:|---:|
| Controller notice→trigger | 144.47 [93.58, 1348.52] | 46 |
| Compiler build | 113.51 [77.56, 468.14] | 46 |
| Agent send→ack | 5.86 [0.61, 8.77] | 46 |
| Save→visible probe | 162.08 [109.10, 11405.19] | 46 |

Medians of phases do not add up to a median total. No speedup is inferred from these diagnostic intervals.
