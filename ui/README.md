# Основа окна HoH

Из корня рабочего дерева на Linux x86-64 (glibc):

```sh
scripts/build-linux.sh prepare
bash ui/scripts/build.sh
bash ui/scripts/build.sh --check
```

Подготовка использует Node.js 22.13+ и pnpm 12.21.0. Она проверяет пакеты lockfile до загрузки архивов. Офлайн-сборка проверяет локальный аудит, манифесты установленных пакетов и готовые файлы интерфейса. Аудит действует 24 часа и привязан к SHA256 lockfile. Для обновления только UI после подготовки используйте `bash ui/scripts/acquire.sh`. Чтобы записать в репозиторий метаданные и отчёт для изменённого lockfile, используйте `bash ui/scripts/acquire.sh --record`.

`--check` дополнительно запускает проверку типов, unit-тесты и браузерные сценарии. Chrome нужен на этом этапе. `pnpm -C ui run build` и `pnpm -C ui run check` вызывают те же команды.

## Прежние снимки и WebKitGTK probe

Эталон — `tests/reference/shell.png`, 1200×800, DPR=1, Chromium этой машины, Inter из npm. Для намеренного обновления после визуального просмотра: `pnpm -C ui run snapshot -- --update-reference`. Обычная проверка эталон не меняет; PNG сравниваются побайтно. Chromium работает с временным HOME, ресурсы открываются по file://, удалённые запросы страницы блокируются и IPv4/IPv6 запрещены через seccomp, браузеры не скачиваются.

Rust — отдельный workspace `apps/hoh-desktop`, его корневая блокировка не связана с движком. Получение и сборка:

```sh
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-ui2 CARGO_HOME=/mnt/toshiba/alexandrie/hoh/cargo-home-ui2 cargo fetch --locked --target x86_64-unknown-linux-gnu --manifest-path apps/hoh-desktop/Cargo.toml
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-ui2 CARGO_HOME=/mnt/toshiba/alexandrie/hoh/cargo-home-ui2 python3 ui/scripts/offline.py cargo build --locked --offline --manifest-path apps/hoh-desktop/Cargo.toml
apps/hoh-desktop/scripts/run-probe.sh
```

Probe запускает реальное окно Tauri с временным HOME под xvfb и без IPv4/IPv6. Результат DOM снимается нативным сигналом заголовка; привилегированных JS-команд нет. CSP не разрешает внешние адреса, inline script/style или eval; capability main пуста. Инициализация движка, фильтр IPC и окно подтверждения — следующий пакет, здесь их нет.
