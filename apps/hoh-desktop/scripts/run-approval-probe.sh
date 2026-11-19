#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/../../.." && pwd)
desktop="$root/apps/hoh-desktop"

export CARGO_HOME="${CARGO_HOME:-/mnt/toshiba/alexandrie/hoh/cargo-home-ui2}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/mnt/toshiba/alexandrie/hoh/target-cf1/desktop}"
export CARGO_NET_OFFLINE=true
export CARGO_BUILD_JOBS=2

fail() {
  printf 'HOH_APPROVAL_PROBE_RESULT:{"pass":false,"detail":"%s"}\n' "$1"
  exit 1
}

python3 "$desktop/scripts/stage-approval-probe.py" \
  || fail "could not stage probe frontend"

(
  cd "$root"
  timeout 10m cargo build -p hoh-window-stub --locked --offline
) || fail "could not build hoh-window-stub"

(
  cd "$desktop"
  TAURI_CONFIG='{"build":{"frontendDist":"probe-dist"}}' \
    timeout 10m cargo build --features probe --locked --offline
) || fail "could not build probe desktop"

if [[ "${1:-}" == "--build-only" ]]; then
  printf 'HOH_APPROVAL_PROBE_BUILD_ONLY:ready\n'
  exit 0
fi

set +e
python3 "$root/ui/scripts/run-isolated-window.py" \
  "$CARGO_TARGET_DIR/debug/hoh-desktop" --approval-probe
probe_status=$?
set -e

if (( probe_status != 0 )); then
  fail "probe process exited with status $probe_status"
fi
