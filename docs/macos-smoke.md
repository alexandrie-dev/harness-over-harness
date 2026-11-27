# Приёмка HoH на Apple Silicon

Проведите проверку на локальном Mac из отдельной учётной записи macOS, в которой
HoH ещё не запускался. Это даёт чистое состояние Finder и HoH без удаления
данных. Установите актуальный полный пакет Codex CLI и войдите в Codex под этой
учётной записью. HoH определяет совместимость по ответам `app-server`.

```sh
curl -fsSL https://chatgpt.com/codex/install.sh | sh
"$HOME/.local/bin/codex" login
```

На macOS установщик создаёт ссылки на `codex` и `codex-code-mode-host` в
`~/.local/bin`, который HoH добавляет в PATH при запуске из Finder. Codex
запускает помощник из своей установки. Если установлен ещё один `codex`,
сверьте результат `command -v codex` с путём к установленному CLI.
[Установка Codex CLI](https://learn.chatgpt.com/docs/codex/cli#install-codex)

## Подготовка пакета

Используйте ZIP, собранный из одной ревизии с интерфейсом, окном и движком.
Запишите ревизию сборки, версию macOS, версию Codex и контрольную сумму ZIP.

```sh
archive="/absolute/path/to/HoH-macos-arm64.zip"
qa_dir="$(mktemp -d "$HOME/HoHMacSmoke.XXXXXX")"
mkdir "$qa_dir/project"
printf 'QA-6819\n' > "$qa_dir/project/note.txt"
/usr/bin/ditto -x -k "$archive" "$qa_dir"
app="$qa_dir/HoH.app"

uname -m
sw_vers -productVersion
command -v codex
"$HOME/.local/bin/codex" --version
test -x "$HOME/.local/bin/codex-code-mode-host"
shasum -a 256 "$archive"
file "$app/Contents/MacOS/hoh-desktop" "$app/Contents/MacOS/hoh"
/usr/bin/codesign --verify --deep --strict "$app"
printf 'Проект: %s\n' "$qa_dir/project"

/usr/bin/open -R "$app"
```

Ожидаются `arm64` у системы и обоих исполняемых файлов HoH, версия полного
пакета Codex и исполняемый `codex-code-mode-host`, успешная проверка подписи
и выделенный в Finder пакет. В тестовой графической сессии не задавайте
`HOH_ENGINE_PATH`, `HOH_TEST_ENGINE` или `HOH_WINDOW_PROFILE`. Если macOS
блокирует открытие, запишите сообщение системы и результат проверки подписи.
[Проверка подписи в документации Apple](https://developer.apple.com/documentation/technotes/tn3161-inside-code-signing-certificates)

## Сценарий в окне

1. Нажмите **⌘O** в Finder, чтобы открыть выделенный HoH. В чате откройте меню
   проектов и выберите **Add project → Local folder**. В палитре HoH нажмите
   **⌘O**. В системном диалоге нажмите **⇧⌘G**, введите путь
   `$qa_dir/project` из подготовки, подтвердите переход и выбор папки.
   Проект появляется в окне. Отдельно откройте диалог повторно и отмените выбор
   через **Esc**. Список проектов сохраняет прежний состав.
2. В новом чате проекта выберите Codex и режим **Supervised**. Попросите
   выполнить `/bin/cat note.txt` в терминале проекта и вывести результат.
   Если HoH запросит подтверждение команды, разрешите её один раз. Проверьте
   выполнение `/bin/cat note.txt` и строку `QA-6819` в выводе инструмента,
   затем дождитесь завершения хода.
3. Двумя отдельными запросами попросите Codex выполнить команду, которая
   записывает маркер в файл рядом с папкой проекта. Используйте абсолютный
   путь из `$qa_dir` и разные имена `deny.txt` и `allow.txt`. Например,
   `printf 'hoh-qa\n' > "/absolute/path/to/HoHMacSmoke.XXXXXX/deny.txt"`.
   Попросите после отказа прекратить ход. В первом случае отклоните запрос
   в окне и убедитесь, что файла нет. Во втором подтвердите один раз и
   проверьте появление `allow.txt` со строкой `hoh-qa`. Для зачёта оба запроса
   должны появиться в окне.
4. Попросите выполнить `/bin/sleep 90`. Если потребуется, подтвердите эту
   команду. Запишите PID Codex app-server и запущенного `sleep` через
   `ps -axww -o pid=,ppid=,command=` в тестовой учётной записи. Нажмите
   **«Стоп»** и проверьте, что ход завершился, а записанные процессы исчезли
   из списка. PID движка HoH при этом сохраняется.
5. Закройте HoH через **⌘Q**, снова откройте из Finder и выберите тот же чат.
   Ожидайте предыдущие сообщения и успешный ответ на новый запрос.

Во время первого запуска проверьте, что окно запустило движок из пакета.
В тестовой учётной записи держите открытым только один экземпляр HoH.

```sh
desktop_pid="$(pgrep -u "$(id -u)" -x hoh-desktop)"
engine_pid="$(pgrep -P "$desktop_pid" -x hoh)"
ps -ww -p "$engine_pid" -o command=
```

Ожидаются путь `HoH.app/Contents/MacOS/hoh` и аргументы
`--profile window-external`. Чтобы подтвердить тот же тред Codex, снимите
записи привязки после закрытия HoH в шаге 5 и после второго закрытия, когда
новое сообщение уже записано. Команда требует `zstd` и выводит только строки
привязки, которые следует хранить локально.

```sh
history="$HOME/Library/Application Support/local.hoh.desktop/engine/sessions"
find "$history" -name '*.jsonl.zstd' -exec zstd -dc -- '{}' \; |
  grep -F '"type":"plugin:external-harness/session"'
```

У проверяемого чата должен сохраниться тот же непустой `nativeSessionId` с
`harness` равным `codex`. Отдельно запишите результат каждого шага, включая
текст системной ошибки или имя незавершившегося процесса при сбое.

## Воспроизводимая проверка движка на Mac

В checkout той же ревизии, с установленным Rust из `rust-toolchain.toml` и
заполненным кэшем зависимостей выполните проверку с тестовым Codex. Она проверяет
подтверждения, потоковый текст и `thread/resume` с тем же ID, без логина или сети.

```sh
qa_target="$qa_dir/target"
CARGO_TARGET_DIR="$qa_target" cargo build --locked --offline \
  -p hoh-harness-codex --features stub --bin codex-stub &&
TMPDIR=/private/tmp CARGO_TARGET_DIR="$qa_target" \
  HOH_CODEX_STUB="$qa_target/debug/codex-stub" \
  cargo test --locked --offline -p hoh --test window_executor \
    external_profile_runs_and_resumes_codex -- --ignored --exact
```

Ожидается один пройденный тест. Этот результат проверяет протокол движка.
Результат запуска окна и реального CLI фиксируется в сценарии Finder отдельно.
