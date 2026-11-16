# hoh-scope ← packages/core/scope @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | все 13 собственных экспортов; синхронный scope, точный disposer, исходная композиция фильтра |
| `src/store.ts` | `src/store.rs` | все 4 экспорта; живые таблицы, поколения, слияние, владение и откат эффектов |
| `tests/scope.spec.ts` | `tests/scope.rs` | все 11 исходных случаев; ещё 7 собственных проверок |
| `tests/store.spec.ts` | `tests/store.rs` | все 10 исходных случаев; ещё 6 собственных проверок |
| `README.md` | rustdoc API и эти заметки | прочитан, контракт учтён |

У образца нет `locale/`, отдельного `LICENSE` или отдельно лицензированного фрагмента.
Образец зависит только от Cordis; перенос использует существующий `hoh-kernel`, включая API четвёртого круга.
Прочитаны `hoh-kernel/PORT_NOTES.md` от «Четвёртый круг: добавленный API» до конца и ext-stage0-k1 §B.
`hoh-error` используется для общего `BoxError`; его API и заметки прочитаны.
Остальные зависимости взяты из workspace: `indexmap`, `thiserror`; `serde_json` и `tokio/test-util` только для тестов.
В образце нет trim, путей, readdir, системных ошибок, JSON-текста и таймеров: соответствующие общие
помощники PORTING §4 здесь не вызываются. Поведение JS Map воспроизведено живыми итераторами таблиц.

## Отличия от образца

### Публичный API для следующих пакетов

`create_scope(&ctx, key, CreateScopeOptions::default()) -> Result<Scope, BoxError>` — **синхронная**
функция: до первого await возвращается настоящий контекст дочернего fiber с исходным именем `scope`.
`scope.ctx` — обычный `Context`; созданные через него эффекты принадлежат ребёнку, включая регистрации
до активации. `scope.dispose()` отправляет запрос сразу и возвращает future полного покоя. Повторные
вызовы и raw-first вызовы присоединяются к той же остановке через `Fiber::dispose()`.

Короткий пример потребителя (`Ping` и `consume` определены самим потребителем):

```rust
use hoh_scope::{create_scope, CreateScopeOptions, ScopeKey};

let key = ScopeKey::new();
let scope = create_scope(&ctx, key, CreateScopeOptions::default())?;
scope.ctx.on::<Ping>(|event| consume(event))?; // usable immediately; owner is the scope
scope.dispose().await;                       // listener and other scope effects are gone
```

`scope.raw_dispose() -> hoh_kernel::Disposer` возвращает **точный** disposer из `Fiber::disposer()`.
Это accessor вместо поля-функции JS: kernel handle потребляется, поэтому новый accessor-вызов выдаёт
handle той же записи. Scope сохраняет исходный handle, так что accessor доступен и после teardown.
Никакой `ScopeDisposer`-обёртки больше нет. Для упорядоченного включения: `undos.push(scope.raw_dispose())?`.
Adoption снимает самостоятельное владение child-effect у родителя; выгрузка самого родителя тоже
дожидается асинхронных соседей в точном порядке inner → scope → outer.

`scope_target(base: Arc<T>, key: Option<ScopeKey>) -> Scoped<T>`, где `T: ScopeFilter`, сохраняет тип
предмета без предоставления его полей. Trait `ScopeFilter` — типизированное свойство `Context.filter`:
обычный предмет реализует пустой `impl ScopeFilter for Subject {}`, фильтрованный предмет переопределяет
`matches_filter(&self, listener: &Context)`. `Context` реализует trait через свой существующий
`matches_filter`; `()` — предмет без фильтра; сам `Scoped<T>` тоже сохраняет свой фильтр как base.
`Arc` сохраняет точный receiver вызова. При `carrier.context(&emitter).emit(&event)` сначала вызывается
фильтр **base**, затем читается ближайшая scope-метка **listener** и его допуск по цепочке родителей.
Фильтр emitter не является фильтром receiver и не добавляется: как `emit(thisArg, ...)` образца.
`Listen::global()` обходит весь фильтр. `with_base_filter` и ручная передача фильтра удалены.

`bind_scope_parent(&key, parent)` возвращает `ScopeParentBinding`; только он делает `rebind(parent)`.
Проверка already-bound предшествует проверке cycle. `scope_parent_of(&key)` читает только прямую связь;
`scope_chain_of(Option<&ScopeKey>)` возвращает `[key, parent, …]`. Вложенный Context сам по себе
не создаёт связь ключей: её задаёт только bind или `CreateScopeOptions { parent: Some(...) }`.
`scope_of(&ctx)` читает приватную метку и учитывает ближайшее затенение.

Идентификаторы `ScopeKey` — отдельные значения, сохраняющие тождество при `Clone`, вместо произвольного
объекта JavaScript. Предмет хранит свой ключ отдельно; сравнение и Hash зависят только от неизменной
идентичности. Родитель хранится в самом ключе, таблицы и переносчики не используют глобальные реестры.
Единственный static — разрешённый AtomicU64 выдачи уникальных id; предел 2^64 выдач — инвариант времени
жизни процесса. Обход, проверка и освобождение неограниченных цепочек итеративны. Между потоками записи
родителя и чтение цепочки проверяют согласованный снимок под блокировками с единым порядком.

`is_scope_carrier(&Scoped<T>)` и `carrier_key_of(&Scoped<T>)` сохраняют keyed/unkeyed различие;
непереносчик не принимается типами Rust. Непрозрачность предмета и отрицательный вызов проверены
compile-fail doctests вместо JS `in`/unknown/Symbol-проверок (§8 PORTING). Это языковая адаптация,
а не ограничение ядра. Payload события содержит реальный предмет.

### Таблицы и эффекты

`NamedEntries<V, E = BoxError>` предоставляет `insert`, `get`, `has`, `keys`, `entries`, `values`, `is_empty`.
Ошибка duplicate создаётся вызывающим и возвращается без преобразования; `insert -> Result<EntryUndo, E>`.
`AnonymousEntries<V>` предоставляет `append`, `values`, `is_empty`; равные значения имеют независимые undo.
`EntryUndo::undo()` синхронен и идемпотентен; `Into<Undo>` передаёт отмену контексту. Старый undo никогда
не удаляет повторную вставку с тем же именем. После удаления последней записи меняется поколение таблицы;
старый итератор не видит новых вставок. До исчерпания итератор живой, после исчерпания остаётся завершённым.
Чтение/итерация требуют `V: Clone`: для исходных заимствованных объектов нужно хранить `Arc<T>`.
Тесты проверяют сохранение идентичности значений, а не только их равенство.

`ScopeLayer` — trait с `is_empty`. `ScopedLayers<L>::global: Arc<L>` создаётся сразу; `peek` никогда
не создаёт overlay и не читает предков. `chain_layers` возвращает существующие слои дальних предков
сначала; `merge` возвращает `IndexMap`, где ближайший слой выигрывает без перемещения позиции имени.
`new` принимает неошибающиеся factory/onChange, `try_new` — callbacks с `Result`.
`EffectOptions::new(label)` задаёт исходный `notify = true`; поле `notify: false` отключает оба уведомления.

`ScopedLayers::effect(&ctx, action, options)` непосредственно возвращает disposer контекста:
`Context` вызывает настоящий `try_effect`, действие возвращает `Result<EntryUndo, BoxError>`, undo
собирается **до** onChange. Ошибка уведомления откатывает вставку; освобождается только полностью пустой
агрегат слоя. Ошибки factory и action сохраняют исходный порядок и условия удаления слоя. Ошибка
cleanup-уведомления оставляет удаление выполненным и логируется с исходной меткой эффекта.
Синхронные мутации атомарны между потоками; повторный вход того же потока из factory/action/onChange
допускается, внутренние mutex не удерживаются при callbacks или await.

Дополнительный trait `store::RegistrationContext` выражает структурный аргумент исходного `effect`.
У него associated `Disposer`, методы `scope_key`, `logger`, `try_effect`; обычный `Context` реализует его
прямой передачей в ядро. Это позволяет перенести исходный mock **без** запуска setup: проверяются точное
тождество возвращённого значения, единственный label и невызванный action. В production вызывающий
по-прежнему передаёт `&Context` и получает настоящий `hoh_kernel::Disposer`.

Новые публичные символы помимо исходных экспортов: `ScopeError`, `ScopeFilter`,
`store::{EntryUndo, EffectOptions, RegistrationContext, Values, Keys, Entries}` и их описанные методы.
Стандартные `new`/`Default`/`Clone` заменяют конструкторы и копирование ссылок JS. `Clone` таблиц и
слоёв разделяет исходное состояние, не копирует регистрации. Labels имеют тип `&'static str`, как у ядра.

### Ошибки и устранённые ограничения ядра

`index.ts:60,74` бросает **обычный `new Error(...)`**, не HarnessError и не его наследника.
`ScopeError` — обычный enum с thiserror, без поля code и без превращения в HarnessError.
Оба исходных сообщения сохранены дословно. Тест через BoxError проверяет downcast в ScopeError
и отрицательный `HarnessError::of`. Ошибки factory/action/onChange передаются как BoxError.

Прежние пять вопросов закрыты реализацией kernel R66–R70:

| требование образца | используемый API ядра | проверка через hoh-scope |
|---|---|---|
| index.ts:136–147, синхронные регистрации | `Context::child("scope")` | исходный pre-activation case и dispose до активации |
| index.ts:115–118,143–145, raw и общий покой | `Fiber::disposer`, `Fiber::dispose` | исходный raw-first case; async ordered parent teardown с заблокированными inner и scope |
| index.ts:140,150,174, ближайшая метка listener | `with_tag` / `tag` | исходные nearest-tag, ancestor и global cases |
| index.ts:170–183, base-first с исходным receiver | `Context::matches_filter` | исходный receiver case, отдельный base Context и фильтрованный carrier как base |
| index.ts:127–140, committed путь minter | `Context::child` и обычный `get` | чтение во время cleanup после withdrawal, isolation и освобождение последней Arc |

Handoff, oneshot, асинхронный mint, ручной base-filter и raw-wrapper удалены; код чужих crates не копируется.
Новый receiver/key API не нужен: kernel notes ссылаются на agent/dispatch.ts:113–125,
tools/pipeline.ts:426–429 и session/index.ts:895–899 — переносимые слушатели берут предмет из payload.
Production `carrierKeyOf(this)` найден только в api/remotes/src/index.ts:60–69 (Typert, исключён §8).
Старые вопросы о receiver и невидимом filter/view больше не являются незавершённой работой.

### Потребители экспортов

До переноса просмотрены импорты из переносимых packages/**/src без client/api/typert/host/experimental:
tools, system-prompt, session, agent, agent-loop, tool-executor-ptc, jobs-local, mcp-resources, mcp-client,
agent-preset-registry, commands, user-questions, user-approval, subagent, skill, cordis-host-runner.
Все используемые экспорты представлены. В production-коде этих пакетов не используются
`isScopeCarrier`, `carrierKeyOf` и имя типа `CreateScopeOptions`; они всё равно перенесены.

## Не перенесено

Нет пропущенных исходных runtime-cases: **21 из 21**, включая ранее пропущенные
`is_usable_synchronously_before_the_backing_fiber_activates` и `returns_the_exact_context_effect_disposer`.
JS type/unknown/Symbol-assertions выражены типами и двумя compile-fail doctests, не объявляются
выполненными runtime-проверками JavaScript. `getEffects`/повторный вызов consuming Disposer заменены
наблюдением label через журнал и однократной отменой при раннем dispose плюс последующей выгрузке.
Два одинаковых исходных имени поколений в store различены префиксами named/anonymous.

## Не проверено

Платформенных веток, сети и внешнего I/O нет. Async-тесты используют остановленное время и явные gate,
без привязки к реальному времени; opposing bind проверяется на 16 парах в двух независимых потоках.
Все пять ранее недоступных runtime-контрактов проверены через публичный API hoh-scope.

## Вопросы ведущему

Нет. Дополнительных API ядра не требуется; незавершённых частей переноса нет.

## Проверки

Команды из корня workspace, каждая с
`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w2`:

- `cargo test -p hoh-scope`: scope — **18 passed; 0 failed**; store — **16 passed; 0 failed**;
  doctests — **2 passed; 0 failed**; unit — 0/0. Всего **36 passed; 0 failed**.
- `cargo clippy -p hoh-scope --all-targets -- -D warnings`: exit 0.
- `cargo fmt -p hoh-scope -- --check`: exit 0.

Изменения ограничены `crates/hoh-scope`; source dsh и остальные crates только прочитаны.
