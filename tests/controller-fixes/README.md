M4-T6 controller regressions use a copied tiny axum fixture, its stock Cargo oracle,
and explicit disposable disk directories. Hold the rustd resource lane around each
run, set `TMPDIR` to a short disk directory, and set qualified support normally.

```sh
RUSTUP_TOOLCHAIN=1.99.0 REFRESH_CORRECTNESS_ONLY=1 \
  python3 tests/controller-fixes/run.py recovery --controller /path/to/cargo-refresh --work /disk/work
# Use modes cfg and split in separate work directories for the other regressions.
```

`recovery` kills only its captured daemon, then makes 100 saves 0.15 seconds apart.
It requires one recovery start, a return to live, the final response and stock match.
`cfg` edits three disabled functions and a disabled macro definition and requires no-effect skips, stock matches,
and an enabled live edit afterward. `split` delivers 30 closely spaced two-file
saves, requires final delivery with no stock fallback, and checks stock output.
These are correctness checks under current machine load; their durations are not
benchmark measurements.
