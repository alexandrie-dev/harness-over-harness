# hoh-tool-fs ← packages/fs/tool-fs @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs | плагин `ToolFs` (имя `tool-fs`), `Config`, `Env`; регистрация read/write/edit/apply_patch, read_image через `ctx.inject` на `attachments`; разделы подсказок по видимости инструментов |
| src/read.ts | src/read.rs | `apply_read_tool`, пределы чтения (`ReadToolCaps`), разбор аргументов, представления |
| src/read-render.ts | src/read_render.rs | нумерованный вывод, окно, подвал, мета окна, язык по расширению |
| src/read-target.ts | src/read_target.rs | |
| src/read-image.ts | src/read_image.rs | `apply_read_image_tool`, определение типа по расширению и сигнатуре, шлюз модальности маршрута, формат конверта |
| src/image-error.ts | src/image_error.rs | `ImageInputError` с кодами |
| src/write.ts, src/edit.ts | src/write.rs, src/edit.rs | |
| src/mutation-result.ts | src/mutation_result.rs | |
| src/diff.ts | src/diff.rs | различия через существующий `hoh-jsdiff` (`structured_patch`, контекст 3) |
| src/error.ts | src/error.rs | `remediate_fs_error`, `remediate_caught` |
| src/sandbox.ts | src/sandbox.rs | `FsSandboxController`: поля эскалации, политика вызова, метка отказа |
| src/session-cwd.ts | src/session_cwd.rs | |
| src/apply-patch.ts | src/apply_patch.rs | |
| src/apply-patch-output.ts | src/apply_patch_output.rs | схема вывода, `render_patch`, `patch_presentation_meta`, `PatchToolError` |
| src/apply-patch-presentation.ts | src/apply_patch_presentation.rs | |
| src/patch/{index,plan,preflight,execute,types}.ts | src/patch/{mod,plan,preflight,execute,types}.rs | |
| locale/*.json | locale/*.json | |
| — | src/shim.rs | остаток заглушек (`sandboxPolicy`, `live_agent`), см. «Вопросы ведущему» |
| tests/error.spec.ts | tests/error.rs | 4 из 4 |
| tests/diff.spec.ts | tests/diff.rs | 12 из 12 |
| tests/read-render.spec.ts | tests/read_render.rs | 31 из 31 |
| tests/tools.spec.ts | tests/tools.rs, tests/guidance.rs | см. «Не перенесено» |
| tests/integration.spec.ts | tests/integration.rs | 36 из 37 |
| tests/read-image.spec.ts | tests/read_image.rs | 44 теста Rust; PTC-случаи и один маршрут не перенесены |
| tests/apply-patch.spec.ts | tests/apply_patch.rs | см. «Не перенесено» |
| tests/apply-patch-presentation.spec.ts | tests/apply_patch_presentation.rs | 5 из 6 |
| tests/patch/{resolution,preflight,commit,success}.spec.ts | tests/patch_{resolution,preflight,commit,success}.rs | настоящий `LocalFileSystem` через обёртку-шпион |
| tests/patch/harness.ts, tests/tools.spec.ts (FakeFs) | tests/support/{mod,patch}.rs | |

Итог прогона после fix1: `cargo test -p hoh-tool-fs` — 281 прошёл, 0 упавших (real_composition 3, apply_patch 10, apply_patch_presentation 5, diff 12, error 4, guidance 2, integration 36, patch_commit 10, patch_preflight 19, patch_resolution 3, patch_success 9, read_image 46, read_render 35, tools 87); clippy `--all-targets --no-deps -D warnings` и `fmt --check` чисты.

## Отличия от образца
- Усечение строки при чтении и ограничение по байтам считаются в единицах UTF-16 (как `String.length` образца). Граница, попавшая внутрь суррогатной пары (emoji), ведёт себя как `substring`: в JavaScript остаётся одиночный старший суррогат, а при кодировании в UTF-8 (байты, которые получает модель) он становится U+FFFD из трёх байт; здесь в конец префикса добавляется U+FFFD, длина в байтах та же. Проверено точным текстом в `tests/read_render.rs` (модуль `utf16_boundary`).
- `ReadWindow.offset` — `f64` (как `number` образца): сообщение «offset … is out of range» печатает число как JavaScript (`1e20` → `100000000000000000000`), без насыщающего приведения к `u64`. Поле `FileReadOutcome.offset` остаётся `u64` (достигается только внутри файла).
- Подтверждение эскалации — настоящий `hoh-user-approval` (`ctx.approval` образца); агент для запроса и для рабочего каталога берётся из реестра `agents` по id сессии (`shim::live_agent`), потому что `ToolExecution.agent` в `hoh-tools` пока малый `hoh_tools::Agent`. Вызов без зарегистрированного агента: рабочий каталог отвечает `ToolContextUnavailableError`, эскалация — «the call has no agent to route it through».
- Маршрут `read_image` без заголовка запроса берётся из `agent.options` зарегистрированного агента (`provider`/`model`).
- `diffs_from_meta` отбрасывает лишние ключи записей мета, образец их сохранял; для настоящих мета различий нет.
- Текст `String(error)` для не-`HarnessError` значений передаётся через `Display` ошибки.
- Публичные имена сверх образца: `Env` (замена замыкания над `ctx`), модули `shim`, `patch::*` типы отчёта; функции помощники `sandbox::FsSandboxController::{map_error,map_fs_error}`.
- `Config.readLimit` и остальные пределы принимаются как `f64`, чтобы сохранить исходный отказ дробных/нечисловых значений; сообщение `tool-fs: {name} must be a positive integer`.
- Тесты. Пакет `@deepseek-ai/dsh-fs-observation-policy` ещё не перенесён: в `tests/support/mod.rs` лежит его локальная замена (карта владелец→цель→последнее наблюдение, одиночные слушатели намерений). Сообщения `FS_NOT_OBSERVED`/`FS_NOT_FOUND` совпадают с образцом, но сама политика не проверяется.
- Тесты. Исходный fixture рабочего каталога игнорирует сигнал; настоящий `WorkingDirectoryService` отказывает на оборванном сигнале до разрешения путей. Тест «aborted before entry» (`patch_resolution.rs`) поэтому выполняется без агента (`run_without_agent`): ожидаемые значения образца те же.
- Тесты. Агенты тестов — малые `hoh_tools::Agent`; для каждой Session в вызове `support::call` регистрируется настоящий `hoh_agent::Agent`-заглушка (настоящая Session, `inject` записывает уведомления, без цикла) в настоящем реестре `agents`, поверх настоящих `SessionProjectionRegistry` и `ApprovalService`; «пользователь» — слушатель настоящего события `approval/request`. Для Session с включённым подтверждением открывается `turn/start` (подтверждение вне хода запрещено).
- Тесты. Исходный FakeFs выбрасывает вооружённую ошибку и в `stat`; настоящий рабочий каталог проверяет свой каталог через `stat`, поэтому для каталогов вооружённая ошибка не срабатывает.
- Тесты `read_image` монтируют настоящий `hoh-attachment-local` и `LlmRuntime`; для сценариев, где настоящий магазин не может показать нужное (лимиты, отказ сохранения, пустое имя, уменьшение), используется небольшая реализация `AttachmentStore` в самом тестовом файле. Маршрут вызова задаётся событием `request/header` в Session агента.

## Не перенесено
- tools.spec «uses capability visibility in PTC mode», read-image.spec (программа показывает выбранное изображение, 8 решений внешних этапов, exception/timeout — 11 случаев), apply-patch.spec «dispatches native and PTC patch calls with identical values»: нужны `hoh-tool-executor-ptc` и `hoh-ptc-runtime`, которых нет в рабочем дереве.
- tools.spec «has no default export (namespace plugin export shape)»: форма JS-модуля, в Rust нет аналога.
- integration.spec «a throwing fs/observed listener surfaces as isError»: слушатель события в ядре Rust возвращает `()` и не может бросить в `emit`.
- tests/patch/commit «provider sandbox denial» (2 случая), patch/success «commits guarded changes under %s policy», apply-patch.spec «advertises escalation only for a confining backend…»: зависят от `SandboxedFileSystem` (`hoh-fs-sandbox`), которого нет. Эскалацию write/edit проверяют тесты tools.rs на собственном «замыкающем» FakeFs.
- apply-patch-presentation.spec «propagates unexpected parser defects»: шпион на парсер модуля, в Rust парсер подменить нельзя без отдельного шва.
- tests/fs-tools.e2e.ts (2 теста): запуск e2e вне модульных тестов.

## Не проверено
- Windows и macOS; проверка с настоящим `hoh-fs-observation-policy`, `hoh-sandbox-policy`, `SandboxedFileSystem`, PTC-исполнителем.
- Версия GIF→WebP в `read_image` сверена с тестом образца (72 байта) только на настоящем `hoh-attachment-local`.

## Вопросы ведущему
- `src/shim.rs`: SHIM(hoh-sandbox-policy) — ключ и трейт `sandboxPolicy` теперь реэкспортируются из принятого `hoh-fs-sandbox` (один тип ключа на оба крейта; раньше два одноимённых ключа не могли жить в одном ядре). Заменить на сервис `hoh-sandbox-policy` вместе с `hoh-fs-sandbox`.
- `src/shim.rs`: SHIM(hoh-tools) — `live_agent`: целый агент из реестра `agents` по Session вызова; убрать, когда `hoh-tools` примет `hoh_agent::Agent` в `ToolExecution.agent`.
- Тестовая замена `hoh-fs-observation-policy` осталась только в `tests/support/mod.rs` (быстрые модульные тесты на FakeFs); `tests/real_composition.rs` идёт с настоящими `hoh-fs-sandbox`, `hoh-fs-observation-policy`, одним `sandboxPolicy`: запись вне папки, через `..`, через симлинк запрещена, непрочитанный файл не перезаписывается.
