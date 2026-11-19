#!/usr/bin/env bash
# Builds the real engine, the scenario model mock and the replay stub for the desktop tests.
# The build dirs are named build-*, not target-*: scripts/tmpfs-builds.sh and clean-old-targets.sh
# empty idle target-* dirs, which would delete the binaries between build and test.
set -uo pipefail
root=$(cd "$(dirname "$0")/../../.." && pwd)
export CARGO_TARGET_DIR="${HOH_ENGINE_TARGET:-/mnt/toshiba/alexandrie/hoh/build-sg6/engine}"
export CARGO_HOME="${HOH_ENGINE_CARGO_HOME:-$HOME/.cargo}" CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2
cd "$root"
timeout 120m cargo build -p hoh -p hoh-llm-openai-mock -p hoh-window-stub --offline
echo "exit $?"
