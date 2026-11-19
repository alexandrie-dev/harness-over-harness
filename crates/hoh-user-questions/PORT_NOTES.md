# hoh-user-questions ← packages/interaction/user-questions @ 39163dd30f70

## Файлы

| TypeScript | Rust | Примечание |
|---|---|---|
| `src/types.ts` | `src/types.rs` | Вопросы, intents, ответы, pending/settled view и request/wait |
| `src/projection.ts` | `src/projection.rs` | Чистый fold и `userQuestions` projection version 2 |
| `src/timed-wait.ts` | `src/timed_wait.rs` | Абсолютный deadline, Host timer и Client claims |
| `src/index.ts` | `src/service.rs`, `src/lib.rs` | Waterfall, валидация, timed ask, late reply и lifecycle reservation |
| `tests/projection.spec.ts` | `src/projection.rs` (`cfg(test)`) | 16 source cases сведены в 19 Rust-тестов, включая strict checkpoint и loose payload |
| `tests/timed-wait.spec.ts` | `src/timed_wait.rs` (`cfg(test)`) | Все 6 source cases |
| `tests/user-questions.spec.ts` | `src/service.rs` (`cfg(test)`) | Waterfall, errors, live-agent attestation, timed waits, late replies и approval validation |
| `locale/{de,en,es,fr,ja,pt,ru,zh}.json` | одноимённые файлы в `locale/` | Перенесены дословно |
| README/package metadata | этот файл и `Cargo.toml` | Прочитаны; README не копируется |

## Сохранённые контракты

- Сервис и событие называются `userQuestions` и `user-questions/request`; event остаётся waterfall и при наличии Agent проходит через его scope carrier.
- `ask` проверяет pre-abort, непустой batch, точного live root Agent и явный `origin: subagent`. Коды `ASK_ABORTED`, `CALLER_NOT_LIVE`, `DELEGATED_CALLER`, `NO_PROVIDER`, `EMPTY_QUESTIONS`, `PROVIDER_ERROR` и все diagnostics intent сохранены.
- `plan-review` и `approval` являются только presentation intent. Проверяются offered labels и detail; approval дополнительно требует один single-select blocking question, разные action labels и точное совпадение набора options.
- Timed wait имеет абсолютный Host deadline. Без claim работает Host timer; Client claim приостанавливает timer, но не сдвигает deadline. Abort client signal сам освобождает claim; последний release после deadline немедленно даёт `ASK_TIMED_OUT`. Parent cancellation и dispose сохраняют first reason.
- Projection читает только записанные `request/header`, `tool/call`, `tool/result`, `tool/ptc-dispatch` и `user/message`. Legacy schema не создаёт cards. Pending payload и `TOOL_OUTCOME_UNKNOWN` переводят call в `continued`; successful answer batch settles, failure удаляет; admitted late reply — единственная durable запись позднего ответа.
- `answer` принимает только полный batch с каждым question id ровно один раз, атомарно проверяет и резервирует reply до `steer`, откатывает reservation при ошибке и не допускает второго reply в reservation/nextTurn/nextStep. Claimed/discarded/session events освобождают reservation в тех же точках, что источник.
- Регистрация foreground wait и проверка `DUPLICATE_WAIT` составляют одну mutex-секцию. RAII guard закрывает и удаляет wait при отмене Rust future, а lifecycle effect закрывает все оставшиеся waits при выгрузке plugin scope, даже если вызовы ещё держат service `Arc`.
- Checkpoint и wire view разбираются строго; active call ids непусты и уникальны; `inheritedEventCount` не даёт seeded prefix повторно влиять на состояние.

## Отличия от образца

- Source class одновременно является Cordis plugin и service. Rust разделяет его на value `UserQuestionService`, plugin `UserQuestionsPlugin` и service key `UserQuestions`; последние два — инфраструктурные публичные символы без отдельного source export. Анонимные вложенные типы получили имена `ApprovalSubject`, `ApprovalDisplay` и `LocalizedQuestionText`.
- `UserQuestionError::new` возвращает общий `hoh_error::HarnessError` с именем `UserQuestionError`, как остальные error owners workspace; отдельный Rust error-класс не дублируется.
- TypeScript `AbortSignal` и `setTimeout` представлены общими `hoh-timeout::{AbortSignal, AbortController}` и `tokio::time::Instant`. `TimedQuestionWait::attach` возвращает RAII `TimedQuestionClaim`: его `frame()` — единственный stream frame, `ended()` — lifetime stream, Drop — закрытие stream/release claim.
- `TimedQuestionClaim` и `WaitFrame` — публичные Rust-аналоги состояния async iterator, которого как именованного export в TypeScript нет; `TimedQuestionWait::new` возвращает `Arc`, чтобы claim и timer разделяли тот же lifetime без `unsafe`.
- Source `wire.view(state)` возвращает ссылку `state.questions`. Workspace wire boundary требует сериализованный `Arc<Value>`, поэтому ссылочная идентичность внутреннего объекта непредставима; тест сохраняет наблюдаемый контракт — равное полное view — и отдельно проверяет `Arc::ptr_eq` самого projection state при неизменяющем событии.
- `WeakMap<Session,...>` представлен identity-списком по pointer-equality `Session`; записи удаляются при admission/discard/turn end, а service state умирает вместе с plugin scope. В отличие от weak-key JS, незавершённая reservation временно удерживает Session до её release либо выгрузки сервиса.
- Typert-generated `remote`/`typert` entry points не генерируются по PORTING §8; публичные методы находятся на `UserQuestionService`.
- `hoh-sdk-jsonrpc-server` сейчас содержит два явных `SHIM(hoh-user-questions)` (`AskUserQuestionRequest` и `UserQuestionError`), а его wire answer/item приходят из `hoh-protocol`. Владелец consumer должен заменить shim на типы этого крейта отдельной правкой; этот пакет чужой API не меняет.
- Конструкция Rust API делает non-integer timeout непредставимым (`u64`); нулевое и превышающее platform timer значение по-прежнему дают `BAD_TIMEOUT`. Произвольное JavaScript rejection непредставимо: answerer возвращает `BoxError`; coded `HarnessError` сохраняется, остальные ошибки получают `PROVIDER_ERROR` с cause.
- Optional source injection сохранён: service публикуется и без `sessionProjections`, а dependency-fiber регистрирует projection, если сервис появляется позже; при снятии dependency binding handle очищается.

Тесты colocated с owning-модулями, а не вынесены в `tests/*.rs`, потому что проверяют private reservation/projection lifecycle seams; это не меняет production API. В источнике 65 именованных `it`/`it.each` групп. Поведение всех 63 представимых групп перенесено в 90 Rust-тестов (табличные и близкие lifecycle cases объединены, конкурентные reservation и Rust cancellation branches выделены отдельно).

## Не перенесено

- Две проверки восстановления plain-object rejection (`restores a transported provider rejection...` и `restores a transported UserQuestionError...`). Rust waterfall возвращает `BoxError`, поэтому значение, которое не реализует `Error`, непредставимо. Нативный `UserQuestionError` и любой coded `HarnessError` сохраняются и проверены.
- Typert-generated `remote`/`typert` entry points — по PORTING §8.

## Не проверено

Нет платформенных веток. Consumer migration из `hoh-sdk-jsonrpc-server` намеренно не проверялась: crate пока использует свои явные shim-типы.

## Вопросы ведущему

- Владелец `hoh-sdk-jsonrpc-server` должен заменить `SHIM(hoh-user-questions)` request/error и wire-типы `hoh-protocol` на owner-типы этого крейта. Чужой крейт здесь не менялся.

## Круг ревью 114201

- `answer` теперь сначала проверяет уже зарезервированный либо находящийся в
  `nextTurn`/`nextStep` ответ и только затем валидирует новый batch. Регрессия
  `queued_reply_precedes_bad_answer_validation` ставит корректный ответ в очередь,
  повторяет вызов с пустым batch и требует `REPLY_QUEUED`; прежний код возвращал
  `BAD_ANSWER`. Финальная проверка и вставка reservation остаются одной mutex-секцией,
  поэтому исправление не ослабляет конкурентную гарантию.
- `ASK_TIMED_OUT` преобразуется в `pending` только для ошибки с именем
  `UserQuestionError`, как `instanceof` в TypeScript. Регрессия
  `foreign_ask_timed_out_code_is_not_mapped_to_pending` возвращает одноимённый код
  от `ForeignProvider`; прежний код ошибочно скрывал его как `pending`.
- Незакрытые замечания о тестах перепроверены по исходным спекам. Уже присутствуют
  strict checkpoint round-trip с `detail`, wire view, сохранение `Arc` fold на
  неизменяющем событии, пустой content успешного `tool/result`, отклонение
  `custom:null`, а также точные (не `contains`) семь approval diagnostics.
- Замечание о чтении inbox под service-state lock оставлено без изменения: финальная
  проверка должна быть линейна с reservation, а production `ReactLoopInbox` getters
  только читают projection и не публикуют lifecycle-события. Тест конкурентного
  резервирования продолжает проверять эту границу на двух потоках.

## Проверка

- `cargo test -p hoh-user-questions --offline`: 92 passed, 0 failed; doc-tests 0/0.
- `cargo clippy -p hoh-user-questions --all-targets --offline -- -D warnings`: чисто.
- `cargo fmt -p hoh-user-questions -- --check`: чисто.

Новых внешних зависимостей нет: внешние зависимости уже присутствовали в workspace/Cargo.lock, остальные зависимости — path-крейты рабочего пространства.
