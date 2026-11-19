# hoh-tool-working-directory ← packages/session/tool-working-directory @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs | плагин `ToolWorkingDirectory` (имя `tool-working-directory`), инструмент `working_directory` |
| locale/*.json | locale/*.json | восемь файлов скопированы без изменений |
| tests/ | tests/tool.rs | у пакета нет собственных тестов; перенесён тест инструмента из tests/working-directory.spec.ts пакета session/working-directory («exposes reading and changing through the same model tool and unregisters it on disposal») |
| README*.md | — | прочитаны |

Итог: `cargo test -p hoh-tool-working-directory` — 3 прошли, 0 упавших; clippy `--all-targets -D warnings` и `fmt --check` чисты.

## Отличия от образца
- Тест инструмента лежит здесь, а не в hoh-working-directory: образец импортирует инструмент в тесте сервиса, в Rust это был бы цикл крейтов.
- Тесты идут поверх настоящих `hoh-working-directory`, `hoh-fs-local`, `hoh-session-projection`, реестра `agents` из `hoh-agent` и `ReactLoopInbox` из `hoh-agent-loop` на временном каталоге. Добавлены два теста сверх образца: уведомление «The working directory changed from … to …» попадает во входящие настоящего агента; после уничтожения агента между проверкой каталога (`stat`) и фиксацией ничего не пишется и уведомления нет (`stat` задерживается обёрткой над `LocalFileSystem`).
- Агент для сервиса берётся из реестра `agents` по id сессии вызова (`hoh_working_directory::live_agent`): `ToolExecution.agent` в `hoh-tools` пока малый `hoh_tools::Agent`. Без зарегистрированного агента инструмент отвечает `ToolContextUnavailableError` («requires an agent registered in the agents registry»).

## Не перенесено
- Ничего: исходник — один файл `src/index.ts`.

## Не проверено
- Настоящий цикл агента: в тестах агент без драйвера и модели.

## Вопросы ведущему
- Реестр `agents` как источник целого агента — временный мост до перехода `ToolExecution.agent` на `hoh_agent::Agent` (см. PORT_NOTES `hoh-working-directory`, `live_agent`).
