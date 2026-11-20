#!/usr/bin/env bash
# Проход по очереди принятых пакетов pipeline/commit-waiting.txt (имя<TAB>пути<TAB>примечание).
# Каждый пакет коммитится через commit-check.sh (чистая выгрузка кандидата). Порядок зависимостей заранее
# неизвестен, поэтому проходы повторяются, пока хоть один пакет закоммитился. Закоммиченные строки
# удаляются из очереди; оставшиеся (с причиной в logs/commit-check.done) ждут ведущего.
set -uo pipefail
cd "$(dirname "$0")/.."
Q=/mnt/toshiba/alexandrie/hoh/pipeline/commit-waiting.txt
while :; do
  progress=0
  while IFS=$'\t' read -r name paths note pkg; do   # pkg: коммит пакета C (CEO 20.11 19:3x); пусто — старые строки, из общего дерева
    [[ -z $name ]] && continue
    read -ra p <<<"$paths"
    for d in "${p[@]}"; do
      [[ $d == crates/* ]] || continue
      c=${d#crates/}
      awk -F'\t' -v OFS='\t' -v c="$c" '$5==c{$8="done"}1' docs/porting/MAP.tsv >/dev/shm/map.tmp && cat /dev/shm/map.tmp >docs/porting/MAP.tsv
    done
    if scripts/commit-check.sh ${pkg:+--pkg "$pkg"} "$name принят: $note

Очередь принятых пакетов; чистая выгрузка кандидата собирается (cargo check --workspace --all-targets --locked).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>" "${p[@]}" docs/porting/MAP.tsv </dev/null; then
      progress=1
      grep -vF "$name"$'\t' "$Q" >/dev/shm/cq.tmp; cat /dev/shm/cq.tmp >"$Q"
    else   # не закоммичен: в MAP не должно остаться done, иначе соседний коммит унесёт его
      for d in "${p[@]}"; do
        [[ $d == crates/* ]] || continue
        awk -F'\t' -v OFS='\t' -v c="${d#crates/}" '$5==c{$8="accepted"}1' docs/porting/MAP.tsv >/dev/shm/map.tmp && cat /dev/shm/map.tmp >docs/porting/MAP.tsv
      done
    fi
  done < <(cat "$Q")
  (( progress )) || break
done
left=$(grep -c . "$Q")
echo "осталось в очереди: $left"
names=$(cut -f1 "$Q" | sort | tr '\n' ' '); last=$(dirname "$Q")/commit-queue.last
[[ $names == "$(cat "$last" 2>/dev/null)" ]] && exit 0   # тот же остаток (пакет ждёт соседа) — не будить ведущего повторно
echo "$names" >"$last"
(( left )) && echo "$(date +%H:%M) NEEDS-LEAD commit-queue: не закоммичено $left — $(cut -f1 "$Q" | tr '\n' ' ')(logs/commit-check.done)" >>"$(dirname "$Q")/attention.log"
exit 0
