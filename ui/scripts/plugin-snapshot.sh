#!/bin/bash -p
set -euo pipefail
unset CDPATH

script_dir=${BASH_SOURCE[0]%/*}
cd -- "$script_dir/.."
out_dir=${1:?usage: scripts/plugin-snapshot.sh OUTPUT_DIRECTORY}
mkdir -p -- "$out_dir"
out_dir=$(cd -- "$out_dir" && pwd -P)

if [[ ${1:-} != --isolated && ${HOH_PLUGIN_SNAPSHOT_ISOLATED:-0} != 1 ]]; then
  pactl list short sink-inputs > "$out_dir/pactl-before.txt" 2> "$out_dir/pactl-before.err" || true
  runtime_dir="$out_dir/xdg"
  mkdir -p -- "$runtime_dir"
  chmod 700 -- "$runtime_dir"
  env -u DBUS_SESSION_BUS_ADDRESS \
    XDG_RUNTIME_DIR="$runtime_dir" \
    PULSE_SERVER=unix:/nonexistent \
    PIPEWIRE_REMOTE=/nonexistent \
    ALSA_CONFIG_PATH=/dev/null \
    LIBGL_ALWAYS_SOFTWARE=1 \
    __GLX_VENDOR_LIBRARY_NAME=mesa \
    HOH_PLUGIN_SNAPSHOT_ISOLATED=1 \
    dbus-run-session -- /bin/bash -p "$script_dir/plugin-snapshot.sh" "$out_dir"
  pactl list short sink-inputs > "$out_dir/pactl-after.txt" 2> "$out_dir/pactl-after.err" || true
  exit 0
fi

if [[ -e /tmp/.X11-unix/X144 ]]; then
  echo 'snapshot: X display :144 is already in use' >&2
  exit 1
fi

Xvfb :144 -screen 0 1920x1080x24 -nolisten tcp -extension GLX > "$out_dir/xvfb.log" 2>&1 &
xvfb_pid=$!
cleanup() {
  if kill -0 "$xvfb_pid" 2>/dev/null; then kill "$xvfb_pid" 2>/dev/null || true; fi
  wait "$xvfb_pid" 2>/dev/null || true
}
trap cleanup EXIT INT TERM
for _ in 1 2 3 4 5; do
  [[ -e /tmp/.X11-unix/X144 ]] && break
  sleep 0.2
done
if ! kill -0 "$xvfb_pid" 2>/dev/null || [[ ! -e /tmp/.X11-unix/X144 ]]; then
  echo 'snapshot: Xvfb :144 did not start' >&2
  exit 1
fi

DISPLAY=:144 node scripts/plugin-snapshot.mjs "$out_dir"
