# Compiler source for cargo-refresh

This directory contains the compiler and its rust-analyzer frontend as ordinary
source files. Start with the [cargo-refresh README](../README.md) to try it.
[Fork identity, upstream credits and changes](README-FORK.md) describe this copy.
No separate checkout or submodule is needed.

From the top-level repository, build native support with:

```sh
scripts/build-bundle.sh --native
```

See [installation requirements](../docs/install-layout.md) for matching rustc-dev,
rust-src and LLVM tools. The included native bundle avoids this build for the
quick start. Tests are in `harness/`; inherited standalone-compiler features
outside the top-level supported scope are not cargo-refresh guarantees.

MIT OR Apache-2.0; licenses are retained here and in `vendor/rust-analyzer`.
