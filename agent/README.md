# cargo-refresh agent

Linux/x86-64 in-process patch agent, ported from EXP-128's `gw/scripts/agent.c`
and its EXP-121/122/123 headers. The [protocol](PROTOCOL.md) defines configuration,
wire format, error codes, transaction guarantees and parking/pressure behavior.

Run from this directory:

```sh
nice -n 19 make
make test
make test-asan
make test-equivalence
make test-robustness test-panic test-pressure
```

`make test` builds C parser/JSON and unwind tests plus a tiny LD_PRELOAD host.
It compares patched behavior with a stock gcc build, checks rollback/recovery,
stalled/disconnected clients, and applies 200 patches with four busy callers.
`make test-asan` runs the same tests with ASan+UBSan (including the host and both
agent variants) in a separate `build-asan/` directory with leak detection enabled.
It also runs the creation/raw-clone and C++ panic stresses. The original suite
keeps ASan first in LD_PRELOAD; the added stress puts the instrumented agent
first, with ASan's link-order check disabled. Otherwise ASan's outer pthread
wrapper can hold its lifecycle mutex and block signals before entering the
agent's creation gate, causing persistent safe park refusals under the storm.
This preload-order limitation is documented rather than counted as patch success.
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
Experimental root scanning and controlled GC remain behind REFRESH_TESTING;
they are research diagnostics, not production GC. Production frame registration
uses agent-owned storage before parking, with no gap in unwind coverage.

`test-robustness` applies 1,000 patches during a C thread-create/exit storm and
EINTR-retrying epoll/nanosleep loops, then checks immediate raw-clone refusal.
`test-panic` applies 200 patches through newly registered FDEs while two C++
threads throw/catch continuously. `test-pressure` registers all 512 near copies,
checks warning/cap accounting, and verifies a real heap-mapping refusal.
`test` also checks a synthetic splice PT_LOAD, 20 subsequent patches, rollback
of a delayed signal-handler timeout, and the old forward-unwind-slot collision.

See [collision evidence and compiler proposal](docs/COLLISION.md). An agent
cannot relocate a legitimate application mapping when rustd chose its address;
those collisions remain safe MMAP_FAILED refusals with VMA diagnostics. Compiler
placement negotiation/reservation, controller epoch scheduling, the complete
60-case Rust panic matrix and symbolized backtraces remain follow-up work.

`make test-controlled-gc` is a separate optional smoke for the preserved E123
controlled leaf collector's page-index/unwind-counter bookkeeping. Its fixture
promises no escaping old-generation pointers. This is not production collection;
conservative false positives can retain pages. Do not run this full writable-root
scan under ASan, whose shadow mappings are also writable roots.

M3-T3 reserves a 1 GiB code region before allocating and accepts new generation
pages only inside it. Replies include region availability, bounds and retained
bytes. Sparse splice ELF notes preserve the base region across restarts; inherited
PT_LOADs are adopted automatically. See [PROTOCOL.md](PROTOCOL.md#reserved-code-region-m3-t3--exp-122-exp-126).
Run `make test-region` for reservation, 320 MiB heap allocation, ownership/bounds,
and occupied-startup refusal tests; it is also included in `make test`/`test-asan`.
