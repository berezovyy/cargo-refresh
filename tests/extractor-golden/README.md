# Python extractor goldens

The large ELF fixtures are kept outside git at
`rust-fast-compiler/work/m2-t3/goldens/`. Each numbered directory contains:

- `input.json`: baseline name, tracing opt-in, function catalog, previous
  applied jumps/data/pages (before this live apply).
- `*.manifest.gz`: ordered compiler deltas needed to reconstruct current state.
- `candidate.elf.gz`: the candidate's program headers, dynamic tables and
  unwind source (full ELF, gzip compressed).
- `expected.wire`: actual Python `ManifestExtractor.send()` output.
- `result.json`: successful agent acknowledgement and Python metadata.

Baselines are shared compressed ELF files at the golden root. The capture
copies the extractor's normalized immutable baseline, so restarting the
application cannot change an earlier fixture. RELATIVE normalization is
idempotent when the Rust reader opens that copy.

Capture instrumentation lives next to this file (`sitecustomize.py` and
`capture.sh`); the actual task run uses copies in `work/m2-t3/scripts/`. It changes
neither research experiment files nor the M0 Python sources. To reproduce:

```sh
tests/extractor-golden/capture.sh /absolute/rust-fast-compiler /fresh/output
```

The script takes the rustd lane itself and refuses existing golden/session
destinations. Capture runs the existing M0 web session with its strict certificate-off
guard and no-certificate compiler, prepends a handler apply/revert pair, then
runs the first 26 ordinary saves. The resulting 28-source replay includes new
helpers and comment/line shifts. Stock replay uses the same generated steps.
Run the capture script under the exclusive rustd lane. Reproduction requires
fresh output destinations; do not overwrite earlier evidence.

The ignored Rust test loads every successful live capture, reconstructs the
manifest and previous acknowledged state, then compares the complete encoded
wire bytes. The normal suite separately checks commit/rollback semantics across
successive generations and rejects stale/out-of-order prepared patches.

For extraction timing, build the release test first, then acquire the rustd
lane, wait for load1 <= 2, record `/proc/loadavg` and `MemAvailable`, and run the
ignored golden test with `REFRESH_TIMINGS=1` and `--test-threads=1`. The timed
region includes the latest manifest's parse/update, candidate ELF header parse,
policy checks, page diff/restores, unwind descriptors, and wire encoding. It
excludes file reading/decompression, baseline indexing, and prior-generation
replay. One sample per golden; report median, min/max and n. The historical
Python ~48 ms controller preparation residual has a different boundary, so it
is contextual, not an A/B speedup measurement.
