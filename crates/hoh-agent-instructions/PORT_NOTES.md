# hoh-agent-instructions ← packages/context/agent-instructions @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/config.ts` | `src/config.rs` | схема, defaults, нормализация кандидатов, process-wide homes и identity baseline |
| `src/digest.ts` | `src/digest.rs` | SHA-1 точных и JS-trimmed UTF-8 строк |
| `src/files.ts` | `src/files.rs` | host/provider discovery, два user-global корня, root markers, bounded read, dedup и scope probes |
| `src/render.ts` | `src/render.rs` | два budget chain, UTF-8 truncation, frame escaping, baseline/replacement/delta rendering |
| `src/state.ts` | `src/state.rs` | durable changes, visible surface, session-local version cache и fail-closed reconciliation |
| `src/index.ts` | `src/lib.rs` | plugin, pre-step composition, inbox sync, tool ancestry, step boundary и последовательные проекции |
| `tests/*.ts` | `tests/agent_instructions.rs`, unit tests | 67 Rust-сценариев: 41 black-box и 26 unit, включая настоящий host/provider I/O и смонтированный plugin pipeline |
| `locale/*.json` | `locale/*.json` | все 8 файлов скопированы без изменений |

## Отличия от образца

- `WeakMap<Session, …>` представлен map по типизированному `SessionId`; записи удаляются на
  `session/disposed`, поэтому lifetime остаётся session-local и не удерживает завершённую сессию.
- `node:crypto.createHash('sha1')` реализован внутри крейта: внешнего SHA-1 нет в workspace/Cargo.lock,
  а правила волны запрещают новую внешнюю зависимость. Реализация проверена стандартными векторами.
- Promise tail проекций представлен FIFO-очередью билетов на сессию. Билет регистрируется
  синхронно до `Context::spawn`, поэтому `agent/pre-step` видит даже ещё не опрошенную задачу;
  tool ancestry и `step/end` остаются commit boundaries образца.
- Ошибки provider resolve/stat дают `unavailable`, подтверждённый non-file — `absent`; cancellation
  пробрасывается отдельно и не превращается в удаление инструкции.
- После ревью fail-closed reconciliation группирует оба global root по candidate group, откатывает
  группу целиком при unavailable/read failure и сохраняет metadata скрытых дубликатов. `cwd`
  берётся из `workingDirectory.ensure`, а не из неизменяемого session header.
- Набор excluded baseline scopes пересобирается из текущего discovery; дубликат представленного
  победителя возвращается в reconciliation для будущего promotion. Preparation публикуется только
  после успешного reconcile.
- Конечные projection-задачи используют lifecycle cancellation Context и сами снимают завершённый
  effect; ошибки после abort не журналируются. Inbox учитывает уже видимый payload на session surface.
- Provider-маркер считает только `FS_NOT_FOUND` отсутствием, остальные ошибки пробрасывает; abort
  между chunks bounded stream также пробрасывается. Это проверяют
  `provider_not_found_marker_is_absent`, `provider_io_marker_error_is_not_treated_as_absent` и
  `abort_during_provider_stream_read_is_propagated` в `src/files.rs`. Provider matrix дополнительно
  проверяет missing/non-file/stat-error, известный размер до открытия stream и ограничение stream без
  известного размера.
- Регрессия `tools_result_projection_is_visible_to_the_immediately_following_pre_step` монтирует
  production `AgentRegistry`, `WorkingDirectory`, `ToolRuntime` и `AgentInstructions`, проводит
  настоящий `read` через `tools/result`, блокирует provider stream и немедленно запускает
  `agent/pre-step`. Тест падает при удалении pre-step barrier: touched `pkg/AGENTS.md` не попадает в
  текущий шаг.
- Плагинное wiring отдельно проверяют
  `step_end_listener_releases_a_deferred_tools_result_projection` (отложенный touch выпускается
  настоящим `session/event`),
  `session_disposed_listener_detaches_a_reused_id_from_the_old_projection_queue` (новая Session с
  тем же id не наследует старую очередь) и `non_abort_projection_failure_is_logged_as_a_warning`.
- `descendant_dirs_between` намеренно принимает каталог с обычным именем `..foo`: за пределы root
  выходят только `..` и `../…`. Это безопаснее буквального TS `startsWith("..")`, который ошибочно
  отбрасывает допустимое имя.
- Host-only fallback пропускает файл с невалидным UTF-8 вместо декодирования с U+FFFD. Production
  использует provider `Fs`, возвращающий текст; отличие ограничено диагностическим host seam.
- `visible_instruction_changes` обходит события по `seq` и фильтрует их текущим surface. Текущий
  session surface сохраняет порядок видимых событий; если будущая реализация разрешит произвольную
  перестановку nodes, обход следует заменить на прямой порядок `surface.nodes()`.

## Соответствие тестовой матрице образца

197 исходных `it` не копируются один-к-одному: ниже перечислены конкретные equivalence classes и
причина, по которой повторяющиеся варианты не размножены.

- First-step/resume/preservation/rebuild (spec 1882–2520) сведены к проверкам
  `baseline_state_has_digests_but_no_invented_provider_versions`,
  `replacement_of_an_empty_baseline_explicitly_supersedes_old_state`,
  `pending_baseline_is_not_reused_after_identity_changes` и
  `baseline_identity_changes_with_discovery_or_budget_semantics`. Startup и resume вызывают один и
  тот же `compose_context`; различается только уже видимый surface, который проверяет
  `surface_payload_removes_the_same_pending_context`.
- Remount/compaction (spec 3223–3348, 5424–5540) не имеют отдельной алгоритмической ветви: новый
  instance заново строит preparation из surface, а identity change запрещает reuse. Эти два
  перехода покрыты указанными выше preparation/identity/surface тестами. Автоматическое снятие
  context listeners при unload — contract `hoh-kernel`; плагин не хранит ручных registration
  handles, поэтому source-варианты 4054/6266 не дублируются здесь.
- Projection block/abort, closed-step seed и serialization/merge (spec 4268–4456, 5759–6047,
  6414–6538) покрывают `tools_result_projection_is_visible_to_the_immediately_following_pre_step`,
  `step_end_listener_releases_a_deferred_tools_result_projection`,
  `projection_signal_is_aborted_during_context_shutdown`,
  `step_boundary_is_open_only_between_start_and_end`,
  `projection_tickets_run_in_issue_order_not_poll_order` и
  `nested_tool_touches_are_released_only_by_the_root_execution`. Варианты меняют только момент
  доставки одного и того же touch; алгоритм очереди общий.
- Warn при projection failure (spec 6133) проверяет
  `non_abort_projection_failure_is_logged_as_a_warning`; abort-ветку, в которой warning подавляется,
  проверяет `projection_signal_is_aborted_during_context_shutdown`.
- Provider cases spec 3676, 3725–3948 покрыты тестами `provider_*` и
  `abort_during_provider_stream_read_is_propagated`; host/provider combinations после получения
  `LoadedInstructionFile` сходятся в один render/reconcile pipeline, поэтому все render budgets и
  dedup cases не повторяются для обоих transport seams.
- `SessionDisposed` wiring проверяет повторное использование того же `SessionId`: pre-step новой
  Session не ждёт незавершённую projection queue старой Session. Это наблюдаемый внешний эффект
  очистки без привязки теста к внутреннему размеру map.

## Не проверено

- Платформы кроме Linux.
- Живой model loop с внешним LLM provider; seams собраны на настоящих workspace API и проверены
  компиляцией всех targets.

## Вопросы ведущему

- Нет.
