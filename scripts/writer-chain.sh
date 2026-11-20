#!/usr/bin/env bash
# Очередь одного писателя строго по порядку (flock сам по себе не FIFO):
#   setsid nohup scripts/writer-chain.sh WRITER LIST1 TXT1 [LIST2 TXT2 ...] >/dev/null 2>&1 </dev/null &
# Держит WRITER.autofix.lock на всю цепочку, поэтому auto-fix.sh конвейера встанет за ней.
# Каждый шаг — scripts/resume-writers.sh LIST TXT (тег и сессия — в LIST). Журнал — logs/lead1118/WRITER.chain.log.
set -uo pipefail
cd "$(dirname "$0")/.."
writer=$1; shift
D=/mnt/toshiba/alexandrie/hoh/logs/lead1118
exec 9>"$D/$writer.autofix.lock"; flock 9
while (( $# >= 2 )); do
  echo "$(date +%H:%M) start $1" >>"$D/$writer.chain.log"
  scripts/resume-writers.sh "$1" "$2"; rc=$?
  echo "$(date +%H:%M) end rc=$rc $1" >>"$D/$writer.chain.log"
  shift 2
done
