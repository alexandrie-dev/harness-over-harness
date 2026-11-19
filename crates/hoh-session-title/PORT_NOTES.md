# hoh-session-title ← packages/session/session-title @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs | plugin, service, provider lifecycle, fallback, rename/refresh, две проекции |
| src/normalize.ts | src/normalize.rs | очистка terminal/control-последовательностей, JS whitespace, word/UTF-8 caps |
| src/types.ts | src/types.rs | события, snapshot, source/model/provider и bounded input state |
| src/client.ts | service key `SessionTitle` в src/lib.rs | типизированный доступ `sessionTitle` представлен kernel service key |
| tests/*.spec.ts | tests/*.rs | все 54 объявления, все табличные варианты и review-регрессии: 68 Rust-тестов |
| locale/*.json | locale/*.json | восемь файлов скопированы без изменений |
| README.md, README.zh.md | — | прочитаны; собственного LICENSE нет |

Сохранена log-backed модель: `session/title` остаётся единственным durable источником истины, `title` — wire-проекция версии 1 с явным JSON `null`, а strict host-only `titleInput` версии 3 хранит только первый prompt, количество и последний seq. Перенесены немедленный fallback, pin через rename, явный refresh, режимы `first-prompt`/`all-prompts`, watermark, route gating, supersession/cancellation, drain регистрации и plugin-unload, а также fail-closed проверка результатов провайдера.

## Отличия от образца

- TypeScript class одновременно является plugin и service. В Rust `SessionTitlePlugin` владеет lifecycle, а клонируемый `SessionTitleService` публикуется ключом `SessionTitle = "sessionTitle"`; `Plugin::NAME` сохранён как `SessionTitleService`.
- `register` принимает `&Context` владельца и возвращает kernel `Disposer`: так Rust-API явно связывает регистрацию с fiber lifecycle вместо неявного Cordis context. Provider object заменён объектобезопасным `SessionTitleProvider`, а promise — `BoxFuture`.
- Публичный `Config` использует `f64`, чтобы runtime-валидация по-прежнему отличала дробные значения; после проверки значения переводятся во внутренние `usize`. Нулевые, дробные, бесконечные и несовместимые byte caps отвергаются до публикации сервиса.
- JavaScript `AbortSignal` представлен текущими `hoh-timeout::{AbortSignal, AbortController}`. Сигнал активного вызова объединяет caller cancellation, lifetime plugin и локальное supersession.
- Динамически malformed provider objects и значения неверных JS-типов в Rust не создаются: форму объекта, строковый title, seq и model обеспечивает типовая система. Runtime-тесты покрывают представимые ошибки: пустой id/title/message list, чужие или неупорядоченные seq и пустую model identity.
- Исходный JSONL round-trip использует workspace-крейты `hoh-session-persistence` и `hoh-session-persistence-jsonl` как dev-dependencies: writer создаёт handle, flush/close и полностью выключается, reader монтирует новый backend на том же каталоге и читает четыре исходных события.
- Исходные `it.each` развернуты в отдельные Rust-тесты. Вариант activation reject создаёт реальный конфликт второй проекции: первая успевает зарегистрироваться, apply откатывается, service и `title` исчезают. Оба варианта «registration pending» заменены late mount по уже записанному журналу и projection rebuild; отдельного hook, способного удержать чужой `SessionProjectionRegistry::register` в середине await, workspace API не предоставляет.
- Для unload проверены reentrant inactive refresh, active/pending cancellation, replacement во время drain и отсутствие warning после начала disposal. Точную Vitest-микропаузу «fallback spawn уже поставлен в очередь, но ещё не начал публикацию» Rust scheduler API не раскрывает; тест предупреждений начинает disposal до доставки eligible event, а production-ветка дополнительно проверяет `service_active()` непосредственно перед `warn`.

## Круг исправлений review 120952

- `clean_title_text` теперь выполняет OSC, CSI, двухбайтовый ESC, control и directional удаления отдельными последовательными проходами, точно как цепочка `.replace()` в `normalize.ts`. Регрессии `ESC ESC]...` и `ESC ESC[...` до исправления оставляли видимые хвосты `[31mA`/`[`; добавлена также проверка границ JS whitespace (`FEFF`, `2028`, `180E`).
- Explicit `refresh` создаёт и публикует `ActiveWork` в одной секции `state` lock, повторно проверяя текущую регистрацию и live Session. `ensure_fallback` также повторно проверяет live Session под lock до `state_for`. Поэтому `SessionDisposed` не может удалить `SessionWork` перед публикацией active/fallback и не оставляет заново созданную запись для уже detached Session.
- Automatic `start_pending` проверяет registration/closing/revision, создаёт active, начинает registration call-count и записывает active в той же секции lock; explicit refresh делает то же. `close_registration` выставляет `closing` под тем же lock и либо предотвращает activation, либо видит уже опубликованный active, abort-ит его и ждёт уже учтённый call-count. Поэтому disposer не может завершить drain в щели между публикацией active и началом учёта вызова.
- Provider registration переведена на `try_effect`: duplicate-check и publication атомарны под `state` lock. Синхронный close-undo исполняется в момент начала disposal перед асинхронным drain, поэтому отложенный provider не входит после вызова disposer. Replacement по-прежнему разрешён после выставления `closing`, а cleanup старой регистрации не удаляет новую.
- `register` проверяет service lifecycle до резервирования effect и повторно непосредственно перед публикацией; stale service после unload и context-disposal во время setup не могут оставить закрытую регистрацию в state.
- `assert_current` сначала проверяет `service_active`, как TS `assertServiceActive()`. Отдельный unit-тест моделирует inactive owner при ещё не отменённом lifetime signal и требует точную ошибку `session-title service disposed`.
- `start_provider` использует Drop-guard для очистки exact active id. Drop/abort ожидающего `refresh` больше не оставляет stale active, который позднее ложным образом abort-ился при rename.
- Scheduling читает только `header.isSeeded` и `header.origin`, как `index.ts:521-524`, вместо десериализации всего lossless header. Регрессия использует допустимый для `hoh-session` integral JSON `delegationDepth: 1.0`, который не десериализуется в Rust `u64`, и проверяет, что first-prompt provider всё равно запускается.
- Дополнены представимые provider-result варианты: duplicate/reordered `messageSeqs`, пустой `model` при непустом provider и точные тексты ошибок. Для `rename`/`refresh` проверяются полные строки `visible characters` и `session "…" is not live in this store`.
- Host-only `titleInput` checkpoint теперь, как strict zod schema, требует явного присутствия всех трёх ключей `first`, `count`, `lastSeq`; раньше serde принимал отсутствующий optional key как `None`.
- Добавлены multi-thread stress-проверка refresh против Session disposal и детерминированные тесты cancel-safety, concurrent double registration и disposal-before-deferred-entry. Красный прогон старого кода воспроизвёл escape-хвост, stale active, две принятые регистрации, вход закрытого provider и принятие checkpoint без обязательного ключа.

## Круг исправлений review 124214

- Устранено повторное создание `SessionWork` после detach в окне между предварительной проверкой и захватом title-state lock. Общий `state_for_live` повторно сверяет exact Session с текущей записью `SessionStore` под тем же lock непосредственно перед созданием work; его используют и explicit `rename`, и automatic `on_user_message`.
- Детерминированная регрессия ставит приватный test-only barrier сразу после первой live-проверки `rename`, отсоединяет сессию и затем продолжает операцию. Старый путь создавал осиротевший work и доходил до append в detached Session; исправленный путь возвращает точную live-ошибку, не пишет title и оставляет `state.work` пустым.

## Инженерные заметки

- Rust отвергает числовые caps выше `usize::MAX`, тогда как JS `Number.isInteger` допускает практически неприменимые значения вроде `1e30`; выделить такой объём всё равно невозможно, поэтому более строгая граница сохранена.
- `defer_fallback` и `start_pending` могут вернуть ошибку `ctx.spawn`, если unload начался сразу после `service_active()`; contained listener изолирует такую ошибку, а опубликованное состояние не повреждается.
- Несколько abort-вызовов выполняются под state mutex. Встроенные consumers сигнала не входят обратно в `SessionTitleService`; reentrant пользовательский callback теоретически мог бы взаимоблокироваться, но такого callback API этот крейт не публикует.
- Warning использует литерал `Error: {failure}`. Для обычных Rust errors это соответствует текущим логам; экзотическое JS-различие `Error.name` в Rust не представимо.

## Не перенесено

Нет исключённых production-экспортов или тестовых объявлений. TypeScript module augmentation из `client.ts` не требует отдельного Rust-модуля: его назначение выполняет типизированный kernel service key.

## Не проверено

Платформенных веток и сетевого/файлового I/O в production-пакете нет. JSONL persistence проверена в режиме `compression: none`; Zstandard не относится к контракту исходного пакета. Точная пауза между двумя projection registration из Vitest заменена проверками late-mount rebuild, rollback второго registration и disposal до публикации: менять чужой registry ради тестового barrier не потребовалось.

`fold_session_title` возвращает `None` для вручную созданного malformed `session/title`, тогда как типизированный TS-код предполагает корректный `SessionEventMap`. Это сохранено как fail-closed Rust boundary: production append и projection validation создают корректные события, а безопасное отсутствие snapshot предпочтительнее panic/частично принятого corrupted состояния.

## Вопросы ведущему

- Чужие API менять не потребовалось, SHIM не добавлялся.

## Проверка

- `cargo test -p hoh-session-title --offline`: 68 passed, 0 failed.
- `cargo clippy -p hoh-session-title --all-targets --no-deps --offline -- -D warnings`: чисто.
- `cargo fmt -p hoh-session-title -- --check`: чисто.

Все команды выполнялись с `CARGO_BUILD_JOBS=2`, `CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x5`, offline и под внешним `timeout`.
