# hoh-command-compact ← packages/compaction/command-compact @ 39163dd3

## Перенесено

- `src/index.ts`: регистрация `/compact`, запрет аргументов, вызов
  backend-independent `Compaction::compact_now`, точные success/error тексты,
  `sourceEventSeq` и передача `commandId`.
- `tests/command-compact.spec.ts`: регистрация и disposal, успешный/no-history
  результат, все шесть `ManualCompactionError`, отмена, неожиданный отказ и
  quiescent teardown.
- Метаданные локалей сохранены в `locale/*.json`.

## Отличия Rust-представления

- `CommandInvocation::agent` переводится в `ManualCompactAgentContext` через
  публичные `Agent::session`, `AgentOptions` и `Agent::run_maintenance_erased`;
  shim не используется.
- JavaScript Promise начинает работу сразу и не отменяется при прекращении
  ожидания. Поэтому реальная операция запускается отдельной Tokio task, а
  handler ждёт её через oneshot. Это сохраняет исходную гарантию: abort может
  завершить ожидание команды, но backend close/flush продолжится, и teardown
  плагина дождётся его.
- Реестр команд снимается до drain благодаря обратному порядку `Undo`, как в
  исходном generator-effect.
- После начала disposal Rust-handler отклоняет новый прямой вызов ошибкой
  `command-compact plugin is disposed`. В JS снятие регистрации не закрывает
  уже сохранённую ссылку на handler; Rust-вариант намеренно безопаснее и не
  допускает операцию, которую teardown уже не сможет учесть.
- Тесты сверяют точные пары `command/run`/`command/done`, пустую surface/message
  проекцию, приоритет abort над поздней summary-ошибкой и двухфазный drain до
  завершения backend close/flush.

## Круг исправлений review 194456

- `expect_last_lifecycle` теперь возвращает exact `CommandId` из
  `command/run`. Success, no-history, usage rejection и все шесть
  `ManualCompactionError` сравнивают его с `CommandExecution::command_id`,
  как `command-compact.spec.ts:154,186,198,205,221`. Откат helper к старой
  сигнатуре делает эти регрессии некомпилируемыми. Production-дефекта
  не было: само review классифицирует пункт как «пробел тестового
  оракула» и прямо отмечает, что старый production-код проходит эту проверку.

## Не перенесено

- `loader-composition.spec.ts`: специфичная для JS Loader проверка исключена по
  §8 `PORTING.md`; Rust-композиция проверяется настоящими Kernel plugins.
- README не копируется в crate; наблюдаемый контракт и точные строки покрыты
  тестами.

## Вопросы ведущему

- Нет; правки API чужих крейтов не требуются.

## Проверка

- `cargo test --offline -p hoh-command-compact`: 6/0.
- `cargo clippy --offline -p hoh-command-compact --all-targets --no-deps -- -D warnings`: чисто.
- `cargo fmt --check --package hoh-command-compact`: чисто.
