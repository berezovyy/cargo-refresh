# Correctness qualification

Recorded October 24, 2026, Rust 1.99.0 on a shared Linux x86_64 host. These are correctness/path counts, not
benchmarks.
[Exact source identities and export scope](data/qualification-sources.json).
The final compiler is `b6e0d81`, frontend `06dd1e3`. App/history qualification used
controller `edade61`; the combined installation tree and focused probes were also
checked. The assertion printer corpus uses the same unchanged frontend printer.
Subsequent public packaging changes do not imply a full repeat of this campaign.

## Safety, semantics and macros

| Check | Recorded result | Evidence |
|---|---|---|
| Async safety matrix | 125/125 pass: 30 live, 95 restart-state | [Rows](data/safety-125.jsonl) |
| Adversarial async matrix | 50/50 pass, all restart-state | [Rows](data/safety-50.jsonl) |
| Runtime semantics, committed profile | 22/22 edit outputs pass; 20 configurations pass, one partial; zero output failures | [Cases](data/semantics-committed.json) |
| Runtime semantics, default profile | 22/22 edit outputs pass; same partial case | [Cases](data/semantics-default.json) |
| Reduced macro campaign | 164/164 exact comparisons; 40 live, 116 restart, 8 stock | [Raw compared outputs](data/macro-comparisons.jsonl) |
| Assertion messages vs stock rustc | 3,916/3,916 byte-identical panic payloads | [Raw per-edition test output](data/assertion-printer.json) |

`restart-state` describes the safety harness restoring its own checkpoint;
cargo-refresh does not persist arbitrary application heap state across restarts.

The semantics partial case needs a session relaunch for changed external environment
variables. Six conservative path differences per profile are distinct from output
correctness. Unsupported mixed-edition/proc-origin assertion items remain native;
edits affecting them may require Cargo. No general arbitrary proc-token admission
claim is made. Assertion editions 2015/2018/2021/2024 have 949/969/989/1009 cases.

## Running applications

| App | Stock-reference checks | Live | Restart | Stock | Wait | No-op |
|---|---:|---:|---:|---:|---:|---:|
| Web fixture | 64/64 | 46 | 16 | 0 | 1 | 1 |
| Conduit | 30/30 | 21 | 8 | 0 | 1 | 0 |
| Atuin | 10/10 | 10 | 0 | 0 | 0 | 0 |

[Web records](data/app-web.json), [Conduit records](data/app-conduit.json),
[Atuin records](data/app-atuin.json). All 104 checks matched their stock references;
77 updates went live and 24 restarted. Wait/no-op cases are not live updates.
Checks cover selected HTTP status/body/stable headers or command output, with
incidental dates and ports normalized. They do not establish complete application
API, changed-function coverage, or full upstream test-suite success.

## Real-history replay

The same frozen corpus contains 500 individual Rust hunks. Whole-commit mode
applies complete diffs including non-Rust context. No-ops are not live patches;
intermediate error/wait outcomes are not completed-commit mismatches.

| Mode | Rust saves | Live | Restart | Stock | No-op | Error/wait |
|---|---:|---:|---:|---:|---:|---:|
| Individual hunks | 500 | 12 (2.4%) | 31 (6.2%) | 57 (11.4%) | 275 (55.0%) | 125 (25.0%) |
| Whole commits | 39 | 0 (0%) | 4 (10.26%) | 33 (84.62%) | 2 (5.13%) | 0 |

Per-app counts, exact reasons and upstream commit pins are in the
[summary](data/history-both-modes-summary.json), [500 hunk rows](data/history-hunk-rows.csv)
and [39 commit rows](data/history-commit-rows.csv).
The hunk replay completed 99/99 primary commits matching stock; whole-commit mode
completed 101/101. The latter includes trailing partial Atuin and Meilisearch
commits. A separate Conduit successor matched 1/1 completed commit in each mode,
for 202 completed-commit comparisons across both modes plus Conduit.
Meilisearch's 205-hunk prefix is incomplete and contributes no completed hunk commit.
Its edits target excluded tests; no live capability is inferred from those no-ops.

Top remaining reasons, Rust saves only:

| Path/reason | Hunks | Whole commits |
|---|---:|---:|
| Stock: outside editing frontier | 39 | 17 |
| Stock: compiler unavailable/recovering | 7 | 4 |
| Stock: Cargo configuration change | 2 | 9 |
| Stock: tracing scalar/discriminant relink | 4 | 1 |
| Stock: no fresh manifest | 2 | 1 |
| Stock: compiler transaction failed | 1 | 1 |
| Stock: listenfd symbol link failure | 1 | 0 |
| Stock: application exited | 1 | 0 |
| Restart: macro definition change | 12 | 2 |
| Restart: types/layout change | 11 | 2 |
| Restart: compiler baseline recovery | 6 | 0 |

Hunk errors include 35 E0425, 31 E0061, 13 E0599, 12 E0308, 7 E0433 and six syntax
waits; consult the CSV for all reasons. There are 237 excluded-input no-ops and 38
proved no-codegen no-ops. Context-only saves are reported separately: hunk mode
has 31 additional watched context saves (27 stock, four errors); whole-commit mode
has 62 context-only commits, 11 watched events (ten stock, one missing-file error).
All context-only commits were stock-compared even when no watched save occurred.

Whole-commit live coverage remains the main gap: **0/39**. Known remaining issues
include native listenfd link resolution, a rust-analyzer missing-AST-ID transaction
abort, and a context missing-file error. Fallbacks preserved stock-matching completed
commit smoke output. Finite smoke observations do not prove every affected function.
