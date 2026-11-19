# hoh-tool-workspace-dependencies ← packages/skill/tool-workspace-dependencies @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/error.ts` | `src/error.rs` | Три кода и подкласс `WorkspaceDependencyError` перенесены по PORTING §7 |
| `src/index.ts` | `src/lib.rs`, `src/runtime.rs`, `src/shim.rs` | Manifest, пути, alias, copy/install/resolve, memoized tool и lifecycle перенесены |
| `tests/primary-runtime-copy.spec.ts` | `tests/primary_runtime_copy.rs` | Три группы: layouts, пять плохих alias, legacy regular command |
| `tests/tool-workspace-dependencies.spec.ts` | `tests/tool_workspace_dependencies.rs` | 30 из 35 групп; табличные варианты объединены в циклы |
| `locale/*.json` | `locale/*.json` | 8 файлов без изменений (`diff -rq` — exit 0) |
| `README*.md`, `package.json` | `Cargo.toml`, этот файл | Контракты payload и model-facing текста учтены |

Всего перенесено 33 из 38 исходных `it`/`it.each`-групп. Rust содержит 27
test-функций, включая review-регрессии; параметризованные варианты manifest,
platform, entry kind и alias проверяются циклами.

## Отличия от образца

- Имена функций и полей Rust — snake_case; `undefined` представлен `Option`, карты
  distribution — `IndexMap`, чтобы порядок ключей сохранялся в model-facing JSON.
  Поля результата сериализуются в фактическом порядке литерала образца:
  `python`, `node`, `nodePackages`, `pnpm`, `pythonPackages`,
  `pythonDistributions`; порядок output schema намеренно остаётся отдельным,
  как в его литерале образца.
- `WorkspaceDependencyError` следует общему правилу ошибок: пустой тип-конструктор
  возвращает `hoh_error::HarnessError` с именем и кодом образца. Ошибка относительного
  config наследует простой `Error`, поэтому имеет отдельный crate-local тип.
- Все видимые JSON-тексты идут через `js_json_parse`/`js_json_stringify[_pretty]`.
  Файловые ошибки форматируются как Node через `node_fs_error_message`; приватная
  обёртка сохраняет исходный `io::Error` в цепочке причины.
- `fs.cp({dereference:true})` реализован асинхронным обходом `tokio::fs`: имена
  сортируются по байтам, обычные links материализуются, два проверенных Python alias
  восстанавливаются относительными symlink. Цикл directory links отвергается до
  бесконечного обхода; его fallback-текст Rust отличается от системного Node `ELOOP`.
- Memoized preparation создаётся до возврата execute-future. Один worker регистрируется
  до инструмента; teardown сначала снимает инструмент, затем ждёт уже начатую операцию
  без отмены, как исходный Promise. Параллельные вызовы разделяют attempt, успех
  остаётся cached, ошибка допускает retry.
- Сравнение установленного и исходного manifest выполняется через JS stringify
  нормализованных структур, поэтому учитывает и порядок distribution-ключей.

## Не перенесено

Пять тестовых групп Vitest, основанных только на `vi.mock` системного модуля, не
имеют production-входа в Rust: искусственные EACCES для `lstat` alias; EACCES для
`readFile`/`stat`; EACCES первого installation `lstat`; отказ выбранного `rename`;
остановка/ошибка выбранного `cp` для наблюдения disposal. Последняя группа содержит
success и failure: production effect ждёт уже начатый `Attempt`, но без управляемой
паузы внутри copy тест не может доказать промежуточное состояние детерминированно;
тест на большой payload был бы зависящим от скорости машины. Реальные соседние пути
проверены: missing/malformed JSON с cause, плохие entry types, symlink-защита,
неполный replacement с сохранением прежнего дерева, retry и unused disposal.

## Не проверено

- Исполнением проверены Linux и POSIX symlink. Ветки macOS/Windows не запускались.
- Windows host `node:path` временно реализован приватным shim; mapping `windows →
  win32`, drive root и UNC share root покрыты чистыми тестами, но полной fixture-сверки
  с Node нет. Linux path и все model-facing строки проверены.
- Ошибки ENOSPC/EIO и rollback, у которого отказал также обратный `rename`, не
  воспроизводились на настоящей файловой системе.
- `fs.cp({dereference:true})` сохраняет режим исходных каталогов, а Rust-обход
  создаёт их с обычным umask. Кроме того, staging образца создаётся `mkdtemp` с
  режимом `0700`, тогда как Rust staging обычно получает `0755`; model-facing
  данные не меняются, но установленное дерево может быть доступнее другим
  локальным пользователям.
- FIFO внутри недоверенного payload образец отклоняет как `ERR_FS_CP_FIFO_PIPE`.
  `tokio::fs::copy` может ждать открытия FIFO на blocking-потоке; disposal при этом
  также ждёт preparation. Payload от поставщика не должен содержать special files.
- Похожие на array index ключи distributions не являются отличием:
  `js_json_stringify[_pretty]` из `hoh-util-values` ставит их первыми по возрастанию,
  как `JSON.stringify`; это применяется и к сравнению manifest, и к тексту tool.

## Вопросы ведущему

1. `hoh-util-workspace-path::node_path` предоставляет только POSIX. Нужен общий
   `path.win32` API; после него удалить `SHIM(hoh-util-workspace-path)` из `src/shim.rs`.
2. Нужен ли общий async filesystem fault-injection seam для точного переноса пяти
   mock-only тестов (`read/lstat/stat/cp/rename`) без глобального тестового состояния?

## Проверки

Команды используют `CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w5`,
`CARGO_BUILD_JOBS=2`, `CARGO_NET_OFFLINE=true`, `--offline`; тесты — под `timeout`.

- `timeout 300s cargo test -p hoh-tool-workspace-dependencies --offline --no-fail-fast`:
  результат последнего круга указан в отчёте писателя.
- `cargo clippy -p hoh-tool-workspace-dependencies --all-targets --no-deps --offline -- -D warnings`
  и `cargo fmt -p hoh-tool-workspace-dependencies -- --check`: результаты последнего
  круга указаны в отчёте писателя.
