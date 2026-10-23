#!/usr/bin/env python3
"""Local archive -> private HOME -> running app -> save, with a stock oracle.

Live mode must run in the rustd resource lane. No performance numbers are emitted.
Use a new --work directory per run; failures retain small logs for diagnosis.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import time
import urllib.request

SOURCE = '''use std::io::{Read, Write};
use std::net::TcpListener;

#[inline(never)]
fn value() -> u64 { 1 }

fn main() {
    let port = std::env::var("PORT").unwrap();
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).unwrap();
    let mut count = 0_u64;
    for stream in listener.incoming() {
        let mut stream = stream.unwrap();
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request);
        count += 1;
        let body = format!("{} {} {}", value(), std::process::id(), count);
        let response = format!("HTTP/1.1 200 OK\\r\\nContent-Length: {}\\r\\nConnection: close\\r\\n\\r\\n{}", body.len(), body);
        stream.write_all(response.as_bytes()).unwrap();
    }
}
'''


def port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def probe(number):
    try:
        with urllib.request.urlopen(f'http://127.0.0.1:{number}', timeout=.5) as response:
            return list(map(int, response.read().decode().split()))
    except OSError:
        return None


def wait(check, proc, log, timeout=240):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        if proc.poll() is not None:
            raise RuntimeError(f'process exited: {proc.returncode}\n{log.read_text()[-12000:]}')
        time.sleep(.025)
    raise TimeoutError(log.read_text()[-12000:])


def events(path):
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()] if path.exists() else []


def stop(proc):
    if proc.poll() is None:
        proc.terminate()
    try:
        proc.wait(20)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()


def main():
    # Docker forwards SIGTERM to PID 1 only when it has an explicit handler.
    # Raise through the owned-process finally blocks so the RAM guard cleans up.
    def interrupted(*unused):
        raise SystemExit(143)
    signal.signal(signal.SIGTERM, interrupted)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--controller', type=Path, required=True)
    parser.add_argument('--archive', type=Path)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--mode', choices=['live', 'unsupported', 'corrupt', 'low-memory', 'override'], default='live')
    args = parser.parse_args()
    root = args.work.resolve()
    root.mkdir(parents=True, exist_ok=False)
    project = root / 'tiny'
    (project / 'src').mkdir(parents=True)
    (project / 'Cargo.toml').write_text('[package]\nname = "install-tiny"\nversion = "0.0.0"\nedition = "2024"\n[workspace]\n')
    source = project / 'src/main.rs'
    source.write_text(SOURCE)
    home = root / 'home'
    home.mkdir()
    rustup = Path(os.environ.get('RUSTUP_HOME', str(Path.home() / '.rustup'))).resolve()
    cargo = Path(os.environ.get('CARGO_HOME', str(Path.home() / '.cargo'))).resolve()
    number = port()
    env = dict(os.environ)
    for key in list(env):
        if key.startswith(('REFRESH_', 'CARGO_REFRESH_', 'CARGO_PROFILE_', 'CARGO_TARGET_')) or key in (
            'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_TARGET', 'CARGO_TARGET_DIR', 'LD_PRELOAD', 'RUSTC', 'XDG_CACHE_HOME'):
            env.pop(key)
    env.update(HOME=str(home), RUSTUP_HOME=str(rustup), CARGO_HOME=str(cargo), RUSTUP_TOOLCHAIN='1.99.0', PORT=str(number),
               REFRESH_MEMORY_MIN_BYTES=str(6 * 1024**3), REFRESH_RUSTD_FRONTIER='1')
    lane = Path(__file__).resolve().parents[2] / 'rust-fast-compiler/tools/lane.sh'
    if lane.exists():
        env['REFRESH_LANE_TOOL'] = str(lane)
    trap = root / 'rustd-started'
    fakebin = root / 'bin'
    fakebin.mkdir()
    rustd = fakebin / 'rustd'
    rustd.write_text(f'#!/bin/sh\ntouch "{trap}"\nexit 99\n')
    rustd.chmod(0o755)
    env['PATH'] = str(fakebin) + os.pathsep + env['PATH']
    env['REFRESH_RUSTD'] = str(rustd)
    if args.mode == 'unsupported':
        compiler = subprocess.check_output(['rustup', 'which', '--toolchain', '1.99.0', 'rustc'], universal_newlines=True).strip()
        vv = subprocess.check_output([compiler, '-vV'], universal_newlines=True).replace('b940084d7eb6a299eb4bfeb8e34901bc051e7ac4', '0' * 40)
        fake = fakebin / 'rustc-active'
        import shlex
        fake.write_text('#!/bin/sh\nif [ "$1" = "-vV" ]; then\ncat <<\'VV\'\n' + vv + '\nVV\nelse\nexec ' + shlex.quote(compiler) + ' "$@"\nfi\n')
        fake.chmod(0o755)
        env['RUSTC'] = str(fake)
    elif args.mode == 'low-memory':
        env['CARGO_REFRESH_WARM_PEAK_BYTES'] = str(1024**5)
    elif args.mode == 'override':
        env['RUSTFLAGS'] = '-C opt-level=1'
    elif args.mode == 'corrupt':
        cache = home / '.cache/cargo-refresh/b940084d7eb6a299eb4bfeb8e34901bc051e7ac4'
        cache.mkdir(parents=True)
        (cache / 'VERSION').write_text('rustc_commit=wrong\n')
    if args.mode == 'live' and not args.archive:
        parser.error('live mode requires --archive')
    eventfile = root / 'events.jsonl'
    command = [str(args.controller.resolve()), 'refresh', 'run', '--events', str(eventfile), '-v']
    if args.archive:
        command += ['--from-archive', str(args.archive.resolve())]
    log = root / 'controller.log'
    with log.open('w') as output:
        proc = subprocess.Popen(command, cwd=project, env=env, stdout=output, stderr=subprocess.STDOUT)
        try:
            wait(lambda: any(e['event'] == 'ready' for e in events(eventfile)), proc, log)
            first = wait(lambda: probe(number), proc, log)
            assert first[0] == 1, first
            source.write_text(SOURCE.replace('fn value() -> u64 { 1 }', 'fn value() -> u64 { 2 }'))
            saved = wait(lambda: next((e for e in events(eventfile) if e['event'] == 'save'), None), proc, log)
            expected = 'live' if args.mode == 'live' else 'stock'
            assert saved['path'] == expected, (saved, log.read_text()[-12000:])
            def edited_probe():
                response = probe(number)
                return response if response and response[0] == 2 else None
            after = wait(edited_probe, proc, log)
            if args.mode == 'live':
                assert first[1] == after[1] and after[2] > first[2], (first, after)
                cache = home / '.cache/cargo-refresh/b940084d7eb6a299eb4bfeb8e34901bc051e7ac4'
                assert all((cache / f).is_file() for f in ['rustd', 'cargo-rustd', 'rustd-rustc', 'librefresh.so', 'abi-facts.bundle', 'VERSION'])
                wait(lambda: any(e['event'] == 'check_ok' for e in events(eventfile)), proc, log)
            else:
                assert first[1] != after[1], (first, after)
                assert not any(e['event'] in ('worker_reset_start', 'compiler_ready') for e in events(eventfile))
            assert not trap.exists(), 'unvalidated rustd was launched'
        finally:
            stop(proc)
    # Independent stock build and run of the exact edited source.
    oracle_env = env.copy()
    oracle_env.pop('RUSTC', None)
    oracle_env['CARGO_TARGET_DIR'] = str(root / 'oracle-target')
    build = ['nice', '-n', '19', 'cargo', 'build', '--offline', '-j', '4']
    if 'REFRESH_LANE_TOOL' in env:
        build = [env['REFRESH_LANE_TOOL'], 'build', '--', *build]
    with (root / 'stock-build.log').open('w') as output:
        subprocess.run(build, cwd=project, env=oracle_env, stdout=output, stderr=subprocess.STDOUT, check=True, timeout=240)
    oracle_port = port()
    oracle_env['PORT'] = str(oracle_port)
    oracle_log = root / 'stock-app.log'
    with oracle_log.open('w') as output:
        oracle = subprocess.Popen([str(root / 'oracle-target/debug/install-tiny')], env=oracle_env, stdout=output, stderr=subprocess.STDOUT)
        try:
            actual = wait(lambda: probe(oracle_port), oracle, oracle_log, 10)
            assert actual[0] == after[0] == 2, (actual, after)
        finally:
            stop(oracle)
    text = log.read_text()
    if args.mode == 'unsupported':
        assert '[refresh] cargo-refresh has no qualified support for rustc 1.99.0 (' + '0' * 40 + ').' in text
    if args.mode == 'low-memory':
        assert 'plus 2 GiB reserve' in text and 'Continuing in cargo mode' in text
    summary = dict(mode=args.mode, save_path=saved['path'], stock_oracle_match=True,
                   pid_preserved=first[1] == after[1], state_preserved=first[1] == after[1] and after[2] > first[2], unvalidated_rustd_started=trap.exists())
    (root / 'result.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps(summary), flush=True)


if __name__ == '__main__':
    main()
