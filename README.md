# cargo-refresh

cargo-refresh is an MVP in development for fast Rust refresh: keeping a compiler warm, recompiling changed functions, and applying supported edits to a running program while preserving its state. Unsupported edits need a restart or a normal Cargo build.

> **A one-line Rust edit can trigger work across whole crates.**
> Linker and backend improvements help when those stages dominate; repeated compiler work can remain.
> [Why Rust rebuilds are slow, and how to check your own project →](docs/why-rust-rebuilds-are-slow.md)

The M2 controller now builds and supervises applications, checks saves, and routes changes through live patching, restart, or stock Cargo. See [controller usage and current scope](docs/controller.md); run `./scripts/test.sh` for the hand-run checks.

Licensed under MIT OR Apache-2.0.
