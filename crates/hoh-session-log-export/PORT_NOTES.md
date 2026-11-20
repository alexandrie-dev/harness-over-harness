# hoh-session-log-export ← packages/session-query/session-log-export @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/archive.ts` | `src/archive.rs`, `src/zip.rs` | canonical JSONL, live flush, persistence read/close, lineage preorder+dedupe, attachment discovery, missing manifest и DEFLATE ZIP перенесены |
| `src/markdown.ts` | `src/markdown.rs` | полный логический Markdown fold, fences/details/attachments и точный trailing newline перенесены |
| `src/index.ts`, `src/routes.ts` | `src/routes.rs`, `src/lib.rs` | команда, query/status/body/header vocabulary и transport-neutral response preparation перенесены |
| `src/client/controller.ts`, `src/client/copy-controller.ts` | `src/client.rs` | coalescing/FIFO download state, dismiss/dispose, deferred/fallback clipboard state и document-relative routes перенесены через browser adapters |
| `src/client/index.ts`, `Dialog.tsx`, `CopyToast.tsx` | типы состояния и controllers в `src/client.rs` | React presentation/composition остаётся владельцу Tauri UI; host-independent behavior находится в этом крейте |
| `locale/*.json` | `locale/*.json` | восемь package metadata locale скопированы без изменений |
| `tests/*.spec.*` | `tests/port.rs`, unit tests | критические варианты сведены в 38 Rust tests: Markdown, archive, persistence, lineage, attachments, ZIP, routes и оба controller |

## Сохранённое поведение

- Экспорт пишет текущий `session.v4.jsonl`: физический header содержит только канонические поля,
  `delegationDepth` по умолчанию равен нулю, затем идёт по одной JSON-строке на событие и обязательный
  финальный перевод строки. Только `SessionPersistenceNotFoundError` означает отсутствие; handle всегда
  закрывается, а ошибка `close` имеет семантику JavaScript `finally`.
- Root идёт первым, descendants — в lineage preorder и ровно один раз. Перед чтением каждого live Session
  выполняется flush. Отсутствующий descendant срывает весь архив. Session id, digest и filename не могут
  сформировать произвольный ZIP path.
- Attachment scan рассматривает только объявленные first-party content carriers и завершённые assistant
  blocks; неизвестные events/поля и malformed JSON остаются непрозрачными. `ATTACHMENT_NOT_FOUND` и
  `INVALID_ATTACHMENT_REF` дают `attachments-missing.txt`; остальные ошибки срывают экспорт. Generic file
  читается по чанкам непосредственно в DEFLATE writer.
- `write_session_log_zip` пишет ZIP инкрементально в переданный `Write`: local entries используют data
  descriptor, CRC/размеры попадают в central directory, вход и проверка cancellation разбиты на 64 KiB.
  `build_session_log_zip` — удобный buffered adapter для транспорта, которому нужен готовый `Vec<u8>`.
- Markdown использует latest real `session/title`, исходный append chronology (не replacement surface),
  human user messages, assistant text и tool call/result. Generated `<details>` защищён от authored closing
  tags вне code blocks; незакрытые fences закрываются; source path и tool payload получают fence длиннее
  authored backtick run.
- Download controller держит одну operation на Session, сохраняет initiation order, не открывает dismissed
  modal после settlement и HEAD-preflight выполняет до save. Copy controller держит одну глобальную operation,
  вызывает deferred clipboard adapter синхронно из `copy`, различает preparation/clipboard failure и подавляет
  поздние notices после dispose.

## Отличия от образца

1. В workspace нет общего Fetch/DOM/React seam (`packages/client/store` помечен `skip` в MAP.tsv).
   Поэтому крейт не регистрирует вымышленный HTTP plugin и не рендерит TSX. Вместо этого
   `prepare_session_log_export_response` возвращает root-only plan для streaming Host adapter,
   `session_markdown_response` возвращает status, headers и bytes,
   а browser-only side effects заданы object-safe `SessionLogDownloadAdapter` и
   `SessionMarkdownCopyAdapter`. Tauri UI может подписать эти состояния своим store.
2. TypeScript `ReadableStream` сам владеет byte high-water mark. Rust core принимает синхронный `Write`;
   backpressure принадлежит переданному bounded bridge. Крейт не удерживает generic file целиком, но convenience
   `build_session_log_zip` и `session_log_export_buffered_response` закономерно удерживают конечный архив.
   `collect_session_log_zip_entries` — явно collecting inspection helper вместо исходного public async generator;
   production Host path — `write_session_log_zip`. Ошибка/отмена прекращает writer до central
   directory, так что частичный поток не становится успешным ZIP. Writer уступает Tokio executor перед входом
   в sink и между 64-КиБ блоками. Сам `Write::write` обязан быть неблокирующим либо обслуживаться независимым
   OS thread; bounded bridge, который ждёт consumer-task на том же executor, запрещён. Если bridge блокирующий,
   Host должен вести writer на выделенном blocking thread (например, `spawn_blocking` + `Handle::block_on`).
3. Controllers запускают cloneable shared operation через Tokio немедленно, поэтому проигнорированный return
   соответствует `void controller.download/copy` в TS. Deferred clipboard `write_deferred` вызывается синхронно
   до возврата из `copy`, сохраняя browser activation seam. `subscribe()` возвращает watch receiver для modal,
   toast и menu rows; `dispose` abort-ит и дожидается всех принятых operations.
4. UI locale dictionaries из `src/client/locales.ts` остаются в Tauri UI layer вместе с компонентами; package
   metadata locale перенесены буквально. Машинные status/error variants и все пользовательские Host error texts
   экспортируются из Rust.

После ревью исправлены расхождения. Структурно неполная image reference больше не исчезает при
typed-deserialization: она доходит до attachment store с безопасным placeholder представлением, а исходный
TS ZIP path сохраняется для `attachments-missing.txt`. File reference без строкового `name`, напротив,
срывает экспорт до missing-manifest: это сохраняет исходный вызов `ref.name.replace(...)`, который бросает
TypeError. Rust выдаёт стабильное `file attachment name is not a string` вместо зависящего от V8 текста.
Markdown fence scanner теперь, как JS `(.*)$`, не открывает fence, если info-string пересекает U+2028 или
U+2029. Отдельные current-thread tests требуют yield до синхронного sink для `add_bytes` и `add_stream`.
Lineage проверен через реальный `SessionStore`/`SessionQueryEngine`: две ветки, вложенный descendant, preorder,
media только из descendants, обе archive paths и отсутствие сохранённого лога. После каждого descendant read
abort проверяется до `None`, поэтому отмена выигрывает у `subagent ... has no stored log`, как в TS.

## Не перенесено

- React DOM компонентов `Dialog`, `CopyToast`, context-menu/root-overlay wiring и feedback slot composition:
  в Rust workspace нет соответствующих React типов, а окно Tauri владеет presentation. Их controller-visible
  behavior (FIFO, dismiss, notices, status) перенесено и тестируется без DOM.
- Cordis loader/HMR registration как JavaScript module lifecycle. Kernel/Host adapter должен зарегистрировать
  экспортированные command/route handlers; новый параллельный connection service в этом крейте не вводился.

## Не проверено

- Реальный browser ClipboardItem/user-activation и DOM anchor: проверен момент синхронного вызова adapter,
  дальнейшая platform behavior принадлежит UI adapter.
- Реальный HTTP slow-consumer bridge: ZIP writer проверен на central/local records, CRC, empty entries и
  streamed multi-chunk file; byte queue и socket cancellation принадлежат connection adapter.
- Платформы кроме Linux; archive format и formatter платформенных веток не имеют.

Отложенные source-spec сценарии и причины:

- Полная propagation-матрица одного signal через root/lineage/attachments и abort на каждой ZIP-фазе не
  размножена. Descendant preorder, absent log, descendant-only media и abort-vs-NotFound проверены на реальных
  `SessionStore`/`SessionQueryEngine` и programmable persistence. Общий descendant двух веток публичной моделью
  не представим: `SessionHeader` содержит ровно один `parent_session`, а query corpus дедуплицирует записи по id;
  exporter `seen` остаётся защитой поверх этой гарантии и не имеет отдельного достижимого integration case.
- Полная route error-матрица (missing services, flush/read/stat failures, 400-before-500, 404) и controller
  dispose/late-success/ClipboardFailed combinations отложены как composition cases Host/UI adapter. В этом
  крейте проверены strict query, root preflight, persistence not-found/close/error, sanitized controller HTTP
  failure, eager deferred write, coalescing и dismissal; тексты/порядок ветвей сверены с source.
- Attachment transport matrix (не-unreadable `read_image`, file first/later chunk error, empty file) требует
  отдельного programmable `AttachmentStore` stream fixture. Здесь проверены image+multi-chunk file success,
  unreadable manifest и malformed reference; generic stream errors в production пробрасываются оператором `?`.
- Редкие carrier rows (`external/image-record`, `session/title-llm-request`, flat tool-role result) отложены:
  scanner использует закрытый allowlist first-party carriers и тест отвергает unknown rows. Полная event-table
  повторяет source switch без отдельной логики.
- Остаток markdown table (4-space/tab indents, backtick в info-string, blank edges, malformed content) частично
  объединён в текущие fold/fence/tool regressions; отдельные строки не размножены. Незакрытый fence и orphan
  tool result уже проверяются явно. U+2028 regression добавлен после ревью.

Transport notes: выбор повторного `sessionId` остаётся обязанностью Host query adapter (source export route берёт
последний, markdown route — первый). Ошибочные `ExportResponse` имеют `content_type: None`; Fetch adapter должен
выдать browser-default `text/plain;charset=UTF-8`, как `Response(string)` в source.

## Вопросы ведущему

- Для непосредственного включения в `apps/hoh` нужен только glue к существующему Tauri transport/UI. Чужой API
  ради этого крейта не менялся.

## Проверка

Команды запускаются offline с `CARGO_BUILD_JOBS=2`,
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x5`; тесты — только под внешним `timeout`.

- `cargo test -p hoh-session-log-export --offline`: 38 passed, 0 failed; doc-tests 0/0.
- `cargo clippy -p hoh-session-log-export --all-targets --no-deps --offline -- -D warnings` — чисто.
- `cargo fmt -p hoh-session-log-export -- --check` — чисто.
