# Основа окна HoH

> Текущий пакет 5: [чат варианта A и поведенческие проверки](tests/chat/README.md). Окно теперь открывает только чат; команды старого snapshot ниже описывают прежнюю пустую оболочку и не входят в текущую приёмку. Интеграция FirstRun выполнена; ожидаемый native bridge отмечен в [PORT_NOTES.md](PORT_NOTES.md).

Пакеты работ 8 и 1 из `docs/window.md`. **Статус: npm gate намеренно красный из-за high-уязвимости picomatch 4.0.7, исправление ещё не опубликовано. React-сборка и эталонный снимок не выполнены.** Модель и движок не подключаются. Поле ввода отключено, навигация показывает пустые разделы, кнопка солнца переключает тему. Типы `src/protocol.ts` принадлежат писателю ui1.

Нужны уже установленные pnpm 12.21.0, Node >=20.19, Python >=3.11 с системным PyYAML, libseccomp, Chromium, Rust 1.97.1, WebKitGTK 4.1 и xvfb. Инструменты глобально не устанавливаются. Python-зависимости не скачиваются.

Из корня репозитория, только получение пакетов использует внешнюю сеть:

```sh
bash ui/scripts/acquire.sh
```

Этот шаг разрешает первую блокировку (если её ещё нет), проверяет метаданные ВСЕХ её пакетов, делает `pnpm audit`, затем получает архивы по замороженной блокировке с отключёнными скриптами и optional. Последующие установки и сборки всегда офлайн. Все архивы npm только с registry.npmjs.org, выдержка ≥10080 минут. Каждый обязательный Linux x64 glibc backend указан в DEPS.md как прямой пакет; все optional игнорируются ещё при разрешении. pnpm 12 не применяет рабочие настройки из старого .npmrc: действующие настройки продублированы в pnpm-workspace.yaml и проверяются через config list. Версия закреплена engines, чтобы поле packageManager не включало загрузку самого pnpm.

```sh
scripts/ui-supply-check.sh
python3 scripts/tests/ui-supply/run.py
pnpm -C ui run build
pnpm -C ui run snapshot
```

Gate по умолчанию использует метаданные и отчёт audit из локального кэша. Отчёт привязан к SHA256 блокировки и действителен 24 часа. Нет данных, изменилась блокировка, устарел audit — отказ; обновление только явным `--refresh`. Если установлен cargo-deny, он запускается без обновления RustSec (GitHub-сеть в этом задании запрещена); отсутствие cargo-deny явно отмечается как пропуск. Сборщик наследует seccomp-запрет IPv4/IPv6 для всех подпроцессов. Путь XDG_RUNTIME_DIR под /tmp нужен pnpm для локальных operation locks, иначе системный /run/user read-only в песочнице.

Эталон — `tests/reference/shell.png`, 1200×800, DPR=1, Chromium этой машины, Inter из npm. Для намеренного обновления после визуального просмотра: `pnpm -C ui run snapshot -- --update-reference`. Обычная проверка эталон не меняет; PNG сравниваются побайтно. Chromium работает с временным HOME, ресурсы открываются по file://, удалённые запросы страницы блокируются и IPv4/IPv6 запрещены через seccomp, браузеры не скачиваются.

Rust — отдельный workspace `apps/hoh-desktop`, его корневая блокировка не связана с движком. Получение и сборка:

```sh
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-ui2 CARGO_HOME=/mnt/toshiba/alexandrie/hoh/cargo-home-ui2 cargo fetch --locked --target x86_64-unknown-linux-gnu --manifest-path apps/hoh-desktop/Cargo.toml
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-ui2 CARGO_HOME=/mnt/toshiba/alexandrie/hoh/cargo-home-ui2 python3 ui/scripts/offline.py cargo build --locked --offline --manifest-path apps/hoh-desktop/Cargo.toml
apps/hoh-desktop/scripts/run-probe.sh
```

Probe запускает реальное окно Tauri с временным HOME под xvfb и без IPv4/IPv6. Результат DOM снимается нативным сигналом заголовка; привилегированных JS-команд нет. CSP не разрешает внешние адреса, inline script/style или eval; capability main пуста. Инициализация движка, фильтр IPC и окно подтверждения — следующий пакет, здесь их нет.
