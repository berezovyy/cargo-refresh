# Self-contained ABI qualification inputs

`helper.rs`, the `fixtures/` sources, and the baseline raw stream in `reference/`
were ported without changing research history from EXP-104b and EXP-128. They
are retained under the repository's MIT/Apache-2.0 license. The fixtures preserve
the crate identities, dependencies and local types used by that qualification;
the embedded experimental state arena is only a probe dependency, not a product
state-persistence feature. These crates are not workspace members or user demos.

`new-toolchain.sh` copies them to its private output before building. The collector
compares the reconstructed facts with the checked-in raw stream and retains the
unchanged qualified baseline bundle after allowing only the documented local
TypeId relocation differences. Candidate toolchains still require manual review.
