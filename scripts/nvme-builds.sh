#!/usr/bin/env bash
# Предел каталогов сборки на NVMe (CEO 20.11 17:5x): /home/save/.cache/hoh-build, не больше 25 ГБ.
#   systemd-run --user --collect --unit hoh-nvme-builds ... bash scripts/nvme-builds.sh
# Каждые 30 с: если каталог больше предела — удаляется самый давний подкаталог, в который сейчас не собирает ни один
# процесс (CARGO_TARGET_DIR в /proc/*/environ). Ничего вне своего каталога не трогает. Остановка сборок при
# свободном месте на / < 30 ГБ — в scripts/cargo-governor.sh.
set -uo pipefail
B=/home/save/.cache/hoh-build; LIMIT_MB=$((25 * 1024)); LOG=/mnt/toshiba/alexandrie/hoh/logs/nvme-builds.log
in_use() {   # прямой путь или ссылка /mnt/toshiba/alexandrie/hoh/<имя> → этот каталог
  grep -qszE "^CARGO_TARGET_DIR=($1|/mnt/toshiba/alexandrie/hoh/$(basename "$1"))(/.*)?$" /proc/[0-9]*/environ 2>/dev/null ||
  # QA и разовые команды ставят CARGO_TARGET_DIR только на время команды: каталог с записью за 30 мин тоже занят
  [[ -n $(find "$1" -mmin -30 -print -quit 2>/dev/null) ]]; }
while :; do
  used=$(du -sm "$B" 2>/dev/null | cut -f1)
  while (( ${used:-0} > LIMIT_MB )); do
    victim=
    while read -r _ d; do in_use "$d" || { victim=$d; break; }; done < <(find "$B" -mindepth 1 -maxdepth 1 -type d -printf '%T@ %p\n' | sort -n)
    [[ -z $victim ]] && { echo "$(date +%H:%M:%S) ${used}M > ${LIMIT_MB}M, все каталоги заняты" >>"$LOG"; break; }
    sz=$(du -sm "$victim" | cut -f1); rm -rf -- "$victim"
    echo "$(date +%H:%M:%S) evicted $(basename "$victim") (${sz}M), было ${used}M" >>"$LOG"
    used=$(du -sm "$B" | cut -f1)
  done
  sleep 30
done
