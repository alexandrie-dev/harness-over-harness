# hoh-deliverable-file-references ← packages/deliverables/deliverable-file-references @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` (30 строк) | `src/lib.rs` | все экспорты: `name`, `inject`, `apply`; текст `FILE_REFERENCE_PROMPT` (в образце не экспортируется) побайтно |
| `tests/prompt.spec.ts` | `tests/prompt.rs` | единственный случай перенесён целиком: снимок текста, сверка с четырьмя файлами `system-prompt.expected.md`, снятие секции при выгрузке |
| `locale/*.json` (8 файлов) | `locale/` | скопированы без изменений |
| `README.md`, `README.zh.md`, `README.i18n.yaml`, `package.json` | эти заметки | прочитаны, контракт учтён |

У образца нет `LICENSE` и отдельно лицензированных фрагментов. Зависимости: `hoh-kernel`, `hoh-system-prompt`
(сервис `SystemPromptKey`, `PromptSection`, порядок `DeliverableFileReferences`); для тестов `serde_json`, `tokio/test-util`.
Код чужих crates не менялся и не копировался.

## Отличия от образца

1. **Плагин.** `name = 'deliverable-file-references'`, `inject = ['systemPrompt']` →
   `#[plugin(DeliverableFileReferences, name = "deliverable-file-references")]`, `Deps { system_prompt: Service<SystemPromptKey> }`,
   конфигурации нет (`NoConfig`). Дополнительно `pub const NAME` с тем же значением. Строка загрузчика
   `@deepseek-ai/dsh-deliverable-file-references` → этот плагин: регистрация в таблице имён — за ведущим.
2. **Текст секции** собран `concat!` из тех же семи фрагментов, что и в образце, без изменений: `":24–30"` —
   с длинным тире U+2013, как в образце. Имя секции `ui:deliverable-file-references`, порядок —
   `SystemPrompt::get_section_order(DeliverableFileReferences)`; больше полей у секции нет (`interpolate` не задан).
3. **Тест сверяет снимки не по пути `../../../../snapshots`**, а по копиям в `tests/fixtures/{web,sdk}/<сценарий>/system-prompt.expected.md`
   (четыре файла: `web/fresh-round-trip`, `web/ptc-round`, `sdk/desktop-context`, `sdk/desktop-ptc-context`;
   скопированы без изменений из `snapshots/` образца). Причина: тест не должен читать вне crate. Если образец
   поменяет эти снимки, копии надо обновить.
4. `await mounted.await()` → `fiber.settled().await`; `ctx.plugin({ apply, inject })` без имени → `ctx.plugin::<DeliverableFileReferences>(json!({}))`.

Публичные символы, которых нет в образце: `DeliverableFileReferences`, `Deps`, `NAME`.

Экспорты, которые переносимые пакеты не используют в коде: все. На пакет ссылаются строки конфигурации и документация
(`apps/cli/package.json`, `tool-present/README.md`, `agent-preset/skills/dsh-doc`). Всё перенесено.

## Не перенесено

- `import type {} from '@deepseek-ai/dsh-system-prompt'` — подмешивание объявлений Cordis (PORTING §8).
- `toMatchInlineSnapshot` как механизм — заменён `assert_eq!` с тем же текстом.

## Не проверено

- Совместимость порядка секции `DeliverableFileReferences` (9000) с живой сборкой промпта в приложении: проверен только по
  таблице `hoh-system-prompt`.

## Вопросы ведущему

Нет.

## Проверки

Команды из корня workspace, каждая с
`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-s4`:

- `cargo test -p hoh-deliverable-file-references`: prompt — **1 passed; 0 failed**.
- `cargo clippy -p hoh-deliverable-file-references --all-targets -- -D warnings`: чисто.
- `cargo fmt -p hoh-deliverable-file-references -- --check`: чисто.
