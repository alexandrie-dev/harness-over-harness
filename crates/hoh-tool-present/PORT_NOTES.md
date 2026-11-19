# hoh-tool-present ← packages/deliverables/tool-present @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/index.ts` | `src/lib.rs` | `ToolPresent`, конфигурация, схема и исполнение `present`, публикация `deliverables/presented` после финального успешного результата |
| `src/types.ts` | `src/lib.rs` | `PresentedFile`, `PresentedEvent` с исходными JSON-полями |
| `tests/present.spec.ts` | `tests/present.rs`, unit test в `src/lib.rs` | все 10 объявлений |
| `tests/built-errors.e2e.ts` | — | исключён по PORTING §10: проверка собранного npm-пакета и Node loader |
| `locale/*.json` | `locale/*.json` | все 8 файлов без изменений |

Исходные README и package exports прочитаны. Runtime-потребители события и схемы проверены по
workspace; `deliverables/presented` уже входит в известный словарь `hoh-session`.

## Отличия от образца

- `Config.max_files` хранится как `f64`, чтобы сохранить приём `z.number()` и последующую проверку
  `Number.isSafeInteger`; предел — `Number.MAX_SAFE_INTEGER`. После проверки значение переводится
  в `usize` (workspace собирается на 64-битной платформе).
- JavaScript `path.trim()` перенесён через общий `hoh_util_values::js_trim`, включая U+FEFF и
  исключая U+0085.
- `WeakMap<ToolExecution, …>` представлен приватным `HashMap<ToolExecutionToken, …>`: token
  создаётся только `hoh-tools`, уникален для точного исполнения и приходит в том же
  `ToolsResult`. При штатном завершении запись удаляется до проверки финального результата.
  Если caller бросит pipeline future после успешного tool body, но до `complete`, `ToolsResult`
  не возникает и запись живёт до размонтирования плагина; TypeScript `WeakMap` освобождает её
  вместе с execution. Для точной очистки нужен lifecycle/drop hook владельца `hoh-tools`;
  чужой API в этом переносе не меняется.
- `SHIM(hoh-tools)`: текущий `ToolExecution.agent` предоставляет только scope и `Session`, тогда
  как настоящий `WorkingDirectoryService::ensure` принимает `hoh_agent::Agent`. Использован уже
  существующий публичный мост `hoh_working_directory::live_agent`; своего параллельного Agent API
  крейт не вводит. При отсутствии живой регистрации возвращается
  `ToolContextUnavailableError`.
- Событие Session записывается типизированным `PresentedEvent`, затем тем же JSON-объектом
  `turn/callId/files`; декларативное TypeScript module augmentation в Rust не требуется.

## Не перенесено

- `built-errors.e2e.ts`: npm build/export/Node smoke согласно PORTING §10.
- TypeScript-only declaration merging для `SessionEventMap`; runtime wire contract сохранён
  сериализуемым публичным типом.

## Не проверено

- npm/Node loader, JavaScript package exports и собранный bundle.
- Платформы кроме Linux; тест symlink использует Unix API, как текущая среда сборки.

## Вопросы ведущему

- После перевода `hoh-tools::ToolExecution.agent` на настоящий `hoh_agent::Agent` удалить
  отмеченный мост и передавать агент напрямую в `WorkingDirectoryService::ensure`.
- Добавить в `hoh-tools` terminal/drop lifecycle hook, чтобы Rust-переносы исходных `WeakMap`
  могли убрать execution-local состояние, даже если caller бросил pipeline до `ToolsResult`.

## Проверка

Все Cargo-команды выполнялись с `CARGO_NET_OFFLINE=true`, `CARGO_BUILD_JOBS=2`,
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x3`; тесты — под `timeout`.

- `cargo check -p hoh-tool-present --tests --offline`: успешно.
- `cargo test -p hoh-tool-present --offline`: 13 прошло, 0 упало; doc-tests 0/0.
- `cargo clippy -p hoh-tool-present --all-targets --no-deps --offline -- -D warnings`: успешно,
  предупреждений нет.
- `cargo fmt -p hoh-tool-present -- --check`: успешно, без вывода.
- Независимое read-only ревью fix-round: все четыре пункта исходного ревью закрыты,
  новых замечаний нет; команды ревьюер не запускал.
