# Re-measure Cargo versus cargo-refresh

The final build has not been benchmarked yet. This driver measures the committed
demo and bundled axum web fixture, with the same visible function-body edit in
separate stock and refresh copies. First install/check the controller:

```sh
./try.sh --smoke
bench/run.sh                 # demo and web, 15 measured pairs each
bench/run.sh demo --pairs 5  # exploratory run
bench/run.sh web --pairs 30
bench/run.sh --validate-only # input check; no compiler session or measurements
```

Requires the quick-start prerequisites and enough RAM for the web compiler. The
whole driver has a 6 GiB MemAvailable guard and uses four build jobs. It waits
for load1 ≤ 2 before each arm (ten-minute timeout). On shared machines, arrange
exclusive benchmark access; an external quiet lane may wrap the entire command.
Do not run other compiler sessions alongside measurements.

For every repetition it writes/closes the stock edit, builds with normal Cargo,
restarts, and polls until the new answer appears; then it applies the same edit
through the running refresh session. This gives A/B/A/B ordering. Setup builds use
nice 19; timed work uses normal priority. One complete warm-up pair is discarded.
Dependencies, filesystem caches and the compiler database are warm; there is no
cache flush or CPU affinity. Refresh must report `live`, retain PID/ticks and match
the stock response. Background checking completes before the next pair.

Time starts after file close and ends at the observed response. HTTP/stdout polling
uses a 2 ms sleep; the demo itself has a 20 ms tick, which limits its resolution.
Web compares the complete stable `/probe` value; PID/ticks are checked separately.
The demo compares message output and continuity; `try.sh --smoke` additionally
checks arithmetic, layout, error and recovery behavior.

`results/run-<UTC timestamp>/` receives raw `pairs.jsonl` (including marked excluded
warm-up), full-precision `summary.json`, `machine.json`, and a readable table.
Every arm records loadavg and MemAvailable before/after. Ratios are computed per
pair, then summarized as median [min, max] with n. Warm targets and logs are retained
under `.bench-work/`; remove those directories when you no longer need them.
Failed runs retain partial rows and logs, never a completed summary claim.

The response-marker scenario does not reproduce every earlier project/scenario or
establish whole-commit speed. To extend the driver, add observable source edits
and matching stock probes, keep the same build settings in both arms, and retain
all raw rows. [Earlier measurements and caveats](../results/speed.md) stay separate.
