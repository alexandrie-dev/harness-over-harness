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
| vendor/schemastery, loader/src/config/diff.ts | src/config.rs | закрытый набор JSON Schema, volatile comparison |

MIT LICENSE всех четырёх vendor пакетов совпадает до байта; сохранён в LICENSE.

## Отличия от спецификации

1. `Fiber::dispose()` сохраняет `Future<Output = ()>`. При ожидании своего fiber/предка из apply команда отправляется, ожидание завершается немедленно, SelfWait пишется в журнал: вернуть Err с таким Output невозможно. Остальные ожидаемые переходы возвращают SelfWait. Методы с результатом (), Option, bool или Snapshot также не могут вернуть Closed: после остановки дают идемпотентный/пустой результат; Result-операции дают Closed.
2. `Realm::new()` использует случайное 64-битное значение стандартного RandomState без глобального счётчика. Абсолютная уникальность в процессе несовместима с запретом глобального изменяемого состояния и сигнатурой без Kernel; вероятность коллизии ненулевая (63 случайных бита, ROOT исключён).
3. `PluginDef::of()` не может вернуть ошибку R48 при контрактном результате PluginDef. Неизвестное слово схемы даёт ValidationError при первой проверке, включая слова в отсутствующих полях.
4. Обычный Emit распространяет панику слушателя: upstream events.spec «ctx.emit()» проверяет исключение. Это приоритет теста над R40. События ядра и VolatileUpdate изолируют панику каждого наблюдателя, R61 сохранено.
5. Асинхронные retained-instance update handlers могут завершаться одновременно; публикация защищена идентичностью raw-снимка, поколением и закрытием активации. dsh core-lifecycle «publishes successful retained-instance updates and ignores stale or rejected completions» ждёт второе обновление до освобождения первого, что несовместимо с сериализацией R42. Приоритет теста; эпохи и переходы остаются строго последовательными.
6. R26 уточнено по isolate.spec и core-lifecycle: собственный поставщик читает свою уже зарегистрированную запись в Loading. Другие fiber видят её только в Active. Запись и отмена предоставления по-прежнему команды actor.
7. Добавлены `Fiber::retain_raw_config(raw)` (командный аналог rawConfig setter в dsh tests) и `Listen::on_update` (prepend в events.spec). Иначе соответствующие исходные случаи не выразить контрактным API.
8. Добавлен скрытый `View::service<K>()`: derive из другого crate должен читать committed view. Конструктора Service нет. ConsoleConfig публичен как ассоциированный Config ConsoleExporter. Volatile имеет Default при T: Default.
9. Универсальное `Logger::error(impl Display)` не позволяет безопасно распознать KernelError без дополнительного Any/'static bound, нарушающего вызовы с заимствованным format_args. R57 реализовано на типизированной границе логирования ошибок ядра; прямой Logger::error(Listeners) печатает одну aggregate-запись. Исходный тест flatten проверяет эту границу через ошибку apply.
10. ConsoleExporter форматирует showTime в UTC: безопасного источника локальной временной зоны среди разрешённых зависимостей нет. ts_ms — Unix milliseconds, привязанные к tokio Instant; пауза времени детерминирована.
11. Trybuild — синхронный compile-only #[test], поскольку macros crate не имеет разрешённой зависимости tokio. Все проверки поведения — #[tokio::test(start_paused = true)].
12. Volatile placement и повторная обёртка проверяются до проверки значения (в том числе при отсутствующем поле), по volatile-config source; JSON Schema несёт x-volatile и служебную x-volatile-invalid для повторной обёртки.

## Отличия от образца

Типизированные Rust API, отмена активации CancellationToken, неизменяемые производные контексты, отсутствие Proxy/HMR, остановка после shutdown — решения спецификации. JSON defaults задаёт serde; Validator не исполняет TS transforms/autofix. Числа и UTF-16 строки форматируются локально: workspace-зависимость hoh-util-values запрещена заданием.

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

Перенесены (1): `settles_all_pending_interval_reads_on_return_throw_dispose`.

Не перенесено / адаптация: completes concurrent interval reads in order, one per tick — один читатель Stream. stops throttle calls and trailing work after disposal (noTrailing=%s); cancels debounce work and makes both wrappers inert after owner disposal — O5/HMR.

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

Нет сетевых/внешних проверок; они не нужны ядру. Настоящий локальный timezone ConsoleExporter не реализован (UTC выше). Абсолютная уникальность Realm не доказуема вероятностным генератором. Многопоточная модель использует Send + actor и короткие Mutex/RwLock; тесты с остановленным временем работают в current-thread runtime.

## Вопросы ведущему

- Разрешить атомарный процессный счётчик Realm либо сохранить вероятностную уникальность?
- Нужен ли dispose Result для возврата SelfWait и Result<PluginDef> для ошибки схемы непосредственно в of?
- Утвердить приоритет source tests над R40/R42/R26 и дополнения retain_raw_config/Listen::on_update?
- Расширить типизированный вход журнала для полного R57 при прямом логировании, и нужен ли локальный timezone для ConsoleExporter?

## Проверки

Все команды: `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-k1`.

- `cargo test -p hoh-kernel`: 165 passed; 0 failed (сумма итоговых строк 20 test binaries; doc-tests 0/0).
- `cargo test -p hoh-kernel-macros`: 1 passed; 0 failed (11 compile-fail + 1 pass trybuild fixture; doc-tests 0/0).
- `cargo clippy -p hoh-kernel --all-targets -- -D warnings`: exit 0.
- `cargo clippy -p hoh-kernel-macros --all-targets -- -D warnings`: exit 0.
- `cargo fmt -p hoh-kernel -- --check`: exit 0.
- `cargo fmt -p hoh-kernel-macros -- --check`: exit 0.
