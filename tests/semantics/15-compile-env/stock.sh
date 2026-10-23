#!/usr/bin/env bash
set -euo pipefail
exec python3 "$(dirname "$0")/../run.py" --mode stock --case "15-compile-env" "$@"
