# Compiler source for cargo-refresh

This is a plain-file copy of our compiler fork at `b6e0d81b2a46b3e1463bc6b73d2c8d0e8e9ac6d7`, based on [DioxusLabs/rustd](https://github.com/DioxusLabs/rustd) upstream commit `159f69351d92fbc09ea9cc787544936fc7f20b37`. Its rust-analyzer source is included under `vendor/rust-analyzer` at `06dd1e35671f0410c543f3498c8d14e48e488339`, based on the DioxusLabs rust-analyzer branch at `6b1940be0e68cb8004301d6c2dcbdf3f71257fca`. No submodule initialization is needed.

Our changes add a persistent compiler session and patch protocol, transactional source updates and rollback, exact-toolchain ABI facts, compact append-only code generations, retained closures and unwind support, conservative async/layout fallbacks, assertion messages from the matching rustc parser/printer, and recovery that preserves the last working compiler baseline. The frontend includes per-body diagnostic memoization and macro/token provenance fixes. Some inherited upstream documents describe features outside cargo-refresh's supported scope; start with the top-level README.

Build from the repository root with `scripts/build-bundle.sh --native`. See `SOURCE.json` for the imported source identities. The local packaging patch
also advertises `support_protocol: 1` in `rustd --capabilities` so the controller
can reject incompatible support before starting a compiler session. Both projects retain their MIT and Apache-2.0 licenses here and in `vendor/rust-analyzer`.
