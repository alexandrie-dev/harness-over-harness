# hoh-working-directory ← packages/session/working-directory @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs | плагин `WorkingDirectory` (имя `WorkingDirectoryService`), `WorkingDirectoryService` (`ensure`/`set`/`get`/`recorded`/`default_directory`), проекция `workingDirectory`, контекст пользователя, очередь изменений, уведомление о смене каталога |
| src/error.ts | src/error.rs | `WorkingDirectoryError` с кодами `WORKING_DIRECTORY_*` |
| locale/*.json | locale/*.json | восемь файлов скопированы без изменений |
| tests/working-directory.spec.ts | tests/working_directory.rs | 18 из 19 объявлений; 22 тестовые функции Rust (два объявления с таблицами разложены на отдельные функции; плюс снимок проекции через общий реестр) |
| tests/admission.spec.ts | — | не перенесён, см. ниже |
| README*.md | — | прочитаны |

## Отличия от образца
- `src/shim.rs` — слой совместимости только для `hoh-sdk-jsonrpc-server` до его миграции (`AgentApi`, `Agent`, `AsLiveAgent`, `SessionProjections`/`SessionProjectionsKey`): `ensure`/`set` принимают и настоящий агент, и этот `Agent` (оборачивается в настоящий `hoh_agent::Agent` без цикла, модели и входящих). Убрать после миграции потребителя.
- Агент — настоящий `hoh_agent::Agent` (`Arc<dyn Agent>`), проекция `workingDirectory` — настоящий `hoh-session-projection` (общий `SessionProjectionRegistry`, схема состояния и `wire.view` как в образце). Заглушек в крейте нет.
- Агент в контексте сборки подсказки — расширение `AssembleAgent` (в образце `context.agent`).
- `live_agent(ctx, session)` (сверх образца): `ToolExecution.agent` в `hoh-tools` пока малый `hoh_tools::Agent`, а рабочему каталогу, подтверждению и маршруту нужен целый агент; функция достаёт его из реестра `agents` по id сессии.
- Чтение проекции может завершиться ошибкой (в образце `stateOf` бросает); `get`/`recorded` возвращают `String`/`Option`, поэтому ошибка пишется предупреждением в журнал, а состояние считается незаписанным.
- Операция учитывается при `dispose` с первого опроса возвращённого future, а не с вызова (в образце промис уже запущен и отклоняется при abort): неопрошенный future не держит выгрузку; ожидание предыдущей операции прерывается abort. Место в очереди занимается при вызове. Операция, отменённая в очереди, отклоняется сразу, но остаётся звеном цепочки до завершения предшественника (место передаётся фоновой задаче-ретранслятору; он ждёт предшественника или выгрузку сервиса, учитывается выгрузкой и счётчиком `live_relay_tasks`; хвост очереди не удаляется), как в образце. Тесты: `disposal_with_unpolled_futures`, `cancelled_queued_change_preserves_predecessor_barrier`, `relay_tasks_do_not_survive_dispose`.
- Очередь изменений — цепочка `oneshot` (завершение операции будит следующую); отмена и выгрузка плагина прерывают ожидающие и текущие операции и ждут их завершения до завершения `dispose`.
- Config — `defaultDirectory` (camelCase); не абсолютный путь отклоняется при применении плагина, до приёма сессий (тест перенесён).

## Не перенесено
- Тест «exposes reading and changing through the same model tool and unregisters…» — это тест инструмента `working_directory`, который принадлежит crate hoh-tool-working-directory (образец импортирует его оттуда).
- tests/admission.spec.ts — требует testkit цикла агента, sandbox-policy и approval; переносится вместе с этими crates.

## Не проверено
- Настоящий драйвер агента (цикл, модель): в тестах агент — структура с настоящей `Session` и настоящим `ReactLoopInbox` из `hoh-agent-loop`, без цикла. Реестр `agents` проверен в тестах `hoh-tool-fs` и `hoh-tool-working-directory`.
- Платформы кроме Linux.

## Вопросы ведущему
- `live_agent` — временный мост до тех пор, пока `ToolExecution.agent` в `hoh-tools` не станет `hoh_agent::Agent`; после этого функцию можно убрать.
