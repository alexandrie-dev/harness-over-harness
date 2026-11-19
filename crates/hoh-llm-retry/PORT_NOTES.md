# hoh-llm-retry ← packages/llm/llm-retry @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/brand.ts` | `src/lib.rs` | `RetryId` — прозрачный строковый бренд |
| `src/types.ts` | `src/lib.rs` | `LlmRetryEventData`, `LlmRetryStartedEventData`; порядок ключей wire JSON задан вручную |
| `src/index.ts` | `src/lib.rs` | плагин, проекция `llmRetry`, waterfall recovery, backoff, отмена и дренирование перенесены |
| `locale/*.json` | `locale/*.json` | 8 файлов скопированы без изменений |
| `tests/initialization.spec.ts` | `tests/initialization.rs` | 4 / 0 случаев образца |
| `tests/loader-composition.spec.ts` | `tests/loader_composition.rs` | 0 / 1: вместо отсутствующего полного Loader-состава проверен собственный plugin contract |
| `tests/persistence.spec.ts` | `tests/persistence.rs` | 1 / 0; настоящий JSONL backend |
| `tests/retry.spec.ts` | `tests/retry.rs`, unit-тест в `src/lib.rs` | 25 / 7 случаев образца; плюс прямые проверки разделённых ветвей и policy key |
| `tests/transport-recovery.spec.ts` | — | 0 / 8, ждёт HTTP/SSE-крейты (см. ниже) |
| `README.md`, `package.json`, `README.i18n.yaml`, `tsconfig.json`, `tsdown.config.ts` | эти заметки и `Cargo.toml` | прочитаны, контракт и зависимости учтены |

LICENSE в пакете образца отсутствует.

## Отличия от образца

1. Функция-конструктор TypeScript `RetryId(id)` представлена обычными Rust-преобразованиями
   `RetryId::from(String | &str)`; одноимённые тип и функция не могут занимать один value namespace Rust.
2. Публичные Rust-символы сверх образца: `LlmRetryPlugin` и `Deps` нужны kernel для функции-плагина;
   `apply` получает `SessionProjectionRegistry` явно, потому что Rust-сервис не имеет Cordis Proxy-контекста.
3. Обычный jitter берёт равномерные 53 случайных бита из уже разрешённого `getrandom`, а не `Math.random`;
   инъекция `RetryInternals.random` и формула совпадают. Случайное значение наружу не записывается отдельно.
4. Kernel logger принимает одну строку: `%o` downstream-ошибки представлен её `Display` в той же WARN-записи.
   Это согласованное отличие диагностического журнала, как в других перенесённых плагинах.
5. `tests/loader_composition.rs` — дополнительная проверка имени, inject и схемы. Исходный Loader-сценарий
   не считается перенесённым.
6. Rust kernel waterfall повторно проверяет `Listener::alive()` перед каждым звеном захваченной цепочки.
   Поэтому listener, удалённый во время более раннего recovery callback, пропускается и управление штатно
   переходит следующему listener; недоступную извне Cordis-семантику «вызвать уже захваченный callback после
   disposal и fail closed» нельзя воспроизвести внутри этого крейта без изменения API `hoh-kernel`.

## Не перенесено

- `transport-recovery.spec.ts`, 8 случаев: отсутствуют `hoh-llm-deepseek` и
  `hoh-llm-mock-server`, которые нужны для настоящего HTTP/SSE, refused connection, partial EOF и idle timeout.
- `loader-composition.spec.ts`, 1 случай: нет Rust-аналога `cordis-plugin-include` и готового полного
  loader/agent-loop test composition.
- 7 случаев `retry.spec.ts`: `EMPTY_RESPONSE` от finish chunk; сохранение partial chunks без tool side
  effect; выбор policy по обслужившей/записанной route (2); замена adapter во время thrown/in-band failure
  (2); отсутствие failed partial output в повторном model context. Они требуют отсутствующих публичных
  `hoh-agent-loop-testkit`/`hoh-tool-testkit` либо HTTP adapter testkit. Логику самого executor для этих ветвей
  проверяют прямые waterfall-тесты, но ослабленным end-to-end тестом исходные случаи не заменены.

Итого случаев образца: 30 перенесено, 16 не перенесено. Проверки типов TypeScript отдельно не считаются:
соответствие payload закреплено сериализацией и Rust-типами.

## Не проверено

- Настоящий `hoh-agent-loop` с живым adapter и tool runtime; оставшиеся границы перечислены выше.
- Перекрытие первого failure с искусственно задержанной успешной регистрацией: API
  `SessionProjectionRegistry` не имеет test hook задержки. Ready/cancelled/disposed и registration failure
  проверены без такого hook.
- Отказ системного источника случайности; Windows/macOS отдельно не запускались.

## Вопросы ведущему

1. Для оставшихся 16 случаев нужны отдельные крейты/testkit: `hoh-agent-loop-testkit`,
   `hoh-tool-testkit`, `hoh-llm-deepseek`, `hoh-llm-mock-server` и Loader include composition. Копировать их
   реализацию в этот крейт запрещено границами задания. После появления API тесты можно добавить без изменения
   executor.
2. Экспорты, которыми сейчас не пользуется другой переносимый пакет: `LlmRetryStartedEventData`, `Config`,
   `RetryInternals`. `RetryId` и объявление `llm/retry` нужны `packages/llm/token-meter`; сам плагин входит в
   bundle-составы.
