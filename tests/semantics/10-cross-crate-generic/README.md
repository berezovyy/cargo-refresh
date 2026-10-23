# 10: Generic instantiations across crate/frontier boundaries

v0.1 expected outcome: **works / stock** (M4 §5). `works` means live patch; `restart` means explicit refusal followed by restart; `stock` means stock rebuild and restart. This is a specification, not a refresh qualification result.

Edit generic_lib::value<T>. The app and caller crate each instantiate u8 and u64, with different sizes. When all three crates are in the frontier all four values must change. With caller outside the frontier this same edit must use stock, updating the dependent crate too.

Refresh qualification must run two frontier configurations: app+generic-lib+caller => live, app+generic-lib with caller outside => stock. The stock oracle is identical for both; frontier control is pending the controller integration.

Send `probe` on stdin; one `probe=...` line is returned. Send `quit` to exit. Run two base probes, apply each cumulative step in `edits.toml`, then run its two probes. Live probes use the original process; stock probes use a fresh process after every edit.

`edits.toml` declares full expected lines. `expected.json` records verified stock observations separately from the live contract. Live behavior is UNMEASURED until the refresh adapter is implemented.

Reproduce stock: `./stock.sh`. Record after intentional fixture changes: `./stock.sh --record`. Run from any directory. Python 3.11+, Rust 1.99.0 and Cargo are required.
