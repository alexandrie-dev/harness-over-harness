# hoh-mcp-resources ← packages/mcp/mcp-resources @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | scoped registry провайдеров, plugin/service, lifecycle общих инструментов и prompt-секция |
| `src/tools.ts` | `src/lib.rs` | три model-facing tool, схемы аргументов, маршрутизация и concurrency classification |
| `src/render.ts` | `src/lib.rs` | attributed JSON projection с удалением строковых `blob` из model history |
| `tests/resources.spec.ts` | `tests/resources.rs` | 20 тестов lifecycle, scope, routing, rollback, optional prompt, ошибок и rendering |
| `locale/*.json` | `locale/*.json` | все 8 файлов перенесены без изменений |
| `README.md`, `README.zh.md` | — | прочитаны; собственного LICENSE у пакета нет |

## Сохранённое поведение

- При пустом реестре пакет не публикует ни одного resource tool и не добавляет MCP-секцию в prompt. Первый провайдер точного scope регистрирует три общих инструмента, последний синхронно убирает их; инструменты принадлежат runtime, а не первому connection plugin.
- Имена серверов и выбор провайдера строятся из одного `ScopedLayers`: ближайший scoped-провайдер затеняет унаследованный, потомки видят провайдер родителя, несвязанные агенты его не видят. Дубликат запрещён только внутри одного слоя.
- `list_mcp_resources`, `list_mcp_resource_templates` и `read_mcp_resource` требуют явный `server`, передают opaque cursor/URI без преобразования и помечены parallel-safe. Отсутствующий сервер отклоняется до provider dispatch точным `ToolArgsError`/`INVALID_ARGS`.
- Provider failure не меняет набор инструментов и имён. Частичная регистрация трёх инструментов откатывается транзакционно и глобально, и в agent scope; после снятия конфликта тот же server можно зарегистрировать.
- Canonical JSON результата сохраняет исходный `blob`. Только model-facing `ContentBlock` заменяет каждое строковое поле с ключом `blob` на описание длины в UTF-16 code units. Сериализация использует `hoh_util_values::js_json_stringify`, поэтому написание чисел совпадает с `JSON.stringify` образца.
- Prompt публикует только видимые caller-у имена, сортирует их как JavaScript `sort()` по UTF-16 и отключает интерполяцию, поэтому `{{literal}}` остаётся буквальным текстом.
- `systemPrompt` остаётся опциональной late-bound зависимостью: без него сервис и инструменты активируются, а при наличии prompt регистрируется динамическая секция. Disposal последнего scoped-провайдера снимает tools синхронно и завершает возвращённый future только после полного teardown принадлежащего runtime tool-scope.

## Отличия Rust API

- TypeScript union `McpResourceRequest` представлен enum-вариантами `List`, `ListTemplates`, `Read`; методы `method()` и `cursor()` дают точные protocol-поля без строковых опечаток.
- `McpResourceProvider::request` возвращает явный `McpResourceFuture`, а execution context передаётся как `Arc<ToolExecution>`. Это Rust-эквивалент Promise и живого `ToolExecution` исходника, не требующий новой внешней зависимости `async-trait`.
- `register` принимает `&Context` владельца и `Arc<dyn McpResourceProvider>`, возвращая kernel `Disposer`. Для tool-регистраций точного scope runtime создаёт собственный child scope с тем же `ScopeKey`; при ошибке setup child fiber немедленно откатывается.
- Plugin и сервис представлены одним `McpResourceRuntime`; service key — `McpResources = "mcpResources"`. Публичная pure-функция `render_resource_result` соответствует `renderResourceResult`, а `register_resource_tools` соответствует исходному именованному экспорту.

## Адаптация тестов

- Native execution проводится через настоящий `ToolExecutionPipeline`; canonical value, model content и structured error проверяются раздельно.
- По итогам ревью матрица вызывает все три отсутствующих инструмента и проверяет `UNKNOWN_TOOL` как при полностью пустом реестре, так и для sibling scope. Оба порядка снятия двух scoped-провайдеров проходят с реальным вызовом оставшегося сервера между снятиями.
- Отдельно зафиксированы opaque cursor для `list_mcp_resource_templates`, точные глобальный и scoped тексты конфликтов при rollback, scoped-дубликат с сохранением рабочего владельца, снятие глобального слоя для owner/child, повторная scoped-регистрация и shadow-дубликат имени в prompt.
- Kernel `Disposer::dispose(self)` является одноразовым по Rust type contract, поэтому буквальный повтор одного provider-disposer из TypeScript не выражается корректной Rust-программой. Наблюдаемая идемпотентность teardown проверена двумя последовательными `Scope::dispose(&self)` после повторной регистрации; инструменты owner и child остаются снятыми.
- PTC runtime/executor в рабочем пространстве ещё отсутствуют. Собственная логика этого пакета заканчивается на общей регистрации `Tools`; PTC declarations/bindings строятся самим `hoh-tools` из тех же определений. Поэтому PTC-only проверки образца здесь не запускаются и SHIM не добавлен.
- Cordis-тест ожидания teardown адаптирован через наблюдение реального runtime-owned scope fiber: tools исчезают при начале disposal, а disposer провайдера завершается только после disposal этого fiber. Отдельно проверены disposal scoped owner и unload runtime.

## Не проверено

- Реальная MCP-сеть, capability negotiation и pagination принадлежат будущему `hoh-mcp-client`; провайдерный seam проверен deterministic probes без сети.
- PTC end-to-end станет проверяемым после появления соответствующих workspace-крейтов; менять чужой API или вводить временный runtime ради этого пакета не требовалось.

## Вопросы ведущему

Нет. Чужие крейты и API не менялись; SHIM не потребовался. Новых внешних зависимостей нет.

## Проверка

- `cargo test -p hoh-mcp-resources --offline`: 20 passed, 0 failed.
- `cargo clippy -p hoh-mcp-resources --all-targets --no-deps --offline -- -D warnings`: чисто.
- `cargo fmt --check -p hoh-mcp-resources`: чисто.
- `diff -r` locale с исходным пакетом: различий нет.

Все команды выполнялись с `CARGO_BUILD_JOBS=2`, `CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x5`, offline и под внешним `timeout`.
