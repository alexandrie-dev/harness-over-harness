#!/usr/bin/env bash
# Снимок формул в НАСТОЯЩЕМ окне HoH (WebKitGTK, его CSP) под Xvfb; движок настоящий, модель — mock с готовым ответом.
# Запуск: ui/scripts/qa-window-formula.sh <имя снимка> [ширина высота]   (после pnpm -C ui build:offline и cargo build -p hoh-desktop)
# Данные одноразовые, в /mnt/toshiba/alexandrie/hoh/qa/u1-t3fix (там же state с настройками мастера, scenario.json, home-docs).
# Браузер системы не запускается: xdg-open подменён заглушкой-логом.
set -uo pipefail
Q=/mnt/toshiba/alexandrie/hoh/qa/u1-t3fix
name=${1:?имя снимка}; W=${2:-1440}; H=${3:-900}
REPO=$(cd "$(dirname "$0")/../.." && pwd)
DESK=${HOH_DESKTOP_BIN:-${CARGO_TARGET_DIR:-$REPO/target}/debug/hoh-desktop}
ENGINE=${HOH_ENGINE_PATH:-/mnt/toshiba/alexandrie/hoh/target-qa/release/hoh}
MOCK=/mnt/toshiba/alexandrie/hoh/release/hoh-llm-openai-mock-93a6f994
PORT=60303
mkdir -p "$Q/shots" "$Q/fakebin"
printf '#!/bin/sh\necho "$(date +%%T) xdg-open $*" >> %s/xdg-open.log\nexit 0\n' "$Q" > "$Q/fakebin/xdg-open"; chmod +x "$Q/fakebin/xdg-open"
"$MOCK" --scenario "$Q/scenario.json" --listen 127.0.0.1:$PORT --journal "$Q/mock-journal.jsonl" > "$Q/mock.log" 2>&1 &
mock=$!
cat > "$Q/inner.sh" <<INNER
#!/usr/bin/env bash
export HOME=$Q/home; rm -rf "\$HOME"; mkdir -p "\$HOME/runtime"; chmod 700 "\$HOME/runtime"
ln -s $Q/home-docs "\$HOME/Documents"; ln -s $Q/state/config "\$HOME/config"; ln -s $Q/state/cache "\$HOME/cache"; ln -s $Q/state/data "\$HOME/data"
printf '{"folder":"%s"}' "\$HOME/Documents/HoH" > $Q/state/config/local.hoh.desktop/workspace.json
export XDG_CONFIG_HOME=\$HOME/config XDG_CACHE_HOME=\$HOME/cache XDG_DATA_HOME=\$HOME/data XDG_RUNTIME_DIR=\$HOME/runtime
export GDK_BACKEND=x11 LIBGL_ALWAYS_SOFTWARE=1 WEBKIT_DISABLE_COMPOSITING_MODE=1 GIO_USE_VFS=local
export PATH=$Q/fakebin:\$PATH HOH_ENGINE_PATH=$ENGINE
$DESK > $Q/app.stdout.log 2> $Q/app.stderr.log &
app=\$!
for i in \$(seq 1 40); do sleep 1; xdotool search --onlyvisible --name '.' >/dev/null 2>&1 && break; done
sleep 8
xdotool key ctrl+n; sleep 3; xdotool mousemove $((W/2)) $((H-100)) click 1; sleep 1
xdotool type --delay 20 -- "формулы"; xdotool key Return
sleep 12
import -window root "$Q/shots/$name.png"
kill \$app; wait \$app 2>/dev/null
INNER
chmod +x "$Q/inner.sh"
timeout 300 xvfb-run -a -e "$Q/xvfb.log" -s "-screen 0 ${W}x${H}x24 -nolisten tcp -extension GLX" dbus-run-session "$Q/inner.sh" > "$Q/run.out" 2>&1
kill $mock 2>/dev/null; wait $mock 2>/dev/null
echo "$Q/shots/$name.png"
