# HarnessOverHarness

HoH состоит из движка `hoh` и окна `hoh-desktop` на Tauri. Окно запускает движок с профилем `window`. Движок и окно собираются из одного checkout, каждый со своим `Cargo.lock`.

## Сборка на Linux

Понадобятся Rust 1.97.1, C toolchain, pkg-config, заголовки GTK 3 и WebKitGTK 4.1, Node 20.19+, pnpm 12.21.0, Python 3 с PyYAML и libseccomp, а также заполненные Cargo-кэш и pnpm store. В ветках с T3-клиентом проверки UI используют установленный Chrome. Node используется при сборке интерфейса.

UI-сборщик очищает окружение и читает пути из [build-env.allow](ui/scripts/build-env.allow), [обёртки pnpm](ui/scripts/tools/pnpm) и [pnpm-workspace.yaml](ui/pnpm-workspace.yaml). В репозитории записаны пути сборочной машины. На другой машине настройте эти пути в своём рабочем дереве. Получение UI-пакетов и обновление аудита с доступом к сети выполняются отдельно через [acquire.sh](ui/scripts/acquire.sh). Для офлайн-сборки нужен отчёт pnpm audit не старше 24 часов, привязанный к SHA256 текущего `ui/pnpm-lock.yaml`. Если он уже получен для этого lockfile, поместите JSON-отчёт в `ui/.supply-cache/audit.json`. При отсутствии локального файла проверка читает `docs/ts/pnpm-audit.json`. При устаревшем или несовпадающем отчёте сборка остановится.

Из корня чистого рабочего дерева экспортируйте `CARGO_HOME` с кэшем для обоих `Cargo.lock` и `HOH_BUILD_DIR` на выделенном томе, оба абсолютными путями с ограничением размера. Для общего хоста нужен свободный слот cargo и минимум 15 ГиБ на `/`. Одна команда собирает UI, движок и окно:

```bash
bash -euc '
  : "${CARGO_HOME:?Укажите каталог offline-кэша Cargo}"
  : "${HOH_BUILD_DIR:?Укажите каталог сборки на выделенном томе}"
  test "$(df -k --output=avail / | tail -1)" -ge 15728640
  bash ui/scripts/build.sh
  test "$(df -k --output=avail / | tail -1)" -ge 15728640
  export CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
  CARGO_TARGET_DIR="$HOH_BUILD_DIR/engine" cargo build --locked --offline --release -p hoh
  test "$(df -k --output=avail / | tail -1)" -ge 15728640
  HOH_ENGINE_PATH="$HOH_BUILD_DIR/engine/release/hoh" CARGO_TARGET_DIR="$HOH_BUILD_DIR/desktop"     cargo build --locked --offline --release --manifest-path apps/hoh-desktop/Cargo.toml
'
```

## Запуск

В графической сессии разработки запустите окно с собранным движком:

```bash
HOH_ENGINE_PATH="$HOH_BUILD_DIR/engine/release/hoh" "$HOH_BUILD_DIR/desktop/release/hoh-desktop"
```

Окно хранит настройки, журнал и данные движка в каталогах приложения `local.hoh.desktop`. Команда использует данные текущей графической сессии. Для приёмки задайте отдельные HOME и XDG-каталоги, отдельную шину без активации пользовательских служб и отключите звук. На общем хосте проводите приёмку с одним Xvfb под общим `flock`. Для реальных Claude и Codex нужны соответствующие авторизованные CLI.

- [Конфигурация окна и запуск движка](apps/hoh-desktop/PORT_NOTES.md)
- [Зависимости и проверка поставки UI](docs/ts/DEPS.md)
