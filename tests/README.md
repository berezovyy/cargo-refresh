# Tests
Cross-component fixtures and scripts live here; controller integration tests currently live in crates/cargo-refresh/tests.

`backtrace-fixture` and `scripts/backtrace-session.py` exercise three live panic
patches, moving source lines, byte-preserving stderr forwarding, and a stock
rustc oracle. The patched leaf lives in its own module so line shifts do not
recompile the active server loop. See `docs/backtraces.md` for the rustd lane and
compiler setup command.
