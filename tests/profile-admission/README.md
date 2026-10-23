The matrix checks all four combinations of `debug-assertions` and
`overflow-checks` against stock, before and after a live leaf edit. It observes
`cfg!(debug_assertions)`, a caught `debug_assert!` panic, and a caught runtime
integer overflow. Profile settings are applied to disposable copies of the
committed fixture.

```sh
python3 tests/profile-admission/run.py --stock-only --work /tmp/profile-admission
python3 tests/profile-admission/run.py --refresh-only --controller /path/to/cargo-refresh --support /path/to/support --work /tmp/profile-admission
```

The driver reuses the semantics adapter's lanes, process ownership checks, and
memory guard. Omit the mode switches to run both oracles in one invocation.
