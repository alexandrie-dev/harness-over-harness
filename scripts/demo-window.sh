#!/usr/bin/env bash
# Показ окна HoH (по умолчанию коммит 9362ae5f, «новый вид»; прежний: HOH_DEMO_COMMIT=93a6f994) на рабочем столе, с mock-моделью.
#   bash scripts/demo-window.sh            — новый показ: новая папка данных, mock с начала сценария
#   bash scripts/demo-window.sh ПАПКА      — повторный запуск с теми же данными (история сохраняется)
#   bash scripts/demo-window.sh --claude [ПАПКА] — выпуск с Claude: claude получает настоящий HOME человека (его вход и
#     личные настройки), данные HoH (настройки, ключ, журнал, сессии, Documents/HoH, webview, .agents) остаются в ПАПКЕ.
# Все данные (настройки, ключ, журнал, рабочая папка) — в ПАПКЕ на /mnt/toshiba; настоящий HOME не трогается.
# mock слушает 127.0.0.1 на свободном порту (не 8000); при закрытии окна скрипт останавливает свой mock.
#   HOH_DEMO_COMMIT=93a6f994 bash scripts/demo-window.sh — показать прежнюю сборку
# Шпаргалка: scripts/demo-window.md
set -uo pipefail
# Предупреждение до пакета A (песочница; CEO 21.11 04:01, QA-1 P0). Снять — поставить SANDBOX_WARNING=0 (одна константа).
# Адрес настоящей модели в мастере заблокировать нельзя, поэтому режим с mock только предупреждает.
SANDBOX_WARNING=1
((SANDBOX_WARNING)) && echo "Встроенный помощник в этой сборке может выполнять команды вне своей папки без вопроса. Подключать настоящую модель и работать с настоящими файлами нельзя до исправления"
CLAUDE_MODE=0; ARGS=()
for a in "$@"; do [[ $a == --claude ]] && CLAUDE_MODE=1 || ARGS+=("$a"); done
set -- "${ARGS[@]+"${ARGS[@]}"}"
R=${HOH_DEMO_RELEASE:-/mnt/toshiba/alexandrie/hoh/release}   # HOH_DEMO_RELEASE — каталог сборок (тест и кандидат выпуска)
# Без флага — 9362ae5f. С --claude умолчание — собранный кандидат выпуска: коммит из файла $R/claude-candidate
# (или HOH_DEMO_COMMIT); если его нет, скрипт отказывает, а не берёт старую сборку без Claude.
if ((CLAUDE_MODE)); then
  C=${HOH_DEMO_COMMIT:-$(cat "$R/claude-candidate" 2>/dev/null)}
  [[ -n $C ]] || { echo "--claude: нет кандидата выпуска: задайте HOH_DEMO_COMMIT или файл $R/claude-candidate" >&2; exit 2; }
else
  C=${HOH_DEMO_COMMIT:-9362ae5f}
fi
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
# --claude: HOME остаётся настоящим (HOH_DEMO_REAL_HOME — только для теста), всё своё окно кладёт в папку показа:
# XDG_* (настройки окна, данные webview, DSH_HOME движка = XDG_DATA_HOME/…/engine), Documents (user-dirs.dirs), .agents.
if ((CLAUDE_MODE)); then
  REAL_HOME=${HOH_DEMO_REAL_HOME:-$HOME}
  [[ -n $REAL_HOME && -d $REAL_HOME ]] || { echo "нет настоящего HOME: $REAL_HOME" >&2; exit 2; }
  printf 'XDG_DOCUMENTS_DIR="%s/Documents"\n' "$D/home" > "$D/config/user-dirs.dirs"
  mkdir -p "$D/agents"
  echo "Claude работает с вашими настоящими личными настройками и входом (HOME=$REAL_HOME); данные HoH — в $D"
  APP_ENV=(HOME="$REAL_HOME" DSH_AGENTS_HOME="$D/agents" HOH_ENGINE_PATH="$ENGINE")
  # Процесс claude получает настоящие HOME и XDG_* человека, а не подменённые окну: движок и claude наследуют
  # окружение окна, поэтому claude ищется через обёртку первой в PATH, она возвращает человеческие значения.
  REAL_CLAUDE=$(command -v claude) || REAL_CLAUDE=
  if [[ -n $REAL_CLAUDE ]]; then
    mkdir -p "$D/bin"
    UNSET=(); SET=("HOME=$REAL_HOME")
    for v in XDG_CONFIG_HOME XDG_CACHE_HOME XDG_DATA_HOME XDG_STATE_HOME; do
      if [[ -n ${!v-} ]]; then SET+=("$v=${!v}"); else UNSET+=(-u "$v"); fi
    done
    { echo '#!/usr/bin/env bash'; echo "# claude с настоящими HOME и XDG_* человека (scripts/demo-window.sh --claude)"
      printf 'exec env'; printf ' %q' "${UNSET[@]}" "${SET[@]}" "$REAL_CLAUDE"; printf ' "$@"\n'; } > "$D/bin/claude"
    chmod +x "$D/bin/claude"
    APP_ENV+=(PATH="$D/bin:$PATH")
  else
    echo "claude не найден в PATH: окно покажет «Claude не установлен»" >&2
  fi
else
  APP_ENV=(HOME="$D/home")
fi

# Дисплей и шина — настоящие (WAYLAND_DISPLAY/DISPLAY/XDG_RUNTIME_DIR/DBUS), всё остальное — в папке показа (HOME — см. выше).
env "${APP_ENV[@]}" HOH_WINDOW_HOME="$D/home" XDG_CONFIG_HOME="$D/config" XDG_CACHE_HOME="$D/cache" XDG_DATA_HOME="$D/data" \
    LIBGL_ALWAYS_SOFTWARE=1 WEBKIT_DISABLE_COMPOSITING_MODE=1 \
    "$DESKTOP" >"$D/window.log" 2>&1
rc=$?
echo "$(date '+%F %T') окно завершилось, код $rc (лог: $D/window.log)" | tee -a "$D/runs.log"
exit $rc
