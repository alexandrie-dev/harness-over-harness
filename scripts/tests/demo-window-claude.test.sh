#!/usr/bin/env bash
# Тест scripts/demo-window.sh --claude (решение CEO 21.11): claude получает заданный (настоящий) HOME, данные HoH — в папке
# показа, в заданный дом от HoH ничего не пишется, кроме того, что пишет сам claude; без флага — как раньше.
# Подделки: окно (имитирует пути окна: XDG_*, user-dirs.dirs, DSH_AGENTS_HOME/.agents и запускает claude с PATH),
# mock, claude (обёртка пишет свой HOME и запускает scripts/tests/claude-live-smoke/fake-claude). Настоящий claude не трогается.
set -uo pipefail
R=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
T=$(mktemp -d -p "${TMPDIR:-/dev/shm/hoh}" dwc.XXXXXX); trap 'rm -rf "$T"' EXIT
fail=0; ok() { echo "PASS $1"; }; bad() { echo "FAIL $1: $2"; fail=1; }
C=$(git -C "$R" rev-parse HEAD)
REL=$T/release; BIN=$T/bin; mkdir -p "$REL" "$BIN"
cat >"$REL/hoh-desktop-$C" <<'SH'
#!/usr/bin/env bash
# подделка окна: пишет туда, куда пишет окно (по тем же правилам выбора папок), и зовёт claude как движок
echo "HOME=$HOME" >"$DEMO_OUT/app-env"; echo "HOH_ENGINE_PATH=${HOH_ENGINE_PATH:-}" >>"$DEMO_OUT/app-env"; echo "XDG_DATA_HOME=$XDG_DATA_HOME" >>"$DEMO_OUT/app-env"
echo "DSH_AGENTS_HOME=${DSH_AGENTS_HOME:-}" >>"$DEMO_OUT/app-env"
mkdir -p "$XDG_DATA_HOME/engine" "$XDG_CONFIG_HOME/app" "${DSH_AGENTS_HOME:-$HOME/.agents}"
docs=$(sed -n 's/^XDG_DOCUMENTS_DIR="\(.*\)"$/\1/p' "$XDG_CONFIG_HOME/user-dirs.dirs" 2>/dev/null); docs=${docs:-$HOME/Documents}
mkdir -p "$docs/HoH"; : >"$XDG_DATA_HOME/engine/journal"; : >"${DSH_AGENTS_HOME:-$HOME/.agents}/state"
claude --version >"$DEMO_OUT/claude-version"
SH
printf '#!/usr/bin/env bash\ncp "$0" /dev/null; exec sleep 30\n' >"$REL/hoh-llm-openai-mock-$C"
sed -i 's|^cp .*; exec|echo "{\\"ready\\":true}"; exec|' "$REL/hoh-llm-openai-mock-$C"
printf '#!/usr/bin/env bash\nexit 0\n' >"$REL/hoh-$C"
printf '#!/usr/bin/env bash\n{ echo "HOME=$HOME"; echo "XDG_CONFIG_HOME=${XDG_CONFIG_HOME-UNSET}"; echo "XDG_DATA_HOME=${XDG_DATA_HOME-UNSET}"; echo "XDG_CACHE_HOME=${XDG_CACHE_HOME-UNSET}"; echo "XDG_STATE_HOME=${XDG_STATE_HOME-UNSET}"; } >"$DEMO_OUT/claude-env"; mkdir -p "$HOME/.claude"; : >"$HOME/.claude/claude-wrote"\nexec "%s" "$@"\n' \
  "$R/scripts/tests/claude-live-smoke/fake-claude" >"$BIN/claude"
chmod +x "$REL"/* "$BIN/claude"
run() { # $1 папка показа, $2 настоящий дом, остальное — флаги; вывод в $T/out; COMMIT_ARG — задать ли HOH_DEMO_COMMIT
  DEMO_OUT=$1.out; mkdir -p "$DEMO_OUT"; export DEMO_OUT
  local commit=(HOH_DEMO_COMMIT=$C); [[ -n ${NO_COMMIT-} ]] && commit=(-u HOH_DEMO_COMMIT)
  env -u XDG_STATE_HOME "${commit[@]}" PATH="$BIN:$PATH" HOH_DEMO_RELEASE=$REL HOH_DEMO_REAL_HOME=$2 HOME=$2 \
    XDG_CONFIG_HOME=$2/.config XDG_DATA_HOME=$2/.local/share XDG_CACHE_HOME=$2/.cache \
    bash "$R/scripts/demo-window.sh" "${@:3}" "$1" >"$T/out" 2>&1
}
listing() { (cd "$1" && find . | sort); }

# 1. --claude: настоящий (поддельный) дом
H=$T/realhome; mkdir -p "$H"; D=/mnt/toshiba/alexandrie/hoh/demo/test-dwc-$$; trap 'rm -rf "$T" "$D" "$D.out"' EXIT
run "$D" "$H" --claude
grep -qx "HOME=$H" "$D.out/claude-env" && ok "claude получил заданный HOME" || bad "claude HOME" "$(cat "$D.out/claude-env" 2>&1)"
grep -qx "HOME=$H" "$D.out/app-env" && ok "окно видит настоящий HOME" || bad "app HOME" "$(cat "$D.out/app-env")"
grep -qx "2.1.0 (Claude Code)" "$D.out/claude-version" && ok "fake-claude ответил" || bad "fake-claude" "$(cat "$D.out/claude-version")"
for x in "XDG_CONFIG_HOME=$H/.config" "XDG_DATA_HOME=$H/.local/share" "XDG_CACHE_HOME=$H/.cache" "XDG_STATE_HOME=UNSET"; do
  grep -qx "$x" "$D.out/claude-env" && ok "claude получил настоящий $x" || bad "claude $x" "$(cat "$D.out/claude-env")"
done
grep -q "^XDG_DATA_HOME=$D/data" "$D.out/app-env" && ok "окно получило XDG_DATA_HOME папки показа" || bad "окно XDG" "$(cat "$D.out/app-env")"
grep -qx "HOH_ENGINE_PATH=$REL/hoh-$C" "$D.out/app-env" && ok "окно знает путь движка кандидата" || bad "engine path" "$(cat "$D.out/app-env")"
[[ $(listing "$H") == $'.\n./.claude\n./.claude/claude-wrote' ]] && ok "в заданный дом пишет только claude" || bad "дом" "$(listing "$H" | tr '\n' ' ')"
[[ -f $D/data/engine/journal && -f $D/agents/state && -d $D/home/Documents/HoH ]] && ok "данные HoH (движок, .agents, Documents) в папке показа" || bad "папка показа" "$(listing "$D" | tr '\n' ' ')"
[[ $(grep -c "^Claude работает с вашими настоящими личными настройками и входом (HOME=$H); данные HoH — в $D\$" "$T/out") == 1 ]] && ok "строка-предупреждение (одна)" || bad "строка" "$(cat "$T/out")"
rm -rf "$D" "$D.out"

# 1а. предупреждение: первая строка вывода в обоих режимах
[[ $(head -1 "$T/out") == "Встроенный помощник в этой сборке может выполнять команды вне своей папки без вопроса. Подключать настоящую модель и работать с настоящими файлами нельзя до исправления" ]] && ok "--claude: первая строка — предупреждение о песочнице" || bad "предупреждение --claude" "$(head -2 "$T/out")"
# 1б. кандидат по умолчанию: без HOH_DEMO_COMMIT и без файла — отказ; с файлом claude-candidate — берётся он
H3=$T/realhome3; mkdir -p "$H3"
NO_COMMIT=1 run "$D" "$H3" --claude; rc=$?
[[ $rc != 0 ]] && grep -q "нет кандидата выпуска" "$T/out" && ok "--claude без кандидата: отказ" || bad "без кандидата" "rc=$rc $(cat "$T/out")"
echo "$C" >"$REL/claude-candidate"; rm -rf "$D" "$D.out"
NO_COMMIT=1 run "$D" "$H3" --claude
grep -qx "HOH_ENGINE_PATH=$REL/hoh-$C" "$D.out/app-env" && ok "--claude берёт кандидата из claude-candidate" || bad "кандидат" "$(cat "$T/out")"
rm -f "$REL/claude-candidate"; rm -rf "$D" "$D.out"

# 2. без флага: прежнее поведение (HOME = папка показа, строки нет, DSH_AGENTS_HOME не задан)
H2=$T/realhome2; mkdir -p "$H2"
run "$D" "$H2"
grep -qx "HOME=$D/home" "$D.out/app-env" && ok "без флага: HOME окна в папке показа" || bad "без флага HOME" "$(cat "$D.out/app-env")"
grep -qx "DSH_AGENTS_HOME=" "$D.out/app-env" && ok "без флага: DSH_AGENTS_HOME не задан" || bad "agents" "$(cat "$D.out/app-env")"
grep -q "Claude работает" "$T/out" && bad "без флага" "строка Claude напечатана" || ok "без флага: строки Claude нет"
[[ -z $(ls -A "$H2") ]] && ok "без флага: настоящий дом пуст" || bad "дом без флага" "$(ls -A "$H2")"
[[ $(head -1 "$T/out") == "Встроенный помощник в этой сборке может выполнять команды вне своей папки без вопроса. Подключать настоящую модель и работать с настоящими файлами нельзя до исправления" ]] && ok "mock: первая строка — предупреждение о песочнице" || bad "предупреждение mock" "$(head -2 "$T/out")"
# claude без флага получает HOME папки показа (как раньше)
grep -qx "HOME=$D/home" "$D.out/claude-env" && ok "без флага: claude получает HOME папки показа" || bad "claude без флага" "$(cat "$D.out/claude-env")"
exit $fail
