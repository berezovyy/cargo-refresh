#!/usr/bin/env bash
# Manual candidate qualification, never publishes or marks an unknown compiler supported.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
usage() { echo 'Usage: scripts/new-toolchain.sh <toolchain> [--dry-run]'; }
[[ $# -ge 1 && $# -le 2 ]] || { usage >&2; exit 2; }
TC=$1
DRY=0
if [[ $# == 2 ]]; then [[ $2 == --dry-run ]] || { usage >&2; exit 2; }; DRY=1; fi
source "$ROOT/scripts/packaging-common.sh"
QUALIFICATION=$ROOT/scripts/qualification
export RUSTUP_TOOLCHAIN=$TC
export CARGO_BUILD_JOBS=4
# These scripts qualify only the default x86-64 GNU ABI, never an inherited CPU/profile override.
for key in RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_TARGET RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER; do
  if [[ -n ${!key:-} ]]; then echo "Unset $key before qualification" >&2; exit 2; fi
done
COMMIT=$(rustc -Vv | sed -n 's/^commit-hash: //p')
[[ $COMMIT =~ ^[0-9a-f]{40}$ ]] || { echo 'Missing exact rustc commit' >&2; exit 1; }
[[ $(rustc -vV | sed -n 's/^host: //p') == x86_64-unknown-linux-gnu ]] || { echo 'Only x86_64 GNU Linux is supported' >&2; exit 1; }
OUT=${REFRESH_QUALIFICATION_OUT:-$ROOT/.release-work/qualification/$COMMIT}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
exec > >(tee "$OUT/qualification.log") 2>&1
rustc -Vv > "$OUT/rustc.txt"
cargo -Vv > "$OUT/cargo.txt"
echo "Candidate $TC ($COMMIT); output $OUT; dry-run=$DRY"
require_rustc_dev
SYSROOT=$(rustc --print sysroot)
EXTRA=()
[[ $(sed -n 's/^release: //p' "$OUT/rustc.txt") != *nightly* ]] || EXTRA+=(--cfg abi_nightly)
# Matching rustc-dev must already be installed; never modify global defaults/config.
ABI_FACTS_RUSTC="$(cat "$OUT/rustc.txt")" RUSTC_BOOTSTRAP=1 \
  python3 "$ROOT/scripts/run-guarded.py" nice -n 19 rustc "$QUALIFICATION/helper.rs" "${EXTRA[@]}" \
  --edition 2024 -C debuginfo=0 -C opt-level=0 -L "$SYSROOT/lib" \
  -C "link-arg=-Wl,-rpath,$SYSROOT/lib" -o "$OUT/abi-facts-helper"
# Copy source only. No experiment file/mtime/target is changed.
python3 - "$QUALIFICATION/fixtures" "$OUT" <<'PY'
import shutil, sys
from pathlib import Path
for name in ('web', 'statearena', 'statearena-derive'):
    shutil.copytree(Path(sys.argv[1]) / name, Path(sys.argv[2]) / name, dirs_exist_ok=True,
                    ignore=shutil.ignore_patterns('target', '*-target', '.git'))
(Path(sys.argv[2]) / 'web/crates/abi-probe/src/lib.rs').touch()
PY
rm -f "$OUT/facts.jsonl"
(
  cd "$OUT/web"
  REFRESH_FACTS_OUT=$OUT REFRESH_FACTS_HELPER=$OUT/abi-facts-helper \
    RUSTC_WRAPPER=$ROOT/scripts/qualification/facts-wrapper.py \
    CARGO_BUILD_TARGET=x86_64-unknown-linux-gnu \
    run_lane build python3 "$ROOT/scripts/run-guarded.py" nice -n 19 \
      cargo build --locked -p abi-probe -j 4
)
REFERENCE=$QUALIFICATION/reference/$COMMIT.json
COMPARE=()
[[ ! -f $REFERENCE ]] || COMPARE=(--reference "$REFERENCE" --reference-facts "$QUALIFICATION/reference/facts-web-$TC.jsonl")
python3 "$ROOT/scripts/qualification/collect.py" "$TC" "$OUT" "${COMPARE[@]}"
# Match runtime stock TypeIds with compiler facts in the same crate identity, at both profiles.
# Full rustd/captured-object TypeId matrix remains an explicit manual release gate.
for OPT in 0 3; do
  nice -n 19 rustc "$ROOT/scripts/qualification/typeids.rs" --edition 2024 --crate-name abi_typeids \
    -A non_camel_case_types -C opt-level=$OPT -o "$OUT/typeids-$OPT"
  "$OUT/typeids-$OPT" > "$OUT/typeids-$OPT.txt"
  mkdir -p "$OUT/typeids-facts-$OPT"
  "$OUT/abi-facts-helper" "$ROOT/scripts/qualification/typeids.rs" --edition 2024 --crate-name abi_typeids \
    -A non_camel_case_types -C opt-level=$OPT --emit=metadata --out-dir "$OUT/typeids-facts-$OPT" \
    > "$OUT/typeids-facts-$OPT.jsonl"
  python3 "$ROOT/scripts/qualification/check-typeids.py" "$OUT/typeids-$OPT.txt" "$OUT/typeids-facts-$OPT.jsonl"

done
cmp "$OUT/typeids-0.txt" "$OUT/typeids-3.txt"
python3 - "$OUT" <<'PY_CHECK_PERSONALITY'
import json, subprocess, sys
from pathlib import Path
out = Path(sys.argv[1])
personality = json.loads((out / 'abi-facts.bundle').read_text())['personality']
symbols = subprocess.check_output(['nm', '-a', str(out / 'typeids-0')], text=True)
if personality not in {line.split()[-1] for line in symbols.splitlines() if line.split()}:
    raise SystemExit('EH personality not found in actual matching-stock ELF: ' + personality)
print('PASS: actual matching-stock ELF contains EH personality ' + personality)
PY_CHECK_PERSONALITY

echo 'PASS: stock TypeId matrix stable across debug/optimized builds (primitive, external/own/private ADT, str/String, function pointer/item)'
cat > "$OUT/intrinsic-audit.md" <<'AUDIT'
# Intrinsic and adapter audit — MANUAL, NOT PASSED
- Review newly unsupported intrinsics and async fallbacks against candidate rustc.
- Exercise HTTP body polling and Bevy registry access, actual live changes, layout restart.
- Check abort_immediate and EH personality, default CPU/features, packaging flags,
  nightly embed-metadata=no, and rejection of unrelated unstable/target/CPU flags.
- Verify private Any::type_id debug and optimized leaf opcode/relocation refusal,
  crate disambiguation, duplicate/conflicting facts, generic function-item spelling.
- Run rustd-vs-matching-capture TypeId matrix (stock-only probe is preliminary).
AUDIT
if (( DRY )); then
  echo 'DRY RUN: helper/extraction/SIMD/personality/reference/stock TypeId checks ran; scripts/test.sh deliberately skipped.'
else
  python3 "$ROOT/scripts/run-guarded.py" "$ROOT/scripts/test.sh"
fi
cat <<'MANUAL'
Manual steps left (required before publishing support):
[ ] Review intrinsic-audit.md; test real packaging flags and fail-closed controls.
[ ] Compare rustd/capture TypeIds: primitives, external/own/private plugin ADTs,
    &str/String, function pointers/items; both private leaf shapes and rejection tests.
[ ] Run matching-stock web/game cells with S1/S2/S3/S8 and layout restore;
    verify PID/ticks/sockets, native delivery, checker and fallback accounting.
[ ] Run generated-program differentials and real-app semantics against matching stock.
[ ] Run scripts/test.sh if dry-run; qualify oldest distro/glibc in clean environment.
[ ] Review licenses, record approval and publish manually. No support was enabled here.
MANUAL
