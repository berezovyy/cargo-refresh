# Controller integration fixture

A disposable copy is required: `scripts/integration-session.py tiny` edits its
project in place. It exercises ten saves: body, new helper, top comment, struct
field, async handler, syntax error, borrow error, Cargo manifest, identical save,
and a body revert. Each valid output is compared with an independently built
stock executable; expected errors are also compiled with stock. The borrow case
asserts auto-revert preserves the editor's invalid source bytes.

The script needs the built controller and qualified support from the committed
bundle. Use a disposable copy and set `CARGO_REFRESH_DIST` to its absolute path:

```sh
python3 scripts/integration-session.py tiny \
  --controller target/release/cargo-refresh \
  --project /absolute/path/to/private/tiny-copy \
  --out /absolute/path/to/new/results-directory
```

Use `scripts/run-guarded.py` for a 6 GiB memory floor. Default workspace tests do not
start a real rustd daemon. The tiny driver is an explicit, hand-run integration
check and waits for load1 <= 2 before starting its session.

The same driver has `web` and `mdbook` modes. Web accepts `--scenarios` pointing
to the research 64-save generator and `--bin web-server --edit api,domain,store`.
mdBook accepts the prepared `--session`, `--expected` stock snapshots, and a
private `--book` directory. Set its required callback policy in the private
project metadata; absent callbacks must restart rather than claim cached HTML
has refreshed.
