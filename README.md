# HarnessOverHarness

HoH состоит из движка `hoh` и окна `hoh-desktop` на Tauri. Окно работает с сессиями по одному протоколу. Исполнители подключаются как плагины. Движок и окно собираются из одного checkout, каждый со своим `Cargo.lock`.

## Сборка на Linux

На Ubuntu 24.04 (amd64) установите системные инструменты и библиотеки:

```bash
sudo apt install build-essential libwebkit2gtk-4.1-dev python3-yaml libseccomp2
```

Для Rust нужен rustup с версией из [rust-toolchain.toml](rust-toolchain.toml). Для интерфейса установите Node.js 22.13 или новее и pnpm 12.21.0. pnpm должен запускаться с пустым `HOME`, поэтому для Corepack укажите путь к уже установленному бинарнику pnpm через `HOH_PNPM`.

```bash
scripts/build-linux.sh prepare
scripts/build-linux.sh build
```

`prepare` один раз привязывает установленные `node` и `pnpm` из `PATH` и готовит оба кэша Cargo и кэш pnpm. Для явного выбора бинарников используйте `HOH_NODE` и `HOH_PNPM`. Это этап с доступом к репозиториям зависимостей. Повторите его после изменения lockfile или когда аудиту UI исполнилось 24 часа. Отчёты подготовки сохраняются в `ui/.supply-cache/`.

`build` собирает интерфейс, движок и окно офлайн с зафиксированными зависимостями. Результат находится в `target/engine/release/` и `target/desktop/release/`. На машине с отдельным диском можно задать `HOH_BUILD_DIR` абсолютным путём. Для проверки типов, unit-тестов и браузерной приёмки интерфейса установите Chrome и выполните `scripts/build-linux.sh check`.

## Сборка на macOS Apple Silicon

Установите Xcode Command Line Tools (`xcode-select --install`), Python 3 и rustup.
Rustup подберёт версию Rust из [rust-toolchain.toml](rust-toolchain.toml).
Распакуйте архив интерфейса из той же ревизии в корень checkout, чтобы
появился `ui/dist/index.html`. Интерфейс можно подготовить на Linux командой
`bash ui/scripts/build.sh` после `scripts/build-linux.sh prepare`.

На Mac выполните из корня checkout:

```sh
cargo fetch --locked --target aarch64-apple-darwin
cargo fetch --locked --target aarch64-apple-darwin --manifest-path apps/hoh-desktop/Cargo.toml
python3 scripts/build-desktop.py --build-dir target/macos
```

Обе команды `cargo fetch` подготавливают зависимости. Сборка использует эти
кэши офлайн и создаёт `target/macos/HoH-macos-arm64.zip`. Распакуйте архив и
откройте `HoH.app` в Finder. Внутри приложения есть движок и локальная подпись
для разработки. На Mac включён профиль внешних исполнителей. Для работы с
Codex установите или обновите полный пакет CLI и перезапустите HoH.

```sh
curl -fsSL https://chatgpt.com/codex/install.sh | sh
"$HOME/.local/bin/codex" login
```

Codex запускает `codex-code-mode-host` из своей установки. Если вы
указываете путь к CLI вручную, используйте бинарник из полного пакета.
Проверьте запуск команд в HoH по [сценарию приёмки на Apple Silicon](docs/macos-smoke.md).

## Релизы и обновления

HoH проверяет последний стабильный выпуск в GitHub Releases при запуске и
каждые шесть часов, пока приложение открыто. Если есть более новая версия
для ОС и архитектуры, HoH покажет уведомление. Кнопка **Update** рядом с
**Settings** внизу боковой панели открывает проверку версии и ссылку на
GitHub Releases. По подтверждению HoH скачивает архив, сверяет его размер и
SHA-256 из GitHub, заменяет окно вместе с движком и перезапускается. Если
новая версия не запускается, HoH восстанавливает предыдущую. Для доступа к закрытому
репозиторию выполните `gh auth login --hostname github.com` на компьютере с HoH.

HoH держит одно окно на пользователя. Повторный запуск завершается, пока окно
открыто или HoH перезапускается после обновления.

Для первой установки распакуйте архив релиза и запустите `hoh-desktop` из
распакованной папки на Linux или переместите `HoH.app` в папку приложений на
macOS. Папка, содержащая установку, должна быть доступна пользователю на запись.
Для архивов Linux нужны системные `curl` и `zstd` (`sudo apt install curl zstd`).

Выпуск собирается локально. Установите новую версию в
[`tauri.conf.json`](apps/hoh-desktop/tauri.conf.json) и соберите интерфейс из той
же ревизии. На Linux после подготовки зависимостей используйте:

```bash
bash ui/scripts/build.sh
python3 scripts/build-desktop.py --release --build-dir target/release-linux
```

На Apple Silicon с готовым `ui/dist` и кэшем Cargo из инструкции выше можно
собрать архив с локальной подписью для проверки:

```bash
python3 scripts/build-desktop.py --release --build-dir target/release-macos
```

Для публичного выпуска macOS подпишите `HoH.app` и вложенные исполняемые
файлы сертификатом Developer ID Application и пройдите
[нотаризацию Apple](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution).

Создайте GitHub Release с тегом этой версии (например, `v0.2.0`) и прикрепите
собранные файлы с их исходными именами: `HoH-linux-x86_64.tar.zst`,
`HoH-linux-aarch64.tar.zst` или `HoH-macos-arm64.zip`. Опубликуйте выпуск как
`Latest` после загрузки всех готовых архивов. По умолчанию сборка читает релизы
`alexandrie-dev/harness-over-harness`. Для отдельного репозитория с артефактами
задайте `HOH_RELEASE_REPOSITORY=owner/repo` при сборке окна.

## Запуск

В графической сессии разработки запустите окно с собранным движком:

```bash
"${HOH_BUILD_DIR:-target}/desktop/release/hoh-desktop"
```

Окно хранит настройки, журнал и данные движка в каталогах приложения `local.hoh.desktop`. Команда использует данные текущей графической сессии. Для приёмки задайте отдельные HOME и XDG-каталоги, отдельную шину без активации пользовательских служб и отключите звук. На общем хосте проводите приёмку с одним Xvfb под общим `flock`. Для реальных Claude и Codex нужны соответствующие авторизованные CLI.

- [Конфигурация окна и запуск движка](apps/hoh-desktop/PORT_NOTES.md)
- [Зависимости и проверка поставки UI](docs/ts/DEPS.md)

### Профиль внешних агентов

На Linux для запуска окна с внешними агентами задайте `HOH_WINDOW_PROFILE=window-external`.
В новом чате можно выбрать установленный Codex, Claude или Pi. Профиль использует
общее хранилище чатов и загружает настройки исполнителей из своего каталога. В
стандартном профиле Linux `window` также доступен встроенный агент.

### Pi Agent

Установите CLI Pi с Node.js 22.19 или новее и настройте модель в терминале.

```sh
npm install -g @earendil-works/pi-coding-agent@1.6
pi
```

В новом чате выберите модель **Pi Agent** и режим **Full access** в меню
разрешений. Для этого харнеса HoH поддерживает Pi CLI 1.6.1 и новее в серии 1.6.
Pi выполняет команды с правами пользователя. Запуски из HoH отключают расширения
и MCP-серверы Pi, игнорируют настройки проекта, требующие доверия, и сохраняют
нативные сессии в профиле HoH.

## Лицензия

Права на оригинальный код и материалы HoH принадлежат Alexandrie.
Условия приведены в [LICENSE](LICENSE). Сторонние компоненты сохраняют
свои лицензии и уведомления. См. [NOTICE](NOTICE) и [ui/NOTICE.md](ui/NOTICE.md).
