# hoh-mcp-manager ← packages/mcp/mcp-manager

## Соответствие

- `src/index.ts` перенесён в `src/config.rs` и `src/runtime.rs`; `src/types.ts` — в `src/types.rs`.
- `locale/*.json` скопированы без изменений.
- Менеджер сохраняет порядок configured/registered записей, применяет volatile-конфигурацию без remount самого менеджера, сериализует reconcile/restart и блокирует новые поколения после неподтверждённого cleanup.
- Регистрация принадлежит вызывающему `Context`; перед сохранением она принудительно получает `enabled = true`, как `McpServerRegistration` в TypeScript.
- Probe принадлежит `ManagedRecord`: изменение конфигурации, failed startup и dispose сначала отменяют и дожидаются всех его cleanup-веток. Первый `cleanup_error` и любой `closeConfirmed = false` сохраняются.
- Dispose закрывает watch в момент остановки manager, до ожидания дочернего cleanup.
- Публичные diagnostics для restart/probe/changing/cleanup/registration и восьми transport-validation веток совпадают с `src/index.ts`.
- Coalesced A→B→A не оживляет retired snapshot: `changing` всегда требует полного unmount, после которого reconcile создаёт новое поколение.
- Cancellation manager немедленно и синхронно ставит `stopped`, снимает status listeners, отменяет probes/lifetime и закрывает watch, даже если actor ещё ждёт долгий child cleanup; actor затем завершает сериализованный teardown.
- После общей Schemastery-фазы (startup/close timeout и независимые reconnect bounds) каждая запись проверяется строго как TS `validateEntries`: имя → transport → templates → cross-field reconnect. Поэтому schema-ошибка поздней записи опережает неверное имя ранней, но неверное имя опережает transport/template ошибку своей или следующей записи.
- `toolCallTimeoutMs` сохраняет исходный контракт `z.number()` без manager-level positive bound; startup/close bounds остаются прежними.
- `restart` записи с retained cleanup failure всё равно делает последнюю попытку dispose живого child Fiber и лишь затем возвращает cleanup diagnostic (`restart_disposes_a_live_fiber_before_reporting_retained_cleanup_failure`).

## Rust-адаптация

- `AsyncIterable` представлен `tokio::sync::watch::Receiver<Vec<McpManagedServerView>>`.
- Методы регистрации получают `&Context` явно, потому что Rust service value не несёт неявный caller context.
- Typert `Remote`-декораторы не переносятся согласно `PORTING.md`; публичные async-методы являются тем же локальным контрактом для будущего JSON-RPC projection.
- Live integration tests запускают отдельный harness-free test binary как stdio MCP-сервер и проходят настоящее `rmcp` negotiation/list-tools через production child transport. Start-log, delayed handshake и credential provider gates наблюдают поколения и races без test-only API в `hoh-mcp-client`; отдельный binary не пишет служебный вывод libtest в protocol stdout.
- В unit-тестах lifecycle дополнительно используется детерминированный gate на внутреннем probe-work для cleanup-результатов, которые реальный корректный сервер намеренно не производит.
- `tokio::watch` с уже непрочитанным snapshot может один раз вернуть `Ok(())` после закрытия sender; это отличие push-итератора TS. Новых snapshots после `signal_stop` manager не публикует.

## Инвентаризация source tests

Обозначения: **P** — перенесено в manager suite; **A** — адаптировано на границе Rust-архитектуры; **D** — относится к Loader/Settings-композиции и оставлено у владельца по решению ведущего от 20.11. Test-only seam в `hoh-mcp-client` не добавлялся: live manager cases используют настоящий stdio transport и локальный `rmcp` server.

| TS case (`manager.spec.ts`) | Статус | Rust evidence / причина |
|---|---:|---|
| 195 composition-base without Settings | P | `live_stdio_server_mounts_probes_restarts_reconfigures_and_unmounts`: live mount без Settings, status `connected`, tool опубликован. |
| 209 Settings user layer | A | `volatile_add_change_and_delete_keep_the_fiber`; persistence/profile layer принадлежит Loader/Settings. |
| 218 changed entry rebuild + unchanged sibling | P | `live_reconfiguration_rebuilds_only_the_changed_server`: раздельные start-log доказывают новое поколение только changed entry. |
| 238 disable/re-enable live instance | P | `live_stdio_server_mounts_probes_restarts_reconfigures_and_unmounts`: tool снимается и возвращается новым процессом. |
| 256 delete live instance | P | Тот же live test: entry и tool исчезают после volatile delete. |
| 267 restart rebuild | P | Тот же live test: start-log увеличивается, status/tool восстанавливаются; unknown ветка — `restart_rejects_unknown_and_reaffirms_disabled`. |
| 279 credential rotation without Fiber expansion | P | `live_credentials_rotate_without_expanding_manager_config_and_recover_after_unset`: raw Fiber config остаётся template, tool меняется после real reconnect. |
| 314 same-value credential notification | P | Тот же test: одинаковое значение не увеличивает start-log и не заменяет tool. |
| 346 HTTP rotated header | A | Manager→client credential/rebuild wiring покрыто live stdio rotation; HTTP header transport generation принадлежит `hoh-mcp-client` и покрыта его `credential_events_preserve_same_connection_and_missing_value_disconnects`/HTTP config tests. Manager не ветвится по месту template после `client_config`. |
| 365 credential unset/restored removes tools | P | `live_credentials_rotate_without_expanding_manager_config_and_recover_after_unset`: `waiting-credentials`, tool removal и real recovery. |
| 383 queued credential update after provider failure | P | `credential_refresh_rechecks_a_queued_update_and_removal_wins_a_late_rebuild`: gated failed resolve, queued notification, recovery и проверка отсутствия secret в log. |
| 418 dispose overtakes credential resolution | P | `removal_and_disposal_overtake_real_gated_credential_resolution`: manager dispose ждёт/отменяет gated real child activation и не запускает процесс после release. |
| 459 missing credential provider | P | `credential_template_without_provider_reports_the_client_diagnostic`: exact production diagnostic и отсутствие child start. |
| 476 static serverName collision | P | `real_client_server_name_collision_is_reported_without_disturbing_static_instance`: статический live client остаётся опубликован, manager сообщает load failure. |
| 498 probe leaves live instance untouched | P | `live_stdio_server_mounts_probes_restarts_reconfigures_and_unmounts`: отдельный настоящий probe возвращает tool, managed tool/status остаются живы. |
| 515 delete cancels probe + retains failed cleanup | P | `config_removal_waits_for_a_probe_only_record`, `probe_cleanup_failure_cannot_be_overwritten_by_a_late_status`. |
| 540 caller abort does not close live connection | P | `aborting_a_real_probe_does_not_close_the_managed_connection`: второй stdio process зависает на list-tools, caller abort завершает только probe; managed status/tool сохранены. |
| 565 probe cleanup survives later connection status | P | `probe_cleanup_failure_cannot_be_overwritten_by_a_late_status`. |
| 584 concurrent probes, firstConfirmed=false | P | `cleanup_composition_keeps_the_first_error_and_any_unconfirmed_close`. |
| 584 concurrent probes, firstConfirmed=true | P | `cleanup_composition_keeps_the_first_error_and_any_unconfirmed_close`. |
| 638 managed unconfirmed close after probe error + removal | P | State composition покрыта `cleanup_composition_keeps_the_first_error_and_any_unconfirmed_close`; removal wait — `config_removal_waits_for_a_probe_only_record`. |
| 669 probe error + later live closure failure | P | `cleanup_composition_keeps_the_first_error_and_any_unconfirmed_close`. |
| 699 captured status callback after dispose | P | `real_delayed_start_cannot_publish_after_reconfiguration_or_disposal` плюс `stop_closes_watch_before_probe_cleanup_settles`: late real activation/status ничего не публикует после dispose. |
| 717 incomplete client cleanup blocks replacement | P | `unclosed_generation_keeps_its_public_reason_and_blocks_replacement`, exact cleanup diagnostic — `public_error_texts_match_the_source`. |
| 739 startup rejection after dispose | P | `failed_startup_waits_for_probe_cleanup_before_load_failure` плюс `stopped` fence; wire rejection не дублируется. |
| 755 queued startup failure after removal | P | `config_removal_waits_for_a_probe_only_record`, `failed_startup_waits_for_probe_cleanup_before_load_failure`. |
| 786 dispose cancels/waits active probe | P | `stop_waits_for_a_probe_without_a_managed_record`. |
| 809 failed startup + removed configuration | P | `failed_startup_waits_for_probe_cleanup_before_load_failure`, `config_removal_waits_for_a_probe_only_record`. |
| 809 failed startup + failed probe cleanup | P | `probe_cleanup_failure_takes_precedence_over_startup_failure`. |
| 850 stale mount after newer configuration | P | `real_delayed_start_cannot_publish_after_reconfiguration_or_disposal`: delayed real negotiation проигрывает новой конфигурации, публикуется только current tool. |
| 850 stale mount after disposal | P | Тот же test: manager dispose до handshake, после delay нет tool/status resurrection. |
| 882 listener throws | A | В Rust throw/reject — единый async `Err`; `status_listener_failure_does_not_skip_later_listeners_or_lifecycle` проверяет containment, следующий listener и завершение live restart. |
| 882 listener rejects | A | Та же Rust async-error ветка и test; отдельного синхронного throw-варианта тип API не имеет. |
| 906 dispose unwinds every instance/tools | P | `live_registered_server_follows_registration_and_manager_lifetimes` и delayed-dispose test: live tool снят, операции после dispose отвергнуты. |
| 918 removed while templates resolve | P | `removal_and_disposal_overtake_real_gated_credential_resolution`: remove выигрывает у gated public Credentials resolve, child не стартует. |
| 941 stale activation/status callbacks | P | `real_delayed_start_cannot_publish_after_reconfiguration_or_disposal`: production child callback после delay fenced. |
| 958 starting instance removed pre-activation | P | Gated credential-removal и delayed-handshake tests покрывают обе реальные pre-activation точки. |
| 972 credential rebuild after removal | P | `credential_refresh_rechecks_a_queued_update_and_removal_wins_a_late_rebuild`: late credential result после removal не создаёт generation/tool. |
| 1001 queued reconcile loses to disposal | P | `cancellation_signals_stop_while_actor_is_blocked_in_child_cleanup` проверяет немедленный stop во время blocked actor await: operations отвергнуты, watch закрыт, чужой probe отменён до release cleanup gate. |
| 1034 registered live server lifetime | P | `live_registered_server_follows_registration_and_manager_lifetimes`: настоящий child/tool живёт ровно lifetime registration. |
| 1050 registration collisions/validation | P | `registration_rejects_configured_and_registered_names`, config exact-validation tests; review-case `enabled=false` — `registration_is_owned_by_the_callers_context`. |
| 1062 configured entry takes registered name | P | `RegistrationConflict` exact text — `public_error_texts_match_the_source`; candidate hook реализован в `McpManager::apply`. |
| 1068 restart/probe registered server | P | Тот же live registered test: real restart и successful wire probe. |
| 1079 registration ends after manager stop | P | Тот же test: manager dispose снимает live tool, последующий registration disposer завершается ограниченно по времени. |
| 1091 watch initial + each change + caller abort | A | `watch_starts_with_a_complete_snapshot`; Rust receiver cancellation — drop semantics `watch::Receiver`, не `AbortSignal`. |
| 1107 waiting watch ends on manager dispose | P | `stop_closes_watch_before_probe_cleanup_settles`, `disposal_closes_existing_watchers`. |
| 1118 startupTimeoutMs=0 | P | `rejects_zero_timeout`. |
| 1118 startupTimeoutMs=-5 | A | Rust field — `u64`, отрицательное значение отклоняется serde/schema до `validate_entry`; отдельного typed value не существует. |
| 1127 in-bounds timeout | P | `accepts_stdio_and_http_entries`, defaults/override validation path. |
| 1138 pre-reconcile enabled/disabled list | P | `disabled_config_is_listed_without_mounting_a_client`; enabled default — `defaults_match_the_source`. |
| 1146 legacy `${env:…}` | P | `rejects_legacy_env_templates`. |
| 1152 valid stdio/http pair | P | `accepts_stdio_and_http_entries`. |
| 1162 invalid server name | P | `rejects_invalid_server_name`. |
| 1163 stdio missing command | P | `transport_validation_errors_match_the_source_exactly`. |
| 1164 stdio with url | P | `transport_validation_errors_match_the_source_exactly`. |
| 1165 stdio with headers | P | `transport_validation_errors_match_the_source_exactly`. |
| 1166 HTTP missing url | P | `transport_validation_errors_match_the_source_exactly`. |
| 1167 HTTP with command | P | `transport_validation_errors_match_the_source_exactly`. |
| 1168 HTTP with args | P | `transport_validation_errors_match_the_source_exactly`. |
| 1169 HTTP with env | P | `transport_validation_errors_match_the_source_exactly`. |
| 1170 HTTP with cwd | P | `transport_validation_errors_match_the_source_exactly`. |
| 1171 unknown template token | P | `rejects_nested_template_openers`/client syntax contract. |
| 1174 nested template opener | P | `rejects_nested_template_openers`. |
| 1175 inverted reconnect delays | P | `rejects_inverted_reconnect_delays`. |
| validation order: empty command + inverted reconnect | P | `transport_error_precedes_reconnect_error` сохраняет первую transport-ошибку TS. |
| validation order: invalid earlier name + invalid later field | P | `invalid_name_in_an_earlier_entry_precedes_a_later_transport_error`; schema-only timeout/reconnect bounds всё ещё проверяются глобально раньше имён. |
| `toolCallTimeoutMs: 0` | P | `tool_call_timeout_has_the_sources_unbounded_number_contract` принимает значение на manager boundary, как `z.number()`. |

| TS case (`composition.spec.ts`) | Статус | Rust evidence / причина |
|---|---:|---|
| 57 inherited/profile add/save/restore/revision race | D | Решение ведущего 20.11: это Loader + Settings persistence composition, не manager transport case; Rust evidence владельца — `hoh-loader::config_editor::edit_validates_persists_raw_expressions_and_reconciles_volatile_values`, manager boundary — `volatile_add_change_and_delete_keep_the_fiber`. |
| 96 invalid edit rejected before persistence | A | Manager candidate validation покрыта exact config tests; persistence rollback принадлежит `hoh-loader::config_editor::activation_rejection_restores_the_file_and_previous_running_composition`. |

## Зависимости и внешние API

- Новых внешних версий нет. Dev-dependency `rmcp.workspace = true` включает уже зафиксированные `server`, `transport-io`, `macros` только для локального stdio fixture.
- Чужой API и корневые dependency declarations не менялись; test-only seam в `hoh-mcp-client` не добавлялся.

## Вопросы ведущему

- Решение ведущего от 20.11 выполнено: прежние manager D-кейсы перенесены через настоящий stdio transport с отдельным harness-free test-binary `rmcp` server; seam в `hoh-mcp-client` не добавлялся. Единственный оставшийся D — `composition.spec.ts:57`, прямо отнесённый решением к Loader/Settings и снабжённый ссылкой на тест владельца.
- `toolCallTimeoutMs = 0` принят на manager boundary и в `hoh-mcp-client::Config::validate`, как исходный `z.number()`; чужая правка API для этого значения не требуется.
- Расхождение в чужом крейте `hoh-mcp-client` (не правилось): `credentials::Binding::resolve` возвращает
  «MCP credential templates require a credentials provider» при `expandTemplates` и отсутствии сервиса
  `credentials` даже для записи без единой ссылки; в TS (`credentials.ts:60-65`) ошибка бросается только при
  разрешении конкретной ссылки. Манёджер без сервиса `credentials` поэтому не поднимает обычный сервер.
  Живые тесты подмонтировывают пустого провайдера (`live_world`); тесты, проверяющие отсутствие провайдера для
  записей с `${credential:…}`, его не монтируют.
- Живые тесты исполняются под одним замком: реестр `serverName` в `hoh-mcp-client` общий для процесса, а
  упавший тест не освобождает имя.
- Порядок проверки записи (круг 4): фаза схемы (таймауты, границы reconnect) по всем записям, затем по каждой
  записи имя → transport → шаблоны → cross-field reconnect; тесты в `config.rs`.
