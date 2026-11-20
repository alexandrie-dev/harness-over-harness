#!/usr/bin/env bash
# Resume Codex writers after the lead's session ended, detached from it:
#   setsid nohup scripts/resume-writers.sh LIST_FILE PROMPT_FILE &
# LIST_FILE lines: WRITER SESSION TAG [EXTRA_ROOTS]. Two writers per wave, waves 5 min apart (CEO memory rule).
# Each finished writer appends "HH:MM WRITER TAG exit=N" to logs/resume-writers.done.
set -uo pipefail
cd "$(dirname "$0")/.."
list=$1 prompt=$2
done_file=/mnt/toshiba/alexandrie/hoh/logs/resume-writers.done
n=0
while read -r writer session tag extra; do
  [[ -z $writer || $writer == \#* ]] && continue
  if (( n > 0 && n % 2 == 0 )); then sleep 300; fi
  ( EXTRA_ROOTS=$extra scripts/codex-writer.sh "$writer" "$session" "$prompt" "$tag" >/dev/null 2>&1
    echo "$(date +%H:%M) $writer $tag exit=$?" >> "$done_file" ) &
  echo "$(date +%H:%M) started $writer $tag pid=$!" >> "$done_file"
  n=$((n + 1))
done < "$list"
wait
