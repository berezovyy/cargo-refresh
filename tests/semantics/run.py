#!/usr/bin/env python3
"""M4 stock oracle and real cargo-refresh runtime semantics driver."""
from __future__ import annotations

import argparse
import contextlib
import fcntl
import hashlib
import json
import os
import re
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
    lane = Path(os.environ.get("REFRESH_LANE", "/path/to/research/tools/lane.sh"))
    if lane.exists():
        command = [str(lane), "build", "--", *command]
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



def qualified_default_profile(case: Path, source: Path) -> list[str]:
    """Explicit supplemental variant: omit only a sole dev debug=0 override.

    Keep opt-level=1 in case 11: dropping it would invalidate that inline test.
    Original fixtures and their golden outputs are never changed.
    """
    normalized = []
    for original in case.rglob("Cargo.toml"):
        path = source / original.relative_to(case)
        text = path.read_text()
        profile = tomllib.loads(text).get("profile", {}).get("dev")
        if profile == {"debug": 0}:
            changed, count = re.subn(r"(?m)^\[profile\.dev\]\n\s*debug\s*=\s*0\n", "", text, count=1)
            if count != 1:
                raise RuntimeError(f"cannot normalize known debug=0 profile in {path}")
            path.write_text(changed)
            normalized.append(str(original.relative_to(case)))
    return normalized


def run_stock(case: Path, work: Path, offline: bool, record: bool, default_profile: bool = False) -> dict:
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
    normalized = qualified_default_profile(case, source) if default_profile else []
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
    result = {"case": case.name, "result": "PASS", "normalized-profiles": normalized, "base": observed_base,
              "steps": observations,
              "final-binary-sha256": hashlib.sha256(executable.read_bytes()).hexdigest()}
    print(f"PASS {case.name} (base + {len(observations)} edits)", flush=True)
    return result


class RefreshApp(App):
    """The controller inherits these pipes; only the application writes stdout."""
    def __init__(self, command: list[str], source: Path, env: dict, log: Path,
                 eventfile: Path, timeout: float):
        memory_guard()
        self.eventfile = eventfile
        self.timeout = timeout
        self.log = log
        self.error = log.open("w")
        self.process = subprocess.Popen(command, cwd=source, env=env,
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=self.error, start_new_session=True)
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        self.pending = b""

    def events(self) -> list[dict]:
        if not self.eventfile.exists():
            return []
        # Writer flushes complete records; defer an in-flight partial last line.
        lines = self.eventfile.read_text().splitlines(keepends=True)
        return [json.loads(line) for line in lines if line.endswith("\n")]

    def wait_event(self, start: int, predicate, label: str) -> dict:
        deadline = time.monotonic() + self.timeout
        while time.monotonic() < deadline:
            memory_guard()
            for event in self.events()[start:]:
                if predicate(event):
                    return event
            if self.process.poll() is not None:
                raise RuntimeError(f"controller exited {self.process.returncode} waiting for {label}; "
                                   f"see {self.log}\n{self.log.read_text()[-4000:]}")
            time.sleep(0.025)
        raise RuntimeError(f"timeout waiting for {label}; see {self.log}")

    def identity(self) -> tuple[int, str]:
        # Supervisor snapshots the app to /tmp/refresh-run-*/app. Search only
        # descendants of this controller, including children of worker threads.
        pending, visited, matches = [self.process.pid], set(), []
        while pending:
            pid = pending.pop()
            if pid in visited:
                continue
            visited.add(pid)
            for task in Path(f"/proc/{pid}/task").glob("*"):
                with contextlib.suppress(FileNotFoundError, ProcessLookupError):
                    pending.extend(map(int, (task / "children").read_text().split()))
            with contextlib.suppress(FileNotFoundError, ProcessLookupError):
                exe = os.readlink(f"/proc/{pid}/exe")
                if exe.startswith("/tmp/refresh-run-") and exe.endswith("/app"):
                    # Start ticks make PID reuse detectable too.
                    ticks = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]
                    matches.append((pid, ticks))
        if len(matches) != 1:
            raise RuntimeError(f"expected one owned application, found {matches}")
        return matches[0]

    def close(self):
        # SIGTERM lets the controller reap its separately grouped app/compiler.
        try:
            if self.process.poll() is None:
                self.process.terminate()
                try:
                    self.process.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    stop(self.process)
        finally:
            self.selector.close()
            self.process.stdin.close()
            self.process.stdout.close()
            self.error.close()


def case_environment(spec: dict) -> dict:
    env = os.environ.copy()
    for key in list(env):
        if key.startswith(("REFRESH_", "RUSTD_", "CARGO_REFRESH_", "CARGO_PROFILE_", "CARGO_TARGET_")) or key in (
                "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
                "CARGO_BUILD_TARGET", "CARGO_TARGET_DIR", "LD_PRELOAD", "RUSTC", "RUSTC_BOOTSTRAP"):
            env.pop(key)
    for key in (*ENV_KEYS, *spec.get("unset_environment", {})):
        env.pop(key, None)
    env.update(spec.get("environment", {}))
    env.update(RUSTUP_TOOLCHAIN="1.99.0", CARGO_BUILD_JOBS="4", CARGO_INCREMENTAL="0")
    return env


def run_refresh(case: Path, work: Path, args, frontier: dict | None = None) -> dict:
    spec = tomllib.loads((case / "edits.toml").read_text())
    assert_equal(json.loads((case / "expected.json").read_text()), expected_contract(spec),
                 f"{case.name}: recorded expectations vs edits.toml")
    name = case.name + ("--" + frontier["name"] if frontier else "")
    source = work / "sources" / name
    if source.exists():
        # Preserve this case's normal Cargo target between attempts. The support
        # installer deliberately refuses CARGO_TARGET_DIR overrides.
        for child in source.iterdir():
            if child.name == "target":
                continue
            if child.is_dir() and not child.is_symlink():
                shutil.rmtree(child)
            else:
                child.unlink()
    shutil.copytree(case, source, copy_function=shutil.copyfile, dirs_exist_ok=True)
    normalized = qualified_default_profile(case, source) if args.default_profile else []
    logs = work / "logs" / name
    logs.mkdir(parents=True, exist_ok=True)
    env = case_environment(spec)
    env.update(REFRESH_MEMORY_MIN_BYTES=str(6 * 1024**3), REFRESH_RUSTD_FRONTIER="1")
    if args.lane.exists():
        env["REFRESH_LANE_TOOL"] = str(args.lane)
    if args.offline:
        env["CARGO_NET_OFFLINE"] = "true"
    if args.support:
        env["CARGO_REFRESH_SUPPORT"] = str(args.support.resolve())
    # Invoke the real Cargo subcommand, with a selected controller if requested.
    if args.controller:
        controller = args.controller.resolve()
        if controller.name != "cargo-refresh":
            raise RuntimeError("--controller must name a cargo-refresh executable")
        env["PATH"] = str(controller.parent) + os.pathsep + env["PATH"]
    allowed = [frontier["outcome"]] if frontier else spec["v0_1"]
    expected_paths = ["live" if path == "works" else path for path in allowed]
    result = {"case": case.name, "configuration": frontier["name"] if frontier else "default",
              "expected-paths": expected_paths, "normalized-profiles": normalized,
              "steps": [], "result": "FAIL"}
    app = None

    def launch(index):
        eventfile = logs / f"session-{index}-events.jsonl"
        eventfile.unlink(missing_ok=True)
        command = ["nice", "-n", "19", "cargo", "refresh", "run", "--events", str(eventfile)]
        if args.archive:
            command += ["--from-archive", str(args.archive.resolve())]
        if frontier:
            command += ["--edit", ",".join(frontier["crates"])]
        instance = RefreshApp(command, source, env, logs / f"session-{index}-controller.log",
                              eventfile, args.timeout)
        try:
            instance.wait_event(0, lambda e: e["event"] == "ready", "initial ready")
            return instance
        except BaseException:
            instance.close()
            raise

    try:
        app = launch(0)
        result["startup-events"] = app.events()
        result["base"] = [app.probe() for _ in spec["base_probes"]]
        result["base-verdict"] = "PASS" if result["base"] == spec["base_probes"] else "FAIL"
        if result["base-verdict"] == "FAIL":
            result["base-error"] = f"expected {spec['base_probes']!r}, got {result['base']!r}"
            print(f"FAIL {name}: base: {result['base-error']}", file=sys.stderr, flush=True)
        # Well-formed but wrong base responses are already a failure. Still
        # apply the declared edits so a baseline bug cannot hide edit coverage.
        identity = app.identity()
        result["base-identity"] = identity
        for index, step in enumerate(spec["steps"], 1):
            row = {"name": step["name"], "expected-paths": expected_paths,
                   "output-verdict": "FAIL", "path-match": False}
            result["steps"].append(row)
            try:
                start = len(app.events())
                if step.get("environment"):
                    # Process environments are immutable from the outside. This
                    # is an explicit user relaunch, never a claimed watcher event.
                    app.close()
                    app = None
                    env.update(step["environment"])
                    # No currently supported hot environment reconfiguration API.
                    # A relaunch tests Cargo's tracked environment inputs, but
                    # does not prove an in-session stock save/fallback route.
                    for edit in step.get("edits", []):
                        apply_edit(source, edit)
                    app = launch(index)
                    event = app.wait_event(0, lambda e: e["event"] == "ready", "relaunch ready")
                    path = "session-relaunch"
                    row["delivery"] = "explicit-controller-relaunch"
                    row["limitation"] = "No in-session environment update API; no stock save event claimed"
                    row["reason"] = "environment changed through explicit controller relaunch; save path unqualified"
                else:
                    for edit in step.get("edits", []):
                        apply_edit(source, edit)
                    try:
                        event = app.wait_event(start, lambda e: e["event"] == "save" and
                                               e["path"] not in ("waiting", "skip"), step["name"])
                    except RuntimeError:
                        row["events"] = app.events()[start:]
                        row["actual-path"] = "no-result"
                        # One diagnostic probe only: stateful probes cannot be polled.
                        with contextlib.suppress(RuntimeError, OSError):
                            row["diagnostic-probe"] = app.probe()
                        raise
                    path = event["path"]
                    row["reason"] = event["reason"]
                    row["delivery"] = "watcher-save"
                row["event"] = event
                row["actual-path"] = path
                row["path-match"] = path in expected_paths
                if path not in ("live", "restart", "stock", "session-relaunch"):
                    raise RuntimeError(f"undelivered edit: {path}: {row['reason']}")
                after_identity = app.identity()
                row["before-identity"], row["after-identity"] = identity, after_identity
                assert_equal(after_identity == identity, path == "live", f"{name}: PID continuity for {path}")
                expected = step["live_expected"] if path == "live" else step["stock_after_restart"]
                actual = [app.probe() for _ in expected]
                row["expected-output"], row["observed-output"] = expected, actual
                assert_equal(actual, expected, f"{name}: {step['name']} ({path})")
                assert_equal(app.identity(), after_identity, f"{name}: no hidden restart during probes")
                row["output-verdict"] = "PASS"
                if case.name.startswith("16-") and path == "stock" and any(
                        e["event"] == "compiler_ready" for e in result["startup-events"]):
                    reset = app.wait_event(start, lambda e: e["event"] in
                                           ("worker_reset", "worker_reset_failed"), "build metadata recapture")
                    row["recapture-event"] = reset
                    if reset["event"] != "worker_reset":
                        row["recapture-verdict"] = "FAIL"
                        raise RuntimeError(f"build metadata recapture failed: {reset.get('message')}")
                    row["recapture-verdict"] = "PASS"
                elif case.name.startswith("16-") and path == "stock":
                    row["recapture-verdict"] = "NOT-APPLICABLE: compiler disabled by installation policy"
                row["events"] = app.events()[0 if path == "session-relaunch" else start:]
                identity = after_identity
            except Exception as error:
                row["error"] = str(error)
                # A failed cumulative state invalidates later live expectations.
                break
        result["result"] = "PASS" if result["base-verdict"] == "PASS" and all(
            r["output-verdict"] == "PASS" and "error" not in r for r in result["steps"]) else "FAIL"
        result["coverage-complete"] = all(r.get("delivery") == "watcher-save" for r in result["steps"])
        if result["result"] == "PASS" and not result["coverage-complete"]:
            result["result"] = "PARTIAL"
        result["path-match"] = all(r["path-match"] for r in result["steps"])
    except Exception as error:
        result["error"] = str(error)
    finally:
        if app:
            app.close()
    if "error" in result:
        print(f"FAIL {name}: {result['error']}", file=sys.stderr, flush=True)
    print(f"{result['result']} {name}: " + ", ".join(
        f"{s['name']}={s.get('actual-path', 'unavailable')} output={s['output-verdict']} "
        f"path={'match' if s['path-match'] else 'MISMATCH'}" for s in result["steps"]), flush=True)
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("stock", "refresh"), default="stock")
    parser.add_argument("--case", action="append", default=[], help="exact case name or two-digit prefix; repeatable")
    parser.add_argument("--record", action="store_true", help="record stock outputs after verifying edits.toml declarations")
    parser.add_argument("--offline", action="store_true", help="require dependencies already cached by Cargo")
    parser.add_argument("--results", type=Path, help="write compact verification JSON (including final binary hashes)")
    parser.add_argument("--controller", type=Path, help="cargo-refresh executable (default: Cargo PATH lookup)")
    parser.add_argument("--support", type=Path, help="complete verified installed-support directory")
    parser.add_argument("--archive", type=Path, help="local support archive with adjacent checksum")
    parser.add_argument("--work", type=Path, help="private work directory (default: .work or .work-refresh)")
    parser.add_argument("--lane", type=Path, default=Path(os.environ.get("REFRESH_LANE",
                        "/path/to/research/tools/lane.sh")),
                        help="research lane.sh; sessions automatically acquire rustd lane")
    parser.add_argument("--timeout", type=float, default=180, help="ready/save timeout in seconds")
    parser.add_argument("--default-profile", action="store_true",
                        help="explicit supplemental run: remove sole debug=0 dev overrides in copies; retain optimized case 11")
    parser.add_argument("--require-paths", action="store_true", help="also fail for safe path mismatches")
    args = parser.parse_args()
    if args.mode == "refresh" and args.record:
        parser.error("--record is only supported for the independent stock oracle")
    if args.mode == "refresh" and args.lane.exists() and not os.environ.get("M4_SEMANTICS_LANE_HELD"):
        env = dict(os.environ, M4_SEMANTICS_LANE_HELD="1")
        return subprocess.call([str(args.lane), "rustd", "--", sys.executable,
                                str(Path(__file__).resolve()), *sys.argv[1:]], env=env)

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
    work = (args.work or ROOT / (".work-refresh" if args.mode == "refresh" else ".work")).resolve()
    work.mkdir(parents=True, exist_ok=True)
    # Keep stable source/target paths warm, but protect them against concurrent runs.
    with (work / "runner.lock").open("w") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise RuntimeError("another semantics runner owns .work; wait for it to finish")
        results = []

        def checkpoint():
            if args.results:
                args.results.parent.mkdir(parents=True, exist_ok=True)
                report = {"mode": args.mode, "toolchain": "1.99.0",
                          "profile-mode": "qualified-default" if args.default_profile else "fixture", "cases": results}
                temporary = args.results.with_suffix(args.results.suffix + ".tmp")
                temporary.write_text(json.dumps(report, indent=2) + "\n")
                temporary.replace(args.results)

        for case in cases:
            try:
                if args.mode == "stock":
                    results.append(run_stock(case, work, args.offline, args.record, args.default_profile))
                else:
                    spec = tomllib.loads((case / "edits.toml").read_text())
                    for frontier in spec.get("frontiers", [None]):
                        results.append(run_refresh(case, work, args, frontier))
            except Exception as error:
                results.append({"case": case.name, "result": "FAIL", "error": str(error)})
                print(f"FAIL {case.name}: {error}", file=sys.stderr, flush=True)
            checkpoint()
        failures = sum(row["result"] != "PASS" for row in results)
        unit = "configurations" if args.mode == "refresh" else "cases"
        print(f"{len(results)-failures}/{len(results)} {unit} passed", flush=True)
        path_mismatches = sum(not row.get("path-match", True) for row in results)
        if args.mode == "refresh":
            print(f"{path_mismatches} configuration(s) with path mismatches (separate from output failures)", flush=True)
        return bool(failures or (args.require_paths and path_mismatches))


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (RuntimeError, OSError, ValueError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        sys.exit(1)
