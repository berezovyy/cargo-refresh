# 17: SQLx checked SQL text

v0.1 expected outcome: **restart / stock** (M4 §5). `works` means live patch; `restart` means explicit refusal followed by restart; `stock` means stock rebuild and restart. This is a specification, not a refresh qualification result.

Compile and execute a real sqlx::query! against in-memory SQLite. Both query versions have committed SQLx offline describe data. Changing the SQL literal must yield the new row; compile-time DB-dependent macro edits are restart or stock.

SQLite plus offline metadata makes this self-contained; no Postgres service or credentials are needed. This tests real SQLx macro expansion and query execution, but not online schema changes or Postgres-specific behavior.

Send `probe` on stdin; one `probe=...` line is returned. Send `quit` to exit. Run two base probes, apply each cumulative step in `edits.toml`, then run its two probes. Live probes use the original process; stock probes use a fresh process after every edit.

`edits.toml` declares full expected lines. `expected.json` records verified stock observations separately from the live contract. Live behavior is UNMEASURED until the refresh adapter is implemented.

Reproduce stock: `./stock.sh`. Record after intentional fixture changes: `./stock.sh --record`. Run from any directory. Python 3.11+, Rust 1.99.0 and Cargo are required.
