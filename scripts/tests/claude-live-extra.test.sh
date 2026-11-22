#!/bin/bash
# Test of scripts/claude-live-extra.sh on a stand-in `claude` (scripts/tests/claude-live-extra/
# fake-claude): no network, no model, no real login, an isolated HOME. Needs a built `hoh`:
# HOH=PATH, or $CARGO_TARGET_DIR/debug/hoh.
set -u -o pipefail
cd "$(dirname "$0")/../.." || exit 2
HOH=${HOH:-${CARGO_TARGET_DIR:-/mnt/toshiba/alexandrie/hoh/target-o1}/debug/hoh}
[ -x "$HOH" ] || { echo "нужен собранный hoh: HOH=PATH ($HOH)" >&2; exit 2; }
FAKE=$PWD/scripts/tests/claude-live-extra/fake-claude
T=$(mktemp -d "${TMPDIR:-/dev/shm/hoh}/claude-live-extra.XXXXXX") || exit 2
trap 'rm -rf "$T"' EXIT
export FAKE_STATE=$T/state
mkdir -p "$FAKE_STATE"
PASS=0 FAILS=0
ok() { PASS=$((PASS + 1)); }
bad() { FAILS=$((FAILS + 1)); echo "FAIL: $1"; }
has() { # $1 name, $2 file, $3 fixed text
  if grep -qF -- "$3" "$2"; then ok; else bad "$1: нет «$3»"; sed 's/^/    | /' "$2" | cut -c1-300; fi
}
lacks() {
  if grep -qF -- "$3" "$2"; then bad "$1: есть «$3»"; else ok; fi
}
run() { # $1 name; the rest: arguments
  local name=$1; shift
  timeout 900 scripts/claude-live-extra.sh --hoh "$HOH" --claude "$FAKE" --dir "$T/out-$name" --home isolated \
    --timeout 60 --check-after 1,2 "$@" >"$T/$name.out" 2>&1
  echo $? >"$T/$name.code"
}

# 1. every row on the stand-in: all PASS, nothing skipped, no secret in the output
run all
has "exit" "$T/all.code" 0
for row in live.stop.confirm live.stop.full live.resume.confirm live.resume.full live.approve.confirm live.approve.full \
           live.question.confirm live.question.full live.v1.mode-switch live.v2.lost-form live.v3.lost-resume \
           live.v5.kill-full live.v5.shutdown500-full live.v6.mcp-project live.v6.mcp-project-enabled \
           live.v7.protected-write live.v7.settings live.v8.read-outside live.v9.secret-url; do
  has "row $row" "$T/all.out" "PASS $row:"
done
# live.v2.cwd follows the i7 contract (a folder change of a Claude chat is refused); the engine of this
# tree has it (an engine that accepts the change, as of cand/i6, makes this row the one FAIL).
has "v2.cwd" "$T/all.out" "PASS live.v2.cwd: смена папки: отклонена с EXECUTOR_FOLDER_FIXED"
# «Стоп» ends the CLI with its command, and the next turn starts it anew (i10b).
has "stop.confirm ends claude" "$T/all.out" "процесс claude после «Стоп»: нет, запущен заново"
lacks "no FAIL" "$T/all.out" "FAIL live."

# 2. a claude that would outlive the end of its stdin does not outlive the engine's KILL: the
# engine's guard ends it (i10b; before, this row was the FAIL «claude пережил остановку движка»)
FAKE_SURVIVES=1 run survives --only v5
has "survives exit" "$T/survives.code" 0
has "survives pass" "$T/survives.out" "PASS live.v5.kill-full:"
lacks "survives words" "$T/survives.out" "claude пережил остановку движка"

# 3. a claude whose interrupt leaves its command running loses it all the same: «Стоп» ends the
# CLI and every process under it (i10b; before, this row was the FAIL «команда sleep осталась: да»)
FAKE_LEAKY_STOP=1 run leaky --only stop
has "leaky exit" "$T/leaky.code" 0
has "leaky pass" "$T/leaky.out" "PASS live.stop.confirm:"
has "leaky words" "$T/leaky.out" "команда sleep осталась: нет"

# 4. usage
scripts/claude-live-extra.sh --only bogus --hoh "$HOH" --claude "$FAKE" --dir "$T/u" >/dev/null 2>&1
if [ $? = 2 ]; then ok; else bad "--only bogus"; fi
scripts/claude-live-extra.sh --dry-run --hoh "$HOH" --claude "$FAKE" --dir "$T/dryout" >"$T/dry.out" 2>&1
has "dry-run" "$T/dry.out" "ПЛАН"
if [ -e "$T/dryout" ]; then bad "dry-run created a folder"; else ok; fi

echo "тесты $PASS/$FAILS"
[ "$FAILS" = 0 ]
