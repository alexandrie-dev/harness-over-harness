#!/bin/sh
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
: "${CARGO_TARGET_DIR:=/mnt/toshiba/alexandrie/hoh/target-cx}"
export CARGO_TARGET_DIR HOH_CODEX_LIVE=1

cd "$repo"
codex --version
cargo test -p hoh-harness-codex --test live real_codex_streams_approves_resumes_and_stops -- --ignored --nocapture --exact
