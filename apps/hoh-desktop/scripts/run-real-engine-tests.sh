#!/usr/bin/env bash
# Desktop tests against the stub and the real engine. Build the engine first (build-real-engine.sh).
set -uo pipefail
root=$(cd "$(dirname "$0")/../../.." && pwd)
engine="${HOH_ENGINE_TARGET:-/mnt/toshiba/alexandrie/hoh/build-sg6/engine}/debug"
export CARGO_HOME="${CARGO_HOME:-/mnt/toshiba/alexandrie/hoh/cargo-home-ui2}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/mnt/toshiba/alexandrie/hoh/build-sg6/desktop}"
export CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2
export HOH_TEST_ENGINE="$engine/hoh-window-stub" HOH_REAL_ENGINE="$engine/hoh" HOH_MOCK_BIN="$engine/hoh-llm-openai-mock"
cd "$root/apps/hoh-desktop"
timeout 90m cargo test --locked --offline "$@"
echo "exit $?"
