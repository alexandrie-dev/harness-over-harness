#!/usr/bin/env bash
# scripts/claude-writer.sh WRITER MODEL PROMPT_FILE TAG — один ход писателя Claude (CEO 20.11: Codex и Kimi на лимите).
# Сессия писателя постоянна: UUID в logs/lead1118/<WRITER>.claude-session; первый ход создаёт её, следующие продолжают.
# Песочницы ОС нет: границы — в тексте задания (stage1-writer.md). Отчёт = итоговый текст в logs/<WRITER>.<TAG>.report.md;
# первая строка «ГОТОВ <крейт>: тесты N/0, clippy, fmt» подхватывается конвейером.
# CEO 20.11 19:3x (указание основателя «чтобы они не мешали друг другу»): у каждого писателя своё рабочее дерево git
# /mnt/toshiba/alexandrie/hoh/wt/<WRITER> на ветке w/<WRITER>. Первый ход после перехода создаёт его из снимка общего
# дерева в этот момент — так несданная работа писателя переезжает вместе с ним. Общее дерево — только интеграция.
# ВАЖНО: править этот файл только подменой (временный файл + mv): идущие ходы дочитывают его по ходу.
set -uo pipefail; cd "$(dirname "$0")/.."
G=$PWD
writer=$1 model=$2 prompt_file=$3 tag=$4
H=/mnt/toshiba/alexandrie/hoh; L=$H/logs; out=$L/$writer.$tag; sf=$L/lead1118/$writer.claude-session
W=$H/wt/$writer; NV=/home/save/.cache/hoh-build
if [[ ! -e $W/.git ]]; then
  mkdir -p "$H/wt"; exec 7>"$H/wt/.git.lock"; flock 7
  idx=$(mktemp -p /dev/shm/hoh wt-idx.XXXXXX)
  GIT_INDEX_FILE=$idx git read-tree HEAD && GIT_INDEX_FILE=$idx git add -A -- . \
    && snap=$(git commit-tree "$(GIT_INDEX_FILE=$idx git write-tree)" -p HEAD -m "база дерева $writer: снимок общего дерева $(date +%F\ %H:%M)") \
    && mkdir -p "$NV/wt/$writer" && { [[ -e $W || -L $W ]] || ln -s "$NV/wt/$writer" "$W"; } \
    && git worktree add -q -B "w/$writer" "$W" "$snap" \
    && echo "$(date +%H:%M) $writer: дерево $W, ветка w/$writer от $snap" >>"$L/worktrees.log" \
    || { echo "$(date +%H:%M) NEEDS-LEAD $writer: дерево не создано — ход не запущен" >>"$H/pipeline/attention.log"; rm -f "$idx"; exit 3; }
  rm -f "$idx"; exec 7>&-
fi
mkdir -p "$H/target-$writer"
export CARGO_HOME=$H/cargo-home CARGO_TARGET_DIR=$H/target-$writer CARGO_BUILD_JOBS=2 TMPDIR=/dev/shm/hoh
enc() { sed 's|[^A-Za-z0-9-]|-|g' <<<"$1"; }
if [[ -s $sf ]]; then
  u=$(cat "$sf"); dst=$HOME/.claude/projects/$(enc "$(readlink -f "$W")")   # дерево может быть ссылкой на NVMe: claude берёт физический путь
  for d in "$G" "$W"; do src=$HOME/.claude/projects/$(enc "$d")/$u.jsonl
    [[ -s $src && ! -e $dst/$u.jsonl ]] && mkdir -p "$dst" && cp "$src" "$dst/"; done   # --resume ищет сессию по папке проекта
  sess=(--resume "$u")
else u=$(cat /proc/sys/kernel/random/uuid); echo "$u" >"$sf"; sess=(--session-id "$u"); fi
cd "$W" || exit 3
mcp=(); [[ $writer == u1 || $writer == q1 ]] || mcp=(--strict-mcp-config --mcp-config '{"mcpServers":{}}')   # CEO 20.11 20:20: без MCP, кроме окна (u1, q1)
claude -p --model "$model" "${sess[@]}" --permission-mode bypassPermissions "${mcp[@]}" \
  --add-dir "$H/target-$writer" --add-dir "$H/refs/deepseek-harness" --add-dir /dev/shm/hoh --add-dir "$L/lead1118" \
  --disallowedTools WebFetch WebSearch -- "$(cat "$L/lead1118/common-claude.txt"; echo; cat "$L/lead1118/common-worktree.txt"; echo; echo "Твой id: $writer. Твоя рабочая папка: $W (ветка w/$writer)."; echo; cat "$prompt_file")" </dev/null >"$out.report.md" 2>"$out.stderr"
rc=$?
[[ $rc -ne 0 && ! -s $out.report.md ]] && grep -qi 'no conversation found' "$out.stderr" && echo "$(date +%H:%M) NEEDS-LEAD $writer: сессия не нашлась в новом дереве ($out.stderr)" >>"$H/pipeline/attention.log"
echo "$(date +%H:%M) claude $writer $tag exit=$rc" >>"$L/resume-writers.done"; exit $rc
