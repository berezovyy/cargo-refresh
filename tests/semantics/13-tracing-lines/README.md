# 13: Instrument span metadata after line insertion

v0.1 expected outcome: **works / restart** (M4 §5). `works` means live patch; `restart` means explicit refusal followed by restart; `stock` means stock rebuild and restart. This is a specification, not a refresh qualification result.

Warm the instrumented callsite twice before inserting three lines above the attribute. An EnvFilter plus fmt subscriber captures a formatted span location, and a custom Layer reads raw metadata.line(). Strict metadata policy may restart; accepted live updates must show the new location in both fields.

The M4 table notes that cached metadata can be stale under the M3 policy. This fixture deliberately demands the correct line for a live outcome; a policy that permits stale lines needs an explicit documented exception and must not weaken this oracle silently.

Send `probe` on stdin; one `probe=...` line is returned. Send `quit` to exit. Run two base probes, apply each cumulative step in `edits.toml`, then run its two probes. Live probes use the original process; stock probes use a fresh process after every edit.

`edits.toml` declares full expected lines. `expected.json` records verified stock observations separately from the live contract. Live behavior is UNMEASURED until the refresh adapter is implemented.

Reproduce stock: `./stock.sh`. Record after intentional fixture changes: `./stock.sh --record`. Run from any directory. Python 3.11+, Rust 1.99.0 and Cargo are required.
