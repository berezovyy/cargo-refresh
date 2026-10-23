# cargo-refresh agent

Linux/x86-64 in-process patch agent, ported from EXP-128's `gw/scripts/agent.c`
and its EXP-121/122/123 headers. The [protocol](PROTOCOL.md) defines configuration,
wire format, error codes, transaction guarantees and the remaining M3 work.

Run from this directory:

```sh
nice -n 19 make
make test
make test-asan
make test-equivalence
```

`make test` builds C parser/JSON and unwind tests plus a tiny LD_PRELOAD host.
It compares patched behavior with a stock gcc build, checks rollback/recovery,
stalled/disconnected clients, and applies 200 patches with four busy callers.
`make test-asan` runs the same tests with ASan+UBSan (including the host and both
agent variants) in a separate `build-asan/` directory with leak detection enabled.
No rustd, Cargo build, research-fixture mutation or network service is required.
`make test-equivalence` runs the original v0 protocol cases against the frozen
research library and both current split libraries. The original pure-refactor
commit was also verified with these same cases before protocol/apply changes.

The Makefile links with eager symbol binding and hides private module symbols;
release exports only the pthread_create interposer and compatibility tick hooks.
Build products are ignored under `build/` and `build-asan/`; `make clean` removes
only those directories.

The original libunwind headers are retained in `src/unwind-include/` with their
upstream license notices; test CFI mode links the system libunwind libraries.
Experimental root scanning, alternate registry strategies and controlled GC code
remain behind REFRESH_TESTING; they are research diagnostics, not production GC.

Deferred: M3 §3.4 thread-creation/parking robustness and EINTR tests, §3.5 inherited
segment adoption, §3.6 pressure/epochs, §3.7 full panic/unwind stress qualification,
and §4 symbol blobs/backtraces. Unwind registration was moved to unreachable
staging before parking in this task to satisfy the allocation audit.
