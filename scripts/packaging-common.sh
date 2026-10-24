#!/usr/bin/env bash
# Shared-machine lanes are optional for users; release builds remain bounded.
run_lane() {
  local lane=$1; shift
  if [[ -n ${REFRESH_LANE:-} ]]; then
    "$REFRESH_LANE" "$lane" -- "$@"
  else
    "$@"
  fi
}

require_rustc_dev() {
  if ! compgen -G "$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/lib/librustc_driver*" >/dev/null; then
    echo "Missing matching rustc-dev component. Run: rustup component add rustc-dev --toolchain $RUSTUP_TOOLCHAIN" >&2
    exit 1
  fi
}
