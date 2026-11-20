# hoh-jobs-local ← packages/jobs/jobs-local @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | `Config`, `LocalJobRegistry` (`JobRegistryApi` + `JobEvents`), плагин `LocalJobRegistryPlugin`; запуск, допуск, чтение, kill, wait, remove, очистка по владельцу, выгрузка сервиса, запись `job/registered`/`job/settled` в сессию |
| `src/events.ts` | `src/events.rs` | `JobLayer`, `Subscription`, `JobEventHub` |
| `src/pump.ts` | `src/pump.rs` | `PumpSink`, `PumpHandle`, `start_pump`, `json_number` |
| `src/ring.ts` | `src/ring.rs` | `OutputRing` |
| `tests/ring.spec.ts` | `tests/ring.rs` | 5 из 5 |
| `tests/pump.spec.ts` | `tests/pump.rs` | 6 из 6 (два с поддельными таймерами идут на паузе часов tokio; один заменён, см. ниже) |
| `tests/events.spec.ts` | `tests/events.rs` | 5 из 5 + 1 (паника слушателя) |
| `tests/jobs.spec.ts` | `tests/jobs.rs` | 85 случаев; `tests/common/mod.rs` — стенд (ядро, реестр агентов, заглушка `Agent`, управляемый производитель) |
| `tests/loader-composition.spec.ts` | последний случай `tests/jobs.rs` | см. «Не перенесено» |
| `locale/*.json` | `locale/*.json` | копия без изменений |
| `README*.md`, `package.json` | эти заметки и `Cargo.toml` | прочитаны |

Имя плагина — `LocalJobRegistry`. Крейт предоставляет сервис `jobs` через `hoh_jobs::provide_jobs`
(архивные слушатели ставятся там же).

## Отличия от образца

- Настройки — `u64` со схемой `minimum 1 / maximum 2^53-1`. Дробные, отрицательные, `null` и строки отклоняет
  схема ядра; `NaN`/`Infinity` в JSON невыразимы.
- Наблюдатели и задачи слежения за заданием — обычный `tokio::spawn`, а не fiber плагина: задание живёт
  дольше fiber-а производителя, а выгрузка сервиса (`dispose_all`) сама дожидается `settled` каждого.
- `wait` регистрирует ожидающего синхронно при вызове (как префикс `async`-метода до первого `await`); тесты
  вызывают `wait`, потом завершают/прерывают и только затем ждут будущее. Освобождение ожидающих идёт до
  события `settled`; долг «вернуть проекцию» вычисляется при завершении по `signal.aborted()`.
- `JobEvents::subscribe` берёт `&Context` явно (подписка — эффект этого контекста).
- Слушатель, вернувший `Err`, и слушатель, запаниковавший, изолируются одинаково; в журнал идёт
  `jobs: event listener threw on <тип>: Error: <текст>`.
- Отказ `done` (Err) — нарушение контракта производителя: задание `failed`, `detail` = `Error: <текст>`,
  в журнал `producer contract violation`.
- Скрытые точки подмены для тестов: `LocalJobRegistry::with_uuid_source` (в образце подменяется
  `node:crypto`) и `wait_resolver_count` (в образце — `service.store`). Оба `#[doc(hidden)]`.
- `Config`-ключи прежние в camelCase; `pump_poll_ms` в `start_pump` — `f64`, как в образце проверяется на
  конечность и положительность.
- Окружение сессии: запись жизненного цикла (`job/registered`, `job/settled`) — `session.append`; сбой записи
  не мешает выполнению и освобождению ожидающих (предупреждение `failed to record lifecycle`).

## Не перенесено

- `jobs.spec` «rejects invalid maxConcurrentJobsPerOwner: 1.5 / NaN / Infinity»: проверено `0`, `-1`, `1.5`,
  `2^53`, `null`, строку; `NaN`/`Infinity` в JSON не существуют.
- `jobs.spec` с `getEffects()` по метке `jobs.ownerCleanup()` (три случая): ядро не показывает список
  эффектов. Заменены поведением: одна очистка снимает все задания владельца ровно по одному `cancel`; очистка
  висит на области агента, а не fiber-е сервиса (сервис после неё жив и пригоден); после выгрузки сервиса
  поздняя выгрузка агента ничего не отменяет и не объявляет.
- `jobs.spec` «publishes original timestamps…»: подмена `Date.now` невозможна; вместо точных `1000`/`2500`
  записи сверяются с `started_at`/`finished_at` из проекций реестра.
- `jobs.spec` «reports append failures…»: `spyOn(session.append)` невозможен; отказ даёт сессия, которой владеет
  `SessionStore`, но к нему не подключённая (`prepare` без `enter`), — `append` у неё всегда возвращает ошибку.
- `jobs.spec` «unregisters …» вторым вызовом того же `detach()`: `Disposer` одноразовый (потребляется).
- `jobs.spec` «preserves the SessionId brand» (`expectTypeOf`): проверка типов TS, в Rust тип `JobView::owner`
  — `Option<SessionId>` по определению.
- `pump.spec` «subscribes to the settlement once…»: счёт `then`-подписок на `PromiseLike` невыразим;
  заменён проверкой «одно чтение за круг опроса и один финальный» на 10 000 кругов при паузе часов.
- `pump.spec` «rejected settlement»: у `until` в Rust нет значения, отказ производителя и успех для насоса
  одинаковы — проверяется добор последних байт.
- `loader-composition.spec`: монтаж через реальный `Loader` принадлежит `hoh-loader`; здесь проверено, что
  строка с `maxConcurrentJobsPerOwner: 1` доходит до провайдера и даёт отказ `(limit: 1)`.
- Причины прерывания не-`Error` (`abort('x')`): причина `AbortSignal` в Rust всегда ошибка.

## Не проверено

- Платформенное поведение при реальных нагрузках и долгие тайминги: тесты с настоящими паузами (`pumpPollMs`
  1–5 мс, ожидания 20–40 мс) зависят от планировщика; три повторных прогона подряд зелёные.

## Вопросы ведущему

- Подтвердить `tokio::spawn` для задач слежения (а не fiber-ы), см. отличия.
- `hoh_kernel` не показывает список эффектов fiber-а; три случая `getEffects()` заменены поведением. Если нужен
  счётчик эффектов, ядру нужен публичный `Fiber::effect_labels()`.
