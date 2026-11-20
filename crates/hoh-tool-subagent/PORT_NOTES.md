# hoh-tool-subagent ← packages/subagent/tool-subagent @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | плагин `tool-subagent`, конфигурация, жизненный цикл provider/tool, схема и выполнение делегирования, prompt/context |
| `src/list-models.ts` | `src/list_models.rs` | discovery-tool, фильтрация Session policy, формат provider/model/effort |
| `src/model-selection.ts` | `src/model_selection.rs` | проверка и слияние маршрутов, preflight через LLM runtime |
| `src/model-selection-settings.ts` | `src/model_selection_settings.rs` | сервис и плагин `subagent-model-selection-settings` |
| `src/model-selection-state.ts` | `src/model_selection_state.rs` | durable projection `subagentModelSelectionPolicy` |
| `src/preset-selection.ts` | `src/preset_selection.rs` | конфигурация, capability-check и разрешение preset |
| `tests/depth-context.spec.ts` (4) | `tests/runtime.rs`, unit-тесты `src/lib.rs` | объединены по контракту depth context |
| `tests/list-models.spec.ts` (20) | unit-тесты `src/list_models.rs`, `tests/pure.rs`, `tests/runtime.rs` | объединены по границам discovery/policy/tool context |
| `tests/model-selection-initialization.spec.ts` (1) | `tests/runtime.rs` | перенесён |
| `tests/model-selection-settings.spec.ts` (14) | `tests/runtime.rs`, `tests/pure.rs` | объединены по projection/settings contract |
| `tests/model-selection.spec.ts` (22) | `tests/pure.rs` | объединены по чистым функциям маршрутизации |
| `tests/preset-selection.spec.ts` (9) | `tests/pure.rs`, `tests/runtime.rs` | объединены по preset contract |
| `tests/scripted-provider.spec.ts` (7) | `tests/runtime.rs` и provider-contract тесты `hoh-subagent` | test-only provider заменён типизированным Rust provider |
| `tests/tool-subagent.spec.ts` (36) | `tests/runtime.rs`, `tests/pure.rs`, unit-тесты `src/lib.rs` | объединены по внешним границам плагина |
| `locale/`, `locale/model-selection-settings/` | те же пути | скопированы без изменений (`diff -qr` чист) |
| `README.md`, `README.zh.md`, `README.i18n.yaml` | те же пути | скопированы без изменений |

Трассировка 113 исходных `it`/`test` — в `TEST_MATRIX.md`: у каждого случая названы конкретные
тесты Rust либо указана причина переноса «отложено». Раздел «Не перенесено» ниже перечисляет отложенное.

## Отличия от образца

- `provider_wording`, `list_subagent_models`, `ModelSelectionPolicyState` и
  `SubagentModelSelectionConfig::current` публичны для типизированных потребителей и
  прямых контрактных тестов. В TypeScript соответствующие детали закрыты модулем или
  классом; наблюдаемое поведение плагина не меняется.
- Test-only `scripted-provider.ts` не является частью `src`/exports. Его поведение
  проверяется через `SubagentProvider` и принятый runtime `hoh-subagent`, без отдельного
  production-типа в этом crate.
- JS-объекты аргументов и конфигурации представлены типами Rust; имена JSON сохраняются
  через serde (`camelCase`), а model-facing поля — готовыми `serde_json::Value` в исходном порядке.

## Не перенесено

Поведение `src/` перенесено целиком. Тесты `list-models.spec` перенесены полностью (ревью 213559).
Отложены только (подробности и причины — в `TEST_MATRIX.md`):

- «shares guidance only among viewing agent's visible definitions», «hides continuable guidance»,
  «discovery guidance only while visible»: отложены, потому что guidance отбирает инструменты по
  `is_delegation_definition` (см. «Отличия» ниже), а проверка без идентичности определений проверяла бы
  эвристику, а не контракт TS;
- continuable-провайдер со скриптом (`isolates cancelled continuable preparation`): путь
  `prepare_continuable` принадлежит `hoh-subagent`, скриптового continuable-провайдера у крейта нет;
- пресеты с реальной загрузкой дочернего агента и переключением пресета (5 случаев preset-selection,
  часть settings), настройка хоста «глубина по умолчанию» (`hoh-subagent`);
- fork-провайдер (`hoh-subagent-fork-in-process`).

## Отличия: видимые delegation-инструменты

В TS `delegationTools` — множество определений на уровне модуля, общее для всех экземпляров плагина
(`index.ts:41`), и guidance сравнивает `ctx.tools.get(name, scope) === tool`. Rust определяет видимые
delegation-инструменты по маркеру описания и именам параметров (`is_delegation_definition`). Идентичность
не перенесена намеренно: общее множество между экземплярами плагина — это глобальное изменяемое состояние
(`PORTING.md`, раздел 6 запрещает `static` с реестрами), а сервис-владелец множества потребовал бы нового
типа сервиса и строки в составе плагинов вне этого крейта. Следствие: чужой инструмент, у которого
в описании есть маркер и есть параметры `description` и `prompt`, попадёт в список имён guidance.
Собственный инструмент экземпляра сравнивается по идентичности (`Arc::ptr_eq` с определением, возвращённым
из реестра в области вызова), поэтому подмена своего инструмента областным переопределением учитывается.

## Другие крейты

Крейт не требует правок других крейтов. Прежняя правка `hoh-subagent/src/runtime.rs`
(`label = spec.label`) для него больше не нужна: инструмент сам кладёт `description` в `request.label`
(раздел ниже). Если правку вносить, то отдельным пакетом `hoh-subagent`.

## Ревью 213559 (s1, 20.11)

- Порядок ошибок `list_subagent_models`: агент → политика/родительский route → сервис `llm`, как
  `list-models.ts:46-61` (`list_subagent_models` принимает `Option<&LlmRuntime>`).
- Слушатель `AgentCreated` в `SubagentModelSelectionSettings` при упавшей регистрации проекции получает
  ошибку вместо бесконечного ожидания (аналог отклонённого `policyReady`). Зависание не воспроизведено:
  ядро снимает слушателя вместе с упавшим плагином, и тест `a_failed_policy_registration_rejects_agent_publication_instead_of_hanging`
  проходит и на прежнем коде; исправление защитное, тест фиксирует наблюдаемый контракт.
- Монтирование в области агента (`agent.ctx().plugin::<ToolSubagent>`) падало «registration was not visible»:
  после регистрации определение читалось только из глобальной области. Теперь читается в области `ctx`
  (`hoh_scope::scope_of`); тест `keeps_discovery_registration_and_disposal_local_to_each_tool_scope`.
- Тест схемы discovery теперь сравнивает определение, зарегистрированное `register_list_subagent_models`
  (описание и `parameters.properties` побайтно с `list-models.ts`, порядок ключей как в источнике).
- Не менялось, вне этого ревью: `validate_provider` проверяет сервис `AgentPresets` на каждом `provider-added`
  (в TS один раз в `apply`); `toolName: ""` отвергается (схема TS его допускает; принимает ли пустое имя `defineTool`, не проверялось). Ветка «agentPreset без сервиса» покрыта тестом
  `an_agent_preset_without_the_presets_service_fails_the_mount`.
- Слияние `{...providerRouteDefaults, ...config.agentOptions}` и «заполнить только None» различаются
  лишь если схемный объект материализует ключи как `undefined`; серде-конфигурация таких ключей не создаёт.

## Не проверено

- Реальный сетевой LLM adapter не запускается: route/catalog/preflight проверяются
  локальным runtime и mock/provider контрактами без сети.
- Поведение не имеет платформенных веток.

## Вопросы ведущему

Нет.

## Метка запроса не зависит от правки hoh-subagent (s1, 20.11)

Конвейер собирал пакет с `hoh-subagent` из main без правки `runtime.rs` (`label = spec.label`), и
`defaults_max_depth_to_one_and_forwards_the_label` / `overlaps_sibling_delegations_…` получали
`label = None`. Теперь инструмент кладёт `description` и в `spec.label`, и в `request.label`
(как `...request` в источнике): тесты зелёные с правкой и без неё. Правка `hoh-subagent` остаётся
верной и отдельным пакетом нужна, но от неё этот крейт больше не зависит.

## Расхождения с TS и их закрытие (ревью 212524, s1 20.11)

- **Пункт 2 (порядок ошибок `list_subagent_models`)**: закрыт. `list_subagent_models` принимает
  `Option<&LlmRuntime>` и проверяет агента и родительский route раньше сервиса `llm`, как TS.
  Ветки preflight без маршрута, без сервиса `llm` и с отвергнутым effort покрыты в
  `tests/review_214605.rs` (ревью 214605, пп. 1–4); во всех случаях `start` не вызывается.
- **Пункт 3.** `agentOptions.maxTokens` выше `Number.MAX_SAFE_INTEGER` отклоняется при загрузке
  (`max_tokens_above_safe_integer_is_rejected`). Пустые `provider`/`model` в `allowedModels`
  отклоняются при разборе конфигурации, как `AllowedModelRouteSchema` (`min(1)`), а не лениво
  (`settings_route_ids_are_validated_when_the_config_loads`). `provider: ""` у инструмента:
  `required()` в schemastery (`vendor/schemastery/src/index.ts:533`) отвергает только null/undefined,
  поэтому пустая строка допустима; проверку «provider must be non-empty» убрали, провайдер
  с пустым именем просто не находится (`an_empty_provider_string_…`). `toolName: ""` Rust по-прежнему
  отклоняет, в TS `z.string().default('subagent')` его принимает — не менялось, вне ревью.
- **Пункт 4.** Тексты `apply` как в источнике: «requires an array of routes» для отсутствующего или
  не-массива, «requires non-empty provider and model ids» для элементов не-объектов и пустых/нестроковых
  id (`projection_apply_error_texts_match_source`). Схема сохранённого состояния — строгие объекты
  без лишних ключей, `min(1)`, дубликаты не проверяются (их ловит только `apply`)
  (`projection_state_schema_is_strict_without_checking_duplicates`).
- **Пункт 5.** `origin:"subagent"` без `parentSession` падает в ветку корневой сессии, как в TS
  (`subagent_header_without_parent_falls_back_to_root_sampling`).
- **Заметки.** Guard `Mutex` в слушателе `SubagentProviderRemoved` больше не держится на время
  `dispose()`. Остаются без изменений: определение видимых delegation-инструментов по тексту
  описания; сортировка имён по байтам UTF-8 (TS — по UTF-16); `register_list_subagent_models`
  выполняется до validate-слушателя (в TS порядок обратный).

## Причина 2 разбора сценариев (s1, 21.11)

- Контекст `tool:<имя>:depth` получает `order + 0.25` для `subagent` и `+ 0.5` для остальных имён
  (`depth_order_offset`). В источнике контексты равного `order` идут в порядке регистрации = порядку
  строк профиля: `subagent:identity` (tool-subagent-control), `tool:subagent:depth`,
  `tool:subagent_fork:depth`, затем `subagent:delegation`. Плагины Rust стартуют конкурентно, порядок
  был гонкой (сценарии проходили через раз), поэтому последовательность закреплена дробями.
- Отказ по глубине доходит до модели как `SubagentDepthError` (`depth_failure`); тест
  `tests/depth_error.rs`.
- Не решено в этих крейтах: `agent-preset-registry` монтирует плагины пресета через
  `PresetRuntime::fallback()` с пустым реестром (`hoh-agent-preset-registry/src/lib.rs:832`), поэтому
  `persona`/`tool-fs`/`tool-subagent` внутри пресета «never started» (`session/subagent-agent-preset`).
