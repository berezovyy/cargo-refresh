# 20: Exported extern C entry reached by a C caller

v0.1 expected outcome: **works / restart** (M4 §5). `works` means live patch; `restart` means explicit refusal followed by restart; `stock` means stock rebuild and restart. This is a specification, not a refresh qualification result.

Compile a tiny C object with the system cc, linked into the Rust app. Its indirect call uses a retained pointer to an exported no_mangle extern C Rust function. Both the C call and direct Rust call must reach new code, or refresh must refuse.

Send `probe` on stdin; one `probe=...` line is returned. Send `quit` to exit. Run two base probes, apply each cumulative step in `edits.toml`, then run its two probes. Live probes use the original process; stock probes use a fresh process after every edit.

`edits.toml` declares full expected lines. `expected.json` records verified stock observations separately from the live contract. Live behavior is UNMEASURED until the refresh adapter is implemented.

Reproduce stock: `./stock.sh`. Record after intentional fixture changes: `./stock.sh --record`. Run from any directory. Python 3.11+, Rust 1.99.0 and Cargo are required; cc and ar are also required.
