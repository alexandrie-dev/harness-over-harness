# hoh-session-persistence-jsonl ← packages/session/session-persistence-jsonl @ 39163dd3

## Круг rv-jsonl (актуальнее первоначального учёта ниже)

Дополнение jsonlfix4: corpus-обходы historical preparation теперь используют
единый аналог TS `readListedHeader`. Посторонние unsupported/corrupt generations
не считаются дочерними сессиями и не блокируют parent; cancellation, I/O и прочие
ошибки по-прежнему передаются вызывающему. Regression в `review_regressions.rs`
проверяет независимые read и write fixtures для none/zstd: read не публикует V4,
write публикует parent, оба источника остаются побайтово неизменными. На старом
коде тест завершался 0/1 с `SessionFormatUnsupportedError` от `future-root`;
после исправления фокусный прогон — 1/0.

Все пять замечаний rv-jsonl.full.md подтверждены по TypeScript.
Frozen row admission и relationships теперь вызываются из принятого
hoh-session-format-v3-to-v4, включая ownership compaction и schema/uniqueness
native subagent catalog. Ручная частичная relationship validation удалена.
Header classification и историческая цепочка используют hoh-session-format-catalog;
child collector читает через historical catalog до V3 и вызывает
historical_child_catalog_source, сохраняя реальные descriptors и child-local failures.
Identity проверяет id/createdAt/parent/origin и cwd/isSeeded/delegationDepth/agentPreset.
Числовое сравнение соответствует JavaScript; absent не равно явному значению.

Read V0–V3 восстанавливает V4 и держит primed immutable events без публикации;
write берёт существующий lease, затем использует generic generation preparation,
native verifier и no-overwrite publication. Исходный generation сохраняется.
Child physical witnesses и membership непосредственных детей перепроверяются перед публикацией.
Foreign numeric version (включая 4.5) проверяется до header shape и выдаёт
SessionFormatUnsupportedError с JS number formatting и исходным upgrade text.

Новые regression groups в tests/review_regressions.rs восстанавливают
migration-refusal:383, catalog-migration:106/213/292, jsonl:798; дополнительно
проверяются все четыре historical versions, обе compression/access и все четыре
identity поля. Точная released V0 one-turn форма из `jsonl.spec.ts:798` проходит
read/service-flush без materialization и write с отдельной V4 publication. Начальный ledger ниже —
история первоначального порта: эти пять source groups больше не отложены.
API зависимостей для перечисленных исправлений достаточно; чужие crates и catalog
не менялись, commit не создавался. Остальные ранее отложенные спецификации не
объявляются проверенными только на основании подключения catalog.

Финальные проверки с `CARGO_NET_OFFLINE=true`, `CARGO_BUILD_JOBS=2` и
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-c4`:

- `RUST_TEST_THREADS=2 cargo test --offline -p hoh-session-persistence-jsonl --no-fail-fast`:
  exit 0, суммарно **275 passed; 0 failed**; вывод `/tmp/c4-jsonlfix4-tests.log`.
- `cargo clippy --offline -p hoh-session-persistence-jsonl --all-targets --no-deps -- -D warnings`:
  exit 0, предупреждений нет; вывод `/tmp/c4-jsonlfix4-clippy.log`.
- `cargo fmt -p hoh-session-persistence-jsonl -- --check`: exit 0;
  вывод `/tmp/c4-jsonlfix4-fmt.log`.

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs`, `src/backend.rs`, `src/io.rs`, `src/error.rs` | kernel plugin, Config, настоящий SessionPersistenceService, V4 create/open/read/append/flush/stat/list, POSIX I/O, metadata/revision caches |
| `src/storage.ts` | `src/storage.rs` | handles, mutation chain, live buffer 200 ms, retained failed batches, single-flight drain/close, writer claims, teardown |
| `src/format.ts` | `src/format.rs`, private `src/shim.rs` | пути, UTF-16 escaping, текущие строки, incremental scanner; physical framing остаётся локальным, frozen admission/relationships вызывают owner v3-to-v4 |
| `src/lease.ts` | `src/lease.rs` | Linux flock, проверка inode, три попытки, освобождение без unlink |
| `src/generation.ts` | `src/generation.rs` | generic adapter/restore, bounded stable read, stage encoding, verification, source witnesses, exclusive publication, cleanup/errors |
| `src/migration-verifier.ts` | `src/migration_verifier.rs` | bounded scheduler, natural exit, cancellation/termination/join; native worker вместо Node Worker |
| `src/worker.ts` | `src/worker.rs` | проверка request, строгое V4 verification; Node workerData/postMessage заменены типизированной границей |
| `src/catalog-migration.ts` | `src/catalog_migration.rs` | witnesses, identity guards и настоящее извлечение исторических descriptors через catalog/v3-to-v4 owners |
| `src/zstd.ts` | `src/zstd.rs` | checksummed concatenated frames, structural scanner, complete/prefix decoding |
| `src/zstd-public-decoder.ts`, `src/zstd-private-decoder.ts` | одноимённые snake_case модули | два исходных имени над безопасным native reusable decoder; Node private ABI не переносится |
| `src/testing/generation.ts` | `src/testing/generation.rs` | per-instance runtime overrides, без глобальных mocks |
| `src/win32.ts` | — | исключено прямым решением ведущего: только Linux |
| `locale/*.json` | `locale/*.json` | все 8 файлов без изменений, `diff -ru` exit 0 |
| `tests/fixtures/released-v0-real-shapes.jsonl` | такой же путь | сохранён для интеграции исторического reader |
| `README.md`, `package.json`, все `tests/*.spec.ts` | эти заметки, manifest, `tests/*.rs` | прочитаны; таблица покрытия ниже |

Собственного LICENSE и отдельно лицензированных фрагментов в пакете нет. Чужие crates,
корневой manifest и lockfile вручную не редактировались.

### Учёт тестов

Единица этой таблицы — **исходное объявление `it`**, без перемножения `it.each` и
`describe.each`. В Rust варианты исполняются циклами; несколько исходных объявлений
иногда проверяются одним тестом. «Часть» не считается полным переносом.

| source spec | объявлений | перенесено/адаптировано | часть | отложено/исключено |
|---|---:|---:|---:|---:|
| jsonl | 142 | 121 | 3 | 18 |
| generation | 60 | 50 | 1 | 9 |
| catalog-migration | 30 | 10 | 0 | 20 |
| migration-verifier | 14 | 12 | 1 | 1 |
| migration-verifier-real | 1 | 1 | 0 | 0 |
| migration-refusal | 15 | 3 | 0 | 12 |
| current-event-admission | 2 | 2 | 0 | 0 |
| fork-delivery | 2 | 2 | 0 | 0 |
| native-developer-admission | 5 | 5 | 0 | 0 |
| native-source-admission | 4 | 4 | 0 | 0 |
| retired-content-admission | 2 | 0 | 2 | 0 |
| v2-system-migration | 4 | 1 | 0 | 3 |
| v2-ptc-migration | 1 | 0 | 0 | 1 |
| v3-restart-migration | 2 | 0 | 0 | 2 |
| content-admission | 2 | 0 | 0 | 2 |
| multi-edge-publication | 3 | 0 | 0 | 3 |
| load-path | 1 | 0 | 1 | 0 |
| lease | 19 | 19 | 0 | 0 |
| win32 | 11 | 0 | 0 | 11 |
| zstd | 29 | 26 | 0 | 3 |
| zstd.compat | 1 | 1 | 0 | 0 |
| **итого direct specs** | **350** | **257** | **8** | **85** |

`jsonl.spec.ts` и `zstd.spec.ts` также вызывают 20 shared contract cases каждый;
36 из этих 40 инстанцирований покрыты `tests/contract.rs` (оба encoding) и torn-tail
тестами. Два исходных случая в каждом encoding требуют отрицательного/дробного
u64 offset либо BigInt/undefined и исключены как непредставимые в typed API.
Из 12 live-write-contract cases перенесено 11; arbitrary string rejection исключён.
Итого по этой единице учёта: **402 source cases, 304 перенесено/адаптировано,
8 частично, 90 отложено/исключено**. Есть дополнительные Rust regressions и
свернутые в циклы таблицы; число Rust test functions не равно этим числам.

Покрытие включает настоящий disk I/O для raw и zstd V4, default/config/plugin wiring,
lazy materialization, exact plaintext layout, dense seq, byte/UTF-16 paths, seeded
cut и fork delivery, Arc sharing, monotonic reads, metadata-only listing, caches,
shared concurrency limit, flock/inode churn, cancellation identity/cleanup, live
buffer/disposal/retry, rollback после fsync failure, partial zstd-frame recovery,
staging/competing publication, checksums и strict embedded stream verification.

## Отличия от образца

- Один plugin `JsonlSessionPersistencePlugin`, `NAME = "session-persistence-jsonl"`;
  предоставляет ключ `hoh_session_persistence::SessionPersistenceService` с исходным
  именем `sessionPersistence`. Config имеет исходные JSON keys `root`, `compression`
  (по умолчанию `zstd`) и `listConcurrency` (3). `JsonlCompressionSchema` выражен
  `schemars::JsonSchema` enum и Config; отдельного schemastery value в Rust нет.
  Kernel сам соединяет dependent unload с provider undo; исходный вложенный Cordis
  service scope заменён provider context. Live subscriptions и handle sweep — его effects.
- Дополнительный публичный concrete backend `JsonlSessionPersistence`, конструкторы
  `Config::new`, `new/with_io_hook`, `close/root/compression` делают Rust API
  проверяемым и пригодным для обычной композиции. `encode_segment_utf16` принимает
  lone surrogates, которые Rust str не представляет. Дополнительные test seams:
  `IoOperation/IoRequest/IoHook`, `NodeFileError::new`, `testing::writer`,
  `create_jsonl_test_backend`, `read_zstd_prefix`, `ZstdFrameDecoder::with_chunk_size`,
  `VerificationScheduler/VerificationWorker/VerificationEvent/VerificationWorkerFactory`,
  typed `VerificationRequest` и `parse_request`. Они не добавляют configuration knobs.
  `Result/JsonlError`, `JsonlGenerationRestore`, callback aliases и DTO
  `SessionLogScan/ScannerCheckpoint/CatalogSource/CatalogFailure/PreparedCatalogFacts`
  именуют исходные anonymous/private return types на публичной Rust границе.
- Метаданные, events, access и branded ids импортированы из владельцев seam/core.
  Header копируется на create; append snapshots выполняются до постановки в chain.
  Read возвращает caller-owned Vec с immutable Arc<SessionEvent>; frozen sharing
  заменён Rust ownership. У offset/length u64 отрицательные/дробные значения
  непредставимы; верхняя safe-integer граница проверяется отдельно. `asyncDispose`
  заменён явным await close; Drop не делает async I/O.
- JSON.parse/stringify представлены только `hoh_util_values::js_json_*`.
  Физические headers имеют `type` первым, затем исходный порядок SessionHeader;
  absent не заменён null. Source ranges пишет shared seq-range encoder.
  Строки и error interpolation используют JS helpers; строки сортируются по байтам
  readdir либо UTF-16 при corpus fingerprint. Нет localeCompare. Generic migration
  adapter по-прежнему может выдавать любой JSON, как исходный JSON.stringify.
- Native libzstd 0.13.5 вместо Node zlib: default level, checksum=true, header отдельно,
  один body frame на durable batch. Public/private decoder names используют один
  safe reusable context и owned output buffers; Node Symbol/ABI probing исключён.
  Backend async frame reads используют bounded blocking jobs, не блокируют executor.
  Побайтовое сравнение compressed output с Node здесь не запускалось; стандартный
  frame/checksum contract и точный восстановленный JSONL проверяются тестами.
- POSIX native fd вместо addon Node descriptor. RAII действительно освобождает fd;
  безопасный std File Drop не возвращает close(2) errno. Отказы close проверяются
  через per-instance IoHook, который после отказа тоже не оставляет held lock.
  Каждый `sync_all` предваряется **await flush**, включая rollback/truncate/directories.
- Native Promise continuations и timers стали handle-owned Tokio tasks: close
  отменяет timer и joins mutation/drain queues, ошибки не обрывают последующие операции.
  Dropped caller wait future не отменяет уже начатый append/publication/verifier.
  Shared single-flight close/drain/publication errors возвращаются через SharedError;
  class читается через get_ref/downcast_ref, identity сохраняется одним Arc.
  У service open обычный Rust Future: его Drop до adoption освобождает claim через
  RAII guard и kernel lease через fd ownership; в JS игнорирование Promise не отменяет
  операцию. Отмена через явный AbortSignal сохраняет исходный порядок checks/cleanup.
  Отдельная Rust regression проверяет отсутствие phantom writer после abort task.
- Worker — отдельная blocking native job с собственным Tokio current-thread runtime;
  block_on вызывается только внутри worker thread. Scheduler per-instance (вместо
  глобального mutable p-limit) удерживает два permits до join. Abort cooperative:
  terminate помечает signal и дожидается завершения native verification, а не убивает
  поток принудительно. Преждевременного освобождения permit или detached worker нет.
  JS stack string не восстанавливается; worker response превращается в plain Error.
  Если active AbortSignal уже сработал до обработки queued worker result, его reason
  выигрывает; отдельная regression сохраняет identity и проверяет terminate/join.
- Все ordinary source errors остаются BoxError/thiserror, не HarnessError. Три
  generation error classes сохраняют имя, поля и причины; Node filesystem messages
  форматирует hoh-error. String(error) использует явное имя класса, включая shared errors.

## Не перенесено

- Исторический body read/open и automatic V0→V4 publication теперь подключены через
  принятые catalog/v3-to-v4 owners. Read не публикует поколение, write публикует V4 и
  сохраняет источник. Отдельный backend-level revision-keyed cache подготовки и все
  source cases его single-flight/waiter-cancellation не восстановлены; generic
  generation preparation/publication остаётся single-flight на своей границе.
- Private `shim.rs` содержит только JSONL physical framing и адаптацию owner API.
  Frozen V4 admission/relationships, compaction ownership и native catalog
  schema/uniqueness выполняет `hoh-session-format-v3-to-v4`; historical descriptors
  и parent completion выполняют catalog/v3-to-v4 owners.
- В `jsonl.spec.ts` остаются отложены historical body groups на строках 681, 850, 873,
  904, 939, 966, 992, 1037, 1059, 1097, 1119, 1141, 1163, 1287, 1316, 1370, 2480.
  Строка 2601 (BigInt/fractional typed createdAt) непредставима.
  Частично: 729 — покрыты не все варианты исходной таблицы; 1945 — обычные removal/storage faults
  покрыты, private-method mock с Node ERR_INVALID_ARG_VALUE/NUL не воспроизводится;
  2588 — проверено detached metadata после create, немедленную mutation borrowed
  аргумента до await запрещает Rust. Остальные варианты этих групп не выдаются за полные.
- В generation отложены V2 catalog cases 306/321/351, Windows 748/1576/1591,
  non-JSON/non-Error JS cases 923/1264/1459. В case 1607 проверен POSIX,
  Windows исключён. Остальные generic algorithm cases адаптированы/сгруппированы.
- Из catalog body/backfill и historical migration-refusal восстановлены именно
  `catalog-migration.spec.ts:106/213/292` и `migration-refusal.spec.ts:383`;
  остальные content/multi-edge, v2-ptc, первые три v2-system groups и v3-restart
  не объявляются проверенными. Catalog metadata/current delivery/witness cases проверяются.
- В retired-content-admission исполнены V4 варианты, V3 отложены. В zstd отложен
  historical V0 read (415), private ABI fallback (231) и private injected stream
  machinery (280). В verifier не представим non-Error abort reason; stack-field
  часть worker response case заменена typed Error, message/lifecycle сохранены.
- Windows `win32.ts` и все 11 его cases исключены решением «Linux only».
  Node Loader.unwrapExports namespace mechanics в load-path заменены реальным
  Rust plugin load/config/service test, поэтому группа отмечена частичной.
  Built npm worker, two-process helper script, Loader teardown e2e и perf.ts не
  запускаются (PORTING §8/10/11); библиотека не запускает внешние процессы ради теста.
- JS BigInt, cycles, undefined fields, arbitrary thrown values и monkey-patching
  невозможны на используемой JSON/BoxError границе; не добавлялись fake passing tests.

## Не проверено

Реальный внешний conformance corpus, реальные crash/kill межпроцессные сценарии,
Windows/macOS, NFS flock semantics и Node-private ABI не запускались. Проверяется
реальный Linux flock между разными fd/backend instances, как в исходном lease spec.
Frozen V4 owner validation и V0–V3 end-to-end для восстановленных review cases проверены;
остальные отложенные source cases не объявляются пройденными. Общая глубина JSON и stack
policy остаются контрактом hoh-util-values/hoh-session; собственного глубокого clone нет.

## Вопросы ведущему

1. Для пяти замечаний rv-jsonl API catalog и v3-to-v4 достаточен; недостающих API нет,
   открытых вопросов по этому fix-round нет.
2. Найдены namespace plugin consumers в переносимых session/agent/team/subagent/tool/
   workflow/server пакетах. Прямых внешних consumers `src/*` helpers не найдено:
   format/path/scanner exports, lease/storage classes, generation runtime/errors,
   verifier worker helpers и native decoder classes сохранены, но сейчас используются
   только собственным backend/tests. Root JsonlCompressionSchema не найден отдельным
   named consumer; schema представлена Rust JsonSchema, см. отличие выше.

## Проверки первоначального порта (история)

Во время написания — только `cargo check -p hoh-session-persistence-jsonl --tests`.
После восстановления машины повторный check завершился exit 0 без предупреждений
(`Finished dev profile … in 38m 48s`, включая ожидание регулятора).
В первом runtime-прогоне `jsonl` дал 105 passed / 1 failed: private Session view
раньше seam отвергал retired `request/header.reason = "fallback"`. Исходная строка
теперь остаётся нетронутой, reason нейтрализуется только в private validation view,
и штатный seam pre-scan возвращает исходный Unsupported; assertion не ослаблен.
Перед повторным полным прогоном также добавлена Linux regression реального EISDIR:
ошибки fd read/write/fsync/ftruncate/fstat используют Node text без pathname suffix.
Окружение: `CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2`
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-c4`.

- `cargo clippy -p hoh-session-format-v2-to-v3 -p hoh-session-persistence-jsonl --all-targets -- -D warnings`:
  exit 0, без предупреждений (`Finished dev profile … in 30m 20s`, включая ожидание регулятора).
- `cargo fmt -p hoh-session-format-v2-to-v3 -p hoh-session-persistence-jsonl -- --check`:
  exit 0, без вывода.
- `RUST_TEST_THREADS=2 cargo test -p hoh-session-persistence-jsonl --no-fail-fast`:
  **exit 0, 267 passed; 0 failed; 0 ignored** (266 integration tests и 1 unit test;
  doc-tests — 0). `Finished test profile [unoptimized + debuginfo] target(s) in 24m 42s`,
  включая очередь регулятора. Полный вывод сохранён в
  `/tmp/hoh-c4-jsonl-verify-20261119-c4-01.log`.

Точные итоговые строки отдельного успешного прогона:

| binary | result line |
|---|---|
| unit | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| cancellation | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| catalog_migration | `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` |
| contract | `test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s` |
| current_event_admission | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` |
| fork_delivery | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s` |
| generation | `test result: ok. 41 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.56s` |
| jsonl | `test result: ok. 106 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s` |
| lease | `test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s` |
| live_write_contract | `test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s` |
| load_path | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` |
| migration_refusal | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` |
| migration_verifier | `test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| migration_verifier_real | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| native_developer_admission | `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s` |
| native_source_admission | `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s` |
| retired_content_admission | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` |
| v2_system_migration | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` |
| zstd | `test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s` |
| zstd_compat | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` |
| doc-tests | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |

Повторная проверка `diff -ru` для всех locale и `diff -q` для исходной V0 fixture —
exit 0. В src нет `todo!()`, `unimplemented!()`, `unsafe` и отладочного вывода.
После этих проверок менялись runtime-код и tests текущего fix-round; поэтому эти
исторические 267/0 не являются его evidence. Актуальные 274/0, clippy и fmt приведены
в начале файла.
