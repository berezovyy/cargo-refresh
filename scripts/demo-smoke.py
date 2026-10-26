#!/usr/bin/env python3
"""Exercise the committed demo with independent stock Cargo checks. No timings."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


def stop(proc):
    if proc.poll() is None:
        proc.terminate()
    try:
        proc.wait(20)
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        proc.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--controller', type=Path, required=True)
    parser.add_argument('--archive', type=Path, required=True)
    args = parser.parse_args()
    work = Path(tempfile.mkdtemp(prefix='smoke-', dir=ROOT / '.try'))
    project = work / 'demo'
    shutil.copytree(ROOT / 'examples/demo', project, ignore=shutil.ignore_patterns('target'))
    source = project / 'src/main.rs'
    original = source.read_text()
    logfile, eventfile = work / 'controller.log', work / 'events.jsonl'
    env = dict(os.environ, DEMO_FAST='1', RUSTUP_TOOLCHAIN='1.99.0', CARGO_BUILD_JOBS='4')
    env.pop('DEMO_ONCE', None)
    env.pop('CARGO_TARGET_DIR', None)
    rows = []
    proc = None

    def interrupted(*_):
        raise SystemExit(143)
    signal.signal(signal.SIGTERM, interrupted)

    def events():
        if not eventfile.exists():
            return []
        result = []
        for line in eventfile.read_text().splitlines():
            try:
                result.append(json.loads(line))
            except json.JSONDecodeError:
                pass  # A writer may still be appending the last line.
        return result

    def probes():
        pid, result = None, []
        if not logfile.exists():
            return result
        for line in logfile.read_text().splitlines():
            if m := re.search(r'demo started, pid (\d+)', line):
                pid = int(m[1])
            if m := re.search(r'tick\s+(\d+) \| total\s+(\d+) \| (.*)', line):
                result.append(dict(pid=pid, tick=int(m[1]), total=int(m[2]), message=m[3]))
        return result

    def wait(check, timeout=240):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if value := check():
                return value
            if proc is not None and proc.poll() is not None:
                raise RuntimeError(f'controller exited {proc.returncode}: {logfile.read_text()[-8000:]}')
            time.sleep(.025)
        raise TimeoutError(f'See {logfile} and {eventfile}\n{logfile.read_text()[-8000:]}')

    def stock(message, factor):
        stockenv = dict(env, DEMO_ONCE='1', CARGO_TARGET_DIR=str(work / 'stock-target'))
        result = subprocess.run(['nice', '-n', '19', 'cargo', 'run', '--quiet', '--bin', 'demo', '-j', '4'], cwd=project,
                                env=stockenv, capture_output=True, text=True, timeout=240, check=True)
        with (work / 'stock.log').open('a') as out:
            out.write(result.stdout + result.stderr)
        assert re.search(rf'tick\s+1 \| total\s+{factor} \| {re.escape(message)}', result.stdout), result.stdout

    def current(message):
        return next((p for p in reversed(probes()) if p['message'] == message), None)

    def valid_save(text, message, factor, expected=None):
        start = len(events())
        before = probes()[-1]
        source.write_text(text)
        event = wait(lambda: next((e for e in events()[start:] if e['event'] == 'save'), None))
        if expected:
            assert event['path'] == expected, event
        after = wait(lambda: next((p for p in reversed(probes()) if p['message'] == message
                                  and (p['pid'] != before['pid'] if expected == 'restart' else p['tick'] > before['tick'])), None))
        if expected == 'live':
            assert after['pid'] == before['pid'] and after['tick'] > before['tick'], (before, after)
        if expected == 'restart':
            assert after['pid'] != before['pid'], (before, after)
        # Two settled consecutive ticks prove the step result, independent of saved total.
        def stable():
            ps = [p for p in probes() if p['pid'] == after['pid'] and p['tick'] > after['tick'] and p['message'] == message]
            return ps[-2:] if len(ps) >= 2 else None
        pair = wait(stable)
        assert pair[1]['tick'] == pair[0]['tick'] + 1 and pair[1]['total'] - pair[0]['total'] == pair[1]['tick'] * factor, pair
        if event['path'] == 'live':
            wait(lambda: any(e['event'] == 'check_ok' and e.get('generation') == event.get('generation') for e in events()[start:]))
        stock(message, factor)
        rows.append(dict(path=event['path'], before=before, after=after, stock_match=True))
        return after

    try:
        with logfile.open('w') as output:
            proc = subprocess.Popen([str(args.controller.resolve()), 'refresh', 'run', '--bin', 'demo', '--events', str(eventfile),
                                     '--from-archive', str(args.archive.resolve()), '-v'], cwd=project, env=env,
                                    stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
            wait(lambda: any(e['event'] == 'ready' for e in events()))
            wait(lambda: current('hello from version 1'))
            stock('hello from version 1', 1)
            v2 = original.replace('hello from version 1', 'hello from version 2')
            valid_save(v2, 'hello from version 2', 1, 'live')
            v3 = v2.replace('n * 1', 'n * 2')
            valid_save(v3, 'hello from version 2', 2, 'live')
            layout = v3.replace('    total: u64,', '    total: u64,\n    _marker: u64,').replace('State { ticks: 0, total: 0 }', 'State { ticks: 0, total: 0, _marker: 0 }')
            valid_save(layout, 'hello from version 2', 2, 'restart')
            start, before = len(events()), probes()[-1]
            source.write_text(layout.replace('"hello from version 2"', '42'))
            wait(lambda: any((e['event'] in ('build_error', 'check_failed') or e.get('path') == 'error') for e in events()[start:]))
            time.sleep(.1)
            after = probes()[-1]
            assert before['pid'] == after['pid'] and after['tick'] > before['tick'], (before, after)
            error = subprocess.run(['nice', '-n', '19', 'cargo', 'check', '-j', '4'], cwd=project,
                                   env=dict(env, CARGO_TARGET_DIR=str(work / 'stock-target')), capture_output=True, text=True, timeout=240)
            assert error.returncode != 0 and 'E0308' in error.stderr, error.stderr
            assert 'E0308' in logfile.read_text(), logfile.read_text()[-8000:]
            rows.append(dict(path='error', old_program_kept_running=True, stock_diagnostic='E0308'))
            valid_save(layout.replace('hello from version 2', 'hello from version 3'), 'hello from version 3', 2)
    finally:
        if proc is not None:
            stop(proc)
        (work / 'result.json').write_text(json.dumps(rows, indent=2) + '\n')
        print(f'Evidence: {work}')
    assert len(rows) == 5
    print('PASS: live / live / restart / error keeps old program / fix recovers; all outputs match stock cargo run')


if __name__ == '__main__':
    main()
