#!/usr/bin/env python3
"""Stock oracle for dormant coroutine/closure instances across semantic promotions.

Run under the rustd resource lane. Set RUSTD_BIN and TMPDIR to disk-backed paths.
--build-lane optionally names lane.sh for the one initial Cargo capture (-j 4).
Every source revision is independently compiled with stock rustc; no timing claims.
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import time

from difftest import Fixture, RUSTD, sysroot_rlibs
import linkline


PREFIX = """use std::future::Future;
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(value) => value,
        _ => panic!("unexpected pending"),
    }
}
#[inline(never)]
fn via_new<F: Future>(future: F) -> F::Output {
    if std::hint::black_box(true) { block_on(future) } else { panic!("unreachable") }
}
#[inline(never)]
fn evaluate() {
"""


def source(kind, step):
    value = 555 + 17 * (step + 1)
    caller = "via_new" if kind == "async-new-wrapper" and step >= 0 else "block_on"
    arg = f"{caller}(async {{ {value}i64 }})" if kind.startswith("async") else f"(|| {value}i64)()"
    args = f"{arg}; 3" if step % 2 == 0 else f"{arg}, {arg} + 1, {arg} - 2"
    return PREFIX + '    println!("{:?}", vec![' + args + ']);\n}\nfn main() { evaluate(); }\n'


def observation(binary):
    ran = subprocess.run([str(binary)], capture_output=True, timeout=15)
    return dict(stdout=ran.stdout.decode(), stderr=ran.stderr.decode(), exit=ran.returncode)


def run_case(work, kind, steps, build_lane):
    work.mkdir(parents=True, exist_ok=True)
    (work / "src").mkdir(exist_ok=True)
    src = work / "src/main.rs"
    src.write_text(source(kind, -1))
    (work / "Cargo.toml").write_text(
        '[workspace]\n[package]\nname="coroutine_restart"\nversion="0.0.0"\nedition="2024"\n'
        '[profile.dev]\ndebug=0\n')
    fx = Fixture(str(work), ["coroutine_restart"], ["--bin", "coroutine_restart", "-j", "4"], touch=str(src))
    # Fixture owns the linker capture and rustc symbol selection, as in editloop.py.
    # Its Cargo call is wrapped only for this setup build when lanes are requested.
    if build_lane:
        capture = work / "capture.py"
        capture.write_text(
            "import sys\nsys.path.insert(0, " + repr(str(Path(__file__).resolve().parent)) + ")\n"
            "from difftest import Fixture\nimport json\n"
            f"f=Fixture({str(work)!r}, ['coroutine_restart'], ['--bin','coroutine_restart','-j','4'], touch={str(src)!r})\n"
            "f.rustc_build()\n"
            f"open({str(work / 'capture.json')!r},'w').write(json.dumps([f.link_args,f.reference,f.rlibs]))\n")
        subprocess.run([str(build_lane), "build", "--", "nice", "-n", "19", "python3", str(capture)], check=True)
        fx.link_args, fx.reference, fx.rlibs = json.loads((work / "capture.json").read_text())
    else:
        fx.rustc_build()
    obj, patch, binary = (str(work / name) for name in ("base.o", "patch.o", "program"))
    link = linkline.ahead(fx.link_args, [patch, obj], RUSTD)
    link = linkline.with_output(link, binary)
    symbols = [arg for file in fx.rlibs + sysroot_rlibs() for arg in ("--symbols", file)]
    target = ["--out", obj, "--patch", patch, "--link-cmd", shlex.join(link), "--splice", binary, *symbols]
    socket = str(work / "socket")
    if len(socket.encode()) >= 104:
        raise RuntimeError("choose a shorter --work path for the Unix socket")
    log_path = work / "daemon.log"
    results = []
    with log_path.open("w") as log:
        daemon = subprocess.Popen([RUSTD, str(work), "--crate", "coroutine_restart", "--socket", socket, *target],
                                  stdout=log, stderr=log, start_new_session=True)
        try:
            deadline = time.monotonic() + 180
            while "watching" not in log_path.read_text():
                if daemon.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError(f"daemon failed to warm: {log_path}")
                time.sleep(.025)
            assert observation(binary) == observation(fx.reference), "startup differs from stock"
            for step in range(steps):
                src.write_text(source(kind, step))
                oracle = work / "stock"
                subprocess.run(["rustc", "--edition=2024", "--crate-name", "coroutine_restart", "-C", "debuginfo=0",
                                str(src), "-o", str(oracle)], check=True, capture_output=True)
                reply = subprocess.run([RUSTD, "sync", socket, "--crate", "coroutine_restart", "--promote", "true", *target],
                                       capture_output=True, text=True, timeout=180)
                if reply.returncode or not reply.stdout.startswith("ok") or "native-rebase=" in reply.stdout:
                    raise RuntimeError(f"expected promoted patch: {reply.stdout} {reply.stderr}")
                expected, got = observation(oracle), observation(binary)
                row = dict(kind=kind, step=step, equal=got == expected, reference=expected, treatment=got)
                results.append(row)
                (work / "results.json").write_text(json.dumps(results, indent=2) + "\n")
                assert row["equal"], row
            assert log_path.read_text().count("loader-promote126:") >= steps, "promotion was not exercised"
        finally:
            os.killpg(daemon.pid, signal.SIGTERM) if daemon.poll() is None else None
            try:
                daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(daemon.pid, signal.SIGKILL)
                daemon.wait()
    print(f"{kind}: startup + {steps} promoted edits match stock", flush=True)
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--steps", type=int, default=10)
    parser.add_argument("--kind", choices=("async", "async-new-wrapper", "closure"), action="append")
    parser.add_argument("--build-lane", type=Path)
    args = parser.parse_args()
    for kind in args.kind or ("async", "async-new-wrapper", "closure"):
        run_case(args.work.resolve() / kind, kind, args.steps, args.build_lane)


if __name__ == "__main__":
    main()
