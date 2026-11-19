# hoh-command-feedback ← packages/feedback/command-feedback @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/index.ts` | `src/lib.rs` | перенесён |
| `src/types.ts` | `src/types.rs` | перенесён |
| `tests/command-feedback.spec.ts` | `tests/command_feedback.rs` | runtime-сценарии перенесены, родственные случаи сгруппированы |
| `tests/loader-composition.spec.ts` | `tests/command_feedback.rs` (`Harness`) | композиция проверяется настоящими Kernel plugins |
| `locale/*.json` | `locale/*.json` | восемь файлов перенесены побайтно |

Из 16 исходных `it` перенесены 15 runtime-сценариев; Typert reflection case
исключён. Они сгруппированы в 6 обычных Rust-тестах. Два ignored helper-теста
запускаются этими тестами в отдельных процессах с изолированным `DSH_HOME` и
проверяют точный anonymous id без изменения окружения общего test process.

## Отличия от образца

- Typert transport не переносится по §8. Обычный публичный метод
  `SessionFeedbackService::record` помечен `Remote: sessionFeedback.record`, а
  сервис сохраняет точное имя `sessionFeedback`.
- `recordFeedback(): void` представлен как `record_feedback() -> Result<(),
  BoxError>`, потому что `Session::append` в Rust явно возвращает возможный
  отказ. Порядок и payload append не изменены.
- Module-global memo anonymous id заменён одним `AnonymousUserIdStore`, которым
  владеет root-контекст Kernel. Он переживает выгрузку/повторную активацию
  `CommandFeedback` и разделяется всеми экземплярами в этом процессе Kernel,
  как требует API владельца, без запрещённого глобального mutable static.
- Блокирующее чтение/создание id выполняется через `spawn_blocking`;
  `feedback/record` добавляется синхронно до первого await, поэтому порядок
  конкурентных dispatch сохраняется. На первом identity I/O появляется
  отсутствующее в однопоточном JS окно: поздний abort может завершить команду
  ошибкой уже после принятого feedback event, пока blocking task закончит
  создание id. Устранение окна потребовало бы запрещённого блокирующего файлового
  I/O на Tokio worker либо нового async/transaction API чужого
  `hoh-anonymous-user-id`; чужой крейт не изменялся.
- `SessionFeedbackService` одновременно является дочерним Plugin type; маркеры
  `SessionFeedback`, `Deps`, `SessionFeedbackDeps` и literal-типы результата —
  Rust-представление сервисной зависимости и строгого wire-union.

## Не перенесено

- Проверка Typert Gateway namespace/reflection method исключена по §8. Namespace
  и имя метода сохранены строкой Remote и wire-тестом сервиса.
- Проверки `default` export и `Loader.unwrapExports` относятся к npm/ES module
  механизму. YAML Loader source-test заменён прямой Kernel-композицией тех же
  runtime plugins без динамического импорта JavaScript.

## Не проверено

- Нет. `cargo test --offline -p hoh-command-feedback` — 6/0 (два child helper
  вызваны родительским тестом); `cargo clippy --offline -p
  hoh-command-feedback --all-targets --no-deps -- -D warnings` и `cargo fmt
  --check --package hoh-command-feedback` чисты.

## Вопросы ведущему

- Нет; правки API чужих крейтов и shim не требуются.
