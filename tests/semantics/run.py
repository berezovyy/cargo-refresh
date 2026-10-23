#!/usr/bin/env python3
"""M4 stock oracle. Refresh mode deliberately fails until a real adapter exists."""
from __future__ import annotations

import argparse
import contextlib
import fcntl
import hashlib
import json
import os
from pathlib import Path
import selectors
import shutil
import signal
import subprocess
import sys
import time
import tomllib

ROOT = Path(__file__).resolve().parent
MIN_MEMORY_KIB = 6 * 1024 * 1024
ENV_KEYS = ("M4_SEMANTICS_REQUIRED", "M4_SEMANTICS_OPTIONAL", "SQLX_OFFLINE")


def memory_guard() -> None:
    for line in Path("/proc/meminfo").read_text().splitlines():
        if line.startswith("MemAvailable:"):
            available = int(line.split()[1])
            if available < MIN_MEMORY_KIB:
                raise RuntimeError(f"MemAvailable {available} KiB < 6 GiB; stopping owned processes")
            return
    raise RuntimeError("MemAvailable missing from /proc/meminfo")


def stop(process: subprocess.Popen) -> None:
    """Every child owns a new process group. Never signal anyone else's group."""
    if process.poll() is None:
        with contextlib.suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(process.pid, signal.SIGKILL)
            process.wait()


def build(source: Path, target: Path, env: dict, log: Path, offline: bool, record: bool) -> None:
    memory_guard()
    command = ["nice", "-n", "19", "cargo", "build", "-j", "4", "--target-dir", str(target)]
    if not record:
        command.append("--locked")
    if offline:
        command.append("--offline")
    with log.open("w") as output:
        process = subprocess.Popen(command, cwd=source, env=env, stdout=output,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        try:
            deadline = time.monotonic() + 600
            while process.poll() is None:
                memory_guard()
                if time.monotonic() > deadline:
                    raise RuntimeError(f"build timed out; see {log}")
                try:
                    process.wait(timeout=0.25)
                except subprocess.TimeoutExpired:
                    pass
            if process.returncode:
                raise RuntimeError(f"cargo build exited {process.returncode}; see {log}\n{log.read_text()[-6000:]}")
        finally:
            stop(process)


class App:
    def __init__(self, executable: Path, source: Path, env: dict, log: Path):
        self.error = log.open("w")
        self.process = subprocess.Popen([str(executable)], cwd=source, env=env,
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=self.error, start_new_session=True)
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        self.pending = b""

    def close(self):
        try:
            stop(self.process)
        finally:
            self.selector.close()
            self.process.stdin.close()
            self.process.stdout.close()
            self.error.close()

    def probe(self) -> str:
        memory_guard()
        self.process.stdin.write(b"probe\n")
        self.process.stdin.flush()
        deadline = time.monotonic() + 10
        while b"\n" not in self.pending:
            memory_guard()
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeError("probe timed out after 10 seconds")
            if not self.selector.select(min(remaining, 0.25)):
                continue
            data = os.read(self.process.stdout.fileno(), 4096)
            if not data:
                raise RuntimeError(f"app exited or closed stdout (status {self.process.poll()})")
            self.pending += data
            if len(self.pending) > 65536:
                raise RuntimeError("probe output exceeds 64 KiB")
        line, self.pending = self.pending.split(b"\n", 1)
        decoded = line.decode("utf-8")
        if not decoded.startswith("probe="):
            raise RuntimeError(f"unexpected stdout line: {decoded!r}")
        return decoded


def observe(executable: Path, source: Path, env: dict, count: int, log: Path) -> list[str]:
    app = App(executable, source, env, log)
    try:
        return [app.probe() for _ in range(count)]
    finally:
        app.close()


def assert_equal(actual, expected, label):
    if actual != expected:
        raise RuntimeError(f"{label}: expected {expected!r}, got {actual!r}")


def apply_edit(source: Path, edit: dict) -> None:
    path = (source / edit["file"]).resolve()
    if not path.is_relative_to(source.resolve()):
        raise RuntimeError(f"edit escapes case directory: {edit['file']}")
    content = path.read_text()
    find = edit["find"]
    if not find or content.count(find) != 1:
        raise RuntimeError(f"{edit['file']}: find must match exactly once: {find!r}")
    path.write_text(content.replace(find, edit["replace"], 1))


def expected_contract(spec: dict) -> dict:
    return {
        "schema": 1,
        "case": spec["case"],
        "v0.1-allowed-outcomes": spec["v0_1"],
        "state-rule": spec["state_rule"],
        "base": spec["base_probes"],
        "steps": [{"name": step["name"],
                   "stock-after-restart": step["stock_after_restart"],
                   "live-expected": step["live_expected"]} for step in spec["steps"]],
        "live-verification": "UNMEASURED: refresh adapter not yet implemented",
    }


def run_stock(case: Path, work: Path, offline: bool, record: bool) -> dict:
    spec = tomllib.loads((case / "edits.toml").read_text())
    contract = expected_contract(spec)
    if not record:
        assert_equal(json.loads((case / "expected.json").read_text()), contract,
                     f"{case.name}: recorded expectations vs edits.toml")
    source = work / "sources" / case.name
    if source.exists():
        shutil.rmtree(source)
    # Fresh mtimes matter: copy2 could make restored base sources older than
    # the last edited binary, causing Cargo to reuse the edited artifact.
    shutil.copytree(case, source, copy_function=shutil.copyfile)
    target = work / "target"
    logs = work / "logs" / case.name
    logs.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    # Never inherit compile input values from the caller accidentally.
    for key in ENV_KEYS:
        env.pop(key, None)
    for key in spec.get("unset_environment", {}):
        env.pop(key, None)
    env.update(spec.get("environment", {}))
    env.update(RUSTUP_TOOLCHAIN="1.99.0", CARGO_BUILD_JOBS="4", CARGO_INCREMENTAL="0")
    # A relative inherited Cargo target or rustflags must not alter this oracle.
    for key in ("CARGO_TARGET_DIR", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"):
        env.pop(key, None)
    package = tomllib.loads((source / "Cargo.toml").read_text())["package"]["name"]
    executable = target / "debug" / package
    build(source, target, env, logs / "base-build.log", offline, record)
    observed_base = observe(executable, source, env, len(spec["base_probes"]), logs / "base-stderr.log")
    assert_equal(observed_base, spec["base_probes"], f"{case.name}: base")
    observations = []
    for index, step in enumerate(spec["steps"], 1):
        for edit in step.get("edits", []):
            apply_edit(source, edit)
        env.update(step.get("environment", {}))
        build(source, target, env, logs / f"edit-{index}-build.log", offline, record)
        actual = observe(executable, source, env, len(step["stock_after_restart"]),
                         logs / f"edit-{index}-stderr.log")
        assert_equal(actual, step["stock_after_restart"], f"{case.name}: {step['name']}")
        observations.append({"name": step["name"], "stock-after-restart": actual})
    if record:
        # Only update after every declaration has passed the actual stock oracle.
        contract["base"] = observed_base
        for recorded, observed in zip(contract["steps"], observations):
            recorded["stock-after-restart"] = observed["stock-after-restart"]
        (case / "expected.json").write_text(json.dumps(contract, indent=2) + "\n")
        shutil.copy2(source / "Cargo.lock", case / "Cargo.lock")
    result = {"case": case.name, "result": "PASS", "base": observed_base,
              "steps": observations,
              "final-binary-sha256": hashlib.sha256(executable.read_bytes()).hexdigest()}
    print(f"PASS {case.name} (base + {len(observations)} edits)", flush=True)
    return result


def run_refresh(*_args):
    # Future adapter contract is detailed in README.md. Do not accidentally turn
    # stock success into refresh success before the real controller is wired up.
    raise RuntimeError("refresh mode is NOT IMPLEMENTED; no refresh behavior has been qualified. "
                       "Adapter must invoke cargo refresh run, observe outcome events, and assert "
                       "live-expected or stock-after-restart without silently substituting modes.")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("stock", "refresh"), default="stock")
    parser.add_argument("--case", action="append", default=[], help="exact case name or two-digit prefix; repeatable")
    parser.add_argument("--record", action="store_true", help="record stock outputs after verifying edits.toml declarations")
    parser.add_argument("--offline", action="store_true", help="require dependencies already cached by Cargo")
    parser.add_argument("--results", type=Path, help="write compact verification JSON (including final binary hashes)")
    args = parser.parse_args()
    if args.mode == "refresh":
        run_refresh()
    cases = sorted(p for p in ROOT.glob("[0-9][0-9]-*") if p.is_dir())
    if args.case:
        selected = []
        for name in args.case:
            matches = [p for p in cases if p.name == name or p.name[:2] == name]
            if len(matches) != 1:
                parser.error(f"case {name!r} matches {len(matches)} cases")
            if matches[0] not in selected:
                selected.append(matches[0])
        cases = selected
    work = ROOT / ".work"
    work.mkdir(exist_ok=True)
    # Keep stable source/target paths warm, but protect them against concurrent runs.
    with (work / "runner.lock").open("w") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise RuntimeError("another semantics stock runner owns .work; wait for it to finish")
        results = []
        for case in cases:
            try:
                results.append(run_stock(case, work, args.offline, args.record))
            except Exception as error:
                results.append({"case": case.name, "result": "FAIL", "error": str(error)})
                print(f"FAIL {case.name}: {error}", file=sys.stderr, flush=True)
        report = {"mode": "stock", "toolchain": "1.99.0", "cases": results}
        if args.results:
            args.results.parent.mkdir(parents=True, exist_ok=True)
            args.results.write_text(json.dumps(report, indent=2) + "\n")
        failures = sum(row["result"] != "PASS" for row in results)
        print(f"{len(results)-failures}/{len(results)} cases passed", flush=True)
        return bool(failures)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (RuntimeError, OSError, ValueError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        sys.exit(1)
