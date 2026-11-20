# hoh-authorization ← packages/credentials/authorization @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/index.ts` | `src/lib.rs` | перенесён |
| `src/types.ts` | `src/lib.rs` | перенесён |
| `tests/authorization.spec.ts` | `tests/authorization.rs` | пересены registry, begin, commit, cancellation, interaction и containment |

## Отличия от образца

- TypeScript non-empty tuple `methods` выражен конструктором
  `AuthorizationFlow::new(first_method, other_methods, ...)`, поэтому
  пустой flow невозможен без runtime-проверки.
- JavaScript `AbortSignal` в prompt остаётся runtime-полем и не
  сериализуется; остальной JSON wire совпадает с `types.ts`.
- Отозванный некооперативный flow доигрывается в дочерней
  kernel-задаче. Задача перестаёт удерживать ключ сразу, как
  `Promise.race` в TS, но при shutdown также подчиняется lifetime владельца.
- Сам attempt также живёт в owner-managed kernel-задаче: после
  первого poll отмена Rust future ожидающего caller не отменяет
  eager-операцию, её cleanup и settlement, как и отброшенный JS
  `Promise`.
- Владение flow регистрируется внутри `Context::try_effect`, так что
  inactive owner не оставляет видимого flow между insertion и rollback.
- `AuthorizationSettled` — типизированное contained kernel-событие;
  sync/async ошибки слушателей логируются с исходным текстом и
  не меняют result attempt.
- Решение между отменой и началом `commit` линеаризовано состоянием
  `withdrawn`/`committing` под одним mutex. Завершение `flow.run` участвует в
  том же решении через `completion_admitted`: отмена, допущенная первой, всегда
  даёт `Cancelled`, даже до фактического `AbortController::abort`; завершение,
  допущенное первым, не может быть задним числом превращено поздней отменой в
  `Cancelled`. Сам `abort` и запись credential выполняются уже без mutex, как в
  одном синхронном тике TypeScript.
- Аналог проверки `instanceof AuthorizationDeclinedError` доступен как
  `AuthorizationDeclinedError::is`; он распознаёт именованную ошибку и после
  стирания до `BoxError`.
- В отличие от eager JavaScript `Promise`, Rust-слот `in_flight` резервируется
  при первом poll owner-managed задачи `begin`, а не в момент создания future.
  Наблюдаемое поведение после начала ожидания совпадает с образцом.

## Не перенесено

- ESM default export и npm locale/README assets не имеют runtime-аналога в
  Rust-крейте; в locale JSON нет данных, используемых seam.

## Вопросы ведущему

- Нет; правки чужих API не требуются.
