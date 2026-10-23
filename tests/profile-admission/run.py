#!/usr/bin/env python3
"""Qualify assertion/overflow combinations and stripped symbols against stock."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--controller", type=Path)
parser.add_argument("--support", type=Path)
parser.add_argument("--work", type=Path, required=True)
parser.add_argument("--stock-only", action="store_true")
parser.add_argument("--refresh-only", action="store_true")
parser.add_argument("--timeout", type=float, default=180)
args = parser.parse_args()
args.lane = Path("/path/to/research/tools/lane.sh")
if not args.stock_only and not os.environ.get("M4_SEMANTICS_LANE_HELD"):
    sys.exit(subprocess.call([str(args.lane), "rustd", "--", sys.executable, str(Path(__file__).resolve()), *sys.argv[1:]],
                             env=dict(os.environ, M4_SEMANTICS_LANE_HELD="1")))
module = importlib.util.spec_from_file_location("semantics", ROOT.parent / "semantics/run.py")
runner = importlib.util.module_from_spec(module)
module.loader.exec_module(runner)
args.archive, args.offline, args.default_profile = None, True, False
args.work = args.work.resolve()
args.work.mkdir(parents=True, exist_ok=True)
results = []
variants = [(f"assertions-{str(debug).lower()}-overflow-{str(overflow).lower()}", debug, overflow, None)
            for debug in (False, True) for overflow in (False, True)]
variants.append(("strip-symbols", False, False, "symbols"))
for name, debug, overflow, strip in variants:
    case = args.work / "fixtures" / name
    shutil.copytree(ROOT / "fixture", case, dirs_exist_ok=True)
    manifest = (case / "Cargo.toml").read_text().replace("debug-assertions = false", f"debug-assertions = {str(debug).lower()}").replace("overflow-checks = false", f"overflow-checks = {str(overflow).lower()}")
    if strip:
        manifest += f'\nstrip = "{strip}"\n'
    (case / "Cargo.toml").write_text(manifest)
    profile = f";cfg={str(debug).lower()};debug={str(debug).lower()};overflow={str(overflow).lower()}"
    edits = (case / "edits.toml").read_text().replace(";cfg=false;debug=false;overflow=false", profile)
    (case / "edits.toml").write_text(edits)
    (case / "expected.json").write_text(json.dumps(runner.expected_contract(tomllib.loads(edits)), indent=2) + "\n")
    work = args.work / name
    work.mkdir(exist_ok=True)
    row = {"profile": name}
    if not args.refresh_only:
        row["stock"] = runner.run_stock(case, work, True, True)
    if not args.stock_only:
        row["refresh"] = runner.run_refresh(case, work, args)
    results.append(row)
    (args.work / ("stock.json" if args.stock_only else "refresh.json")).write_text(json.dumps(results, indent=2) + "\n")
sys.exit(any(result[mode]["result"] != "PASS"
             or (mode == "refresh" and not result[mode].get("path-match", False))
             for result in results for mode in ("stock", "refresh") if mode in result))
