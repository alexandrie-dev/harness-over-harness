#!/usr/bin/env bash
# Run a Codex writer (new or resumed session) on a prompt file, never waiting on stdin.
#   scripts/codex-writer.sh WRITER SESSION|new PROMPT_FILE TAG
# EXTRA_ROOTS (optional, resume only): more writable roots, e.g. '"/path/a","/path/b"'.
# Build directory: /mnt/toshiba/alexandrie/hoh/target-WRITER. Logs: logs/stage-WRITER.TAG.{report.md,stdout,stderr}.
set -uo pipefail
cd "$(dirname "$0")/.."
writer=$1 session=$2 prompt_file=$3 tag=$4
L=/mnt/toshiba/alexandrie/hoh/logs
target=/mnt/toshiba/alexandrie/hoh/target-$writer
out=$L/$writer.$tag
mkdir -p "$target"
prompt=$(cat "$prompt_file")
if [[ $session == new ]]; then
  codex exec -s workspace-write --add-dir "$target" --add-dir /dev/shm/hoh -c model_reasoning_effort=high \
    -o "$out.report.md" "$prompt" </dev/null >"$out.stdout" 2>"$out.stderr"
else
  codex exec resume "$session" -c 'sandbox_mode="workspace-write"' \
    -c "sandbox_workspace_write.writable_roots=[\"$target\",\"/dev/shm/hoh\"${EXTRA_ROOTS:+,$EXTRA_ROOTS}]" -c model_reasoning_effort=high \
    -o "$out.report.md" "$prompt" </dev/null >"$out.stdout" 2>"$out.stderr"
fi
rc=$?
if grep -q 'usage limit' "$out.stderr" 2>/dev/null; then   # 19.11: остановка поставщика, а не конец хода
  P=/mnt/toshiba/alexandrie/hoh/pipeline
  [[ -e $P/codex.disabled ]] || echo "$(date +%H:%M) NEEDS-LEAD CODEX-LIMIT: $writer $tag — $(grep -o 'try again at [^.]*' "$out.stderr" | head -1)" >>"$P/attention.log"
  touch "$P/codex.disabled"; rc=75
fi
echo "codex $writer $tag exit=$rc"
echo "--- report $out.report.md"
cat "$out.report.md" 2>/dev/null || echo "(no report)"
exit $rc
