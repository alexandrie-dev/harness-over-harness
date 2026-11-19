#!/usr/bin/env bash
# clippy for the desktop workspace; the build dir is build-*, not target-* (tmpfs daemon empties idle target-*).
set -uo pipefail
cd "$(dirname "$0")/.."
export CARGO_HOME="${CARGO_HOME:-/mnt/toshiba/alexandrie/hoh/cargo-home-ui2}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/mnt/toshiba/alexandrie/hoh/build-sg6/desktop}"
export CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2
timeout 90m cargo clippy --all-targets --locked --offline --no-deps -- -D warnings
echo "exit $?"
