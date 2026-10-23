#!/usr/bin/env python3
"""Inspect versioned symbols/DT_NEEDED without loading the artifacts."""
from pathlib import Path
import re
import subprocess
import sys

root = Path(sys.argv[1])
print('Runtime requirements measured on this build host; oldest-distro loading is NOT qualified.')
for name in ('librefresh.so', 'rustd', 'cargo-rustd', 'rustd-rustc'):
    symbols = subprocess.check_output(['objdump', '-T', str(root / name)], text=True)
    versions = set(re.findall(r'GLIBC_(\d+(?:\.\d+)+)', symbols))
    newest = max(versions, key=lambda s: tuple(map(int, s.split('.')))) if versions else 'none'
    dynamic = subprocess.check_output(['objdump', '-p', str(root / name)], text=True)
    needed = re.findall(r'^\s*NEEDED\s+(\S+)', dynamic, re.M)
    print(f'{name}: newest GLIBC_{newest}; DT_NEEDED={", ".join(needed)}')
print('If libunwind.so.8/libunwind-x86_64.so.8 is listed, install your distro libunwind runtime package.')
print('On Debian/Ubuntu the package is libunwind8. No compiler development package is needed at runtime.')
