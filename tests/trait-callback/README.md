# Owned callback behind a stock generic runtime

M4-T3 stale-restart regression: `stock_dispatch::dispatch::<Value>` cannot be
compiled by rustd (over-aligned SysV C argument), so its rustc instance calls
the owned `Label::label` implementation. Discovery must continue across the
forwarded runtime. Editing `label` from 1 to 2 must produce 2, matching stock.

Copy this fixture into a disposable work directory. Hold a rustd resource lane
and use `scripts/trait-callback.py --project COPY --out RESULTS --controller BIN --expect-path live`.
The script checks HTTP output against an independent stock build and restores
source. The x86_64 Linux qualification toolchain is Rust 1.99.0.
