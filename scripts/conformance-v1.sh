#!/bin/bash
# Run the V1-SCENARIOS.tsv selection through the built hoh. Output: $OUT/summary.tsv, diffs/, runs/.
# usage: scripts/conformance-v1.sh [group: in|extra|all] [extra hoh-conformance flags]
set -u
cd "$(dirname "$0")/.."
GROUP=${1:-all}; shift || true
TARGET=${CARGO_TARGET_DIR:-target}/debug
# hoh carries the Rust counterparts of dsh's local: test plugins only when built for this runner.
timeout 1800 cargo build --offline -p hoh -p hoh-conformance --features hoh/conformance-fixtures || exit 2
STAMP=$(date +%Y%m%d-%H%M%S)
OUT=/mnt/toshiba/alexandrie/hoh/conformance-runs/$STAMP
HOME_DIR=/dev/shm/hoh/conformance-home-$STAMP
mkdir -p "$OUT" "$HOME_DIR"
echo "$OUT"
HOME=$HOME_DIR TMPDIR=/dev/shm/hoh exec "$TARGET/hoh-conformance" \
  --hoh "$TARGET/hoh" --refs /mnt/toshiba/alexandrie/hoh/refs/deepseek-harness \
  --v1 conformance/V1-SCENARIOS.tsv --group "$GROUP" --out "$OUT" --jobs 4 "$@"
