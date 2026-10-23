# refresh-guard

In-process admission and edit classification for M2 step 3. No filesystem reads,
subprocesses, compiler session, or async certificates are used by the library.

```rust
use refresh_guard::{Admission, Class, FileSnapshot, Guard};

let before = [FileSnapshot::new("src/lib.rs", b"fn answer() -> u8 { 1 }".to_vec())];
let after = [FileSnapshot::new("src/lib.rs", b"fn answer() -> u8 { 2 }".to_vec())];
let mut guard = Guard::new();
assert_eq!(guard.admit(&before, &after), Admission::Admit);
assert!(matches!(guard.classify(&before, &after), Class::LiveEligible { .. }));
```

`admit(before, after)` and `classify(before, after)` free functions provide the
same API using a thread-local default cache. Prefer an owned `Guard` for one
controller session. It is `Send + Sync`; mutating methods need exclusive access.

Supply matching changed-file scopes, using lexical paths relative to the same
project root. A created file appears only in `after`; a deleted file appears
only in `before`. Include the previous **accepted** snapshot, never a rejected
buffer. Unchanged files outside the scope need not be supplied. Supplying the
whole project on every save defeats the intended per-edit cost. Names are
checked per file, just as in EXP-119; definitions in another file do not exempt
an unknown bare call.

`Admission::Reject` carries a typed `AdmissionRule` and a path/name diagnostic.
`Class` is `LiveEligible { changed_fns }`, `Restart { reason }`, or
`Stock { reason }`. Reasons are typed and expose stable `code()` strings.
`ChangedFn` contains a path and an inline-module-qualified free-function name
(`::answer`, `module::answer`). Order is deterministic. Identical tokens and
comment/whitespace edits produce an empty list. This list is informational;
rustd remains responsible for discovering compilation dependencies.

## Admission policy

The EXP-119 rules run in this order over the whole supplied Rust scope:

1. Parse both snapshots with syn (also validate UTF-8 and reject duplicate paths).
2. Reject a newly appearing `(enclosing fn name, expression name)` whose name is
   a strict prefix of a baseline identifier or current free-function name,
   unless the name itself names a current free function.
3. Reject a newly missing tail in an explicit non-unit free function or impl
   method. A final expression without a semicolon or an explicit `return`
   satisfies this check. Existing violations are subtracted.
4. Reject a new bare reference to a locally defined free function.
5. Reject a new unqualified bare call without a local free-function definition.
6. Reject a removed baseline free function (including inline modules).

This is scheduling policy, not semantic validation. The deliberately preserved
EXP-119 false rejects include valid local names that prefix known identifiers,
`fn n() -> u8 { panic!(); }`, and function-pointer uses. A `u8` function ending
in a string literal is deliberately accepted. Imported functions, local closure
calls, and owner-name collisions also expose the heuristic's limits. Stock
rustc remains the type/borrow-checking authority.

## Classifier policy

Priority is global across the supplied scope: changed configuration → `Stock`
(`manifest`); Rust file additions/removals → `Restart` (`file-set`); changed
structure or impl hazards → `layout`; changed async free-function body →
`async`; changed existing free-function policy fingerprint → `macro-body`;
unsupported new free function → `new-complex-fn`; otherwise live-eligible.

Every non-Rust snapshot is conservatively a configuration input. `build.rs` is
also a configuration input despite its suffix. Thus `Cargo.toml`, `Cargo.lock`,
`build.rs`, and `.cargo/config.toml` changes/additions/removals always take the
stock/recapture route, fixing classify109's dead manifest branch. With a stable
Rust file set, invalid Rust passed directly to `classify` returns `Stock(Parse)`; normally admission has
already rejected it. Duplicate paths return `Stock(InvalidSnapshots)`.

The strict M0 (`M0_CERTS=0`) syntax policy is unconditional:

- Async free-function bodies keep **all body tokens**, including pre/post-await
  strings, await points and cross-await locals. Comments/spacing are not tokens.
- Async-block body tokens, opaque macro token streams, closures, and nested items are
  hazards. A changed hazard in a free function restarts. The prototype captures
  an async block's block tokens, rather than its outer `async move` prefix.
- A free function's header contains its signature, visibility, and attributes.
  Any header change restarts. `main` and any function with an attribute other
  than `doc` or builtin `inline(never)` also protect the entire body. The `macro-body` reason covers these broader
  header/local-item/closure/attribute hazards, matching the prototype policy.
- Impl-method bodies are erased from structure. Their headers remain. An
  ordinary method's literal/arithmetic body edit can therefore go live, with
  **no entry in `changed_fns`**. Its closures, local items, macro bodies and async
  blocks are collected into the file's structure hazards. An async method or a
  method with an attribute other than `doc`/`inline(never)` adds its entire body to those hazards. Changes
  to these hazards restart as `layout`, before free-function async checks.
  Trait default-method bodies remain in structure and also restart as `layout`.
- New free functions must have no attributes other than `doc`/`inline(never)`, unsafe/async/extern marker,
  generic parameters, or body hazards. A new ordinary `const fn` is allowed.
  Existing generic/unsafe/extern free-function bodies follow the usual body rule.
- Structure preserves uses, consts/statics, types, impls, traits and file docs,
  and removes free functions. **Changed imports restart**: this follows the
  actual prototype despite the phase prose's broader statement about imports.

Standard expression macros have a closed allowlist: `format`, `format_args`,
`print`, `println`, `eprint`, `eprintln`, `write`, `writeln`, `panic`, `assert`,
`assert_eq`, `assert_ne`, `debug_assert`, `debug_assert_eq`, `debug_assert_ne`,
`unreachable`, `todo`, `unimplemented`, `vec`, `matches`, `dbg`, `concat`,
`stringify`, `line`, `column`, `file`. Bare and `std::`/`core::` forms qualify.
Nested opaque macros, item/closure/async tokens retain the hazard. Known macro
shadowing disables bare-name exceptions. Tracing event macros retain the opaque
rule; tracing metadata still needs the controller's separate extraction policy.

Live eligibility is permission to try rustd and extraction, not proof that a
patch will succeed. Their refusals must still take the restart/stock ladder.

## Cache and resource bounds

Analysis is cached by `(lexical path, BLAKE3(bytes))`, shared between admission
and classification. Hits still hash bytes and compare derived fingerprints;
they do not parse, walk ASTs, or regenerate tokens. Parse failures are cached.
No ASTs/spans are retained. Default LRU limits are 128 versions and 16 MiB of
conservative derived-data weight. `with_cache_limits`, `clear_cache`, and
`cache_stats` support session lifecycle, eviction tests, and measurements.
Oversized entries are analyzed without retention. The byte counter is an
accounting estimate of derived strings/collections, not a process RSS limit.

For large files (at least 8 KiB) with multiple top-level inline modules, a
compiler lexer identifies byte boundaries and at most three scoped workers
parse independent module scopes while the calling thread parses the outside
file. Every scope still goes through syn. Raw module source, exact structure,
qualified function names, and impl-hazard visitation order are preserved. No
AST or span crosses a thread boundary. Small files and uncertain partitions
use the serial parser. Unknown lexical forms, newer literal prefixes (including
C/guarded strings), BOM/shebang, and mismatched delimiters also use serial syn.
The lexer is only a partitioning aid; it never grants admission. Worker creation
failure uses the serial path. Workers have 8 MiB stack reservations and exit
after analysis; there is no extra persistent process or daemon. The outline
removes direct free functions in one pass. Differential tests retain the
prototype's placeholder/removal pass as an independent outline oracle.

The optimized and serial analyzers are compared field-for-field on all 134
frozen source blobs, the benchmark source, and grammar/hazard-order edge cases.
Another 512 deterministic partial-buffer mutations check parse acceptance and
the complete analysis against serial syn. A threshold-crossing comment edit
checks that switching parse strategies does not change the classification.
The proc-macro support features of syn/quote are disabled: this library analyzes
ordinary source outside the compiler's procedural-macro environment.

## Verification

```sh
RUSTUP_TOOLCHAIN=1.99.0 nice -n 19 cargo test -p refresh-guard -j 4
RUSTUP_TOOLCHAIN=1.99.0 nice -n 19 cargo clippy -p refresh-guard --all-targets -j 4 -- -D warnings
```

The workspace's `scripts/test.sh` already includes this crate. Portable cases
need no research checkout or Python: 64 web transitions, 35 actual fixture S1–S8
edits, 25 EXP-055 async edits, and 119 manually specified policy boundaries plus 98 standard-expression-macro cases.
Each case runs cold and cached. Fourteen additional API tests cover cache sharing,
keys, bounds, parse failures, invalid input, thread transfer, and the free API.
The corpus is in `tests/cases/`; source blobs are named by SHA-256 and provenance
hashes identify the original inputs. Fixtures and research history are untouched.

For read-only comparisons against retained research binaries:

```sh
RUSTUP_TOOLCHAIN=1.99.0 nice -n 19 cargo build -p refresh-guard --release --example guard_probe -j 4
python3 crates/refresh-guard/tests/compat/check.py --research /path/to/rust-fast-compiler \
  --probe "$PWD/target/release/examples/guard_probe" --out /tmp/guard-compat.json
python3 crates/refresh-guard/tests/compat/stock.py --out /tmp/guard-stock.json
python3 crates/refresh-guard/tests/compat/bench.py \
  --prototype /path/to/rust-fast-compiler/work/m0/v4/gw/bin/refresh-guard-m0 \
  --probe "$PWD/target/release/examples/guard_probe" --out /tmp/guard-bench.json
```

`check.py` executes the strict binary and the actual `source_guard.classify109`
logic, refusing async owners as the certificate-off controller does. Web save
54 is a parse rejection; save 55 compares against the last successful AST.
The expected live/restart paths come from M0 smoke-c events, independently of
the port. `stock.py` checks the annotated std-only admission boundaries against
stock rustc 1.99.0. `bench.py` records 30 AB pairs per mode, waits for load1 ≤ 2,
and checks available RAM ≥ 6 GiB on each pair. Input JSON/IPC are outside the
Rust timer. It reports classifier-only time (including shared name/outline
analysis) separately from combined admission/classification time.
Combined modes compare one uncached analysis, two uncached versions, a primed baseline
plus edited version, and fully cached decisions separately. The prototype arm
includes process startup; ratios describe that concrete architecture comparison.

`freeze.py --research ...` regenerates research-derived cases; `boundaries.py`
regenerates the manually reviewed table. These are maintenance tools, not part
of `cargo test`. They disable Python bytecode writes to research directories.

Internal parser/visitor panics are contained at every `Guard` entrypoint. Admission
returns `internal-error`; classification returns `Restart(InternalError)`. Both
log the panic payload and clear partially populated syntax caches. The controller
handles either result with a stock Cargo build and process restart, so an internal
analysis failure cannot leave a save waiting indefinitely or crash its worker.
Underscore imports (`use Trait as _`) never shadow a macro name; raw aliases such
as `r#println` do participate in the conservative shadowing policy.
