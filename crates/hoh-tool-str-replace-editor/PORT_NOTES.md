# hoh-tool-str-replace-editor ← packages/fs/tool-str-replace-editor

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | plugin, config, схема и четыре команды `view`/`create`/`str_replace`/`insert` |
| `tests/tools.spec.ts` | `tests/tools.rs` | все 16 исходных `it` |
| `locale/*.json` | `locale/*.json` | восемь файлов скопированы без изменений |
| `README.md`, `README.zh.md`, `package.json` | эти заметки и `Cargo.toml` | контракт и публичные экспорты учтены |

## Отличия от образца

- `ToolStrReplaceEditor` — native plugin с исходным row name `tool-str-replace-editor`; `Deps`
  импортирует только `tools` и `fs`. Опциональный `sandboxPolicy` читается по принятому ключу
  `hoh-sandbox-policy`, только когда backend сообщает confinement.
- `maxOutputChars` хранится как `f64` на конфигурационной границе, чтобы сохранить исходный отказ
  дробных, неположительных и unsafe значений; после проверки предел становится `u64`. Усечение и
  сравнение путей считают UTF-16 code units, как JavaScript. Попавшая внутрь surrogate pair граница
  отбрасывает всю пару через общий `hoh-output-retention`.
- Неизменяемые Rust-строки не представляют одиночный surrogate. Остальные модельные строки,
  schema descriptions, результаты и ошибки перенесены дословно. Literal replacement использует
  byte offsets только после точного UTF-8 поиска; это сохраняет те же подстроки и номера строк.
- `presentCall` не публикует `locations[].line`, когда вычисленная строка не помещается в `u64`.
  Это устраняет прежнее ложное насыщение до `18446744073709551615` для входа `1e20`. Точно
  сохранить такой JS `number` без правки чужого API нельзя: `hoh_tools::FileLocation.line` имеет
  тип `Option<u64>`. Исполнение всё равно отклоняет небезопасный/вне-диапазонный `insert_line`;
  для всех представимых schema-valid целых строка совпадает с
  `Math.max(1, insert_line + 1)` образца. На невалидном дробном входе TypeScript presentation
  сохраняет дробь, а Rust presentation усекает её из-за того же `u64`; выполнение обеих версий
  отклоняет такой аргумент по integer-схеме.
- `map_error` при невозможной комбинации `FS_SANDBOX_DENIED` без resolved policy возвращает
  исходную ошибку, тогда как TypeScript попытался бы прочитать `policy.mode` и бросил `TypeError`.
  Ветка недостижима в штатной композиции: confined backend без `SandboxPolicy` отклоняется при
  запуске plugin, а backend без confinement не выдаёт sandbox denial.
- Вспомогательные строки `TRUNCATED_MESSAGE` и `DEFAULT_DESCRIPTION` оставлены приватными, как
  неэкспортируемые константы TypeScript. Их отсутствие в публичном API закреплено compile-fail
  doctest.
- В тестах локальный backend обёрнут узким provider-spy для исходных подмен `stat`, `listDir` и
  `writeText`. Read-before-edit проверяет настоящий `hoh-fs-observation-policy`; sandbox-проверка
  использует настоящий `hoh-sandbox-policy`, а provider-spy проверяет переданную policy и
  возвращает тот же `FS_SANDBOX_DENIED`.

## Не перенесено

Нет: перенесены единственный source-файл, все 16 тестовых объявлений и все locale-файлы.

## Не проверено

- Windows/macOS spelling абсолютных путей: текущий HoH filesystem stack и задание Linux-only;
  здесь используется общий POSIX `node_path::is_absolute`.
- Композиция именно с `hoh-fs-sandbox` до удаления его собственного service-key shim; передача
  настоящей resolved policy в confining provider проверена эквивалентным provider-spy.

## Вопросы ведущему

Нет.

## Проверка

Все cargo-команды запускаются с `CARGO_BUILD_JOBS=2`, `CARGO_NET_OFFLINE=true`,
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x7` и внешним `timeout` для тестов.

- `cargo test -p hoh-tool-str-replace-editor --offline`: 17 passed, 0 failed (16 integration +
  1 compile-fail doctest); unit 0/0.
- `cargo clippy -p hoh-tool-str-replace-editor --all-targets --no-deps --offline -- -D warnings`:
  exit 0.
- `cargo fmt -p hoh-tool-str-replace-editor -- --check`: exit 0.
- Независимые read-only review-круги сверили эталон и крейт; замечания к API и независимости
  surrogate-oracle закрыты, reviewers проверки не запускали.
