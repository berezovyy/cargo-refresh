#!/usr/bin/env python3
"""Functional backtrace acceptance. Run a disposable fixture with qualified support.
No latency measurements. Also compares the final source with stock rustc.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import time
import urllib.request


def wait(predicate, process=None, timeout=240):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        if process is not None and process.poll() is not None:
            raise RuntimeError(f"controller exited: {process.returncode}")
        time.sleep(.02)
    raise TimeoutError("backtrace acceptance timed out")


def http(port, path="/"):
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=2) as response:
            return response.read()
    except OSError:
        return None


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--controller", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--lane", type=Path, help="optional external build resource wrapper")
    parser.add_argument("--project", type=Path, help="Reuse a disposable fixture and its warm target")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    work = args.work.resolve()
    work.mkdir(parents=True, exist_ok=False)
    project = args.project.resolve() if args.project else work / "fixture"
    shutil.copytree(root / "tests/backtrace-fixture", project, dirs_exist_ok=bool(args.project), ignore=shutil.ignore_patterns("target"))
    source = project / "src/patched.rs"
    original = source.read_text()
    events = work / "events.jsonl"
    port = free_port()
    env = os.environ | {"PORT": str(port), "RUSTUP_TOOLCHAIN": "1.99.0", "RUST_BACKTRACE": "1", "REFRESH_MEMORY_MIN_BYTES": str(6 * 1024**3)}
    if args.lane:
        env["REFRESH_LANE_TOOL"] = str(args.lane.resolve())
    command = [str(args.controller.resolve()), "refresh", "run", "--no-check", "--events", str(events)]
    stderr = work / "stderr.log"
    def rows():
        if not events.exists():
            return []
        return [json.loads(line) for line in events.read_text().splitlines() if line.endswith("}")]
    def output():
        return stderr.read_bytes()
    def check_frame(data, generation, line):
        expected = f"src/patched.rs:{line}   [patched, gen {generation}]".encode()
        frame = next((row for row in data.splitlines() if b"at refresh_backtrace_fixture::patched::panic_site  " in row), b"")
        assert expected in frame, (expected, data[-5000:])
        assert data.count(b"ordinary-before\xff\0\r\n") == 1, data[-5000:]
        assert data.count(b"ordinary-after\xfe\r\n") == 1, data[-5000:]
    with stderr.open("wb") as log, (work / "stdout.log").open("wb") as stdout:
        process = subprocess.Popen(command, cwd=project, env=env, stdout=stdout, stderr=log)
        try:
            wait(lambda: any(row["event"] == "ready" for row in rows()), process)
            assert http(port) == b"1"
            generations = []
            for generation in range(1, 4):
                text = original.replace("pub fn panic_site", "\n" * generation + "pub fn panic_site").replace('); } 1 }', f'); }} {generation + 1} }}')
                line = next(n for n, textline in enumerate(text.splitlines(), 1) if textline.startswith("pub fn panic_site"))
                before = max((row.get("revision", 0) for row in rows()), default=0)
                source.write_text(text)
                event = wait(lambda: next((row for row in rows() if row["event"] == "save" and row["revision"] > before), None), process)
                assert event["path"] == "live", event
                assert http(port) == str(generation + 1).encode()
                offset = len(output())
                assert http(port, "/panic") == b"999"
                wait(lambda: b"ordinary-after\xfe\r\n" in output()[offset:], process)
                data = output()[offset:]
                check_frame(data, generation, line)
                (work / f"generation-{generation}.stderr").write_bytes(data)
                generations.append({"generation": generation, "line": line, "path": event["path"]})
            # Stock oracle: same final source, same panic line and unrelated bytes.
            stock_env = {key: value for key, value in env.items() if key not in ("RUSTFLAGS", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "LD_PRELOAD", "CARGO_BUILD_TARGET")}
            stock_env["CARGO_TARGET_DIR"] = str(work / "stock-target")
            with (work / "stock-build.log").open("wb") as build_log:
                subprocess.run(([str(args.lane.resolve()), "build", "--"] if args.lane else []) + ["nice", "-n", "19", "cargo", "build", "--offline", "-j", "4"], cwd=project, env=stock_env, stdout=build_log, stderr=subprocess.STDOUT, check=True)
            stock_port = free_port()
            with (work / "stock.stderr").open("wb") as stock_log:
                stock = subprocess.Popen([str(work / "stock-target/debug/refresh-backtrace-fixture")], cwd=project, env=stock_env | {"PORT": str(stock_port)}, stderr=stock_log, start_new_session=True)
                try:
                    wait(lambda: http(stock_port) == b"4", stock, 10)
                    assert http(stock_port, "/panic") == b"999"
                finally:
                    stock.terminate()
                    stock.wait(10)
            stock_bytes = (work / "stock.stderr").read_bytes()
            assert f"src/patched.rs:{line}:".encode() in stock_bytes, stock_bytes
            for marker in [b"ordinary-before\xff\0\r\n", b"ordinary-after\xfe\r\n"]:
                assert stock_bytes.count(marker) == 1
            (work / "result.json").write_text(json.dumps({"generations": generations, "stock_matches": True, "rust_backtrace": "1", "timings": "UNMEASURED"}, indent=2))
            print("PASS: three live generations, current panic lines, stderr bytes, stock oracle")
        finally:
            process.send_signal(signal.SIGTERM)
            process.wait(30)


if __name__ == "__main__":
    main()
