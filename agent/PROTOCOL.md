# Agent protocol v1

Linux/x86-64, little endian, one request and one newline-terminated JSON reply
per Unix stream connection. All integers are unsigned unless stated otherwise.
The controller must create a unique per-run socket directory with mode **0700**;
the agent binds the supplied path and does not create that directory. It accepts
only peers whose `SO_PEERCRED` UID equals the application's `getuid()`.
Accepted sockets have 5-second `SO_RCVTIMEO` and `SO_SNDTIMEO` settings. These
are per-I/O inactivity timeouts, not a whole-message deadline. A disconnected
client cannot signal the app with SIGPIPE. Requests are processed serially.

## Configuration

New names take precedence, including when the new value is empty. Legacy
fallbacks are accepted for one release. Using any fallback produces **one**
deprecation line per active process, rather than one line per variable.

| Variable | Legacy fallback | Meaning |
|---|---|---|
| `REFRESH_AGENT_SOCKET` | `E077_AGENT` | Socket path; absent/empty disables the server. |
| `REFRESH_HOST_EXE` | `E106_HOST_EXE` | Exact absolute `/proc/self/exe` path. Required when using the new socket variable, preventing activation in inherited child executables. |
| `REFRESH_AFTER_PATCH` | `E099_AUTO_HOOK` | `1` calls `fast_refresh_after_patch` after a successful APPLY. |
| `REFRESH_DEBUG` | `E121_DEBUG` | Nonzero enables the prototype ptrace diagnostics. |
| `REFRESH_INHERITED` | `E106_INHERITED_SEGMENTS` | Rename recognized; adopting these segments remains M3 §3.5 work. Existing mappings are still protected by MAP_FIXED_NOREPLACE. |
| `REFRESH_PARK_SIGNAL` | none | Decimal signal number, default `SIGRTMIN+7`. Invalid/unusable signals disable agent startup; an existing handler is warned about. |

Release uses hybrid IP/frame-pointer/conservative-stack checks. `E084_MODE`,
`E054_SAFEPOINT`, `E121_DISABLE_UNWIND`, all `E123_*` controls and failure
injection are confined to `REFRESH_TESTING` builds. The exported
`e054_tick_enter/leave` compatibility functions remain available. Thread-creation
races, second task enumeration and EINTR qualification remain M3 §3.4 work.

## Header

The v1 header is exactly 24 bytes, without C padding:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 4 | ASCII `RFR1` (u32 value `0x31524652`) |
| 4 | 2 | Version, exactly `1` |
| 6 | 2 | Flags/message type: `0` APPLY, `1` PING, `2` HOOK |
| 8 | 4 | Record count, at most 8192 |
| 12 | 4 | Transaction ID, echoed verbatim in the reply |
| 16 | 8 | **total_len, including this 24-byte header** |

`total_len` must be between 24 and 268435456 (256 MiB). Oversize headers are
rejected before reading any body. For APPLY it must exactly equal 24 plus the
sum of all record headers, payloads and fixup offsets. PING and HOOK require
count=0 and total_len=24. Unknown versions/flags are refused. Transaction IDs
are correlation IDs; requests are not deduplicated.

PING reports the current generation without applying or invoking a callback.
HOOK explicitly invokes the optional `fast_refresh_after_patch` function,
regardless of `REFRESH_AFTER_PATCH`, without changing the generation. Missing
callbacks produce `INTERNAL`. An empty APPLY retains v0 behavior: it is an
accepted transaction and advances the generation; use PING for a status query.

## Records

Each record is a 28-byte header followed by `length` payload bytes and then
`nfix` little-endian u32 offsets:

```
u64 address | u64 length | u32 prot | u32 kind | u32 nfix
u8 payload[length]
u32 fixup_offsets[nfix]
```

Addresses are main-executable link-time virtual addresses; the agent adds its
PIE load base. Payload fixups add that same base to the little-endian u64 at
each offset. Offsets must be ascending, nonoverlapping, and wholly within the
payload (`offset+8 <= length`, `nfix <= length/8`). Address, range, page-rounding
and fixup addition overflows are refused. Length must be nonzero and at most
128 MiB. Only Linux protection bits R=1, W=2, X=4 are accepted; R is required.

| Kind | Contract |
|---:|---|
| 0 | Fresh appended code/data. MAP_FIXED_NOREPLACE never replaces application mappings. No page may overlap a previously committed generation. Records may share a fresh page only with identical final protections; payload writes occur in record order. |
| 1 | Existing entry write/restore: exactly 5 bytes, prot=5, no fixups. Every affected page must be original executable `r-xp` memory of the main executable (inode **and device** checked). |
| 2 | Original immutable data: at most 4096 bytes, prot=1. Every affected page must be original `r--p` memory of the main executable. A record may straddle two pages. |
| 3 | Unwind descriptor: length=8, prot=1, no fixups. Payload is a u64 source span (8 bytes–1 MiB), address points into retained/new patch pages. A terminated near copy is validated and rebased. |

Kind 4 is reserved for the M3 §4 symbol-blob implementation and currently
returns BAD_PROTOCOL; the controller must omit it until that work lands.
The generation owns its pages permanently after commit. Limits are 65536
retained pages (256 MiB), 8192 retained unwind copies and 512 possible near-copy
locations, spaced 2 MiB apart at load_base+256 MiB. No session reclamation or
inherited-segment adoption is provided by this task.

## Transaction semantics

Before parking, the agent validates original mapping permissions, allocates the
undo log, maps/fills/fixes up all kind-0 pages and applies their final protections.
It creates and registers private unwind copies while new code is unreachable.
Registration can allocate or lock, so it occurs **before** parking. Refused
copies are deregistered **after** releasing threads. This also avoids publishing
entry jumps before their unwind coverage exists.

While parked, only kind-1/2 bytes are saved to the preallocated undo log and
modified. Original page protections are restored after each write. A commit
failure reverses all touched writes in reverse order and restores each original
page's permissions before release. Staged pages/copies are then discarded;
`ROLLED_BACK` means the previous generation is intact, with no callback and no
generation increment. A failed rollback mprotect is fail-stop (`exit_group(127)`),
never a false ROLLED_BACK acknowledgement. Ordinary preflight refusals preserve
the application and return their precise error code. The next request can reuse
the failed transaction's fresh addresses.

No production path from the first park signal through release calls allocation,
stdio, getenv, dlsym or libgcc/loader locks. The audited call graph is recorded at
the top of `src/park.c`. The library uses eager dynamic binding (`-z now`) and
exports only `pthread_create` and the two compatibility tick functions; its
internal state cannot interpose on identically named host symbols.

## Replies

Every reply has `ok`, `code`, escaped `message`, legacy `error` (the same escaped
text), `txid`, `generation`, and the existing apply/stack/unwind/callback counters.
Successful requests use code `OK` and empty message/error strings. `generation`
is the count of successful APPLY messages; PING/HOOK and refusals do not advance
it. `patch_count` is the compatibility alias. A transport disconnect does not undo
an already committed transaction: the controller must reconcile ambiguous ACKs.

Example:

```json
{"ok":false,"code":"ROLLED_BACK","message":"commit failed; original bytes and protections restored","error":"commit failed; original bytes and protections restored","txid":42,"generation":3}
```

| Stable code | Meaning |
|---|---|
| `OK` | Request succeeded. |
| `BAD_PROTOCOL` | Malformed/truncated/timed-out input, unsupported fields, invalid existing entry target or conflicting fresh-page permissions. |
| `NOT_ORIGINAL_RO` | Kind-2 target is not validated original immutable memory. |
| `OVERLAP` | Kind-0 pages overlap a retained generation. |
| `PAGE_LIMIT` | Retained-page budget exhausted. |
| `UNWIND_LIMIT` | Retained unwind-copy budget exhausted. |
| `MMAP_FAILED` | Fresh or near unwind mapping failed. |
| `UNWIND_UNAVAILABLE` | libgcc registration entry points are unavailable. |
| `UNWIND_INVALID` | Unwind span, source or encoding is invalid/unsupported. |
| `PARK_TIMEOUT` | Parking timed out or exceeded the thread limit. |
| `STACK_COLLISION` | A parked IP/stack intersects the write hazards after retries. |
| `MPROTECT_FAILED` | Staging/final-protection preparation failed before commit. |
| `ROLLED_BACK` | Commit failed or a test fault was injected; previous application state is intact. |
| `INTERNAL` | Allocation/map inspection failure or requested hook is absent. |

The controller may retry STACK_COLLISION/PARK_TIMEOUT once after 50 ms, then
restart; other errors select restart. Pressure metrics/epoch policy are deferred
to M3 §3.6. Escaping covers quotes, backslashes, control bytes and non-ASCII bytes
(the latter as `\u00xx`), keeping every reply valid JSON. Numeric formatting uses a thread-local C numeric
locale only during reply formatting, preserving the application's locale.

## v0 compatibility

The old 8-byte header `u32 magic=0x76313232 | u32 count` uses the same records,
fixups and new transaction implementation. It has a cumulative 256 MiB wire
budget but cannot verify an advertised total length. The old empty HOOK header
`u32 magic=0xe099ca11 | u32 count=0` remains accepted. Replies include the v1
code/message fields for both versions; v0 txid is zero. Old EXP-121 magic
`0x76313231` is intentionally unsupported.

## Test-only injection

Build with REFRESH_TESTING (`build/librefresh-testing.so`). Set
`REFRESH_TESTING_FAIL_AT` to a zero-based record index. Kind 0 fails after new
pages have been filled/protected; kind 3 after staged registration; kinds 1/2
after their write/protection restore. `REFRESH_TESTING_FAIL_PHASE=write|restore`
additionally exercises the before-write and before-final-protection failure
branches for existing writes. The integration host can clear these variables
and prove a later good patch succeeds. Release ignores all injection variables.
