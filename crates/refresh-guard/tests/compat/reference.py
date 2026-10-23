"""Read-only access to the exact research prototypes; no daemon or Cargo build."""
import functools
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys

sys.dont_write_bytecode = True


def module(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def rust_source(path):
    return Path(path).suffix == '.rs' and Path(path).name != 'build.rs'


class Reference:
    def __init__(self, research):
        self.root = Path(research)
        self.guard = self.root / 'work/m0/v4/gw/bin/refresh-guard-m0'
        self.names = self.root / 'experiments/EXP-119-strict-gate-preemption/bin/strict-admission119'
        self.policy = module(self.root / 'experiments/EXP-128-fast-refresh-v4/gw/scripts/source_guard.py', 'guard_policy')
        self.env = dict(os.environ, M0_CERTS='0')

    @functools.lru_cache(maxsize=512)
    def run(self, binary, source):
        result = subprocess.run([str(binary)], input=source.encode(), capture_output=True, env=self.env, check=False)
        if result.returncode:
            raise ValueError(result.stderr.decode().strip())
        return json.loads(result.stdout)

    def analyze(self, sources):
        return {path: self.run(self.guard, text) if Path(path).suffix == '.rs'
                else {'raw': hashlib.sha256(text.encode()).hexdigest()}
                for path, text in sorted(sources.items())}

    def classify(self, before, after):
        try:
            old, new = self.analyze(before), self.analyze(after)
        except ValueError as error:
            return 'reject', str(error), []
        reason, owners = self.policy.classify109(old, new)
        if reason:
            return 'restart', reason, owners
        if owners:
            return 'restart', 'async owner refused (M0_CERTS=0)', owners
        return 'live', None, []

    def admission(self, before, after):
        pairs = []
        try:
            for path in sorted(before.keys() | after.keys()):
                if rust_source(path):
                    pairs.append((path, self.run(self.names, before.get(path, '')),
                                  self.run(self.names, after.get(path, ''))))
        except ValueError:
            return 'parse'
        # Apply EXP-119 rule order globally over the supplied per-file scope.
        checks = []
        for path, old, new in pairs:
            known = set(old['identifiers']) | set(new['functions'])
            sites = set(map(tuple, new['expression_sites'])) - set(map(tuple, old['expression_sites']))
            prefix = {name for _, name in sites if name not in new['functions']
                      and any(k != name and k.startswith(name) for k in known)}
            checks.append((prefix, set(new['bad_returns']) - set(old['bad_returns']),
                           (set(new['references']) - set(old['references'])) & set(new['functions']),
                           set(new['calls']) - set(old['calls']) - set(new['functions'])))
        for index, rule in enumerate(['incomplete-expression', 'missing-return-tail', 'bare-function-reference', 'unknown-call']):
            if any(check[index] for check in checks):
                return rule
        for path, _, _ in pairs:
            try:
                a = self.run(self.guard, before.get(path, ''))
                b = self.run(self.guard, after.get(path, ''))
            except ValueError:
                return 'parse'
            if a['functions'].keys() - b['functions'].keys():
                return 'removed-function'
        return 'admit'


def load_case(case, root):
    def sources(files):
        return {path: source if isinstance(source, str) else (root / source['file']).read_text()
                for path, source in files.items()}
    return sources(case['before']), sources(case['after'])


class Probe:
    def __init__(self, binary):
        self.process = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)

    def ask(self, before, after, **kwargs):
        self.process.stdin.write(json.dumps(dict(before=before, after=after, **kwargs)) + '\n')
        self.process.stdin.flush()
        line = self.process.stdout.readline()
        if not line:
            raise RuntimeError(f'guard_probe exited: {self.process.poll()}')
        return json.loads(line)

    def close(self):
        self.process.stdin.close()
        self.process.wait(timeout=10)
        assert self.process.returncode == 0
