# hoh-session-format-v3-to-v4 ← packages/session/session-format-v3-to-v4 @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs | Реэкспорты codec, migration, header/relationships/restorer, child evidence и frozen vocabulary |
| src/codec.ts | src/codec.rs | V2 physical framing, native V4 admission до recoverable suppression, отдельная проверка ignorable developer при записи |
| src/migration.ts | src/migration.rs | Независимые streaming stages, explicit child evidence, interrupted turn insertion, source/target cuts и delivery guards |
| src/{facts,tool-role,content,references}.ts | одноимённые snake_case modules | Child facts, wrapper lift с раздельной metadata владельцев, namespace extensions и локальные координаты |
| src/{sources,message-sources,extension-identities}.ts | одноимённые snake_case modules | Declared message traversal, producer identity и фиксированный V3 inventory |
| src/{developer,system-message,fork-result,retired-syntax}.ts | одноимённые snake_case modules | Нативные поля и hard refusals |
| src/{validation,relationships}.ts | одноимённые modules | Нативные V4 header/cut/vocabulary и generation-owned lifecycle relationships |
| src/testing/validation.ts | src/testing/validation.rs, mod.rs | Physical encode/decode validation helper |
| tests/attribution.spec.ts | tests/attribution.rs | 1 группа; обе роли, Session derivation также в catalog integration |
| tests/canonical.spec.ts | tests/canonical.rs, ../hoh-session-format-catalog/tests/v3_to_v4_integration.rs | 4 группы здесь, 1 noncanonical-metadata группа через полный catalog |
| tests/catalog-completion.spec.ts | tests/catalog_completion.rs | Все 16 групп |
| tests/codec.spec.ts | tests/codec.rs | Все 19 групп, строки таблиц выполняются циклами |
| tests/content.spec.ts | tests/content.rs | Все 18 групп; surrogate-строка ниже |
| tests/corpus.spec.ts | tests/corpus.rs, tests/common/corpus_contract.rs, tests/fixtures/ | 1 группа, 46 неизменённых JSONL fixtures; полный installed catalog также в tests/recorded_corpus.rs соседнего crate |
| tests/developer-relationships.spec.ts | tests/developer_relationships.rs | Все 6 групп; Session surface/derivation дополнительно в catalog |
| tests/developer.spec.ts | tests/developer.rs | Все 29 групп |
| tests/fork.spec.ts | tests/fork.rs | 5 групп; pruner-адаптация ниже, настоящий fork builder дополнительно в catalog |
| tests/interrupted-turn.spec.ts | tests/interrupted_turn.rs | Все 9 групп; installed image projection и derived messages дополнительно в catalog |
| tests/message-sources.spec.ts | tests/message_sources.rs | Все 5 групп |
| tests/migration.spec.ts | tests/migration.rs, ../hoh-session-format-catalog/tests/v3_to_v4_integration.rs | 12 групп здесь, 1 seeded v0…v3 chain группа в catalog |
| tests/open-vocabulary.spec.ts | tests/open_vocabulary.rs | Все 5 групп |
| tests/relationships.spec.ts | tests/relationships.rs | Все 14 групп, включая все lifecycle/compaction/PTC/retry таблицы |
| tests/retired-content.spec.ts | tests/retired_content.rs | Все 4 группы; 15 content carriers |
| tests/retired-syntax.spec.ts | tests/retired_syntax.rs | Обе группы |
| tests/sources.spec.ts | tests/sources.rs | Все 14 групп; две surrogate-строки ниже |
| tests/system-message.spec.ts | tests/system_message.rs | Обе группы, полная таблица malformed fields |
| tests/tool-role.spec.ts | tests/tool_role.rs | Все 32 группы; параметры остаются циклами |
| tests/validation.spec.ts | tests/validation.rs | Все 9 групп; невыразимый null-event подслучай ниже |
| Дополнительные проверки Rust | tests/native_regressions.rs | UTF-16 child order/numeric dates, JS index-key order, deep-strict negative zero, identity исторического name extension и порядок отказов reference containers/sources/endpoints |

Все src и spec-файлы, README.md и package.json прочитаны. Locale и отдельного LICENSE нет.

В исходных 20 spec — 209 групп it/it.each (строки таблиц не умножают число).
207 представлены здесь, 2 требуют полной цепочки/installed Session и исполняются
в назначенном соседнем catalog crate, чьи разрешённые зависимости уже это покрывают.
Из 209 групп одна использует protocol fixture вместо ещё отсутствующего pruner;
остальные исходные проверки JSON форм перенесены. Здесь дополнительно 5 регрессий.
Выполнены все 212 Rust test-функций, без отказов; результаты ниже.

## Отличия от образца

- Используется существующий синхронный API hoh-session-format: owned JSON events,
  Box<dyn Stage/Decoder/Run>, borrowed headers, Result<_,SessionFormatError>.
  Никакой async/IO работы библиотека не добавляет. Run раскрывается итератором.
- Экспортируемые объекты — RELEASED_V3_SESSION_FORMAT_CODEC (реэкспорт),
  RELEASED_V4_SESSION_FORMAT_CODEC и SESSION_FORMAT_V3_TO_V4; новые unit-типы
  ReleasedV4SessionFormatCodec и SessionFormatV3ToV4 реализуют базовые traits.
  create_session_format_v3_to_v4 принимает owned Vec<Value> и сохраняет immutable Arc.
  Object.freeze/JS identity заменены владением; входные значения тестов не изменяются.
- RELEASED_V3_EVENT_TYPES — immutable slice в исходном порядке. Внешний known
  vocabulary — HashSet<String>, только membership. Видимый порядок pending tools,
  catalog entries и side effects сохраняется IndexMap/Vec. Сортировка childId
  сравнивает UTF-16 units, childCreatedAt — проверенные числа; сортировка стабильная.
- JSON-текст использует общие js_json_*; числовые и произвольные diagnostics —
  js_string. Вход должен иметь JS property order, гарантируемый js_json_parse;
  JS_JSON_MAX_DEPTH/64 MiB application stack — общий контракт PORTING §4.
  Дополнительный assert_released_v4_header_value проверяет raw Value до Map narrowing.
  Optional arguments заданы явно (включая known types для row admission).
- JS strict equality в metadata не превращается в deep equality; PTC arguments
  и exact spans используют Node deep-strict semantics, включая знак нуля.

## Не перенесено

Одиночный UTF-16 surrogate в трёх строках it.each не является
Rust String/serde_json::Value; scalar string variants этих групп перенесены.
Не-object event внутри Vec<SessionFormatEvent=Map> не представим; raw codec row
по-прежнему принимает Value и проверяется отдельно. JS prototype identity
заменена сохранением собственных __proto__/constructor JSON members.

- В round_trips_a_pruned_fork_result исходные TokenMeter/ToolResultPruner не
  запускаются: их crates ещё отсутствуют. Проверяется сохранённая форма протокола
  compaction/prune + replacement, те же id, sourceEventSeqs, marker text, native
  encode/decode/restore и обе порчи replacement identity. Fixture использует
  допустимую цену 0; оценка tokens и собственный алгоритм pruner относятся к его
  владельцу и здесь не объявляются проверенными. Runtime/plugin код не копировался.
- Writer-order assertion tool-role сравнивается с точным golden JSON текущего
  writer, без вызова чужого Message-конструктора. Все message/result-owned fields,
  их порядок и отсутствие isError при отсутствии wrapper flag проверяются.
- Corpus fixture placeholder cwd заменяется на стабильный абсолютный /snapshot-cwd
  вместо cwd процесса теста; исходные 46 файлов остаются побайтово неизменными.
  Наблюдаемые parent/child clocks, 25 связей, единственный descriptor-less child
  и весь список 24 конфликтов сохранены. Проверка hashes 46 файлов прошла до и
  после восстановления сессии после переполнения диска.

## Не проверено

Runtime suite, отдельный Clippy этого crate и финальный fmt --check чисты.
Полная application composition/minimal profile не запускается этой библиотекой.
Windows/macOS не запускались; общий path helper зависимостей сейчас POSIX.

## Вопросы ведущему

- SHIM(hoh-util-workspace-path): прямой path dependency отсутствует в skeleton и
  разрешённом списке shared crates брифа. До ответа на запрос он не добавлен.
  absolute_cwd использует публичный V3 header validator с заведомо корректным
  минимальным header и единственным проверяемым cwd; validator уже вызывает
  общий node_path::is_absolute. Алгоритм path не копируется. При разрешении прямой
  зависимости этот adapter заменяется прямым вызовом общего helper.
- Добавление dev-dependencies не потребовалось: integration assertions с реальным
  Session и полным каталогом размещены в другом назначенном crate catalog.
  Никакие несуществующие crates не добавлены. Отсутствуют hoh-token-meter и
  hoh-compaction-tool-result-pruner; ограничение их выполнения описано выше.
- Экспорты проверены по внешним переносимым src: JSONL scanner использует
  assertV4RowAdmission/assertReleasedV4Relationships, prerequisite collector —
  historicalChildCatalogSource, catalog — V4 codec/migration/header/restorer и
  RELEASED_V3_EVENT_TYPES. У реэкспорта releasedV3SessionFormatCodec через этот
  root и у публичных subpath helpers внешних переносимых src-потребителей не
  найдено; они сохранены. Остальные три предыдущие migration/format crates не
  редактировались; их алгоритмы используются через публичные traits.

## Проверки

Все Cargo-команды: CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2,
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w3. Остановленные регулятором
сборки вручную не возобновляются. При написании только cargo check --tests.
Первый `cargo check -p hoh-session-format-v3-to-v4 --tests`: exit 0,
Finished dev profile, 10m 43s (до завершения всех тестовых файлов).
Следующий общий `cargo check -p hoh-session-format-v3-to-v4 -p hoh-session-format-catalog --tests`
прошёл, exit 0, 26m 58s; выявил только unused imports shared test helper, затем
удалённые. Поздний check указал на неоднозначный пустой Vec в одном assertion;
проверка заменена равносильным is_empty. Его сеанс был утрачен при переполнении
диска; успех этого запуска не заявляется. После освобождения места запущен новый
общий check, а файлы проверены на целостность: 90 nonempty UTF-8 файлов, без NUL,
с final newline; все module paths существуют, cargo fmt разбирает все Rust файлы.
После восстановления общий check завершился exit 0, без предупреждений,
Finished dev profile, 10m 39s. Финальная source-сверка сохранила точный порядок
reference failures (surface container → sourceEventSeqs → range endpoints).
Runtime suite запущена один раз после завершения кода; exit 0,
Finished test profile, 41m 26s. Все 212 тестов этого crate прошли; 0 failed,
0 ignored. Числа passed по integration binaries в порядке Cargo:
attribution 1, canonical 4, catalog_completion 16, codec 19, content 18,
corpus 1, developer 29, developer_relationships 6, fork 5, interrupted_turn 9,
message_sources 5, migration 12, native_regressions 5, open_vocabulary 5,
relationships 14, retired_content 4, retired_syntax 2, sources 14,
system_message 2, tool_role 32, validation 9. В каждом —
`test result: ok. N passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.
Unit/doc suites: 0 passed, 0 failed. Общий результат двух crates: 243 passed,
0 failed; это сумма отдельных строк Cargo, а не отдельная строка его вывода.
`cargo fmt -p hoh-session-format-v3-to-v4 -p hoh-session-format-catalog -- --check`:
exit 0, без вывода. Точная runtime-команда в указанном окружении:
`RUST_TEST_THREADS=2 cargo test -p hoh-session-format-v3-to-v4 -p hoh-session-format-catalog`.

Первый общий `cargo clippy -p hoh-session-format-v3-to-v4 -p hoh-session-format-catalog --all-targets -- -D warnings`
завершился exit 101 в чужом hoh-compaction-image-offload/src/lib.rs:33
(clippy::new_ret_no_self); чужой код не изменялся. Повтор общего запуска
потерян при остановке фоновой задачи по лимиту времени; его успех не заявляется.
После указания ведущего сдавать по одному выполнены отдельные проверки:
`cargo clippy -p hoh-session-format-v3-to-v4 --all-targets -- -D warnings`:
exit 0, без диагностик, `Finished dev profile [unoptimized + debuginfo] target(s) in 24m 17s`.
`cargo fmt -p hoh-session-format-v3-to-v4 -- --check`: exit 0, без вывода.
Код crate после успешного runtime-прогона не менялся; тесты повторно не запускались.
Незавершённой реализации в этом crate нет. Неисполненные проверки чужого pruner,
ограничения представления JS values и необязательное упрощение cwd adapter
перечислены выше. Catalog сдаётся отдельно по следующему указанию ведущего.

## Решение ведущего по ревью rv-v3v4 (19.11)

Одиночные UTF-16 суррогаты (`\ud800` в JSON-экранировании) после разбора становятся U+FFFD — это общее решение
`hoh-util-values` (`src/js_json.rs:425`, тест `tests/js_json.rs:157`), а не этого крейта. Поэтому строки
`sources.spec.ts:84` с одиночными суррогатами исключены, а имя источника вида `plugin:\ud800` после миграции
становится `plugin:�`. Наш движок пишет строки Rust и одиночный суррогат породить не может; расхождение возможно
только при миграции журнала dsh с заведомо битым именем источника. Записано как известное отличие уровня
`hoh-util-values`; пересмотр — вместе с этим крейтом, если появится реальный журнал с таким именем.
