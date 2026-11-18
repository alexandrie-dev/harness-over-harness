# hoh-system-prompt ← packages/core/system-prompt @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` (699 строк) | `src/lib.rs` | все экспорты: сервис `SystemPrompt`, `Config`, два события, `render_*`, порядки секций и контекстов, обе константы имён персоны, `TOOL_ORDER_REST`; весь код образца в одном файле, как в образце |
| `tests/system-prompt.spec.ts` | `tests/system_prompt.rs` | все 53 `it`/`it.each` (параметризованные — циклами внутри теста), плюс 4 дополнительных (UTF-16 порядок имён, 16 единиц в тексте ошибки, пустой снимок, схема конфигурации) |
| `tests/scoped.spec.ts` | `tests/scoped.rs` | все 14 случаев |
| `tests/tool-order.spec.ts` | `tests/tool_order.rs` | все 12 случаев |
| `locale/*.json` (8 файлов) | `locale/` | скопированы без изменений |
| `README.md`, `README.zh.md`, `README.i18n.yaml`, `package.json` | эти заметки | прочитаны, контракт учтён |

Общие помощники тестов — `tests/common/mod.rs`. У образца нет `LICENSE` и отдельно лицензированных
фрагментов. Зависимости: `hoh-kernel`, `hoh-llm` (`ToolSchema`, `ContextSnapshotSection`), `hoh-scope`
(`ScopedLayers`, `NamedEntries`, `AnonymousEntries`, `scope_target`), общие `hoh-error`, `hoh-timeout`
(`AbortSignal` в `AssembleContext`), `hoh-util-values` (текст `JSON.stringify` в сообщении), `futures-util`,
`indexmap`, `schemars`, `serde`, `serde_json`, `thiserror`. Код чужих crates не менялся и не копировался.

## Отличия от образца

1. **Методы сервиса принимают `ctx: &Context` первым параметром** (`section`, `context`,
   `suppress_runtime_context`, `tools`, `variable`): в образце `this.ctx` — контекст вызывающего (Proxy
   сервиса), регистрация становится его эффектом (docs/kernel.md §7). Возвращают
   `Result<Disposer, BoxError>`: `Disposer` — «точный disposer Cordis»; `dispose()` снимает запись
   синхронно (недожидаться возвращённого future можно). `assemble` и `refresh_context` контекста вызывающего
   не принимают (см. п. 6).
2. **Плагин.** Образец — класс-плагин без `export const name`: имя fiber — имя класса. Перенос:
   `impl Plugin for SystemPrompt`, `NAME = "SystemPrompt"`, `Config = Config`, `Deps = ()`. Сервис
   предоставляется ключом `SystemPromptKey` (`"systemPrompt"`). `ctx.plugin::<SystemPrompt>(json)`, затем
   `ctx.get::<SystemPromptKey>()`. Строка загрузчика `@deepseek-ai/dsh-system-prompt` → этот плагин:
   регистрация в таблице имён — за ведущим (`hoh-official-bundles`).
3. **`Config`** — структура с `#[serde(default, rename_all = "camelCase")]`, ключи и умолчания образца
   (`includeHarnessIdentity` и `includeRuntimeContext` — `true`, обе персоны — `""`, `toolOrder` отсутствует).
   Умолчания лежат в схеме, поэтому `?? true` и `?? ''` конструктора образца не нужны: при прямом вызове
   `SystemPrompt::new(ctx, &Config::default())` («tolerates a schema-bypassing direct construction») поля
   всегда заполнены. Проверка `toolOrder` (дубль, нет `<unlisted-tools>`) идёт в конструкторе, как в образце:
   плагин не активируется, текст ошибки виден через `Fiber::settled()`. Отличие схемы: у `toolOrder`
   тип `["array","null"]` (так `schemars` описывает `Option`), поэтому явный `null` принимается как «не задан»;
   В источнике (`vendor/schemastery/src/index.ts:533–542`, 'object' на 757–764) явный `null` без
   умолчания сохраняется, и `validateToolOrder` падает в `for...of`: `TypeError: toolOrder is not iterable`. Перенос:
   `Config::tool_order: Option<Option<Vec<String>>>` (`None` — ключа нет, `Some(None)` — явный `null`,
   serde-функция `explicit_null`; схема по-прежнему `["array","null"]`), конструктор возвращает
   `SystemPromptError::ToolOrderNotIterable` с текстом `toolOrder is not iterable` (V8 печатает имя параметра).
   Тесты `review_fixes.rs`. Явный `null` у полей с умолчанием (`includeHarnessIdentity`, `includeRuntimeContext`,
   `personaPrefix`, `personaSuffix`) источник заменяет умолчанием; это правило ядра (kernel round 7), четыре
   теста `review_fixes.rs` с `#[ignore = "kernel round 7: schemastery null rule"]` — снять, когда оно появится.
4. **`AssembleContext`: слияние объявлений заменено расширениями по типу.** Образец дополняет интерфейс из
   других пакетов (`agent`, `modelSelection` в `agent`, `toolSet` в `tools`; тест кладёт `who`). В Rust
   структура хранит `scope`, `signal` и таблицу значений по типу: `with_extension(value)` и `extension::<T>()`.
   Пакет-расширитель объявляет свой тип и читает его. `assemble` принимает `impl Into<Arc<AssembleContext>>`,
   чтобы слушатели получали тот же объект (тест «the caller's context reaches listeners» сравнивает по
   тождеству через `Arc::ptr_eq`). Значение по умолчанию `{}` — `AssembleContext::default()`.
5. **Поставщики могут «бросать».** Текст секции и контекста (`PromptText::provider`), переменная, поставщик
   инструментов возвращают `Result<_, BoxError>`; ошибка поставщика сорвёт сборку, как исключение в образце.
   Статический текст — `PromptText::Static` (`From<&str>`, `From<String>`). Поля `complete` и `required` —
   `bool` (образец проверяет только `=== true`); `interpolate` остаётся `Option<bool>`, потому что
   `resolveContext`/`assemble` различают «не задано» и `true` (тест «preserves literal context»).
6. **Контекст рассылки `system-prompt/assemble`.** Образец (`index.ts:684`, `vendor/cordis/src/service.ts:64`,
   `utils.ts:163–204`) рассылает с `scopeTarget(this, scope)`; `this` — прокси вызывающего, `this.ctx` — контекст
   вызывающего, и базовый фильтр сравнивает область (`isolate`) слушателя с областью *вызывающего*. Значение сервиса в
   Rust не знает вызывающего, поэтому: `assemble_from(&caller, context)` (символ вне образца) берёт
   `caller.realm_of("systemPrompt")` (`RealmFilter(Realm)`), затем метка области (`scope_target`); `assemble(context)` —
   это `assemble_from` с контекстом самого сервиса (сигнатура сохранена для зависимых crates).
7. **События.** `system-prompt/assemble` — `SystemPromptAssemble` (режим `Waterfall<PromptPresentation>`):
   слушатель получает `Arc<SystemPromptAssemble>` и `Next`. `PromptPresentation` — общий объект цепочки
   (`Arc<Mutex<PresentationData>>`; клон — тот же объект, `same_object`): `PresentationData` держит поля
   `sections`, `contexts`, `variables`. Меняется через `event.update(|data| ..)` или `presentation.update(..)`,
   читается `event.assembly()` (тот же объект, не копия) и `presentation.snapshot()` (копия содержимого). Хвост
   цепочки возвращает тот же объект, который меняют слушатели (`index.ts:684–686`), поэтому добавление позднего
   слушателя видно раннему, вернувшему «свой» объект. Публичные символы вне образца: `PresentationData`,
   `PromptPresentation::{new, update, snapshot, same_object}`. У `PromptPresentation` нет
   поля `tools`, поэтому слушатель не может подменить объявления инструментов (в образце лишнее поле
   отбрасывается: `{...transformed, tools}`). `system-prompt/change` — `SystemPromptChange` (`Emit`), рассылается
   корневым контекстом (`ctx.root().emit`) без фильтра: в образце `this.ctx.emit('system-prompt/change')` с именем
   события первым аргументом — без получателя и без его фильтра (`events.ts:162–170`), поэтому фильтр рассылки
   контекста сервиса не действует (тест `change_notifications_reach_every_listener_…`). Режим события остался `Emit`,
   а не `Contained`: на `on::<SystemPromptChange>` опираются слушатели; рассылка без фильтра у `root().emit` та же. Паника слушателя `change` перехватывается (`catch_unwind`) и превращается
   в ошибку с текстом паники: так же, как исключение слушателя в образце ломает регистрацию и откатывает её
   (тесты P1-1); `ScopedLayers::effect` откатывает вставку.
8. **`assemble` исполняет поставщиков при вызове**, как `async` метод образца до первого `await`: возвращает
   `BoxFuture<'static, ..>`; ошибки конфигурации и поставщиков приходят через это future. Каскад проверок
   сохранён: переменные → слои → инструменты → «multiple complete» → текст секций → `orderTools` → контексты.
9. **`refresh_context(&PromptAssembly, &AssembleContext) -> Result<PromptAssembly, BoxError>`** возвращает
   новое значение; секции, инструменты и переменные в нём — равные копии, а не те же объекты (образец
   проверяет `toBe`; перенос проверяет равенство).
10. **Порядки.** `SECTION_ORDERS` и `CONTEXT_ORDERS` — перечисления `PromptSectionOrderName` и
    `PromptContextOrderName` (варианты в CamelCase: `ToolBash`, `HarnessSource`), значения — `f64`, как числа
    образца; `get_section_order`/`get_context_order` — ассоциированные функции без `&self`. Сравнение
    `a.order - b.order || compareNames` и `compareNames` (единицы UTF-16, не байты и не code point)
    перенесены; сортировка устойчивая, как у `Array.prototype.sort`.
11. **Переменные** — `Variables` (обёртка над `IndexMap<String, Option<String>>`; `None` — «зарегистрирована,
    значения нет»). Перечисление (`keys`, `iter`) идёт в порядке `Object.keys`: индексные ключи массива
    (`"2"`, `"10"`) по возрастанию, затем остальные по вставке (`index.ts:128, 627–635`); этот порядок видят слушатели
    waterfall, отрисовка и диагностика `registered variables: …`. `remove` — `delete`: повторная вставка уходит в конец.
    Публичный символ вне образца: `Variables`. Ключи вроде `__proto__`, которые JavaScript обрабатывает особо, не
    воспроизводятся. `Object.hasOwn` → `contains_key`.
12. **Текст `malformed prompt variable reference at "…"`** берёт 16 единиц UTF-16 от `{{`; разрезанная
    суррогатная пара даёт U+FFFD (Node при записи в UTF-8 делает то же).
13. **Ошибки реестра** — `SystemPromptError` (enum, `thiserror`): тексты дословно из образца; `TypeError`
    образца (нефинитный `order`) — `OrderNotFinite`. Это обычные ошибки, не `HarnessError`, как и в образце.

Публичные символы, которых нет в образце: `SystemPromptKey`, `SystemPromptError`, `PromptText`,
`AssembleContext::{new, with_extension, extension}`, `SystemPromptAssemble::{assembly, update}`,
`SystemPromptChange`, `Config` как отдельный тип вместо
`static Config`, `SystemPrompt::new` (конструктор класса).

Экспорты, которые по импортам в переносимых пакетах не используются (методы сервиса при этом вызываются
через `ctx.systemPrompt` везде): `renderContextSnapshot`, `joinContextSections`, `TOOL_ORDER_REST`,
`PromptContextOrderName` и типы `AssembledSection`/`AssembledContext`/`PromptPresentation`. Используются:
`renderPrompt`, `renderContextSections` (agent-loop), `PERSONA_PREFIX_SECTION`, `PERSONA_SUFFIX_SECTION`
(persona), `SystemPrompt` и `Config` (agent-loop-testkit), `AssembleContext`, `PromptAssembly`,
`PromptSection`, `ToolProviderResult`. Все перенесены.

## Не перенесено

- `declare module '@deepseek-ai/cordis' { interface Context { systemPrompt } ... Events }` — механизм слияния
  объявлений (PORTING §8); вместо него `SystemPromptKey` и типы событий.
- `export default SystemPrompt`, `z<Config>` (`schemastery`) — вместо них `Plugin::Config` со `schemars`.
- Проверки типов TypeScript в тестах (`@ts-expect-error`-подобные) отсутствуют; `expect(assembly).not.toHaveProperty('tools')`
  выражено типом (у `PromptPresentation` нет `tools`).

## Не проверено

- Платформенных веток, сети, файлов нет. `isolate` сервиса `systemPrompt` (п. 6) покрыт одним тестом (`review_fixes.rs`), тест образца его не знает.
- Явный `null` у полей с умолчанием (п. 3) — четыре игнорируемых теста ждут правила ядра (kernel round 7).

## Унаследовано от ядра (решения ядра, не этого раунда)

Вход ревью: `/mnt/toshiba/alexandrie/hoh/logs/review-hoh-system-prompt.codex.md`.
- Ревью п. 5: слушатель `change` A снимает последующий слушатель B во время рассылки. Источник
  (`vendor/cordis/src/events.ts:169–171, 207–209`) рассылает по замороженному снимку: `["A","B"]`; ядро
  (`hoh-kernel/src/events.rs:628`, R41) пропускает снятого: `["A"]`. То же у waterfall. Решает ядро.
- Ревью п. 6: прямое `dispose()` регистрации при падающем слушателе `change`. Источник
  (`scope/src/store.ts:257–260`, `fiber.ts:455–469, 513–526`) снимает и бросает; ядро возвращает `Disposer`
  без результата и пишет ошибку в журнал (ограничение задокументировано в PORT_NOTES ядра). Решает ядро.

## Вопросы ведущему

1. Устраивает ли замена слияния полей `AssembleContext` типизированными расширениями (п. 4)? Пакеты `agent`
   (`agent`, `modelSelection`) и `tools` (`toolSet`) должны объявить свои типы и класть их через
   `with_extension`; читать — `context.extension::<T>()`.
2. Методы сервиса принимают `ctx: &Context` (п. 1), а `assemble`/`refresh_context` — нет (п. 6). Если
   потребителю нужна область вызывающего для фильтра рассылки, добавить параметр можно без изменения
   остального.
3. Имя плагина `SystemPrompt` и ключ `SystemPromptKey` — по прецеденту `DeepSeekLlmApiExtensions`; если в
   `hoh-official-bundles` принято другое имя, поменять `NAME`.

## Проверки

Команды из корня workspace, каждая с
`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-s4`:

- `cargo test -p hoh-system-prompt`: system_prompt — **57 passed; 0 failed**; scoped — **14 passed; 0 failed**;
  tool_order — **12 passed; 0 failed**. Всего **83 passed; 0 failed**.
- `cargo clippy -p hoh-system-prompt --all-targets -- -D warnings`: чисто.
- `cargo fmt -p hoh-system-prompt -- --check`: чисто.
