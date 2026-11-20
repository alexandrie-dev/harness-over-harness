#!/usr/bin/env bash
# Guard for the critical-path build directory in tmpfs (CEO 19.11 08:45): /dev/shm/hoh-crit holds one acceptance lane
# (accept/) and the lead's loader run (l1/), at most 10 GB in all. Every 15 s: if MemAvailable < 8 GB or the directory
# exceeds 10 GB, stop the builds that use it, empty it and leave /dev/shm/hoh-crit/disabled (accept.sh then skips the
# tmpfs lane). Run detached: setsid nohup scripts/shm-guard.sh &
set -uo pipefail
dir=/dev/shm/hoh-crit
log=/mnt/toshiba/alexandrie/hoh/logs/shm-guard.log
mkdir -p "$dir"
while :; do
  avail=$(awk '/^MemAvailable/{print int($2/1024)}' /proc/meminfo)
  used=$(du -sm "$dir" 2>/dev/null | cut -f1)
  if [[ ! -e $dir/disabled ]] && (( avail < 8000 || used > 10240 )); then
    for p in $(pgrep -x cargo); do
      tr '\0' '\n' <"/proc/$p/environ" 2>/dev/null | grep -q "^CARGO_TARGET_DIR=$dir" && kill -TERM -- "-$(ps -o pgid= -p "$p" | tr -d ' ')" 2>/dev/null
    done
    sleep 5
    find "$dir" -mindepth 1 -delete 2>/dev/null
    touch "$dir/disabled"
    echo "$(date +%H:%M:%S) tmpfs off: MemAvailable=${avail}M used=${used}M — builds stopped, directory emptied" >>"$log"
  fi
  sleep 15
done
