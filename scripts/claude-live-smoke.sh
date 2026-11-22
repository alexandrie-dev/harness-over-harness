#!/bin/bash
# Short live run of HoH with the Claude package on the real `claude` CLI (HOH-18), in both access
# modes the user chooses in the window (CEO 20.11):
#   confirm) a typical task (read a file, edit one file, run one command) with confirmation: every
#            question the CLI sends reaches the approval service; headless has no window, so
#            nobody answers "allow" and the edit must not happen. The report counts and lists
#            the questions.
#   full)    the same task under full access: no question at all, the edit happens, and the
#            report shows the exact switches the `claude` process ran with.
# Then the personal settings of the CLI under confirm (CEO 21.11), each in both values of the
# settings sources (`t3`: as in T3, only the human's `user` settings apply, CEO 21.11 ~02:00;
# `isolated`: none apply), on the run's own fixtures in the task folder (the human's ~/.claude is
# neither read nor changed):
#   confirm-allow-*)  permissions.allow in .claude/settings.json and .claude/settings.local.json;
#   confirm-mcp-*)    a stub MCP server in .mcp.json whose tool writes a marker file;
#   confirm-hooks-*)  PreToolUse/PostToolUse hooks that write markers and answer "allow".
# And the project's own instructions (CEO 21.11: the founder works with project CLAUDE.md and
# rules), the fact "loaded: yes/no" with its proof, in both values:
#   confirm-project.claude_md-*)        CLAUDE.md asking for the word HOH-MARK-PROJ in every answer;
#   confirm-project.rules-*)            .claude/rules/hoh.md, the same with HOH-MARK-RULES;
#   confirm-project.skills-*)           .claude/skills/hoh-live-skill/SKILL.md: its name in system/init;
#   confirm-project.hooks-*)            SessionStart/UserPromptSubmit hooks that write markers;
#   confirm-project.nested_claude_md-*) sub/CLAUDE.md with HOH-MARK-NESTED, the task reads sub/note.txt.
# The mode comes from the saved window choice next to the engine's profile
# (`external-harness.json`, what `hoh/executor/access/set` writes); nothing else turns full on.
# See docs/claude-live-run.md. Run it only when the lead says so; --dry-run prints the plan and
# runs nothing.
#
# usage: scripts/claude-live-smoke.sh [--dry-run] [--hoh PATH] [--claude PATH] [--build]
#                                     [--timeout SECONDS] [--only confirm|full|personal|project|RUN]
#                                     [--dir PATH]
# exit: 0 every check passed; 1 a check failed; 2 bad usage or missing CLI/binary;
#       3 the hoh binary has no Claude package registered (nothing was run).
set -u -o pipefail
cd "$(dirname "$0")/.." || exit 2

DRY=0 BUILD=0 TIMEOUT=300 ONLY= OUT=/mnt/toshiba/alexandrie/hoh/scratch-live
TARGET_DIR=${CARGO_TARGET_DIR:-/mnt/toshiba/alexandrie/hoh/target-o1}
HOH=$TARGET_DIR/debug/hoh
CLAUDE=$(command -v claude || true)
while [ $# -gt 0 ]; do
  case $1 in
    --dry-run) DRY=1 ;;
    --build) BUILD=1 ;;
    --hoh) HOH=${2:?--hoh needs a path}; shift ;;
    --claude) CLAUDE=${2:?--claude needs a path}; shift ;;
    --timeout) TIMEOUT=${2:?--timeout needs seconds}; shift ;;
    --only) ONLY=${2:?--only needs confirm, full, personal, project or a run}; shift ;;
    --dir) OUT=${2:?--dir needs a path}; shift ;;
    -h|--help) sed -n '2,33p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done
[[ $TIMEOUT =~ ^[0-9]+$ ]] || { echo "--timeout needs whole seconds" >&2; exit 2; }
# The fixtures carry these paths inside JSON and shell lines: plain characters only.
[[ $OUT =~ ^/[A-Za-z0-9_./-]+$ ]] || { echo "--dir needs an absolute path of [A-Za-z0-9_./-]" >&2; exit 2; }
PERSONAL=()
for row in allow mcp hooks; do
  for sources in t3 isolated; do PERSONAL+=("confirm-$row-$sources"); done
done
PROJECT=()
for row in claude_md rules skills hooks nested_claude_md; do
  for sources in t3 isolated; do PROJECT+=("confirm-project.$row-$sources"); done
done
ALL=(confirm full "${PERSONAL[@]}" "${PROJECT[@]}")
case $ONLY in
  '') RUNS=("${ALL[@]}") ;;
  personal) RUNS=("${PERSONAL[@]}") ;;
  project) RUNS=("${PROJECT[@]}") ;;
  *)
    if printf '%s\n' "${ALL[@]}" | grep -qxF -- "$ONLY"; then
      RUNS=("$ONLY")
    else
      echo "--only needs confirm, full, personal, project or one of: ${ALL[*]}" >&2; exit 2
    fi ;;
esac
# A run's name is <mode>[-<row>-<sources>].
parts() { IFS=- read -r MODE ROW SOURCES <<<"$1"; }

STAMP=$(date +%Y%m%d-%H%M%S)
RUN=$OUT/$STAMP
LOGS=$RUN/logs
HOME_DSH=$RUN/dsh-home  # hoh's data (profiles, sessions); the real HOME stays for the CLI
PROFILE_DIR=$HOME_DSH/profiles/headless
CHOICE=$PROFILE_DIR/external-harness.json
PATCHES=$RUN/patches
FIXTURES=$RUN/fixtures
MARKERS=$RUN/markers
PYTHON=$(command -v python3 || echo python3)
TOKEN=HOH-LIVE-$STAMP-$RANDOM
TASK="In the current working directory: 1) read first.txt; 2) append one line with the word edited to notes.txt; 3) run the command: wc -l first.txt. Then reply in one line with the first line of first.txt and the output of the command. If a step is denied, skip it and say which one."
TASK_MCP="Call the MCP tool hoh_live_write of the server hoh-live with the text edited. If the tool is not available or the call is denied, say so. Reply in one line."
TASK_READ="In the current working directory read first.txt and reply in one line with its first line."
TASK_NESTED="In the current working directory read sub/note.txt and reply in one line with its first line."
# Plugin rows the hoh registry must know (apps/hoh/src/registry.rs; see the document).
PLUGINS=(hoh:destructive-classifier hoh:external-harness hoh:harness-claude)

# --- settings sources (s4, `harness-claude` config `settingSources`): the one place that knows
# how the value reaches the Claude package and which switches each value adds. The runs say `t3`
# for the package's `personal` (as in T3: only the human's `user` settings, CEO 21.11 ~02:00); the
# package's default (no value given) is `personal`.
SOURCES_DEFAULT=t3
sources_value() { case $1 in t3) echo personal ;; isolated) echo isolated ;; esac; }
sources_config() { # $1: t3 | isolated | '' (the default); the lines under `harness-claude.config`
  [ -n "$1" ] && printf '        settingSources: %s\n' "$(sources_value "$1")"
}
sources_flags() { # $1: t3 | isolated
  case $1 in
    t3) echo "--setting-sources user" ;;
    isolated) echo "--setting-sources '' --strict-mcp-config" ;;
  esac
}
# The switches hoh-harness-claude passes in each mode (`launch_for`, `session_args`, s4), quoted
# as the shell would; compared as a set with the live ones.
FLAGS_confirm="--permission-mode default --permission-prompt-tool stdio --disallowedTools WebFetch,WebSearch"
FLAGS_full="--permission-mode bypassPermissions --permission-prompt-tool stdio --allow-dangerously-skip-permissions"
expected_flags() { # $1: the run
  parts "$1"
  local flags=FLAGS_$MODE
  echo "${!flags} $(sources_flags "${SOURCES:-$SOURCES_DEFAULT}")"
}
task_of() {
  parts "$1"
  case $ROW in
    mcp) echo "$TASK_MCP" ;;
    project.nested_claude_md) echo "$TASK_NESTED" ;;
    project.*) echo "$TASK_READ" ;;
    *) echo "$TASK" ;;
  esac
}
# The project rows run the CLI through the tap (fixtures/claude-tap): what CLAUDE.md and rules do
# shows in the answer, but the skills the CLI loaded only in its own `system/init`.
executable_of() { parts "$1"; [[ $ROW == project.* ]] && echo "$FIXTURES/claude-tap" || echo "$CLAUDE"; }

# hoh with its data isolated; telemetry and the title model (DeepSeek network) are off.
run_env=(env "DSH_HOME=$HOME_DSH" "DSH_AGENTS_HOME=$RUN/agents-home"
  DSH_TELEMETRY_DISABLED=1 "TMPDIR=${TMPDIR:-/dev/shm/hoh}")
# No --approval-policy: headless has no window, nobody answers, every question ends
# `unavailable` and the slot answers the CLI "deny".
hoh_cmd() { # $1: the run; sets CMD (the task text is appended by the caller)
  CMD=(timeout --kill-after=15 "$TIMEOUT" "$HOH" --profile headless --patch "$PATCHES/$1.yml" --json)
}
shown() { printf '%q ' "$@"; }
choice_text() { printf '{"access": {"claude": "%s"}}\n' "$1"; }

patch_text() { # $1: the run
  local exe
  exe=$(executable_of "$1")
  exe=${exe//\'/\'\'}
  parts "$1"
  cat <<EOF
# Live smoke of the Claude package over the headless profile (scripts/claude-live-smoke.sh).
- insert:
    - id: destructive-classifier
      name: 'hoh:destructive-classifier'
    - id: external-harness
      name: 'hoh:external-harness'
    - id: harness-claude
      name: 'hoh:harness-claude'
      config:
        executable: '$exe'
$(sources_config "$SOURCES")

# Sessions go to the harness named "claude"; its CLI picks its own default model.
- id: agent-default-model
  config:
    provider: claude
    model: default

# No network from hoh itself: the title model and telemetry stay off.
- id: session-title-llm
  disabled: true

- id: session-telemetry-otel
  disabled: true
EOF
}

# --- fixtures of the personal-settings rows: in the task folder (areas `project` and `local` of
# the CLI); the helper programs and the markers stay outside it.
hook_text() {
  cat <<'EOF'
#!/bin/sh
# Hook of the HoH live run: $1 mark|allow, $2 pre|post, $3 the marker folder. The hook's input
# (the tool call) is read and dropped.
cat >/dev/null
case $1 in
  mark) : >"$3/hook-$2" ;;
  allow)
    : >"$3/hook-allow"
    printf '%s\n' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"HoH live-run fixture"}}'
    ;;
esac
exit 0
EOF
}

mcp_text() {
  cat <<'EOF'
# Stub MCP server of the HoH live run (stdio, one JSON-RPC message per line). Its one tool,
# hoh_live_write, only writes a marker; starting, listing and calling each leave their own marker.
import json, os, sys

markers = sys.argv[1]

def mark(name):
    open(os.path.join(markers, name), "a").close()

mark("mcp-started")
for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    method, ident = message.get("method"), message.get("id")
    if ident is None:
        continue
    if method == "initialize":
        version = (message.get("params") or {}).get("protocolVersion", "2025-06-18")
        result = {"protocolVersion": version, "capabilities": {"tools": {}},
                  "serverInfo": {"name": "hoh-live", "version": "1"}}
    elif method == "tools/list":
        mark("mcp-listed")
        result = {"tools": [{"name": "hoh_live_write",
                             "description": "Writes a marker file for the HoH live run.",
                             "inputSchema": {"type": "object",
                                             "properties": {"text": {"type": "string"}}}}]}
    elif method == "tools/call":
        mark("mcp-called")
        result = {"content": [{"type": "text", "text": "written"}]}
    elif method == "ping":
        result = {}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": ident,
                          "error": {"code": -32601, "message": "unknown method"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": ident, "result": result}), flush=True)
EOF
}

tap_text() {
  printf '#!%s -I\n' "$PYTHON"
  cat <<'EOF'
# Tap of the HoH live run: runs the real claude (tap.json next to it) with the same arguments,
# stdin and stderr, passes its stdout through line by line, and copies only the CLI's
# `system/init` lines to <markers>/<the task folder's name>/system-init.jsonl.
import json, os, signal, subprocess, sys

with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "tap.json"), encoding="utf-8") as file:
    config = json.load(file)
child = subprocess.Popen([config["claude"], *sys.argv[1:]], stdout=subprocess.PIPE)
for number in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
    signal.signal(number, lambda received, _frame: child.send_signal(received))
folder = os.path.join(config["markers"], os.path.basename(os.getcwd()))
try:
    for line in iter(child.stdout.readline, b""):
        sys.stdout.buffer.write(line)
        sys.stdout.buffer.flush()
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if isinstance(message, dict) and message.get("type") == "system" and message.get("subtype") == "init":
            os.makedirs(folder, exist_ok=True)
            with open(os.path.join(folder, "system-init.jsonl"), "ab") as file:
                file.write(line)
except BrokenPipeError:
    child.terminate()
code = child.wait()
sys.exit(code if code >= 0 else 128 - code)
EOF
}

fixture_files() { # $1: the run, $2: its task folder; prints "<path>" then its content, per file
  # (a content is one line; `\n` in it stands for a line break)
  parts "$1"
  local work=$2 marks=$MARKERS/$1 hook=$FIXTURES/hook.sh matcher='Bash|Edit|Write|MultiEdit'
  case $ROW in
    allow)
      printf '%s\n' "$work/.claude/settings.json"
      printf '{"permissions": {"allow": ["Bash(wc:*)"]}}\n'
      printf '%s\n' "$work/.claude/settings.local.json"
      printf '{"permissions": {"allow": ["Edit", "Write"]}}\n' ;;
    mcp)
      printf '%s\n' "$work/.mcp.json"
      printf '{"mcpServers": {"hoh-live": {"type": "stdio", "command": "%s", "args": ["-I", "%s", "%s"]}}}\n' \
        "$PYTHON" "$FIXTURES/mcp_server.py" "$marks"
      printf '%s\n' "$work/.claude/settings.local.json"
      printf '{"enableAllProjectMcpServers": true, "enabledMcpjsonServers": ["hoh-live"]}\n' ;;
    hooks)
      printf '%s\n' "$work/.claude/settings.json"
      printf '{"hooks": {"PreToolUse": [{"matcher": "%s", "hooks": [{"type": "command", "command": "%s mark pre %s"}, {"type": "command", "command": "%s allow pre %s"}]}], "PostToolUse": [{"matcher": "%s", "hooks": [{"type": "command", "command": "%s mark post %s"}]}]}}\n' \
        "$matcher" "$hook" "$marks" "$hook" "$marks" "$matcher" "$hook" "$marks" ;;
    project.claude_md)
      printf '%s\n' "$work/CLAUDE.md"
      printf '%s\n' 'In every answer, add the word HOH-MARK-PROJ at the end of your reply.' ;;
    project.rules)
      printf '%s\n' "$work/.claude/rules/hoh.md"
      printf '%s\n' 'In every answer, add the word HOH-MARK-RULES at the end of your reply.' ;;
    project.skills)
      printf '%s\n' "$work/.claude/skills/hoh-live-skill/SKILL.md"
      printf '%s\n' '---\nname: hoh-live-skill\ndescription: Fixture of the HoH live run; never needed for any task.\n---\nThis skill does nothing.' ;;
    project.hooks)
      printf '%s\n' "$work/.claude/settings.json"
      printf '{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "%s mark start %s"}]}], "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "%s mark prompt %s"}]}]}}\n' \
        "$hook" "$marks" "$hook" "$marks" ;;
    project.nested_claude_md)
      printf '%s\n' "$work/sub/CLAUDE.md"
      printf '%s\n' 'In every answer, add the word HOH-MARK-NESTED at the end of your reply.'
      printf '%s\n' "$work/sub/note.txt"
      printf 'SUB-%s\n' "$TOKEN" ;;
  esac
}

write_fixtures() { # $1: the run, $2: its task folder
  local path
  while IFS= read -r path && IFS= read -r content; do
    mkdir -p "$(dirname "$path")" && printf '%s\n' "${content//\\n/$'\n'}" >"$path"
  done < <(fixture_files "$1" "$2")
}

registered() { # every plugin row name is compiled into the binary
  local name
  for name in "${PLUGINS[@]}"; do grep -a -q -F "$name" "$HOH" || return 1; done
}

if [ $DRY = 1 ]; then
  echo "ПЛАН (dry-run: ничего не создаётся и не запускается)"
  echo "hoh:     $HOH $( [ -x "$HOH" ] && echo '[есть]' || echo '[нет: собрать с --build]')"
  if [ -x "$HOH" ]; then
    registered && echo "         пакет Claude зарегистрирован" \
      || echo "         пакета Claude в реестре нет: прогон завершится кодом 3"
  fi
  echo "claude:  ${CLAUDE:-не найден в PATH} (проверка: --version в 2.0.0–2.x)"
  echo "папка:   $RUN"
  echo "данные:  DSH_HOME=$HOME_DSH, DSH_AGENTS_HOME=$RUN/agents-home; HOME не меняется"
  echo "managed: /etc/claude-code/managed-settings.json $( [ -e /etc/claude-code/managed-settings.json ] && echo есть || echo нет) (только наличие)"
  [ $BUILD = 1 ] && echo "сборка:  timeout 1800 cargo build --offline -p hoh"
  echo "наложение профиля $PATCHES/confirm.yml (остальные отличаются только строкой источников настроек):"
  patch_text confirm | sed 's/^/  | /'
  for run in "${RUNS[@]}"; do
    parts "$run"
    hoh_cmd "$run"
    echo "прогон $run: cwd $RUN/$run, first.txt (первая строка $TOKEN), notes.txt"
    echo "  выбор окна: $CHOICE = $(choice_text "$MODE")"
    echo "  источники настроек: ${SOURCES:-не заданы, умолчание пакета $(sources_value "$SOURCES_DEFAULT") (как в T3)}$(sources_config "$SOURCES" | sed 's/^ */; в наложении: /')"
    echo "  ожидаемые флаги claude: $(expected_flags "$run")"
    [[ $ROW == project.* ]] && echo "  исполняемый: $(executable_of "$run") (прокладка: всё передаёт $CLAUDE, копирует system/init в $MARKERS/$run/system-init.jsonl)"
    if [ -n "$ROW" ]; then
      echo "  фикстуры:"
      fixture_files "$run" "$RUN/$run" | sed 's/^/    | /'
    fi
    echo "  $(shown "${run_env[@]}" "${CMD[@]}" "$(task_of "$run")")"
  done
  echo "проверки: version; для каждого прогона <run>.bound, <run>.turn, <run>.flags;"
  echo "          confirm.asked, confirm.denied, confirm.unchanged; full.questions (0), full.edited;"
  echo "          confirm.personal.<allow|mcp|hooks>.isolated (PASS/FAIL) и .t3 (факт: да/нет);"
  echo "          confirm.project.<claude_md|rules|skills|hooks|nested_claude_md>.<t3|isolated>"
  echo "          (факт: загружено да/нет и доказательство) и их таблица;"
  echo "          managed (наличие); модель по ответу CLI; secrets; leftovers (предупреждение)"
  echo "отчёт:   $RUN/report.txt; код 0 — всё прошло, 1 — проверка не прошла, 2/3 — не запускалось"
  exit 0
fi

fail_early() { echo "НЕ ЗАПУСКАЛОСЬ: $1" >&2; exit "${2:-2}"; }
if [ $BUILD = 1 ]; then
  timeout 1800 cargo build --offline -p hoh || fail_early "сборка hoh не прошла"
fi
[ -x "$HOH" ] || fail_early "нет собранного hoh: $HOH (--build или --hoh)"
registered || fail_early "в $HOH нет плагинов ${PLUGINS[*]}: собран старый hoh? (docs/claude-live-run.md)" 3
if [ -z "$CLAUDE" ] || [ ! -x "$CLAUDE" ]; then fail_early "CLI claude не найден (--claude PATH)"; fi
VERSION_LINE=$("$CLAUDE" --version 2>/dev/null | head -1)
if [[ $VERSION_LINE =~ ([0-9]+)\.([0-9]+)\.([0-9]+) ]] && [ "${BASH_REMATCH[1]}" = 2 ]; then
  VERSION_OK=1
else
  VERSION_OK=0
fi

mkdir -p "$LOGS" "$PROFILE_DIR" "$RUN/agents-home" "$PATCHES" "$FIXTURES" || fail_early "нет папки $RUN"
hook_text >"$FIXTURES/hook.sh" && chmod 755 "$FIXTURES/hook.sh"
mcp_text >"$FIXTURES/mcp_server.py"
tap_text >"$FIXTURES/claude-tap" && chmod 755 "$FIXTURES/claude-tap"
"$PYTHON" -I -c 'import json, sys; print(json.dumps({"claude": sys.argv[1], "markers": sys.argv[2]}))' \
  "$CLAUDE" "$MARKERS" >"$FIXTURES/tap.json"

# The `claude` session process of this run (cwd inside the run folder): its command line, once
# seen. Its PID is not ours to stop; we only read /proc.
watch_argv() { # $1: the hoh pid, $2: the file to write
  local pid=$1 out=$2 proc cwd args words
  while kill -0 "$pid" 2>/dev/null; do
    for proc in /proc/[0-9]*; do
      cwd=$(readlink "$proc/cwd" 2>/dev/null) || continue
      [[ $cwd == "$RUN"* ]] || continue
      args=$(tr '\0' ' ' <"$proc/cmdline" 2>/dev/null) || continue
      if [[ $args == *--permission-mode* && $args == *--session-id* || $args == *--permission-mode*--resume* ]]; then
        # Quoted word by word: the empty value of `--setting-sources` must survive.
        mapfile -d '' -t words <"$proc/cmdline" 2>/dev/null || continue
        printf '%q ' "${words[@]}" >"$out"
        echo >>"$out"
        return 0
      fi
    done
    sleep 0.2
  done
}

run_task() { # $1: the run
  local run=$1 work=$RUN/$1 pid
  parts "$run"
  mkdir -p "$work" "$MARKERS/$run"
  printf '%s\nsecond line\n' "$TOKEN" >"$work/first.txt"
  printf 'notes\n' >"$work/notes.txt"
  write_fixtures "$run" "$work"
  patch_text "$run" >"$PATCHES/$run.yml"
  # The user's choice in the window, as `hoh/executor/access/set` keeps it.
  choice_text "$MODE" >"$CHOICE"
  hoh_cmd "$run"
  shown "${CMD[@]}" "$(task_of "$run")" >"$LOGS/$run.cmd"
  (cd "$work" && exec "${run_env[@]}" "${CMD[@]}" "$(task_of "$run")") >"$LOGS/$run.out" 2>"$LOGS/$run.err" &
  pid=$!
  watch_argv "$pid" "$LOGS/$run.argv"
  wait "$pid"
  echo $? >"$LOGS/$run.code"
}
if [ "$VERSION_OK" = 1 ]; then
  for run in "${RUNS[@]}"; do run_task "$run"; done
fi

# Our own leftovers only: processes whose cwd is inside this run's folder.
LEFT=()
for proc in /proc/[0-9]*; do
  cwd=$(readlink "$proc/cwd" 2>/dev/null) || continue
  [[ $cwd == "$RUN"* ]] && LEFT+=("${proc#/proc/}")
done
[ ${#LEFT[@]} -gt 0 ] && kill -TERM "${LEFT[@]}" 2>/dev/null

EXPECTED=()
for run in "${RUNS[@]}"; do EXPECTED+=("$run" "$(expected_flags "$run")"); done
python3 -I - "$RUN" "$TOKEN" "$VERSION_OK" "$VERSION_LINE" "${#LEFT[@]}" "${EXPECTED[@]}" \
  <<'PY' | tee "$RUN/report.txt"
import json, os, re, shlex, subprocess, sys

run, token, version_ok, version_line, left = sys.argv[1:6]
pairs = sys.argv[6:]
expected_flags = {pairs[i]: shlex.split(pairs[i + 1]) for i in range(0, len(pairs), 2)}
runs = list(expected_flags)
logs, sessions, markers = (os.path.join(run, name) for name in ("logs", os.path.join("dsh-home", "sessions"), "markers"))
results, notes = [], []
project = {}  # (row, sources) -> loaded: да/нет

def check(name, ok, detail, warn=False):
    results.append((("WARN" if warn else "FAIL") if not ok else "PASS", name, detail))

def fact(name, detail):
    results.append(("INFO", name, detail))

def yes(value):
    return "да" if value else "нет"

def json_lines(text):
    for line in text.splitlines():
        try:
            value = json.loads(line)
        except ValueError:
            continue
        if isinstance(value, dict):
            yield value

def read(path):
    try:
        if path.endswith(".zstd"):
            return subprocess.run(["zstd", "-dcq", path], capture_output=True, check=True).stdout.decode("utf-8", "replace")
        with open(path, encoding="utf-8", errors="replace") as file:
            return file.read()
    except (OSError, subprocess.CalledProcessError):
        return ""

session_texts = {}
for base, _, files in os.walk(sessions):
    for name in files:
        if name.endswith((".jsonl", ".jsonl.zstd")):
            path = os.path.join(base, name)
            session_texts[path] = read(path)

def run_of(name):
    out = list(json_lines(read(os.path.join(logs, name + ".out"))))
    sid = next((e.get("sessionId") for e in out if e.get("type") == "session"), None)
    final = next((e for e in out if e.get("type") == "final"), {})
    events = []
    for path, text in session_texts.items():
        if sid and (sid in path or sid in text):
            for record in json_lines(text):
                data = record.get("data") if isinstance(record.get("data"), dict) else record
                events.append((record.get("type"), data))
    code = read(os.path.join(logs, name + ".code")).strip() or "не запускалось"
    return sid, final, events, code

def of_type(events, kind):
    return [data for event_type, data in events if event_type == kind]

def switches(words):
    """The permission, tool and settings switches of a `claude` command line, each with its value."""
    with_value = {"--permission-mode", "--setting-sources", "--permission-prompt-tool", "--disallowedTools", "--tools", "--allowedTools"}
    alone = {"--allow-dangerously-skip-permissions", "--dangerously-skip-permissions", "--strict-mcp-config", "--safe-mode"}
    picked, i = [], 0
    while i < len(words):
        if words[i] in with_value and i + 1 < len(words):
            picked.append(tuple(words[i:i + 2]))
            i += 2
            continue
        if words[i] in alone:
            picked.append((words[i],))
        i += 1
    return sorted(picked)

def shown(groups):
    return shlex.join([word for group in groups for word in group])

def steps(events):
    """Calls the session recorded, and those that ended without an error (they ran)."""
    calls = {d.get("callId"): d for d in of_type(events, "tool/call")}
    ran = [calls[cid] for cid in ((d.get("message") or {}).get("toolCallId") for d in of_type(events, "tool/result") if not d.get("error")) if cid in calls]
    return list(calls.values()), ran

def marked(name, marker):
    return os.path.exists(os.path.join(markers, name, marker))

def system_init(name):
    """The last `system/init` of the run's CLI, as the tap copied it (None: not caught)."""
    found = list(json_lines(read(os.path.join(markers, name, "system-init.jsonl"))))
    return found[-1] if found else None

def setting_sources(name):
    words = expected_flags[name]
    at = words.index("--setting-sources")
    return shlex.join(words[at:at + 2])

MARKS = {"claude_md": "HOH-MARK-PROJ", "rules": "HOH-MARK-RULES", "nested_claude_md": "HOH-MARK-NESTED"}

check("version", version_ok == "1", f"claude --version: «{version_line.strip()}», нужна 2.0.0–2.x")
managed = [path for path in ("/etc/claude-code/managed-settings.json", "/etc/claude-code/managed-mcp.json") if os.path.exists(path)]
fact("managed", f"управляемые настройки CLI: {', '.join(managed) if managed else 'нет'} (только наличие, содержимое не читалось)")

for name in runs:
    mode, _, rest = name.partition("-")
    row, _, sources = rest.partition("-")
    sid, final, events, code = run_of(name)
    bound = [d for d in of_type(events, "plugin:external-harness/session") if d.get("harness") == "claude"]
    check(f"{name}.bound", bool(bound), f"сессия {sid or 'нет'} {'идёт через исполнителя claude' if bound else 'не привязана к claude'}")
    reason = final.get("reason") or {}
    kind = reason.get("kind") if isinstance(reason, dict) else None
    error = (reason.get("error") or {}).get("code") if isinstance(reason, dict) else None
    check(f"{name}.turn", kind == "completed" and code == "0", f"ход: {kind or 'нет'}{' ' + error if error else ''}, код hoh {code}")
    argv = read(os.path.join(logs, name + ".argv")).strip()
    live = switches(shlex.split(argv))
    want = switches(expected_flags[name])
    check(f"{name}.flags", live == want, f"флаги claude: {shown(live) or 'процесс не пойман'} (ждали {shown(want)})")
    asked = of_type(events, "approval/asked")
    asked_tools = [a.get("toolName") or "" for a in asked]
    listed = [f"{a.get('toolName')}: {((a.get('displayReason') or {}).get('en') if isinstance(a.get('displayReason'), dict) else None) or a.get('reason') or ''}".strip()[:160] for a in asked]
    notes_text = read(os.path.join(run, name, "notes.txt"))
    edited = "edited" in notes_text
    calls, ran = steps(events)
    command_ran = any(c.get("name") == "bash" and "wc" in str(c.get("arguments")) for c in ran)
    command_asked, edit_asked = "bash" in asked_tools, "edit_file" in asked_tools
    notes.append(f"в {name} вопросов {len(asked)}" + (":" if asked else ""))
    notes += [f"  {n + 1}. {line}" for n, line in enumerate(listed)]
    if name == "confirm":
        check("confirm.asked", len(asked) >= 2, f"вопросов: {len(asked)} (ждали ≥ 2: правка и команда)")
        outcomes = [d.get("outcome") for d in of_type(events, "approval/decided")]
        check("confirm.denied", bool(outcomes) and all(o != "allowed-once" for o in outcomes), f"ответы: {outcomes or 'нет'}")
        check("confirm.unchanged", not edited, "notes.txt не изменён" if not edited else "notes.txt изменён без разрешения")
    elif name == "full":
        check("full.questions", not asked, f"вопросов: {len(asked)}")
        check("full.edited", edited, "notes.txt изменён" if edited else "notes.txt не изменён")
        notes.append(f"флаги CLI в full: {shown(live) or 'процесс не пойман'}")
        notes.append(f"командная строка claude в full: {argv or 'нет'}")
    elif row.startswith("project."):
        what = row.partition(".")[2]
        said = final.get("text") if isinstance(final.get("text"), str) else ""
        if what in MARKS:
            loaded = MARKS[what] in said
            proof = f"метка {MARKS[what]} в финальном тексте: {yes(loaded)}"
            if what == "nested_claude_md":
                proof += f"; sub/note.txt прочитан (его строка SUB-… в ответе): {yes(('SUB-' + token) in said)}"
            proof += f"; финальный текст: «{' '.join(said.split())[:200] or 'нет'}»"
        elif what == "skills":
            init = system_init(name)
            if init is None:
                loaded, proof = False, "system/init не пойман (нет markers/<прогон>/system-init.jsonl)"
            else:
                skills = init.get("skills")
                names = [s.get("name") if isinstance(s, dict) else s for s in skills or []]
                in_skills = "hoh-live-skill" in names
                in_commands = "hoh-live-skill" in (init.get("slash_commands") or [])
                loaded = in_skills or in_commands
                listed = "поля нет" if skills is None else f"имён {len(names)}, hoh-live-skill среди них: {yes(in_skills)}"
                proof = f"system/init.skills: {listed}; system/init.slash_commands: hoh-live-skill {yes(in_commands)}"
        else:
            start, prompt = marked(name, "hook-start"), marked(name, "hook-prompt")
            loaded = start or prompt
            proof = f"файлы-метки хуков из .claude/settings.json: SessionStart {yes(start)}, UserPromptSubmit {yes(prompt)}"
        project[(row, sources)] = yes(loaded)
        fact(f"confirm.{row}.{sources}", f"загружено: {yes(loaded)}; {proof}")
    else:
        line = f"confirm.personal.{row}.{sources}"
        if row == "allow":
            silent = (command_ran and not command_asked) or (edited and not edit_asked)
            what = (f"команда wc (правило Bash(wc:*) в settings.json): выполнена {yes(command_ran)}, спрошена {yes(command_asked)}; "
                    f"правка notes.txt (Edit, Write в settings.local.json): сделана {yes(edited)}, спрошена {yes(edit_asked)}")
            ok = command_asked and edit_asked and not edited and not command_ran
        elif row == "mcp":
            started, listed_tools, called = (marked(name, m) for m in ("mcp-started", "mcp-listed", "mcp-called"))
            mcp_asked = any(tool.startswith("mcp__") for tool in asked_tools)
            silent = called and not mcp_asked
            what = (f"сервер из .mcp.json запущен CLI {yes(started)}, список инструментов запрошен {yes(listed_tools)}, "
                    f"вызов hoh_live_write спрошен окном {yes(mcp_asked)}, метка вызова {'есть' if called else 'нет'}")
            ok = not called and (not listed_tools or mcp_asked)
        else:
            pre, post, allow = (marked(name, m) for m in ("hook-pre", "hook-post", "hook-allow"))
            silent = (edited and not edit_asked) or (command_ran and not command_asked)
            what = (f"хуки: PreToolUse-метка {yes(pre)}, PostToolUse-метка {yes(post)}, «allow» на PreToolUse отработал {yes(allow)}; "
                    f"вопросов окна {len(asked)}; notes.txt изменён {yes(edited)}, команда выполнена {yes(command_ran)}")
            ok = not (pre or post or allow) and bool(asked) and not edited
        if sources == "t3":
            fact(line, f"действие выполнено без вопроса окна: {yes(silent)}; {what}")
        else:
            check(line, ok, what)

# The model the CLI actually ran with comes in its `system/init`, which only the Claude package
# reads; the slot's contract has no field for it yet (s4, 21.11: `HarnessEvent::ModelReported`).
# The session log therefore has none, and the CLI's own transcript under ~/.claude is not read.
notes.append("модель по ответу CLI (confirm и full): не передаётся — в контракте гнезда нет поля для модели "
             "из system/init (s4 предложил HarnessEvent::ModelReported)")

secret = re.compile(
    r"sk-ant-[A-Za-z0-9_-]{8,}|sk-[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9]{20,}|github_pat_\w{20,}"
    r"|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{30,}|xox[abprs]-[A-Za-z0-9-]{10,}"
    r"|eyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}|-----BEGIN [A-Z ]*PRIVATE KEY-----"
    r"|(?i:bearer)\s+[A-Za-z0-9._~+/=-]{20,}"
    r"|\"(?:accessToken|refreshToken|oauthToken|apiKey)\"\s*:\s*\"(?!\[redacted\])[^\"]{8,}\"")
texts = dict(session_texts)
for name in os.listdir(logs):
    texts[os.path.join(logs, name)] = read(os.path.join(logs, name))
found = [f"{os.path.relpath(path, run)}:{text.count(chr(10), 0, m.start()) + 1}"
         for path, text in texts.items() for m in secret.finditer(text)]
check("secrets", not found, f"совпадений: {len(found)} {' '.join(found[:5])}".strip())
check("leftovers", left == "0", f"процессов прогона после конца: {left} (остановлены)", warn=True)

if project:
    first = {s: next(n for n in runs if n.startswith("confirm-project.") and n.endswith("-" + s)) for _, s in project}
    notes.append("проектные настройки в confirm, загружено: "
                 + "; ".join(f"{s} — {setting_sources(n)}" for s, n in sorted(first.items(), reverse=True)))
    notes.append("| строка | t3 | isolated |")
    notes.append("|---|---|---|")
    for row in dict.fromkeys(r for r, _ in project):
        notes.append(f"| {row} | {project.get((row, 't3'), '—')} | {project.get((row, 'isolated'), '—')} |")

for status, name, detail in results:
    print(f"{status} {name}: {detail}")
for line in notes:
    print(line)
failed = [name for status, name, _ in results if status == "FAIL"]
print(f"ИТОГ: {'не прошло: ' + ', '.join(failed) if failed else 'все проверки прошли'} ({run})")
sys.exit(1 if failed else 0)
PY
exit "${PIPESTATUS[0]}"
