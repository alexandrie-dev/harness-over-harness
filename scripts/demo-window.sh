#!/usr/bin/env bash
# Показ окна HoH (по умолчанию коммит 9362ae5f, «новый вид»; прежний: HOH_DEMO_COMMIT=93a6f994) на рабочем столе, с mock-моделью.
#   bash scripts/demo-window.sh            — новый показ: новая папка данных, mock с начала сценария
#   bash scripts/demo-window.sh ПАПКА      — повторный запуск с теми же данными (история сохраняется)
# Все данные (настройки, ключ, журнал, рабочая папка) — в ПАПКЕ на /mnt/toshiba; настоящий HOME не трогается.
# mock слушает 127.0.0.1 на свободном порту (не 8000); при закрытии окна скрипт останавливает свой mock.
#   HOH_DEMO_COMMIT=93a6f994 bash scripts/demo-window.sh — показать прежнюю сборку
# Шпаргалка: scripts/demo-window.md
set -uo pipefail
C=${HOH_DEMO_COMMIT:-9362ae5f}
R=/mnt/toshiba/alexandrie/hoh/release
DESKTOP=$R/hoh-desktop-$C          # окно, собранное из $C (движок вшит: $R/hoh-$C)
ENGINE=$R/hoh-$C
MOCK=$R/hoh-llm-openai-mock-$C
REPO=$(cd "$(dirname "$0")/.." && pwd)
for f in "$DESKTOP" "$ENGINE" "$MOCK"; do [[ -x $f ]] || { echo "нет файла: $f" >&2; exit 2; }; done

D=${1:-/mnt/toshiba/alexandrie/hoh/demo/$(date +%Y%m%d-%H%M%S)}
case $D in /mnt/toshiba/*) ;; *) echo "папка показа должна быть на /mnt/toshiba: $D" >&2; exit 2;; esac
mkdir -p "$D"/{home,config,cache,data}
W=$D/home/Documents/HoH                 # папка «Документы/HoH» из мастера
if [[ ! -e $D/scenario.json ]]; then    # первый запуск: сценарий из коммита и заготовки файлов
  git -C "$REPO" show "$C:crates/hoh-llm-openai-mock/scenarios/qa-window-a.json" > "$D/scenario.json" || exit 2
  mkdir -p "$W"
  printf 'alpha\nbeta\ngamma TODO\n' > "$W/notes.txt"
  echo 'старый файл 1' > "$W/old-1.txt"; echo 'старый файл 2' > "$W/old-2.txt"
  echo 'старый отчёт' > "$W/report.txt"; echo 'mode = draft' > "$W/config.txt"
  echo 'print("привет из python")' > "$W/script.py"
fi

PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
[[ $PORT == 8000 ]] && PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
"$MOCK" --scenario "$D/scenario.json" --listen "127.0.0.1:$PORT" --part-delay 0=200 --part-delay 1=100 \
        --journal "$D/mock-journal.jsonl" >"$D/mock.log" 2>&1 &
MOCK_PID=$!
trap 'kill "$MOCK_PID" 2>/dev/null; wait "$MOCK_PID" 2>/dev/null' EXIT INT TERM
for _ in $(seq 50); do grep -q '"ready"' "$D/mock.log" 2>/dev/null && break; sleep 0.1; done
grep -q '"ready"' "$D/mock.log" || { echo "mock не запустился, см. $D/mock.log" >&2; exit 3; }

echo "Папка показа: $D"
echo "Адрес модели для мастера:  http://127.0.0.1:$PORT/v1   (ключ — любой текст, например demo)"
echo "Рабочая папка:             $W"
echo "Закройте окно, чтобы завершить показ (mock остановится сам)."

# Дисплей и шина — настоящие (WAYLAND_DISPLAY/DISPLAY/XDG_RUNTIME_DIR/DBUS), всё остальное — в папке показа.
env HOME="$D/home" XDG_CONFIG_HOME="$D/config" XDG_CACHE_HOME="$D/cache" XDG_DATA_HOME="$D/data" \
    LIBGL_ALWAYS_SOFTWARE=1 WEBKIT_DISABLE_COMPOSITING_MODE=1 \
    "$DESKTOP" >"$D/window.log" 2>&1
rc=$?
echo "$(date '+%F %T') окно завершилось, код $rc (лог: $D/window.log)" | tee -a "$D/runs.log"
exit $rc
