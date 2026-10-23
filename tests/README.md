# Tests
Cross-component fixtures and scripts live here; controller integration tests currently live in crates/cargo-refresh/tests.

`backtrace-fixture` and `scripts/backtrace-session.py` exercise three live panic
patches, moving source lines, byte-preserving stderr forwarding, and a stock
rustc oracle. The patched leaf lives in its own module so line shifts do not
recompile the active server loop. See `docs/backtraces.md` for the rustd lane and
compiler setup command.

`trait-callback/` covers M4-T3 owned callbacks reached through an unsupported
stock generic runtime. Its runner compares the saved program to stock and can
require live delivery; see its README.
