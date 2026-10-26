The matrix checks all four combinations of `debug-assertions` and
`overflow-checks` against stock, before and after a live leaf edit. It observes
`cfg!(debug_assertions)`, a caught `debug_assert!` panic, and a caught runtime
integer overflow. An additional `strip = "symbols"` case must also patch live,
so loss of the ELF symbols needed by the patch pipeline cannot silently pass.
Profile settings are applied to disposable copies of the
committed fixture.

```sh
python3 tests/profile-admission/run.py --stock-only --work /path/to/work/profile-admission
python3 tests/profile-admission/run.py --refresh-only --controller /path/to/cargo-refresh --support /path/to/support --work /path/to/work/profile-admission
```

The driver reuses the semantics adapter's lanes, process ownership checks, and
memory guard. Omit the mode switches to run both oracles in one invocation.
