#!/usr/bin/env bash
# The lead's own verification run, detached so it survives a restart of the lead's session:
#   setsid nohup scripts/lead-run.sh NAME TARGET_DIR TIMEOUT_S CRATE... &
# Runs cargo test (--no-fail-fast), clippy -D warnings, fmt --check with HOH_PRIORITY=1 (governor priority).
# Logs: logs/NAME.lead-{test,clippy}.txt; result line appended to logs/lead-runs.done.
set -uo pipefail
cd "${HOH_TREE:-$(dirname "$0")/..}"   # CEO 20.11 19:3x: прогон в выгрузке коммита пакета
name=$1 target=$2 limit=$3; shift 3
L=/mnt/toshiba/alexandrie/hoh/logs
sel=(); for c in "$@"; do sel+=(-p "$c"); done
export HOH_PRIORITY=1 CARGO_TARGET_DIR=$target CARGO_BUILD_JOBS=2
export CARGO_HOME=/mnt/toshiba/alexandrie/hoh/cargo-home   # CEO 20.11: кэш crates не на корневом диске
timeout "$limit" cargo test --offline --no-fail-fast "${sel[@]}" >"$L/$name.lead-test.txt" 2>&1; t=$?
timeout "$limit" cargo clippy --offline "${sel[@]}" --all-targets --no-deps -- -D warnings >"$L/$name.lead-clippy.txt" 2>&1; c=$?
cargo fmt "${sel[@]}" -- --check >/dev/null 2>&1; f=$?
pass=$(grep -oE '^test result: [a-zA-Z]+\. [0-9]+ passed; [0-9]+ failed' "$L/$name.lead-test.txt" | awk '{p+=$4; f+=$6} END{print p"/"f}')
echo "$(date +%H:%M) $name test=$t ($pass) clippy=$c fmt=$f" >> "$L/lead-runs.done"
[[ $target == /dev/shm/* ]] && rm -rf "$target"   # tmpfs is RAM: free it as soon as the run is over
