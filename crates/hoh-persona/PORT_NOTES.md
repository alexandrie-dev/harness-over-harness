# hoh-persona ← packages/preset/persona @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` (75 строк) | `src/lib.rs` | все экспорты: `name`, `inject`, `Config`, `apply`, обе константы имён секций (реэкспорт из `hoh-system-prompt`) |
| `tests/persona.spec.ts` | `tests/persona.rs` | все 11 случаев (`it.each` из двух параметров — цикл внутри одного теста); проверки не ослаблены |
| `locale/*.json` (8 файлов) | `locale/` | скопированы без изменений |
| `README.md`, `README.zh.md`, `README.i18n.yaml`, `package.json` | эти заметки | прочитаны, контракт учтён |

У образца нет `LICENSE` и отдельно лицензированных фрагментов. Зависимости: `hoh-kernel`, `hoh-system-prompt`
(сервис `SystemPromptKey`, `PromptSection`, порядок секций), общие `serde` и `schemars` (конфигурация).
Для тестов: `hoh-scope` (`create_scope`, `scope_of`; в образце это `devDependencies`), `serde_json`, `tokio/test-util`.
Код чужих crates не менялся и не копировался.

## Отличия от образца

1. **Плагин.** `export const name = 'persona'` и `inject = ['systemPrompt']` → `#[plugin(Persona, name = "persona")]`
   над `pub async fn apply(ctx, deps, config)` и `#[derive(Inject)] struct Deps { system_prompt: Service<SystemPromptKey> }`.
   Имя fiber — `Persona::NAME == "persona"`; дополнительно `pub const NAME` с тем же значением. Строка загрузчика
   `@deepseek-ai/dsh-persona` → этот плагин: регистрация в таблице имён — за ведущим (`hoh-official-bundles`).
2. **`Config`** — структура с `rename_all = "camelCase"`: `prefix` (обязательное, без умолчания), `suffix` (`""`),
   `complete` (`false`), `includeRuntimeContext` (`true`). Ключи, умолчания и порядок полей как в `z.object` образца.
   В образце `suffix`, `complete`, `includeRuntimeContext` в типе необязательны (`?`), и `apply` сам подставляет
   `?? ''` и `?? true`, чтобы пережить вызов в обход схемы. В Rust у полей типа `String`/`bool` нет «отсутствует»,
   умолчания лежат в serde, поэтому эти `??` не нужны.
3. **`apply` не вызывается в обход схемы.** Тест образца «keeps runtime context by default when apply bypasses schema
   defaults» вызывает `Persona.apply(scopedCtx, { prefix })` напрямую. `Cfg<C>` вне ядра не построить, а обхода схемы
   в Rust нет. Случай перенесён как есть по смыслу (конфигурация только с `prefix` оставляет контекст среды),
   но проходит через загрузку со схемой; ветку `?? true` он не исполняет — её в переносе нет.
4. **Эффекты.** `ctx.effect(() => ctx.systemPrompt.section({...}), 'persona.section()')` →
   `ctx.try_effect("persona.section()", |undos| undos.push(prompt.section(&ctx, ...)?))`: точный `Disposer`
   регистрации секции переходит во владение составного эффекта с той же меткой (`persona.section()`,
   `persona.suffix()`). `suppressRuntimeContext()` вызывается без обёртки, как в образце; его `Disposer`
   остаётся у fiber.
5. **Поле `complete`** — `bool` (образец проверяет `config.complete ? { complete: true } : {}`); у `PromptSection`
   в Rust оно `bool`, `false` = не задано. У суффикса поле `complete` не задаётся, как в образце.
6. **Порядок секций** — `SystemPrompt::get_section_order(DeploymentPersonaPrefix | DeploymentPersonaSuffix)`
   вместо `ctx.systemPrompt.getSectionOrder('DEPLOYMENT_PERSONA_PREFIX')`.
7. **Тест «rejects an unscoped mount»** проверяет текст из `fiber.settled()` (`"deployment:persona-prefix" is
   already registered`), как `rejects.toThrow(/.../)` образца: подстрока, не равенство.

Публичные символы, которых нет в образце: `Persona` (тип плагина, порождён макросом), `Deps`, константа `NAME`.

Экспорты, которые переносимые пакеты не используют в коде: все. В исходниках `packages/*` (кроме исключённых)
на `@deepseek-ai/dsh-persona` ссылаются только строки конфигурации и документация
(`apps/cli/package.json`, `agent-preset/skills/dsh-doc`); `PERSONA_PREFIX_SECTION` и `PERSONA_SUFFIX_SECTION`
в других пакетах импортирует только `system-prompt` (их владелец). Всё перенесено.

## Не перенесено

- `import type {} from '@deepseek-ai/dsh-system-prompt'` — подмешивание объявлений Cordis (PORTING §8).
- `Config: z<Config>` (`schemastery`) — вместо неё `Plugin::Config` со `schemars`.
- Обход схемы при прямом вызове `apply` (п. 3).

## Не проверено

- Явный `null` в `suffix` и неправильные типы значений: тексты отказа схемы не сравнивались с `schemastery`.
- Поведение при смонтированном `isolate` сервиса `systemPrompt` (см. отличие 6 заметок `hoh-system-prompt`).

## Вопросы ведущему

1. Зависимость `hoh-scope` добавлена в `[dev-dependencies]` (в манифесте-заготовке её не было; в образце она
   в `devDependencies`). Нужна только тестам.

## Проверки

Команды из корня workspace, каждая с
`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-s4`:

- `cargo test -p hoh-persona`: persona — **11 passed; 0 failed**.
- `cargo clippy -p hoh-persona --all-targets -- -D warnings`: чисто.
- `cargo fmt -p hoh-persona -- --check`: чисто.
