#!/usr/bin/env bash
# scripts/kimi-writer.sh WRITER PROMPT_FILE TAG — one Kimi writer run (non-interactive, decides on its own).
# Kimi has no sandbox: the prompt must state the boundaries. Report = its printed output.
set -uo pipefail; cd "$(dirname "$0")/.."
writer=$1 prompt_file=$2 tag=$3
L=/mnt/toshiba/alexandrie/hoh/logs; out=$L/$writer.$tag; mkdir -p "/mnt/toshiba/alexandrie/hoh/target-$writer"
kimi --add-dir "/mnt/toshiba/alexandrie/hoh/target-$writer" --add-dir /mnt/toshiba/alexandrie/hoh/refs/deepseek-harness \
  -p "$(cat "$prompt_file")" </dev/null >"$out.report.md" 2>"$out.stderr"
rc=$?; echo "kimi $writer $tag exit=$rc"; tail -c 4000 "$out.report.md"; exit $rc
