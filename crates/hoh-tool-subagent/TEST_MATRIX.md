# Матрица тестов TypeScript → Rust

Исходник: `packages/subagent/tool-subagent/tests` @ `39163dd3`. Всего исходных случаев: **113**.
Обозначения тестов: `exec::` — `tests/execution.rs`, `sel::` — `tests/selection_runtime.rs`,
`rt::` — `tests/runtime.rs`, `rv::` — `tests/review_213559.rs`, `pure::` — `tests/pure.rs`, `lib::` — unit-тесты `src/*.rs`.
Общая обвязка (скриптовый провайдер, среда, адаптер с воротами пред-проверки маршрута) лежит в
`tests/support/mod.rs`. Строка «отложено» — случай не перенесён, с причиной.

## tool-subagent.spec.ts (36)

| случай | тест Rust |
|---|---|
| rejects configured child agent options at mount | `exec::rejects_configured_agent_options_when_the_provider_cannot_apply_them` |
| starts a managed subagent and returns its name | `exec::starts_a_managed_subagent_and_returns_its_name_without_collecting_output` |
| resolves an explicit child directory | `exec::resolves_an_explicit_child_directory_against_the_parent_directory` |
| exposes only task inputs | `rt::mounted_tool_has_exact_required_surface_and_activation_output` |
| classifies delegations as concurrency-safe | `rt::mounted_tool_has_exact_required_surface_and_activation_output` |
| overlaps sibling delegations | `exec::overlaps_sibling_delegations_dispatched_concurrently` |
| configurable toolName | `exec::registers_under_a_configurable_tool_name_so_providers_coexist` |
| skips startup when cancellation wins route preflight | `exec::skips_startup_when_cancellation_wins_asynchronous_route_preflight` |
| rejects startup when the provider changes during preflight | `exec::rejects_startup_when_the_provider_changes_during_route_preflight` |
| merges model overrides over provider route defaults | `exec::merges_model_overrides_over_provider_route_defaults_before_preflight` |
| does not inherit parent effort for provider default | `exec::does_not_inherit_parent_effort_for_a_provider_route_default` |
| defaults toolName / omits agentOptions on direct apply | `pure::config_defaults_tool_and_selection_switches`, `exec::passes_max_depth_through_to_the_start_request_and_omits_agent_options` (прямой `apply()` в обход схемы в Rust не существует: конфигурация типизирована) |
| fails loud without a calling agent | `rt::call_without_agent_fails_before_provider_start` |
| registers when the provider appears later | `exec::registers_when_the_provider_appears_later_and_ignores_other_providers` |
| keeps continuable guidance empty while provider absent | `exec::description_offers_follow_up_messages_only_for_a_continuable_provider` (описание строится при монтировании; без провайдера инструмента нет — `rt::provider_lifecycle_mounts_and_removes_the_tool`) |
| shares delegation guidance across visible tools | `exec::shares_one_guidance_paragraph_across_visible_delegation_tools`, `rt::guidance_is_visible_only_while_own_definition_is_visible` |
| shares guidance only among the viewing agent's visible definitions | отложено: guidance отбирает инструменты эвристикой `is_delegation_definition`, идентичность определений не перенесена (PORT_NOTES, «видимые delegation-инструменты»); тест проверял бы эвристику |
| mirrors the provider lifecycle | `rt::provider_lifecycle_mounts_and_removes_the_tool`, `exec::registers_when_the_provider_appears_later_and_ignores_other_providers` |
| plugin fiber owns its lifecycle listeners | `exec::disposing_the_plugin_fiber_unmounts_the_tool` (гонка «zombie-mount» после выгрузки: слушатель снимается с волокном — проверяется последней строкой теста) |
| ignores lifecycle events for other providers | `exec::registers_when_the_provider_appears_later_and_ignores_other_providers` |
| fresh-conversation wording / inherited wording | `pure::provider_wording_distinguishes_fresh_and_inherited_history`, `exec::description_offers_follow_up_messages_only_for_a_continuable_provider` |
| skips startup for an already-aborted signal | `exec::skips_provider_startup_for_an_already_aborted_signal` |
| no `subagent` tool without ctx.subagents | не применимо: `Subagents` — обязательная зависимость (`Deps`), ядро не монтирует плагин без неё |
| namespace-plugin export shape | `lib::namespace_and_model_facing_defaults_match_the_source` |
| passes maxDepth through | `exec::passes_max_depth_through_to_the_start_request_and_omits_agent_options` |
| validates maxDepth at initialization | `lib::max_depth_accepts_safe_integers_and_provider_managed_only` |
| omitted agentOptions not materialized | `exec::passes_max_depth_through_to_the_start_request_and_omits_agent_options` |
| classifies local activations concurrency-safe | `rt::mounted_tool_has_exact_required_surface_and_activation_output` |
| starts a local activation, returns only its name | `exec::starts_a_managed_subagent_and_returns_its_name_without_collecting_output` |
| requested name; duplicate sibling rejected | `exec::starts_a_child_under_a_requested_name_and_rejects_a_sibling_with_the_same_name` (проверка очистки отвергнутого ребёнка — в `hoh-subagent`) |
| hides continuable guidance when agent cannot see the tool | отложено: как «shares guidance only among viewing agent» (видимость по областям) |
| isolates cancelled continuable preparation from sibling | отложено: путь `prepare_continuable` принадлежит `hoh-subagent`; нужен скриптовый continuable-провайдер, которого у крейта нет |
| defaults maxDepth to 1 and forwards it | `exec::defaults_max_depth_to_one_and_forwards_the_label` |
| rejects numeric maxDepth without depthLimit | `exec::rejects_a_numeric_max_depth_on_a_provider_without_the_depth_limit_capability` |
| 'provider-managed' omits the cap | `exec::provider_managed_omits_the_cap_so_a_capability_less_provider_mounts_and_starts` |

## model-selection.spec.ts (22)

| случай | тест Rust |
|---|---|
| rejects empty route ids | `pure::route_policy_rejects_empty_provider_and_model` |
| checks complete explicit routes against policy or parent | `pure::absent_policy_allows_only_parent_route`, `pure::recorded_policy_replaces_parent_fallback` |
| leaves defaults outside the allowlist usable | `pure::omitted_request_preserves_configured_options` |
| exposes route fields and discovery when enabled | `sel::exposes_route_fields_and_discovery_when_selection_is_enabled` |
| rejects route arguments without Agent options support | `sel::rejects_route_arguments_for_a_backend_without_agent_options_support` |
| hides and rejects route fields without modelSelection | `sel::hides_and_rejects_route_fields_without_model_selection` |
| discovery guidance only while its tool is visible | отложено: видимость инструмента по области и порядок монтирования (см. выше) |
| settings-owned route fields without a Session policy | отложено: нужен Session без записанной политики и настройки хоста одновременно; логика проверена `pure::absent_policy_allows_only_parent_route` |
| discovers only the parent route without policy | `rv::resolves_only_the_current_parent_model_without_a_session_policy` |
| rejects enabled selection when provider cannot apply options | `sel::rejects_enabled_model_selection_when_the_provider_cannot_apply_agent_options` |
| unlisted complete route clears configured effort | `pure::explicit_route_overlays_config_and_clears_effort` |
| compares explicit routes with latest logged parent selection | отложено: нужна смена выбора модели родителем через событие сессии |
| rejects an effort without any effective route | `pure::effort_without_complete_route_has_specific_diagnostic` |
| rejects preflight without effective provider and model | `review_214605::preflight_without_an_effective_provider_and_model_is_rejected` (прямой вызов), `a_configured_effort_with_a_parent_without_a_route_fails_the_call_before_start` (через инструмент), `selected_routes_are_rejected_without_the_llm_service`, `a_configured_effort_is_rejected_without_the_llm_service`, `a_configured_unknown_effort_is_validated_before_child_creation` |
| LLM runtime validates provider and effort before child creation | `exec::merges_model_overrides_over_provider_route_defaults_before_preflight` (пред-проверка вызывается), `sel::propagates_an_exact_route_resolver_failure_before_child_creation` |
| validates configured effort before child creation | отложено: адаптер-заглушка не публикует список усилий рассуждения |
| validates configured route before child creation | `exec::skips_startup_when_cancellation_wins_asynchronous_route_preflight` (настроенный маршрут проходит пред-проверку) |
| rejects selections when the LLM service is absent | отложено: тестовая среда всегда монтирует `llm` |
| pure inherited routing without resolving a route | `sel::pure_inherited_routing_does_not_resolve_an_llm_route` |
| fork tool enforces Session authorization | отложено: fork-провайдер — `hoh-subagent-fork-in-process` |
| fork selection unavailable without opt-in | отложено: то же |
| propagates resolver failure | `sel::propagates_an_exact_route_resolver_failure_before_child_creation` |

## preset-selection.spec.ts (9)

| случай | тест Rust |
|---|---|
| default without model selector; default when omitted, rejects unlisted | `pure::preset_resolution_applies_default_and_allowlist`, `pure::model_preset_selection_disabled_is_distinct_from_default` |
| rejects forced selection at the JSON resolver | `pure::preset_metadata_matches_omission_behavior` |
| requires the preset service | `pure::preset_requires_independent_supported_provider`; ветка «нет `agentPresets`» — `src/lib.rs` (`agent preset selection requires the agent-presets service`) |
| duplicates / missing default | `pure::preset_configuration_rejects_duplicates_and_missing_default` |
| unloadable child preset via tool result; preset that selects itself; sharing across Agents; schemas after preset switch; missing child reference | отложено: требуют реестра пресетов `hoh-agent-preset-registry` с загрузкой дочернего агента из пресета и переключением пресета; провайдер этого крейта в таких сценариях только передаёт `agentPreset` в `start` |

## model-selection-settings.spec.ts (14), model-selection-initialization.spec.ts (1)

| случай | тест Rust |
|---|---|
| default without settings provider; defaults off / validated layer | `tests/model_selection_settings.rs`, `pure::settings_default_off_with_no_allowed_routes` |
| duplicate routes, empty settings, empty durable policy | `pure::route_policy_rejects_exact_duplicates_only`, `pure::policy_projection_is_first_event_wins_and_rejects_empty` |
| samples each new root Session; inherits parent decision; seeded decisions | `rt::model_selection_setting_records_fresh_policy_and_child_inherits_it`, `tests/model_selection_initialization.rs` |
| direct Agent setup before publication; blank Session joins a preset; registry refresh; shared definitions; inherited-tool filter; independent tool without settings; saved default depth per delegation | отложено: требуют пресетов и обновления реестра инструментов (см. preset-selection), а «saved default depth» — настройки хоста `subagent depth`, живущие в `hoh-subagent` |
| rejects a forced route outside the Session policy | `pure::explicit_selection_requires_enabled_instance`, `pure::absent_policy_allows_only_parent_route` |

## list-models.spec.ts (20)

| случай | тест Rust |
|---|---|
| defaults discovery off | `tests/list_models.rs` |
| model lines / descriptions; unavailable provider alternatives | `lib::model_lines_preserve_optional_descriptions_byte_for_byte`, `lib::missing_provider_reports_only_registered_authorized_alternatives` |
| schema and wording | `rv::registered_definition_is_byte_identical_to_the_source` (определение, которое регистрирует `register_list_subagent_models`; прежний тест-литерал удалён) |
| registered only on its owner, removed on unload | `rv::registers_discovery_only_on_its_configured_owner_and_removes_it_on_unload` |
| second owner rejected | `rv::rejects_a_second_discovery_owner_in_the_same_scope_without_removing_the_first` |
| scope-local registration | `rv::keeps_discovery_registration_and_disposal_local_to_each_tool_scope` |
| requires calling agent and parent route (порядок: агент, маршрут, затем llm) | `rv::list_without_agent_and_without_llm_reports_agent_first`, `rv::list_without_parent_route_and_without_llm_reports_the_route_first` |
| stays registered without the LLM service | `rv::discovery_stays_registered_without_llm_and_rejects_calls` |
| recorded allowlist; live provider changes; one provider's models; allowlist intersection; unauthorized provider; empty catalog; unlisted/removed models; failed lookup; cancellation after lookup; parent-only without policy (оба варианта каталога); exact-model efforts; models without reasoning metadata; incomplete requests; empty exact model; alternatives for unavailable provider; none available | одноимённые тесты `tests/review_213559.rs` (`reads_the_recorded_session_allowlist`, `lists_registered_providers_and_follows_live_registration_changes`, … `reports_no_available_provider_when_the_authorized_registry_intersection_is_empty`) |

## depth-context.spec.ts (4), scripted-provider.spec.ts (7)

| случай | тест Rust |
|---|---|
| depth: warns at the boundary; follows the visible definition; disposes with the plugin | `rt::max_depth_context_warns_at_the_boundary`, `rt::guidance_is_visible_only_while_own_definition_is_visible`, `exec::disposing_the_plugin_fiber_unmounts_the_tool` |
| depth: runtime depth deeper than saved header; omits unknown limits and agentless diagnostics | `tests/depth_context.rs` (числовая и `provider-managed` формы) |
| scripted-provider.spec (7) | не переносится как набор: это проверки тестового вспомогательного файла `scripted-provider.ts`, которого нет в `src`. Рабочий аналог (`tests/support/mod.rs::Scripted`) проверяется самими тестами `exec::*`; контракт имени и возможностей — `tests/scripted_provider.rs` |
