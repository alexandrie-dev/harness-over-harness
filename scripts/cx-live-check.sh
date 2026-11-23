#!/bin/sh
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
CARGO_HOME=/mnt/toshiba/alexandrie/hoh/cargo-home
CARGO_TARGET_DIR=/home/save/.cache/hoh-build/line-codexw
CARGO_BUILD_JOBS=4
export CARGO_HOME CARGO_TARGET_DIR CARGO_BUILD_JOBS HOH_CODEX_LIVE=1

cd "$repo"
codex --version
cargo test --offline -p hoh-harness-codex --test live real_codex_streams_approves_resumes_and_stops -- --ignored --nocapture --exact
