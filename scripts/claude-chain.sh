#!/usr/bin/env bash
# Строгая очередь одного писателя Claude: bash scripts/claude-chain.sh WRITER MODEL PROMPT TAG [PROMPT TAG]...
# Запуск — временной службой: systemd-run --user --collect --unit hoh-chain-WRITER ... (переживает перезапуск T3).
# ВАЖНО: править этот файл только подменой (запись во временный файл + mv): идущие цепочки читают его по ходу.
set -uo pipefail
cd "$(dirname "$0")/.."
writer=$1 model=$2; shift 2
D=/mnt/toshiba/alexandrie/hoh/logs/lead1118
B=/home/save/.cache/hoh-build
exec 9>"$D/$writer.autofix.lock"; flock 9

# CEO 20.11 19:17: не больше 8 одновременно идущих ходов claude -p (писатели, ревью, QA). CEO 20.11 20:20: пока USB/диск
# перегружен (/proc/pressure/io full avg10 ≥ 20 %) — не больше 6; после разгрузки 8. Приоритет: u1|q1|o1|s4 → 1, s5 → 2,
# s1|s2 → 3, остальные (f1, s3, jobs…) → 5 и стартуют только при full avg10 < 20 %.
io_full() { awk '/^full/{sub("avg10=","",$2); print int($2)}' /proc/pressure/io 2>/dev/null || echo 0; }
max_turns() { (( $(io_full) >= 20 )) && echo 6 || echo 8; }   # ревью конвейера — отдельный предел 8 в pipeline.sh
prio() { case $1 in u1|q1|o1|s4) echo 1;; s5) echo 2;; s1|s2) echo 3;; *) echo 5;; esac; }
turns() { local p n=0; for p in /proc/[0-9]*; do [[ $(cat "$p/comm" 2>/dev/null) == claude ]] || continue
  [[ $(tr '\0' '\n' <"$p/cmdline" 2>/dev/null | sed -n 2p) == -p ]] && n=$((n + 1)); done; echo $n; }
wait_slot() {   # ожидающий с меньшим приоритетом (или тем же, но позже) пропускает вперёд
  local me=$D/slots/$writer.wait mine=$(prio "$writer") f best
  mkdir -p "$D/slots"; echo "$mine" >"$me"
  while :; do
    best=$(for f in "$D"/slots/*.wait; do [[ -e $f ]] && echo "$(cat "$f") $(stat -c %Y "$f") $f"; done | sort -n -k1,1 -k2,2 | head -1 | cut -d' ' -f3)
    if [[ $best == "$me" ]] && { (( mine < 5 )) || (( $(io_full) < 20 )); }; then
      exec 8>"$D/slots.lock"; flock 8
      if (( $(turns) < $(max_turns) )); then rm -f "$me"; return 0; fi   # замок 8 держится до появления процесса хода
      flock -u 8
    fi
    sleep 30
  done
}
# CEO 20.11 19:17: NVMe ≤40 ГБ (20.11 20:20). На границе хода: при занятых >37 ГБ каталог писателя переезжает с NVMe на toshiba.
nvme_move() {
  local link=/mnt/toshiba/alexandrie/hoh/target-$writer used
  [[ -L $link && $(readlink "$link") == "$B"/* ]] || return 0
  used=$(du -sm "$B" 2>/dev/null | cut -f1)
  (( ${used:-0} > 37 * 1024 )) || return 0
  rm -f "$link" && mkdir -p "$link" && rm -rf -- "$B/target-$writer"
  echo "$(date +%H:%M) target-$writer: NVMe ${used}M > 37G — каталог перенесён на toshiba" >>/mnt/toshiba/alexandrie/hoh/logs/nvme-builds.log
}

while (( $# >= 2 )); do
  echo "$(date +%H:%M) start $2 ($1)" >>"$D/$writer.chain.log"
  for try in 1 2 3; do   # CEO 20.11: ENOSPC — не провал пакета, ход перезапускается (до трёх раз, пауза 5 мин)
    nvme_move; wait_slot
    ( sleep 20; flock -u 8 ) & bash scripts/claude-writer.sh "$writer" "$model" "$1" "$2" 8>&-; rc=$?
    exec 8>&-
    r=/mnt/toshiba/alexandrie/hoh/logs/$writer.$2.report.md
    head -1 "$r" 2>/dev/null | grep -q '^ГОТОВ' && break
    grep -qsE 'No space left on device|ENOSPC|нет места' "$r" || break
    echo "$(date +%H:%M) ENOSPC — повтор $try $2" >>"$D/$writer.chain.log"; cp "$r" "$r.enospc$try"; sleep 300
  done
  echo "$(date +%H:%M) end rc=$rc $2" >>"$D/$writer.chain.log"
  shift 2
done
