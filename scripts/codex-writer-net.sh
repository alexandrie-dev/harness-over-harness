#!/usr/bin/env bash
# scripts/codex-writer-net.sh WRITER PROMPT_FILE TAG [EXTRA_DIR...]
# A new Codex writer whose sandbox may use the network (only for briefs that allow downloads).
set -uo pipefail; cd "$(dirname "$0")/.."
writer=$1 prompt_file=$2 tag=$3; shift 3
L=/mnt/toshiba/alexandrie/hoh/logs; out=$L/$writer.$tag
dirs=(--add-dir "/mnt/toshiba/alexandrie/hoh/target-$writer"); for d in "$@"; do dirs+=(--add-dir "$d"); done
codex exec -s workspace-write "${dirs[@]}" -c sandbox_workspace_write.network_access=true -c model_reasoning_effort=high \
  -o "$out.report.md" "$(cat "$prompt_file")" </dev/null >"$out.stdout" 2>"$out.stderr"
rc=$?; echo "codex $writer $tag exit=$rc"; echo "--- report $out.report.md"; cat "$out.report.md" 2>/dev/null || echo "(no report)"; exit $rc
