#!/usr/bin/env python3
"""Compare every frozen pair with actual strict M0 + classify109, certificates off."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
from reference import Probe, Reference, load_case


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--research', type=Path, required=True)
    parser.add_argument('--probe', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent / 'cases'
    ref = Reference(args.research)
    probe = Probe(args.probe)
    rows = []
    file_rows = []
    disagreements = []
    try:
        for suite in ['web-session.json', 'fixtures.json', 'async-exp055.json', 'boundaries.json']:
            for case in json.loads((root / suite).read_text()):
                before, after = load_case(case, root)
                old_class, reason, owners = ref.classify(before, after)
                gate = ref.admission(before, after)
                new = probe.ask(before, after)
                new_class = 'reject' if new['reason'] == 'parse' else new['class']
                # The only policy correction: configuration edits go to stock.
                manifest_fix = new_class == 'stock' and new['reason'] == 'manifest'
                agree = new_class == old_class or manifest_fix
                row = dict(id=case['id'], suite=suite, prototype_class=old_class,
                           rust_class=new_class, prototype_reason=reason, rust_reason=new['reason'],
                           owners=owners, admission=gate, rust_admission=new['admission'],
                           manifest_fix=manifest_fix, agree=agree and gate == new['admission'])
                rows.append(row)
                if not row['agree']:
                    disagreements.append(row)
                if suite == 'web-session.json':
                    for path in sorted(before.keys() | after.keys()):
                        a = {path: before[path]} if path in before else {}
                        b = {path: after[path]} if path in after else {}
                        file_class, file_reason, _ = ref.classify(a, b)
                        file_gate = ref.admission(a, b)
                        file_new = probe.ask(a, b)
                        observed = 'reject' if file_new['reason'] == 'parse' else file_new['class']
                        file_row = dict(id=case['id'], path=path, prototype_class=file_class,
                                        rust_class=observed, prototype_reason=file_reason,
                                        agree=observed == file_class and file_gate == file_new['admission'])
                        file_rows.append(file_row)
                        if not file_row['agree']:
                            disagreements.append(file_row)
    finally:
        probe.close()
    result = dict(rows=rows, count=len(rows), file_rows=file_rows, web_file_pairs=len(file_rows), disagreements=disagreements,
                  web_counts=dict(Counter(row['rust_class'] for row in rows if row['suite'] == 'web-session.json')),
                  manifest_fixes=sum(row['manifest_fix'] for row in rows),
                  binaries={str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in [ref.guard, ref.names, args.probe]})
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({k: result[k] for k in ['count', 'web_file_pairs', 'web_counts', 'manifest_fixes', 'disagreements']}))
    assert not disagreements


if __name__ == '__main__':
    main()
