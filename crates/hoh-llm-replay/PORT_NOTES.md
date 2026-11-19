# hoh-llm-replay ← packages/test-support/llm-replay @ 39163dd3

В сообщении задания указан `packages/llm/llm-replay`; в закреплённом клоне такого каталога нет.
Использован настоящий пакет `packages/test-support/llm-replay`, указанный также в skeleton Cargo.toml.
README, единственный src/index.ts и оба исходных spec прочитаны. Собственного LICENSE и locale нет;
package.json объявляет MIT. Корень workspace, зависимости и исходный клон не менялись.

## Файлы

| образец | перенос | состояние |
|---|---|---|
| src/index.ts: ReplayEntry/Config/SessionScript, exports | src/lib.rs, src/config.rs | Все именованные runtime-экспорты; plugin `llm-replay`, inject `llm`; env defaults |
| src/index.ts: parseSessionFixture, parseSessionLog/Header, prepareSessionSnapshotFixtureForComparison | src/fixture.rs, src/shim.rs | projected/complete envelopes, исходные физические строки, cwd/tools projections, неизменность входа; зависимость от полного catalog явно отмечена ниже |
| src/index.ts: deriveReplayScript | src/script.rs | embedded Assistant streams через hoh-llm; отдельные retries; только явно помеченные local compaction calls; отсутствие finish требует override |
| src/index.ts: overrides/fromRequest/session tokens | src/script.rs | replace/patch/append, точные формы entries, последний regex match, первый capture, deep-copy, типизированные session ordinals |
| src/index.ts: ReplayAdapter | src/adapter.rs | catalog, context/maxTokens/efforts/default, prompt/tool update, badge, retry policy, image/offloaded pricing; provider I/O отсутствует |
| src/index.ts: installLlmReplay/replayEntry/apply | src/replay.rs, src/lib.rs | adapter или настоящий llm/stream waterfall, независимые cursors, consumption, dispose, prefix+throw/hang, readyFile, pacing/abort, DeepSeek post-2xx acceptance |
| tests/llm-replay.spec.ts | tests/llm_replay.rs, tests/support/mod.rs | source cases на текущем формате, настоящий LlmRuntime, исходные таблицы сведены в циклы; явные исключения ниже |
| tests/session-format-corpus.spec.ts, inventory.ts | tests/session_format_corpus.rs | existence inventory, семь правил unsupported policy, текущие committed fixtures без переписывания; полная историческая реставрация пока недоступна |

## Отличия от образца

- Один исходный index.ts разделён на приватные модули; все исходные экспорты доступны из корня crate.
  Дополнительные Rust-имена: LlmReplay/Deps (Plugin/Inject), ParsedSessionHeader (возвращаемый объект),
  Result/ReplayError (throw), Config::resolve/resolve_with_env (чистая проверка env без process-global мутаций).
  NAME/INJECT — Rust-имена исходных name/inject. Новых конфигурационных настроек нет.
- LlmReplay применяет настоящий LlmAdapter/LlmStream и LlmStreamEvent из hoh-llm; сервисы и события не
  копировались. Опциональный DeepSeekLlmApiExtensions запрашивается в момент принятого official-вызова.
  Его prepare получает `{messages:[]}`, sessionId/purpose/signal; accept завершается перед первым chunk.
  Для throw учитывается accepted либо наличие prefix; ошибки сохраняют LlmError/HarnessError code.
- ReplayHandle имеет идемпотентные методы dispose/assert_consumed вместо destructurable JS closures.
  Регистрация принадлежит ctx вызывающего. Consumption проверяется только явным вызовом, как в источнике.
  Изменяемые cursors находятся под коротким Mutex без await; резервирование происходит при вызове replay,
  разрешение placeholders и принятие extensions — при первом polling возвращённого потока.
- Config хранит providers/paceMs как JSON до ручных исходных диагностик, затем формирует typed ReplayConfig.
  Sidecar reader проверяет только известный type каждого chunk, без проверки его остальных полей.
  Дополнительный Rust-вариант ReplayEntry::Unvalidated(Value) сохраняет JSON entries, чьи chunks ещё
  нельзя представить как StreamChunk; обычные Chunks/Throw/Hang и их конструкторы сохранены.
  Envelope собирается в исходном порядке, сами chunks сохраняются без изменений. Substitutions работают
  и с этим вариантом. На типизированной границе LlmStream каждый такой chunk сужается только перед выдачей;
  некорректный payload там даёт serde error. Сам sidecar загружается успешно, как в TS. Передавать
  произвольный malformed chunk через Stream<Item=StreamChunk> без изменения hoh-llm невозможно;
  это ограничение воспроизведения malformed payload, не прежний отказ при загрузке sidecar.
- resolve_scripted_entry возвращает Cow: Borrowed сохраняет тождество исходного entry без placeholders,
  Owned — разрешённый deep-copy. Строковые листья requests обходятся в JS-порядке integer keys.
  JSON reader/writer и String/числа/trim — общие hoh-util-values; ошибки fs оформляет hoh-error.
- Синхронные публичные load/parse helpers сохранены; apply исполняет загрузку через spawn_blocking.
  readyFile записывается tokio::fs::write. Pacing принадлежит ctx.sleep, unload отменяет ожидание;
  это требование kernel/PORTING о владении работой. Тестовое время — paused Tokio, не wall-clock sleeps.
- Детерминированный tie-break детей использует CLDR root collation feruca 0.13.0, non-ignorable,
  без byte tie-break, как исходный localeCompare при en-US (разрешено DEPS §5.6). Сортировка stable;
  primary остаётся первым. Проверены c1/c2, a/B, é/z и точные replay texts после сортировки.
- prepare_session_snapshot_fixture_for_comparison вызывает настоящие encode_current_header/event
  из hoh-session-format-catalog: canonical key order и range encoding sourceEventSeqs.
  Сначала выполняется physical encoding, затем возвращаются cwd/tools placeholders, как в TS.
- Регулярные выражения fromRequest исполняет разрешённый workspace regex. Переведены JS non-u ASCII
  классы, word boundaries и четыре line terminators для dot; escaped backslashes не переписываются.
  Last-match/capture/brace-run и все паттерны выбранных sidecars поддерживаются. Полная ECMAScript RegExp
  (lookaround/backreferences, UTF-16 unit semantics, все identity escapes/anchors) не заявляется;
  SyntaxError detail содержит диагностику Rust regex при сохранённом исходном префиксе.

### Потребители экспортов

В переносимых packages/src прямой именованный потребитель — session-snapshot/normalize.ts:
prepareSessionSnapshotFixtureForComparison. Остальные runtime helpers/types напрямую используются
тестами llm-replay и office fixtures; plugin монтируется по имени в snapshot patches. По поиску вне
client/api/typert/host/experimental других именованных src-импортов не найдено. Все экспорты сохранены.

## Не перенесено

Учёт по именованным `it`/`it.each`, варианты каждой таблицы объединены в Rust-цикл:
llm-replay.spec.ts содержит 129 clauses (121 it + 8 it.each); session-format-corpus.spec.ts — 9.
110 из 138 clauses имеют перенос/адаптацию; 28 пока не перенесены полностью:

- Шесть module-mock clauses llm-replay.spec.ts:234,257,283,314,345,382 — замена модулей/произвольных JS
  throws исключена PORTING §10; отсутствует отдельный catalog, версию которого можно сравнить при импорте.
- 21 catalog/history clause llm-replay.spec.ts:463,481,499,538,558,583,621,662,677,699,712,744,759,
  795,816,1068,1189,1828,1833,1841,1855 — требуют released v0–v3 decoder/migrations либо их точных ошибок.
  Текущие аналоги empty-tools, readonly fixture и fork-cut проверены отдельно и не выданы за исторические.
- Полный параметризованный обход всех поколений в session-format-corpus.spec.ts:80 — историческое
  восстановление и 13 точных отказов требуют настоящего catalog. Текущие поколения обходятся отдельно,
  без exemptions; headerless-файлы разрешены только исходным явным inventory.

Публичные replay/load/derive/adapter/stream возможности реализованы; это **не** заявление о готовности
released format catalog. Никакие recorded tool results этому плагину не поручены: он отдаёт только
StreamChunk, а read/write/bash исполняет настоящий агент.

## Не проверено

Native Windows delimiter/fs, 32-bit индексы, localeCompare при других локалях и произвольный ECMAScript RegExp.
Суррогатные/JSON edge cases наследуют ограничения hoh-util-values. Не запускался весь hoh milestone:
integration test этого crate воспроизводит recorded read/write/bash chunks через настоящий LlmRuntime;
сборка CLI, persistence и исполнители инструментов принадлежат другим writer lanes.

## Вопросы ведущему

- `SHIM(hoh-session-format-catalog)` в src/shim.rs: оставлена прежняя restore seam; owner crate уже есть.
  В этом fix round подключены настоящие current encoders, restore/history integration в задание не входит.
  Текущий shim использует
  Session::from_restore для header, публичный adopt_session_event для content/surface metadata,
  непрерывность seq, safe time и final inherited marker для cut;
  он не подменяет полный released content-only restore и исторические migrations.
  v0–v3 явно отклоняются типизированным UnsupportedMigration с физической строкой. После приёмки catalog
  заменить оставшуюся seam на createRestore/decodeRow/finish и включить 22 зависимых clauses.
  Runtime surface fold намеренно не запускается: image/offload требует owning projection plugin,
  которого content-only reader исходного catalog не загружает. Полная schema и cross-event relationships
  остаются обязанностью настоящего catalog, и текущий shim не выдаёт их за проверенные.
- `SHIM(hoh-compaction)` в src/shim.rs: отсутствует owner-типа compaction/summary. Приватный read-only
  payload содержит только llmStreamCall/rawOutput/usage из source declaration; заменить его импортом
  владельца после публикации. StreamChunk/ContentBlock/TokenUsage уже импортированы из hoh-llm.
- По решению ведущего lookahead в fromRequest оставлен известным отличием, без новой regex-зависимости.
  Например, TS для {{fromRequest:goal-(?=42)42}} и request goal-42 подставляет goal-42;
  Rust возвращает точно следующий многострочный текст (закреплён regression test):

```text
llm-replay: fromRequest has an invalid pattern "goal-(?=42)42": SyntaxError: regex parse error:
    goal-(?=42)42
         ^^^
error: look-around, including look-ahead and look-behind, is not supported
```

Другие invalid-pattern diagnostics также содержат Rust regex detail вместо V8 SyntaxError detail.
Новых regex crates нет; feruca уже разрешён workspace/DEPS для исходной collation.

## Проверка

Все Cargo-команды: CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-cf1.
Во время записи выполнялся только `cargo check -p hoh-llm-replay --tests`.
Library check прошёл; test check обнаружил недоступный futures_util::poll!, заменённый std::future::poll_fn.
Итоговый `cargo check -p hoh-llm-replay -p hoh-conformance --tests` — exit 0 (19m02s).
`cargo test -p hoh-llm-replay -p hoh-conformance -- --test-threads=2 --nocapture` — exit 0:
replay `test result: ok. 96 passed; 0 failed; 0 ignored` (0.08s),
corpus `test result: ok. 9 passed; 0 failed; 0 ignored` (28.86s); всего **105 passed, 0 failed**.
Первый corpus-прогон выявил ошибочное требование runtime image/offload projection; shim исправлен
на content-only чтение с публичными owner validators, после чего весь набор прошёл.
Первый clippy выявил large_enum_variant; AdapterRegistrationHandle помещён в Box.
`cargo fmt -p hoh-llm-replay -p hoh-conformance -- --check` — exit 0.
Логи итоговых check/test/clippy сохранены в target-cf1/cf1-{check,tests,clippy}.log.
`cargo clippy -p hoh-llm-replay -p hoh-conformance --all-targets -- -D warnings` — exit 0,
`Finished dev profile [unoptimized + debuginfo] target(s) in 3.64s`.
Итоговый clippy log: target-cf1/cf1-clippy-final.log; fmt-check обоих crates после lint-правки — exit 0.

## Круг rv-replay w7

Ревью /mnt/toshiba/alexandrie/hoh/logs/rv-replay.full.md прочитано целиком.
Пункт 1 оставлен известным отличием по решению ведущего; точный текст приведён выше.
Пункт 2: admission неполных chunks по одному type, lossless Unvalidated entry;
регрессии проверяют chunks/throw и patches, все семь типов, точный JSON и неизвестные поля.
Пункт 3: настоящий catalog physical encoder, отдельные exact regressions для header key order
и range encoding. Каталог и другие зависимости не редактировались.
Пункт 4: locale tie-break, включая a/B и é/z с различными текстами ответов.
Пункт 5: возвращено строгое сравнение всех трёх compaction entries, включая overflow failure,
summary block-start/block-end, usage 9/2, finish stop и завершающий обычный вызов.
Ранее отложенные 28 source clauses остаются отложенными; этот круг добавляет четыре regression tests
и усиливает существующие проверки, не объявляя restore/history integration выполненной.

Окружение этого круга: CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2,
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w7, все команды с `cargo --offline`.
`cargo --offline check -p hoh-llm-replay --tests`: exit 0,
`Finished dev profile [unoptimized + debuginfo] target(s) in 18m 36s`.
`cargo --offline fmt -p hoh-llm-replay -- --check`: exit 0, без вывода.
`cargo --offline test -p hoh-llm-replay -- --test-threads=2`: exit 0,
`Finished test profile [unoptimized + debuginfo] target(s) in 17m 37s`.
Основной набор: `test result: ok. 100 passed; 0 failed; 0 ignored` (0.37s).
Corpus: `test result: ok. 9 passed; 0 failed; 0 ignored` (1513.15s, высокая дисковая нагрузка).
Unit/doc: по 0 tests, exit 0. Итого **109 passed; 0 failed; 0 ignored**.
`cargo --offline clippy -p hoh-llm-replay --all-targets -- -D warnings`: exit 0,
без предупреждений. Первый запуск был оборван перезапуском до начала проверки и
не оставил результата; повтор завершился успешно на неизменённых исходниках.
Логи: /tmp/w7-replay-{check,test,clippy}.log.

## Решение ведущего по проверке закрытия (19.11, Sonnet)

Публичный `Deserialize` у `ReplayEntry` / `ReplayOverrideDoc` после добавления `ReplayEntry::Unvalidated(Value)` принимает
любой JSON (например `42` или `{"kind":"bogus"}`) как `Unvalidated`. Проверка формы, как в TS, выполняется только
в `read_override` при загрузке файла; поэтому записи для replay нужно получать через загрузку файла, а не прямым
`serde_json::from_value::<ReplayEntry>`. Крейт тестовый, прямых внешних вызовов нет; если появятся — сузить ручным
`Deserialize` до kind chunks/throw.
