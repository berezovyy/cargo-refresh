#!/usr/bin/env python3
"""Reject any payload whose versioned glibc imports exceed the release floor."""
import re
import subprocess
import sys
from pathlib import Path

root, ceiling = Path(sys.argv[1]), tuple(map(int, sys.argv[2].split('.')))
for name in ('rustd', 'cargo-rustd', 'rustd-rustc', 'librefresh.so'):
    symbols = subprocess.check_output(['objdump', '-T', str(root / name)], text=True)
    versions = [tuple(map(int, v.split('.'))) for v in re.findall(r'GLIBC_(\d+(?:\.\d+)+)', symbols)]
    if 'GLIBC_PRIVATE' in symbols or any(v > ceiling for v in versions):
        raise SystemExit(f'{name}: glibc exceeds {sys.argv[2]}')
    print(f'{name}: GLIBC_{".".join(map(str, max(versions)))} <= {sys.argv[2]}')
