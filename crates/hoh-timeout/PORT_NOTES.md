# hoh-timeout ← packages/util/timeout @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs | перенесён |
| tests/timeout.spec.ts | tests/timeout.rs | все 27 it, плюс проверки отмены во время demand, eager-завершения и Number-границ |
| locale/ | — | отсутствует в образце |

## Отличия от образца
- `clamp_timeout` принимает Option для hint и имени (None означает timeoutMs). Исключения представлены Result с публичным enum TimeoutError; TimeoutReason реализует Error/Display, имя класса — имя Rust-типа.
- Миллисекунды — f64, поскольку источник принимает дробные задержки. Планирование повторяет setTimeout: отбрасывает дробь и использует минимум 1 ms; reason сохраняет исходное значение. MAX_TIMER_DELAY_MS — u64.
- `timeout_of` — чистая функция над Option<&dyn Any + Send + Sync> и кодом; вызывающий передаёт reason из носителя. Нет зависимости от runtime в этой сигнатуре.
- Публичный AbortSignal — временная замена AbortSignal/AbortController: new/default, clone, abort, aborted, reason, cancelled. reason хранится как Arc<dyn Any + Send + Sync>; первая причина сохраняется вместе с идентичностью Arc. Fusion синхронно передаёт отмену; уведомления ожидателей используют tokio::sync::watch. Связи с получателями слабые, глобального реестра нет.
- Deadline и IdleWatchdog используют tokio::time и управляемые JoinHandle; dispose и Drop очищают таймер. Нужен активный Tokio runtime для положительной задержки.
- IdleWatchdog::next принимает Future одного запроса итератора вместо AsyncIterator. Future и результат должны быть Send + 'static, потому что запрос исполняется в задаче Tokio. Проверка concurrent demand и запуск таймера происходят при вызове, до polling результата. Завершение demand снимает таймер даже если результат ещё не прочитан, как у eager Promise. При таймауте demand продолжает выполняться; владелец транспорта обязан освободить его через signal, как в образце.
- Публичный IdleWatchdogError различает Timeout(Arc<TimeoutReason>), State(TimeoutError), Task(tokio::task::JoinError). Последний вариант представляет только Rust-специфичные сбой задачи/остановку runtime. Результат demand не преобразуется и может быть Result. Drop возвращённого future не отменяет выполняющийся запрос, как игнорирование Promise в источнике.

## Не перенесено
Нет исключённых исходных файлов или тестов. README прочитан; npm-документация не копировалась.

## Не проверено
Платформенных веток нет. Одновременная отмена с разных OS-потоков (JavaScript однопоточен) не имеет соответствующего сценария образца.

## Вопросы ведущему
- Согласовать AbortSignal и носитель причины с типом отмены ядра, когда будет написан docs/kernel.md.
- Согласовать Deadline/IdleWatchdog: tokio::time::sleep_until, tokio::spawn, JoinHandle::abort, watch-уведомления и dispose/Drop с будущим владением таймерами ядра.
- Согласовать API next(Future + Send + 'static), задачи для eager-исполнения, вариант ошибки JoinError и освобождение demand, пережившего timeout, с будущим API потоков ядра. dispose снимает таймер, но не отменяет outstanding demand, в точности как источник.
- В исходнике next после первого собственного timeout слушает только новое событие abort: уже отменённый watchdog второй раз не отклоняет demand таймером. Это поведение сохранено, не исправлялось.
