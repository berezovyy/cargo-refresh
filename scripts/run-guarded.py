#!/usr/bin/env python3
"""Run an owned process group; stop it if MemAvailable drops below 6 GiB."""
import os
import signal
import subprocess
import sys
from pathlib import Path


def available():
    return next(int(line.split()[1]) for line in Path('/proc/meminfo').read_text().splitlines()
                if line.startswith('MemAvailable:'))


def main():
    if available() < 6 * 1024 * 1024:
        raise SystemExit('MemAvailable < 6 GiB; refusing to start')
    child = subprocess.Popen(sys.argv[1:], start_new_session=True)

    def stop(*_):
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()

    def interrupted(signum, _):
        stop()
        raise SystemExit(128 + signum)

    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, interrupted)
    try:
        while child.poll() is None:
            if available() < 6 * 1024 * 1024:
                stop()
                raise SystemExit('MemAvailable < 6 GiB; stopped owned process group')
            try:
                child.wait(timeout=1)
            except subprocess.TimeoutExpired:
                pass
        return child.returncode
    finally:
        stop()


if __name__ == '__main__':
    sys.exit(main())
