# A running counter you can edit

From the repository root run `./try.sh`, then edit `src/main.rs` here.

1. Change `message()` from `"hello from version 1"` to `"hello from version 2"`.
   It goes live; PID, ticks and total continue.
2. Change `step()` from `n * 1` to `n * 2`. It goes live; the total grows faster.
3. Add `_marker: u64` to `State` and `_marker: 0` to its initializer. The program
   restarts, its PID changes, and its in-memory counters reset.
4. Make `message()` return `42` while its signature still returns `&'static str`.
   The error is shown and the previous program keeps counting. Fix the return
   value to recover.

`./try.sh --smoke` applies all these edits to a private copy, verifies the paths,
checks PID/state continuity, compares with stock Cargo, restores nothing in your
working demo, and exits. Logs are under `.try/smoke-*/`.

Compare manually using plain Cargo from this directory:

```sh
RUSTUP_TOOLCHAIN=1.99.0 cargo run --bin demo
# Stop, edit, and rerun: the process and counters start over.
DEMO_ONCE=1 RUSTUP_TOOLCHAIN=1.99.0 cargo run --quiet --bin demo
```

`DEMO_ONCE=1` prints one tick for the stock oracle. `DEMO_FAST=1` uses a 20 ms tick
instead of one second for automated checks and measurements.
