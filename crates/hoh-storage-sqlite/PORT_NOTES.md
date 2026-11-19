# hoh-storage-sqlite ← packages/storage/storage-sqlite @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Config, backend/facet, eager open reservation, close и plugin `storage-sqlite` |
| src/schema.ts | src/schema.rs | создание пути/файла, pragmas, physical schema version и metadata tables |
| src/unit.ts | src/unit.rs | record/global JSON, version stamping, batch transactions, rollback quarantine |
| tests/sqlite-backend.spec.ts | tests/sqlite_backend.rs | 31 развёрнутый исходный сценарий: 30 проверены напрямую или согласованным Rust-триггером, hostile `toJSON` неприменим |
| ../../storage/tests/contract.ts | tests/contract_runner.rs | все 18 общих KV contract сценариев на реальном SQLite-файле |
| locale/*.json | locale/*.json | все 8 файлов без изменений |
| README.md, README.zh.md, package.json | — | прочитаны; отдельной LICENSE нет |

До переноса просмотрены разрешённые импорты packages/src. Пакет используется как plugin/provider; корневые `Config`, `SqliteStorageBackend`, `JournalMode`, `STORAGE_SQLITE_SCHEMA_VERSION` сохранены. Исходные subpath-экспорты доступны как `unit::SqliteKvUnit` и snake_case `schema::{open_database, record_table_name}`; observable aggregate cause — как `unit::RollbackFailure`. Неиспользуемые другими переносимыми пакетами экспорты: эти четыре subpath-типа/функции.

## Отличия от образца

- `DatabaseSync` заменён на `rusqlite 0.43.0` с workspace-feature `bundled` по решению 9 DEPS.md. Один `Connection` защищён mutex backend; операции выполняются синхронно до возврата ready `BackendFuture`, как синхронные SQLite-примитивы внутри исходных Promise. Busy timeout равен нулю. Новых внешних зависимостей крейт не добавляет.
- JavaScript `number` версии представлен `u64` из `hoh-storage`; перед SQLite INTEGER выполняется checked conversion в `i64`. Layout `single`/`per-record` намеренно не меняет document-per-row SQLite layout, как в образце.
- Обычный JS object результата заменён `IndexMap`. Array-index keys `0..=2^32-2` перечисляются первыми по возрастанию, остальные сохраняют порядок SQLite rows; `__proto__`/`constructor` остаются обычными ключами. JSON использует общие `js_json_parse/js_json_stringify`.
- Ошибки `StorageError` возвращаются напрямую для сохранения single-downcast contract. Retained `unknown-state` клонирует оболочку `StorageError`, но все клоны разделяют cause через `Arc`; полная JS identity Error не представима. `RollbackFailure` хранит обе причины в исходном порядке и отдаёт original как standard source.
- `rusqlite` не предоставляет экземплярный monkey-patch `exec`, поэтому три исходных сценария с искусственным отказом самого `ROLLBACK` представлены двумя white-box regressions: отдельно проверены обе причины aggregate и quarantine всех уже открытых units/new opens. Реальные `ABORT`, `ROLLBACK`, busy `BEGIN` и ошибки version update проверяются интеграционно. Completed open materialизуется синхронно и хранит готовый Result до await; close корректно переносит уже полученный отказ.
- Rust `serde_json::Value` не имеет исполняемого `toJSON`, циклов, `undefined` или non-Error throw. Поэтому hostile-toJSON сценарий неприменим; общий contract использует согласованный undeclared-table trigger. Проверка guard order дополнительно гарантирует, что `closed`/retained failure предшествуют table/global validation.
- `StorageSqlite` — дополнительный typed `ServiceKey` для динамического имени `storage.backend.sqlite`; registry и service получают тот же `Arc`. Dispose снимает регистрацию раньше close. Config сохраняет точные `path`, `journalMode` и default `wal`; неизвестные поля не запрещены.
- Пути разрешаются общим Node-compatible helper. На Unix новые директории/файл получают 0700/0600, существующий mode не меняется. `Connection::close` вызывается явно; результат первого close запоминается для повторных/concurrent close.

## Не перенесено

- Только исполнение hostile `toJSON` и оборачивание брошенной строки: тип `Value` такого состояния не допускает. Остальные 30/31 развёрнутых package-сценариев перенесены; три rollback-failure cases используют описанный white-box trigger, не пропущены по проверяемому контракту.
- npm exports/types и Schemastery как runtime-механизмы не переносятся; derive `Deserialize`/`JsonSchema` задаёт тот же config shape.

## Не проверено

- Windows permissions/path spelling и Windows-тексты filesystem errors. Linux проверяет EACCES, NUL, owner-only creation и сохранение существующего mode.
- Настоящие ENOSPC/EIO, повреждение процесса или питание не инъецировались. Проверены реальные SQLite transactions, два trigger action, lock contention, reopen и explicit close.
- Встроенная SQLite из `libsqlite3-sys` может отличаться версией от Node SQLite; используемые STRICT/pragma/transaction primitives покрыты тестами.

## Вопросы ведущему

Нет: драйвер одобрен решением 9 DEPS.md; API зависимостей достаточны, чужие крейты не менялись.

## Проверки

Все команды выполнены с `CARGO_BUILD_JOBS=2`, `CARGO_NET_OFFLINE=true`, `CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x4`.

- `timeout 300s cargo test --offline -p hoh-storage-sqlite`: unit — `3 passed; 0 failed`; shared contract — `18 passed; 0 failed`; sqlite_backend — `29 passed; 0 failed`; doc-tests — `0 passed; 0 failed`. Всего 50/0.
- `timeout 300s cargo clippy --offline -p hoh-storage-sqlite --all-targets --no-deps -- -D warnings`: exit 0.
- `cargo fmt --package hoh-storage-sqlite -- --check`: exit 0.
- `cmp` каждого из 8 locale-файлов с образцом, `git diff --check`, поиск `todo!/unimplemented!`: exit 0.
- Независимый read-only review не нашёл production-дефектов; по его замечаниям восстановлены полные assertions busy/version-update/repair, точный вид regex в diagnostics, Windows cfg тестов и описание subpath exports.
