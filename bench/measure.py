#!/usr/bin/env python3
"""Warm, interleaved stock Cargo / live refresh save-to-visible pairs."""
import argparse
import datetime
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import socket
import statistics
import subprocess
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]


def resource():
    return dict(loadavg=Path('/proc/loadavg').read_text().strip(),
                mem_available_kib=next(int(l.split()[1]) for l in Path('/proc/meminfo').read_text().splitlines() if l.startswith('MemAvailable:')))


def settle():
    deadline, notice = time.monotonic() + 600, 0
    while os.getloadavg()[0] > 2:
        if time.monotonic() > deadline:
            raise TimeoutError('load1 did not settle to <= 2 within 10 minutes; no sample taken')
        if time.monotonic() > notice:
            print('Waiting for load1 <= 2:', resource(), flush=True)
            notice = time.monotonic() + 30
        time.sleep(1)


def stop(proc):
    if proc is None:
        return
    if proc.poll() is None:
        proc.terminate()
    try:
        proc.wait(20)
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        proc.wait()


def wait(check, processes, timeout=300):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if value := check():
            return value
        for process in processes:
            if process is not None and process.poll() is not None:
                raise RuntimeError(f'owned process {process.pid} exited {process.returncode}; inspect retained logs')
        time.sleep(.002)
    raise TimeoutError('Expected observable output/event did not arrive; inspect retained logs')


def events(path):
    result = []
    if path.exists():
        for line in path.read_text().splitlines():
            try:
                result.append(json.loads(line))
            except json.JSONDecodeError:
                pass
    return result


class App:
    def __init__(self, name, base):
        self.name, self.proc = name, None
        self.project = base / name
        base.mkdir(parents=True)
        if name == 'demo':
            shutil.copytree(ROOT / 'examples/demo', self.project, ignore=shutil.ignore_patterns('target'))
            self.source = self.project / 'src/main.rs'
            self.original, self.marker, self.binary = self.source.read_text(), 'hello from version 1', 'demo'
        else:
            fixtures = ROOT / 'scripts/qualification/fixtures'
            for item in ['web', 'statearena', 'statearena-derive']:
                shutil.copytree(fixtures / item, base / item, ignore=shutil.ignore_patterns('target'))
            self.source = self.project / 'crates/api/src/handlers.rs'
            self.original, self.marker, self.binary = self.source.read_text(), 'S1:v1', 'web-server'
        self.log, self.eventfile = base / 'app.log', base / 'events.jsonl'
        self.env = dict(os.environ, RUSTUP_TOOLCHAIN='1.99.0', CARGO_BUILD_JOBS='4', DEMO_FAST='1')
        for key in ['CARGO_TARGET_DIR', 'DEMO_ONCE', 'LD_PRELOAD']:
            self.env.pop(key, None)
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            self.env['PORT'] = str(sock.getsockname()[1])

    def edit(self, marker):
        assert self.original.count(self.marker) == 1
        self.source.write_text(self.original.replace(self.marker, marker))
        return time.perf_counter_ns()  # after write/close, identical in both arms

    def build(self, setup=False):
        cmd = ['cargo', 'build', '--locked', '--bin', self.binary, '-j', '4']
        if setup:
            cmd = ['nice', '-n', '19', *cmd]
        with (self.log.parent / 'build.log').open('a') as out:
            subprocess.run(cmd, cwd=self.project, env=self.env, stdout=out, stderr=subprocess.STDOUT, check=True, timeout=900)

    def start(self, controller=None, archive=None):
        if controller:
            cmd = [str(controller), 'refresh', 'run', '--bin', self.binary, '--events', str(self.eventfile), '--from-archive', str(archive)]
            if self.name == 'web':
                cmd += ['--edit', 'api,domain,store']
        else:
            cmd = [str(self.project / 'target/debug' / self.binary)]
        with self.log.open('a') as out:
            self.proc = subprocess.Popen(cmd, cwd=self.project, env=self.env, stdout=out, stderr=subprocess.STDOUT, start_new_session=True)

    def probe(self, marker):
        if self.name == 'web':
            try:
                with urllib.request.urlopen(f'http://127.0.0.1:{self.env["PORT"]}/probe', timeout=.5) as r:
                    body = r.read().decode()
                fields = dict(l.split('=', 1) for l in body.splitlines())
                if fields['probe'].split('|')[0] == marker:
                    return dict(value=fields['probe'], pid=int(fields['pid']), ticks=int(fields['ticks']))
            except (OSError, ValueError, KeyError):
                return None
        else:
            pid, found = None, None
            for line in self.log.read_text().splitlines() if self.log.exists() else []:
                if m := re.search(r'demo started, pid (\d+)', line):
                    pid = int(m[1])
                if m := re.search(r'tick\s+(\d+) \| total\s+(\d+) \| (.*)', line):
                    if m[3] == marker:
                        found = dict(value=m[3], pid=pid, ticks=int(m[1]), total=int(m[2]))
            return found


def stats(values):
    return dict(median=statistics.median(values), min=min(values), max=max(values), n=len(values))


def measure(name, args, out):
    work = Path(tempfile.mkdtemp(prefix=name + '-', dir=ROOT / '.bench-work'))
    stock, refresh = App(name, work / 'stock'), App(name, work / 'refresh')
    observations = []
    try:
        stock.build(setup=True)
        stock.start()
        wait(lambda: stock.probe(stock.marker), [stock.proc])
        refresh.start(args.controller, args.archive)
        wait(lambda: any(e['event'] == 'ready' for e in events(refresh.eventfile)), [refresh.proc], 900)
        wait(lambda: refresh.probe(refresh.marker), [refresh.proc])
        for pair in range(args.pairs + 1):  # one excluded warm-up pair
            marker = f'bench-{pair}'
            previous = refresh.probe(refresh.marker if pair == 0 else f'bench-{pair - 1}')
            settle()
            before = resource()
            start = stock.edit(marker)
            stock.build()
            stop(stock.proc)
            stock.start()
            a = wait(lambda: stock.probe(marker), [stock.proc])
            stock_ms = (time.perf_counter_ns() - start) / 1e6
            stock_after = resource()
            # Settle again outside the measured refresh interval.
            settle()
            refresh_before = resource()
            count = len(events(refresh.eventfile))
            start = refresh.edit(marker)
            b = wait(lambda: refresh.probe(marker), [refresh.proc])
            refresh_ms = (time.perf_counter_ns() - start) / 1e6
            refresh_after = resource()
            event = wait(lambda: next((e for e in events(refresh.eventfile)[count:] if e['event'] == 'save'), None), [refresh.proc])
            assert event['path'] == 'live', event
            assert previous['pid'] == b['pid'] and b['ticks'] > previous['ticks'], (previous, b)
            assert a['value'] == b['value'], (a, b)
            wait(lambda: any(e['event'] == 'check_ok' and e.get('revision') == event.get('revision') for e in events(refresh.eventfile)[count:]), [refresh.proc])
            row = dict(app=name, pair=pair, warmup=pair == 0, stock_ms=stock_ms, refresh_ms=refresh_ms,
                       ratio=stock_ms / refresh_ms, path=event['path'], stock_match=True,
                       stock_before=before, stock_after=stock_after, refresh_before=refresh_before,
                       refresh_after=refresh_after, stock_probe=a, refresh_probe=b)
            observations.append(row)
            with (out / 'pairs.jsonl').open('a') as stream:
                stream.write(json.dumps(row) + '\n')
            print(f'{name} pair {pair}: {"warm-up (excluded)" if pair == 0 else "PASS"}', flush=True)
        retained = observations[1:]
        return dict(app=name, stock_ms=stats([r['stock_ms'] for r in retained]),
                    refresh_ms=stats([r['refresh_ms'] for r in retained]),
                    paired_ratio=stats([r['ratio'] for r in retained]))
    finally:
        stop(refresh.proc)
        stop(stock.proc)
        print(f'Warm targets and logs: {work}', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('app', nargs='?', choices=['demo', 'web', 'all'], default='all')
    parser.add_argument('--pairs', type=int, default=15)
    parser.add_argument('--controller', type=Path, default=ROOT / '.try/install/bin/cargo-refresh')
    parser.add_argument('--archive', type=Path, default=ROOT / 'bundle/cargo-refresh-0.1.0-dev-b940084d7eb6a299eb4bfeb8e34901bc051e7ac4-x86_64-linux.tar.zst')
    parser.add_argument('--validate-only', action='store_true', help='check inputs without running builds, daemons or measurements')
    args = parser.parse_args()
    if args.pairs < 1:
        parser.error('--pairs must be positive')
    args.controller, args.archive = args.controller.resolve(), args.archive.resolve()
    if not args.archive.is_file() or not Path(str(args.archive) + '.sha256').is_file():
        parser.error('missing archive/checksum; restore bundle/ or pass --archive')
    for name in ['web', 'statearena', 'statearena-derive']:
        assert (ROOT / 'scripts/qualification/fixtures' / name / 'Cargo.toml').is_file()
    assert (ROOT / 'examples/demo/src/main.rs').read_text().count('hello from version 1') == 1
    assert (ROOT / 'scripts/qualification/fixtures/web/crates/api/src/handlers.rs').read_text().count('S1:v1') == 1
    if args.validate_only:
        print('PASS: demo/web inputs, source markers and archive/checksum exist; no measurements run')
        return
    if not args.controller.is_file():
        parser.error('controller missing: run ./try.sh --smoke first, or pass --controller')
    def interrupted(*_):
        raise SystemExit(143)
    signal.signal(signal.SIGTERM, interrupted)
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')
    out = ROOT / 'results' / ('run-' + stamp)
    out.mkdir()
    (out / 'machine.json').write_text(json.dumps(dict(date=stamp, platform=platform.platform(),
        cpu=next((l.split(':', 1)[1].strip() for l in Path('/proc/cpuinfo').read_text().splitlines() if l.startswith('model name')), 'unknown'),
        logical_cpus=os.cpu_count(), rustc=subprocess.check_output(['rustc', '-Vv'], text=True),
        source_commit=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
        initial_resources=resource(), cache_state='warm dependency/OS/compiler caches; one discarded warm-up pair',
        methodology='ABAB stock Cargo + restart then refresh; settle load1 <= 2 before each arm; 2 ms polling; demo 20 ms cadence; debug=0; 4 build jobs'), indent=2) + '\n')
    summaries = []
    for app in ['demo', 'web'] if args.app == 'all' else [args.app]:
        summaries.append(measure(app, args, out))
        (out / 'summary.json').write_text(json.dumps(summaries, indent=2) + '\n')
    lines = ['# Local benchmark', '', 'Warm caches, one excluded warm-up, ABAB pairs. Values are median [min, max].', '',
             '| App | Cargo + restart ms | Refresh ms | Per-pair ratio | n |', '|---|---:|---:|---:|---:|']
    for s in summaries:
        def cell(key):
            d = s[key]
            return f"{d['median']:.2f} [{d['min']:.2f}, {d['max']:.2f}]"
        lines.append(f"| {s['app']} | {cell('stock_ms')} | {cell('refresh_ms')} | {cell('paired_ratio')} | {s['paired_ratio']['n']} |")
    lines += ['', 'Raw pairs and machine/load/RAM metadata are adjacent. Demo includes its 20 ms output cadence.',
              'Only a response-marker edit is covered; this does not qualify layout/async or whole-commit speed.']
    (out / 'README.md').write_text('\n'.join(lines) + '\n')
    print(f'Results: {out}')


if __name__ == '__main__':
    main()
