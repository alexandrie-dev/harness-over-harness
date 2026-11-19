mod support;

use futures_util::future::join_all;
use hoh_agent_preset_registry::{AgentPresets, Config, PresetDefinition, entry_list_problem};
use hoh_error::HarnessError;
use hoh_scope::{ScopeKey, create_scope};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use support::{
    DelayGate, DelayGateService, agent, definition, fresh, panicking_agent, selection_fresh,
};

#[tokio::test]
async fn eagerly_scopes_registrations_and_shares_one_revision_across_agents() {
    let (kernel, ctx, registry) = fresh();
    registry
        .register(
            &ctx,
            definition("standard", json!([{"name":"fixture:active"}])),
        )
        .await
        .unwrap();
    registry
        .register(
            &ctx,
            definition("minimal", json!([{"name":"fixture:active"}])),
        )
        .await
        .unwrap();
    let first = create_scope(&ctx, ScopeKey::new(), Default::default()).unwrap();
    let second = create_scope(&ctx, ScopeKey::new(), Default::default()).unwrap();
    let third = create_scope(&ctx, ScopeKey::new(), Default::default()).unwrap();
    registry.mount(&first.ctx, None).await.unwrap();
    registry.mount(&second.ctx, Some("minimal")).await.unwrap();
    registry.mount(&third.ctx, None).await.unwrap();
    assert!(registry.inspect_compositions(Some(&ctx)).await.is_empty());
    assert_eq!(
        registry.inspect_compositions(Some(&first.ctx)).await[0].id,
        "standard"
    );
    assert_eq!(
        registry.inspect_compositions(Some(&second.ctx)).await[0].id,
        "minimal"
    );
    assert_eq!(registry.inspect_compositions(None).await.len(), 2);
    kernel.shutdown().await;
}

#[tokio::test]
async fn retains_a_replaced_revision_for_existing_scopes_and_children_then_releases_it() {
    let (kernel, ctx, registry) = fresh();
    let old = registry
        .register(
            &ctx,
            definition("standard", json!([{"name":"fixture:active"}])),
        )
        .await
        .unwrap();
    let parent = create_scope(&ctx, ScopeKey::new(), Default::default()).unwrap();
    registry.mount(&parent.ctx, None).await.unwrap();
    old.dispose().await;
    registry
        .register(
            &ctx,
            definition("standard", json!([{"name":"fixture:active"}])),
        )
        .await
        .unwrap();
    assert_eq!(registry.inspect_compositions(None).await.len(), 2);
    let child = create_scope(&ctx, ScopeKey::new(), Default::default()).unwrap();
    assert_eq!(
        registry.compose_from(&child.ctx, &parent.ctx).unwrap(),
        Some("standard".into())
    );
    parent.dispose().await;
    assert_eq!(registry.inspect_compositions(None).await.len(), 2);
    child.dispose().await;
    assert_eq!(registry.inspect_compositions(None).await.len(), 1);
    kernel.shutdown().await;
}

#[tokio::test]
async fn removes_definitions_without_revoking_live_bindings() {
    let (kernel, ctx, registry) = fresh();
    let registration = registry
        .register(&ctx, definition("standard", json!([])))
        .await
        .unwrap();
    let scope = create_scope(&ctx, ScopeKey::new(), Default::default()).unwrap();
    registry.mount(&scope.ctx, None).await.unwrap();
    registration.dispose().await;
    assert!(registry.list().await.is_empty());
    assert!(registry.resolve(None).await.is_err());
    assert_eq!(
        registry.composed_preset(&scope.ctx).as_deref(),
        Some("standard")
    );
    scope.dispose().await;
    assert!(registry.inspect_compositions(None).await.is_empty());
    kernel.shutdown().await;
}

#[tokio::test]
async fn reports_a_failed_definition_while_allowing_healthy_definitions() {
    let (kernel, ctx, registry) = fresh();
    registry
        .register(
            &ctx,
            definition("broken", json!([{"name":"fixture:missing"}])),
        )
        .await
        .unwrap();
    registry
        .register(&ctx, definition("standard", json!([])))
        .await
        .unwrap();
    assert!(
        registry
            .resolve(Some("broken"))
            .await
            .unwrap()
            .broken
            .is_some()
    );
    let scope = create_scope(&ctx, ScopeKey::new(), Default::default()).unwrap();
    assert!(registry.mount(&scope.ctx, Some("broken")).await.is_err());
    assert_eq!(
        registry.mount(&scope.ctx, None).await.unwrap().id,
        "standard"
    );
    assert!(
        registry
            .remote_export_list()
            .await
            .presets
            .iter()
            .any(|row| row.id == "standard" && row.is_default)
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_duplicate_ids_without_disposing_the_first_definition() {
    let (kernel, ctx, registry) = fresh();
    registry
        .register(&ctx, definition("standard", json!([])))
        .await
        .unwrap();
    let duplicate = registry
        .register(&ctx, definition("standard", json!([])))
        .await;
    assert!(
        duplicate
            .err()
            .unwrap()
            .to_string()
            .contains("Duplicate agent preset")
    );
    assert_eq!(registry.resolve(None).await.unwrap().id, "standard");
    kernel.shutdown().await;
}

#[tokio::test]
async fn rebinds_a_blank_scope_and_preserves_a_child_on_its_former_revision() {
    let (kernel, ctx, registry) = fresh();
    for id in ["standard", "minimal"] {
        registry
            .register(&ctx, definition(id, json!([])))
            .await
            .unwrap();
    }
    let scope = create_scope(&ctx, ScopeKey::new(), Default::default()).unwrap();
    registry.mount(&scope.ctx, None).await.unwrap();
    registry.mount(&scope.ctx, None).await.unwrap();
    let child = create_scope(&ctx, ScopeKey::new(), Default::default()).unwrap();
    registry.compose_from(&child.ctx, &scope.ctx).unwrap();
    registry.recompose(&scope.ctx, "minimal").await.unwrap();
    assert_eq!(
        registry.composed_preset(&scope.ctx).as_deref(),
        Some("minimal")
    );
    assert_eq!(
        registry.composed_preset(&child.ctx).as_deref(),
        Some("standard")
    );
    let free = create_scope(&ctx, ScopeKey::new(), Default::default()).unwrap();
    assert_eq!(registry.compose_from(&free.ctx, &ctx).unwrap(), None);
    kernel.shutdown().await;
}

#[tokio::test]
async fn logs_selection_and_rejects_changes_after_the_first_turn() {
    let harness = selection_fresh().await;
    for id in ["standard", "minimal"] {
        harness
            .registry
            .register(&harness.ctx, definition(id, json!([])))
            .await
            .unwrap();
    }
    let (_scope, agent) = agent(&harness.ctx, "selection");
    harness.registry.mount(agent.ctx(), None).await.unwrap();
    assert_eq!(
        harness
            .registry
            .select(agent.clone(), "minimal")
            .await
            .unwrap(),
        "minimal"
    );
    assert_eq!(
        harness
            .registry
            .preset_for_session(agent.session())
            .unwrap(),
        Some("minimal".into())
    );
    agent
        .session()
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    let error = harness
        .registry
        .select(agent, "standard")
        .await
        .unwrap_err();
    assert_eq!(
        HarnessError::of(error.as_ref()).unwrap().code(),
        "agent-preset/locked"
    );
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn inventories_active_disabled_entries_and_display_metadata_in_order() {
    let (kernel, ctx, registry) = fresh();
    let mut standard = definition("standard", json!([{"name":"fixture:active"}]));
    standard.name = Some("Standard".into());
    standard.description = Some("General".into());
    standard.order = Some(2.0);
    registry.register(&ctx, standard).await.unwrap();
    let mut empty = definition("empty", json!([{"name":"fixture:missing","disabled":true}]));
    empty.order = Some(1.0);
    registry.register(&ctx, empty).await.unwrap();
    assert_eq!(
        registry
            .list()
            .await
            .into_iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        ["empty", "standard"]
    );
    let inventory = registry.composition_inventory().await;
    let standard = inventory.iter().find(|row| row.id == "standard").unwrap();
    assert_eq!(standard.name.as_deref(), Some("Standard"));
    assert!(standard.is_default);
    let empty = inventory.iter().find(|row| row.id == "empty").unwrap();
    assert_eq!(empty.rows[0].module_name, "fixture:missing");
    let roster = serde_json::to_value(registry.remote_export_list().await).unwrap();
    let standard = roster["presets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "standard")
        .unwrap();
    assert_eq!(standard["order"], 2.0);
    kernel.shutdown().await;
}

#[tokio::test]
async fn renders_a_declaration_as_entry_list_yaml_conditions_included() {
    let (kernel, ctx, registry) = fresh();
    let mut standard = definition(
        "standard",
        json!([
            {"id":"contribute","name":"fixture:active","config":{"tool":"standard"}},
            {"id":"windows-only","name":"missing","disabled":{"__jsExpr":"process.platform !== 'win32'"}}
        ]),
    );
    standard.name = Some("Standard".into());
    standard.description = Some("General".into());
    registry.register(&ctx, standard).await.unwrap();
    let document = registry.read_document("standard").unwrap();
    assert_eq!(document.name.as_deref(), Some("Standard"));
    assert!(
        document
            .content
            .starts_with("- id: contribute\n  name: fixture:active")
    );
    assert!(
        document
            .content
            .contains("\n  config:\n    tool: standard\n")
    );
    assert!(
        document
            .content
            .contains("disabled: !!js process.platform !== 'win32'\n")
    );
    assert!(!document.content.contains("__jsExpr"));
    assert!(registry.read_document("absent").is_err());
    kernel.shutdown().await;
}

#[tokio::test]
async fn renders_multiline_and_yaml_typed_strings_like_js_yaml() {
    let (kernel, ctx, registry) = fresh();
    registry
        .register(
            &ctx,
            definition(
                "standard",
                json!([{"name":"fixture:active","config":{
                    "prompt":"line1\nline2",
                    "hex":"0x1F",
                    "underscored":"1_000",
                    "date":"2001-01-01",
                    "hash":"foo#bar",
                    "dash":"-foo"
                }}]),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        registry.read_document("standard").unwrap().content,
        "- name: fixture:active\n  config:\n    prompt: |-\n      line1\n      line2\n    hex: '0x1F'\n    underscored: 1_000\n    date: 2001-01-01\n    hash: foo#bar\n    dash: '-foo'\n"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn emits_an_indent_indicator_for_a_more_indented_first_content_line() {
    let (kernel, ctx, registry) = fresh();
    registry
        .register(
            &ctx,
            definition(
                "standard",
                json!([{"name":"fixture:active","config":{"prompt":"  a\nb"}}]),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        registry.read_document("standard").unwrap().content,
        "- name: fixture:active\n  config:\n    prompt: |2-\n        a\n      b\n"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn double_quotes_and_escapes_js_yaml_non_printable_characters() {
    let (kernel, ctx, registry) = fresh();
    registry
        .register(
            &ctx,
            definition(
                "standard",
                json!([{"name":"fixture:active","config":{
                    "tab":"a\tb",
                    "carriageReturn":"x\ry",
                    "multilineTab":"l1\n\tl2",
                    "nel":"a\u{85}b",
                    "nbsp":"a\u{a0}b",
                    "lineSeparator":"a\u{2028}b",
                    "paragraphSeparator":"a\u{2029}b",
                    "bom":"a\u{feff}b"
                }}]),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        registry.read_document("standard").unwrap().content,
        "- name: fixture:active\n  config:\n    tab: \"a\\tb\"\n    carriageReturn: \"x\\ry\"\n    multilineTab: \"l1\\n\\tl2\"\n    nel: \"a\\Nb\"\n    nbsp: \"a\\_b\"\n    lineSeparator: \"a\\Lb\"\n    paragraphSeparator: \"a\\Pb\"\n    bom: \"a\\uFEFFb\"\n"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn quotes_only_strings_resolved_by_the_pinned_js_yaml_json_schema() {
    let (kernel, ctx, registry) = fresh();
    registry
        .register(
            &ctx,
            definition(
                "standard",
                json!([{"name":"fixture:active","config":{
                    "inf":"inf",
                    "nan":"nan",
                    "infinity":"Infinity",
                    "date":"2001-01-01",
                    "datePrefix":"2024-05-01 notes",
                    "underscored":"1_000",
                    "merge":"<<",
                    "nanCamel":".NaN",
                    "infCamel":".Inf",
                    "infUpper":".INF",
                    "positiveInf":"+.inf",
                    "positiveLeadingDot":"+.2",
                    "positiveLeadingDotExponent":"+.2e4",
                    "dash":"-foo"
                }}]),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        registry.read_document("standard").unwrap().content,
        "- name: fixture:active\n  config:\n    inf: inf\n    nan: nan\n    infinity: Infinity\n    date: 2001-01-01\n    datePrefix: 2024-05-01 notes\n    underscored: 1_000\n    merge: <<\n    nanCamel: '.NaN'\n    infCamel: '.Inf'\n    infUpper: '.INF'\n    positiveInf: '+.inf'\n    positiveLeadingDot: +.2\n    positiveLeadingDotExponent: +.2e4\n    dash: '-foo'\n"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn applies_the_pinned_scalar_writer_to_tagged_js_expressions() {
    let (kernel, ctx, registry) = fresh();
    registry
        .register(
            &ctx,
            definition(
                "standard",
                json!([{"name":"fixture:active","config":{
                    "bang":{"__jsExpr":"!env.X"},
                    "ternary":{"__jsExpr":"a ? b : c"},
                    "quoted":{"__jsExpr":"\"x\""},
                    "boolean":{"__jsExpr":"false"},
                    "extra":{"__jsExpr":"env.X","ignored":true},
                    "multiline":{"__jsExpr":"a\nb"}
                }}]),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        registry.read_document("standard").unwrap().content,
        "- name: fixture:active\n  config:\n    bang: !!js '!env.X'\n    ternary: !!js 'a ? b : c'\n    quoted: !!js '\"x\"'\n    boolean: !!js 'false'\n    extra: !!js env.X\n    multiline: !!js |-\n      a\n      b\n"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn formats_negative_zero_and_fractional_scientific_numbers_like_js_yaml() {
    let (kernel, ctx, registry) = fresh();
    registry
        .register(
            &ctx,
            definition(
                "standard",
                json!([{"name":"fixture:active","config":{
                    "negativeZero":-0.0,
                    "small":1e-7,
                    "subnormal":5e-324,
                    "fractional":1.5e-7,
                    "largeInteger":1e21
                }}]),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        registry.read_document("standard").unwrap().content,
        "- name: fixture:active\n  config:\n    negativeZero: -0.0\n    small: 1.e-7\n    subnormal: 5.e-324\n    fractional: 1.5e-7\n    largeInteger: 1e+21\n"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn retains_a_retired_revision_for_an_in_flight_cold_read() {
    let (kernel, ctx, registry) = fresh();
    let registration = registry
        .register(&ctx, definition("standard", json!([])))
        .await
        .unwrap();
    let lease = registry.acquire_scope(None).await.unwrap();
    registration.dispose().await;
    assert_eq!(registry.inspect_compositions(None).await.len(), 1);
    lease.dispose().await;
    lease.dispose().await;
    assert!(registry.inspect_compositions(None).await.is_empty());
    kernel.shutdown().await;
}

#[tokio::test]
async fn contains_plugin_activation_failures_and_root_realm_service_leaks() {
    let (kernel, ctx, registry) = fresh();
    for (id, name, reason) in [
        ("throws", "fixture:failing", "refused"),
        ("leaks", "fixture:leak", "require isolate realms"),
    ] {
        registry
            .register(&ctx, definition(id, json!([{"name":name}])))
            .await
            .unwrap();
        assert!(
            registry
                .resolve(Some(id))
                .await
                .unwrap()
                .broken
                .unwrap()
                .contains(reason)
        );
    }
    assert!(registry.inspect_compositions(None).await.is_empty());
    kernel.shutdown().await;
}

#[tokio::test]
async fn keeps_a_row_waiting_for_an_absent_service_mounted_and_refuses_bindings() {
    let (kernel, ctx, registry) = fresh();
    registry
        .register(
            &ctx,
            definition("invalid", json!([{"name":"fixture:needs-missing"}])),
        )
        .await
        .unwrap();
    let broken = registry
        .resolve(Some("invalid"))
        .await
        .unwrap()
        .broken
        .unwrap();
    assert!(broken.contains("serviceThatDoesNotExist"));
    assert_eq!(registry.inspect_compositions(None).await.len(), 1);
    let scope = create_scope(&ctx, ScopeKey::new(), Default::default()).unwrap();
    assert!(registry.mount(&scope.ctx, Some("invalid")).await.is_err());
    kernel.shutdown().await;
}

#[tokio::test]
async fn a_host_provider_can_activate_a_retained_pending_row() {
    let (kernel, ctx, registry) = fresh();
    registry
        .register(
            &ctx,
            definition("late", json!([{"name":"fixture:needs-missing"}])),
        )
        .await
        .unwrap();
    assert!(
        registry
            .resolve(Some("late"))
            .await
            .unwrap()
            .broken
            .is_some()
    );
    let _provider = ctx
        .provide::<support::MissingService>(Arc::new(()))
        .await
        .unwrap();
    assert_eq!(registry.resolve(Some("late")).await.unwrap().broken, None);
    kernel.shutdown().await;
}

#[tokio::test]
async fn inspection_reports_a_service_leaked_after_a_pending_row_activates() {
    let (kernel, ctx, registry) = fresh();
    registry
        .register(
            &ctx,
            definition("late", json!([{"name":"fixture:late-leak"}])),
        )
        .await
        .unwrap();
    assert!(
        registry.inspect_compositions(None).await[0]
            .leaked_services
            .is_empty()
    );
    let _provider = ctx
        .provide::<support::MissingService>(Arc::new(()))
        .await
        .unwrap();
    assert_eq!(registry.resolve(Some("late")).await.unwrap().broken, None);
    assert_eq!(
        registry.inspect_compositions(None).await[0].leaked_services,
        ["lateFixtureService"]
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn allows_an_isolated_service_without_reporting_a_root_leak() {
    let (kernel, ctx, registry) = fresh();
    registry
        .register(
            &ctx,
            definition(
                "standard",
                json!([{"name":"cordis:group","group":true,
                    "isolate":{"fixtureService":true},
                    "config":[{"name":"fixture:leak"}]}]),
            ),
        )
        .await
        .unwrap();
    assert_eq!(registry.resolve(None).await.unwrap().broken, None);
    kernel.shutdown().await;
}

#[tokio::test]
async fn resolves_the_saved_default_over_the_deployment_default() {
    let kernel = hoh_kernel::Kernel::start();
    let ctx = kernel.root();
    let registry = hoh_agent_preset_registry::AgentPresetRegistry::new(
        &ctx,
        Config {
            default: "standard".into(),
            selected_default: hoh_kernel::Volatile(Some("minimal".into())),
        },
        support::runtime(),
    )
    .unwrap();
    assert_eq!(registry.default_id(), "minimal");
    kernel.shutdown().await;
}

#[test]
fn ignores_a_retired_mode_selection_enabled_field() {
    let config: Config = serde_json::from_value(json!({
        "default":"standard","selectedDefault":"minimal","modeSelectionEnabled":false
    }))
    .unwrap();
    assert_eq!(config.selected_default.0.as_deref(), Some("minimal"));
}

#[test]
fn keeps_its_own_instance_off_generated_settings_fields() {
    let schema = schemars::schema_for!(Config);
    let text = serde_json::to_string(&schema).unwrap();
    assert!(!text.contains("modeSelectionEnabled"));
    assert!(!text.contains("auto"));
}

#[tokio::test]
async fn refuses_unscoped_and_duplicate_bindings_without_leaking_references() {
    let (kernel, ctx, registry) = fresh();
    let registration = registry
        .register(&ctx, definition("standard", json!([])))
        .await
        .unwrap();
    assert!(registry.mount(&ctx, None).await.is_err());
    assert!(
        registry
            .register(&ctx, definition(" ", json!([])))
            .await
            .is_err()
    );
    let parent = create_scope(&ctx, ScopeKey::new(), Default::default()).unwrap();
    registry.mount(&parent.ctx, None).await.unwrap();
    assert!(registry.compose_from(&ctx, &parent.ctx).is_err());
    let child = create_scope(&ctx, ScopeKey::new(), Default::default()).unwrap();
    registry.compose_from(&child.ctx, &parent.ctx).unwrap();
    assert!(registry.compose_from(&child.ctx, &parent.ctx).is_err());
    registration.dispose().await;
    child.dispose().await;
    parent.dispose().await;
    assert!(registry.inspect_compositions(None).await.is_empty());
    kernel.shutdown().await;
}

#[tokio::test]
async fn serializes_competing_blank_session_selections_and_recovers_after_failure() {
    let harness = selection_fresh().await;
    for id in ["standard", "minimal"] {
        harness
            .registry
            .register(&harness.ctx, definition(id, json!([])))
            .await
            .unwrap();
    }
    let (_scope, agent) = agent(&harness.ctx, "competing");
    harness.registry.mount(agent.ctx(), None).await.unwrap();
    let outcomes = join_all([
        harness.registry.select(agent.clone(), "absent"),
        harness.registry.select(agent.clone(), "minimal"),
        harness.registry.select(agent.clone(), "standard"),
    ])
    .await;
    assert!(outcomes[0].is_err());
    assert_eq!(outcomes[1].as_ref().unwrap(), "minimal");
    assert_eq!(outcomes[2].as_ref().unwrap(), "standard");
    assert_eq!(
        harness
            .registry
            .preset_for_session(agent.session())
            .unwrap(),
        Some("standard".into())
    );
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn selection_of_one_agent_does_not_wait_for_another_agents_activation() {
    let harness = selection_fresh().await;
    harness
        .registry
        .register(&harness.ctx, definition("standard", json!([])))
        .await
        .unwrap();
    let (_first_scope, first) = agent(&harness.ctx, "slow-agent");
    let (_second_scope, second) = agent(&harness.ctx, "healthy-agent");
    harness.registry.mount(first.ctx(), None).await.unwrap();
    harness.registry.mount(second.ctx(), None).await.unwrap();

    let gate = Arc::new(DelayGate {
        started: Default::default(),
        release: Default::default(),
    });
    let _provider = harness
        .ctx
        .provide::<DelayGateService>(gate.clone())
        .await
        .unwrap();
    let registering = {
        let registry = harness.registry.clone();
        let ctx = harness.ctx.clone();
        tokio::spawn(async move {
            registry
                .register(
                    &ctx,
                    definition("slow", json!([{"name":"fixture:delayed"}])),
                )
                .await
        })
    };
    gate.started.notified().await;
    let selecting_first = {
        let registry = harness.registry.clone();
        tokio::spawn(async move { registry.select(first, "slow").await })
    };
    tokio::task::yield_now().await;
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(1),
            harness.registry.select(second, "standard"),
        )
        .await
        .expect("a different agent must have an independent selection queue")
        .unwrap(),
        "standard"
    );
    gate.release.notify_one();
    registering.await.unwrap().unwrap();
    assert_eq!(selecting_first.await.unwrap().unwrap(), "slow");
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn a_panicking_selection_does_not_poison_the_agents_queue() {
    let harness = selection_fresh().await;
    for id in ["standard", "minimal"] {
        harness
            .registry
            .register(&harness.ctx, definition(id, json!([])))
            .await
            .unwrap();
    }
    let (_scope, agent) = panicking_agent(&harness.ctx, "panic-once");
    harness.registry.mount(agent.ctx(), None).await.unwrap();
    let error = harness
        .registry
        .select(agent.clone(), "minimal")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("fixture session panic"));
    assert_eq!(
        harness.registry.select(agent, "minimal").await.unwrap(),
        "minimal"
    );
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn retains_a_newly_acquired_generation_when_activation_is_replaced() {
    let (kernel, ctx, registry) = fresh();
    let gate = Arc::new(DelayGate {
        started: Default::default(),
        release: Default::default(),
    });
    let _provider = ctx.provide::<DelayGateService>(gate.clone()).await.unwrap();
    let registering = {
        let ctx = ctx.clone();
        let registry = registry.clone();
        tokio::spawn(async move {
            registry
                .register(
                    &ctx,
                    definition("standard", json!([{"name":"fixture:delayed"}])),
                )
                .await
        })
    };
    gate.started.notified().await;
    let acquiring = {
        let registry = registry.clone();
        tokio::spawn(async move { registry.acquire_scope(None).await })
    };
    gate.release.notify_one();
    let registration = registering.await.unwrap().unwrap();
    let lease = acquiring.await.unwrap().unwrap();
    registration.dispose().await;
    assert_eq!(registry.inspect_compositions(None).await.len(), 1);
    lease.dispose().await;
    assert!(registry.inspect_compositions(None).await.is_empty());
    kernel.shutdown().await;
}

#[tokio::test]
async fn repeated_current_selection_does_not_log_or_notify_again() {
    let harness = selection_fresh().await;
    for id in ["standard", "minimal"] {
        harness
            .registry
            .register(&harness.ctx, definition(id, json!([])))
            .await
            .unwrap();
    }
    let (_scope, agent) = agent(&harness.ctx, "repeated");
    harness.registry.mount(agent.ctx(), None).await.unwrap();
    harness
        .registry
        .select(agent.clone(), "minimal")
        .await
        .unwrap();
    harness
        .registry
        .select(agent.clone(), "minimal")
        .await
        .unwrap();
    let count = agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == "agent-preset/selected")
        .count();
    assert_eq!(count, 1);
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn logs_the_default_preset_when_selected_without_a_recorded_preset() {
    let harness = selection_fresh().await;
    harness
        .registry
        .register(&harness.ctx, definition("standard", json!([])))
        .await
        .unwrap();
    let (_scope, agent) = agent(&harness.ctx, "unrecorded");
    harness.registry.mount(agent.ctx(), None).await.unwrap();
    assert_eq!(
        harness
            .registry
            .preset_for_session(agent.session())
            .unwrap(),
        None
    );
    harness
        .registry
        .select(agent.clone(), "standard")
        .await
        .unwrap();
    assert_eq!(
        harness
            .registry
            .preset_for_session(agent.session())
            .unwrap(),
        Some("standard".into())
    );
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn recomposes_and_logs_a_repeated_selection_after_definition_replacement() {
    let harness = selection_fresh().await;
    harness
        .registry
        .register(&harness.ctx, definition("standard", json!([])))
        .await
        .unwrap();
    let old = harness
        .registry
        .register(&harness.ctx, definition("minimal", json!([])))
        .await
        .unwrap();
    let (_scope, agent) = agent(&harness.ctx, "redefined");
    harness.registry.mount(agent.ctx(), None).await.unwrap();
    harness
        .registry
        .select(agent.clone(), "minimal")
        .await
        .unwrap();
    old.dispose().await;
    harness
        .registry
        .register(&harness.ctx, definition("minimal", json!([])))
        .await
        .unwrap();
    harness
        .registry
        .select(agent.clone(), "minimal")
        .await
        .unwrap();
    let count = agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == "agent-preset/selected")
        .count();
    assert_eq!(count, 2);
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn does_not_lose_the_durable_selection_when_a_tools_observer_panics() {
    let harness = selection_fresh().await;
    for id in ["standard", "minimal"] {
        harness
            .registry
            .register(&harness.ctx, definition(id, json!([])))
            .await
            .unwrap();
    }
    let (_scope, agent) = agent(&harness.ctx, "observer");
    harness.registry.mount(agent.ctx(), None).await.unwrap();
    harness
        .ctx
        .on::<hoh_tools::ToolsChange>(|_| panic!("observer refused"))
        .unwrap();
    assert_eq!(
        harness
            .registry
            .select(agent.clone(), "minimal")
            .await
            .unwrap(),
        "minimal"
    );
    assert_eq!(
        harness
            .registry
            .preset_for_session(agent.session())
            .unwrap(),
        Some("minimal".into())
    );
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn inventories_failed_conditional_entries_without_executing_them_again() {
    let (kernel, ctx, registry) = fresh();
    let mut broken = definition(
        "broken",
        json!([{"name":"missing","disabled":{"__jsExpr":"false"}}]),
    );
    broken.name = Some("Broken".into());
    registry.register(&ctx, broken).await.unwrap();
    let inventory = registry.composition_inventory().await;
    assert!(inventory[0].broken.is_some());
    assert_eq!(inventory[0].rows[0].condition.as_deref(), Some("false"));
    kernel.shutdown().await;
}

#[tokio::test]
async fn registration_disposal_is_idempotent_at_the_registry_boundary() {
    let (kernel, ctx, registry) = fresh();
    let disposer = registry
        .register(&ctx, definition("standard", json!([])))
        .await
        .unwrap();
    disposer.dispose().await;
    registry
        .register(&ctx, definition("standard", json!([])))
        .await
        .unwrap();
    assert_eq!(registry.resolve(None).await.unwrap().id, "standard");
    kernel.shutdown().await;
}

#[tokio::test]
async fn refuses_to_inherit_a_revision_owned_by_a_different_registry() {
    let (kernel, ctx, registry) = fresh();
    let (other_kernel, other_ctx, other) = fresh();
    other_ctx
        .provide::<AgentPresets>(Arc::new(other.clone()))
        .await
        .unwrap();
    other
        .register(&other_ctx, definition("standard", json!([])))
        .await
        .unwrap();
    let parent = create_scope(&other_ctx, ScopeKey::new(), Default::default()).unwrap();
    other.mount(&parent.ctx, None).await.unwrap();
    let child = create_scope(&ctx, ScopeKey::new(), Default::default()).unwrap();
    assert!(registry.compose_from(&child.ctx, &parent.ctx).is_err());
    kernel.shutdown().await;
    other_kernel.shutdown().await;
}

#[tokio::test]
async fn validates_malformed_child_yaml_and_retains_diagnostics() {
    let (kernel, ctx, registry) = fresh();
    registry
        .register(&ctx, definition("invalid", json!([1])))
        .await
        .unwrap();
    assert!(
        registry
            .resolve(Some("invalid"))
            .await
            .unwrap()
            .broken
            .unwrap()
            .contains("not a plugin row")
    );
    assert!(registry.composition_inventory().await[0].rows.is_empty());
    kernel.shutdown().await;
}

#[test]
fn names_the_first_invalid_row() {
    let cases: Vec<(Value, Option<&str>)> = vec![
        (
            json!("nope"),
            Some("the composition must be a top-level list of plugin rows"),
        ),
        (
            json!([[]]),
            Some("row 1 is not a plugin row (expected a map with a \"name\")"),
        ),
        (
            json!([{"name":""}]),
            Some("row 1 names no plugin (a \"name\" string is required)"),
        ),
        (
            json!([{"name":"group","group":true,"config":"x"}]),
            Some("group row 1 must hold a list of plugin rows"),
        ),
        (
            json!([{"name":"group","group":true,"config":[{}]}]),
            Some("row 1 row 1 names no plugin (a \"name\" string is required)"),
        ),
        (
            json!([{"name":"group","group":true,"config":[{"name":"child"}]}]),
            None,
        ),
    ];
    for (rows, expected) in cases {
        assert_eq!(entry_list_problem(&rows).as_deref(), expected);
    }
}

#[tokio::test]
async fn reports_every_process_global_service_published_by_a_preset() {
    let (kernel, ctx, registry) = fresh();
    registry
        .register(&ctx, definition("leaks", json!([{"name":"fixture:leak"}])))
        .await
        .unwrap();
    assert!(
        registry
            .resolve(Some("leaks"))
            .await
            .unwrap()
            .broken
            .unwrap()
            .contains("fixtureService")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn reports_multiple_leaked_services_in_locale_sorted_order() {
    let (kernel, ctx, registry) = fresh();
    registry
        .register(
            &ctx,
            definition("leaks", json!([{"name":"fixture:multi-leak"}])),
        )
        .await
        .unwrap();
    assert_eq!(
        registry
            .resolve(Some("leaks"))
            .await
            .unwrap()
            .broken
            .unwrap(),
        "Preset services require isolate realms: presetAlpha, presetZulu"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn create_scope_failure_rejects_registration_and_removes_its_definition() {
    let (owner_kernel, _owner_ctx, registry) = fresh();
    owner_kernel.shutdown().await;
    let active_kernel = hoh_kernel::Kernel::start();
    let active_ctx = active_kernel.root();
    let result = registry
        .register(&active_ctx, definition("standard", json!([])))
        .await;
    assert!(result.is_err());
    assert!(registry.list().await.is_empty());
    active_kernel.shutdown().await;
}

#[test]
fn preset_definition_keeps_plugins_as_ordered_raw_json() {
    let definition: PresetDefinition = serde_json::from_value(json!({
        "id":"ordered","plugins":[{"name":"a","config":{"z":1,"a":2}}]
    }))
    .unwrap();
    let keys: Vec<_> = definition.plugins[0]["config"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(keys, ["z", "a"]);
}
