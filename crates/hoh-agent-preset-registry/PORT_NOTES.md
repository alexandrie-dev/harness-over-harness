# hoh-agent-preset-registry ← packages/preset/agent-preset-registry @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | plugin/service, ревизии, leases, bind/recompose/select, remote list/read |
| `src/definition.ts` | `src/definition.rs` | конфигурация и рекурсивная проверка Loader rows |
| `src/display.ts` | `src/display.rs` | built-in ids и ключи переводов |
| `src/mount.ts` | `src/mount.rs`, `src/shim.rs` | mount/audit/leak check, fixed Loader runtime, YAML emitter |
| `src/preset.ts` | `src/preset.rs` | публичные `AgentPreset` и `Config` |
| `src/session.ts` | `src/session.rs` | projection `agentPreset`, header и durable selection events |
| `src/composition-inventory.ts` | `src/composition_inventory.rs` | declaration/runtime inventory, conditional rows |
| `src/types.ts` | `src/types.rs` | client payloads, ошибки и событие `agent-preset/selected` |
| `tests/*.spec.ts`, `tests/harness.ts` | `tests/*.rs`, `tests/support/mod.rs` | все 43 объявления и развёрнутые source cases; дополнительные lifecycle/review regressions |
| `locale/{de,en,es,fr,ja,pt,ru,zh}.json` | те же пути | побайтно скопированы |
| `README.md`, `README.zh.md`, `README.i18n.yaml`, `package.json`, `tsconfig.json` | эти заметки, rustdoc, Cargo manifest | прочитаны; build/i18n metadata не копируется |

Зависимости только workspace/path и уже закреплённые в `Cargo.lock`; новых внешних
пакетов нет. Чужие crates не менялись.

## Отличия от образца

- Cordis context proxy заменён явными `&Context`; сервис имеет ключ `AgentPresets =
  "agentPresets"`. `AgentPresetRegistry` остаётся plugin с исходным именем. Регистрации,
  scope bindings и projection принадлежат kernel effects и освобождаются вместе с ними.
- Ревизия — отдельный Loader scope. Удалённая/заменённая ревизия удерживается bindings и
  `PresetScopeLease`; `dispose()` идемпотентен, `Drop` выполняет синхронный release и запускает
  уже зарегистрированный scope disposer без блокировки. Acquire повторно проверяет identity
  definition под тем же mutex, поэтому удаление во время ожидания не привязывает старую ревизию.
- JS promise-очередь выбора перенесена как независимый async mutex на каждый session id:
  выборы одного агента сохраняют порядок, а медленная активация одного preset не блокирует
  других агентов. Panic внутри выбора преобразуется в ошибку и не отравляет следующую
  операцию; panic синхронного tools observer также изолируется, durable event записывается.
- `Map`/object insertion order хранится `IndexMap` и `serde_json` workspace с
  `preserve_order`. Сортировка preset ids и leak names использует CLDR root через `feruca`;
  это ближайший доступный аналог `localeCompare`.
- Typert decorators не нужны transport-слою Rust: методы `remote_export_list` и
  `read_document` сохраняют payload и отмечены rustdoc `Remote`.
- Source `AsyncDisposable` представлен явным async `dispose` и безопасным `Drop`.
  `Config.selectedDefault` остаётся `Volatile<Option<String>>`; неизвестный старый
  `modeSelectionEnabled` игнорируется serde, как неописанное поле Loader schema.
- Runtime inventory возвращает `description`, как фактическая TS-реализация, хотя старый
  interface в `composition-inventory.ts` его не перечислял. У Loader `EntryInfo` пока нет
  declaring `baseURL`, поэтому module rows имеют `base_url: None`, `use_host_base: true`.

## Fixed-interface shims

- `PresetRuntime` передаёт `Arc<PluginRegistry>` и `EvalEnv` явно. Публичный `Loader` service
  не раскрывает registry/env уже работающего дерева, поэтому plugin-конструктор временно
  использует fallback registry (с Loader built-ins) и ждёт общий read-only child-mount API.
  Программный `new`/`new_with_services` получает полный runtime и им покрыты mount tests.
- `shim::stringify_entry_yaml` сохраняет entry-list order, вложенность и `!!js` выражения,
  повторяет выбор scalar style закреплённого `js-yaml@4.3.2` с `JSON_SCHEMA`: literal block
  (включая indent/chomp indicators), double-quoted escapes для непечатных символов и
  JSON-schema ambiguity quoting. Он нужен, пока `hoh-loader` не экспортирует свой YAML writer.
- Kernel snapshot раскрывает имя/realm/owner provision, но не typed value и не контекст
  Loader row. Поэтому `service_for<K>` точно фильтрует ownership для видимого из agent
  контекста сервиса, но не может извлечь значение из private realm, созданного внутри group.
- В исходнике plugin вызывает Settings `configure({ auto: false }, fiber)`. `hoh-settings`
  сейчас остановлен на собственном Loader API gap, поэтому этот вызов оставлен именованным
  швом, без локальной подмены сервиса.

## Не перенесено

Пропущенных source-файлов и тестовых объявлений нет. Source integration с настоящими
`tool-executor-ptc` и `tool-testkit` представлен проверкой тех же двух контрактов registry:
executor живёт с ревизией, pending dependency запрещает mount и становится доступной после
host provision. Самих Rust crates `hoh-ptc-runtime`, `hoh-tool-executor-ptc` и
`hoh-tool-testkit` в workspace ещё нет, поэтому выполнение PTC-программы не дублируется здесь.

По PORTING §8 не переносятся Vitest spies/dynamic import/Typert metadata и проверка Settings
UI helper. Вместо spy на runtime tree проверяются отдельные Loader mounts, относительные ids,
fiber identity и inventory. Реальное live-config удаление `selectedDefault` заменено чистой
проверкой приоритета `Volatile`; live update покрывается владельцем kernel config.

## Не проверено

Произвольный YAML за пределами Loader `Entry` shape не поддерживается: emitter проверен на
scalar/object/list, multiline literal blocks, `JSON_SCHEMA` typed-string quoting,
непечатные escapes, plain-safe hash и `!!js`, которые публикует этот пакет. Не проверены
dynamic private-realm service values и production plugin wiring через активный Loader до
появления API выше. Внешнего I/O, сети и платформенных системных вызовов в крейте нет.

## Исправления по ревью 114702

- `remote_export_list` теперь не теряет `order`; JSON roster покрыт regression-тестом.
- `inspect_compositions` стал async и каждый раз вычисляет root-realm leaks по живому kernel
  snapshot, включая сервис, опубликованный после разрешения pending dependency.
- Имена leaked services сначала сортируются CLDR-root collator, затем дедуплицируются;
  несколько имён диагностируются в стабильном порядке.
- Ошибка `create_scope` отклоняет `register` и удаляет вставленную definition. Ошибка mount
  по-прежнему сохраняется как broken preset — это соответствует разному поведению исходника
  для отказа создать scope и отказа активировать его содержимое.
- Вложенное отличие `use_host_base` не менялось: `EntryInfo` Loader по-прежнему не раскрывает
  declaring base URL. Debug-вывод секретов не добавлялся; registry не логирует plugin config.

## Исправления по ревью 120752

- Literal block теперь получает indent indicator `2`, когда первая непустая строка начинается
  с пробела (`|2-`/`|2`/`|2+`), поэтому следующая менее отступившая строка остаётся внутри
  scalar. Отдельный regression проверяет точный документ для `"  a\nb"`.
- Tab, CR, NEL, NBSP, Unicode line/paragraph separator, BOM и остальные непечатные code point
  переводят scalar в double-quoted style с теми же escape-последовательностями, что
  `js-yaml`; multiline с tab тоже печатается одной escaped строкой.
- Эталон проверен по фактически закреплённому исходнику, а не по предположению ревью:
  `packages/preset/agent-preset-registry/src/index.ts:241` вызывает vendor
  `stringifyEntryYaml(..., { lineWidth: -1 })`; `vendor/include/src/index.ts:24,51`
  передаёт `js-yaml` его `JSON_SCHEMA`, а `pnpm-lock.yaml:216` закрепляет `js-yaml@4.3.2`.
  Эталонные строки дополнительно сверены локальным запуском именно
  `refs/js-yaml@4.3.2`; его `lib/dumper.js:188,242,298,315` задаёт printable-набор,
  запрет начального `-`, indent indicator и выбор double/literal style.
  Поэтому даты, `1_000` и `<<` остаются plain: JSON schema содержит только null/bool/int/float,
  integer resolver не принимает `_`, а merge/timestamp resolvers в схему не входят.
  Напротив, `.NaN`, варианты `.Inf`, `+.inf` и числа квотируются; `-foo` квотируется потому,
  что `dumper.js:isPlainSafeFirst` безусловно запрещает начальный `-`. Эти спорные случаи
  закреплены одним golden regression-тестом. Ветка leading-dot float в
  `lib/type/float.js:8-15` не разрешает знак, поэтому `+.2` и `+.2e4` остаются plain; это
  отдельно добавлено после независимой проверки исправлений.

## Исправления по ревью 125637

- Значение `__jsExpr` теперь проходит через тот же scalar writer, что обычная строка, прежде чем
  получить префикс `!!js`. Поэтому начальные `!`, тернарный `: `, строковые/boolean-подобные
  выражения корректно single-quote-ятся, а printable multiline получает literal block
  `!!js |-`. Golden regression содержит все пять контрпримеров ревью и падает на прежнем raw
  `format!("!!js {expression}")`.
- `isJsExpr` приведён к `vendor/loader/src/config/utils.ts:25-27`: наличие строкового
  `__jsExpr` достаточно, дополнительные свойства объекта не отменяют expression. Golden test
  закрепляет объект с дополнительным ключом; Loader такие объекты обычно не создаёт, но emitter
  теперь повторяет публичный predicate образца.
- Числовой writer сохраняет отрицательный ноль как `-0.0` и для нецелой научной записи без точки
  вставляет точку перед `e`, как `refs/js-yaml/lib/type/float.js:54,75-84`. Regression проверяет
  `-0.0`, `1e-7`, `5e-324`, неизменное `1.5e-7` и целое `1e21`; оба исправленных случая падали
  на прежнем коде.

## Вопросы ведущему

Нужны три API владельцев: (1) read-only `Loader` registry/env и child-tree mount,
(2) публичный Loader entry-list stringify, (3) kernel/Loader typed lookup provision по
mount owner и isolated realm. После них удалить `PresetRuntime` fallback/YAML shim и сделать
`service_for` полным. Отдельно после разблокировки `hoh-settings` подключить
`configure({ auto: false })`. До этого production plugin с произвольными Loader modules и
private service lookup нельзя считать полностью связанным, хотя standalone registry contract
и lifecycle реализованы.

## Проверки

Все Cargo-команды: `CARGO_BUILD_JOBS=2`, `CARGO_NET_OFFLINE=true`,
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x6`; tests запускались под
`timeout 900s` и с `RUST_TEST_THREADS=2`.

- `cargo test --offline -p hoh-agent-preset-registry` — 56 passed, 0 failed;
  doc target 0/0. Представлены все source declarations и review regressions.
- `cargo clippy --offline -p hoh-agent-preset-registry --all-targets --no-deps -- -D warnings` — exit 0.
- `cargo fmt -p hoh-agent-preset-registry -- --check` — exit 0.
- `diff -ru <source>/locale crates/hoh-agent-preset-registry/locale` — exit 0.

Первый полный тестовый прогон выявил технический serde-текст для malformed child row;
mount теперь запускает исходный `entry_list_problem` до Loader decode. После исправления
повторён весь пакет, не только упавший тест; итог выше относится к актуальному состоянию.
