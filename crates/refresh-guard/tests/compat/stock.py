#!/usr/bin/env python3
"""Stock rustc 1.99 oracle for the annotated std-only admission boundary cases."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
from reference import load_case


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent / 'cases'
    cases = json.loads((root / 'boundaries.json').read_text())
    rows = []
    env = dict(os.environ, RUSTUP_TOOLCHAIN='1.99.0')
    with tempfile.TemporaryDirectory(prefix='refresh-guard-stock-') as scratch:
        directory = Path(scratch)
        for case in cases:
            if 'stock' not in case:
                continue
            available = int(next(line.split()[1] for line in Path('/proc/meminfo').read_text().splitlines()
                                 if line.startswith('MemAvailable:'))) * 1024
            if available < 6 * 1024**3:
                raise MemoryError('MemAvailable below 6 GiB')
            _, after = load_case(case, root)
            assert list(after) == ['lib.rs']
            (directory / 'lib.rs').write_text(after['lib.rs'])
            command = ['nice', '-n', '19', 'rustc', '--edition=2024', '--crate-type=lib', '--emit=metadata',
                       '--error-format=json', '-o', str(directory / 'case.rmeta'), str(directory / 'lib.rs')]
            run = subprocess.run(command, env=env, capture_output=True, text=True, check=False)
            observed = 'valid' if run.returncode == 0 else 'invalid'
            rows.append(dict(id=case['id'], expected=case['stock'], observed=observed,
                             admission=case['expected']['admission'], command=command,
                             diagnostics=[json.loads(line) for line in run.stderr.splitlines()],
                             loadavg=Path('/proc/loadavg').read_text().strip(), available_bytes=available))
    result = dict(rustc=subprocess.check_output(['rustc', '--version', '--verbose'], env=env, text=True),
                  count=len(rows), disagreements=[row['id'] for row in rows if row['expected'] != row['observed']], rows=rows)
    args.out.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({key: result[key] for key in ['count', 'disagreements']}))
    assert not result['disagreements']


if __name__ == '__main__':
    main()
