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
| Псевдоним `npm:` в lock проверяется по целевому пакету (есть в lock) | case-alias-clean.json | проходит |
| Псевдоним `npm:` на отсутствующий в lock пакет | case-alias-missing.json | lock: missing dependency |
| Лицензия целевого пакета псевдонима проверяется как у обычного | case-alias-target-license.json | license: forbidden |
| Исключение лицензии: точный name@version, точная строка, непустая причина | case-license-exception.json | проходит |
| Исключение лицензии не действует на другую версию | case-license-exception-other-version.json | license: forbidden |
| Исключение лицензии не действует на другую строку лицензии | case-license-exception-other-license.json | license: forbidden |
| Исключение лицензии без причины | case-license-exception-no-reason.json | license: forbidden |
| Установленный пакет: лицензия из исключения на этот name@version | case-installed-license-exception.json | проходит |
| Установленный манифест с лицензией вне списка и вне исключения | case-installed-license-no-exception.json | license: installed |
| Установленный манифест назван чужим именем (и берёт чужие исключения) | case-installed-swapped-name.json | installed: manifest differs |
| Установленный манифест с чужой версией | case-installed-swapped-version.json | installed: manifest differs |
| Установленный пакет вне lock | case-installed-not-in-lock.json | installed: not in lock |
| Неразбираемое имя каталога в node_modules/.pnpm | case-installed-unexpected-entry.json | installed: unexpected store entry |
| Причина исключения null | case-license-exception-reason-null.json | license: forbidden |
| Исключение без строки license (и пакет без лицензии) | case-license-exception-no-license-field.json | license: forbidden |
| Псевдоним на другой пакет, чистый / с плохой лицензией цели | case-alias-other-clean.json, case-alias-other-license.json | проходит / license: forbidden target@1.0.0 |
| Псевдоним на scoped-цель, чистый / с плохой лицензией | case-alias-scoped.json, case-alias-scoped-license.json | проходит / license: forbidden @scope/target@1.0.0 |
| Псевдоним с peer-суффиксом ссылки | case-alias-peer-suffix.json | проходит |
| Псевдоним на git/file:/link: значение | case-alias-git.json, case-alias-file.json, case-alias-link.json | lock: missing dependency |
| Вложенный реальный пакет с install-скриптом в установленном каталоге | case-installed-nested-script.json | script: installed hook |
| Реальный сосед в node_modules каталога с install-скриптом | case-installed-neighbour-script.json | script: installed hook |
| Вложенный реальный пакет вне lock | case-installed-nested-not-in-lock.json | installed: not in lock |
| Реальный каталог без манифеста | case-installed-unexpected-dir.json | installed: unexpected entry |
| Патч через симлинк за пределы ui/ | case-patch-symlink-outside.json | patch: file outside ui/ |
| Запись с точкой (не `.bin`) в node_modules вне lock | case-installed-dot-entry.json | installed: not in lock .evil |
| Scoped-сосед вне lock | case-installed-scoped-neighbour.json | installed: not in lock @scope/ghost |
| Симлинк за пределы .pnpm | case-installed-symlink-outside.json | installed: symlink leaves the store |
| Симлинк на каталог, чей ключ вне lock | case-installed-symlink-unknown.json | installed: symlink to unknown package |
| node_modules пакета сам симлинк | case-installed-node-modules-symlink.json | installed: node_modules is a symlink |
| .bin: симлинк или shim с целью вне .pnpm | case-installed-bin-symlink-outside.json, case-installed-bin-shim-outside.json | installed: .bin/tool leaves the store |
| Допустимые симлинк в .pnpm и .bin (симлинк, shim с `cmd-shim-target`) | case-installed-links-clean.json | проходит |
| patchedDependencies: файл, хэш (sha256, CRLF→LF) и набор workspace = lock | case-patch-clean.json | проходит |
| Изменённый файл патча | case-patch-changed-file.json | patch: hash differs |
| Лишний патч в workspace | case-patch-extra-in-workspace.json | patch: workspace and lock patchedDependencies differ |
| Файл патча отсутствует | case-patch-missing-file.json | patch: missing file |
| Ключ патча не name@version из lock | case-patch-not-in-lock.json | patch: ... is not an exact name@version in the lock |

Чистый контроль case-clean.json проходит и явно выводит `skipped: cargo-deny not installed`. Итого 58 сценариев: 49 отказов и 9 проходящих (контроли: case-installed-links-clean, case-clean, case-alias-clean, case-alias-other-clean, case-alias-scoped, case-alias-peer-suffix, case-license-exception, case-installed-license-exception, case-patch-clean). Настоящий lock дополнительно остановлен реальным live pnpm audit (picomatch 4.0.7), отчёт в docs/ts/pnpm-audit.json.

Дополнительная проверка самого pnpm: `python3 scripts/tests/ui-supply/native-age.py` после первого resolve. Она использует уже разрешённую версию React и сохранённые метаданные, отключает сеть и получение архивов, но подставляет усиленный возрастной порог из native-age.yaml. 2026-11-18 получен `ERR_PNPM_NO_MATURE_MATCHING_VERSION`, где pnpm явно называет minimumReleaseAge.

## Исключение CEO 2026-11-18 и чистое окружение

Тот же `python3 scripts/tests/ui-supply/run.py` дополнительно запускает:

- `environment.py`: подставная печать окружения вместо сборки, прямой offline runner, package scripts и Tauri beforeBuildCommand. VP_RUN_NODE_CLIENT_PATH, NODE_OPTIONS, чужие PATH/HOME, npm/Python-настройки и секрет не наследуются; разрешённые литералы доходят, HOME удаляется, IP запрещён. Фиксированные профили существующих window/Cargo проверяются только печатью окружения, без запуска этих программ.
- `exceptions.py` + `exceptions/base.json` + `exceptions/case-*.json`: 29 сценариев (из них case-alias-edge: псевдоним зависимости на другом ребре графа, который обход путей picomatch обязан пройти по настоящему имени), включая разрешённую пару, другой GHSA/пакет/версию, дополнительную находку, runtime-путь по audit и по lock, обход Vite, расхождение путей, лишний счётчик, истечение/продление срока, исправление офлайн/refresh/registry, исчезновение отозванной находки и удаление исключения. Проверяются также stale/непривязанный audit и код ошибки pnpm вне обычного 1. Refresh работает с локальным печатающим JSON stub и свежим локальным metadata cache, под seccomp-запретом сети. Сохранённый audit не очищается исключением.
- Контроль настоящего lock на изолированной копии существующих метаданных и audit: peer-контексты и все девять build-only путей проходят, настоящий package manager не вызывается. Чтобы сценарии не зависели от дня запуска, gate-at-ceo-date.py исполняет production Python-код обёртки с тестовыми часами, включая 02/03.12.2026 для границы срока, временная копия настоящего audit получает тестовый timestamp. В production подмены часов нет: свежесть audit ≤24 часов и разрешение CEO до 2026-12-02 проверяются по настоящему времени.
- `artifacts.py`: 8 проверок на подложенных dist/binary. Чистые файлы проходят; picomatch в JS, source map, бинарнике, на границе блоков и в UTF-16 отклоняется; отсутствующий бинарник также отклоняется. Запускается только `verify.sh --window-artifacts-only`, до report/cargo-фаз.

Результат 18.11.2026: 54 PASS. Ни одной настоящей сборки, установки, загрузки или запуска браузера. Для синтаксиса: `bash -n scripts/ui-supply-check.sh scripts/verify.sh ui/scripts/build.sh`.

## Исключения политики (лицензии, скрипты) и установленные пакеты

Решение CEO 20.11.2026: `licenseExceptions` (dompurify@3.4.17) и `scriptExceptions` (prepare) действуют только на точный name@version, только с точной строкой лицензии и непустой строковой причиной. Для установленных пакетов исключение выдаётся по имени каталога `node_modules/.pnpm/<name>@<version>[_контекст]`, который обязан быть ключом lock; манифест внутри обязан совпасть с этим именем и версией. Реальные (не симлинк) соседи в `node_modules` каталога и вложенные `node_modules` тоже проверяются: манифест должен быть ключом lock, иначе отказ. Симлинк в `node_modules` обязан вести в `.pnpm/<каталог с ключом из lock>/node_modules/<name>`; симлинкнутый `node_modules` и записи с точкой, кроме `.bin`, отказ; в `.bin` допустимы только симлинки и shim pnpm с `cmd-shim-target` внутри `.pnpm`. patchedDependencies проверяются: набор в workspace и в lock, файл, хэш и ключ name@version из lock.

Не проверяется сознательно (P2-3 ревью, решение владельца): содержимое самой политики в DEPS.md. Расширение `licenseExceptions` или `scriptExceptions` правкой DEPS.md гейт принимает; такая правка видна в diff и принимается владельцем. Overrides workspace и lock гейт не сверяет, набор патчей в политике DEPS.md не закреплён (проверяются согласованность workspace, lock и файлов). Всё это решение ведущего: политика DEPS.md и lock под контролем владельца, как и P2-3.

Окружение сборки окна: Node 22.23.1 (`ui/scripts/build-env.allow`, ожидание в environment.py); pdfjs-dist и commander требуют Node не ниже 22. В worktree environment.py всё равно сверяет PATH с каталогом `ui/scripts/tools` текущего корня, а allow-файл называет путь основного дерева, поэтому там он проходит только из `/home/save/Alexandrie/HarnessOverHarness`.
