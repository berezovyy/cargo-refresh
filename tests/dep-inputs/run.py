#!/usr/bin/env python3
"""Watch existing embedded inputs and inputs introduced by a changed include path."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--controller", type=Path)
parser.add_argument("--support", type=Path)
parser.add_argument("--work", type=Path, required=True)
parser.add_argument("--stock", action="store_true")
parser.add_argument("--timeout", type=float, default=180)
args = parser.parse_args()
args.lane = Path("/path/to/research/tools/lane.sh")
if not args.stock and not os.environ.get("M4_SEMANTICS_LANE_HELD"):
    sys.exit(subprocess.call([str(args.lane), "rustd", "--", sys.executable, str(Path(__file__).resolve()), *sys.argv[1:]],
                             env=dict(os.environ, M4_SEMANTICS_LANE_HELD="1")))
module = importlib.util.spec_from_file_location("semantics", ROOT.parent / "semantics/run.py")
runner = importlib.util.module_from_spec(module)
module.loader.exec_module(runner)
args.archive, args.offline, args.default_profile = None, True, False
args.work = args.work.resolve()
args.work.mkdir(parents=True, exist_ok=True)
result = (runner.run_stock(ROOT / "fixture", args.work, True, False) if args.stock else
          runner.run_refresh(ROOT / "fixture", args.work, args))
(args.work / ("stock.json" if args.stock else "refresh.json")).write_text(json.dumps(result, indent=2) + "\n")
sys.exit(result["result"] != "PASS")
