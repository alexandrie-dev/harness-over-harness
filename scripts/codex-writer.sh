#!/usr/bin/env bash
# Run a Codex writer (new or resumed session) on a prompt file, never waiting on stdin.
#   scripts/codex-writer.sh WRITER SESSION|new PROMPT_FILE TAG
# Build directory: /mnt/toshiba/alexandrie/hoh/target-WRITER. Logs: logs/stage-WRITER.TAG.{report.md,stdout,stderr}.
set -uo pipefail
cd "$(dirname "$0")/.."
writer=$1 session=$2 prompt_file=$3 tag=$4
L=/mnt/toshiba/alexandrie/hoh/logs
target=/mnt/toshiba/alexandrie/hoh/target-$writer
out=$L/$writer.$tag
prompt=$(cat "$prompt_file")
if [[ $session == new ]]; then
  codex exec -s workspace-write --add-dir "$target" -c model_reasoning_effort=high \
    -o "$out.report.md" "$prompt" </dev/null >"$out.stdout" 2>"$out.stderr"
else
  codex exec resume "$session" -c 'sandbox_mode="workspace-write"' \
    -c "sandbox_workspace_write.writable_roots=[\"$target\"]" -c model_reasoning_effort=high \
    -o "$out.report.md" "$prompt" </dev/null >"$out.stdout" 2>"$out.stderr"
fi
rc=$?
echo "codex $writer $tag exit=$rc"
echo "--- report $out.report.md"
cat "$out.report.md" 2>/dev/null || echo "(no report)"
exit $rc
