# Нормализация первой версии

Записи dsh сделаны полным набором плагинов dsh и провайдером `llm-deepseek-api-key` + `llm-replay`.
Первая версия HoH — 118 строк (`research/v1-engine-scope.codex.md`). Правило этого файла:
**нормализуется только то, что вносит плагин вне 118, и только в записанной (expected) стороне.**
Фактическая (actual) сторона не переписывается никогда: то, что добавил или изменил наш плагин, остаётся
расхождением. Расхождение нашего поведения нормализацией не закрывается.

Каждое правило кода из `normalize::v1` имеет раздел ниже: что, где, почему, какой тест. Тест
`every_rule_has_a_section_in_normalization_md` (`tests/normalization.rs`) падает, если в `normalize::v1::RULES`
есть правило без раздела здесь, а раздела без правила в коде не бывает.

## Правила плагинов вне 118

### N-BOOT-FOREIGN-PLUGINS

- **Что.** Последний `--patch` каждого запуска отключает строки профиля (`disabled: true`) для пакетов, которых
  нет в 118 и для которых нет Rust-реализации: `connection`, `file-upload` (браузерный клиент, MAP: skip),
  `deepseek-account`, `llm-deepseek-account`, `llm-pi-ai`, `hooks-claude-code`, `hooks-codex`, `plugin-manager`,
  `tool-plugin-manager`, `ptc-runtime`, `workflow-ptc`, `tool-workflow`, `tool-fs-search`,
  `repeat-tool-reminder`, `plan-mode`, `typert`, `typert-loader`, `typert-gateway`, `remote-access`, `timer`,
  `session-title-provider` (второй вариант, `all-prompts`). Полный список — `normalize::v1::FOREIGN_PLUGINS`.
- **Где.** `runner::execute` дописывает `<n>-v1-exclusions.cordis.yml` после патчей сценария.
- **Почему.** Профили dsh (`base`, `headless`, `sdk-app`) монтируют эти строки. Без их отключения профиль sdk
  не стартует: обязательная строка `connection` не имеет реализации (`startup failed`), а остальные дают лишний
  шум в stderr. Строки внутри 118, которых ещё нет (`tool-jobs`, `jobs-local`), **не отключаются** — их отсутствие
  остаётся «не запустилось/расхождение».
- **Тест.** `exclusion_patch_disables_every_foreign_row_and_nothing_else`,
  `no_foreign_plugin_is_inside_the_118` (`tests/normalization.rs`).

### N-PROMPT-FOREIGN

- **Что.** Из записанного системного промпта удаляются абзацы (текст между пустыми строками), начинающиеся с
  `Use the glob tool — not shell find — ` и `Use the grep tool — not shell grep or rg — ` (пакет
  `tool-fs-search`), `Use the workflow tool ONLY when the user explicitly asks for a workflow` (`tool-workflow`).
- **Где.** `compare::headers` (сравнение с `system-prompt*.expected.md`) и `compare::hydrate` (запись fake).
- **Почему.** Эти абзацы пишет `ctx.systemPrompt.section` плагина вне 118; без плагина абзаца нет и быть не
  должно. Абзац `Track every background job id…` принадлежит `tool-jobs` (внутри 118) и **не нормализуется**.
- **Тест.** `foreign_prompt_paragraphs_are_dropped_and_others_kept`, `a_dropped_last_paragraph_keeps_the_closing_newline`.

### N-TOOLS-FOREIGN

- **Что.** Из записанных наборов схем инструментов (`tool-schemas*.expected.json`, `initial` и каждый
  `changes[i]`) удаляются инструменты `glob`, `grep` (`tool-fs-search`), `workflow` (`tool-workflow`).
- **Где.** `compare::schema_sets` (наборы схем), `compare::logs` через `normalize::v1::header_rows` (поле `tools` в записанных строках `request/header`: список имён или определений; только записанная сторона).
- **Почему.** Плагины вне 118 не загружены, их инструменты не регистрируются. Остальные инструменты, в том
  числе их описания и параметры, сравниваются без правок.
- **Тест.** `foreign_tools_are_dropped_from_a_schema_set_and_the_rest_is_untouched`, `foreign_tools_are_dropped_from_recorded_header_rows_in_both_forms`.

## Нормализация провайдера (условие CEO)

Записи сделаны провайдером `llm-deepseek-api-key` (route `deepseek-official`) и воспроизведены плагином
`llm-replay` под тем же идентификатором провайдера: патч сценария (`cordis.snapshot.yml`) отключает
`llm-deepseek-api-key` и вставляет `llm-replay` с `providers: [{id: deepseek-official, models: …}]`, а
`agent-default-model` выбирает `deepseek-official` и модель из `request/header.config`.
Поэтому на первом прогоне **ни одного правила замены провайдера нет**:

| что могло бы нормализоваться | правило | состояние |
|---|---|---|
| заголовки HTTP-запроса | — | не нормализуются: `llm-replay` не посылает запросов, заголовок запроса в журнале — событие `request/header` |
| поля модели `provider`/`model`/`maxTokens` в `request/header.config` | — | сравниваются как есть; маршрут берётся из самой записи (`runner::environment`) |
| `thinking`/`reasoningEffort` | — | сравниваются как есть; значения записаны в `config` заголовка |
| идентификаторы сессий, сообщений, запросов | `identity.ts` (перенос в `normalize::identities`) | это не замена провайдера, а штатная типизированная нормализация образца |
| системный промпт, схемы инструментов | N-PROMPT-FOREIGN, N-TOOLS-FOREIGN | только плагины вне 118, не провайдер |

Если прогон покажет расхождение, вызванное именно заменой провайдера, правило появится здесь строкой
«что, где, почему, тест» вместе с кодом — не раньше. Тест `provider_replacement_adds_no_normalization_rule`
закрепляет, что в `normalize::v1::RULES` нет правил с `PROVIDER` в имени, пока этот раздел не изменён.
