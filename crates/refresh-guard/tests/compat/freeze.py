#!/usr/bin/env python3
"""Freeze research inputs into portable before/after cases (never edits research)."""
import argparse
import hashlib
import json
from pathlib import Path
import tomllib
from reference import Reference, module

HERE = Path(__file__).resolve().parent
CASES = HERE.parent / 'cases'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--research', type=Path, required=True)
    args = parser.parse_args()
    ref = Reference(args.research)
    provenance = []

    def freeze(text):
        data = text.encode()
        name = 'sources/' + hashlib.sha256(data).hexdigest() + '.rs'
        (CASES / name).parent.mkdir(parents=True, exist_ok=True)
        (CASES / name).write_bytes(data)
        return {'file': name}

    def case(case_id, before, after, origin, expected_class=None, comment=None):
        decision, reason, owners = ref.classify(before, after)
        gate = ref.admission(before, after)
        if expected_class is not None:
            assert decision == expected_class, (case_id, decision, expected_class, reason)
        expected = {'admission': gate, 'class': 'stock' if decision == 'reject' else decision}
        if decision == 'reject':
            expected['reason'] = 'parse'
        result = dict(id=case_id, before={p: freeze(t) for p, t in before.items()},
                      after={p: freeze(t) for p, t in after.items()}, expected=expected,
                      origin=origin, prototype={'class': decision, 'reason': reason, 'owners': owners})
        if comment:
            result['comment'] = comment
        return result

    scenarios_path = args.research / 'experiments/EXP-128-fast-refresh-v4/gw/scripts/scenarios.py'
    scenarios = module(scenarios_path, 'web_scenarios')
    project = args.research / 'work/m0/v4/gw/work/web'
    previous = {p: (project / p).read_text() for p in [scenarios.HAND, scenarios.DOMAIN]}
    steps = list(scenarios.generate(project))
    assert len(steps) == 64
    event_path = args.research / 'work/m0/v4/results/smoke-c/live-session/events.jsonl'
    events = {r['generation']: r for r in map(json.loads, event_path.read_text().splitlines())
              if r['event'] in ('save', 'build_error')}
    assert len(events) == 64
    web = []
    for index, step in enumerate(steps, 1):
        event = events[index]
        decision = 'reject' if event['event'] == 'build_error' else ('live' if event['path'] == 'live' else 'restart')
        item = case(f'web-{index:02}-{step["kind"]}', previous, step['sources'],
                    f'EXP-128 scenarios.py save {index}; M0 smoke-c events generation {index}', decision,
                    'Before is last successfully analyzed AST, not the rejected syntax buffer.' if index == 55 else None)
        item['event_path'] = event.get('path', event['event'])
        web.append(item)
        if decision != 'reject':
            previous = step['sources']
    (CASES / 'web-session.json').write_text(json.dumps(web, indent=2) + '\n')

    fixtures = []
    for manifest in sorted((args.research / 'fixtures').glob('*/fixture.toml')):
        data = tomllib.loads(manifest.read_text())
        for scenario in data['scenario']:
            path = scenario['file']
            before = (manifest.parent / path).read_text()
            if 'find' in scenario:
                assert before.count(scenario['find']) == 1, (manifest, scenario['id'])
                after = before.replace(scenario['find'], scenario['replace'], 1)
            else:
                after = before
            fixtures.append(case(f'fixture-{data["name"]}-{scenario["id"]}', {path: before}, {path: after},
                                 str(manifest.relative_to(args.research)) + ':' + scenario['id']))
    (CASES / 'fixtures.json').write_text(json.dumps(fixtures, indent=2) + '\n')

    exp055 = args.research / 'experiments/EXP-055-async-patch-safety'
    edits = module(exp055 / 'scripts/cases.py', 'async_cases')
    asynchronous = []
    for kind in edits.KINDS:
        for variant in ['a', 'b', 'c', 'd_add', 'd_remove', 'f']:
            asynchronous.append(case(f'exp055-{kind}-{variant}', {'async_cases.rs': edits.BASE},
                                     {'async_cases.rs': edits.edit(variant, kind)},
                                     f'EXP-055 cases.py {kind}/{variant}', 'restart'))
    asynchronous.append(case('exp055-helper-e', {'async_cases.rs': edits.BASE}, {'async_cases.rs': edits.edit('e')},
                             'EXP-055 cases.py helper/e', 'restart',
                             'The helper is #[inline(never)]; its attributed body is guarded even though synchronous.'))
    (CASES / 'async-exp055.json').write_text(json.dumps(asynchronous, indent=2) + '\n')
    # Exact 1,733-line benchmark source, distinct from 1,742-line M0 prepared source.
    bench = args.research / 'fixtures/web/crates/api/src/handlers.rs'
    (CASES / 'web-handlers-1733.rs').write_bytes(bench.read_bytes())
    for path in [ref.guard, ref.names, scenarios_path, event_path, bench,
                 args.research / 'work/m0/v4/gw/tools/guard/src/main.rs',
                 args.research / 'experiments/EXP-128-fast-refresh-v4/gw/scripts/source_guard.py']:
        provenance.append(dict(path=str(path.relative_to(args.research)), sha256=hashlib.sha256(path.read_bytes()).hexdigest()))
    (CASES / 'provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
    print(json.dumps(dict(web=len(web), fixtures=len(fixtures), async_cases=len(asynchronous),
                          source_files=len(list((CASES / 'sources').iterdir())))))


if __name__ == '__main__':
    main()
