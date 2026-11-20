#!/bin/bash
# Run the V1-SCENARIOS.tsv selection through the built hoh. Output: $OUT/summary.tsv, diffs/, runs/.
# usage: scripts/conformance-v1.sh [group: in|extra|all] [extra hoh-conformance flags]
set -u
cd "$(dirname "$0")/.."
GROUP=${1:-all}; shift || true
# CEO 20.11 19:47: полный прогон — только интеграция (scripts/conformance-main.sh), когда нет сборок приёмки.
# Писатели гоняют свои сценарии точечно: --scenario suite/name ...
if [[ " $* " != *" --scenario "* && ${HOH_FULL_RUN:-} != 1 ]]; then
  echo "полный прогон делает ведущий (scripts/conformance-main.sh); для своих сценариев добавь --scenario suite/name" >&2; exit 3
fi
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
  --v1 conformance/V1-SCENARIOS.tsv --group "$GROUP" --out "$OUT" --jobs "${HOH_CONFORMANCE_JOBS:-2}" "$@"   # CEO 20.11 19:3x: не упираться в нагрузку
