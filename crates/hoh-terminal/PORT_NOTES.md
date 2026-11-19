# hoh-terminal ← packages/terminal/terminal @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/types.ts` | `src/types.rs` | backend/session contracts, branded id, snapshots and operations |
| `src/error.ts` | `src/error.rs` | stable codes, cleanup detail and ordered aggregate causes |
| `src/index.ts` | `src/lib.rs` | registry, exact-owner authorization, publication and awaited teardown |
| `tests/service.spec.ts` | `tests/service.rs`, unit test in `src/lib.rs` | 24 / 0 раскрытых исходных сценария; дополнительные race/ordered-cause проверки |
| `locale/*.json` | `locale/*.json` | 8 файлов без изменений |

## Отличия от образца

- Caller-owned backend effect передаётся явно в
  `register_backend(&Context, Arc<dyn TerminalBackend>)`.
- Синхронный JavaScript `startSend()` выражен как `Result<TerminalSendOperation, BoxError>`;
  `TerminalBackendSession::read` также возвращает `Result`, чтобы Rust backend seam мог
  сообщить JavaScript throw без паники.
- `TerminalSignal` использует Rust-варианты `Sigint` … `Sighup`, сохраняя wire-значения
  `SIGINT` … `SIGHUP` через serde.
- Нативный JavaScript `AggregateError` представлен дополнительным публичным
  `AggregateError` с ordered `Vec<SharedError>`. Дополнительная публичная
  `TerminalBackendCleanupDetail` хранит поля наследника `spawnError`, `cleanupError` и
  `errors` по правилу PORTING §7.
- Дополнительный публичный alias `TerminalFuture` — object-safe Rust-замена возвращаемых
  backend `Promise`; `TerminalSessionDeps` и service marker `Terminals` нужны механизму
  статической инъекции hoh-kernel.
- Приватные в TypeScript `disposeOwned`/`disposeAll` открыты как дополнительные публичные
  `dispose_owned`/`dispose_all`: они являются awaitable lifecycle entry points для Rust
  service actor и позволяют backend/service владельцам явно дождаться очистки.
- Асинхронные backend futures исполняет service-owned actor. `spawn()` синхронно резервирует
  имя/pending slot и передаёт driver актёру до вызова backend; `kill()` также завершает изменение
  registry внутри actor-owned driver. Поэтому drop caller future не отменяет setup/close и не
  оставляет lifecycle gap.
- Все конкурентные `dispose_all()` и `dispose_owned()` после начала глобального teardown ждут
  одно actor-owned settlement и получают ту же ошибку. Как в образце, финальная очистка каждого
  владельца повторно закрывает сессии, которые не удалось закрыть общим проходом.
- После начала глобального teardown новые backend-регистрации и `start_send()` отклоняются с
  `SERVICE_DISPOSING`. Это намеренно строже образца: Rust `Context` может пережить service scope,
  поэтому поздний contribution или backend-side send создали бы осиротевший эффект.
- Ручные `Debug` реализации не печатают terminal input/output и MOTD: поля содержимого заменены
  на `<redacted>`, чтобы диагностическое форматирование не раскрывало пароли и вывод терминала.
- Type-only проверка TypeScript brand заменена проверкой конструктора `terminal_session_id`.
  Доступ теста к private backend map заменён unit-тестом exact contribution helper; остальные
  private-map проверки наблюдаются через публичный API.

## Не перенесено

- README и `README.i18n.yaml` не являются исполняемым API крейта.
- Typert/Remote entry points отсутствуют в пакете-образце; генерировать их не требовалось.

## Не проверено

- Реальные PTY, POSIX process groups и платформенные signal syscalls принадлежат backend-пакетам;
  этот registry проверен на объектно-безопасных stub backend/session реализациях.

## Вопросы ведущему

- После приёмки terminal backend/tool крейты следует перевести с локальных shim-типов на
  `hoh_terminal::{Terminals, TerminalBackend, TerminalBackendSession, ...}`.
