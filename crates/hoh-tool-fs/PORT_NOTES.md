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
| — | src/shim.rs | `live_agent`: `ToolExecution.agent` уже несёт полный `hoh_agent::Agent`, функция возвращает тот же `Arc` |
| tests/error.spec.ts | tests/error.rs | 4 из 4 |
| tests/diff.spec.ts | tests/diff.rs | 12 из 12 |
| tests/read-render.spec.ts | tests/read_render.rs | 31 из 31 + 5 регрессий (`\r` на позиции предела, и др.) |
| tests/tools.spec.ts | tests/tools.rs, tests/guidance.rs | см. «Не перенесено» |
| tests/integration.spec.ts | tests/integration.rs | 36 из 37 (PTC-часть «current working directory» не перенесена) |
| tests/read-image.spec.ts | tests/read_image.rs | PTC-случаи не перенесены (см. «Не перенесено»); остальные перенесены |
| tests/apply-patch.spec.ts | tests/apply_patch.rs | см. «Не перенесено» |
| tests/apply-patch-presentation.spec.ts | tests/apply_patch_presentation.rs | 5 из 6 |
| tests/patch/{resolution,preflight,commit,success}.spec.ts | tests/patch_{resolution,preflight,commit,success}.rs | настоящий `LocalFileSystem` через обёртку-шпион |
| tests/patch/harness.ts, tests/tools.spec.ts (FakeFs) | tests/support/{mod,patch}.rs | |

Итог прогона после fix2 (ревью 160916): `cargo test -p hoh-tool-fs` — 287 прошли, 0 упавших (real_composition 4, apply_patch 10, apply_patch_presentation 5, diff 12, error 4, guidance 2, integration 36, patch_commit 12, patch_preflight 19, patch_resolution 3, patch_success 10, read_image 47, read_render 36, tools 87); clippy `--all-targets --no-deps -D warnings` и `fmt --check` чисты.

## Отличия от образца
- Усечение строки при чтении и ограничение по байтам считаются в единицах UTF-16 (как `String.length` образца). Граница, попавшая внутрь суррогатной пары (emoji), ведёт себя как `substring`: в JavaScript остаётся одиночный старший суррогат, а при кодировании в UTF-8 (байты, которые получает модель) он становится U+FFFD из трёх байт; здесь в конец префикса добавляется U+FFFD, длина в байтах та же. Проверено точным текстом в `tests/read_render.rs` (модуль `utf16_boundary`).
- `ReadWindow.offset` — `f64` (как `number` образца): сообщение «offset … is out of range» печатает число как JavaScript (`1e20` → `100000000000000000000`), без насыщающего приведения к `u64`. Поле `FileReadOutcome.offset` остаётся `u64` (достигается только внутри файла).
- Подтверждение эскалации — настоящий `hoh-user-approval` (`ctx.approval` образца); `ToolExecution.agent` несёт полный `hoh_agent::Agent`, и `shim::live_agent` возвращает тот же `Arc`. Вызов без агента: рабочий каталог разрешает путь без каталога сессии, эскалация отвечает «the call has no agent to route it through». Ветка `ToolContextUnavailableError` в `session_cwd.rs` недостижима (`live_agent` всегда `Some`) и оставлена как защитная.
- Маршрут `read_image` без заголовка запроса берётся из `agent.options` зарегистрированного агента (`provider`/`model`).
- `diffs_from_meta` отбрасывает лишние ключи записей мета, образец их сохранял; для настоящих мета различий нет.
- Текст `String(error)` для не-`HarnessError` значений передаётся через `Display` ошибки.
- Публичные имена сверх образца: `Env` (замена замыкания над `ctx`), модули `shim`, `patch::*` типы отчёта; функции помощники `sandbox::FsSandboxController::{map_error,map_fs_error}`.
- `Config.readLimit` и остальные пределы принимаются как `f64`, чтобы сохранить исходный отказ дробных/нечисловых значений; сообщение `tool-fs: {name} must be a positive integer`.
- Тесты идут на настоящем `hoh-fs-observation-policy` (плагин монтируется в `tests/support/mod.rs::mount_policy`), а там, где проверяется песочница, — на настоящих `hoh-sandbox-policy` + `hoh-fs-sandbox` (`PatchOptions.mode`, `Options.sandbox_policy`). Локальной замены политики больше нет.
- Тесты. Исходный fixture рабочего каталога игнорирует сигнал; настоящий `WorkingDirectoryService` отказывает на оборванном сигнале до разрешения путей. Тест «aborted before entry» (`patch_resolution.rs`) поэтому выполняется без агента (`run_without_agent`): ожидаемые значения образца те же.
- Тесты. Агенты тестов — малые `hoh_tools::Agent`; для каждой Session в вызове `support::call` регистрируется настоящий `hoh_agent::Agent`-заглушка (настоящая Session, `inject` записывает уведомления, без цикла) в настоящем реестре `agents`, поверх настоящих `SessionProjectionRegistry` и `ApprovalService`; «пользователь» — слушатель настоящего события `approval/request`. Для Session с включённым подтверждением открывается `turn/start` (подтверждение вне хода запрещено).
- Тесты. Исходный FakeFs выбрасывает вооружённую ошибку и в `stat`; настоящий рабочий каталог проверяет свой каталог через `stat`, поэтому для каталогов вооружённая ошибка не срабатывает.
- Тесты `read_image` монтируют настоящий `hoh-attachment-local` и `LlmRuntime`; для сценариев, где настоящий магазин не может показать нужное (лимиты, отказ сохранения, пустое имя, уменьшение), используется небольшая реализация `AttachmentStore` в самом тестовом файле. Маршрут вызова задаётся событием `request/header` в Session агента.

## Не перенесено
- tools.spec «uses capability visibility in PTC mode», read-image.spec (программа показывает выбранное изображение, 8 решений внешних этапов, exception/timeout — 11 случаев), apply-patch.spec «dispatches native and PTC patch calls with identical values»: нужны `hoh-tool-executor-ptc` и `hoh-ptc-runtime`, которых нет в рабочем дереве.
- tools.spec «has no default export (namespace plugin export shape)»: форма JS-модуля, в Rust нет аналога.
- integration.spec «a throwing fs/observed listener surfaces as isError»: слушатель события в ядре Rust возвращает `()` и не может бросить в `emit`.
- tests/patch/commit «provider sandbox denial» (2 случая), patch/success «commits guarded changes under %s policy», apply-patch.spec «advertises escalation only for a confining backend…» перенесены на настоящие `hoh-sandbox-policy` + `hoh-fs-sandbox` (`PatchOptions.mode`): `patch_commit.rs`, `patch_success.rs`, `apply_patch.rs`. Прежняя проверка `parameters.contains_key("sandbox_permissions")` искала ключ на верхнем уровне нормализованной схемы, где его нет, и не могла упасть; теперь ключ ищется в `properties`, в обе стороны.
- integration.spec «current working directory»: PTC-часть (`run_code`/`ReadBindingRuntime`, `tool/ptc-dispatch`) не перенесена — нет `hoh-tool-executor-ptc` и `hoh-ptc-runtime`; остальное перенесено, в том числе `sandboxPolicy.resolve({session}).workspaceRoot` до и после смены каталога и `session.header.cwd`.
- tools.spec «rejects a non-JSON numeric offset» (`tools.spec.ts:308`): в `serde_json::Value` нельзя передать `NaN`/`Infinity` (JSON их не содержит), поэтому случай невыразим; отклонение нечисловых/дробных значений проверяют соседние тесты на `offset`.
- apply-patch-presentation.spec «propagates unexpected parser defects»: шпион на парсер модуля, в Rust парсер подменить нельзя без отдельного шва.
- tests/fs-tools.e2e.ts (2 теста): запуск e2e вне модульных тестов.

## Не проверено
- Windows и macOS; проверка с PTC-исполнителем.
- Версия GIF→WebP в `read_image` сверена с тестом образца (72 байта) только на настоящем `hoh-attachment-local`.

## Вопросы ведущему
- `src/shim.rs`: SHIM(hoh-sandbox-policy) снят — ключ `sandboxPolicy` и `SandboxPolicyRequest` берутся из `hoh-sandbox-policy`, как у `hoh-fs-sandbox`. Остался только `live_agent` (возвращает тот же `Arc`; можно убрать при следующей правке `src/`, вне объёма ревью тестов).
- `tests/real_composition.rs` идёт с настоящими `hoh-fs-sandbox`, `hoh-fs-observation-policy`, одним `sandboxPolicy`: запись вне папки, через `..`, через симлинк запрещена, непрочитанный файл не перезаписывается.
