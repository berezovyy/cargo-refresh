Five cumulative regressions cover a root-level `include_str!` input, changing its
path, editing the newly discovered file, introducing `include_bytes!`, and
editing that newly discovered byte input. Every step must produce a stock save
and the exact stock output; a timeout or an unchanged probe fails.

```sh
python3 tests/dep-inputs/run.py --stock --work /tmp/dep-inputs-stock
python3 tests/dep-inputs/run.py --controller /path/to/cargo-refresh --support /path/to/support --work /tmp/dep-inputs-refresh
```

The driver reuses the semantics adapter and acquires the research lanes and
memory guard. Keep the work directory to reuse warm targets.

Introduced input names are unique per run so an old dep-info file cannot make a
new dependency accidentally pre-watched. The source/target directory stays stable
to preserve warm Cargo artifacts.
