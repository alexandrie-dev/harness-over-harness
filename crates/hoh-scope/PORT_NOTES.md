# hoh-scope ← packages/core/scope @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | все 13 собственных экспортов представлены; ограничения ядра ниже |
| `src/store.ts` | `src/store.rs` | все 4 экспорта; живые итераторы, поколения таблиц, откат и владение эффектами |
| `tests/scope.spec.ts` | `tests/scope.rs` | 10 из 11 исходных случаев с адаптациями; ещё 2 собственных теста |
| `tests/store.spec.ts` | `tests/store.rs` | 9 из 10 исходных случаев с адаптациями; ещё 5 собственных тестов |
| `README.md` | документация API и эти заметки | прочитан, поведение учтено |

У пакета нет `locale/`, отдельного `LICENSE` или заимствованного фрагмента с отдельной лицензией.
Cordis заменён существующим `hoh-kernel`; его публичный API и `PORT_NOTES.md` прочитаны.
Других библиотек образец не использует. Дополнительные Rust-зависимости взяты только из workspace:
`indexmap`, `serde_json`, `thiserror`, `tokio`; `tokio/test-util` только для тестов.

## Отличия от образца

### Соответствие публичного API

| образец | Rust |
|---|---|
| `ScopeKey` (произвольный объект, сравниваемый по тождеству) | `ScopeKey::new()`, `Clone`, `Eq`, `Hash` по неизменной идентичности; предмет хранит свой ключ отдельно |
| `ScopeParentBinding`, `bindScopeParent` | `ScopeParentBinding`, `bind_scope_parent(&key, parent) -> Result<_, ScopeError>` |
| `scopeParentOf`, `scopeChainOf` | `scope_parent_of(&key)`, `scope_chain_of(Option<&ScopeKey>)` |
| `CreateScopeOptions` | одноимённая структура с `parent: Option<ScopeKey>`, `Default` |
| `Scope`, `createScope` | `Scope`, `create_scope(&ctx, key, options).await -> Result<Scope, hoh_kernel::Error>` |
| `Scope.ctx`, `Scope.rawDispose`, `Scope.dispose()` | `ctx: Context`, `raw_dispose: ScopeDisposer`, `dispose()` возвращает ожидаемый future |
| `scopeOf` | `scope_of(&ctx)`; приватный тип метки сохраняет ближайшее значение |
| `Scoped<T>`, `scopeTarget` | `Scoped<T>`, `scope_target(&subject, Option<ScopeKey>)`; контекст рассылки — `carrier.context(&ctx)` |
| `isScopeCarrier`, `carrierKeyOf` | `is_scope_carrier(&Scoped<T>)`, `carrier_key_of(&Scoped<T>)`; непереносчик отвергается типами |
| `ScopeLayer` | одноимённый trait с `is_empty()` |
| `NamedEntries<V>` | `NamedEntries<V, E = hoh_kernel::Error>`; фабрика ошибки вызывающего, `insert -> Result<EntryUndo, E>` |
| `AnonymousEntries<V>` | одноимённый тип, `append -> EntryUndo` |
| `ScopedLayers<L>` | одноимённый тип; `global: Arc<L>`, `peek`/`chain_layers` возвращают `Arc<L>`, `merge` — `IndexMap` |
| `ScopedLayers.effect` | `effect(&ctx, action, EffectOptions) -> Result<Disposer, hoh_kernel::Error>`; action возвращает `Result<EntryUndo, _>` |

Новые публичные имена: `ScopeError` (две обычные ошибки с дословными исходными сообщениями),
`ScopeDisposer` (тип raw-границы вместо функции), `store::EntryUndo` (клонируемое право на точную
синхронную отмену через `.undo()`, а также `Into<Undo>`), `store::EffectOptions` (обязательный `label`
и `notify`, конструктор `new(label)` задаёт `true`), `store::{Values, Keys, Entries}` (типы живых
итераторов). `ScopedLayers::try_new` выражает бросающие фабрику слоя и уведомление через `Result`;
обычный `new` принимает неошибающиеся функции. `ScopeDisposer: Into<Undo>` позволяет включать
raw-границу в составной эффект. `Scoped::{context, admits, with_base_filter}` выражают маршрутизацию
без JS-переносчика с символьным свойством. Стандартные `new`, `Default`, `Clone` заменяют конструкторы
и копирование ссылок JS; `Clone` таблиц и слоёв сохраняет общий объект, а не копирует регистрации.

Значения при чтении/итерации требуют `V: Clone`; для исходного заимствованного объекта нужно хранить
`Arc<T>`, а не делать глубокую копию `T`. Проверка идентичности `Arc` есть в тесте anonymous entries.
Синхронный undo идемпотентен и снимает конкретный слот, поэтому старая отмена не удаляет новую
регистрацию под тем же именем. Удаление последней записи меняет поколение; исчерпанный итератор
никогда не оживает. Порядок слияния и сохранение позиции затенённого имени совпадают с JS Map.

Глобальных изменяемых таблиц нет: родитель хранится в самом ключе, переносчик хранит свой ключ.
Единственный `static` — разрешённый счётчик уникальных идентификаторов; исчерпание 2^64 ключей вне
представимого времени жизни процесса. Цепочки обходятся и освобождаются итеративно. Параллельные
записи родителя и чтение цепочки проверяют согласованный снимок под упорядоченными блокировками.
Синхронные изменения реестра сериализованы между потоками; повторный вход того же потока из action,
factory или onChange разрешён. Внутренние mutex не удерживаются при вызове этих callbacks или через await.
Ошибки callbacks выражаются `Result`; паника подчиняется правилам изоляции ядра.

### Ограничения существующего ядра

1. В `Fiber` нет публичного контекста до активации. Поэтому `create_scope` асинхронен: приватный
   плагин с исходным именем **`scope`** передаёт свой настоящий `Context` из apply через oneshot.
   Возвращённый контекст владеет регистрациями и наследует теги, realms и intercepts; контекст родителя
   вместо дочернего не подставляется. Опциональная связь parent ставится до создания fiber, как в образце.
   Синхронная доступность до запуска child apply пока не реализована.
2. В `Context` нет метода прочитать/скомпоновать уже установленный filter: `with_filter` заменяет его.
   `scope_target` сохраняет тип предмета, но не извлекает скрытый фильтр предмета. Его нужно передать явно
   через `.with_base_filter(move |listener| base.filter(listener))`, захватив правильный receiver.
   Такой фильтр вызывается до проверки scope. Автоматическое сохранение фильтра произвольного base или
   фильтрованного `Context` пока не реализовано; `.context(&ctx)` заменяет filter этого ctx.
3. Ядро не выдаёт `Disposer` родительского эффекта, владеющего дочерним fiber. `ScopeDisposer` обращается
   к настоящему `Fiber::dispose`, повторные вызовы ожидают общую остановку. Явное снятие составного
   эффекта соблюдает порядок `inner, scope, outer` (исходный тест перенесён). Однако это не точная
   идентичность исходного Cordis disposer и не перенос владения родительским child-effect через
   `Undos::push`: при выгрузке всего родителя child-effect остаётся самостоятельным. Строгий порядок
   относительно асинхронных соседей в этом сценарии без API adoption child-effect не гарантирован.
4. Контекст child не получает committed dependency view minter: kernel `Context::binding` ищет свой
   view и опубликованную таблицу, а не view предка. Обычные опубликованные сервисы доступны; доступ через
   scoped ctx к уже снятому, но удержанному инъекцией minter сервису не гарантирован. Для полного
   исходного контракта нужен механизм ядра наследовать этот view/путь разрешения.
5. `Context` — конкретный тип, не подменяемый объектом с методом effect; списка эффектов с labels нет
   в публичном API. `Disposer` потребляется один раз. Метод `ScopedLayers::effect` непосредственно
   возвращает результат `ctx.try_effect` без обёртки. Дословная JS-проверка mock-идентичности невозможна;
   label проверен через публичный журнал ошибки cleanup, идемпотентность — ранним снятием и shutdown.

`is_scope_carrier` принимает только типизированный переносчик, поэтому всегда истинен; это следствие
статического ограничения параметра, а не динамическая проверка произвольного JS value. Исходные проверки
непереносчика и отсутствия полей предмета выражены compile-fail doctests. `carrier_key_of` сохраняет
различие keyed/unkeyed; в kernel-слушателях нет JS `this`, поэтому получение ключа из receiver слушателя
требует отдельного решения на уровне событий. Kernel ограничивает labels строками `&'static str`.

### Потребители экспортов

Проверены импорты из переносимых `packages/**/src` без client/api/typert/host/experimental и окон:
tools, system-prompt, session, agent, agent-loop, tool-executor-ptc, jobs-local, mcp-resources,
mcp-client, agent-preset-registry, commands, user-questions, user-approval, subagent, skill,
cordis-host-runner. Все используемые ими экспорты представлены. Нет использования в исходном коде
переносимых пакетов у `isScopeCarrier`, `carrierKeyOf` и имени типа `CreateScopeOptions`; эти экспорты
всё равно перенесены. Первые два используются тестами scope/user-approval/subagent.

## Не перенесено

- `tests/scope.spec.ts`: `is usable synchronously before the backing fiber activates` — нет
  публичного синхронного child-context API ядра (ограничение 1). Не подменён ослабленным тестом.
- `tests/store.spec.ts`: `returns the exact context effect disposer` — JS-mock конкретного
  `Context` неприменим (ограничение 5). Непосредственный возврат `ctx.try_effect` проверяется чтением кода.
- JS WeakMap, Symbol и declaration merging заменены типами и собственным состоянием (§8 PORTING).
  Динамические отрицательные проверки непереносчика заменены compile-fail, не объявлены runtime-проверками.

Всего 21 исходный it: 19 перенесены с указанными адаптациями, 2 не перенесены.
Имена исходных случаев сохранены в snake_case; два одинаковых имени поколения в store различены
префиксами named/anonymous. У исходного теста ordered raw disposer проверен порядок, не JS function identity.
У теста order/idempotence/labels introspection заменена журналом label и однократным снятием эффекта.

## Не проверено

Платформенных веток и внешнего I/O в пакете нет. Тесты без сети; async-тесты с остановленным временем.
Два независимых потока проверяют противонаправленные bind на 16 парах ключей.
Полный исходный контракт синхронного mint, скрытого base-filter, raw adoption при выгрузке родителя,
наследования удержанного committed view и JS receiver ограничен пунктами выше и не заявляется выполненным.

Дополнительные тесты: carrier следует rebind и сохраняются имя scope/intercept; одновременные bind не
замыкают цикл; merge дальних/ближних предков, отсутствие побочных слоёв при чтении и снятие при dispose;
живые keys/entries при удалении/повторной вставке; повторный вход из action; исходный label в журнале;
неактивный контекст не вызывает фабрику/action. Два compile-fail doctest проверяют непрозрачность carrier.

## Вопросы ведущему

Нужны решения/расширения **hoh-kernel**, сам этот crate зависимости не меняет:

- синхронно доступный дочерний Context с корректным владением эффектами;
- публичная композиция существующего dispatch filter;
- получение/adoption точного child-fiber Disposer в составном эффекте;
- наследование scoped ctx пути разрешения committed dependencies minter, включая teardown;
- нужно ли событию нести типизированный carrier/dispatch key для потребителей исходного `this`.

Пока приняты явные Rust-адаптации выше; полная эквивалентность этим пяти аспектам не достигнута.

## Проверки

Каждая команда выполняется из корня workspace с
`CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w2`.

- `cargo test -p hoh-scope`: integration scope — **12 passed; 0 failed**; integration store —
  **14 passed; 0 failed**; doctests — **2 passed; 0 failed**; unit — 0/0. Всего **28 passed; 0 failed**.
- `cargo clippy -p hoh-scope --all-targets -- -D warnings`: exit 0.
- `cargo fmt -p hoh-scope -- --check`: exit 0.

Изменения писателя ограничены `crates/hoh-scope`; исходник dsh только прочитан.
