# Recorded results

The final compiler was qualified for correctness on October 24, 2026. Its speed
has **not** been benchmarked. Earlier-build timings are kept separately and clearly
labelled. Public packaging changes have their own clone/demo checks.

| Claim | Explanation | Data |
|---|---|---|
| Safety matrices: 125/125 and 50/50 | [Correctness](correctness.md) | [125 cases](data/safety-125.jsonl), [50 cases](data/safety-50.jsonl) |
| Semantics: 22/22 edit outputs, both profiles | [Scope and partial case](correctness.md) | [Committed profile](data/semantics-committed.json), [default profile](data/semantics-default.json) |
| Macro campaign: 164/164 exact comparisons | [Correctness](correctness.md) | [Per-comparison outputs](data/macro-comparisons.jsonl) |
| Assertion printer: 3,916/3,916 byte-identical | [Correctness](correctness.md) | [Per-edition raw test output](data/assertion-printer.json) |
| Web/Conduit/Atuin: 104/104 stock checks; 77 live, 24 restart | [App sessions](correctness.md) | [Web](data/app-web.json), [web totals](data/web-completed.json), [Conduit](data/app-conduit.json), [Atuin](data/app-atuin.json) |
| 500 Rust hunks: 12 live; 39 whole-commit saves: zero live | [History and reasons](correctness.md) | [Hunk rows](data/history-hunk-rows.csv), [commit rows](data/history-commit-rows.csv), [summary with reasons](data/history-both-modes-summary.json), [identity comparison](data/history-comparison.json) |
| Earlier-build per-app save→visible comparisons | [Speed and caveats](speed.md) | [Paired raw rows](data/earlier-speed-pairs.jsonl), [per-edit statistics](data/earlier-speed-summary.json), [toolchain](data/earlier-speed-environment.json), [resources](data/earlier-speed-resources.jsonl), [binary identities](data/earlier-speed-provenance.json) |
| Loaded correctness run: controller median 144 ms, compile 114 ms | [Diagnostic phase breakdown](speed.md) | [Raw phase timestamps, load and RAM](data/loaded-web-phases.json) |
| Clone quick start, stock checks, 183 product tests and source support build | [Clone check](clone-check.md) | [Verification records](data/clone-verification.json) |
| Source versions and export scope | [Qualification scope](correctness.md) | [Source metadata](data/qualification-sources.json) |

Small evidence exports retain numeric observations, verdicts and relevant outputs.
Developer-local paths are replaced with placeholders; large logs/commands and some
unneeded event fields are omitted. These are recorded checks with stated scope,
not a Rust soundness proof or a claim that complete upstream suites passed.
Use `./try.sh --smoke`, `scripts/test.sh`, and [bench/run.sh](../bench/README.md)
for checks and new measurements from this checkout.
