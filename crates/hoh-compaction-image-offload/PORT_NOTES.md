# hoh-compaction-image-offload ← packages/compaction/compaction-image-offload @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` (`name`, `inject`, `apply`) | `src/lib.rs`: `ImageOffloadPlugin` (`NAME = "compaction-image-offload"`, `Deps` = `agents`, `sessions`, `NoConfig`) | перенесён |
| `src/projection.ts` (`ImageOffloadTarget`, `SessionEventMap['image/offload']`, `imageOffloadProjection`) | `src/projection.rs`: `ImageOffloadTarget`, `ImageOffloadEvent` (константа `TYPE`), `ImageOffloadProjection`, `image_offload_projection()` | перенесён |
| `src/project-message.ts` (`offloadMessageImages`) | `src/project_message.rs`: `offload_message_images` | перенесён |
| `src/image-offload.ts` (`offloadOldestImages`) | `src/image_offload.rs`: `offload_oldest_images` | перенесён |
| `locale/*.json` (8 файлов) | `locale/*.json` | скопированы, `cmp` без различий |
| `tests/projection.spec.ts` (5 `it`, один — `it.each` из 17 строк) | `tests/projection.rs`: 21 тест | перенесён полностью, кроме `Object.isFrozen` |
| `tests/image-offload.spec.ts` (8 + 9 `it`) | `tests/image_offload.rs` | перенесено 11 из 17 (+3 добавленных), см. «Не перенесено» |

Точка входа `./projection` из `exports` — публичный модуль `projection`. Чистая проекция не требует
сервисов: её берёт `hoh-session-format-catalog` (`message_projections.rs`).

## Отличия от образца
- **Данные сообщений — `serde_json::Value`.** Сессия `hoh-session` хранит сообщения как `Arc<Value>`, поэтому
  проекция работает над JSON; `deepFreeze` (`dsh-util-values`) не нужен — `Arc<Value>` неизменяем по типу.
  Зависимость `hoh-util-values` из скелета убрана как неиспользуемая. Ключ `offloaded` ставится как в
  `{ ...block, offloaded: true }`: существующий ключ остаётся на месте, новый идёт последним.
- **`image_offload_projection()`** возвращает один общий `Arc<dyn SessionMessageProjection>` (аналог константы
  `imageOffloadProjection`): Session сравнивает определения проекций по идентичности. `ImageOffloadProjection`
  (структура) и `ImageOffloadEvent` публичны как носители типа и данных события; в образце это интерфейс
  `SessionEventMap['image/offload']` и константа.
- **`offload_oldest_images(session, &[SessionSeq], count: u64) -> Result<bool, BoxError>`**: `count` — `u64`
  (`failure.offload_images` в `hoh-llm` — `Option<u64>`). `eventAt(seq)!` и `deriveEventMessage(event)!` образца
  при отсутствии значения бросают `TypeError`; здесь это `SessionError::TypeError` с текстами V8
  («Cannot read properties of undefined (reading 'type')», «… null (reading 'content')»). Недостижимо при
  корректной сессии. Блок `null` в содержимом даёт `TypeError` «… null (reading 'type')», как в образце.
- **Ошибки проекции** — простой `Error` образца: тип `ImageOffloadError` (только сообщение), тексты
  `image/offload: …` побайтово. Содержимое не-массив (недостижимо после допуска сообщения сессией) —
  `image/offload: message content must be an array`; в образце здесь был бы `TypeError` JavaScript.
- **Слушатель `agent/request-error`**: `signal.aborted()` проверяется вместе с кодом, до обращения к сессии;
  порядок проверок образца сохранён (все они чистые). Ошибка чтения поверхности/записи события уходит
  в цепочку слушателей как `Err`, как брошенное исключение в образце.
- **Слушатель `compaction/summary-error`**: `error instanceof LlmError` → `HarnessError::of(..).is_named("LlmError")`;
  `error.failure.offloadImages` → `detail::<LlmFailure>().offload_images`. `signal?.throwIfAborted()` → `Err(reason)`.
- **Событие**: событие `compaction/summary-error` объявлено в пакете `compaction` (`todo`) —
  временная копия в `src/shim.rs` (см. шимы).

## Не перенесено
- `tests/image-offload.spec.ts`, блок `summary image offload`, 6 случаев из 8: «recovers real summary image
  preparation…», «offloads only the selected summary span…», «preserves omission when a subsequent summary
  failure is terminal», «does not record a decision after cancellation during a failed summary», «does not
  retry when cancellation follows a durable omission», «rejects a concurrently changed selection…». Они вызывают
  `BasicCompactionEngine.compactNow/compactRegion`, `TokenMeter` и `inlineImages` из `llm-deepseek` —
  пакеты `compaction-basic`, `token-meter`, `llm-deepseek` ещё `todo` в `MAP.tsv`. Ждут этих пакетов.
  Вместо них добавлены 3 теста слушателя через само событие (`summary_error_*`): выбранный промежуток и
  исчерпание, отмена до записи решения, ошибка другого вида. Два оставшихся случая блока («delegates unhandled
  summary errors», 2 строки `it.each`) перенесены на агенте без `TokenMeter` и движка (на ассерты они не влияют).
- `Object.isFrozen(...)` в первом тесте `projection.spec.ts` (3 проверки): заморозка объектов — понятие
  JavaScript; в Rust сообщение — `Arc<Value>`, изменить его нельзя по типу.
- Типы TypeScript (`declare module … SessionEventMap`) — объявление слияния; заменено `ImageOffloadEvent`.
- `README.md`, `README.zh.md`, `README.i18n.yaml`, `package.json`, `tsconfig.json` — документация и сборка npm.

## Экспорты, которыми не пользуются перенесённые пакеты
Единственный потребитель вне пакета — `session-format-catalog` (`imageOffloadProjection`). `ImageOffloadTarget`,
`offloadMessageImages`, `offloadOldestImages` другие пакеты не импортируют; перенесены целиком.

## Не проверено
- Ветки платформ в пакете отсутствуют.
- Параллельная смена выбора компактации между решением и записью (тест «rejects a concurrently changed
  selection») не покрыта: ждёт `compaction-basic`.

## Вопросы ведущему
- **SHIM(hoh-compaction)** `src/shim.rs`: `CompactionSummaryError` — событие `compaction/summary-error`
  (waterfall, результат `bool`; поля `session`, `source_event_seqs`, `error: SharedError`, `signal`).
  Заменить событием из `hoh-compaction`, когда пакет будет принят; тогда же убрать модуль `shim` и
  переключить `ImageOffloadPlugin` и тесты на него.
- **`hoh-session-format-catalog`** может заменить свой SHIM(hoh-compaction-image-offload) на
  `hoh_compaction_image_offload::image_offload_projection()` (общий `Arc`, идентичность сохраняется).
- **dev-зависимости** `hoh-agent-loop`, `hoh-system-prompt`, `hoh-tools`, `futures-util`, `tokio`: тесты
  `image_offload.rs` собирают контур агента по образцу `crates/hoh-agent-loop/tests/common` (его нельзя
  импортировать). `hoh-agent-loop` и `hoh-agent` на момент написания не приняты; при смене их API тесты
  `image_offload.rs` придётся поправить.
- Образец не обращается к `hoh-util-values`/`deepFreeze` в рабочем коде, кроме замораживания результата;
  см. «Отличия».
- **Состояние тестов `image_offload.rs` (19.11)**: 9 из 14 проходят; 5 падают на проверке содержимого запроса
  (`offloaded_names(request)` пуст): запрос, который `hoh-agent-loop` отдаёт адаптеру, содержит только
  system-сообщение — сообщения пользователя в нём нет. Решение сессии `image/offload` записано, повтор
  запроса выполнен, `derive_messages()` отдаёт `offloaded: true` (проверено отладочным выводом). Те же симптомы
  у собственных тестов `hoh-agent-loop` (`cargo test -p hoh-agent-loop --test interception`: 8 падают, среди них
  «enter_with_content_rewrites_the_prompt_before_it_is_recorded» с `request_text(..).contains("REWRITTEN")`).
  Это дефект загрузки/сборки запроса в `hoh-agent-loop`, а не этого крейта. Падающие: `logs_one_exact_image_…`,
  `counts_the_adapter_prefix_…`, `offloads_a_tool_result_image_…`, `advances_across_consecutive_failures_…`,
  `cancelling_after_an_image_omission_…`. Проверки не ослаблялись.
- **`#[ignore]` (решение ведущего, 19.11)**: эти 5 тестов помечены
  `#[ignore = "ждёт hoh-agent-loop: запрос без сообщения пользователя (s6 исправляет)"]`; тела и проверки
  не изменены. Снять `ignore` после исправления `hoh-agent-loop` (по просьбе ведущего) и прогнать
  `cargo test -p hoh-compaction-image-offload -- --include-ignored`.
