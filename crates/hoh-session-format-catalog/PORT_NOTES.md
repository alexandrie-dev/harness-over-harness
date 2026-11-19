# hoh-session-format-catalog ← packages/session/session-format-catalog @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs | Public catalog/children/historical exports и общий SessionFormatError |
| src/generated.ts | src/generated.rs | Фиксированный порядок codecs 0…4 и edges 0→1→2→3→4, current/transformed/header validation |
| src/children.ts | src/children.rs | Подстановка только V3→V4 edge, независимые стадии |
| src/historical.ts | src/historical.rs | Frozen V3 vocabulary и цепочка 0…3 без child completion |
| src/current.ts | src/current.rs | Installed Session metadata/adoption/surface validation |
| src/message-projections.ts | src/message_projections.rs | Массив detached replay interpreters |
| tests/catalog.spec.ts | tests/catalog.rs | Все 14 групп |
| tests/children.spec.ts | tests/children.rs | Обе группы |
| tests/current.spec.ts | tests/current.rs | Обе группы |
| tests/developer-history.spec.ts | tests/developer_history.rs | 2 runtime группы, 1 TypeScript-only группа исключена |
| tests/preset-migration.spec.ts | tests/preset_migration.rs | Обе группы, версии в циклах |
| Полная интеграция соседнего v3→v4 пакета | tests/v3_to_v4_integration.rs | 2 исходные группы edge и ещё 6 проверок installed Session/fork/projection/derivation |
| Recorded parent/child corpus | tests/recorded_corpus.rs | Та же общая corpus contract, через production historical/current catalog и Session validation |

Все шесть src, пять spec, README.md и package.json прочитаны. Locale и отдельного LICENSE нет.
В исходнике 23 группы it/it.each: 22 runtime перенесены, единственная type-only
группа исключена. Кроме них здесь 2 source группы edge, требующие полного
catalog, и 7 дополнительных cross-package integration checks. Выполнена 31 Rust
test-функция, без отказов; результаты ниже.

## Отличия от образца

- Static module exports — функции session_format_catalog(),
  historical_session_format_catalog(), session_format_catalog_options(),
  current_session_message_projections(): фиксированный inventory принадлежит
  build, независим от mounted plugins, но не хранит глобальный mutable registry.
  Конструктор возвращает Result вместо исключения при module initialization.
- Current adapters используют текущий Session::from_restore с Arc<SessionEvent>,
  branded SessionId и checked SessionLogOffset; не копируют Session validators.
  Ошибки installed Session проходят через SessionFormatError::Other с исходной
  boxed error внутри варианта Other. Version skew — собственный plain InstalledCurrentSessionError
  с name Error, не HarnessError; дополнительный публичный Rust тип задокументирован.
- Прямой export SessionFormatUnsupportedMigrationError — alias общего
  SessionFormatError; класс представлен его вариантом UnsupportedMigration,
  как в принятом базовом API. Создание subclass в Rust — через
  SessionFormatError::unsupported_migration, проверка — is_unsupported_migration;
  базовый new создаёт Format, а не subclass.
- current_session_message_projections возвращает MessageProjections зависимости:
  owned Arc/Mutex vector вместо readonly JS array; новых listeners нет.
  Stateless definition берётся из hoh_compaction_image_offload::image_offload_projection(),
  чей общий immutable Arc соответствует source module const и проверке identity
  в Session. Здесь нет своего registry, Session/Context, caches или lifecycle
  state. Повторная identity проверяется в catalog test.

## Не перенесено

stores_only_the_activated_name_in_each_addition в developer history содержит
только expectTypeOf и не переносится по PORTING §10. Runtime история definitions,
removal/re-add, replacements, producer mutation, restart и fork перенесена.

## Не проверено

Runtime suite с публичной проекцией зависимости и финальный fmt --check чисты.
Ранее Clippy блокировался чужим hoh-compaction-image-offload; владелец исправил
lint, общий Clippy Session/catalog после исправления версии прошёл. Итоговые
числа и повторная проверка текущего дерева записаны в разделе rv-catalog ниже.
Полная application composition и платформы Windows/macOS здесь не запускаются.

## Вопросы ведущему

- SHIM(hoh-compaction-image-offload) удалён: во время финальной проверки у
  зависимости появился public image_offload_projection(). Теперь используется
  этот export; чужие crates не менялись. Повторные check и runtime suite чисты.
- Внешний блокер clippy::new_ret_no_self в hoh-compaction-image-offload закрыт
  владельцем: конструктор ImageOffloadError переименован в boxed. В рамках
  catalog чужой код не менялся. Недостающих public API и оставшихся shim нет.
- Внешние переносимые потребители: session-persistence-jsonl, llm-replay и
  session-query. sessionFormatCatalog, historicalSessionFormatCatalog,
  createSessionFormatCatalogWithChildren и currentSessionMessageProjections
  используются; alias subclass error импортируют JSONL и llm-replay (в Rust
  класс проверяется вариантом общего enum). У публичных generated/current
  helpers не найден прямой внешний named-import потребитель; они сохранены.

## Проверки

Окружение: CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2,
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w3. При написании использован
cargo check --tests. Остановленные регулятором сборки вручную не возобновляются.
Общий check ранее прошёл с exit 0 (26m 58s), до завершения последних test files.
Поздний запуск оборвался вместе с сервером при переполнении диска; его успех не
заявляется. После восстановления: 17 nonempty UTF-8 файлов, без NUL, с final
newline; явные module paths целы, cargo fmt прошёл.
Новый общий check завершился exit 0, без warnings, 10m 39s. Финальный runtime
прогон двух crates запущен один раз после завершения кода; exit 0,
Finished test profile, 41m 26s. Все 31 тест этого crate прошли; 0 failed,
0 ignored. Числа passed по integration binaries: catalog 14, children 2,
current 2, developer_history 2, preset_migration 2, recorded_corpus 1,
v3_to_v4_integration 8. В каждом —
`test result: ok. N passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.
Unit/doc suites: 0 passed, 0 failed. Общий результат двух crates: 243 passed,
0 failed; это сумма отдельных строк Cargo, а не отдельная строка его вывода.
`cargo fmt -p hoh-session-format-v3-to-v4 -p hoh-session-format-catalog -- --check`:
exit 0, без вывода. Runtime-команда:
`RUST_TEST_THREADS=2 cargo test -p hoh-session-format-v3-to-v4 -p hoh-session-format-catalog`.

Первый общий `cargo clippy -p hoh-session-format-v3-to-v4 -p hoh-session-format-catalog --all-targets -- -D warnings`
завершился exit 101 в чужом hoh-compaction-image-offload/src/lib.rs:33
(clippy::new_ret_no_self); чужой код не изменялся. За время финальной проверки
там появился public image_offload_projection(), поэтому свой shim удалён и
подключён этот export. После замены `cargo check -p hoh-session-format-catalog --tests`:
exit 0, без warnings, Finished dev profile, 9m 21s. Общий Clippy повторён после
ожидания; результат этого фонового запуска утрачен при остановке по лимиту
времени, его успех не заявляется. После отдельной сдачи v3→v4 работа возобновлена
только в catalog. Отдельный `cargo fmt -p hoh-session-format-catalog -- --check`:
exit 0, без вывода. Отдельный Clippy снова завершился exit 101 на том же
clippy::new_ret_no_self в чужом image-offload/src/lib.rs:33. Чужие файлы не менялись.

После удаления shim выполнен один финальный runtime-прогон:
`RUST_TEST_THREADS=2 cargo test -p hoh-session-format-catalog` в указанном окружении.
Exit 0, `Finished test profile [unoptimized + debuginfo] target(s) in 11m 40s`.
Строки результатов: catalog — `test result: ok. 14 passed; 0 failed`, children,
current, developer_history, preset_migration — по `test result: ok. 2 passed; 0 failed`,
recorded_corpus — `test result: ok. 1 passed; 0 failed`, v3_to_v4_integration —
`test result: ok. 8 passed; 0 failed`. Во всех 0 ignored, 0 measured, 0 filtered out.
Unit/doc suites: 0 passed, 0 failed. Итого catalog: 31 passed, 0 failed.
После этого ожидания отдельный Clippy повторён и снова завершился exit 101 на
том же чужом lint. После дополнительной паузы 55 секунд выполнен третий отдельный
запуск той же команды; он также завершился exit 101 на image-offload/src/lib.rs:33.
Точная команда всех трёх отдельных запусков в указанном окружении:
`cargo clippy -p hoh-session-format-catalog --all-targets -- -D warnings`.
Никаких обходов lint, ручного возобновления пауз или изменений чужого кода нет.
Исходники catalog после успешных 31/0 и fmt --check не менялись, обновлены только
эти заметки. После передачи v3→v4 на ревью его файлы не редактировались.
На том этапе реализация catalog была завершена, todo!/unimplemented!/заглушек не
было; незавершённым условием сдачи оставался Clippy. Его блокер закрыт владельцем
зависимости при следующем исправлении rv-catalog ниже.

## Исправление rv-catalog: числовая версия 4.0

По отдельному разрешению ведущего изменено только сравнение version в
hoh-session/src/validation.rs: вместо Value/u64 сравнивается numeric value,
как `record.version === SESSION_FORMAT_VERSION` в core/session/src/index.ts:106.
Session принимает отдельно сконструированный Value::from(4.0); 4.5 и строка "4"
по-прежнему отвергаются с прежними сообщениями. В Session добавлен regression
`validates_header_version_as_a_javascript_number`, а в tests/current.rs —
`accepts_a_current_header_whose_numeric_version_is_an_integral_float` с прямыми
вызовами обоих публичных current-валидаторов и пустым artifact.
Production-код catalog не менялся, v3→v4 мной не редактировался. Старые числа выше
относятся к прежнему набору. Проверки отдельного исправления завершены:

Окружение всех команд: CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2,
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w3.

- `cargo check -p hoh-session -p hoh-session-format-catalog --tests`: exit 0,
  без warnings, Finished dev profile, 5.93s.
- `RUST_TEST_THREADS=2 cargo test -p hoh-session -p hoh-session-format-catalog`:
  exit 0, Finished test profile, 64m 22s. Суммы строк результатов: Session
  304 passed, 0 failed, 1 ignored; catalog 32 passed, 0 failed, 0 ignored.
  Итого 336 passed, 0 failed. Session review_fixes: 8 passed, 0 failed, 1 ignored;
  catalog current: `test result: ok. 3 passed; 0 failed; 0 ignored`.
  Во всех 0 measured, 0 filtered out. Doc-tests обоих: 0 passed, 0 failed.
- `cargo clippy -p hoh-session -p hoh-session-format-catalog --all-targets -- -D warnings`:
  exit 0, без диагностик. После исправления lint err_expect в новом тесте первый
  чистый запуск — 0.75s; повтор по текущему дереву — Finished dev profile, 29m 32s.
- `cargo fmt -p hoh-session -p hoh-session-format-catalog -- --check`: exit 0,
  без вывода, включая повтор после появления параллельной правки depth-теста.

Во время ожидания тестов другой исполнитель активировал depth-4096 regression
Session на application stack; эта правка не относится к моему fix catalog и не
менялась мной. Поэтому текущий полный набор имеет только один ignored — прежний
raw_creation_veto_stops_the_next_raw_listener, ожидающий API kernel. Все прочие
тесты, включая обе новые регрессии числовой версии, прошли. Недостающих API,
shim и незавершённых условий именно этого исправления catalog нет.
