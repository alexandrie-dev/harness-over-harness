# hoh-fs-observation-policy ← packages/fs/fs-observation-policy @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs | gate состояния и четыре fs/* listener, plugin без сервиса/config/inject |
| src/types.ts | src/types.rs | FsObservationActor и именованный Rust-тип вложенного agent |
| tests/policy.spec.ts | tests/policy.rs | все 24 объявления |
| locale/*.json | locale/*.json | восемь файлов скопированы без изменений |
| README.md, README.zh.md | — | прочитаны; собственного LICENSE нет |

Все 24 исходных тестовых случая сохранены. Дополнительный интеграционный тест вехи создаёт настоящий `hoh_tools::ToolExecution` через публичный pipeline и проводит `createIfAbsent` через настоящий `LocalFileSystem`: перезапись существующего непрочитанного файла возвращает точный `FS_NOT_OBSERVED`, исходный файл остаётся неизменным. Тот же тест подтверждает перенос наблюдения между двумя execution одного agent/session и отсутствие сильного удержания production-agent политикой.

## Отличия от образца
- `FsActor` текущего `hoh-fs` — `Arc<dyn Any>`, поэтому структурное TypeScript-сужение невозможно. Production actor явно downcast-ится к текущему `hoh_tools::ToolExecution`, откуда берётся `agent.session`; экспортируемый `FsObservationActor` покрывает прямых event callers и исходный публичный structural type. Из-за этого добавлена workspace-зависимость `hoh-tools`, которой нет в TypeScript imports.
- `FsObservationActor.agent.session` представлен `Option<FsObservationAgent>`/`Option<Session>`. `FsObservationAgent` — дополнительное публичное Rust-имя исходного анонимного вложенного объекта.
- Для production `ToolExecution` gate хранит только `Weak<dyn Agent>`, сравнивает полученные из живых agent `Session` по pointer identity (`PartialEq` у Session использует `Arc::ptr_eq`) и удаляет записи умерших agent при следующем обращении. Поэтому политика не удерживает agent и его журнал Session. Прямой compatibility actor хранит `Session` по той же pointer identity; это не позволяет разным объектам с одинаковым `SessionId` разделить разрешение, но удерживает только compatibility-сессию до disposal плагина.
- Асинхронные waterfall listeners возвращают `Some(intent)` и не вызывают `Next`, сохраняя single-slot first-wins. `FsObserved` остаётся синхронным listener; mutex не удерживается через await.
- Исходный `name` и default export представлены plugin-типом `FsObservationPolicy` с `NAME = "fs-observation-policy"`; `apply` экспортирован макросом plugin.

## Не перенесено
Нет исходных файлов, экспортов или тестовых объявлений, исключённых из переноса.

## Не проверено
Если production-agent уничтожен, пока тот же `Session` отдельно ещё жив, запись может быть удалена раньше JavaScript WeakMap: текущий `hoh-session` не раскрывает weak identity. Иных платформенных веток и файлового I/O в production-плагине нет.

## Вопросы ведущему
- Для точного WeakMap lifecycle нужен публичный weak identity/handle у `hoh-session::Session` либо owner abstraction в `hoh-fs::FsActor`. Чужие crates не менялись; текущая production-реализация не удерживает agent/session, освобождает все записи при disposal и функционально fail-closed.
- После замены `hoh-tools::Agent` настоящим `hoh_agent::Agent` downcast остаётся к стабильному `ToolExecution`;
  production-тест использует настоящий контракт агента и повторно проверяет owner extraction.

## Проверка
- `cargo test -p hoh-fs-observation-policy --offline`: 25 passed, 0 failed (24 исходных случая и 1 интеграционный regression test вехи).
- `cargo clippy -p hoh-fs-observation-policy --all-targets --offline -- -D warnings`: чисто.
- `cargo fmt -p hoh-fs-observation-policy --check`: чисто.

Все команды выполнялись с `CARGO_BUILD_JOBS=2`, заданным `CARGO_TARGET_DIR`, offline и под `timeout`.
