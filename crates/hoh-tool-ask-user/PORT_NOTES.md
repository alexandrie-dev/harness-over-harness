# hoh-tool-ask-user ← packages/interaction/tool-ask-user

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | legacy-плагин, схема, запрос и JSON-ответ перенесены |
| `src/timed.ts` | `src/lib.rs` | timed-схема, валидация, pending/answer и indefinite wait перенесены |
| `locale/*.json` | `locale/*.json` | 8 файлов скопированы без изменений |
| `tests/tool-ask-user.spec.ts` | `tests/tool_ask_user.rs` | 21 / 3 случаев образца; проверки образца и Rust-границ собраны в 27 тестов |
| `README.md`, `README.zh.md`, `README.i18n.yaml`, `package.json` | эти заметки и `Cargo.toml` | прочитаны, публичный контракт и состав пакета учтены |

LICENSE в пакете образца отсутствует.

## Отличия от образца

1. Rust-плагин называется `ToolAskUser`; сервисы передаются в публичные `apply` и
   `register_timed_ask_user` явно, поскольку Rust не использует Cordis Proxy-свойства контекста.
2. `SKIPPED_ANSWER_GUIDANCE` использует Rust-стиль имени константы, сохраняя текст образца буквально.
3. SHIM(hoh-tools): `ToolExecution.agent` пока содержит только `scope_key` и `Session`, тогда как
   `hoh-user-questions` принимает полный `hoh-agent::Agent`. `src/shim.rs` находит тот же живой объект в
   сервисе `Agents` по id Session, как соседние принятые tool-крейты. После унификации API `hoh-tools`
   адаптер заменяется прямой передачей `exec.agent`. Если объект не зарегистрирован, перенос возвращает
   точный наблюдаемый контракт самого `hoh-user-questions`: `UserQuestionError/CALLER_NOT_LIVE` с текстом
   `human interaction requires the exact live calling agent when an agent is supplied`. Провайдер в обоих
   вариантах не вызывается; прежнего отдельного `ToolContextUnavailableError` больше нет. Это остаётся
   API-ограничением относительно TypeScript: урезанный незарегистрированный `hoh-tools::Agent` нельзя
   безопасно выдать за полный `hoh-agent::Agent` и передать провайдеру. Временный фальшивый полный Agent
   обошёл бы проверку живого root и защиту delegated caller, поэтому такой адаптер здесь не создаётся;
   полное устранение требует унификации чужого `hoh-tools` API.
4. Схема timed-инструмента по-прежнему показывает модели `timeout: integer`, а исполнение проверяет это
   поле в `hoh-tool-ask-user`. Это сохраняет JS `Number.isInteger`: JSON-числа `3`, `3.0` и `1e2`
   эквивалентны, а дробные и выходящие за диапазон значения получают один исходный timeout-текст.
   Встроенный валидатор `hoh-tools` намеренно не ставится перед этой проверкой: его безопасно-целая граница
   иначе меняла бы ошибку для `1e21` до вызова перенесённой логики. Только отсутствующий ключ выбирает
   configured default; явные `null`, строки и boolean отклоняются как неверный timeout до вызова провайдера.

## Не перенесено

- 3 из 24 случаев `tool-ask-user.spec.ts` остаются интеграционными: pending после принятого UI-запроса,
  pending у непринятого запроса по deadline и структурированный `DELEGATED_CALLER` для subagent. Ответ
  внутри положительного timed wait и resumed root Agent теперь проверяются локальной минимальной заглушкой,
  зарегистрированной в настоящем `Agents`, без частного testkit.

Итого случаев образца: 21 перенесён, 3 не перенесены. Дополнительные проверки Rust-схемы Config не считаются
случаем образца.

## Не проверено

- Полный Tauri/UI состав и поздний ответ на timed pending через настоящий agent loop: принадлежат составам
  `hoh-user-questions`, клиента и отсутствующего пока `hoh-tool-testkit`, а не этому leaf-крейту.

## Вопросы ведущему

1. После замены временного `hoh-tools::Agent` на `hoh-agent::Agent` удалить `src/shim.rs` и зависимости
   `hoh-agent`; правка чужого API из этого задания не требуется.
