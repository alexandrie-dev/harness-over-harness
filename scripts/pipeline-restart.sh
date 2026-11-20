#!/usr/bin/env bash
# Перезапуск службы конвейера без потери идущих ревью и прогонов (CEO 20.11 19:0x: в 18:07 перезапуск оборвал пять ревью).
#   bash scripts/pipeline-restart.sh [MAX_WAIT_SECONDS=5400]
# Ждёт, пока в cgroup службы hoh-pipeline не останется ничего, кроме оболочек и sleep (то есть нет claude/kimi/cargo/
# lead-run/accept), затем перезапускает. Не дождался — не перезапускает и пишет NEEDS-LEAD.
set -u
cg=/sys/fs/cgroup$(systemctl --user show hoh-pipeline -p ControlGroup --value)
busy() {
  local p c
  for p in $(cat "$cg"/cgroup.procs 2>/dev/null); do
    c=$(cat /proc/"$p"/comm 2>/dev/null) || continue
    [[ $c =~ ^(bash|sleep|timeout|flock|cat|tail|awk|sed|grep|date)$ ]] || { echo "$p $c"; return 0; }
  done
  return 1
}
deadline=$((SECONDS + ${1:-5400}))
while b=$(busy); do
  (( SECONDS >= deadline )) && { echo "$(date +%H:%M) NEEDS-LEAD pipeline-restart: не дождался, занято: $b" >>/mnt/toshiba/alexandrie/hoh/pipeline/attention.log; exit 1; }
  sleep 20
done
systemctl --user restart hoh-pipeline && echo "$(date +%H:%M) pipeline перезапущен без идущих ревью"
