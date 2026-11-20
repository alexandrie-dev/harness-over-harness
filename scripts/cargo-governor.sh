#!/usr/bin/env bash
# scripts/cargo-governor.sh [MAX] — global limit on concurrent cargo runs of THIS project (default 5).
# Every 15 s: finds top-level cargo processes whose working directory is inside this repository, keeps the
# oldest MAX running and pauses the others with SIGSTOP on their process group; resumes the oldest paused one when
# a slot frees. Only the cargo process and its descendants are paused (never the writer that started it, whose
# network session must stay alive). Nothing is killed: a paused build continues where it stopped. A group that is already running test
# binaries is never paused (timing-sensitive tests). Processes of other projects are never touched.
# Stop with SIGTERM: it resumes everything it paused before exiting.
set -uo pipefail
cd "$(dirname "$0")/.."
repo=$(pwd -P)
max=${1:-5}
state=/mnt/toshiba/alexandrie/hoh/cargo-governor.paused
touch "$state"
prio=/mnt/toshiba/alexandrie/hoh/governor-priority.txt hold=/mnt/toshiba/alexandrie/hoh/governor-hold.txt
touch "$prio" "$hold"
declare -A prev=() ; finished=0

tree() { local p=$1 c; echo "$p"; for c in $(pgrep -P "$p"); do tree "$c"; done; }
signal_tree() { local sig=$1 p; for p in $(tree "$2"); do kill "-$sig" "$p" 2>/dev/null; done; }
resume_all() { cat "$state" "$state.new" 2>/dev/null | while read -r p; do [[ -n $p ]] && signal_tree CONT "$p"; done; : >"$state"; rm -f "$state.new"; exit 0; }  # state.new: paused in the interrupted pass
trap resume_all TERM INT

running_tests() {  # true if a descendant of pid runs a binary from a target/*/deps directory
  local pid=$1 c exe
  for c in $(pgrep -P "$pid"); do
    exe=$(readlink "/proc/$c/exe" 2>/dev/null || true)
    [[ $exe == */deps/* ]] && return 0
    running_tests "$c" && return 0
  done
  return 1
}

kill_hung_tests() {  # 19.11 09:40: a test binary alive > 20 min at < 1 % average CPU is deadlocked and holds a slot forever
  local pid et cpu exe
  while read -r pid et cpu; do
    exe=$(readlink "/proc/$pid/exe" 2>/dev/null) || continue
    [[ $exe == */deps/* && $(readlink "/proc/$pid/cwd" 2>/dev/null) == "$repo"* ]] || continue
    (( et > 1200 )) && awk -v c="$cpu" 'BEGIN{exit !(c < 1.0)}' || continue
    kill -TERM "$pid" 2>/dev/null && echo "$(date +%H:%M:%S) HUNG TEST killed: pid $pid $(basename "$exe") ${et}s cpu ${cpu}%"
  done < <(ps -eo pid=,etimes=,pcpu=)
}

while :; do
  kill_hung_tests
  mapfile -t rows < <(ps -eo pid=,pgid=,etimes=,comm= | awk '$4=="cargo"{print $1, $2, $3}')
  groups=() ; held=() ; unset seen; declare -A seen=()
  for r in "${rows[@]}"; do
    read -r pid pgid et <<<"$r"
    ppid_comm=$(ps -o comm= -p "$(ps -o ppid= -p "$pid" | tr -d ' ')" 2>/dev/null)
    [[ $ppid_comm == cargo ]] && continue                      # nested cargo: counted with its parent
    cwd=$(readlink "/proc/$pid/cwd" 2>/dev/null) || continue
    [[ $cwd == "$repo"* ]] || continue                         # not this project
    [[ -n ${seen[$pgid]:-} ]] && continue; seen[$pgid]=1
    # CEO 19.11 03:05: acceptance builds (CARGO_TARGET_DIR=…/target-accept-*) go first, ahead of writers
    # also the lead's verification runs of a writer's work (HOH_PRIORITY=1): they gate acceptance the same way
    env=$(tr '\0' '\n' <"/proc/$pid/environ" 2>/dev/null)
    grep -qE '^(CARGO_TARGET_DIR=(.*/target-accept-|/dev/shm/hoh/accept)|HOH_PRIORITY=1$)' <<<"$env" && et=$((et + 1000000000))
    # CEO 19.11 08:45: order of the critical path (governor-priority.txt, one target-dir pattern per line, first = highest);
    # targets in governor-hold.txt do not build at all while the disk is overloaded
    tdir=$(sed -n 's/^CARGO_TARGET_DIR=//p' <<<"$env")
    if [[ -n $tdir ]] && grep -qxFf "$hold" <<<"$(basename "$tdir")" 2>/dev/null; then held+=("$pid"); continue; fi
    rank=0; while read -r pat; do [[ -z $pat ]] && continue; rank=$((rank + 1)); [[ $(basename "$tdir") == $pat ]] && { et=$((et + (100 - rank) * 10000000)); break; }; done <"$prio"
    groups+=("$et $pgid $pid")
  done
  mapfile -t sorted < <(printf '%s\n' "${groups[@]}" | sort -rn)   # oldest first
  disk_free=$(df -BG --output=avail /mnt/toshiba | tail -1 | tr -dc 0-9)   # CEO 18.11: < 30 GB — no build runs; the lead removes the last started writer
  limit=$max; (( disk_free < 30 )) && { limit=0; echo "$(date +%H:%M:%S) DISK<30G: all builds paused, remove the last started writer"; }
  # 19.11 10:20: builds and their tmpfs directories share RAM; 6 compiling builds tripped the 8 GB tmpfs guard at 09:43.
  # Admit fewer builds as memory shrinks, long before the guard has to switch tmpfs off.
  avail=$(awk '/^MemAvailable/{print int($2/1024)}' /proc/meminfo)
  # CEO 20.11 17:5x: сборки на NVMe, память освобождена от tmpfs — предел по доступной памяти «не меньше 8 ГБ».
  if (( avail < 8000 )); then limit=$(( limit < 2 ? limit : 2 )); elif (( avail < 9500 )); then limit=$(( limit < 3 ? limit : 3 )); fi
  root_free=$(df -BG --output=avail / | tail -1 | tr -dc 0-9)   # CEO 20.11: на NVMe (/) свободно < 30 ГБ — сборки стоп, сообщить
  if (( root_free < 30 )); then limit=0; echo "$(date +%H:%M:%S) ROOT<30G: builds paused"
    [[ -e /mnt/toshiba/alexandrie/hoh/pipeline/root-low.flag ]] || { touch /mnt/toshiba/alexandrie/hoh/pipeline/root-low.flag
      echo "$(date +%H:%M) NEEDS-LEAD / свободно ${root_free} ГБ < 30: сборки на NVMe остановлены (CEO 20.11)" >>/mnt/toshiba/alexandrie/hoh/pipeline/attention.log; }
  fi
  active=0; : >"$state.new"
  for p in "${held[@]}"; do running_tests "$p" && continue; [[ $(ps -o stat= -p "$p" | cut -c1) != T ]] && signal_tree STOP "$p"; echo "$p" >>"$state.new"; done
  for k in "${!prev[@]}"; do [[ -z ${seen[$k]:-} ]] && finished=$((finished + 1)); done   # builds that ended since the last pass
  unset prev; declare -A prev=(); for k in "${!seen[@]}"; do prev[$k]=1; done
  for g in "${sorted[@]}"; do
    [[ -z $g ]] && continue
    read -r et pgid pid <<<"$g"
    st=$(ps -o stat= -p "$pid" | cut -c1)
    if (( active < limit )) || running_tests "$pid"; then
      signal_tree CONT "$pid"   # whole tree: a running cargo may still have a stopped rustc child (19.11: rustc held Tl 1 h 46 m)
      active=$((active + 1))
    else
      [[ $st != T ]] && signal_tree STOP "$pid"
      echo "$pid" >>"$state.new"
    fi
  done
  mv "$state.new" "$state"
  echo "$(date +%H:%M:%S) running=$active paused=$(wc -l <"$state") $(awk '/some/{print $3}' /proc/pressure/memory) swapfree=$(awk '/SwapFree/{print int($2/1024)}' /proc/meminfo)M disk=${disk_free}G mem=${avail}M limit=${limit} held=${#held[@]} finished_total=$finished"
  sleep 5   # CEO 19.11 09:10: tmpfs slots — admit quickly, a build should not run unadmitted long
done
