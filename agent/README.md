# cargo-refresh agent

Linux/x86-64 in-process patch agent, ported from EXP-128's `gw/scripts/agent.c`
and its EXP-121/122/123 headers. `src/` separates the server, wire protocol,
parking, hazard inspection, page apply, and unwind preparation.

Build: `make`. Run the small local regression hosts: `make test`. Compare the
ported implementation against the frozen research source: `make test-equivalence`.
The latter does not need rustd or modify any research fixture.

The original libunwind headers are retained in `src/unwind-include/` with their
upstream license notices; linking uses the system libunwind shared libraries.
Experimental root scanning and controlled collection are test-only.
