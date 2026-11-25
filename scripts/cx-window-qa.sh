#!/usr/bin/env bash
# Fixture-backed HOH-18 window harness. It deliberately requires an explicit
# deterministic UI driver: this repository has no stable DOM/accessibility test
# hook for the WebKit window, so coordinate-only automation must not be reported
# as proof of the requested UI states.
set -euo pipefail

QA_ROOT=/mnt/toshiba/alexandrie/hoh/qa
DEFAULT_TARGET=/home/save/.cache/hoh-build/line-codexw/debug
DESKTOP="$DEFAULT_TARGET/hoh-desktop"
ENGINE="$DEFAULT_TARGET/hoh"
CODEX_STUB="$DEFAULT_TARGET/codex-stub"
OUTPUT="$QA_ROOT/codexw-fixture-$(date +%Y%m%d-%H%M%S)"
DISPLAY_REQUEST=auto
DRIVER=
WIDTH=1440
HEIGHT=900

usage() {
  cat <<'EOF'
Usage: scripts/cx-window-qa.sh [options]

Launch an already-built HOH desktop and engine under a private 1440x900 Xvfb,
with a fixture executable standing in for `codex app-server`. No real nested
Codex process or login is needed. This script does not build or install anything.

Options:
  --desktop PATH     built hoh-desktop binary
  --engine PATH      built hoh engine binary
  --codex-stub PATH  built hoh-harness-codex `codex-stub` fixture executable
  --driver PATH      deterministic UI driver (required; contract below)
  --output DIR       new isolated QA directory
  --display :N       free display :110 through :119
  --display auto     choose the first free display in that range (default)
  -h, --help         show this help

Driver contract:
  The executable is run after the window is visible, on the same Xvfb display.
  It receives HOH_QA_DISPLAY, HOH_QA_OUTPUT, HOH_QA_SHOTS, HOH_QA_WINDOW_ID,
  HOH_QA_WIDTH=1440, HOH_QA_HEIGHT=900, and HOH_QA_PROMPT. It must drive the
  actual desktop and create exactly these root-window screenshots:

    01-composer-codex.png  Codex selected in the composer
    02-turn-steps.png      an active/completed turn with streamed steps
    03-approval-card.png   the Codex command approval card before answering it
    04-home-codex.png      home/history showing a Codex agent badge

  Each PNG must be 1440x900. The driver must exit nonzero if it cannot prove a
  requested state. `import -window root "$HOH_QA_SHOTS/name.png"` is sufficient
  for capture after a state assertion.

Why --driver is required:
  The current WebKit desktop exposes no stable selector/accessibility fixture
  API. Hard-coded clicks can capture the wrong screen while still producing a
  valid PNG. This harness therefore fails clearly instead of calling such an
  image QA evidence. Add a test-only, state-queryable UI hook or pass a reviewed
  environment-specific driver; do not weaken app-server permissions to automate
  the flow.
EOF
}

while (($#)); do
  case "$1" in
    --desktop) [[ $# -ge 2 ]] || { echo "--desktop requires PATH" >&2; exit 2; }; DESKTOP=$2; shift 2 ;;
    --engine) [[ $# -ge 2 ]] || { echo "--engine requires PATH" >&2; exit 2; }; ENGINE=$2; shift 2 ;;
    --codex-stub) [[ $# -ge 2 ]] || { echo "--codex-stub requires PATH" >&2; exit 2; }; CODEX_STUB=$2; shift 2 ;;
    --driver) [[ $# -ge 2 ]] || { echo "--driver requires PATH" >&2; exit 2; }; DRIVER=$2; shift 2 ;;
    --output) [[ $# -ge 2 ]] || { echo "--output requires DIR" >&2; exit 2; }; OUTPUT=$2; shift 2 ;;
    --display) [[ $# -ge 2 ]] || { echo "--display requires :N or auto" >&2; exit 2; }; DISPLAY_REQUEST=$2; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ $OUTPUT = /* && $OUTPUT != *'/../'* && $OUTPUT != */.. ]] || {
  echo "--output must be an absolute path without '..': $OUTPUT" >&2; exit 2;
}
case "$OUTPUT" in
  /|/mnt/toshiba/alexandrie/hoh|/mnt/toshiba/alexandrie/hoh/pilot-founder*|/mnt/toshiba/alexandrie/hoh/release|/mnt/toshiba/alexandrie/hoh/release/*)
    echo "refusing unsafe or reserved output directory: $OUTPUT" >&2; exit 2 ;;
esac
[[ ! -L $OUTPUT ]] || { echo "refusing symlink output directory: $OUTPUT" >&2; exit 2; }
if [[ -e $OUTPUT && -n $(find "$OUTPUT" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null) ]]; then
  echo "fixture QA output must be new or empty: $OUTPUT" >&2
  exit 2
fi
mkdir -p "$OUTPUT"/{agents,cache,config,data,fakebin,home/Documents/HoH,logs,runtime,shots,state}
chmod 700 "$OUTPUT/runtime"

REPORT="$OUTPUT/REPORT.md"
write_report() {
  local result=$1 detail=$2
  cat >"$REPORT" <<EOF
# HOH-18 Codex window QA

- Result: **$result**
- Evidence class: **fixture**, not live Codex
- Detail: $detail
- Display: ${QA_DISPLAY:-not started}, 1440x900
- Desktop: $DESKTOP
- Engine: $ENGINE
- Codex endpoint: local codex-stub fixture ($CODEX_STUB)
- Driver: ${DRIVER:-not supplied}
- Output: $OUTPUT

The fixture exercises the production codex app-server transport through the
HoH engine, but it does not prove native Codex authentication, a real nested
Codex process, or real sandbox enforcement. Those remain live-lead checks.

Expected fixture screenshots:

- shots/01-composer-codex.png
- shots/02-turn-steps.png
- shots/03-approval-card.png
- shots/04-home-codex.png

Logs are under logs/; owned process identifiers are under state/.
EOF
}
fail() {
  local message=$1 code=${2:-3}
  write_report FAIL "$message"
  echo "$message" >&2
  echo "report: $REPORT" >&2
  exit "$code"
}

[[ -n $DRIVER ]] || fail "no deterministic UI driver supplied; see --help for the required state-queryable driver contract" 4
[[ $DRIVER = /* ]] || fail "UI driver path must be absolute: $DRIVER" 2
case "$DRIVER" in
  /mnt/toshiba/alexandrie/hoh/pilot-founder*|/mnt/toshiba/alexandrie/hoh/release|/mnt/toshiba/alexandrie/hoh/release/*)
    fail "refusing reserved UI driver path: $DRIVER" 2 ;;
esac
[[ -x $DRIVER ]] || fail "UI driver is not executable: $DRIVER" 2
for binary in "$DESKTOP" "$ENGINE" "$CODEX_STUB"; do
  [[ $binary = /* ]] || fail "binary paths must be absolute: $binary" 2
  case "$binary" in
    /mnt/toshiba/alexandrie/hoh/pilot-founder*|/mnt/toshiba/alexandrie/hoh/release|/mnt/toshiba/alexandrie/hoh/release/*)
      fail "refusing reserved binary path: $binary" 2 ;;
  esac
done
[[ -x $DESKTOP ]] || fail "desktop is not executable: $DESKTOP" 2
[[ -x $ENGINE ]] || fail "engine is not executable: $ENGINE" 2
[[ -x $CODEX_STUB ]] || fail "codex fixture is not executable: $CODEX_STUB (build it separately with feature stub)" 2
for command in Xvfb dbus-run-session setsid xdotool identify; do
  command -v "$command" >/dev/null || fail "required preinstalled command is missing: $command" 2
done

choose_display() {
  local candidate number
  if [[ $DISPLAY_REQUEST == auto ]]; then
    for number in {110..119}; do
      candidate=:$number
      [[ ! -S /tmp/.X11-unix/X$number ]] && { printf '%s\n' "$candidate"; return 0; }
    done
    return 1
  fi
  [[ $DISPLAY_REQUEST =~ ^:11[0-9]$ ]] || return 1
  number=${DISPLAY_REQUEST#:}
  [[ ! -S /tmp/.X11-unix/X$number ]] || return 1
  printf '%s\n' "$DISPLAY_REQUEST"
}
QA_DISPLAY=$(choose_display) || fail "no requested display is free in the allowed :110..:119 range" 2

printf 'XDG_DOCUMENTS_DIR="%s"\n' "$OUTPUT/home/Documents" >"$OUTPUT/config/user-dirs.dirs"
mkdir -p "$OUTPUT/config/local.hoh.desktop"
printf '{"folder":"%s"}\n' "$OUTPUT/home/Documents/HoH" >"$OUTPUT/config/local.hoh.desktop/workspace.json"
printf 'fixture workspace for HOH-18 window QA\n' >"$OUTPUT/home/Documents/HoH/README.txt"
cat >"$OUTPUT/fakebin/codex" <<EOF
#!/usr/bin/env bash
exec $(printf '%q' "$CODEX_STUB") "\$@"
EOF
cat >"$OUTPUT/fakebin/xdg-open" <<'EOF'
#!/usr/bin/env bash
printf '%s xdg-open blocked: ' "$(date --iso-8601=seconds)" >>"${HOH_QA_OUTPUT:?}/logs/xdg-open.log"
printf '%q ' "$@" >>"${HOH_QA_OUTPUT:?}/logs/xdg-open.log"
printf '\n' >>"${HOH_QA_OUTPUT:?}/logs/xdg-open.log"
exit 0
EOF
chmod +x "$OUTPUT/fakebin/codex" "$OUTPUT/fakebin/xdg-open"

XVFB_PID=
APP_PID=
cleanup() {
  local status=$?
  trap - EXIT INT TERM
  if [[ -n ${APP_PID:-} ]] && kill -0 "$APP_PID" 2>/dev/null; then
    kill -TERM -- "-$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true
  fi
  if [[ -n ${XVFB_PID:-} ]] && kill -0 "$XVFB_PID" 2>/dev/null; then
    kill -TERM "$XVFB_PID" 2>/dev/null || true
    wait "$XVFB_PID" 2>/dev/null || true
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

Xvfb "$QA_DISPLAY" -screen 0 "${WIDTH}x${HEIGHT}x24" -nolisten tcp -extension GLX >"$OUTPUT/logs/xvfb.log" 2>&1 &
XVFB_PID=$!
printf '%s\n' "$XVFB_PID" >"$OUTPUT/state/xvfb.pid"
for _ in {1..50}; do
  [[ -S /tmp/.X11-unix/X${QA_DISPLAY#:} ]] && break
  kill -0 "$XVFB_PID" 2>/dev/null || fail "Xvfb exited; see logs/xvfb.log"
  sleep 0.1
done
[[ -S /tmp/.X11-unix/X${QA_DISPLAY#:} ]] || fail "Xvfb did not become ready"

setsid dbus-run-session -- env \
  DISPLAY="$QA_DISPLAY" HOME="$OUTPUT/home" \
  XDG_CONFIG_HOME="$OUTPUT/config" XDG_CACHE_HOME="$OUTPUT/cache" \
  XDG_DATA_HOME="$OUTPUT/data" XDG_STATE_HOME="$OUTPUT/state" \
  XDG_RUNTIME_DIR="$OUTPUT/runtime" DSH_AGENTS_HOME="$OUTPUT/agents" \
  HOH_ENGINE_PATH="$ENGINE" HOH_QA_OUTPUT="$OUTPUT" \
  CODEX_STUB_SCENARIO=approvals PATH="$OUTPUT/fakebin:$PATH" \
  GDK_BACKEND=x11 GIO_USE_VFS=local LIBGL_ALWAYS_SOFTWARE=1 \
  WEBKIT_DISABLE_COMPOSITING_MODE=1 \
  "$DESKTOP" >"$OUTPUT/logs/window.stdout.log" 2>"$OUTPUT/logs/window.stderr.log" &
APP_PID=$!
printf '%s\n' "$APP_PID" >"$OUTPUT/state/window.pgid"

WINDOW_ID=
for _ in {1..80}; do
  WINDOW_ID=$(DISPLAY="$QA_DISPLAY" xdotool search --onlyvisible --name '.' 2>/dev/null | head -n 1 || true)
  [[ -n $WINDOW_ID ]] && break
  kill -0 "$APP_PID" 2>/dev/null || fail "desktop exited before a window appeared; see logs/window.stderr.log"
  sleep 0.25
done
[[ -n $WINDOW_ID ]] || fail "desktop window did not appear"
printf '%s\n' "$WINDOW_ID" >"$OUTPUT/state/window.id"

PROMPT='Run pwd, show the command step, ask before execution, then explain the result.'
if ! env \
  DISPLAY="$QA_DISPLAY" HOH_QA_DISPLAY="$QA_DISPLAY" HOH_QA_OUTPUT="$OUTPUT" \
  HOH_QA_SHOTS="$OUTPUT/shots" HOH_QA_WINDOW_ID="$WINDOW_ID" \
  HOH_QA_WIDTH="$WIDTH" HOH_QA_HEIGHT="$HEIGHT" HOH_QA_PROMPT="$PROMPT" \
  "$DRIVER" >"$OUTPUT/logs/driver.stdout.log" 2>"$OUTPUT/logs/driver.stderr.log"; then
  fail "deterministic UI driver failed; see logs/driver.stderr.log"
fi

expected=(
  01-composer-codex.png
  02-turn-steps.png
  03-approval-card.png
  04-home-codex.png
)
for name in "${expected[@]}"; do
  file="$OUTPUT/shots/$name"
  [[ -s $file ]] || fail "driver did not produce required screenshot: shots/$name"
  dimensions=$(identify -format '%wx%h' "$file" 2>>"$OUTPUT/logs/identify.log" || true)
  [[ $dimensions == "${WIDTH}x${HEIGHT}" ]] || fail "wrong dimensions for shots/$name: ${dimensions:-unreadable}, expected ${WIDTH}x${HEIGHT}"
done

write_report PASS "four deterministic 1440x900 fixture screenshots were produced and dimension-checked; inspect them visually before accepting QA"
echo "fixture QA complete: $REPORT"
