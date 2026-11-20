#!/usr/bin/env bash
# Automatic path from a writer's "ГОТОВ" to acceptance (CEO 19.11 10:45). The lead only gets what needs a decision.
#
#   scripts/pipeline.sh submit WRITER_MODEL KIND CRATE... [-- NOTE]   enqueue one package (one action per report)
#       WRITER_MODEL: codex | sonnet   (decides the reviewer: codex code → Kimi, sonnet code → Codex)
#       KIND: new (full review) | fix (closure check against the crate's last review)
#   setsid nohup scripts/pipeline.sh daemon >/dev/null 2>&1 </dev/null &   process the queue (also scans Codex reports)
#
# Per job: (1) the lead's run outside the sandbox (scripts/lead-run.sh, tmpfs, HOH_PRIORITY); (2) review by the other
# model with a template brief that must end with "ВЕРДИКТ: ЧИСТО" or "ВЕРДИКТ: ЗАМЕЧАНИЯ"; (3) on ЧИСТО: scripts/accept.sh.
# Results: READY-COMMIT lines (acceptance green — the lead commits) and NEEDS-LEAD lines (red run, review findings,
# acceptance failure, unparsable report) are appended to $P/attention.log; everything to $P/pipeline.log.
# Codex writer reports are picked up by themselves: a new logs/<writer>.<tag>.report.md whose first line is
# "ГОТОВ <crate>: тесты N/0, clippy, fmt" becomes a job (kind fix when the tag starts with "fix", otherwise new).
set -uo pipefail
cd "$(dirname "$0")/.."
H=/mnt/toshiba/alexandrie/hoh; L=$H/logs; P=$H/pipeline; REF=$H/refs/deepseek-harness
mkdir -p "$P/queue" "$P/done" "$P/reviews"
say() { echo "$(date +%H:%M:%S) $*" >>"$P/pipeline.log"; }
attention() { echo "$(date +%H:%M) $*" >>"$P/attention.log"; say "ATTENTION $*"; }

submit() {
  local model=$1 kind=$2; shift 2; local crates=() note=""
  while (($#)); do [[ $1 == -- ]] && { shift; note="$*"; break; }; crates+=("$1"); shift; done
  local id; id=$(date +%H%M%S)-${crates[0]}
  printf 'model=%s\nkind=%s\ncrates=%s\nnote=%s\n' "$model" "$kind" "${crates[*]}" "$note" >"$P/queue/$id.job"
  echo "queued $id"
}

brief() {  # brief KIND CRATES... → review prompt on stdout
  local kind=$1; shift; local c src prev
  echo "You only read. Do not run builds, tests, servers, browsers or any command that writes; do not edit or create files."
  echo "The lead already ran the tests, clippy and fmt outside the sandbox: green. Read docs/porting/PORTING.md (§14 checklist)."
  for c in "$@"; do
    src=$(awk -F'\t' -v c="$c" '$5==c{print $3; exit}' docs/porting/MAP.tsv)
    echo "Crate: crates/$c (read its PORT_NOTES.md first; declared differences are not findings unless wrong or unjustified)."
    [[ -n $src ]] && echo "TypeScript source: $REF/$src (src/, tests/, locale)."
    prev=$(ls -t "$P/reviews/$c".*.md 2>/dev/null | head -1)
    [[ $kind == fix && -n $prev ]] && echo "This is a fix round. Previous review to check closure against: $prev"
  done
  if [[ $kind == fix && ! " $* " =~ (sandbox|credential|approval|guard|supply|permission|harness) ]]; then
    # CEO 20.11 16:1x: круг исправлений — только исправленные места и затронутые связи; блокируют только ошибка и
    # нарушение требования, рекомендации — заметками (не держат приёмку). Защита (список выше) — полный просмотр.
    echo "For each previous finding: closed / not closed, file:line, the proving test, would it fail on the old code."
    echo "Look ONLY at the changed places and the code they touch. A new finding must be a bug or a violated requirement;"
    echo "everything else goes under 'Заметки' and does not block: such notes alone still mean 'ВЕРДИКТ: ЧИСТО'."
  elif [[ $kind == fix ]]; then
    echo "Security-relevant crate: for each previous finding: closed / not closed, file:line, the proving test, would it fail on the old code."
    echo "Then a full review of the crate against the source as for a new package."
  else
    echo "Full review against the source: behaviour, order of effects, exact error/output texts, data loss, leaks, locks held across await,"
    echo "blocking calls on the runtime thread, secrets in Debug/errors. Compare the source spec files with the Rust tests: every case"
    echo "present or listed as deferred with a reason. Unobservable type differences are notes, not findings."
  fi
  # CEO 20.11 19:0x: для гнезда внешнего исполнителя отчёт явно называет режим по умолчанию и что включает расширенный.
  [[ " $* " =~ (external-harness|harness-claude) ]] && echo "State explicitly: which permission mode is the default, and exactly what a person turns on by switching to the extended mode (which actions then run without a question)."
  echo "Output in Russian, most severe first: kind, Rust file:line, TS file:line, input with both outputs, test to add. ≤700 words."
  echo "The LAST line must be exactly one of: 'ВЕРДИКТ: ЧИСТО' (no bug, no missing required behaviour; notes allowed) or 'ВЕРДИКТ: ЗАМЕЧАНИЯ'."
}

review_slot() {   # ждать, пока идущих ходов claude -p меньше 8; ревью — между защитой (1) и прогонщиком, без очереди приоритетов
  local p n
  while :; do
    n=0; for p in /proc/[0-9]*; do [[ $(cat "$p/comm" 2>/dev/null) == claude ]] || continue
      [[ $(tr '\0' '\n' <"$p/cmdline" 2>/dev/null | sed -n 2p) == -p ]] && n=$((n + 1)); done
    (( n < 8 )) && return 0; sleep 30
  done
}

run_job() {
  local job=$1 id model kind crates note; id=$(basename "$job" .job)
  model=$(sed -n 's/^model=//p' "$job"); kind=$(sed -n 's/^kind=//p' "$job"); crates=$(sed -n 's/^crates=//p' "$job")
  note=$(sed -n 's/^note=//p' "$job")
  # shellcheck disable=SC2086
  set -- $crates
  say "$id start ($model, $kind: $crates)"
  scripts/lead-run.sh "pl-$id" "/mnt/toshiba/alexandrie/hoh/lead-pl-build" 2400 "$@"   # CEO 20.11 19:17: NVMe ≤25 ГБ — прогон на toshiba; было 17:2x: прогон ведущего на USB, tmpfs — писателям
  local line; line=$(grep " pl-$id " "$L/lead-runs.done" | tail -1)
  if [[ $line =~ (test|clippy)=(143|137|130)\  ]]; then   # 19.11 19:08: оборван сигналом (предохранитель памяти) — не дефект пакета
    say "$id: прогон оборван сигналом ($line) — повтор через 2 мин"; sleep 120
    submit "$model" "$kind" "$@" -- "$note" >/dev/null; return
  fi
  if [[ $line != *"test=0 "*"clippy=0 fmt=0"* ]] && grep -qsE 'No space left on device|signal 7 \[Bus error\]' "$L/pl-$id.lead-test.txt" "$L/pl-$id.lead-clippy.txt"; then
    say "$id: кончилось место в tmpfs — повтор через 3 мин"; sleep 180   # 20.11 02:08: не дефект пакета
    submit "$model" "$kind" "$@" -- "$note" >/dev/null; return
  fi
  if [[ $line != *"test=0 "*"clippy=0 fmt=0"* ]] && grep -qs 'failed to load manifest for workspace member' "$L/pl-$id.lead-test.txt" "$L/pl-$id.lead-clippy.txt"; then
    say "$id: чужой недописанный манифест ломает workspace — повтор через 3 мин"; sleep 180
    submit "$model" "$kind" "$@" -- "$note" >/dev/null; return   # не будить ведущего: это не дефект пакета
  fi
  if [[ $line != *"test=0 "*"clippy=0 fmt=0"* && $note =~ ^from\ $L/([a-z0-9]+)\.[^/]*\.report\.md$ ]] \
     && grep -q "^${BASH_REMATCH[1]}	" "$P/writers.tsv" \
     && (( $(ls "$L/lead1118/${BASH_REMATCH[1]}-autofix-$1-"*.txt 2>/dev/null | wc -l) < 3 )); then   # красный прогон → писателю
    local w=${BASH_REMATCH[1]} rr=$P/reviews/$1.$id-redrun.md
    { echo "Прогон ведущего ВНЕ песочницы (сокеты разрешены): $line"
      echo "Логи: $L/pl-$id.lead-test.txt, $L/pl-$id.lead-clippy.txt; fmt: cargo fmt -p $1 -- --check."
      grep -E '^(---- |thread .* panicked)|^  (left|right):|^error(\[|:)' "$L/pl-$id.lead-test.txt" "$L/pl-$id.lead-clippy.txt" | head -40
      echo "В песочнице писателя сокеты запрещены — такие тесты там не исполняются по-настоящему. Найди причину по логам и TS-источнику, исправь код или тест (что неверно), объясни в отчёте."
    } >"$rr"
    setsid nohup scripts/auto-fix.sh "$w" "$1" "$rr" >/dev/null 2>&1 </dev/null &
    say "$id AUTO-FIX (красный прогон) $w $1"; echo "$(date +%H:%M) AUTO-FIX красный $w $crates — $rr" >>"$P/auto-fix.log"; return
  fi
  if [[ $line != *"test=0 "*"clippy=0 fmt=0"* ]]; then attention "NEEDS-LEAD $crates: прогон красный — $line (logs/pl-$id.lead-*.txt) $note"; return; fi
  local out=$P/reviews/${1}.$id.md prompt; prompt=$(brief "$kind" "$@")
  if [[ $model == codex ]]; then
    timeout 3600 kimi --add-dir "$REF" -p "$prompt" </dev/null >"$out" 2>"$out.stderr"
    if ! grep -qE 'ВЕРДИКТ: (ЧИСТО|ЗАМЕЧАНИЯ)' "$out"; then   # Kimi quota/crash: Sonnet reviews instead (still not Codex)
      say "$id kimi gave no verdict ($(tail -c 200 "$out.stderr" | tr '\n' ' ')); Sonnet reviews"
      review_slot   # CEO 20.11 19:17: общий предел 8 ходов claude -p (scripts/claude-chain.sh)
      timeout 3600 claude -p --model sonnet --add-dir "$REF" --allowedTools Read Grep Glob \
        --disallowedTools Bash Edit Write NotebookEdit WebFetch WebSearch -- "$prompt" </dev/null >"$out" 2>"$out.stderr"
    fi
  else   # 19.11 18:25 (CEO): лимит Codex сброшен — код Sonnet ревьюит Codex только на чтение; при лимите/без вердикта Kimi;
         # Opus не используется (лимит Claude бережём); без вердикта — к ведущему. Sonnet свой код не ревьюит.
    timeout 3600 codex exec -s read-only --add-dir "$REF" -c model_reasoning_effort=high "$prompt" </dev/null >"$out" 2>"$out.stderr"
    if grep -q 'usage limit' "$out.stderr" 2>/dev/null; then
      touch "$P/codex.disabled"; say "$id codex review: usage limit — codex.disabled"
      echo "$(date +%H:%M) NEEDS-LEAD CODEX-LIMIT (ревью $id): $(grep -o 'try again at [^.]*' "$out.stderr" | head -1)" >>"$P/attention.log"
    fi
    if ! grep -qE 'ВЕРДИКТ: (ЧИСТО|ЗАМЕЧАНИЯ)' "$out"; then
      say "$id codex gave no verdict ($(tail -c 200 "$out.stderr" | tr '\n' ' ')); Kimi reviews"
      timeout 3600 kimi --add-dir "$REF" -p "$prompt" </dev/null >"$out" 2>"$out.stderr"
    fi
  fi
  for c in "${@:2}"; do cp "$out" "$P/reviews/$c.$id.md"; done
  local verdict; verdict=$(grep -oE 'ВЕРДИКТ: (ЧИСТО|ЗАМЕЧАНИЯ)' "$out" | tail -1)
  if [[ $verdict == "ВЕРДИКТ: ЗАМЕЧАНИЯ" && $note =~ ^from\ $L/([a-z0-9]+)\.[^/]*\.report\.md$ ]] \
     && grep -q "^${BASH_REMATCH[1]}	" "$P/writers.tsv" \
     && (( $(ls "$L/lead1118/${BASH_REMATCH[1]}-autofix-$1-"*.txt 2>/dev/null | wc -l) < 3 )); then   # после 3 кругов — к ведущему   # Codex-писатель: круг исправлений без ведущего
    local w=${BASH_REMATCH[1]}
    setsid nohup scripts/auto-fix.sh "$w" "$1" "$out" >/dev/null 2>&1 </dev/null &
    say "$id AUTO-FIX $w $1 по $out"; echo "$(date +%H:%M) AUTO-FIX $w $crates — $out" >>"$P/auto-fix.log"; return
  fi
  if [[ $verdict != "ВЕРДИКТ: ЧИСТО" ]]; then attention "NEEDS-LEAD $crates: ревью ${verdict:-без вердикта} — $out $note"; return; fi
  if scripts/accept.sh "pl-$id" "$@" >"$P/done/$id.accept.txt" 2>&1; then
    attention "READY-COMMIT $crates: прогон $line; ревью чисто ($out); приёмка зелёная (reports/pl-$id) $note"
    # коммит без ведущего: в очередь и проход commit-queue (чистая выгрузка); отказ остаётся в очереди и идёт в attention
    printf '%s\t%s\t%s\n' "$crates" "$(printf 'crates/%s ' "$@")" "принят $(date +%H:%M) конвейером (pl-$id)" >>"$P/commit-waiting.txt"
    setsid nohup bash -c "exec 9>$P/commit-queue.lock; flock 9; exec scripts/commit-queue.sh" >>"$L/commit-queue.txt" 2>&1 </dev/null &
  else
    attention "NEEDS-LEAD $crates: приёмка красная — $P/done/$id.accept.txt $note"
  fi
}

scan_codex_reports() {  # Codex writer reports with a ГОТОВ first line become jobs by themselves
  local f first crate tag seen=$P/seen-reports
  touch "$seen"
  for f in $(find "$L" -maxdepth 1 -name '*.report.md' -newer "$P/started" 2>/dev/null); do
    grep -qxF "$f" "$seen" && continue
    first=$(grep -m1 -E '^(ГОТОВ|ОСТАТОК|ПРИЧИНА|СПИСОК ПУСТ)' "$f")
    [[ -z $first ]] && continue   # still being written, or another kind of report
    echo "$f" >>"$seen"
    tag=$(basename "$f" .report.md); tag=${tag#*.}
    crate=$(sed -nE 's/^ГОТОВ (hoh-[a-z0-9-]+): тесты [0-9]+\/0.*/\1/p' <<<"$first")
    if [[ $crate == hoh-desktop ]]; then say "skip $f: hoh-desktop вне workspace — ведущий гоняет probe и тесты сам"
    elif [[ -n $crate ]]; then submit codex "$([[ $tag == fix* ]] && echo fix || echo new)" "$crate" -- "from $f" >/dev/null
      w=$(basename "$f"); w=${w%%.*}   # 19.11 14:20: писатель сразу берёт следующий пакет своего списка
      if [[ ! -e $P/codex.disabled ]] && grep -q "^$w	" "$P/writers.tsv"; then
        local nl="$L/lead1118/$w-next-$(date +%H%M%S).list"
        printf '%s %s next-%s \n' "$w" "$(grep "^$w	" "$P/writers.tsv" | cut -f2)" "$(date +%H%M%S)" >"$nl"
        setsid nohup bash -c "exec 9>$L/lead1118/$w.autofix.lock; flock 9; exec scripts/resume-writers.sh $nl $L/lead1118/resume-auto.txt" >/dev/null 2>&1 </dev/null &
        say "AUTO-NEXT $w после $crate"
      fi
    elif rest=$(sed -nE 's/^ОСТАТОК (hoh-[a-z0-9-]+).*/\1/p' <<<"$first"); w=$(basename "$f"); w=${w%%.*}; \
         [[ -n $rest && ! -e $P/codex.disabled ]] && grep -q "^$w	" "$P/writers.tsv" \
         && (( $(ls "$L/lead1118/$w-cont-$rest-"*.list 2>/dev/null | wc -l) < 4 )); then   # 19.11 14:16: остаток Codex — продолжение без ведущего, до 4 раз на крейт
      local n; n=$(( $(ls "$L/lead1118/$w-cont-$rest-"*.list 2>/dev/null | wc -l) + 1 ))
      local lst="$L/lead1118/$w-cont-$rest-$n.list"
      printf '%s %s cont-%s-%s \n' "$w" "$(grep "^$w	" "$P/writers.tsv" | cut -f2)" "$rest" "$n" >"$lst"
      setsid nohup bash -c "exec 9>$L/lead1118/$w.autofix.lock; flock 9; exec scripts/resume-writers.sh $lst $L/lead1118/resume-auto.txt" >/dev/null 2>&1 </dev/null &
      say "AUTO-CONTINUE $w $rest ($n) по $f"
    else attention "NEEDS-LEAD отчёт писателя требует решения: $f — $first"; fi
  done
}

daemon() {
  [[ -e $P/started ]] || touch "$P/started"
  echo $$ >"$P/daemon.pid"   # перезапуск: kill "$(cat $P/daemon.pid)" — НЕ по pgrep (совпадает с оболочкой и подоболочками заданий)
  say "daemon start (pid $$)"
  while :; do
    scan_codex_reports
    for job in "$P"/queue/*.job; do
      [[ -e $job ]] || continue
      mv "$job" "$P/done/" && ( run_job "$P/done/$(basename "$job")" ) &
    done
    sleep 60
  done
}

case ${1:-} in
  submit) shift; submit "$@" ;;
  daemon) daemon ;;
  *) echo "usage: $0 submit codex|sonnet new|fix CRATE... [-- NOTE] | daemon" >&2; exit 2 ;;
esac
