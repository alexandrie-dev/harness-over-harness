#!/usr/bin/env bash
# Круг исправлений по ревью без ведущего (для пакетов Codex-писателей):
#   scripts/auto-fix.sh WRITER CRATE REVIEW_FILE
# Сессия писателя берётся из /mnt/toshiba/alexandrie/hoh/pipeline/writers.tsv (WRITER<TAB>SESSION<TAB>EXTRA_ROOTS).
# Ждёт конца текущего хода этого писателя (если он идёт), затем возобновляет его с заданием исправить замечания.
# Его отчёт «ГОТОВ <крейт>» с тегом fix… конвейер снова проверит сам (проверка закрытия против этого ревью).
set -uo pipefail
[[ -e /mnt/toshiba/alexandrie/hoh/pipeline/codex.disabled ]] && { echo "$(date +%H:%M) NEEDS-LEAD auto-fix $1 $2: Codex отключён (лимит) — круг не запущен, ревью $3" >>/mnt/toshiba/alexandrie/hoh/pipeline/attention.log; exit 0; }
cd "$(dirname "$0")/.."
writer=$1 crate=$2 review=$3
H=/mnt/toshiba/alexandrie/hoh; D=$H/logs/lead1118
line=$(awk -F'\t' -v w="$writer" '$1==w{print; exit}' "$H/pipeline/writers.tsv")
[[ -z $line ]] && { echo "нет сессии для $writer" >&2; exit 2; }
session=$(cut -f2 <<<"$line"); extra=$(cut -f3 <<<"$line")
n=1; while [[ -e $D/$writer-autofix-$crate-$n.txt ]]; do n=$((n + 1)); done
tag=fixauto-$crate-$n
cat >"$D/$writer-autofix-$crate-$n.txt" <<EOF
Ведущий (автоматически): сначала круг исправлений $crate по ревью — он важнее начатого пакета; если начатый не сдан, оставь как есть и вернись к нему после этого отчёта.
Ревью: $review — исправь все замечания, незакрытые пункты и новые дефекты; заметки — по своему решению, с записью в PORT_NOTES. Если пункт ошибочен — докажи ссылкой на TS-исходник и тестом.
Каждое исправление — тестом, который падает на старом коде. cargo test, cargo clippy --all-targets --no-deps -- -D warnings, cargo fmt --check по крейту — зелёные.
Первая строка отчёта строго: «ГОТОВ $crate: тесты N/0, clippy, fmt».
EOF
printf '%s %s %s %s\n' "$writer" "$session" "$tag" "$extra" >"$D/$writer-autofix-$crate-$n.list"
exec 9>"$D/$writer.autofix.lock"; flock 9   # по одному кругу на писателя; замок держится до конца его хода
pid=$(pgrep -f "codex exec resume $session" | head -1)
[[ -n $pid ]] && tail --pid="$pid" -f /dev/null
sleep 5
exec scripts/resume-writers.sh "$D/$writer-autofix-$crate-$n.list" "$D/$writer-autofix-$crate-$n.txt"
