# hoh-loader-smoke ← packages/test-support/loader-smoke @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | `ExampleMode`, разбор `DSH_EXAMPLE_MODE`, выбор src/lib-артефакта, очистка proxy env, изоляция cwd/DSH homes, EOF stdin, deadline/SIGKILL, stdout/stderr, expected exit и cleanup |
| `src/agent-turn.ts` | `src/agent_turn.rs` | выбор единственного root Agent, ожидание публикации, граница durable receipt, canonical events, последний assistant text, usage по step и flush |
| `tests/example-launch.spec.ts` | `tests/example_launch.rs` | все 13 случаев; environment-default проверяется в отдельном процессе без изменения глобального env тестового процесса |
| `tests/loader-smoke.spec.ts` | `tests/loader_smoke.rs`, `src/bin/loader-smoke-fixture.rs` | все 6 случаев; переносимый native fixture заменяет TS/tsx fixtures success/fail/hang |
| `tests/agent-turn.spec.ts` | `tests/agent_turn.rs` | все 6 случаев на настоящих `Agents`, `Sessions` и `session/event`; две строки `it.each` считаются двумя случаями |
| `tests/fixtures/{success,fail,hang}.ts` | `src/bin/loader-smoke-fixture.rs` | те же наблюдаемые EOF, argv/env/cwd, потоки, exit 7 и зависание |
| `tests/fixtures/{cli-mock-llm,headless-driver,production-profile}.ts` | — | repository-only composition fixtures, не exports пакета и не используются его `*.spec.ts`; их владельцы — внешние e2e потребители |
| `README*.md`, `README.i18n.yaml`, `package.json`, `tsconfig.json` | rustdoc и эти заметки | прочитаны; npm/build/i18n metadata не копируется |

В образце нет `locale/`, собственного `LICENSE` и отдельно лицензированных фрагментов. Новых внешних
зависимостей нет: `tokio`, `tempfile`, `indexmap`, `serde` и `serde_json` уже присутствуют в workspace и lock;
вместо npm-only `execa`/`tsx` используется существующий native стек.

## Отличия от образца

- Rust запускает выбранный executable напрямую. `src` выбирает `src_bin`, требует `tsconfig_path` и сохраняет
  `TSX_TSCONFIG_PATH`; `lib` выбирает явный `lib_bin` либо выполняет точное преобразование последнего
  `/src/*.ts`/`\\src\\*.ts` в `lib/*.js`. Флаги Node `--import tsx` не создаются: Node/tsx — не артефакты
  Rust workspace, а реальный HoH bin уже скомпилирован. `SourceImport::TsxEsm` сохраняет явный выбор API,
  но для native spawn не меняет argv. Это единственное намеренное изменение launch wire.
- `ExampleLaunch.command` — выбранный bin, а не `process.execPath`; следствие предыдущего пункта. Остальные
  наблюдаемые контракты subprocess сохранены: overlay поверх унаследованного env, удаление proxy names,
  точный argv, немедленный EOF, оба потока на отказе, direct-child kill, expected exit и ownership cleanup.
- Объектный union `LoaderSmokeOptions` выражен одной Rust-структурой с runtime-проверкой взаимоисключающих
  `cwd`/`temp_dir_prefix`. `undefined` в child env выражен `ChildEnvironment` со значением `None`.
- Callback-типы названы публично `LoaderSmokeHook` и `FixtureEventObserver`, чтобы выразить TypeScript function
  fields без закрытых неименуемых Rust типов. Observer получает `&SessionId`, а не голую строку — правило
  branded identity. `SourceImport` и эти aliases — дополнительные публичные Rust symbols.
- Rust `Agent::when_idle` имеет безошибочный `Future<Output = ()>`, поэтому последний source-тест вызывает
  ошибку из синхронного `followup` после durable receipt и проверяет то же обязательство: listener снят на
  неуспешном turn, а flush не выполняется. Отказ `whenIdle` невозможно выразить принятым чужим trait API.
- После регистрации `agent/created` listener выполняется повторная проверка registry. В JavaScript между
  первой проверкой и `ctx.on` нет планирования; в многопоточном Rust эта проверка закрывает эквивалентное окно.
- Создание и закрытие `tempfile::TempDir` выполняются через `spawn_blocking`, поэтому синхронные filesystem
  операции не блокируют async worker. `TempDir::close` возвращает cleanup failure вызывающему и, как source
  `finally`, может заменить исходную ошибку. Unwind по-прежнему имеет резервный best-effort `Drop`.
- Ошибка native spawn сворачивается в source-совместимую process diagnostic с label, пустыми потоками и
  `exited undefined`; исходная OS cause намеренно не добавляется, поскольку `execa({ reject: false })` не
  включает её в проверяемый шаблон. Signal-only exit также печатается как `undefined`.
- `LoaderSmokeResult` не реализует serde: результат остаётся внутрипроцессным test-support envelope; JSON
  наружу пишет запускаемый fixture/приложение, как в образце.

## Не перенесено

- Npm artifact resolution (`import.meta.resolve('tsx')`) и запуск TypeScript source через tsx: они не имеют
  смысла для native HoH bin и потребовали бы отсутствующий runtime, а не Rust crate. Новая зависимость не добавлена.
- `tests/fixtures/production-profile.ts`, `headless-driver.ts`, `cli-mock-llm.ts` собирают полный shipped
  headless профиль для тестов других пакетов. Это repository fixtures, не исходники/exports данного пакета и
  не случаи его трёх spec-файлов. Native потребители передают свой скомпилированный driver в тот же harness.

Все **25 из 25** исходных test cases перенесены. Дополнительно проверены proxy override после очистки,
самостоятельное преобразование последнего `src` segment, spawn/signal diagnostics, нулевой deadline,
неблокирующий cleanup и malformed последний usage; всего на Unix в crate 32 теста.

## Не проверено

- Windows-процесс: чистое преобразование пути покрыто, но spawn/kill и удаление временного каталога запускались
  только на Linux. На Windows `Child::kill` завершает direct child средствами tokio, не Unix SIGKILL.
- Полный внешний headless profile и npm `src` execution, исключённые выше.

## Круг ревью 131639

- В timeout и exit diagnostics восстановлен обязательный разделитель `\n` между stdout и `stderr:`.
  Регрессии проверяют пустой stdout nonzero-exit и непустой stdout timeout; прежние тесты принимали
  ошибочный склеенный текст.
- Spawn failure и signal-only exit теперь совпадают с `execa({ reject: false })`: сохраняют label и печатают
  `undefined`. Оба прежних Rust-пути (`spawn …` и `null`) покрыты отдельными падающими регрессиями.
- `process_timeout_ms = Some(0)` теперь отключает deadline, как execa `timeout: 0`; delayed fixture падал на
  прежнем немедленном kill.
- Tempdir create/close вынесены в blocking pool. Current-thread regression запускает runnable task прямо перед
  удалением дерева: прежний синхронный `Drop` возвращался, не дав задаче выполниться.
- Тест выбора root повторно публикует Agent после завершения ожидания; listener уже снят. Failure-turn тест
  отдельно считает `session/flush` и подтверждает ноль вызовов.
- В первом `assistant/attempt` возвращён packed `text-chunks` перед usage, а обе foreign записи снова имеют
  тип `assistant/message`, как в TS. Тем самым проверяются поиск последнего usage в смешанном stream и exact
  Session identity для canonical message events.
- Последний malformed usage больше не пропускается с fallback к раннему валидному: decoder возвращает ошибку,
  как `expandAssistantStream`. Unit regression падает на прежнем `find_map(... .ok())`.
- Два structural fail-fast stub (`Model`, `Inbox`) перенесены прямо в `agent_turn` integration test; зависимость
  от полного `hoh-agent-loop-testkit` удалена. Это сохраняет прежнее поведение stubs и изолирует smoke-крейт от
  несвязанного production loop/tool graph.
- Observer остаётся под `TurnState` lock намеренно: canonical `Session::append` запрещает reentrant append той
  же Session до повторной event dispatch, а снятие lock внесло бы возможность переставить output/usage между
  concurrent callbacks. Для принимаемых session events наблюдаемой разницы с source нет.

## Круг ревью 142524

- `assistant/attempt` сначала полностью валидируется и разворачивается общим
  `hoh_llm::expand_assistant_stream_value`, как исходный `expandAssistantStream`; malformed packed-запись
  больше не скрывается последующим валидным usage chunk.
- Одноразовое ожидание `agent/created` вынесено во внутренний serial-event helper. Listener снимается после
  доставки и после post-registration recheck; отдельная регрессия повторной эмиссией доказывает cleanup.

## Вопросы ведущему

1. Если будущим Rust e2e нужен отдельный выбор debug/release артефакта, следует расширить `ExampleMode` общим
   решением для native bins; сейчас имена `src`/`lib` сохранены для совместимости с источником.
2. Если cleanup должен быть cancellation-safe при abort самого `run_loader_smoke` future, потребуется отдельный
   async owned-temp supervisor. Текущий путь полностью ждёт `TempDir::close` на обычном success/error возврате;
   при unwind остаётся best-effort `Drop`.

## Проверки

Команды выполняются offline с `CARGO_BUILD_JOBS=2`,
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x6`; tests — только под `timeout`.

- `cargo test --offline -p hoh-loader-smoke --no-fail-fast` — 34 passed, 0 failed; doc-tests 0/0.
- `cargo clippy --offline -p hoh-loader-smoke --all-targets --no-deps -- -D warnings` — чисто.
- `cargo fmt -p hoh-loader-smoke -- --check` — чисто.
