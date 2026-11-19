# hoh-session-title-first-prompt-llm ← packages/session/session-title-first-prompt-llm @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/index.ts` | `src/lib.rs` | config, plugin, first-prompt provider, prompt/framing и output policy перенесены |
| `tests/provider.spec.ts` | `tests/provider.rs`, unit-тест в `src/lib.rs` | все 8 объявлений, включая 7 табличных parse-вариантов, перенесены |
| `tests/loader-composition.spec.ts` | `tests/provider.rs` | Kernel-композиция и reasoning floor перенесены без JS Loader и HTTP-сервера |
| `tests/provider.e2e.ts` | — | real-API smoke не запускается без сети и `DEEPSEEK_API_KEY` |
| `locale/*.json` | `locale/*.json` | восемь файлов скопированы без изменений |

## Отличия от образца

- `Config` публикует `JsonSchema` и flatten-ит принятый
  `SessionTitleLlmConfig`; custom `Deserialize` сохраняет порядок и точные
  тексты direct-construction validation.
- Cordis `inject` представлен `Deps` с теми же `sessionTitle`, `llm`
  и `sessions`. Provider владеет клонируемым Kernel `Context`, а registration
  привязана к lifecycle plugin context.
- TypeScript provider object представлен `SessionTitleProvider`; Promise —
  `BoxFuture`. Закрытый finish union исчерпывающе сопоставляется
  Rust enum, поэтому runtime `assertNever` не нужен.
- Loader composition проверяется настоящими Kernel plugins; JS YAML
  dynamic import/reflection не имеет Rust runtime-аналога. Pi-AI HTTP-случай
  заменён adapter metadata с `low/high/max` и logged main route с effort `max`:
  auxiliary title call наследует route, но выбирает собственный `low`.

## Круг исправлений review 202100

- Inherited-route сценарий приведён к точной deployment policy исходного
  loader-теста: provider config не содержит route, `request/header` задаёт
  `main-route/main-model` и effort `max`, а auxiliary call и durable event
  используют тот же route, собственный floor effort `low` и cap `64`.
- Reasoning-only ответ теперь содержит явный `block-start`, как
  `tests/provider.spec.ts:226-229`, перед `reasoning-delta` и по-прежнему
  отклоняется точным `title model produced no text`.
- Замечание о custom-тексте через plugin loader неприменимо: TS-тест
  `tests/provider.spec.ts:236-245` вызывает `providerPlugin.apply` напрямую,
  минуя Loader schema. Rust Kernel намеренно валидирует опубликованную схему
  до `Plugin::apply`; тест закрепляет его точную schema-ошибку, а отдельный
  typed-policy тест закрепляет исходный custom-текст `targetWords must be a
  positive integer`.

## Не перенесено

- Real DeepSeek E2E из `provider.e2e.ts`: исходник сам пропускает его без
  `DEEPSEEK_API_KEY`; текущая приёмка запрещает сеть.
- npm ESM/default export, Cordis Loader reflection и generated config-catalog не
  являются runtime-контрактами Rust-крейта.

## Не проверено

- Real DeepSeek API (см. выше). Все локальные ветви проверены:
  `cargo test --offline -p hoh-session-title-first-prompt-llm` — 10/0;
  `cargo clippy --offline -p hoh-session-title-first-prompt-llm --all-targets --no-deps -- -D warnings`
  и `cargo fmt --check --package hoh-session-title-first-prompt-llm` чисты.

## Вопросы ведущему

- Нет; shim и правки API чужих крейтов не требуются.
