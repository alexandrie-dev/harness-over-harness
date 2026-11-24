#!/usr/bin/env bash
# Run the already-built HOH-18 desktop against the user's real Codex login while
# keeping every HoH-owned file in an isolated directory. This script never
# builds, installs, or starts a network service.
set -euo pipefail

QA_ROOT=/mnt/toshiba/alexandrie/hoh/qa
DEFAULT_TARGET=/home/save/.cache/hoh-build/line-codexw/debug
DESKTOP="$DEFAULT_TARGET/hoh-desktop"
ENGINE="$DEFAULT_TARGET/hoh"
OUTPUT="$QA_ROOT/codexw-live-$(date +%Y%m%d-%H%M%S)"
DISPLAY_REQUEST=:110
REUSE=0
SINGLE_RUN=0

usage() {
  cat <<'EOF'
Usage: scripts/cx-demo-window.sh [options]

Lead-run HOH-18 live-window helper. It uses the real `codex` from PATH and the
current user's native login (HOME and the user's original XDG values), but puts
HoH settings, engine data, webview data, agents, workspace, logs, and runtime
files below one isolated output directory.

Options:
  --desktop PATH   built hoh-desktop binary
  --engine PATH    built hoh engine binary
  --output DIR     isolated run directory (default: qa/codexw-live-TIMESTAMP)
  --display :N     Xvfb display :110 through :119 (default: :110)
  --display auto   choose the first free display in :110 through :119
  --reuse          reuse an existing run directory (for a later manual rerun)
  --single-run     do not automatically restart after the first window closes
  -h, --help       show this help

Manual scenario (first window):
  1. If first-run setup asks for a project, choose the printed isolated workspace.
     Create a new thread and select Codex and one of its installed models.
  2. Send a message that asks Codex to run a harmless command in the displayed
     workspace, for example: `Run pwd, then explain the result.`
  3. Confirm that the command approval appears in the chat and click Approve.
  4. Wait for the streamed answer and visible execution steps.
  5. Send a longer command request and press Stop while it is running.
  6. Close the first window. The helper restarts it with the same HoH data.
  7. Confirm the conversation survived the restart, then close the window.

The display is private Xvfb. Drive or inspect it from another shell with
DISPLAY=:N (for example, xdotool/import/VNC chosen by the lead). Paths to all
logs are printed before launch. No screenshot or UI assertion is automated by
this live helper.
EOF
}

while (($#)); do
  case "$1" in
    --desktop) [[ $# -ge 2 ]] || { echo "--desktop requires PATH" >&2; exit 2; }; DESKTOP=$2; shift 2 ;;
    --engine) [[ $# -ge 2 ]] || { echo "--engine requires PATH" >&2; exit 2; }; ENGINE=$2; shift 2 ;;
    --output) [[ $# -ge 2 ]] || { echo "--output requires DIR" >&2; exit 2; }; OUTPUT=$2; shift 2 ;;
    --display) [[ $# -ge 2 ]] || { echo "--display requires :N or auto" >&2; exit 2; }; DISPLAY_REQUEST=$2; shift 2 ;;
    --reuse) REUSE=1; shift ;;
    --single-run) SINGLE_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

for binary in "$DESKTOP" "$ENGINE"; do
  [[ $binary = /* ]] || { echo "binary paths must be absolute: $binary" >&2; exit 2; }
  case "$binary" in
    /mnt/toshiba/alexandrie/hoh/pilot-founder*|/mnt/toshiba/alexandrie/hoh/release|/mnt/toshiba/alexandrie/hoh/release/*)
      echo "refusing reserved binary path: $binary" >&2; exit 2 ;;
  esac
done
[[ -x $DESKTOP ]] || { echo "desktop is not executable: $DESKTOP" >&2; exit 2; }
[[ -x $ENGINE ]] || { echo "engine is not executable: $ENGINE" >&2; exit 2; }
REAL_CODEX=$(command -v codex || true)
[[ -n $REAL_CODEX && -x $REAL_CODEX ]] || { echo "codex is not executable on PATH" >&2; exit 2; }
REAL_HOME=${HOME:?HOME must point to the native Codex home of the user}
[[ -d $REAL_HOME ]] || { echo "HOME does not exist: $REAL_HOME" >&2; exit 2; }
[[ $OUTPUT = /* && $OUTPUT != *'/../'* && $OUTPUT != */.. ]] || {
  echo "--output must be an absolute path without '..': $OUTPUT" >&2; exit 2;
}
case "$OUTPUT" in
  /|/mnt/toshiba/alexandrie/hoh|/mnt/toshiba/alexandrie/hoh/pilot-founder*|/mnt/toshiba/alexandrie/hoh/release|/mnt/toshiba/alexandrie/hoh/release/*)
    echo "refusing unsafe or reserved output directory: $OUTPUT" >&2; exit 2 ;;
esac
[[ ! -L $OUTPUT ]] || { echo "refusing symlink output directory: $OUTPUT" >&2; exit 2; }
if [[ -d $OUTPUT && -n $(find "$OUTPUT" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null) && $REUSE -ne 1 ]]; then
  echo "output is not empty; pass --reuse to keep and reuse it: $OUTPUT" >&2
  exit 2
fi

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
QA_DISPLAY=$(choose_display) || {
  echo "requested display is invalid or busy; use :110..:119 or auto" >&2; exit 2;
}
command -v Xvfb >/dev/null || { echo "Xvfb is required" >&2; exit 2; }
command -v dbus-run-session >/dev/null || { echo "dbus-run-session is required" >&2; exit 2; }
command -v setsid >/dev/null || { echo "setsid is required for owned process-group cleanup" >&2; exit 2; }

mkdir -p "$OUTPUT"/{agents,cache,config,data,fakebin,home/Documents/HoH,logs,runtime,state}
chmod 700 "$OUTPUT/runtime"
printf 'XDG_DOCUMENTS_DIR="%s"\n' "$OUTPUT/home/Documents" >"$OUTPUT/config/user-dirs.dirs"
mkdir -p "$OUTPUT/config/local.hoh.desktop"
printf '{"folder":"%s"}\n' "$OUTPUT/home/Documents/HoH" >"$OUTPUT/config/local.hoh.desktop/workspace.json"
printf 'HOH-18 isolated live workspace.\n' >"$OUTPUT/home/Documents/HoH/README.txt"
# CEO 24.11 (C9 QA: claude under QA started the founder's user-level playwright MCP via npx @latest):
# QA never brings up the human's settings or MCP servers. The engine's isolated profile passes
# `--setting-sources "" --strict-mcp-config` with no config; the founder's window is not launched by this script.
mkdir -p "$OUTPUT/data/local.hoh.desktop/engine/profiles/window"
printf -- '- id: harness-claude\n  config:\n    settingSources: isolated\n' \
  >"$OUTPUT/data/local.hoh.desktop/engine/profiles/window/cordis.patch.yml"

# Preserve Codex's native login environment while isolating the desktop and
# engine. The wrapper does not inspect, copy, or modify any Codex-owned file.
CODEX_WRAPPER="$OUTPUT/fakebin/codex"
{
  printf '#!/usr/bin/env bash\nexec env'
  for name in XDG_CONFIG_HOME XDG_CACHE_HOME XDG_DATA_HOME XDG_STATE_HOME; do
    [[ -v $name ]] || printf ' -u %q' "$name"
  done
  for name in XDG_CONFIG_HOME XDG_CACHE_HOME XDG_DATA_HOME XDG_STATE_HOME; do
    [[ -v $name ]] && printf ' %q' "$name=${!name}"
  done
  # The runtime dir stays the run's own (never /run/user/1000, rule "звук и шина"); Codex's login is in ~/.codex.
  printf ' %q' "XDG_RUNTIME_DIR=$OUTPUT/runtime"
  printf ' HOME=%q %q "$@"\n' "$REAL_HOME" "$REAL_CODEX"
} >"$CODEX_WRAPPER"
cat >"$OUTPUT/fakebin/xdg-open" <<'EOF'
#!/usr/bin/env bash
printf '%s xdg-open blocked: ' "$(date --iso-8601=seconds)" >>"${HOH_DEMO_OUTPUT:?}/logs/xdg-open.log"
printf '%q ' "$@" >>"${HOH_DEMO_OUTPUT:?}/logs/xdg-open.log"
printf '\n' >>"${HOH_DEMO_OUTPUT:?}/logs/xdg-open.log"
exit 0
EOF
chmod +x "$CODEX_WRAPPER" "$OUTPUT/fakebin/xdg-open"

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

Xvfb "$QA_DISPLAY" -screen 0 1440x900x24 -nolisten tcp -extension GLX >"$OUTPUT/logs/xvfb.log" 2>&1 &
XVFB_PID=$!
printf '%s\n' "$XVFB_PID" >"$OUTPUT/state/xvfb.pid"
for _ in {1..50}; do
  [[ -S /tmp/.X11-unix/X${QA_DISPLAY#:} ]] && break
  kill -0 "$XVFB_PID" 2>/dev/null || { echo "Xvfb exited; see $OUTPUT/logs/xvfb.log" >&2; exit 3; }
  sleep 0.1
done
[[ -S /tmp/.X11-unix/X${QA_DISPLAY#:} ]] || { echo "Xvfb did not become ready" >&2; exit 3; }

echo "HOH-18 live window"
echo "  display:   $QA_DISPLAY (1440x900)"
echo "  desktop:   $DESKTOP"
echo "  engine:    $ENGINE"
echo "  data:      $OUTPUT"
echo "  workspace: $OUTPUT/home/Documents/HoH"
echo "  logs:      $OUTPUT/logs"
echo "Follow the select Codex -> command -> approval -> response -> Stop scenario from --help."

run_window() {
  local run=$1 rc=0
  # The private bus and everything it activates (portal, keyring) get the isolated runtime dir too:
  # the outer environment carries XDG_RUNTIME_DIR=/run/user/1000 (rule "звук и шина", lead 24.11).
  # Its own bus config has no service directories: nothing is activated on it (no portal, no keyring).
  cat >"$OUTPUT/runtime/bus.conf" <<BUS
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir=$OUTPUT/runtime</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
BUS
  setsid env XDG_RUNTIME_DIR="$OUTPUT/runtime" PULSE_SERVER=unix:/nonexistent PIPEWIRE_REMOTE=/nonexistent \
    GTK_USE_PORTAL=0 NO_AT_BRIDGE=1 dbus-run-session --config-file="$OUTPUT/runtime/bus.conf" -- env \
    DISPLAY="$QA_DISPLAY" HOME="$REAL_HOME" \
    XDG_CONFIG_HOME="$OUTPUT/config" XDG_CACHE_HOME="$OUTPUT/cache" \
    XDG_DATA_HOME="$OUTPUT/data" XDG_STATE_HOME="$OUTPUT/state" \
    XDG_RUNTIME_DIR="$OUTPUT/runtime" DSH_AGENTS_HOME="$OUTPUT/agents" \
    HOH_ENGINE_PATH="$ENGINE" HOH_DEMO_OUTPUT="$OUTPUT" \
    PATH="$OUTPUT/fakebin:$PATH" GDK_BACKEND=x11 GIO_USE_VFS=local \
    LIBGL_ALWAYS_SOFTWARE=1 WEBKIT_DISABLE_COMPOSITING_MODE=1 \
    "$DESKTOP" >"$OUTPUT/logs/window-$run.stdout.log" 2>"$OUTPUT/logs/window-$run.stderr.log" &
  APP_PID=$!
  printf '%s\n' "$APP_PID" >"$OUTPUT/state/window-$run.pgid"
  wait "$APP_PID" || rc=$?
  APP_PID=
  printf '%s run=%s exit=%s\n' "$(date --iso-8601=seconds)" "$run" "$rc" >>"$OUTPUT/logs/runs.log"
  return "$rc"
}

run_window first || { rc=$?; echo "first window failed with $rc; see $OUTPUT/logs" >&2; exit "$rc"; }
if [[ $SINGLE_RUN -eq 0 ]]; then
  echo "Restarting the window with the same data. Confirm history, then close it."
  run_window restart || { rc=$?; echo "restart failed with $rc; see $OUTPUT/logs" >&2; exit "$rc"; }
fi
echo "Live-window run complete. Data and logs remain in $OUTPUT"
