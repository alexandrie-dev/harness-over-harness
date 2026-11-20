#!/usr/bin/env bash
# Будильник ведущего (CEO 19.11 11:19): запускать НЕ отделённым фоновым заданием харнесса, после каждой обработки снова.
#   scripts/lead-alarm.sh [SECONDS]   (по умолчанию 600 — запасной таймер)
# Выходит, как только есть НЕПРОСМОТРЕННАЯ строка в pipeline/attention.log или новое «exit=» писателя
# в logs/resume-writers.done (курсор хранится в pipeline/alarm.cursor, поэтому строки, пришедшие между
# пробуждением и перевзводом, не теряются), выключился tmpfs, или истёк срок. Печатает всё непросмотренное.
set -uo pipefail
H=/mnt/toshiba/alexandrie/hoh; A=$H/pipeline/attention.log; W=$H/logs/resume-writers.done; C=$H/pipeline/alarm.cursor
limit=${1:-600}
if [[ -s $C ]]; then read -r a0 w0 <"$C"; else a0=$(wc -l <"$A"); w0=$(grep -c 'exit=' "$W"); fi
end=$((SECONDS + limit))
while (( SECONDS < end )); do
  # 14:22: READY-COMMIT, ГОТОВ/ОСТАТОК и выходы писателей конвейер обрабатывает сам — будит только решение ведущего
  sed -n "$((a0 + 1)),\$p" "$A" | grep -q 'NEEDS-LEAD' && break
  [[ -e /dev/shm/hoh.disabled ]] && break
  sleep 10
done
a1=$(wc -l <"$A"); w1=$(grep -c 'exit=' "$W")
echo "БУДИЛЬНИК $(date +%H:%M)"
sed -n "$((a0 + 1)),${a1}p" "$A" | grep 'NEEDS-LEAD' | sed 's/^/внимание: /'
echo "без ведущего: READY-COMMIT $(sed -n "$((a0 + 1)),${a1}p" "$A" | grep -c READY-COMMIT), выходов писателей $((w1 - w0)), коммитов $(git -C /home/save/Alexandrie/HarnessOverHarness log --oneline --since='15 minutes ago' | wc -l) за 15 мин"
echo "$a1 $w1" >"$C"
[[ -e /dev/shm/hoh.disabled ]] && echo "TMPFS ВЫКЛЮЧЕН"
(( SECONDS >= end )) && echo "запасной таймер ${limit} с"
exit 0
