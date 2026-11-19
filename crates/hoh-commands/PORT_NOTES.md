# PORT_NOTES — hoh-commands

Источник: `packages/interaction/commands` из read-only дерева DeepSeek Harness. Порт выполнен без исполнения TypeScript и без изменений исходника.

## Карта файлов

| TypeScript | Rust | Примечание |
|---|---|---|
| `src/brand.ts` | `src/brand.rs` | `CommandDefinitionId`, `CommandId` сохранены разными branded newtype |
| `src/builtin.ts` | `src/builtin.rs` | Все шесть definition id, английские/китайские токены и приоритет exact-name сохранены |
| `src/types.ts` | `src/types.rs` | Descriptor, definition, invocation, submit attachment, execution и result представлены закрытыми Rust-типами |
| `src/index.ts` | `src/lib.rs` | Scoped registry, change event, parse/execute, lifecycle и admission вложений |
| `tests/builtin.spec.ts` | `tests/builtin.rs` | Оба именованных случая |
| `tests/commands.spec.ts` | `tests/commands.rs`, `tests/common/mod.rs` | Поведенческие сценарии registry/execute/attachments |
| `locale/{de,en,es,fr,ja,pt,ru,zh}.json` | одноимённые файлы в `locale/` | Перенесены дословно |
| `README.md`, `README.zh.md`, `package.json`, `README.i18n.yaml` | этот файл и `Cargo.toml` | Прочитаны; README не копируется |

## Сохранённые контракты

- Имя сервиса/плагина и события: `commands`, `commands/change`.
- Grammar команды ASCII-точная: `^/[a-z][a-z0-9_-]*(?=$|[\t\n\r ])`; `raw_input` не trim/normalize.
- Глобальный слой и точные agent-scope shadows используют `hoh-scope::ScopedLayers`; сортировка discovery идёт по имени, duplicate diagnostics сохранены.
- Уведомления не являются veto: panic/error каждого observer контейнируется ядром и логируется, остальные observers продолжают работу.
- `command/run` создаётся до handler, `command/done` — при success, ожидаемой ошибке, abort и ошибке handler/admission. Syntax/unknown/pre-abort ничего не пишут. Идентификатор имеет форму `cmd-<8 символов UUID>-<монотонный номер>`.
- `record_input = Some(false)` единственный отключает запись `args`.
- Изображения и файловые receipt сначала проходят policy/admission, файловые receipt разрешаются до записи изображений, исходный смешанный порядок восстанавливается. Expected `AttachmentError` превращается в `CommandResult::Error`, прочие ошибки пробрасываются после `command/done`.
- Resolver receipt имеет одного владельца; его `EntryUndo` точный и идемпотентный.
- Cancel до admission не открывает lifecycle; cancel после асинхронной admission и до handler закрывает пару ошибкой и не вызывает handler.
- Panic обработчика (как при синхронном вызове, так и при poll возвращённого future) и resolver-а receipt контейнируется и закрывает `command/run` парным `command/done kind:error`, как синхронный throw источника.

## Rust-адаптации и сознательные расхождения

- `CommandRuntime::register` принимает явный `&Context`: это Rust-представление source `this.ctx`, необходимое для выбора caller scope. Handler и resolver хранятся как `Arc<dyn Fn... + Send + Sync>`; handler возвращает `BoxFuture<Result<CommandResult, BoxError>>`.
- Возвращаемый `hoh_kernel::Disposer` имеет consuming API `dispose(self)`: повторный `dispose(); dispose()` из `commands.spec.ts:139-140` статически непредставим. Однократность teardown обеспечена владением, а тест проверяет два уведомления — при регистрации и единственном снятии — и исчезновение команды из `find`.
- `Agent` в рабочем пространстве — `Arc<dyn Agent>`, поэтому invocation владеет тем же `Arc`. Массив blocks — `Arc<[ContentBlock]>`; immutable `Object.freeze` обеспечивается владением, закрытыми полями wire-типа и отсутствием разделяемой mutable коллекции.
- Source union `ImageBlock | FileBlock` представлен открытым `hoh_llm::ContentBlock`, потому что именно этот workspace-тип используют model/session boundaries. Создание blocks остаётся только image/file.
- `CommandResult`, `CommandInputDescriptor` и `CommandDefinition` — типизированные Rust-значения. Поэтому JavaScript boundary-проверки non-string `description`/`hint`, non-function handler, non-boolean attachments, `undefined`/`null`/unknown result kind, non-string success text и unsafe/negative `sourceEventSeq` непредставимы без `unsafe` либо второго JSON API и не дублируются. Пустой error text и все представимые metadata invariants проверяются runtime.
- JavaScript rejection не может содержать произвольное non-`Error` значение: Rust handler возвращает `BoxError`. Соответственно ветви diagnostic `non-Error value` и hostile coercion не создаются; исходная ошибка `BoxError` сохраняется без обёртки.
- Typert-generated entry points `./typert` и `./remote` не имеют ручных файлов в `src` и по PORTING §8 не генерируются. Rust public API экспортируется из корня крейта.
- Source `CommandExecution`/`CommandResult` принадлежат этому пакету. У `hoh-protocol` сейчас есть исторические одноимённые типы, а `hoh-sdk-jsonrpc-server` содержит `SHIM(hoh-commands)`. Эти consumers должны перейти на `hoh-commands` отдельной правкой владельца; чужие крейты в этой сдаче не менялись.

## Тесты источника

В источнике 36 именованных `it`/`it.each` групп (34 commands + 2 builtin; `it.each` считается одной группой). Представимые сценарии сведены в 42 Rust-теста без ослабления assertions; отдельные regression-тесты покрывают обе точки panic обработчика, panic resolver-а и повторный вход из `session/event`. Табличные проверки grammar/validation остаются таблицами внутри тестов. Две группы целиком непредставимы типами Rust: non-string metadata и non-boolean `input.attachments`. В трёх смешанных группах перенесены представимые ветви, а JSON-only подслучаи перечислены выше: non-function handler, non-Error rejection и malformed untyped result. Проверки `Object.isFrozen` заменены проверками owned/immutable API, а не фиктивным runtime-флагом.

Новых внешних зависимостей нет: все внешние зависимости уже находятся в workspace/Cargo.lock; остальные — path-зависимости на перенесённые крейты.
