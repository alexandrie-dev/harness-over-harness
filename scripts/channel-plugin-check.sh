#!/bin/bash
# Isolated live check for HOH-47 U4. It never reads or writes the caller's Claude data: HOME,
# CLAUDE_CONFIG_DIR, HoH data, plugins, bot.pid and logs all live below --dir.
#
# The fake channel server reproduces the official channel startup hazard: if it starts, it sends
# SIGTERM to the pid in bot.pid and writes second-server-started. The HoH window launch must pass
# a selective enabledPlugins=false override, leave the old process alive, and never start it.
# A real Claude login is intentionally not copied. If the installed CLI refuses initialization
# before loading plugins without a test login, the argv and process-safety assertions still run;
# use an independently authenticated disposable CLAUDE_CONFIG_DIR to exercise a full answer.
set -euo pipefail

cd "$(dirname "$0")/.." || exit 2

QA_ROOT=/mnt/toshiba/alexandrie/hoh/qa
RUN="$QA_ROOT/cx-q1b-channel-$(date +%Y%m%d-%H%M%S)"
HOH=${HOH:-/home/save/.cache/hoh-build/line-claude/debug/hoh}
CLAUDE=${CLAUDE:-$(command -v claude || true)}
TIMEOUT=45
while [ $# -gt 0 ]; do
  case $1 in
    --dir) RUN=${2:?--dir needs a path}; shift ;;
    --hoh) HOH=${2:?--hoh needs a path}; shift ;;
    --claude) CLAUDE=${2:?--claude needs a path}; shift ;;
    --timeout) TIMEOUT=${2:?--timeout needs seconds}; shift ;;
    -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done
QA_ROOT=$(realpath -e "$QA_ROOT")
RUN_NAME=${RUN#"$QA_ROOT"/}
[[ $RUN == "$QA_ROOT"/* && $RUN_NAME =~ ^cx-q1b-[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || {
  echo "--dir must be a fresh direct $QA_ROOT/cx-q1b-* child" >&2
  exit 2
}
RUN="$QA_ROOT/$RUN_NAME"
[[ $TIMEOUT =~ ^[0-9]+$ ]] || { echo "--timeout needs whole seconds" >&2; exit 2; }
[ -x "$HOH" ] || { echo "HoH binary is not executable: $HOH" >&2; exit 2; }
[ -x "$CLAUDE" ] || { echo "Claude binary is not executable: $CLAUDE" >&2; exit 2; }
[ ! -e "$RUN" ] && [ ! -L "$RUN" ] || { echo "run directory already exists: $RUN" >&2; exit 2; }

CONFIG=$RUN/home/.claude
PROJECT=$RUN/project
CHANNEL=$CONFIG/plugins/cache/test/channel-fixture/1
ORDINARY=$CONFIG/plugins/cache/test/ordinary-fixture/1
BOT=$CONFIG/channels/channel-fixture
DSH_HOME=$RUN/hoh-home
ARGV=$RUN/claude-argv.txt
LOG=$RUN/hoh.log
mkdir -p "$CHANNEL/.claude-plugin" "$ORDINARY/.claude-plugin" "$BOT" "$PROJECT" "$DSH_HOME/profiles/headless"

cat >"$CONFIG/settings.json" <<'JSON'
{"enabledPlugins":{"channel-fixture@test":true,"ordinary-fixture@test":true}}
JSON
cat >"$CONFIG/plugins/installed_plugins.json" <<JSON
{"version":2,"plugins":{"channel-fixture@test":[{"scope":"user","installPath":"$CHANNEL"}],"ordinary-fixture@test":[{"scope":"user","installPath":"$ORDINARY"}]}}
JSON
cat >"$CHANNEL/.claude-plugin/plugin.json" <<JSON
{"name":"channel-fixture","version":"1.0.0","keywords":["fixture"],"mcpServers":{"channel":{"command":"node","args":["$CHANNEL/server.js"]}}}
JSON
cat >"$ORDINARY/.claude-plugin/plugin.json" <<'JSON'
{"name":"ordinary-fixture","version":"1.0.0","keywords":["fixture"]}
JSON
cat >"$CHANNEL/server.ts" <<'TS'
const capabilities = { experimental: { 'claude/channel': {} } };
void capabilities;
TS
cat >"$CHANNEL/server.js" <<JS
const fs = require('fs');
const pidFile = ${BOT@Q} + '/bot.pid';
try { process.kill(Number(fs.readFileSync(pidFile, 'utf8').trim()), 'SIGTERM'); } catch (_) {}
fs.writeFileSync(${RUN@Q} + '/second-server-started', String(process.pid));
process.stdin.resume();
JS
cat >"$RUN/claude-tap" <<'SH'
#!/bin/bash
printf '%s\n' "$@" >"$HOH_CHANNEL_ARGV"
exec "$HOH_CHANNEL_CLAUDE" "$@"
SH
chmod 700 "$RUN/claude-tap"
cat >"$RUN/window.patch.yml" <<EOF
- insert:
    - id: destructive-classifier
      name: 'hoh:destructive-classifier'
    - id: external-harness
      name: 'hoh:external-harness'
    - id: harness-claude
      name: 'hoh:harness-claude'
      config:
        executable: '$RUN/claude-tap'
- id: agent-default-model
  config:
    provider: claude
    model: default
- id: session-title-llm
  disabled: true
- id: session-telemetry-otel
  disabled: true
EOF

(
  trap 'touch "$RUN/old-received-sigterm"; exit 0' TERM
  echo "$BASHPID" >"$BOT/bot.pid"
  while :; do sleep 1; done
) &
OLD_PID=$!
cleanup() {
  if kill -0 "$OLD_PID" 2>/dev/null; then kill "$OLD_PID" 2>/dev/null || true; fi
  wait "$OLD_PID" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

set +e
env -i PATH=/usr/local/bin:/usr/bin:/bin \
  HOME="$RUN/home" CLAUDE_CONFIG_DIR="$CONFIG" DSH_HOME="$DSH_HOME" \
  DSH_AGENTS_HOME="$RUN/agents-home" DSH_TELEMETRY_DISABLED=1 \
  HOH_CHANNEL_ARGV="$ARGV" HOH_CHANNEL_CLAUDE="$CLAUDE" \
  timeout --kill-after=5 "$TIMEOUT" "$HOH" --profile headless --patch "$RUN/window.patch.yml" \
    --json "Reply with OK without using tools." >"$LOG" 2>&1
STATUS=$?
set -e

[ -f "$ARGV" ] || { echo "FAIL: Claude was not launched; see $LOG" >&2; exit 1; }
python3 - "$ARGV" <<'PY'
import json, pathlib, sys
args = pathlib.Path(sys.argv[1]).read_text().splitlines()
try:
    at = args.index("--settings")
    settings = json.loads(args[at + 1])
except (ValueError, IndexError, json.JSONDecodeError) as error:
    raise SystemExit(f"FAIL: no valid --settings overlay in argv: {error}")
expected = {"enabledPlugins": {"channel-fixture@test": False}}
if settings != expected:
    raise SystemExit(f"FAIL: overlay is {settings!r}, expected {expected!r}")
PY
kill -0 "$OLD_PID" 2>/dev/null || { echo "FAIL: old channel process died" >&2; exit 1; }
[ ! -e "$RUN/old-received-sigterm" ] || { echo "FAIL: old channel process received SIGTERM" >&2; exit 1; }
[ ! -e "$RUN/second-server-started" ] || { echo "FAIL: a second channel server started" >&2; exit 1; }

echo "PASS: selective channel override; old pid $OLD_PID alive; no second server (HoH exit $STATUS)"
echo "artifacts: $RUN"
