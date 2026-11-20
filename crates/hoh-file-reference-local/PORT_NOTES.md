# hoh-file-reference-local ← packages/context/file-reference-local @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | `Config`, `LocalFileReferenceService` (плагин и `FileReferenceService`), проверка настроек перенесены |
| `src/search.ts` | `src/search.rs` | `WorkspaceFileSearch`, `FileSearchConfig`, три константы по умолчанию, реэкспорт `active_at_token`/`format_file_mention` |
| `tests/search.spec.ts` | `tests/search.rs` | 11 из 12 случаев (два — грамматика); не перенесён один под-случай, см. ниже |
| `tests/service.spec.ts` | `tests/service.rs` | 6 из 7 случаев; `tests/support/mod.rs` — стенд (агент, входящие, `Fs`) по образцу `hoh-working-directory` |
| `locale/*.json` | `locale/*.json` | копия без изменений |
| `README*.md`, `package.json` | эти заметки и `Cargo.toml` | прочитаны; собственного `LICENSE` нет |

Используется переносимыми пакетами: сервис `fileReferences` (через `hoh-file-reference`) и плагин; публичные
символы `search` (`WorkspaceFileSearch`, константы, реэкспорт грамматики) нужны только тестам и встройщикам.
Имя плагина — `LocalFileReferenceService` (класс без `export const name`).

## Отличия от образца

- `maxResults`, `maxEntries` — `u64`, схема `minimum: 1`. Дробное значение (`1.5`) отклоняет схема, а не
  `validateConfig`; текст отказа — текст проверки схемы ядра, ключ в нём назван. Те же проверки `0`, `> 2^53-1`
  и `excludedDirectories` остались в коде и дают тексты образца.
- `WorkspaceFileSearch::new` возвращает `Result` (в образце конструктор бросает).
- Индекс строит `tokio::spawn`-задача (у `WorkspaceFileSearch` нет контекста плагина, как и в образце, где
  обход — свободный промис). Отмена — `AbortController` индекса, `dispose` её вызывает; результат раздаётся
  через `watch`. Ожидающий вызывающий выходит по своему `AbortSignal` с его причиной.
- Прекращение очистки подсказки: `Fiber::dispose()` не возвращает ошибку, поэтому ветка
  `ctx.logger.warn('file-reference-local: prompt cleanup failed: …')` недостижима и не написана. Незавершённые
  освобождения запоминаются как `Fiber` и дожидаются при выгрузке сервиса (в образце — `promptDisposals`).
- Ключ кэша и подсказок — `SessionId` агента (в образце — сам объект `Agent`); два живых агента с одним id в
  реестре невозможны.
- Имена файлов с не-UTF-8 байтами читаются через `to_string_lossy`. Порядок имён — по единицам UTF-16, как
  `compareText` образца. Подпоследовательность считается по единицам UTF-16 (`indexOf`).
- Текст ошибки нечитаемого корня — `ENOENT: no such file or directory, scandir '…'` через
  `hoh_error::node_fs_error_message`.

## Не перенесено

- `search.spec`: проверка `abort('cancelled')` с причиной не-`Error` (`file search aborted`): причина
  `AbortSignal` в Rust всегда ошибка, ветка `errorReason(…, fallback)` для не-ошибки недостижима. Не перенесена
  и проверка дробного `maxEntries: 1.5` у конструктора поиска — тип `u64` её не выражает (см. выше).
- `search.spec` «injected readdir failure»: подмена `node:fs/promises` невозможна; случай покрыт вариантом с
  `chmod 0`, который сам пропускает проверку, если каталог остался читаемым (запуск от root).
- `service.spec` «logs rejected prompt cleanup»: подмена `ctx.inject` с отклонённым `dispose` невозможна, а
  `Fiber::dispose()` не отклоняется.
- `service.spec` «defers guidance…»: образец монтирует сервис без `systemPrompt` и `tools`; в Rust
  `WorkingDirectory` сам требует `systemPrompt`, а сервис — настоящее значение, не заглушку, поэтому поздно
  появляется только реестр инструментов (и его исполнитель).
- `service.spec` «invalidates … and disposes»: шпионы по `WorkspaceFileSearch.prototype` заменены проверкой
  поведения (индекс обновляется только после `tool/result` сессии самого агента); освобождение поиска вместе с
  агентом наблюдается тестом `aborts_a_pending_first_search_when_its_agent_is_disposed…`: ожидающий первый `list`
  получает `file search index disposed`, повторный `agent/disposed` безвреден. Счётчик `prompt_disposals`
  снаружи не виден и не проверяется.
- `service.spec` «falls back to the process cwd»: вместо подмены `list` проверяется выдача из каталога запуска
  (`Cargo.toml` пакета); дедупликация проверяется неизменным числом fiber после повторного `agent/created`. Повторное использование
  кэша поиска прямой проверкой не покрыто (шпиона на `WorkspaceFileSearch` нет).

## Не проверено

- `search.spec` «root unreadable»: образец считает повторные чтения корня (`rootReads()`); подмены `readdir` нет,
  поэтому Rust проверяет только следствие — после возвращения корня запрос видит его файлы.
- `maxResults: 20.0` в JSON `serde_json` отклоняет для `u64`, JS принял бы; отличие принято, ручной JSON.
- Паника внутри задачи обхода оставит поколение занятым («file search index failed» для следующих запросов);
  достижимо только при панике в `scan_workspace`.

Ветки Windows (`isAbsolute(fromRoot)`, junction) не написаны под `cfg` отдельно: `node_path` работает со
строками POSIX; проверка выполнена только на Linux. `search.spec` использует каталог вида `…/tmp/…` внутри
`TMPDIR`: образцовый тест рассчитывает, что в `/tmp/…` буквы `t`, `u`, `i` идут по порядку.

## Вопросы ведущему

- Тесты `service.rs` требуют dev-зависимостей `hoh-agent-loop`, `hoh-fs`, `hoh-fs-patch`, `hoh-sandbox`,
  `hoh-scope`, `hoh-session-projection`, `hoh-tool-executor-native`, `hoh-tool-testkit`, `hoh-llm`
  (стенд, как у `hoh-working-directory`). Всё это уже есть в workspace.
