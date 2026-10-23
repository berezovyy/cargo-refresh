#!/usr/bin/env python3
"""Interleaved prototype subprocess / in-process guard benchmark (ABAB)."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import time
from reference import Probe


def machine():
    return dict(loadavg=Path('/proc/loadavg').read_text().strip(),
                available_bytes=int(next(line.split()[1] for line in Path('/proc/meminfo').read_text().splitlines()
                                         if line.startswith('MemAvailable:'))) * 1024)


def settle():
    start = time.monotonic()
    while True:
        state = machine()
        if state['available_bytes'] < 6 * 1024**3:
            raise MemoryError('MemAvailable below 6 GiB')
        if float(state['loadavg'].split()[0]) <= 2:
            return state
        if time.monotonic() - start > 600:
            raise TimeoutError('Load did not settle to <= 2 within ten minutes')
        print(json.dumps(dict(waiting_for_load=state)), flush=True)
        time.sleep(5)


def summary(values):
    return dict(median=statistics.median(values), min=min(values), max=max(values), n=len(values))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prototype', type=Path, required=True)
    parser.add_argument('--probe', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--pairs', type=int, default=30)
    args = parser.parse_args()
    source = (Path(__file__).resolve().parent.parent / 'cases/web-handlers-1733.rs').read_text()
    assert len(source.splitlines()) == 1733
    # Both arms see the exact same source. B computes changed_fns and classifies;
    # combined modes also time admission. A is the analyzer subprocess alone.
    before = {'handlers.rs': source}
    after = {'handlers.rs': source.replace('"S1:v1"', '"S1:bench"', 1)}
    probe = Probe(args.probe)
    env = dict(os.environ, M0_CERTS='0')
    rows = []
    summaries = {}
    cpu = next(line.split(':', 1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines()
               if line.startswith('model name'))
    def persist():
        result = dict(summaries=summaries, rows=rows, source_sha256=hashlib.sha256(source.encode()).hexdigest(),
                      source_lines=1733, source_bytes=len(source.encode()), cpu=cpu, logical_cpus=os.cpu_count(),
                      platform=platform.platform(), utc=time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
                      rustc=subprocess.check_output(['rustc', '--version', '--verbose'],
                                                    env=dict(os.environ, RUSTUP_TOOLCHAIN='1.99.0'), text=True),
                      method='ABAB, A: strict analyzer subprocess wall time; B: in-process timer per mode; input JSON and IPC excluded from B',
                      machine_state='Shared host; no owned daemon; OS caches warm; one discarded warm-up pair per mode; no affinity; normal process priority.',
                      modes={'classify': 'One uncached snapshot supplied on both sides, classifier only (includes shared name/outline analysis); admission verified outside timer.',
                             'analysis': 'One uncached snapshot supplied on both sides, admission + classification.',
                             'cold': 'Two uncached file versions, admission + classification.',
                             'edit': 'Baseline primed outside timer; one uncached edited file, admission + classification.',
                             'warm': 'Both versions cached; hashing + comparison, admission + classification.'},
                      binaries={str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in [args.prototype, args.probe]})
        args.out.write_text(json.dumps(result, indent=2) + '\n')
    try:
        for mode in ['classify', 'analysis', 'cold', 'edit', 'warm']:
            settle()
            baseline = after if mode in ('analysis', 'classify') else before
            # One excluded warm-up pair per mode; OS caches stay warm.
            run = subprocess.run([str(args.prototype)], input=after['handlers.rs'].encode(), env=env, capture_output=True, check=True)
            json.loads(run.stdout)
            probe.ask(baseline, after, measure='edit')
            probe.ask(baseline, after, measure=mode)
            block = []
            for index in range(args.pairs):
                state = settle()
                start = time.perf_counter_ns()
                run = subprocess.run([str(args.prototype)], input=after['handlers.rs'].encode(), env=env, capture_output=True, check=True)
                old_ns = time.perf_counter_ns() - start
                json.loads(run.stdout)  # Outside the timed A interval.
                result = probe.ask(baseline, after, measure=mode)
                assert result['class'] == 'live' and result['admission'] == 'admit', result
                row = dict(mode=mode, pair=index + 1, prototype_ms=old_ns / 1e6, rust_ms=result['elapsed_ns'] / 1e6,
                           ratio=result['elapsed_ns'] / old_ns, before=state, after=machine())
                block.append(row)
                rows.append(row)
            summaries[mode] = dict(prototype_ms=summary([r['prototype_ms'] for r in block]),
                                   rust_ms=summary([r['rust_ms'] for r in block]),
                                   rust_over_prototype=summary([r['ratio'] for r in block]))
            print(json.dumps(dict(mode=mode, summary=summaries[mode])), flush=True)
            persist()
    finally:
        probe.close()
        persist()


if __name__ == '__main__':
    main()
