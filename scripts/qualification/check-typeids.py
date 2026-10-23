#!/usr/bin/env python3
"""Compare rustc_driver TypeId facts with runtime matching-stock TypeId values."""
import json
from pathlib import Path
import re
import sys

runtime = dict(line.split(' ', 1) for line in Path(sys.argv[1]).read_text().splitlines())
rows = [json.loads(line) for line in Path(sys.argv[2]).read_text().splitlines()]
roots = {r['name'].split('::')[-1]: r['ty'] for r in rows if r['kind'] == 'root'}
types = {r['name']: r['type_id'] for r in rows if r['kind'] == 'type'}
count = 0
for name, value in runtime.items():
    if name == 'function-item':
        continue  # Full rustd generic function-item spelling is a manual gate.
    match = re.fullmatch(r'TypeId\(0x([0-9a-f]+)\)', value)
    if not match or name not in roots or types[roots[name]] != match[1].zfill(32):
        raise SystemExit(f'TypeId mismatch for {name}: {value}')
    count += 1
if count != 8:
    raise SystemExit(f'Incomplete TypeId matrix ({count}/8)')
print(f'PASS: {count} compiler facts equal matching stock runtime TypeIds')
