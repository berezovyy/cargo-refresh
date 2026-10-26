# Tests
Cross-component fixtures and scripts live here; controller integration tests currently live in crates/cargo-refresh/tests.

`backtrace-fixture` and `scripts/backtrace-session.py` exercise three live panic
patches, moving source lines, byte-preserving stderr forwarding, and a stock
rustc oracle. The patched leaf lives in its own module so line shifts do not
recompile the active server loop. See `docs/install-layout.md` for setting up compiler
support.

`trait-callback/` covers owned callbacks reached through an unsupported
stock generic runtime. Its runner compares the saved program to stock and can
require live delivery; see its README.

`dep-inputs/` checks existing and newly introduced include_str!/include_bytes!
inputs through stock and real refresh sessions. `profile-admission/` checks the
four debug-assertions/overflow-checks combinations against stock before/after a
live leaf edit. Both reuse the semantics adapter's process/lane/memory checks.
