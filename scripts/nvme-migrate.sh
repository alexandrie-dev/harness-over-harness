#!/usr/bin/env bash
# CEO 20.11 20:17: машина стоит в ожидании USB (sdb). Горячее переезжает на NVMe /home/save/.cache/hoh-build (предел 40 ГБ,
# разрешил основатель): каталоги сборки конвейера/приёмки/commit-check/писателей и рабочие деревья wt/*, it/*.
# На /mnt/toshiba остаются CARGO_HOME, журналы, снимки, pnpm-store, QA.
#   systemd-run --user --collect --unit hoh-nvme-migrate --setenv=PATH="$PATH" --setenv=HOME="$HOME" bash scripts/nvme-migrate.sh
# Круг каждые 30 с по всем ещё не перенесённым пунктам; простаивающий переносится, занятый ждёт следующего круга.
#  - каталог сборки: замок дорожки (flock -n) + никто не собирает в него и не стоит в нём; переносится БЕЗ копирования:
#    прежний → $H/usb-old/<имя> (не target-*: hoh-tmpfs превращает любой target-* в ссылку), на его месте ссылка на пустой каталог NVMe (сборка начнётся заново);
#  - дерево: rsync без target/ (с node_modules), на старом месте ссылка — пути git и сессий Claude не меняются;
#    дерево писателя (wt/*) копируется под $D/slots.lock (новые ходы цепочек ждут), только пока идёт копия;
#  - замки: it/run-<дорожка>.lock (конвейер), accept-lane-<дорожка>.lock (приёмка), pipeline/commit-queue.lock (commit-check);
#  - свободно на / < 30 ГБ — перенос пропускается; старые копии в $H/usb-old удаляются в конце с ionice idle, по одной.
set -uo pipefail
H=/mnt/toshiba/alexandrie/hoh; B=/home/save/.cache/hoh-build; D=$H/logs/lead1118; LOG=$H/logs/nvme-migrate.log
log() { echo "$(date +%H:%M:%S) $*" | tee -a "$LOG"; }
root_free() { df -BG --output=avail / | tail -1 | tr -dc 0-9; }
BUSY_BY=
busy() {   # кто-то собирает в каталог ($1, путь на toshiba) или стоит в нём (cwd)
  local real m; real=$(readlink -f "$1"); BUSY_BY=
  m=$(grep -lszE "^CARGO_TARGET_DIR=($1|$real)(/.*)?$" /proc/[0-9]*/environ 2>/dev/null | head -1)
  [[ -n $m ]] && { BUSY_BY="CARGO_TARGET_DIR, $(dirname "$m")"; return 0; }
  m=$(find /proc -mindepth 2 -maxdepth 2 -name cwd -printf '%h %l\n' 2>/dev/null \
      | awk -v a="$1" -v b="$real" '$2==a||$2==b||index($2,a "/")==1||index($2,b "/")==1{print $1; exit}')
  [[ -n $m ]] && { BUSY_BY="cwd, $m ($(cat "$m/comm" 2>/dev/null))"; return 0; }
  return 1; }
space() { local f; f=$(root_free); (( f >= 30 + ${1:-0} )) || { log "ПРОПУСК: на / свободно ${f}G (нужно ≥30 + ${1:-0})"; return 1; }; }

move_build() {   # $1 — путь на toshiba; 0 — перенесён, 1 — занят/пропуск
  local p=$1 name; name=$(basename "$p")
  if [[ -L $p && $(readlink "$p") == "$B"/* ]]; then return 0; fi
  [[ -e $p || -L $p ]] || return 0
  space || return 1
  busy "$p" && { MSG="$name занят: $BUSY_BY"; return 1; }
  mkdir -p "$B/$name"
  mkdir -p "$H/usb-old"
  if [[ -L $p ]]; then rm -f "$p"; else mv "$p" "$H/usb-old/$name.usb-old"; fi
  ln -s "$B/$name" "$p"; log "сборка $name → NVMe (заново, без копирования)"
}
move_tree() {   # $1 — путь дерева на toshiba; копия на NVMe, на старом месте ссылка
  local p=$1 rel sz old sfd=; rel=${p#"$H"/}
  [[ -L $p ]] && return 0
  [[ -d $p ]] || return 0
  busy "$p" && { MSG="$rel занят: $BUSY_BY"; return 1; }
  sz=$(du -sm --exclude=target "$p" 2>/dev/null | cut -f1)
  space $(( ${sz:-0} / 1024 + 1 )) || return 1
  mkdir -p "$B/$(dirname "$rel")"
  rsync -a --delete --exclude=/target "$p/" "$B/$rel/" || { log "ОШИБКА копии $rel — оставлен на toshiba"; rm -rf -- "$B/$rel"; MSG="$rel: ошибка копии"; return 1; }
  if [[ ${SLOTS:-} == 1 ]]; then   # новые ходы придерживаем только на время короткой досинхронизации
    exec {sfd}>"$D/slots.lock"; flock -w 120 "$sfd" || { exec {sfd}>&-; MSG="$rel: slots.lock занят"; return 1; }
  fi
  busy "$p" && { MSG="$rel занят после копии: $BUSY_BY"; [[ -n $sfd ]] && exec {sfd}>&-; return 1; }
  rsync -a --delete --exclude=/target "$p/" "$B/$rel/"   # догнать изменения за время первой копии
  busy "$p" && { MSG="$rel занят после второй копии: $BUSY_BY"; [[ -n $sfd ]] && exec {sfd}>&-; return 1; }
  old=$H/usb-old/$(tr / _ <<<"$rel").usb-old; mkdir -p "$H/usb-old"
  mv "$p" "$old" && ln -s "$B/$rel" "$p"
  if ! git -C "$p" status --porcelain >/dev/null 2>&1; then   # проверка: git работает через ссылку
    [[ -n $sfd ]] && exec {sfd}>&-; rm -f "$p"; mv "$old" "$p"; log "ОШИБКА git status в $rel после подмены — подмена отменена"; MSG="$rel: git status"; return 1
  fi
  [[ -n $sfd ]] && exec {sfd}>&-
  log "дерево $rel → NVMe (${sz}M)"
}

# пункты: "вид путь замок" (замок «-» — нет, «slots» — $D/slots.lock). Деревья: it/* — по дорожкам, wt/* — динамически.
items() {
  local lane w t
  for lane in a b; do
    echo "build $H/lead-pl-build-$lane $H/it/run-$lane.lock"; echo "tree $H/it/run-$lane $H/it/run-$lane.lock"
    echo "build $H/target-accept-$lane $H/accept-lane-$lane.lock"; echo "tree $H/it/acc-$lane $H/accept-lane-$lane.lock"
  done
  echo "build $H/cc-build $H/pipeline/commit-queue.lock"; echo "build $H/cc-build-disk $H/pipeline/commit-queue.lock"
  for t in target-s4 lead-pl-build target-conf-main; do echo "build $H/$t -"; done
  echo "tree $H/it/integ -"
  for w in "$H"/wt/*; do [[ -d $w ]] && echo "tree $w slots"; done
}
declare -A DONE=()
MSG=
try_item() {   # $1 вид, $2 путь, $3 замок
  local kind=$1 p=$2 lock=$3 rc fd=
  [[ -e $p || -L $p ]] || return 0
  SLOTS=; [[ $lock == slots ]] && SLOTS=1
  if [[ $lock != - && $lock != slots ]]; then exec {fd}>"$lock"; flock -n "$fd" || { exec {fd}>&-; MSG="$(basename "$p"): замок $(basename "$lock") занят"; return 1; }; fi
  MSG=; "move_$kind" "$p" </dev/null; rc=$?
  [[ -n $fd ]] && exec {fd}>&-
  return $rc
}

log "старт: / свободно $(root_free)G, NVMe $(du -sm "$B" | cut -f1)M; $(grep full /proc/pressure/io)"
round=0
while :; do
  left=0; (( round++ ))
  while read -r kind p lock; do
    [[ -n ${DONE[$p]:-} ]] && continue
    if try_item "$kind" "$p" "$lock"; then DONE[$p]=1; else left=$((left + 1)); (( round % 10 == 1 )) && log "жду: $MSG"; fi
  done < <(items)
  (( left == 0 )) && break
  sleep 30
done
log "всё перенесено: / свободно $(root_free)G, NVMe $(du -sm "$B" | cut -f1)M; $(grep full /proc/pressure/io)"

# Старые копии на USB — удаление с наименьшим приоритетом ввода-вывода, по одной
del_old() { local o; for o in "$H"/usb-old/*.usb-old; do
  [[ -e $o ]] || continue; ionice -c3 nice -n19 rm -rf -- "$o"; log "удалён $(basename "$o")"; done; }
del_old
# Деревья писателей, которые создаст уборка общего дерева (hoh-main-cleanup) на toshiba: подхватить, пока она жива
while systemctl --user is-active -q hoh-main-cleanup; do
  while read -r kind p lock; do
    [[ $kind == tree && $lock == slots && -z ${DONE[$p]:-} ]] && try_item "$kind" "$p" "$lock" && DONE[$p]=1
  done < <(items)
  del_old; sleep 30
done
log "готово: служба hoh-main-cleanup завершена, новых деревьев нет"
