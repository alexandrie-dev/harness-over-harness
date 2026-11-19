# hoh-session-log-deepseek ← packages/session/session-log-deepseek @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs | plugin/service, incremental acceptance fold, exclusions, wire projection и byte-bounded batching |
| src/types.ts | src/types.rs | request, exclusion, header/event/surface-op и extension DTO |
| tests/config.spec.ts, tests/upload.spec.ts, tests/feedback-composition.spec.ts | tests/upload.rs | 37 тестов всех представимых контрактов и интеграции `Session` → registry |
| locale/*.json | locale/*.json | все 8 файлов скопированы до байта |
| README.md, README.zh.md | — | прочитаны; собственного LICENSE нет |

## Сохранённое поведение

- Поле `dsh_session_log` появляется только для живой Session и непустого pending suffix. `enabled` читается из volatile `Cfg` на каждом запросе; `maxBytes` проверяется и фиксируется при создании сервиса.
- Acceptance-marker хранит `sessionId`, текущую версию формата и `throughSeq`. Свёртка сначала проверяет версию поколения, затем watermark; отсутствующая или JSON-null версия следует исходному `?? 0`, чужие session id и старые поколения игнорируются, malformed текущего поколения закрывают путь ошибкой, а конкурентные acceptance берут максимум.
- Wire header удаляет `isSeeded` и заменяет его точным `seedLength`. Surface operation/source refs остаются только на допустимых типах; assistant не получает `sourceEventSeqs`, известные log-only события очищаются, неизвестные ignorable записи сохраняют opaque metadata.
- Лимит считается по UTF-8 JSON всего поля тем же инкрементальным алгоритмом: выбирается самый длинный непрерывный prefix; первый слишком большой event блокирует хвост и не двигает watermark. Предупреждение для одной блокирующей последовательности выдаётся один раз.
- Exclusion registrations принадлежат вызывающему Context, дубликаты независимы, ошибка predicate изолируется реестром полей, а уже подготовленная acceptance остаётся действительной после регистрации нового exclusion или unload. Перед вызовом predicates снимается `Arc`-snapshot, поэтому reentrant регистрация не взаимоблокируется на mutex.

## Отличия Rust API

- Исходный класс разделён на `SessionLogDeepSeekPlugin` и публикуемый `SessionLogDeepSeekService`; ключ сервиса — `SessionLogDeepSeek = "sessionLogDeepSeek"`, имя plugin-класса — `SessionLogDeepSeekService`, пакетное имя экспортировано как `NAME`.
- `acceptedThrough` представлен `accepted_through(&Session) -> Result<SessionSeqCursor, BoxError>`, потому что malformed persisted log в Rust выражается `Result`, а не исключением. Стабильный `Arc<Value>` заголовка служит identity exact Session; process-wide cache хранит только `Weak<Value>`, инкрементально читает appended tail и не удерживает Session/журнал, повторяя WeakMap образца без правки чужого API.
- `registerExclusion` принимает явный `&Context` владельца и predicate с `Result<bool, BoxError>`, возвращая kernel `Disposer`. Это сохраняет throw/cleanup/rebind семантику без JavaScript function identity.
- `DeepSeekSessionLogWireEvent` хранит opaque `surfaceOp`/`sourceEventSeqs` как JSON, поскольку неизвестные ignorable записи обязаны сохранять произвольную форму. Для публичного известного surface-контракта также экспортирован типизированный `DeepSeekSessionLogWireSurfaceOp`.
- Rust `serde_json::Value` всегда является lossless JSON и не допускает JS getter/cycle/BigInt. Поэтому искусственный Vitest monkeypatch `JSON.stringify` на `RangeError: Invalid string length` непредставим; production `json_bytes` всё равно fail-closed возвращает превышающий любой лимит размер при ошибке serializer.

## Адаптация тестов

- TypeScript type-only assertions для raw numeric DTO и surface operation покрыты публичными Rust-типами. Зависимый union event-type → допустимые metadata представлен одним wire DTO с `Value` для opaque forward compatibility; публичный DTO технически позволяет собрать неверное сочетание вручную, но единственный production-конструктор закрыт и строго удаляет недопустимые поля. Это сознательное отличие публичной type boundary.
- `eventAt` у `Session` читает append-only Vec и не может вернуть missing event ниже уже захваченного `seq` без изменения чужого API; исходный mock-only случай отмечен как непредставимый. Weak incremental cache проверен через последовательные prepare/accept и persisted reconstruction.
- Три полных Loader/HTTP feedback-composition случая сведены к границам, принадлежащим этому крейту: retry до accept побайтно повторяет исходный payload, feedback records уходят неизменёнными и не меняют message surface, backlog дренируется более чем двумя непрерывными byte-bounded request, volatile enable продолжает с подтверждённого cursor без смены fiber. Реальный HTTP transport и JSONL persistence принадлежат соответствующим workspace-крейтам и здесь повторно не поднимаются.
- Табличные варианты source-тестов объединены внутри отдельных Rust-тестов: purposes, malformed versions/watermarks, config limits, opaque metadata и exclusion lifecycle.
- Точное число чтений incremental-fold (2→0→2) не наблюдаемо через конкретный Rust `Session`: `event_at` не инъецируется. Вместо mock-счётчика проверено сохранение результата при повторном fold и подхват только дописанного acceptance-хвоста; production cache по-прежнему хранит `scanned_events`.

## Инженерные заметки

- `ACCEPTANCE_FOLDS` — process-wide `Mutex<Vec<_>>` с линейным поиском. Он не удерживается через `await` и содержит только `Weak` identities, но первый fold большой сессии выполняется внутри синхронной критической секции.
- Дедупликация blocked-warning живёт в экземпляре сервиса, тогда как TS использует модульный `WeakMap`: после полной замены plugin Rust может повторить предупреждение для той же блокирующей последовательности. На содержимое поля и cursor это не влияет.
- Производные `Debug` у wire DTO включают полные message data. Эти значения не логируются самим крейтом, но вызывающий код не должен печатать их в чувствительный журнал через `{:?}`.

## Не проверено

- Платформенных веток и собственного I/O нет. Внешняя сеть не используется.
- Искусственное исчерпание размера Rust `String`/`Vec` безопасно не воспроизводится; oversized finite event, UTF-8 boundary, exact limit, default 8 MiB и warn-once production-ветка покрыты.

## Вопросы ведущему

Нет. Чужие крейты и API не менялись; SHIM не потребовался. Новых внешних зависимостей нет.

## Проверка

- `cargo test -p hoh-session-log-deepseek --offline`: 37 passed, 0 failed.
- `cargo clippy -p hoh-session-log-deepseek --all-targets --no-deps --offline -- -D warnings`: чисто.
- `cargo fmt -p hoh-session-log-deepseek -- --check`: чисто.
- `diff -r` locale с исходным пакетом: различий нет.

Все команды выполнялись с `CARGO_BUILD_JOBS=2`, `CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x5`, offline и под внешним `timeout`.
