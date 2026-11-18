# hoh-kernel ← vendor/cordis, vendor/timer, vendor/logger-console, vendor/schemastery @ 39163dd3

API compiles — 2026-11-16 16:59 UTC (`cargo build -p hoh-kernel`).

## Файлы

| Источник | Rust | Состояние |
|---|---|---|
| core/src/fiber.ts, registry.ts | src/fiber.rs | actor, эпохи, committed view, конфигурация |
| core/src/context.ts, reflect.ts, service.ts | src/context.rs, src/lib.rs | типизированные сервисы, области, контексты |
| core/src/events.ts | src/events.rs | пять режимов, снимки, фильтры, hooks |
| core/src/fiber.ts effect runner | src/effects.rs | Undo, составные эффекты, rollback |
| vendor/cordis/src/logger.ts, vendor/logger-console | src/logger.rs | журнал, stderr exporter |
| vendor/timer | Context::sleep, interval | отменяемое время tokio |
| vendor/schemastery, loader/src/config/diff.ts | src/config/{schema,diff}.rs | закрытый набор JSON Schema, volatile comparison |

MIT LICENSE всех четырёх vendor пакетов совпадает до байта; сохранён в LICENSE.

## Отличия от спецификации

Пункты 1–10 согласованы ведущим в brief A и docs/kernel.md §10.1; прежние вопросы закрыты.

1. `Fiber::dispose()` сохраняет `Future<Output = ()>`. При ожидании своего fiber/предка команда отправляется, ожидание завершается немедленно, SelfWait пишется в журнал: вернуть Err с таким Output невозможно. Метка охватывает apply, cleanup, update, ctx.spawn и задачи dispatch. Остальные ожидаемые переходы возвращают SelfWait. Методы с результатом (), Option, bool или Snapshot также не могут вернуть Closed: после остановки дают идемпотентный/пустой результат; Result-операции дают Closed.
2. Прежнее отклонение Realm устранено: процессный `AtomicU64::fetch_add`, ROOT исключён, разрешённое исключение PORTING §6.
3. `PluginDef::of()` не может вернуть ошибку R48 при контрактном результате PluginDef. Неизвестное слово схемы даёт ValidationError при первой проверке, включая слова в отсутствующих полях.
4. Обычный Emit распространяет панику слушателя: upstream events.spec «ctx.emit()» проверяет исключение. Это приоритет теста над R40. События ядра и VolatileUpdate изолируют панику каждого наблюдателя, R61 сохранено.
5. Асинхронные retained-instance update handlers могут завершаться одновременно; публикация защищена идентичностью raw-снимка, поколением и закрытием активации. dsh core-lifecycle «publishes successful retained-instance updates and ignores stale or rejected completions» ждёт второе обновление до освобождения первого, что несовместимо с сериализацией R42. Приоритет теста; эпохи и переходы остаются строго последовательными.
6. R26 уточнено по isolate.spec и core-lifecycle: собственный поставщик читает свою уже зарегистрированную запись в Loading. Другие fiber видят её только в Active. Запись и отмена предоставления по-прежнему команды actor.
7. Добавлены `Fiber::retain_raw_config(raw)` (командный аналог rawConfig setter в dsh tests) и `Listen::on_update` (prepend в events.spec). Иначе соответствующие исходные случаи не выразить контрактным API.
8. Добавлен скрытый `View::service<K>()`: derive из другого crate должен читать committed view. Конструкторов View и Service вне ядра нет. ConsoleConfig публичен как ассоциированный Config ConsoleExporter. Volatile имеет Default при T: Default.
9. Универсальное `Logger::error(impl Display)` не позволяет безопасно распознать KernelError без дополнительного Any/'static bound, нарушающего вызовы с заимствованным format_args. R57 реализовано на типизированной границе логирования ошибок ядра; прямой Logger::error(Listeners) печатает одну aggregate-запись. Исходный тест flatten проверяет эту границу через ошибку apply.
10. Прежнее отклонение UTC устранено: ConsoleExporter использует местный пояс `jiff::tz::TimeZone::system()`. Форматирование получает пояс параметром; тесты передают UTC и фиксированный UTC+03. ts_ms — Unix milliseconds, привязанные к tokio Instant; пауза времени детерминирована.
11. Trybuild — синхронный compile-only #[test], поскольку macros crate не имеет разрешённой зависимости tokio. Все проверки поведения — #[tokio::test(start_paused = true)].
12. Volatile placement и повторная обёртка проверяются до проверки значения (в том числе при отсутствующем поле), по volatile-config source; JSON Schema несёт x-volatile и служебную x-volatile-invalid для повторной обёртки.
13. Новый no-op `Context::child` держит открытую активацию в Pending (исключение из R17), поэтому dispose до старта проходит Pending → Unloading → Disposed, а не прямой переход R14. Так требует vendor/cordis/src/fiber.ts:305–323 для ранних эффектов. Пятый круг подтвердил существующее поведение тестом InternalStatus; подробности и границы исключения — R66 ниже.

## Отличия от образца

Типизированные Rust API, отмена активации CancellationToken, неизменяемые производные контексты, отсутствие Proxy/HMR, остановка после shutdown — решения спецификации. JSON defaults задаёт serde; Validator не исполняет TS transforms/autofix. Видимый текст JSON и чисел формирует разрешённый теперь hoh-util-values; длина строк считается в UTF-16.

Прямой ctx.foo и запись ctx.foo из TypeScript запрещены компилятором для любого Context (UI access_check/access_error/pinned_access). Поэтому pinned same-realm запрет строгого property access проверяется компилятором, а runtime get возвращает Option по общей Realm согласно R36. В isolate tests доступ ребёнка к значению родителя передаётся явно либо объявляется в Deps. UUID fiber в Rust остаётся u32; is_disposed заменяет сравнение uid с null.

## Учёт тестов по исходным файлам

Имена Rust ниже соответствуют source-case names в snake_case. Параметризованные варианты объединены циклами одного теста. В исходном volatile-config файле фактически 11 вызовов it, хотя таблица §8.2 указывает 10.

### [U] component.spec.ts → tests/component.rs

Перенесены (6): `apply_functional_component`, `inactive_context`, `nested_components`, `compare_snapshot`, `root_dispose`, `service_init`.

Не перенесено / адаптация: apply invalid component — runtime type check; context inspect — util.inspect; ctx.registry — Map API. apply object component и deprecated ctx.plugin сведены к apply_functional_component (§8.1).

### [U] dispose.spec.ts → tests/dispose.rs

Перенесены (14): `dispose_by_component`, `dispose_manually`, `yield_dispose`, `async_return_1`, `async_return_2`, `async_yield_1`, `async_yield_2_aborted`, `async_yield_3_aborted`, `async_yield_aborted_finally`, `async_yield_4_await_dispose`, `return_with_error`, `yield_with_error`, `async_return_with_error`, `async_yield_with_error`.

Не перенесено / адаптация: async yield invalid effect — исключён типом Undo.

### [U] events.spec.ts → tests/events.rs

Перенесены (17): `ctx_on`, `removes_empty_event_buckets_after_disposal`, `skips_listeners_removed_during_dispatch`, `defers_added_listeners_until_the_next_dispatch`, `skips_a_listener_disposed_by_an_earlier_component_during_dispatch`, `waterfall_continues_past_a_removed_listener`, `waterfall_defers_added_listeners_until_the_next_dispatch`, `internal_update_continues_past_a_removed_fiber_listener`, `ctx_parallel`, `ctx_emit`, `ctx_serial`, `ctx_bail`, `ctx_waterfall`, `ctx_waterfall_supports_nested_async_calls`, `does_not_accumulate_across_reloads`, `can_be_disposed_explicitly`, `runs_prepended_listeners_first`.

Не перенесено / адаптация: deprecated dispatch returns bound callbacks that respect later removal; deprecated waterfall dispatch continues past a removed callback — устаревший dispatch. supports symbol event names across dispatch modes; supports symbol event names with ctx.once(); ctx.parallel() labels symbol events — Symbol. treats prototype property names as ordinary events — прототипы JS. ctx.once(); ctx.once() runs once when an earlier listener emits recursively; ctx.once() runs once across concurrent serial dispatches; waterfall continues after a recursive dispatch has consumed a once listener — O5. ctx.waterfall() rejects duplicate next(); ctx.waterfall() rejects continuations from outer frames; ctx.waterfall() rejects duplicate next() after awaiting — потребляющий Next, UI next_twice. global listeners are not tied to a child fiber reload — O5.

### [U] fiber.spec.ts → tests/fiber.rs

Перенесены (22): `inertia_lock_1`, `inertia_lock_2`, `inertia_lock_3`, `component_error`, `failed_fiber_does_not_re_enter_on_dependency_refresh`, `update_recovers_a_failed_fiber`, `update_waits_for_asynchronous_recovery`, `update_surfaces_a_failed_recovery_to_its_caller`, `update_surfaces_a_failed_reload_to_its_caller`, `update_does_not_leak_a_dropped_failure`, `dispose_error`, `settles_a_pending_fiber_to_disposed_on_dispose`, `settles_a_failed_fiber_to_disposed_on_dispose`, `keeps_the_active_dispose_sequence_stable`, `update_config_on_wrapped_fiber`, `restart_wrapped_fiber`, `update_config_while_injected_service_reloads`, `fail_on_first_load`, `keep_a_running_component_on_update`, `keep_the_issues_and_their_paths`, `pass_the_resolved_config_to_listeners`, `keep_the_source_config_when_a_listener_handles_the_update`.

Не перенесено / адаптация: Нет; все 22 случая.

### [U] update.spec.ts → tests/update.rs

Перенесены (10): `settles_concurrent_updates_on_the_latest_config`, `skips_superseded_loads_before_executing_the_component`, `coalesces_updates_during_unloading`, `honors_restart_config_aba_while_loading_even_when_config_is_unchanged`, `cleans_up_a_superseded_failure_without_failing_the_latest_update`, `rejects_all_waiting_updates_when_the_latest_load_fails`, `stops_advancing_an_outdated_async_generator_and_collects_its_final_yield`, `waits_for_dependencies_and_loads_only_the_latest_config`, `does_not_restart_when_a_listener_accepts_an_update_during_loading`, `does_not_load_a_queued_config_after_disposal`.

Не перенесено / адаптация: Нет; все 10 случаев, параметризованные ветви сохранены.

### [U] service.spec.ts → tests/service.rs

Перенесены (8): `pending_inject`, `traceable_effect_with_inject`, `traceable_effect_without_inject`, `compare_snapshot`, `multiple_injects`, `dispose_dependents_before_provider`, `dispose_dependents_before_provider_restarts`, `dispose_dependents_before_isolated_provider`.

Не перенесено / адаптация: answers for null and undefined; answers for a plain object after walking to the end of the chain — Symbol.hasInstance.

### [U] service-dispose.spec.ts → tests/service_dispose.rs

Перенесены (17): `releases_an_injected_binding_after_providing_the_same_name_in_another_realm`, `waits_for_a_disposing_consumer_that_has_already_left_the_registry`, `withdraws_one_binding_without_waiting_for_another_binding_from_the_same_provider`, `retains_resources_during_consumer_cleanup`, `retains_transitive_dependencies_while_a_consumer_providing_another_service_drains`, `retains_dependencies_through_in_flight_setup_and_rollback`, `releases_dependencies_after_a_cleanup_error`, `keeps_provider_listeners_alive_through_the_final_consumer_event`, `allows_unrelated_providers_and_sibling_effects_to_clean_up_concurrently`, `supports_composed_provider_effects`, `keeps_an_inherited_service_accessible_to_a_child_in_a_composed_effect`, `retains_an_ancestor_injection_until_its_child_finishes_cleanup`, `waits_for_a_child_that_explicitly_injects_its_parent_service`, `tracks_same_name_services_in_separate_isolation_realms`, `withdraws_only_the_old_binding_without_waiting_for_the_next_load`, `replaces_a_binding_from_the_same_provider_during_consumer_initialization`, `waits_for_current_and_withdrawn_bindings_during_provider_disposal`.

Не перенесено / адаптация: Нет; все 17 случаев, параметризованные ветви сохранены.

### [U] isolate.spec.ts → tests/isolate.rs

Перенесены (6): `does_not_expose_own_provisions_across_isolation_boundaries`, `does_not_inherit_the_injected_view_across_isolation_boundaries`, `isolated_context`, `shared_label`, `isolated_event`, `keeps_own_provisions_separate_from_the_injected_view`.

Не перенесено / адаптация: Нет целых случаев. Прямой доступ ctx.foo заменён Deps, явной передачей значения ребёнку или get; pinned access проверен UI, см. отличия.

### [U] intercept.spec.ts → tests/intercept.rs

Перенесены (1): `merge_context_intercepts_from_outer_to_inner`.

Не перенесено / адаптация: merge injected config under context intercepts; take injected config from the declaring fiber; stop looking for injected config at an isolation boundary; take no injected config for own provisions; take injected config from the def site; custom merge — O5.

### [U] reflect.spec.ts → tests/reflect.rs

Перенесены (3): `service_injection`, `service_inject_leak`, `does_not_load_a_fiber_disposed_during_service_notification`.

Не перенесено / адаптация: Context.is() — идентичность копий библиотеки. access check и access error — перенесены в UI access_check/access_error/service_private.

### [U] logger.spec.ts → tests/logger.rs

Перенесены (9): `keeps_the_bounded_buffer_in_place_and_chronological`, `disposes_the_exporter_that_registered_the_disposer`, `uses_fiber_name_when_called_from_outside_any_service`, `honours_explicit_name_argument`, `honours_intercept_name`, `service_writes_through_its_own_context_under_its_fiber_name`, `logs_an_error_whose_errors_array_is_empty`, `flattens_an_aggregate_error_into_its_members`, `exporter_levels_override_context_level_and_default`.

Не перенесено / адаптация: uses service name when called from inside a Service method (regression); still lets outer caller intercept override the service-derived name; uses the innermost service name and restores the outer service; uses service name when called from inside [Service.init] (unchanged behaviour) — Proxy, заменены одним тестом собственного контекста сервиса. exporter_levels_override_context_level_and_default — собственный тест.

### [D] cordis-config-access.spec.ts → tests/config_access.rs

Перенесены (4): `keeps_raw_and_running_references_unchanged_while_resolving_valid_or_invalid_candidates`, `uses_the_canonical_fiber_through_registry_wrappers_and_retains_raw_input_without_updating`, `publishes_updates_through_registry_wrappers_to_the_canonical_fiber`, `retains_a_pending_raw_update_until_its_injection_is_available`.

Не перенесено / адаптация: Нет целых случаев. Вычислитель выражений загрузчика представлен ConfigResolver; rawConfig setter — retain_raw_config.

### [D] cordis-core-lifecycle.spec.ts → tests/core_lifecycle.rs

Перенесены (7): `awaits_the_latest_update_while_loading`, `cleans_up_a_superseded_activation_failure_and_accepts_the_latest_update`, `retains_isolated_providers_through_dependent_teardown`, `separates_own_isolated_provisions_from_the_committed_injection`, `publishes_successful_retained_instance_updates_and_ignores_stale_or_rejected_completions`, `does_not_publish_an_old_update_after_raw_retention_restart_dispose`, `keeps_the_restarted_config_identity_when_a_listener_delegates`.

Не перенесено / адаптация: supports symbol events and rejects repeated waterfall delegation — Symbol, потребляющий Next; UI.

### [D] cordis-core-rc12.spec.ts → tests/core_rc12.rs

Перенесены (7): `exposes_the_root_context_through_context_root_only`, `does_not_inherit_the_injected_view_across_an_isolation_boundary`, `isolates_a_context_pinned_to_its_parent_realm_label_from_the_injected_view`, `settles_pending_and_failed_fibers_to_disposed`, `names_the_failing_event_in_the_parallel_aggregate_error`, `formats_config_validation_paths_with_keyed_segments`, `logs_an_aggregate_error_without_members_as_itself`.

Не перенесено / адаптация: merges injected metadata under context intercepts; merges through an overridden mergeIntercept in increasing precedence — O5. Исходный запрет pinned.foo — UI pinned_access; get остаётся необязательным поиском в явно общей области R36.

### [D] cordis-timer.spec.ts → tests/timer.rs

Адаптированы (2): `completes_concurrent_interval_reads_in_order_one_per_tick`, `settles_all_pending_interval_reads_on_return_throw_dispose`. Теперь каждый случай проверяет период 10 мс (9 мс — Pending, ещё 1 — tick). Во втором случае перед каждой операцией действительно есть pending read, проверены завершение без продвижения времени и повторное None после dispose.

Ограничение контрактного Stream<Item=()>: один poll_next с эксклюзивным доступом; нет AsyncIterator.return(value), throw(error) и очереди трёх next(). Поэтому три чтения последовательны; return — отмена чтения и Drop потока, throw — ошибка потребителя с отменой/Drop, dispose — None (не JS rejection). В тесте select явно выполняет consumer return/error при pending read; это адаптация, а не проверка отсутствующих методов ядра. Значение return и идентичность JS error не перенесены. Счётчика активных таймеров tokio API не предоставляет; поток не создаёт фоновой задачи, Sleep уничтожается вместе с ним.

Не перенесено: stops throttle calls and trailing work after disposal (noTrailing=%s); cancels debounce work and makes both wrappers inert after owner disposal — O5/HMR.

### [D] volatile-config.spec.ts → tests/volatile_config.rs

Перенесены (5): `parses_references_with_defaults_and_absent_values_while_preserving_schema_metadata`, `rejects_repeat_wrapping_and_supports_an_entire_array_or_union_as_a_value`, `rejects_placements_even_when_input_omits_the_field`, `ignores_reference_contents_while_retaining_ordinary_differences`, `commits_live_config_and_emits_one_local_event`.

Не перенесено / адаптация: copies and freezes data snapshots without freezing the input — заморозка JS, Arc-снимок типизирован. retains ordinary fields and secret traversal in serialized schemas; accepts metadata-free serialized nodes and checks their nested volatile declarations — сериализованные узлы schemastery и redactor другого пакета. resolves recursive lazy schemas only for the supplied data — lazy schemastery. enumerates references through shared config containers without descending into snapshots or cycles — identity/cycles JS. compares sparse array slots as undefined in both directions — JSON не имеет дыр и undefined. reports unsupported volatile values as validation issues with their path — Date не представим в JSON. В rejects %s placements ветвь transform не имеет аналога JSON Schema; остальные представимые ветви проверены. commits_live_config_and_emits_one_local_event — собственный тест.

### [D] loader-config-diff.spec.ts → tests/config_diff.rs

Перенесены (9): `ignores_volatile_fields_in_frozen_raw_inputs_without_running_schema_callbacks`, `ignores_absent_nested_and_whole_object_array_union_volatile_fields`, `preserves_ordinary_and_unknown_fields_object_defaults_and_parent_expressions`, `keeps_ordinary_raw_changes_even_if_schema_validation_would_normalize_them_to_equal_values`, `compares_raw_values_strictly_without_a_schema`, `compares_recursive_schemas_conservatively_and_visits_shared_schema_nodes_at_each_path`, `equates_absent_objects_with_defaults_while_preserving_ordinary_field_differences`, `preserves_parent_expression_edits_even_when_their_marker_matches_a_volatile_field`, `compares_metadata_free_serialized_schema_nodes`.

Не перенесено / адаптация: detects ordinary sparse array edits in both directions; compares unknown fields whose names match Object prototype properties — исключены §8.2. Подпроверки undefined и Date не представимы в JSON, см. ниже.

## Не перенесено

- [U] utils.spec.ts — DisposableList — внутренность, покрыта эффектами и снимками: `iterates unshifted entries before pushed ones`; `disposer removes its own entry after a later push`; `disposer removes its own entry after a later unshift`; `delete removes the entry it was given`; `delete removes an unshifted entry`; `clear removes every entry and returns them in order`.
- [U] associate.spec.ts — связанные свойства Proxy: `service injection`; `property injection`; `associated type - service injection`; `associated type - accessor injection`; `associated access follows the service fiber`; `inspect`.
- [U] decorator.spec.ts — заменён derive Inject / UI: `@Inject on class method`.
- [U] invoke.spec.ts — вызываемый как функция сервис: `functional service`; `uses the service shadow for callable extensions`.
- [U] shadow.spec.ts — теневые контексты Proxy; resolves same-name own provisions at their definition sites покрыт isolate.rs: `keeps caller metadata separate from the service shadow`; `applies the service inject when entered from root`; `resolves dependencies through a captured shadow`; `resolves same-name own provisions at their definition sites`; `keeps unchecked access for services provided on the root ctx`; `exposes the caller without preserving shadow for noShadow services`; `exposes the caller to callable services`; `strips service shadow before creating components`.
- [U] replace.spec.ts — замена кода на лету, решение 2: `swaps the implementation behind a fiber in place`; `emits internal/replace for each moved fiber`; `validates the config with the new schema`; `takes the dependencies of the new component`; `starts a pending fiber when the new component drops the missing dependency`; `starts a pending fiber when the new component injects an available service`; `stops depending on a dependency that the new component drops`; `reloads a consumer of a replaced service`; `merges the fibers into a registered component`; `restarts a failed fiber`; `leaves a fiber failed when the new component fails to apply`; `forwards the old component to the new one`; `forwards a component that is not registered`; `follows a chain of replacements`; `switches back to a replaced component`; `unregisters the new component when its last fiber is disposed`.

- [D] cordis-include-journal.spec.ts — O4 — обратная запись не переносится: `merges independent file and runtime edits with %s ordering`; `lets the file win a conflicting config edit and reports the conflict`; `retains unsaved edits across a missing or invalid file and clears failure after recovery`; `routes patch-owned fields and inserted rows into the parent file`; `preserves file text on file-driven removal and drains runtime edits before shutdown`; `keeps persisted runtime edits when the Include config changes`; `keeps generated ids and fibers for anonymous file and inserted rows across updates`; `persists a row moved into the Include from another tree as a complete creation`; `rejects a cross-tree move onto an id the destination already holds`; `transfers the members of a group moved into the Include`.
- [D] cordis-loader-resolve.spec.ts — Node module resolution: `loads import-only dependencies, subpaths and package mappings from the config project`; `keeps project scopes independent and shares concurrent resolution within one scope`; `imports relative, URL and builtin specifiers without changing their identity`; `retains native import when helper creation is denied`; `retains native import without a file-backed package scope`.
- [D] cordis-vendor-hmr-include.spec.ts — HMR: `logs a rejected Include refresh instead of leaving it unhandled`.
- [D] cordis-core-api.spec.ts — docs generator: `renders the five detailed pages from pinned vendor declarations`; `rejects a public core class without source JSDoc`.
- [D] cordis-config-files.spec.ts — служебный обход репозитория: `finds Loader YAML without treating translation records as configs`.
- loader-volatile-update, entry-yaml, entry-commit и §8.3 — область writer hoh-loader, здесь не дублируются.
- Дополнительные непредставимые подпроверки: идентичность JS object/Reference, Object.freeze, undefined против null и дыр массива, Date/RegExp/URL/ArrayBuffer/cycles в volatile comparison; TS transform/autofix/simplify/secret-redactor. JSON Value и типизированные Arc-снимки не имеют этих состояний. Эти подпроверки не объявляются выполненными; основные JSON/default/ordinary-field/volatile assertions сохранены.
- Оболочки registry/wrapped fiber перенесены Clone одного Fiber; отдельного JS Proxy нет.

## Собственные проверки

tests/concurrency.rs: R59, R60, R61, R17, R35, закрытие, отмена spawn, типы сервисов/hooks, dispatch/tag/filter/global. Внутренние unit tests: R23 dropped Undo без cleanup, R62 отказ actor и Closed для всех waiters; чистое форматирование console time. tests/config.rs покрывает R48 keywords/ref/combinations, накопление путей, ASCII классы JS и UTF-16. Macros: UI и успешная экспансия, см. соседний PORT_NOTES.

## Не проверено

Нет сетевых/внешних проверок; они не нужны ядру. Пояса проверены фиксированными параметрами, без зависимости от TZ машины. Добавлены четыре bounded multi_thread-теста с четырьмя workers (E1) и многопоточный тест атомарности prepend (C12). Остальные асинхронные проверки используют остановленное время.

## Вопросы ведущему

Нет новых вопросов. Решения A реализованы; разногласия C3, C10 и подпункт B3 про неизвестный ключ описаны ниже с источниками.

## Fix round: воспроизведение и результат

До изменений реализации запущены новые тесты: fix_config — 11 passed / 17 failed; fix_lifecycle — 2 passed / 13 failed; unit review — 1 passed / 7 failed / 1 служебный ignored. Отдельно получены красные A7 и trybuild D1/D2 (hygiene не компилировался, View::default неожиданно компилировался). После этого исправлялась реализация. Дополнительные регрессии во время исправления также сначала получены красными: B2 recursive root, B4 вложенный unused $defs, B6 порядок ключей, E1 освобождение последней ссылки до пробуждения dispose.

| Пункт | Тест/результат |
|---|---|
| A1 | Красный a1_realms_are_process_counter_ids; RandomState заменён счётчиком. |
| A2 | Решение принято; C5 расширяет SelfWait на все задачи fiber. |
| A3 | Решение принято; неизвестное слово проверяется при validate, включая unused $defs. |
| A4 | Приняты R40/R42/R26 §10.1; прежние source assertions сохранены. |
| A5 | Дополнения API сохранены; View закрыт (D2). |
| A6 | Типизированная граница сохранена; рекурсивное раскрытие C13. |
| A7 | Красный a7_fixed_timezone_not_utc; jiff, параметр TimeZone. |
| B1 | Красные set/integer-map/signed-NonZero/root-recursion/writeOnly; остальные запрошенные derived-типы тоже проверены valid + invalid с путём. Полный набор ниже. |
| B2 | Красный b2_nonrecursive_refs; следуем нерекурсивным ссылкам в compare и paths; ссылки с циклом остаются raw. Существующий recursive source test не ослаблен. |
| B3 | Восемь красных тестов: JS number, typed integral/range path, exact bounds/multipleOf, type union, schema pattern error, JSON text, value depth, numeric equality. Подпункт unknown-key оспорен ниже. |
| B4 | Красные b4_unused_defs_are_inspected и b4_nested_unused_defs_are_inspected; обход всех объявлений, включая вложенные. |
| B5 | Красный nullable inline Option; null сохраняется. |
| B6 | Красные recursive unknown-fields и key-order; merge идёт по объектам и массивам, не теряет неизвестные поля/позиции ключей. |
| C1 | Два красных unit-теста по 10 000 disposals; завершённые эффекты и child undo удаляются, Weak освобождены. |
| C2 | Два красных теста: late push выполняется с Inactive, adoption только при передаче undo эффекту. |
| C3 | Исходный код уже проходит точный сценарий A accept / B reject / restart. Оспорен; raw B, effective прежний, restart B. |
| C4 | Красный pending-resolver; provisional activation сразу закрыта, регистрация Inactive. |
| C5 | Красные cleanup/spawn/indirect serial wait; ancestry наследуется cleanup/update/spawn/dispatch; внутренний tail restart остаётся разрешён. |
| C6 | Красные filter-panic и actor-failure; catch filter, отмена всех activation tokens, Closed даже для suspended update. |
| C7 | Два красных теста: zero → 1 мс, idle ticks не накапливаются. |
| C8 | Красные outside-runtime dispose и reuse service name/type; synchronous prefix выполняется, async остаток явно логируется; таблица типов освобождается без providers/consumers. |
| C9 | Красный Weak-тест со старым Context; закрытый View очищается, actor держит committed bindings до завершения cleanup. |
| C10 | Исходный код уже проходит точный сценарий stale A.next после retained B. Оспорен; fiber перемонтируется с B, это поведение источника. |
| C11 | Красный loading-withdrawal; notifications отслеживают записи, включая ещё невидимые. |
| C12 | Красный concurrent prepend snapshot; вставка под одним write lock. |
| C13 | Красный nested aggregate; итеративное раскрытие до leaves, порядок сохраняется. |
| C14 | Красный subprocess stack overflow; каждая continuation — задача. 2 000 waterfall и 2 000 update forwarding listeners проходят на 2 МиБ. |
| D1 | Красный trybuild pass; квалифицированные пути/macros, mixed-site identifiers; функции config/ctx/deps/__hoh_ctx. |
| D2 | Новый compile-fail сначала неожиданно компилировался; Default удалён, empty доступен только ядру. |
| E1 | Четыре multi_thread(4) timeout-теста: 64 child/dispose, 256 begin/finish, 128 reserve/close, 4×25 updates. Последний выявил race: dispose reply до Drop node; порядок исправлен. Проверены cleanup, Weak, snapshot, owner effects. |
| E2 | Восстановлены все перечисленные assertions; компонент регистрирует реальный listener, stale failure реально возвращает ошибку (красные fixture-тесты: callback count, один Error, точный latest load failed). Подробности ниже. |
| F | Обновлены оба PORT_NOTES; lifetime, keywords, limits и адаптации перечислены. |

### Разногласия, без изменения исходного поведения

Пути ниже относительно `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/`.

- **C3 disputed:** `vendor/cordis/src/fiber.ts:811` сохраняет raw до handler; `:835` вызывает publish только при успехе, rollback отсутствует. `scripts/cordis-core-lifecycle.spec.ts:144-146` прямо требует raw 3 и effective 2 после rejection. Тест c3_source_retains_rejected_raw прошёл на принятой базе. Исправлять на rollback означало бы менять источник.
- **C10 disputed:** `vendor/cordis/src/fiber.ts:817-819` restart closure безусловно вызывает restart; revision/epoch guards `:822-824` защищают только publish. Упомянутый harness test `scripts/cordis-core-lifecycle.spec.ts:122-148` не вызывает next у старого обработчика и не устанавливает обратного правила. Тест c10_source_stale_delegation_still_restarts прошёл на принятой базе; после принятого без remount обновления B старый A.next перемонтирует fiber с B (load 0, unload 0, load 2).
- **B3 unknown-key wording disputed:** `vendor/schemastery/src/index.ts:811-822` не выдаёт unknown-key error: обычный object сохраняет ключи через merge, strict их отбрасывает. Сообщения, на которое просит заменить brief, в этом checker нет. Не изменён прежний отказ boolean false schema (`expected nullable ...`); deny_unknown_fields по-прежнему отклоняет с путём. Это JSON Schema ограничение Rust-типа, которому нет прямого strict-ошибочного аналога schemastery.

### Набор слов схемы (schemars 1.2.3)

Проверены локальные исходники `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`. Ниже S = `schemars-1.2.3/src/`, D = `schemars_derive-1.2.3/src/`. Настройки — SchemaGenerator::default(), draft2020_12, Deserialize, без transforms; project features preserve_order/indexmap2. Произвольные пользовательские schemars extensions не делают любое слово допустимым.

| Слова | Источник |
|---|---|
| `$schema`, `$defs`, `$ref` (включая `#`), `title` | S generate.rs (draft2020_12, root/subschema generation) |
| `type`, `format`, `minimum`, `maximum`, `minLength`, `maxLength` | S json_schema_impls/primitives.rs:16-134 (числа, char, std strings/IP); std_time.rs, serdejson.rs, ffi.rs используют те же слова |
| `items`, `uniqueItems` | S json_schema_impls/sequences.rs:23-52; indexmap2.rs делегирует этим реализациям |
| `items`, `minItems`, `maxItems`, `prefixItems` | S json_schema_impls/array.rs:19-45, tuple.rs:23-36 |
| `properties`, `additionalProperties`, `patternProperties` | S json_schema_impls/maps.rs:41-141; D schema_exprs.rs (object/enum) |
| `required` | S json_schema_impls/core.rs:43-119, std_time.rs:17-40; D schema_exprs.rs:448-452, 720-755 |
| `not`, `const`, `minimum` | S json_schema_impls/nonzero.rs:13-25 (signed not const 0; unsigned minimum 1) |
| `oneOf`, `anyOf`, `allOf`, `enum`, `const` | D schema_exprs.rs (enum representations, flatten); S json_schema_impls/core.rs (Option, Result, Bound), S _private/mod.rs:49-160, 199-282, 368-386, 449-501 |
| `unevaluatedProperties` | S _private/mod.rs:405-441 (complex flatten/deny_unknown_fields) |
| `readOnly`, `writeOnly` | D ast/mod.rs:123-128 (serde skip annotations; не ограничения значения) |
| `title`, `description`, `examples`, `deprecated` | D attr/mod.rs:197-226, schema_exprs.rs:595 (метаданные/документация; аннотации) |
| `default` | D schema_exprs.rs:680-715, 772-822 (serde default; применяется serde, не validator) |
| `minLength`, `maxLength`, `minItems`, `maxItems`, `minimum`, `maximum`, `pattern`, `format` | D attr/validation.rs:73-116 (length/range/pattern/contains/format), S _private/mod.rs::must_contain (pattern) |

Это опись второго раунда. В третьем раунде дополнительные standard applicators удалены; действующий полный набор с точными строками источников приведён ниже. Исходные R48 numeric constraints сохранены с явно отмеченным разногласием. `x-*` остаются расширениями; bool-схемы поддерживаются. `format` — аннотация R48, кроме диапазонов Rust integer formats для pathed typed conversion (B3).

### E2: уточнённый учёт исходных случаев

- fiber.spec: listener failed fiber удалён, callback ровно один; extra=1 проверено после второго load; resolved config сравнивается целиком. Инерция проверяет Active и значение сервиса (1 до withdrawal / 2 после повторной активации).
- update.spec: до последнего load settled count=0, затем 3; ABA проверяет Active, raw value и identity канонического Arc snapshot (идентичность переданного JS object неприменима к owned Value). Stale failure логируется ровно один раз; оба caller получают точный `latest load failed`; после неудачного push стоит наблюдаемый marker, которого нет в логе.
- service-dispose.spec: sibling cleanup принадлежит тому же blocked consumer; composed case удаляет owner subtree, root остаётся активным, assertions до release сохранены.
- events.spec: проверка удаления последнего bucket перенесена также в private unit e2_empty_buckets_are_removed; публичная проверка callbacks сохранена.
- cordis-timer.spec: оба исходных имени представлены, период и все три операции проверены с реальным pending read; ограничения Stream описаны в учёте файла выше. Дополнительных целиком пропущенных случаев нет.
- Остальные списки ported/not ported исходных файлов не изменились. Старый unsupported-keyword тест использует `unsupported` вместо ставшего допустимым `not`; его проверка отказа на отсутствующем поле сохранена.

### Пределы и жизнь ядра

- Kernel живёт до явного `shutdown()`. Drop всех внешних Kernel/Context/Fiber handles не останавливает actor: он сам держит Shared с sender. Shutdown обязателен для завершения экземпляра; global registry нет.
- JSON nesting — `hoh_util_values::JS_JSON_MAX_DEPTH` (4096 контейнеров), считаются уровни значения, не `$ref` hops. По PORTING потоки приложения имеют стек 64 МиБ; глубина 200 recursive nodes проверена на таком стеке. Произвольно глубокие Value собирать запрещено PORTING; обход значений рекурсивный в этом пределе.
- Схемы с циклом не зацикливают validator; повтор того же schema на том же instance останавливается. Diff идёт по нерекурсивным refs; циклический ref сравнивает всю ссылочную ветвь raw. External refs не разрешаются.
- JSON integers i64/u64 сравниваются точно (i128 intermediate); integral f64 принимается для typed conversion только точно и в пределах ±2^53, затем проверяются пределы Rust integer format. Видимый number/JSON text и numeric equality — hoh-util-values (JS nearest double).
- Неподдерживаемые regex (look-ahead/back-reference и т.п.) — schema error с самим pattern. UTF-16 length и ASCII d/w/b остаются как в source.
- Количество listeners ограничено регистрациями плагинов, фиксированного численного лимита нет. Continuations выполняются отдельными задачами, без линейного роста стека. Тест C14 запускает 2 000 listeners каждого из waterfall/update на 2 МиБ в subprocess; ignored helper не пропущен: его явно запускает обычный wrapper test.
- E1 bounded iterations/timeout 10 с: 64/256/128/4×25. C12 ограничен 30 000 регистрациями; всё без сети. Эти concurrency tests используют настоящий monotonic timeout, а не wall-clock assertions.
- Realm — 64-битный монотонный counter, 0=root; его исчерпание за 2^64 выдач — документированный invariant process lifetime. Fiber uid — u32, binding/revision/generation — u64, по контракту этапа 0. Logger buffer — последние 1 000 records; строка console обрезается до 10 240 UTF-16 units, ts_ms saturating i64.
- interval(0) — 1 мс; нет tick backlog, дедлайн не должен переполнять Instant (переполнение завершает stream). Stream один reader, disposal даёт None; cancellation sleep даёт Inactive. За пределами tokio runtime Disposer выполняет синхронный префикс и пишет Error про невыполненный async остаток, не запускает новый runtime.

## Третий раунд (fix2-stage0-k1)

Изменения ограничены двумя kernel crates. Прежние C3/C10/B3 unknown-key disputes приняты ведущим; R28/R49/R48 integer/R60 уточнены в docs/kernel.md §10.1. Перед изменениями реализации получены красные проверки:

- `c11_loading_withdrawal_notifies`: [false, false] вместо [false] без очистки observations; `hidden_registration_activation_withdrawal_sequence`: ложное уведомление до активации.
- `ready_replies_survive_shutdown`: три уже доставленных результата стали Closed (settled/update/internal restart).
- `optional_integral_float_loads`: serde floating-point error без пути; `optional_integer_format_range_has_path`: overflow Option<u32> без пути.
- `type_release_skips_unrelated_commands`: 11 scans вместо 0 после десяти snapshot-команд. Счётчик обходов и команда чтения существуют только под cfg(test); измеряется сам обход, не wall-clock время.
- `referenced_definition_is_inspected_once`, `recursive_root_is_inspected_once`: два одинаковых замечания вместо одного.
- `settled_active_fiber_allows_marked_serial_wait`: SelfWait для активного, устоявшегося A при dispatch из его spawn-задачи.
- `non_emitted_schema_words_are_unsupported`: checker принимал лишние слова.

### Результат по пунктам brief

1. Только активация/активная регистрация дают available=true, снятие записи всегда даёт false. Скрытая регистрация молчит. Убрана очистка observations. Проверены точные последовательности register→withdraw [false] и register→activate→withdraw [true,false].
2. Готовый reply имеет приоритет над closing token, включая internal restart. Closing token по-прежнему прерывает ожидание, если reply отсутствует; actor-failure/suspended-handler tests сохранены.
3. Integer type в списке обрабатывается так же, как scalar integer: точная конверсия и диапазон format. Null не конвертируется. Замечание формата не дублирует более строгий явный bound. **Частный пример Option<u8>(300) оспорен:** он уже давал правильный путь и текст на принятой базе, поскольку schemars-1.2.3/src/json_schema_impls/primitives.rs:54 и :112 явно дают maximum=255. Никакого изменения этой working bound check нет; `optional_integer_range_has_path` закрепляет результат. Настоящий пропущенный format-bound воспроизведён на Option<u32>(4294967296), где maximum не выдаётся (:114 и :105).
4. `derived_access_annotations_are_emitted_and_load` добавлен и прошёл на принятой базе: production checker уже поддерживал обе аннотации. WriteOnly получается в обычном Deserialize schema; ReadOnly — у производного типа с skip_deserializing при Serialize contract (в Deserialize контракте это поле исключено). Обе схемы проверены без handwritten schema, оба config-типа загружены. Изменений рабочего checker для аннотаций нет.
5. `unknown_key_uses_never_wording_and_path` закрепляет точный Issue для deny_unknown_fields: path=[extra], message="expected nullable but got 7". Тест зелёный до изменений: это принятое **never wording** из vendor/schemastery/src/index.ts:653-655; нового unknown-key wording нет.
6. types.retain вызывается лишь после фактического удаления binding или node в drive. provide/start/status/snapshot и другие команды без удаления обход не вызывают. Unit-тест измеряет 0 обходов read-only команд и один после withdrawal; прежний C8 reuse name/type проходит.
7. Объявление схемы (keywords/pattern/ref validity) проверяется один раз по идентичности location, включая неиспользованные и вложенные defs. Contextual volatile placement анализируется отдельно для distinct (location, restricted, wrapped), максимум четыре состояния; ошибка одного объявления не дублируется. Это сохраняет проверку разных употреблений shared defs, например обычное поле и запрещённые array items.
8. Actor отвечает, устоялся ли fiber, в порядке поступивших команд. Готовый результат возвращается без SelfWait; только реальное ожидание Loading/Unloading проходит ancestry guard. Поэтому queued update перед settled не теряется. `settled_active_fiber_allows_marked_serial_wait` зелёный; `loading_spawn_wait_still_reports_self_wait` и прежний indirect apply/serial guard остаются SelfWait. **Исправлен setup собственного concurrency-теста:** оба прежних SelfWait assertions сохранены, но ancestor теперь действительно Loading parent, ожидающий child; добавлен Ok для уже устоявшегося root. Прежний setup требовал SelfWait от Active root и противоречил новому R60. Источник: vendor/cordis/src/fiber.ts:776-782 ждёт только существующую inertia, иначе возвращает сразу.
9. Удалены 12 недоказанных additions и весь оценивающий их код. unevaluatedProperties сохранён: есть реальная emission S _private/mod.rs:441 и derived witness `derived_flatten_emits_unevaluated_properties`. **Частичное разногласие:** сохранены исходные R48 exclusiveMinimum/exclusiveMaximum/multipleOf. Их natural emission при project features не подтверждена; generic extend не используется как уловка для доказательства. Удалить multipleOf и одновременно сохранить обязательный тест tests/config.rs:5–26 невозможно: schema inspection остановится до требуемых восьми value/path issues. Brief запрещает ослаблять этот тест. Исходная спецификация docs/kernel.md:780–785 прямо требует все три; source vendor/schemastery/src/index.ts:699–704 требует range/step и текст multiple-of. Разногласие закреплено зелёным до изменений `original_r48_numeric_constraints_remain_supported`; реализация этих трёх слов не менялась.
10. Каждый шаг update/event chain исполняется своей задачей. После начала шага Drop ожидающего future не отменяет задачу: шаг продолжает работу, как JavaScript promise. `chain_steps_continue_after_caller_drop` закрепляет event и update ветви. У settled fiber settled() никогда не выдаёт SelfWait (ошибка Failed/Closed по-прежнему возможна по соответствующему состоянию).

Новые тесты 4, 5, 10 — pin/coverage существующего корректного поведения, а не невоспроизведённые production fixes. Нет удалённых или ослабленных assertions второго раунда; исправлен противоречивый ancestor setup и удалена скрывавшая регрессию очистка. Списки исходных ported/not-ported cases не изменились.

### Текущий WORDS: источник каждого слова

S = schemars-1.2.3/src, D = schemars_derive-1.2.3/src в локальном cargo registry, как выше. Строки указывают emission, а не упоминание при обходе/transform. Все 33 emitted слова перечислены; ещё три исходных R48 слова — отдельное разногласие ниже.

| Слово | Emission file:line |
|---|---|
| $schema | S generate.rs:463 |
| $ref | S schema.rs:132 (generator вызов generate.rs:363) |
| $defs | S generate.rs:110 задаёт definitions_path; generate.rs:598–603 вставляет определения |
| title | S generate.rs:459 |
| description | D attr/mod.rs:208 |
| default | D schema_exprs.rs:715 |
| examples | D attr/mod.rs:226 |
| deprecated | D attr/mod.rs:215 |
| readOnly | D ast/mod.rs:123 (Serialize contract witness) |
| writeOnly | D ast/mod.rs:128 |
| format | S json_schema_impls/primitives.rs:33 |
| type | S json_schema_impls/primitives.rs:17 |
| enum | D schema_exprs.rs:150 |
| const | S json_schema_impls/nonzero.rs:22 |
| properties | S json_schema_impls/maps.rs:141 |
| patternProperties | S json_schema_impls/maps.rs:131 |
| required | S json_schema_impls/std_time.rs:17 |
| additionalProperties | S json_schema_impls/maps.rs:111 |
| unevaluatedProperties | S _private/mod.rs:441 |
| items | S json_schema_impls/sequences.rs:25 |
| prefixItems | S json_schema_impls/tuple.rs:29 |
| uniqueItems | S json_schema_impls/sequences.rs:51 |
| minItems | S json_schema_impls/tuple.rs:32 |
| maxItems | S json_schema_impls/tuple.rs:33 |
| minimum | S json_schema_impls/primitives.rs:53 |
| maximum | S json_schema_impls/primitives.rs:54 |
| minLength | S json_schema_impls/primitives.rs:133 |
| maxLength | S json_schema_impls/primitives.rs:134 |
| pattern | D attr/validation.rs:85; S _private/mod.rs:351 (contains string attribute даёт pattern, не contains keyword) |
| anyOf | S _private/mod.rs:94 |
| oneOf | S json_schema_impls/core.rs:47 |
| allOf | S _private/mod.rs:386 |
| not | S json_schema_impls/nonzero.rs:21 |

Kept, legacy R48 dispute: **exclusiveMinimum, exclusiveMaximum, multipleOf** — всего WORDS=36. Расширения x-* и boolean schemas сохранены.

Removed: **contains, minContains, maxContains, if, then, else, dependentRequired, dependentSchemas, propertyNames, $id, minProperties, maxProperties**. Упоминания этих слов в S transform.rs / schema.rs не являются emission. Dynamic schemars extend позволяет любое слово и не расширяет закрытый набор. Их inspection traversal/value evaluation удалены; неизвестное слово даёт schema error до value validation.

Witness unevaluatedProperties использует derived deny_unknown_fields + flattened tagged enum: схема действительно выдаётся schemars и проверяется напрямую (valid и extra-field invalid). Сам serde предупреждает об ограничении сочетания flatten/deny_unknown_fields; из этого witness не заявляется successful typed loading. Обычный flattened struct и strict struct по-прежнему загружаются отдельными прежними тестами.

## Четвёртый круг: добавленный API

Все пути исходников ниже относительно `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/`, если явно не указан upstream Cordis. Эта секция обновлена пятым кругом и предназначена для переноса в docs/kernel.md. Два потребителя, hoh-scope и hoh-credentials, только прочитаны; их код не менялся. Единственное изменение прежнего теста перечислено ниже; остальные сохранены.

### Публичные сигнатуры

```rust
pub struct Contained; // Event::Mode; Mode::DISPATCH == Dispatch::Emit
pub enum ListenerFailure { Threw(Error), Rejected(Error) }
pub struct Callbacks<E: Event<Mode = Contained>>; // opaque

impl Listen<'_> {
    pub fn on_contained<E>(self,
        f: impl Fn(&E) -> Result<(), Error> + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError> where E: Event<Mode = Contained>;
    pub fn on_contained_async<E, Fut>(self,
        f: impl Fn(Arc<E>) -> Fut + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError>
    where E: Event<Mode = Contained>, Fut: Future<Output = Result<(), Error>> + Send + 'static;
}
impl Context {
    pub fn on_contained<E>(&self,
        f: impl Fn(&E) -> Result<(), Error> + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError> where E: Event<Mode = Contained>;
    pub fn on_contained_async<E, Fut>(&self,
        f: impl Fn(Arc<E>) -> Fut + Send + Sync + 'static,
    ) -> Result<Disposer, KernelError>
    where E: Event<Mode = Contained>, Fut: Future<Output = Result<(), Error>> + Send + 'static;
    pub fn dispatch<E: Event<Mode = Contained>>(&self, event: impl Into<Arc<E>>) -> Callbacks<E>;
    pub fn child(&self, name: &'static str) -> Result<Context, KernelError>;
    pub fn matches_filter(&self, listener: &Context) -> bool;
    pub fn and_filter(&self, filter: impl Fn(&Context) -> bool + Send + Sync + 'static) -> Self;
    pub fn effect(&self, label: impl Into<Arc<str>>,
        setup: impl FnOnce() -> Undo,
    ) -> Result<Disposer, KernelError>;
    pub fn try_effect<E: From<KernelError>>(&self, label: impl Into<Arc<str>>,
        setup: impl FnOnce(&Undos) -> Result<(), E>,
    ) -> Result<Disposer, E>;
    pub async fn effect_async<E, Fut>(&self, label: impl Into<Arc<str>>,
        setup: impl FnOnce(Undos) -> Fut,
    ) -> Result<Disposer, E>
    where E: From<KernelError>, Fut: Future<Output = Result<(), E>> + Send;
    pub fn spawn<Fut>(&self, label: impl Into<Arc<str>>,
        task: impl FnOnce(CancellationToken) -> Fut + Send + 'static,
    ) -> Result<Disposer, KernelError>
    where Fut: Future<Output = Result<(), Error>> + Send + 'static;
}
impl<E: Event<Mode = Contained>> Callbacks<E> {
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn invoke(self, report: impl Fn(ListenerFailure) + Send + Sync + 'static);
}
impl Fiber {
    pub fn disposer(&self) -> Option<Disposer>;
}
```

Новые items не заменяют прежние on/emit и пять режимов. Реализация proc macros не менялась: event! уже принимает тип Mode. Для Contained обычные emit/bail/parallel/serial/waterfall/on не компилируются; dispatch не компилируется для остальных режимов. В trybuild добавлен contained_modes (семь отрицательных вызовов).

### Дополнительные правила

**R63. Снимок Contained.** dispatch фиксирует живые на момент разрешения слушатели в исходном порядке с prepend, global и фильтром dispatch-контекста; global обходит фильтр. on_dispatch получает Dispatch::Emit во время dispatch, до вызова слушателей; для internal/* сохраняется прежнее исключение. Добавленный позднее слушатель не входит в снимок; снятый позднее синхронный callback остаётся в нём. len/is_empty описывают этот снимок. Основание: vendor/cordis/src/events.ts:162–184 фильтрует hooks и копирует bound callbacks без последующей проверки removal. Это именно vendored DSH, а не более новый upstream: refs/cordis/packages/core/src/events.ts:97–105 уже проверяет hook.removed. Снимок удерживает callback до invoke/Drop.

**R64. Синхронная изоляция.** Callbacks::invoke потребляет снимок, вызывает синхронные функции по очереди на задаче вызывающего и до следующего callback вызывает report(Threw(error)) при Err. Паника синхронного слушателя тоже Threw: для payload типа &str или String текст ошибки — `event listener panicked: <text>`, для остальных — `event listener panicked`. То же сохранение payload действует для Rejected при панике async factory/future. Остальные слушатели продолжают работу. Паника самого report изолируется и записывается в журнал ядра. Основание: packages/core/agent/src/dispatch.ts:125–135 и packages/core/tools/src/pipeline.ts:418–438 содержат отдельные per-callback try/catch; R61 задаёт границу Rust panic. Snapshot-before-commit следует packages/core/session/src/index.ts:895–907, :978–985.

**R65. Асинхронный Contained.** on_contained_async регистрирует источник promise; вся его ошибка, включая панику future/factory, имеет вид Rejected. Factory и ровно один первый poll future выполняются inline на потоке вызывающего invoke, до следующего callback; poll получает ancestry fiber слушателя. Ready(Ok) заканчивает вызов без задачи и записи эффекта. Ready(Err), паника factory и паника первого poll накапливаются и сообщаются как Rejected после всего snapshot loop, но до возврата invoke: rejection не вклинивается между слушателями. Только Pending передаётся отдельной задаче с ancestry слушателя. Она учитывается эффектом владельца, отменяется его token, при отмене ничего не сообщает; teardown ждёт Drop future. Завершившаяся задача удаляет собственную запись. Если fiber закрыт между dispatch и invoke, frozen factory и первый poll всё равно выполняются; Ready обрабатывается как выше, Pending немедленно уничтожается без задачи и сообщения. report может вызываться из другой задачи и требует Send+Sync. Отсутствие tokio runtime даёт Rejected только для Pending живого слушателя; Ready и синхронные callbacks работают без runtime. Основание: packages/core/session/src/index.ts:413 вызывает async prefix внутри callback; packages/core/agent/src/dispatch.ts:129–134 различает returned-promise rejection и throw. Граница одного inline poll и доставка ready rejection до возврата invoke — решение ext2-stage0-k1 §1; владение/отмена — ext-stage0-k1 A с R17/R60. Обычный emit и его propagation panic не изменены.

**R66. Синхронный дочерний контекст.** Context::child(name) создаёт именованный no-op fiber и сразу возвращает его Context: теги, realms, intercepts и filter унаследованы, эффекты принадлежат ребёнку. Контекст пригоден до первого await, когда state ещё Pending; первый переход в Loading/Active использует ту же активацию, поэтому ранние регистрации не теряются. Если dispose застал ребёнка Pending, наблюдатель видит Pending → Unloading, затем выполняются и ожидаются ранние cleanup, затем Unloading → Disposed. Это исключение из прямого Pending → Disposed в R14: уже есть работа для unload. Исключение из R17 касается открытой активации и регистрации эффектов в Pending; после unload старый Context закрыт, restart по-прежнему получает новую активацию. Гонка с dispose родителя тоже дожидается ранних эффектов. При отказе actor закрывает и освобождает детей с ещё не обработанной Create-командой. Обычный plugin/inject с ещё не разрешёнными Deps по-прежнему не выдаёт активный pending Context. Основание: packages/core/scope/src/index.ts:136–147; vendor/cordis/src/fiber.ts:305–313 явно вызывает _unload и сообщает UNLOADING для pre-activation work, :317–323 ждёт inertia и лишь затем обновляет cached state до DISPOSED (:602–614). Конструктор без блокировки родителем/observer может уже перейти в Loading (:334–346), поэтому тест Pending использует окно dispose родителя до обработки Create ребёнка. Пятый круг подтвердил существующее поведение, менять state machine не потребовалось.

**R67. Точный disposer и покой.** Fiber::disposer выдаёт handle того же родительского эффекта ctx.plugin(), а не новую обёртку. Повторно полученные handles указывают на одну запись; root или уже освобождённая запись дают None. Undo::from(handle) и передача его в Undos::push используют существующее adoption: ребёнок перестаёт быть независимым sibling в списке родителя и занимает точное место ordered composite. У Disposer сохраняется consuming dispose(self). Уже существующий Fiber::dispose() можно вызывать многократно; все вызовы ждут полного R10 teardown, даже если raw disposer уже начал его. Отдельный quiesce API не нужен. Основание: packages/core/scope/src/index.ts:115–118, :143–145; vendor/cordis/src/fiber.ts:294–326. Проверяется порядок асинхронных inner → scope → outer, а не только синхронный call log.

Все входы метки эффекта (effect, try_effect, effect_async, spawn) принимают impl Into<Arc<str>>, запись хранит Arc<str> и печатает исходную метку в cleanup diagnostics. String от format! не требует утечки памяти; строковые литералы и число generic-параметров для turbofish сохранены. Основание: packages/llm/deepseek-llm-api-extensions/src/index.ts:99 строит метку register из fieldName; ext2-stage0-k1 §2.

**R68. Значение listener-контекста.** Нового API для B.3 не требуется: with_tag<T: Send+Sync+'static>(&self, value:T)->Context и tag<T: Send+Sync+'static>(&self)->Option<Arc<T>> уже реализуют ближайшую типизированную метку. Дочерние contexts наследуют её, with_tag того же типа затеняет только производный контекст. Bus хранит Context регистрации; filter получает именно его, а не контекст отправителя. Основание: packages/core/scope/src/index.ts:139–140, :150–151, :174–175; vendor/cordis/src/events.ts:162–173 (hook.ctx).

**R69. Композиция фильтра.** matches_filter(listener) вызывает существующий predicate контекста, либо возвращает true без фильтра; это позволяет переносчику сохранить фильтр другого base Context. and_filter возвращает производный Context: сначала вызывается прежний filter, false прекращает проверку, затем дополнительный filter. with_filter сохраняет прежнюю семантику полной замены. Иммутабельные копии и global bypass сохранены. В Rust захваченный receiver находится в closure; bus изолирует паники фильтра как раньше. Основание: packages/core/scope/src/index.ts:170–183, особенно :173 baseFilter.call(base, ctx), vendor/cordis/src/events.ts:170–171.

**R70. Путь сервисов scope.** Новый child и его потомки при чтении сервисов намеренно наследуют путь создателя: собственная provision → committed view ближайшего объявляющего fiber → предки. Это распространяется и на потомков scoped context: locate в vendor/cordis/src/reflect.ts:145–165 идёт по parents для каждого fiber, а не только по непосредственному scope. Совпадение realm обязательно; явная isolation-граница прерывает проход. Объявление без доступной committed записи также останавливает проход; публичный get сохраняет общий необязательный fallback R36. При teardown child ещё читает удержанную creator binding, пока actor не отпустил её после cleanup; право регистрировать эффекты закрывается немедленно. После удаления child его сохранённый Context не удерживает и не читает старые значения. Путь состоит из плоского списка Weak, Node держит committed view только до R10 release; рекурсивного Drop цепочки нет. Прежние contexts, полученные обычным plugin/inject вне нового child, сохраняют прежнюю семантику get и R17. Основание: packages/core/scope/src/index.ts:127–140; vendor/cordis/src/reflect.ts:145–165, :156–158 уважает isolation. Цитата vendor/cordis/src/fiber.ts:734–759 проверена: :735–740 ждёт dependents, :742–752 ждёт все disposable cleanup, :754–757 удаляет committed dependencies, :759 очищает committed.

### Примеры A и B.1–B.5

A — разделить snapshot, изменение состояния и fan-out; сообщения определяет эмиттер:

```rust
struct Updated;
event!(Updated = "credentials/updated", Contained);
ctx.on_contained::<Updated>(|_| Ok(()))?;
ctx.on_contained_async::<Updated, _>(|_| async {
    // Этот prefix исполняется внутри invoke, до следующего listener.
    observe_committed_state();
    Ok(()) // Ready: отдельная задача и effect не создаются.
})?;
let callbacks = ctx.dispatch(Updated);
// Commit состояния эмиттера здесь, до любого observer.
let log = ctx.logger();
callbacks.invoke(move |failure| match failure {
    ListenerFailure::Threw(error) => log.warn(format_args!("listener threw: {error}")),
    ListenerFailure::Rejected(error) => log.warn(format_args!("listener rejected: {error}")),
});
// Ready rejection уже сообщён после остальных listeners, до этой строки.
```

B.1 — синхронное mint, без handoff и oneshot:

```rust
let scoped = ctx.child("scope")?;
scoped.on::<Ping>(|_| {})?; // уже принадлежит ребёнку, до settled
let fiber = scoped.fiber();
```

B.2 — точное adoption и повторное ожидание:

```rust
let scoped = ctx.child("scope")?;
let fiber = scoped.fiber();
let raw = fiber.disposer().ok_or(KernelError::Inactive)?;
ctx.try_effect::<KernelError>("scope composite", |undos| undos.push(raw))?;
fiber.dispose().await; // полный teardown, даже если raw уже вызван
fiber.dispose().await; // тот же завершённый покой
```

B.3 — метка считывается из listener context:

```rust
struct ScopeTag(u64); // приватный тип пакета hoh-scope
let scoped = ctx.child("scope")?.with_tag(ScopeTag(7));
scoped.on_contained::<Updated>(|_| Ok(()))?;
ctx.with_filter(|listener| listener.tag::<ScopeTag>().is_none_or(|tag| tag.0 == 7))
    .dispatch(Updated).invoke(|_| {});
```

B.4 — фильтр того же Context или отдельного base:

```rust
let routed = base_context.and_filter(move |listener| admits_scope(listener));
// В scope_target base может отличаться от ctx отправителя:
let base = base_context.clone();
let routed = ctx.with_filter(move |listener| {
    base.matches_filter(listener) && admits_scope(listener)
});
```

B.5 — чтение зафиксированной зависимости создателя при cleanup:

```rust
// ctx принадлежит fiber, который объявил Resource в Deps.
let scoped = ctx.child("scope")?;
let reader = scoped.clone();
scoped.effect("read on cleanup", move || Undo::future(move || async move {
    let resource = reader.get::<Resource>().ok_or(KernelError::Inactive)?;
    use_resource(&resource);
    Ok(())
}))?;
```

### Receiver/carrier и границы адаптации тестов

В поддерживаемых agent/tools/session receiver служит фильтром, а реальный subject передаётся payload: packages/core/agent/src/dispatch.ts:113–125 добавляет agent в fused payload; packages/core/tools/src/pipeline.ts:426–429 передаёт exec/result; packages/core/session/src/index.ts:895–899 передаёт session/event. Они не читают carrier key внутри listener, поэтому нового receiver/key API здесь нет и событие не обязано дублировать receiver key. Единственное найденное production-чтение carrierKeyOf(this) — packages/api/remotes/src/index.ts:60–69: это Typert remote transport, исключённый PORTING §8; в рамках поддерживаемых consumers оно не требует расширять Event trait.

Два [U] dispatch case names добавлены в tests/contained.rs. **Различие источников явно сохранено:** deprecated_dispatch_returns_bound_callbacks_that_respect_later_removal проверяет vendored frozen callback — снятый после snapshot sync callback исполняется, новый dispatch его уже не видит. Более новый [U] ожидает пропуск, но vendor/cordis/src/events.ts:170–184 этого guard не содержит. deprecated_waterfall_dispatch_continues_past_a_removed_callback проверяет snapshot/removed/next через существующий typed waterfall: новый dispatch контрактно доступен только Contained, поэтому дословный raw waterfall dispatch не представим. Обычный waterfall по-прежнему пропускает снятый listener, как до этого раунда. Не заявляется, что исходные bail-return/JS this/Symbol assertions можно выразить Callbacks::invoke()->().

Дополнительные тесты: девять contained cases (ошибки/паники, порядок, snapshot/filter/global/prepend, отмена и ancestry), девять scoped_context cases (sync ownership, pre-activation teardown, async ordered adoption, quiescence, base-filter/tag/realm/intercept, service release через Weak). Три unit cases проверяют точность Arc disposer, pre-Create actor failure и отсутствие накопления после 1 000 async invocations. Существующие 241 tests не изменены. Новый private failure test сначала воспроизвёл отсутствие cancel и удержание callback cycle; eager children теперь учитываются до Create, а registry undo использует Weak listener и держит временный strong handle вне bus locks во время снятия.

### Пятый круг: воспроизведение и закрытие ext2-stage0-k1

1. `tests/ext2.rs::async_prefix_and_ready_rejection_run_inside_invoke` сначала получил `[A, C]` вместо `[A, B, C, rejected]` на прежней реализации; `disposed_snapshot_still_polls_async_prefix` получил только `[sync]`. Исправлен первый inline poll и отложенная до конца snapshot доставка ready rejection. Теперь второй тест также проверяет ready rejection уже закрытого listener. Unit `inline_completion_reserves_no_task_record` проверяет ровно одну оставшуюся запись регистрации слушателя.
2. `dynamic_label_is_preserved_in_cleanup_diagnostics` сначала не собирался: String от format! не подходил к &'static str. После расширения всех четырёх публичных входов метки проверяется её точный текст в журнале cleanup. Старые literal/turbofish вызовы сохранены в прежних тестах.
3. Гипотеза о неверном Pending → Unloading оспорена источником: fiber.ts:305–323 требует этот переход и cleanup до Disposed. `pending_child_uses_source_unloading_status_and_drains_early_effects` сохраняет живого root-наблюдателя, явно проверяет Pending до собственного child.dispose() и проверяет `[Pending->Unloading, cleanup, Unloading->Disposed]` без изменения state machine. Валидного красного теста для заявленного дефекта получить нельзя. Первоначальная проба через shutdown root теряла финальный status потому, что сам наблюдатель root уже снят; этот setup исправлен, его отказ не считается подтверждением дефекта. Исключения из R14/R17 и точные строки источника включены в R66.
4. Unit `ancestor_barriers_and_realm_mismatch_stop_at_published_replacement` удерживает creator binding 1 в cleanup, публикует replacement 2 и проверяет plain=1, barrier=2, realm mismatch=2. Без каждого соответствующего break вернулось бы 1; это не прежнее слабое сравнение с None после withdrawal. Для независимой проверки realm guard тест меняет приватный ancestor realm без установки barrier (публичный isolate меняет оба). Unit `disposal_releases_a_hundred_pending_contained_tasks` проверяет 100 Pending задач, завершение dispose, пустые effect lists ребёнка/родителя и отсутствие сообщений при отмене. Non-root listener и factory panic покрыты `non_root_factory_and_future_panics_preserve_payloads`.
5. Тот же тест различает Threw/Rejected, проверяет &str, String, fallback для другого payload и панику future после Pending. R64/R65/R66/R70 и пример A обновлены на месте. Цитата :270 убрана; :734–759 проверена по реальному порядку cleanup/release.

Единственный изменённый прежний тест: `tests/contained.rs::reports_threw_before_next_call_and_rejected_from_async_task` — в ожидание сразу после invoke добавлен `rejected: async error`: по новому R65 ready rejection доставлен до возврата. Имя сохранено. Прежний `tests/scoped_context.rs` не менялся; сильные проверки добавлены отдельно. Других прежних тестов не изменено, новых неперенесённых случаев нет.

Запись завершившегося Pending-вызова по-прежнему удаляется линейным detach из упорядоченного списка активации; изменения структуры ради необязательной оптимизации не делались. Ready больше не создаёт эту запись. Выбор динамического имени события остаётся работой потребителя. Нового API события или изменения macros нет.

## Проверки

Все команды: `CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-k1`.

- `cargo test -p hoh-kernel`: 270 passed; 0 failed; 1 ignored (повторный запуск после возобновления 2026-11-18, сумма итоговых строк 26 test binaries; doc-tests 0/0). Ignored — только C14 subprocess helper, фактически исполненный wrapper test на 2 МиБ.
- `cargo test -p hoh-kernel-macros`: 1 passed; 0 failed (13 compile-fail + 2 pass trybuild fixtures; doc-tests 0/0).
- `cargo clippy -p hoh-kernel -p hoh-kernel-macros --all-targets -- -D warnings`: exit 0.
- `cargo fmt -p hoh-kernel -p hoh-kernel-macros -- --check`: exit 0.
- `cargo check --workspace --all-targets`: exit 101 вне границ ядра. Последующий повтор остановился на `hoh-conformance`: объявлены, но ещё не созданы `src/main.rs` и `src/bin/fake.rs` (4 target resolution errors); после ожидания повтор с `--keep-going` подтвердил то же. Ранее полный `--keep-going` нашёл известный отсутствующий `hoh-attachment-local/tests/index.rs` (lib.rs:439) и две E0308 в `hoh-storage-domain/tests/domain.rs:468,491` (Result<Value, _> вместо Value). Последнее исправлено параллельным писателем: повторная проверка ниже проходит и этот crate. В раннем запуске у hoh-loader было предупреждение unused `entry` (runtime.rs:1100). Чужие crates этим писателем не менялись.
- `cargo check --workspace --all-targets --exclude hoh-conformance --exclude hoh-attachment-local --keep-going`: exit 0, `Finished dev profile … in 28.26s`. Проверены все остальные targets, включая hoh-scope, hoh-credentials, hoh-storage-domain, hoh-llm и hoh-loader; ошибок совместимости с новым API нет.
