#!/usr/bin/env bash
# The lead's acceptance run: scripts/accept.sh NAME CRATE...
# Runs scripts/verify.sh for the crates in the first free of three build lanes (separate CARGO_TARGET_DIR each),
# so acceptance runs go in parallel with each other and do not wait on writers' build directories.
# Report: /mnt/toshiba/alexandrie/hoh/reports/NAME/summary.txt. Exit code is verify.sh's.
set -uo pipefail
cd "$(dirname "$0")/.."
name=$1; shift
H=/mnt/toshiba/alexandrie/hoh
memory_ok() {  # do not start a lane when available memory or free swap is low (the machine has killed processes before)
  read -r avail swapfree < <(awk '/^MemAvailable/{a=$2} /^SwapFree/{s=$2} END{print int(a/1024), int(s/1024)}' /proc/meminfo)
  disk=$(df -BG --output=avail /mnt/toshiba | tail -1 | tr -dc 0-9)   # CEO 18.11: no new lane below 60 GB free
  (( avail >= 8000 && disk >= 60 ))  # CEO 19.11 02:05: MemAvailable > 8 ГБ (условие по swap снято), диск ≥ 60 ГБ
}
while :; do
  memory_ok || { sleep 60; continue; }
  for lane in a b; do  # CEO 20.11 17:2x: приёмка только на USB (target-accept-a/b), tmpfs отдан писателям
    exec {fd}>"$H/accept-lane-$lane.lock"
    if flock -n "$fd"; then
      echo "lane $lane: $*"
      dir=$H/target-accept-$lane   # CEO 20.11 19:17: NVMe ≤25 ГБ — приёмка на toshiba
      tree=   # CEO 20.11 19:3x: приёмка в выгрузке коммита пакета HOH_PKG (main + пути пакета)
      if [[ -n ${HOH_PKG:-} ]]; then tree=$H/it/acc-$lane; bash scripts/pkg.sh checkout "$tree" "$HOH_PKG" || exit 1; fi
      HOH_TREE=$tree CARGO_TARGET_DIR="$dir" CARGO_BUILD_JOBS=4 HOH_REPORT_DIR="$H/reports/$name" \
        bash scripts/verify.sh "$@" >/dev/null 2>&1
      rc=$?
      cat "$H/reports/$name/summary.txt" 2>/dev/null || echo "(no summary)"
      echo "verify exit=$rc"
      exit $rc
    fi
    exec {fd}>&-
  done
  sleep 20
done
