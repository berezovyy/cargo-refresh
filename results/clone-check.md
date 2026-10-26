# Clone-and-try check

On October 25, 2026, a local `git clone --no-hardlinks -b public` was checked
with a private HOME, cleared environment and independently downloaded Rust.
No developer Cargo/rustup installation or target cache was copied into that HOME.
This is same-account isolation, not a separate user, VM, or older distribution.

`./try.sh --smoke` passed with only cargo, rustc, rust-std and rust-src installed.
It installed Rust automatically from an initially empty HOME, built the controller,
and used the committed clean-build archive. The two body edits went live with
PID/counter continuity, a struct edit restarted, E0308 kept the old program
running, and the fix recovered live. Baseline and valid source states matched
independent `cargo run` outputs; the invalid state matched `cargo check`.

In the first independent clone, `scripts/test.sh` passed formatting, strict
Clippy, and 183 tests, with two explicitly ignored external/integration tests.
`scripts/build-bundle.sh --native` built support from that clone's own `./rustd`,
including ABI/reference, TypeId, personality and payload checksum checks.
The clean build exposed a missing source-build prerequisite: llvm-tools-preview.
The release script now diagnoses it early and the installation docs include it.
The runtime quick start does not need rustc-dev or LLVM tools.

[Raw demo observations, test summary lines and archive checksum](data/clone-verification.json).
The archive is native and imports GLIBC_2.43; older-distro compatibility of this
archive was not tested. Benchmark input validation passed; no new speed
measurement was run. Final-build performance remains **UNMEASURED**.
