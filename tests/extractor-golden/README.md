# Optional captured extractor goldens

The large historical ELF capture corpus is not included in this repository. The
ignored Rust compatibility test can read a separately supplied capture directory;
normal workspace tests cover commit/rollback, stale/out-of-order manifests and
wire validation without it. A clone does not require those external objects.

Each capture has input.json, ordered compressed manifests, a compressed candidate
ELF, expected.wire, and result.json. Baselines are shared compressed ELF files.
The test reconstructs prior acknowledged state and compares the encoded wire bytes.
