#!/usr/bin/env bash
# All project builds in tmpfs (CEO 19.11 09:10). Run detached; idempotent, also the recovery after a reboot:
#   setsid nohup scripts/tmpfs-builds.sh >/dev/null 2>&1 </dev/null &
# Layout: every build directory name stays at /mnt/toshiba/alexandrie/hoh/target-NAME, but is a symlink to
# /dev/shm/hoh/target-NAME, so writers, the lead and acceptance (accept.sh lanes shm/shm2 use /dev/shm/hoh/accept*)
# need no new paths. "Slots": the governor runs at most 4 builds at once (scripts/cargo-governor.sh 4); this daemon keeps
# room for them. Every 5 s:
#   - guard: MemAvailable < 8 GB → stop all builds in tmpfs, empty it, put real directories back on the USB disk
#     and leave /dev/shm/hoh.disabled (re-enable: rm /dev/shm/hoh.disabled and restart this script);
#   - room: when /dev/shm/hoh uses more than 8 GB (memory headroom for 4 compiling builds), empty the least recently used
#     directories no process is building in (a full rebuild costs ~2 min);
#   - conversion: a real target-* directory on the USB disk no process uses → its old content is deleted, it becomes
#     the symlink (target-accept-a/b stay real: fallback lanes when tmpfs is off);
#   - after a reboot: recreates /dev/shm/hoh/target-NAME for every symlink, so no link dangles.
set -uo pipefail
H=/mnt/toshiba/alexandrie/hoh
T=/dev/shm/hoh
log=$H/logs/tmpfs-builds.log
cap_mb=$((4 * 1024))   # 19.11 16:30: 6 ГБ + линковка тестов (rust-lld по 1,4 ГБ) + Firefox 3 ГБ → MemAvailable 7,99 ГБ в 16:28; 4 ГБ
mkdir -p "$T"
in_use() { grep -qlszE "^CARGO_TARGET_DIR=($1|$2)(/.*)?$" /proc/[0-9]*/environ 2>/dev/null; }   # USB path or tmpfs path
say() { echo "$(date +%H:%M:%S) $*" >>"$log"; }
say "start (pid $$)"
peak_max=0 avail_min=999999
while :; do
  if [[ -e $T.disabled ]]; then   # 19.11 18:47: чужие пики (TTS, qemu) кратковременны — включаемся сами, когда снова >= 12 ГБ
    a=$(awk '/^MemAvailable/{print int($2/1024)}' /proc/meminfo)
    (( a >= 12000 )) && rm -f "$T.disabled" && say "tmpfs ON again: MemAvailable=${a}M"
    sleep 30; continue
  fi
  avail=$(awk '/^MemAvailable/{print int($2/1024)}' /proc/meminfo)
  # 19.11 17:00 (CEO): сначала освобождаем только простаивающие папки (не в cargo, не тронутые 10 мин) — живые
  # сборки писателей не трогаем. Полное выключение с остановкой сборок — лишь если и после этого < 6 ГБ.
  if (( avail < 8000 )); then
    while read -r _ d; do
      n=$(basename "$d"); in_use "$H/$n" "$d" && continue
      [[ -n $(find "$d" -maxdepth 3 -newermt "-10 minutes" -print -quit 2>/dev/null) ]] && continue
      sz=$(du -sm "$d" | cut -f1); find "$d" -mindepth 1 -delete 2>/dev/null
      say "low memory (${avail}M): evicted idle $n (${sz}M)"
    done < <(find "$T" -mindepth 1 -maxdepth 1 -type d -printf '%T@ %p\n' | sort -n)
    sleep 3; avail=$(awk '/^MemAvailable/{print int($2/1024)}' /proc/meminfo)
  fi
  if (( avail < 6000 )); then
    stopped=""
    for p in $(pgrep -x cargo); do
      d=$(tr '\0' '\n' <"/proc/$p/environ" 2>/dev/null | sed -n 's/^CARGO_TARGET_DIR=//p')
      [[ -n $d && ( $d == "$T"* || ( -L ${d%%/desktop} ) ) ]] && stopped+="$(basename "${d%%/desktop}") "
    done
    echo "$(date +%H:%M) NEEDS-LEAD tmpfs OFF: MemAvailable=${avail}M, остановлены сборки: ${stopped:-нет} — сообщить писателям" >>"$H/pipeline/attention.log"
    for p in $(pgrep -x cargo); do
      d=$(tr '\0' '\n' <"/proc/$p/environ" 2>/dev/null | sed -n 's/^CARGO_TARGET_DIR=//p')
      [[ -n $d && ( $d == "$T"* || ( -L ${d%%/desktop} ) ) ]] && kill -TERM -- "-$(ps -o pgid= -p "$p" | tr -d ' ')" 2>/dev/null
    done
    sleep 5
    for l in "$H"/target-*; do [[ -L $l && $(readlink "$l") == "$T"/* ]] && rm -f "$l" && mkdir -p "$l"; done   # 20.11: ссылки на NVMe не трогать
    find "$T" -mindepth 1 -delete 2>/dev/null
    touch "$T.disabled"
    say "tmpfs OFF: MemAvailable=${avail}M — builds stopped, tmpfs emptied, real directories back on the USB disk"
    continue
  fi
  # after a reboot (or eviction of a whole directory): every link must point at an existing directory
  for l in "$H"/target-*; do [[ -L $l && $(readlink "$l") == "$T"/* ]] && mkdir -p "$(readlink "$l")"; done
  # room for the next build
  used=$(du -sm "$T" 2>/dev/null | cut -f1)
  if (( used > cap_mb )); then
    while read -r _ d; do
      (( used <= cap_mb )) && break
      n=$(basename "$d"); in_use "$H/$n" "$d" && continue
      [[ -n $(find "$d" -maxdepth 3 -newermt "-10 minutes" -print -quit 2>/dev/null) ]] && continue   # 19.11: не удалять папку, тронутую за 10 мин (писатель между командами cargo, выгрузки commit-check, конвейер, приёмка)
      sz=$(du -sm "$d" | cut -f1); find "$d" -mindepth 1 -delete 2>/dev/null; used=$((used - sz))
      say "evicted $n (${sz}M), tmpfs now ~${used}M"
    done < <(find "$T" -mindepth 1 -maxdepth 1 -type d -printf '%T@ %p\n' | sort -n)
  fi
  # USB directories → links, as soon as nothing builds in them
  for d in "$H"/target-*; do
    n=$(basename "$d")
    [[ -L $d || $n == target-accept-a || $n == target-accept-b ]] && continue
    in_use "$d" "$T/$n" && continue
    t=$H/trash-$n-$(date +%H%M%S)
    mv "$d" "$t" && mkdir -p "$T/$n" && ln -s "$T/$n" "$d" && say "$n → tmpfs (old content deleted)"
    ionice -c3 rm -rf "$t" &
  done
  now=$(du -sm "$T" 2>/dev/null | cut -f1); (( now > peak_max )) && peak_max=$now; (( avail < avail_min )) && avail_min=$avail
  echo "$(date +%H:%M:%S) used=${now}M avail=${avail}M since_start: peak_used=${peak_max}M min_avail=${avail_min}M" >"$H/logs/tmpfs-builds.now"
  sleep 5
done
