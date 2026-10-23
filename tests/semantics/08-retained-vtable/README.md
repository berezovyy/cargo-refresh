# 08: Retained trait object, method pointer and direct call

v0.1 expected outcome: **works / restart** (M4 §5). `works` means live patch; `restart` means explicit refusal followed by restart; `stock` means stock rebuild and restart. This is a specification, not a refresh qualification result.

Retain a Box<dyn Value> and a concrete method pointer before the edit. Probe retained vtable, fresh vtable, saved method pointer, and direct method call. With entry jumps all four reach the new method. Actual live routing is unmeasured until integration; refuse if this contract cannot be met.

M4 requests documenting actual behavior, not assuming vtable relocation. The adapter must record all four fields. Without a method-entry redirect the retained vtable may still return 1; such a result is evidence to document/refuse, not an automatically accepted alternative. The direct and indirect paths are both probed.

Send `probe` on stdin; one `probe=...` line is returned. Send `quit` to exit. Run two base probes, apply each cumulative step in `edits.toml`, then run its two probes. Live probes use the original process; stock probes use a fresh process after every edit.

`edits.toml` declares full expected lines. `expected.json` records verified stock observations separately from the live contract. Live behavior is UNMEASURED until the refresh adapter is implemented.

Reproduce stock: `./stock.sh`. Record after intentional fixture changes: `./stock.sh --record`. Run from any directory. Python 3.11+, Rust 1.99.0 and Cargo are required.
