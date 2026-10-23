#!/usr/bin/env bash
# Usage: capture.sh /absolute/research/root /absolute/output/root
set -euo pipefail
research=$(realpath "$1")
output=$(realpath -m "$2")
script_dir=$(cd -- "$(dirname -- "$0")" && pwd)
if [[ ${M2_CAPTURE_LANE:-0} != 1 ]]; then
  exec "$research/tools/lane.sh" rustd -- env M2_CAPTURE_LANE=1 "$script_dir/capture.sh" "$research" "$output"
fi
[[ ! -e "$output/goldens" && ! -e "$output/session" ]] || { echo 'Use fresh goldens/session destinations.' >&2; exit 1; }
mkdir -p "$output"
export M2_V4="$research/work/m0/v4"
export PYTHONPATH="$script_dir${PYTHONPATH:+:$PYTHONPATH}"
export M2_GOLDENS="$output/goldens" M0_RESULTS="$output/session"
export M0_RUSTD_BIN="${M0_RUSTD_BIN:-$research/work/m0/nocert-bin}"
export M0_CERTS=0 M0_NO_CERTS=1 M0_SAVE_LIMIT=28 RUSTUP_TOOLCHAIN=1.99.0 CARGO_BUILD_JOBS=4
export M0_OWNER="$output"
unset E122_BASELINE
# Do not remove any target directory which already exists (a peer may own it).
python3 - "$M2_V4" "$output" <<'PY'
import pathlib,sys,json
v4=pathlib.Path(sys.argv[1]);out=pathlib.Path(sys.argv[2])
(out/'targets-before.json').write_text(json.dumps([str(p) for p in (v4/'gw/work').rglob('target') if p.is_dir()],indent=2))
PY
cd "$M2_V4"
./tools/fast-refresh session --config configs/web.toml
python3 gw/scripts/stock_oracle.py

python3 "$script_dir/summarize_capture.py" "$output"
