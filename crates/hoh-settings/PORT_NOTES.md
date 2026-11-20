# hoh-settings ← packages/settings/settings @ 39163dd3

## Перенесено

- `src/index.ts` → `src/lib.rs`: публичные descriptor/options/path-op/conflict contracts,
  `SettingsApi`, typed service/event, schema-derived forms, presentation policy, monotonic
  revisions, transactional update/replace/mutate, inherited reset, stale-write refusal,
  profile document access, reload coalescing and one-shot legacy import.
- `src/schema.ts` → `src/schema.rs`: volatile projection for JSON Schema `x-volatile`,
  `$ref`/`$defs`, defaults, expressions, projection and path classification.
- `src/redact.ts` → `src/redact.rs`: structural `x-role: secret` redaction for objects,
  dictionaries, arrays and schema combinations, including unset slots and deduplication.
- `src/types.ts` → `src/types.rs`: branded namespace and complete wire views.
- All eight `locale/*.json` copied unchanged.
- The crate now has 54 focused and real Loader/ConfigEditor cases. They cover live in-place
  persistence, nullable secret redaction, stale and concurrent revisions, replace/mutate/reset,
  presentation policy, terminal invalidation order and rename-before-write legacy migration.

## Review 024544: причина и исправление

- `schemars` represents nullable containers with a type union such as
  `"type":["array","null"]`. Exact string comparison skipped those containers, so `describe`
  could return a nullable dict/array secret. Redaction and form projection now recognize a type
  inside the union. `redaction_handles_nullable_dictionary_and_array_types` and
  `real_describe_redacts_nullable_secret_arrays` are regressions for the direct and wire paths.
- JSON Schema adaptation had made `is_volatile_path` broader than source
  `schema.dict?.[key]`: it followed `items`, `additionalProperties` and union branches. It now
  follows declared object properties only (while retaining `$ref` resolution), and stripping
  walks the filtered form's declared properties. The public runtime cannot host the review's
  deliberately invalid nested-volatile array/dict schema: `hoh-kernel` rejects that schema before
  activation as `invalid volatile placement`. Therefore
  `mutation_preflight_rejects_volatile_nodes_below_dynamic_containers` exercises the exact
  `SettingsForms::write` preflight function and error, while
  `volatile_paths_and_stripping_do_not_descend_into_dynamic_containers` covers preservation.
  No foreign crate was changed.
- Revision fingerprints now normalize a missing/null entry config to `{}`, matching source `??
  {}`. `absent_and_explicit_empty_config_share_revision` uses the real editor and checks unchanged
  revision, no event and continued validity of the old expected revision.
- Empty-path scalar mutation now returns the source error `Config root must be a plain object`.
  `SettingsDescriptor` has a manual secret-safe `Debug` implementation that omits schema, value,
  base and user payloads.

## Матрица тестов исходника, отмеченных ревью

| TS case | Rust status |
| --- | --- |
| `configuration-inheritance`: shared group/insert inheritance | Loader-owned composition; detached inherited values are covered by `hoh-loader::configuration_returns_detached_inherited_and_explicit_values`; the exact group/insert scenario remains a Loader follow-up. |
| absent config and bundle refresh | `hoh-loader::missing_and_null_configs_are_empty_objects_and_an_empty_edit_is_a_noop` plus `absent_and_explicit_empty_config_share_revision`. |
| 190 entries with 0/13/190 overrides | Deferred to `hoh-loader`: Rust exposes no composition-work spy/counter, so a faithful bound assertion cannot be made from this crate. |
| `editor-failures` read denial/truncated/wrong/removed patch | Loader-owned atomic edit path. `hoh-loader::fallible_callback_reads_reloaded_configuration_and_preserves_its_error_before_write`, `invalid_config_is_rejected_before_the_file_changes`, and activation rollback cover the public failure guarantees. Exact post-validation filesystem injection remains a Loader test follow-up because `ConfigEditor` has no filesystem injection seam. |
| home-layer shadowing | `hoh-loader::home_and_commandline_overrides_are_reported_before_persistence`. |
| concurrent writes and stale external revision | `concurrent_writes_serialize_and_only_one_revision_wins`, `real_describe_redacts_secrets_and_rejects_stale_revision`, and `hoh-loader::stale_entries_are_rejected_and_concurrent_edits_observe_the_latest_raw_config`. |
| activation rejection rollback | `hoh-loader::activation_rejection_restores_the_file_and_previous_running_composition`. |
| write held across entry removal | Terminal settings behavior is covered by `removed_form_emits_one_terminal_revision`; stale held editor handles are covered by `hoh-loader::stale_entries_are_rejected_and_concurrent_edits_observe_the_latest_raw_config`. |
| group-owned entry | Deferred to Loader's group composition follow-up; settings consumes the flattened `ConfigEditor::configuration` contract and has no group traversal of its own. |
| entry removed by external bundle edit | Loader stale-entry test above; settings' terminal revision is covered locally. |
| entry disabled after form read | Deferred to a Loader lifecycle integration case; settings already filters non-active fibers on every `describe` and rechecks inside the serialized editor transaction. |
| schema-default array rows | `mutate_uses_array_schema_default` and `mutate_appends_and_splices_arrays`. |
| dictionary keys / optional containers | `project_form_preserves_volatile_dictionary_keys` and `mutate_creates_optional_dictionary_containers`; the source permits mutation because the dict/container itself is volatile, not because its dynamic child is volatile. |
| inherited secret | `inherited_secret_is_restored_without_crossing_redacted_describe`. |
| queued refresh after dispose | Deferred lifecycle integration: `SettingsPlugin` owns the cancellation worker and the current fixture mounts `SettingsForms` directly. The worker checks both `closed` and fiber state before publication. |
| required value only in profile / failed refresh | Required inherited fallback is covered by `hoh-loader::inherited_resolution_uses_the_actual_codec_and_context_without_apply_or_candidate_hooks`; injected refresh-logger failure remains a SettingsPlugin lifecycle follow-up. |
| malformed redaction containers, opaque secret container, missing relation maps | `redaction_matches_malformed_opaque_and_relationless_source_cases`. |
| transform redaction | Schemastery transforms have no Schemars/JSON Schema representation and no Rust runtime transform wrapper; union/intersection JSON relations remain covered by `redaction_conservatively_combines_union_branches`. |

## Решения по заметкам ревью

- `revisions` uses `IndexMap`, so terminal events retain the source `Map` insertion order;
  `removed_form_events_follow_configuration_order` fixes the observable order.
- Legacy-file existence uses async `tokio::fs::try_exists`, avoiding blocking filesystem work in
  the async import task.
- Non-mapping legacy YAML intentionally fails closed with `legacy settings document must be a
  mapping`. TypeScript's unchecked `Object.entries` on a scalar is accidental coercion (including
  character indexes for strings), not a meaningful section import contract.
- Array indexes now use the source decimal grammar `^(0|[1-9][0-9]*)$`; `+1`, leading zeroes and
  signed forms are rejected by `mutate_rejects_invalid_array_indexes` and the corrected lookup /
  schema-selection branches are pinned by `lookup_and_schema_reject_signed_array_indexes`.

## Rust representation

- Schemars JSON Schema is the native schema boundary. Schemastery `meta.volatile` and
  `meta.role` correspond to `x-volatile` and `x-role`; traversal also accepts the source-like
  `meta` shape for direct compatibility tests.
- `describe` is async because Rust `ConfigEditor::configuration` and inherited codec resolution
  are async. `SettingsApi` returns boxed futures and requires an explicit owner `Fiber` for
  `configure`, since Rust services do not have Cordis caller-bound proxy state.
- Rust write inputs are `serde_json::Value`, so BigInt, Symbol, functions, Dates, cycles,
  `undefined` array entries and non-finite numbers are unrepresentable at the public boundary.
  Object-root validation and exact array/path errors remain explicit.
- A single cancellation-bound refresh worker implements the source microtask coalescing and
  suppresses a queued publication after disposal.

## Зависимости и shim

- Новых lockfile dependencies нет. `serde`, `serde_json`, `schemars`, `indexmap`,
  `saphyr-parser`, `thiserror` and `tokio` already belong to the workspace.
- `// SHIM(hoh-yaml)` in `src/yaml.rs` is a local YAML 1.2-core materializer for the removed
  `settings.yaml`. Replace it with the planned `hoh-yaml` crate when that interface exists.
  Rename happens before parsing or the first profile write, as in the source. The shim accepts
  core tags, radix scalars and aliases with a 100000-node expansion budget. Non-finite YAML
  numbers fail closed at parse time because `serde_json::Value` cannot represent them; the TS
  source parses them and then rejects the affected section at its JSON-shaped write boundary.

## Внешние follow-up (чужие крейты не менялись)

- `hoh-agent-default-model/src/lib.rs` still declares a private `settings` ServiceKey and trait.
  Kernel keys are TypeId-strict, so it must later import `hoh_settings::{Settings, SettingsApi,
  SettingsPresentation}` to consume this service.
- `hoh-permission-presets/src/lib.rs` and `hoh-agent-preset-registry/src/lib.rs` retain explicit
  omissions of source `settings.configure({ auto: false }, fiber)` calls. They should register
  against this API in their own review rounds.
- These follow-ups affect end-to-end automatic-page policy of those consumers, not the behavior
  or completeness of this crate's service and tests.

## Проверка

Команды при сдаче запускаются с `CARGO_BUILD_JOBS=2`, offline и под `timeout` в выделенном
`target-x6`: package tests, clippy `--all-targets --no-deps -D warnings`, fmt check.
