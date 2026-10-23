#!/usr/bin/env python3
"""Port of EXP-104b collect104.py: validate raw facts and write schema 1 bundle."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

def require(condition, message):
    if not condition:
        raise SystemExit(message)


parser = argparse.ArgumentParser()
parser.add_argument('toolchain')
parser.add_argument('output', type=Path)
parser.add_argument('--reference', type=Path)
parser.add_argument('--reference-facts', type=Path)
args = parser.parse_args()
data = (args.output / 'facts.jsonl').read_bytes()
rows = [json.loads(line) for line in data.splitlines()]
manifests = [r for r in rows if r['kind'] == 'manifest']
require(len(manifests) == 1, 'missing/ambiguous manifest')
m = manifests[0]
vv = subprocess.check_output(['rustc', '+' + args.toolchain, '-Vv'], text=True).strip()
require(m['rustc'].strip() == vv, 'rustc identity mismatch')
require(m['target'] == 'x86_64-unknown-linux-gnu', 'unsupported target')
require(m['panic_strategy'] == 'Unwind', 'unsupported panic strategy')
require(not any(r['kind'] == 'fn_error' for r in rows), 'incomplete ABI facts')
simd = [r for r in rows if r['kind'] == 'fn' and
        (r['name'].startswith('callee:abi_sse_load:') or r['name'].endswith('::abi_sse_load'))]
require(simd, 'SIMD128 sample absent')
modes = {r['ret']['mode'].split('(')[0].split(' ')[0].lower() for r in simd}
require(len(modes) == 1 and modes <= {'direct', 'indirect'}, 'ambiguous SIMD carrier')
personalities = [r['symbol'] for r in rows if r['kind'] == 'lang_item' and r['name'] == 'EhPersonality']
require(len(personalities) == 1, 'missing/ambiguous EH personality')
commit = next(line.split(': ', 1)[1] for line in vv.splitlines() if line.startswith('commit-hash: '))
# Only the already qualified baseline uses mirrored TypeIds. Others remain candidates.
bundle = dict(schema=1, commit_hash=commit, rustc_vv=vv, target=m['target'],
              simd128=modes.pop(), personality=personalities[0],
              type_id_source='mirrored' if vv.startswith('rustc 1.99.0 ') else 'rustc-symbol',
              source_sha256=hashlib.sha256(data).hexdigest(), fixture='web')
path = args.output / 'abi-facts.bundle'
path.write_text(json.dumps(bundle, indent=2) + '\n')
if args.reference:
    reference = json.loads(args.reference.read_text())
    current_fields = {k: v for k, v in bundle.items() if k != 'source_sha256'}
    reference_fields = {k: v for k, v in reference.items() if k != 'source_sha256'}
    require(current_fields == reference_fields, 'toolchain ABI policy differs from qualified reference')
    if bundle != reference:
        require(args.reference_facts, 'need historical raw stream to explain digest mismatch')
        old_data = args.reference_facts.read_bytes()
        require(hashlib.sha256(old_data).hexdigest() == reference['source_sha256'], 'historical facts digest mismatch')
        old_rows = [json.loads(line) for line in old_data.splitlines()]
        require(len(rows) == len(old_rows), 'fact count changed')
        changes = []
        for old, new in zip(old_rows, rows):
            if old == new:
                continue
            # Cargo hashes local path-package identities with their source location.
            # Fixture-local ADTs and reference types with local late-bound regions change.
            # All layouts/ABI/other IDs must match the historical stream.
            allowed = {'domain::Project', 'domain::RecordStatus', 'domain::VersionStamp', 'domain::Ticket',
                       '&mut std::collections::HashMap<u32, std::sync::Arc<str>>', '&axum::Router',
                       '&tower::util::BoxCloneSyncService<axum::http::Request<axum::body::Body>, axum::http::Response<axum::body::Body>, std::convert::Infallible>'}
            require(old['kind'] == new['kind'] == 'type' and old['name'] in allowed, 'unexpected fact drift')
            require({k: v for k, v in old.items() if k != 'type_id'} == {k: v for k, v in new.items() if k != 'type_id'}, 'layout/fact drift')
            changes.append(new['name'])
        (args.output / 'reference-comparison.json').write_text(json.dumps(dict(
            rebuilt_source_sha256=bundle['source_sha256'], retained_source_sha256=reference['source_sha256'],
            relocated_local_typeids=changes, policy='retain qualified bundle; never relabel rebuilt raw-stream hash'), indent=2) + '\n')
        print(f'Rebuilt raw digest differs ONLY in fixture-identity-sensitive TypeIds: {changes}')
        # Keep the rebuilt candidate with truthful raw-stream digest for review.
        path.rename(args.output / 'rebuilt-abi-facts.bundle')
    path.write_bytes(args.reference.read_bytes())
    print('PASS: ABI facts match retained qualification; abi-facts.bundle is the unchanged qualified reference (byte-identical)')
print(f"SIMD128={bundle['simd128']}; EH personality={bundle['personality']}; bundle={path}")
