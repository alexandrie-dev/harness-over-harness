# hoh-tool-todo ← packages/todo/tool-todo

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | Config, проекция `todos`, `todo_write`, валидация, event append и presentation перенесены |
| `src/types.ts`, `src/client.ts` | `src/lib.rs` | `TodoItem` и `TodoStatus` — единые host/client-safe Rust-типы |
| `locale/*.json` | `locale/*.json` | 8 файлов скопированы без изменений |
| `tests/tool-todo.spec.ts` | `tests/tool.rs` | 22 / 0 случаев; прямой tool runtime и настоящий Session log |
| `tests/projection.spec.ts` | `tests/projection.rs` | 5 / 0 случаев; настоящий registry, replay и lifecycle; плюс summary exclusion |
| `tests/integration.spec.ts`, `tests/loader-composition.spec.ts` | — | требуют отсутствующих testkit/Loader-составов, см. ниже |
| `README.md`, `README.zh.md`, `README.i18n.yaml`, `package.json` | эти заметки и `Cargo.toml` | прочитаны, контракт и состав пакета учтены |

LICENSE в пакете образца отсутствует.

## Отличия от образца

1. Строковый union статуса представлен `TodoStatus`; JSON-граница сохраняет `pending`, `in_progress`,
   `completed` буквально.
2. Публичные Rust-символы `ToolTodo`, `Deps`, `TODO_WRITE_EVENT`, `TODOS_PROJECTION` и
   `todo_projection_definition` нужны typed kernel и прямым host-потребителям; TypeScript использует module
   augmentation и namespace exports вместо этих имён.
3. Публичный `apply` получает `ToolsService` и `SessionProjectionRegistry` явно, потому что Rust не использует
   Cordis Proxy-свойства контекста.
4. Канонизация `content` использует явно перечисленный набор ECMAScript `String.prototype.trim`, а не
   Rust `str::trim`: BOM `U+FEFF` удаляется, NEL `U+0085` сохраняется. Этот же канонический текст служит
   ключом дедупликации. Так журнал `todo/write` и ошибки совпадают с TypeScript на Unicode-границах.

## Не перенесено

- `integration.spec.ts`, 2 случая: полный model → agent-loop → tool путь ждёт публичный
  `hoh-agent-loop-testkit`/mock adapter fixture.
- `loader-composition.spec.ts`, 4 случая: в Rust пока нет полного аналога Cordis Loader include composition и
  готового `hoh-tool-testkit`. Обязательность/тип Config и обе policy-ветви проверены напрямую, но это не
  считается переносом Loader-сценариев.

Итого случаев образца: 27 перенесено, 6 не перенесено. Несколько смежных случаев образца объединены;
дополнительные прямые проверки Config, summary exclusion и Unicode trim дают 29 Rust-тестов. Ошибки
валидации закреплены не только текстом, но и структурными `name`/`code` (`INVALID_ARGS` и
`TOOL_CONTEXT_UNAVAILABLE`).

## Не проверено

- Полный app/agent-loop состав с модельным tool call и записью парных `tool/call`/`tool/result` вокруг
  `todo/write`; leaf-крейт проверяет собственную запись настоящего Session и projection registry.

## Вопросы ведущему

1. После появления `hoh-agent-loop-testkit` и `hoh-tool-testkit` добавить 2 loop-сценария; изменений
   production API этого крейта для них не требуется.
