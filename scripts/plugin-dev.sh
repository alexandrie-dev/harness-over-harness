#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ $# != 1 ]]; then
  echo 'usage: scripts/plugin-dev.sh <plugin-id>' >&2
  exit 2
fi
: "${CARGO_HOME:?Set CARGO_HOME to the offline HoH cargo home}"
: "${CARGO_TARGET_DIR:?Set CARGO_TARGET_DIR to the line target}"
python3 scripts/generate-plugin-index.py --candidate "$1"
cargo build --offline -p hoh
printf '\nEngine built. Start a separately built HoH window with:\n  HOH_ENGINE_PATH=%q <path-to-hoh-desktop>\n' "$CARGO_TARGET_DIR/debug/hoh"
