#!/usr/bin/env python3
"""EXP-104b wrapper, with outputs confined to the caller's private directory."""
import json
import os
from pathlib import Path
import subprocess
import sys

args = sys.argv[1:]
result = subprocess.run(args)
if result.returncode:
    sys.exit(result.returncode)
if '--crate-name' in args and args[args.index('--crate-name') + 1] == 'abi_probe':
    out = Path(os.environ['REFRESH_FACTS_OUT'])
    (out / 'invocation.json').write_text(json.dumps(dict(argv=args, cwd=os.getcwd()), indent=2) + '\n')
    with (out / 'facts.jsonl').open('w') as stream:
        sys.exit(subprocess.run([os.environ['REFRESH_FACTS_HELPER'], *args[1:]], stdout=stream).returncode)
