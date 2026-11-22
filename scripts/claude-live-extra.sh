#!/bin/bash
# Extra live rows of HoH with the real `claude` (CEO 21.11 ~03:38; verify-4, "Что добавить в живой
# прогон"). Does not touch scripts/claude-live-smoke.sh. Run it only when the lead says so: it
# spends model requests. --dry-run prints the plan and runs nothing.
#
# A host (scripts/claude-live-extra.py) drives the built `hoh --profile window` over stdio as the
# window core does: it answers `approval/request` (signed) and `user-question/ask`. `claude` runs
# through a tap that records its exact argv, folder, pids and frames. Each row has its own DSH_HOME
# in the run folder; the real HOME stays for the CLI (login), nothing in it or in the human's data
# folder is changed: to every request to write there the host answers "rejected".
#
# Rows (each PASS or FAIL with its fact; INFO only for a skipped row with the reason):
#   live.stop.<confirm|full>      «Стоп» mid-turn: turn ends, long command's process gone, next turn works
#   live.resume.<confirm|full>    engine restarted on the same DSH_HOME: --resume, answer remembers a word
#   live.approve.<confirm|full>   confirm: request -> «allow» -> marker file, decision in the journal;
#                                 full: marker file without any request
#   live.question.<confirm|full>  AskUserQuestion: the human's pick reaches Claude (full: fact)
#   live.v1.mode-switch           mode changed between turns: access/get, flags of the process, back, two chats
#   live.v2.cwd                   contract of i7: a folder change of a Claude chat is refused (EXECUTOR_FOLDER_FIXED),
#                                 the next turn runs in the old folder, after an engine restart the same
#                                 conversation resumes (--resume, same id, same folder); an accepted change is FAIL
#   live.v2.lost-form             the same restart: what the CLI says when a conversation is lost
#   live.v3.lost-resume           a missing id in --resume, form of the message
#   live.v5.kill-full, live.v5.shutdown500-full   engine killed mid-turn: claude alive after 5 and 30 s?
#   live.v6.mcp-project[-enabled] .mcp.json in the folder with --setting-sources user,project,local: mcp_servers
#   live.v7.protected-write, live.v7.settings  asked to write into the engine's data / isolated settings: host's text
#   live.v8.read-outside          file outside the folder: does the CLI ask, what the host sees
#   live.v9.secret-url            command with a secret-named URL parameter: window's text vs the command
#
# usage: scripts/claude-live-extra.sh [--dry-run] [--hoh PATH] [--claude PATH] [--dir PATH]
#          [--timeout SECONDS] [--home real|isolated] [--only GROUP[,GROUP…]] [--check-after S1,S2]
#   GROUP: stop resume approve question v1 v2 v3 v5 v6 v7 v8 v9
#   --home isolated: HOME for the CLI is a folder of the run (no login: for a stand-in `claude`);
#     real (default): the human's HOME, only read by the CLI.
# exit: 0 no FAIL; 1 a FAIL; 2 bad usage or no CLI/binary.
set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

OUT=/mnt/toshiba/alexandrie/hoh/scratch-live TIMEOUT=240 ONLY='' HOME_MODE=real AFTER=5,30 DRY=0
TARGET_DIR=${CARGO_TARGET_DIR:-/mnt/toshiba/alexandrie/hoh/target-o1}
HOH=$TARGET_DIR/debug/hoh
CLAUDE=$(command -v claude || true)
while [ $# -gt 0 ]; do
  case $1 in
    --dry-run) DRY=1 ;;
    --hoh) HOH=${2:?--hoh needs a path}; shift ;;
    --claude) CLAUDE=${2:?--claude needs a path}; shift ;;
    --dir) OUT=${2:?--dir needs a path}; shift ;;
    --timeout) TIMEOUT=${2:?--timeout needs seconds}; shift ;;
    --home) HOME_MODE=${2:?--home needs real or isolated}; shift ;;
    --only) ONLY=${2:?--only needs groups}; shift ;;
    --check-after) AFTER=${2:?--check-after needs two numbers}; shift ;;
    -h|--help) sed -n '2,36p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done
[[ $TIMEOUT =~ ^[0-9]+$ ]] || { echo "--timeout needs whole seconds" >&2; exit 2; }
case $HOME_MODE in real|isolated) ;; *) echo "--home needs real or isolated" >&2; exit 2 ;; esac
[[ $AFTER =~ ^[0-9.]+,[0-9.]+$ ]] || { echo "--check-after needs two numbers: 5,30" >&2; exit 2; }
[[ $OUT =~ ^/[A-Za-z0-9_./-]+$ ]] || { echo "--dir needs an absolute path of [A-Za-z0-9_./-]" >&2; exit 2; }

RUN=$OUT/extra-$(date +%Y%m%d-%H%M%S)-$$
if [ $DRY = 1 ]; then
  echo "ПЛАН (dry-run: ничего не создаётся и не запускается)"
  echo "hoh:     $HOH $( [ -x "$HOH" ] && echo '[есть]' || echo '[нет]')"
  echo "claude:  ${CLAUDE:-не найден в PATH}"
  echo "HOME для claude: $HOME_MODE; данные hoh: $RUN/<строка>/dsh-home (у каждой строки свои)"
  echo "группы:  ${ONLY:-все}; проверки после остановки движка: через ${AFTER//,/ и } с"
  sed -n '/^# Rows/,/^#$/p' "$0" | sed 's/^# \{0,1\}//'
  exit 0
fi
if [ -z "$CLAUDE" ] || [ ! -x "$CLAUDE" ]; then
  echo "НЕ ЗАПУСКАЛОСЬ: CLI claude не найден (--claude PATH)" >&2; exit 2
fi
[ -x "$HOH" ] || { echo "НЕ ЗАПУСКАЛОСЬ: нет собранного hoh: $HOH (--hoh PATH)" >&2; exit 2; }
mkdir -p "$RUN" || exit 2

exec python3 -I "$(dirname "$0")/claude-live-extra.py" --hoh "$HOH" --claude "$CLAUDE" --dir "$RUN" \
  --timeout "$TIMEOUT" --home "$HOME_MODE" --only "$ONLY" --check-after "$AFTER"
