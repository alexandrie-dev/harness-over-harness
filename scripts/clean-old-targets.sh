#!/usr/bin/env bash
# CEO 19.11 08:45: after the profile change every old build directory is useless. For 3 h, every 2 min: each
# /mnt/toshiba/alexandrie/hoh/target-* not used by any process (no live process has CARGO_TARGET_DIR pointing into it)
# and not yet cleaned is renamed to trash-*, recreated empty at once (a writer's sandbox needs the path to exist), and
# the old content is deleted at idle I/O priority. Run detached: setsid nohup scripts/clean-old-targets.sh &
set -uo pipefail
H=/mnt/toshiba/alexandrie/hoh
log=$H/logs/clean-old-targets.log
declare -A done_=()
in_use() { grep -qlszE "^CARGO_TARGET_DIR=$1(/.*)?$" /proc/[0-9]*/environ 2>/dev/null; }
end=$((SECONDS + 3 * 3600))
while (( SECONDS < end )); do
  for d in "$H"/target-*; do
    n=$(basename "$d")
    [[ -n ${done_[$n]:-} || $n == target-accept-a || $n == target-accept-b ]] && continue
    in_use "$d" && continue
    t=$H/trash-$n-$(date +%H%M%S)
    mv "$d" "$t" && mkdir -p "$d" && done_[$n]=1 && echo "$(date +%H:%M) $n cleared (deleting $t)" >>"$log"
    ionice -c3 rm -rf "$t"
  done
  sleep 120
done
echo "$(date +%H:%M) finished; not cleaned (in use for 3 h): $(for d in "$H"/target-*; do n=$(basename "$d"); [[ -z ${done_[$n]:-} ]] && printf '%s ' "$n"; done)" >>"$log"
