# Подложенные входы supply gate

`python3 scripts/tests/ui-supply/run.py` запускает настоящий scripts/ui-supply-check.sh на отдельных временных деревьях, без сети. base.json — чистый контроль; case-*.json — сохранённые плохие входы. Время молодого релиза и привязка audit к хешу блокировки устанавливаются при запуске, поэтому сценарии не устаревают через неделю. Ни одна строка install/prepare не выполняется.

| Требование | Подложенный вход | Ожидаемый отказ |
|---|---|---|
| Хеш каждой версии | case-missing-hash.json | integrity: missing/invalid |
| Хеш совпадает с registry | case-changed-hash.json | integrity: registry mismatch |
| Выдержка ≥7 суток | case-young.json | age: younger than 7 days |
| Установочные скрипты | case-install-script.json, case-prepare-script.json | script: install hook |
| Проверка tarball manifest | case-archive-script.json | script: installed hook |
| Неявный node-gyp | case-binding-gyp.json | script: install hook/binding.gyp |
| Лицензии | case-license.json | license: forbidden |
| Известная уязвимость | case-vulnerability.json | audit: known vulnerability |
| Недоступность audit | case-audit-outage.json | audit: unavailable |
| Единственный источник архивов | case-foreign-tarball.json | source: foreign tarball |
| Не ослаблять возраст | case-age-config.json | config: minimumReleaseAge |
| Причина прямой зависимости в DEPS.md | case-missing-why.json | direct: missing documented reason |
| Автоматическая загрузка pnpm/runtime | case-self-manager.json, case-automatic-runtime.json | direct: automatic package manager/runtime downloads forbidden |
| Пустой allowBuilds | case-build-allowlist.json | config: allowBuilds |

Чистый контроль case-clean.json проходит и явно выводит `skipped: cargo-deny not installed`. Итого 17 сценариев: 16 отказов + 1 чистый контроль. Настоящий lock дополнительно остановлен реальным live pnpm audit (picomatch 4.0.7), отчёт в docs/ts/pnpm-audit.json.

Дополнительная проверка самого pnpm: `python3 scripts/tests/ui-supply/native-age.py` после первого resolve. Она использует уже разрешённую версию React и сохранённые метаданные, отключает сеть и получение архивов, но подставляет усиленный возрастной порог из native-age.yaml. 2026-11-18 получен `ERR_PNPM_NO_MATURE_MATCHING_VERSION`, где pnpm явно называет minimumReleaseAge.

## Исключение CEO 2026-11-18 и чистое окружение

Тот же `python3 scripts/tests/ui-supply/run.py` дополнительно запускает:

- `environment.py`: подставная печать окружения вместо сборки, прямой offline runner, package scripts и Tauri beforeBuildCommand. VP_RUN_NODE_CLIENT_PATH, NODE_OPTIONS, чужие PATH/HOME, npm/Python-настройки и секрет не наследуются; разрешённые литералы доходят, HOME удаляется, IP запрещён. Фиксированные профили существующих window/Cargo проверяются только печатью окружения, без запуска этих программ.
- `exceptions.py` + `exceptions/base.json` + `exceptions/case-*.json`: 27 сценариев, включая разрешённую пару, другой GHSA/пакет/версию, дополнительную находку, runtime-путь по audit и по lock, обход Vite, расхождение путей, лишний счётчик, истечение/продление срока, исправление офлайн/refresh/registry, исчезновение отозванной находки и удаление исключения. Проверяются также stale/непривязанный audit и код ошибки pnpm вне обычного 1. Refresh работает с локальным печатающим JSON stub и свежим локальным metadata cache, под seccomp-запретом сети. Сохранённый audit не очищается исключением.
- Контроль настоящего lock на изолированной копии существующих метаданных и audit: peer-контексты и все девять build-only путей проходят, настоящий package manager не вызывается. Чтобы сценарии не зависели от дня запуска, gate-at-ceo-date.py исполняет production Python-код обёртки с тестовыми часами, включая 02/03.12.2026 для границы срока, временная копия настоящего audit получает тестовый timestamp. В production подмены часов нет: свежесть audit ≤24 часов и разрешение CEO до 2026-12-02 проверяются по настоящему времени.
- `artifacts.py`: 8 проверок на подложенных dist/binary. Чистые файлы проходят; picomatch в JS, source map, бинарнике, на границе блоков и в UTF-16 отклоняется; отсутствующий бинарник также отклоняется. Запускается только `verify.sh --window-artifacts-only`, до report/cargo-фаз.

Результат 18.11.2026: 54 PASS. Ни одной настоящей сборки, установки, загрузки или запуска браузера. Для синтаксиса: `bash -n scripts/ui-supply-check.sh scripts/verify.sh ui/scripts/build.sh`.
