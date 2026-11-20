//! Review 213559: discovery error order, a failed projection registration and the list-models cases.

mod support;
use hoh_agent::{Agent, AgentOptions, AgentSessionMeta, Live};
use hoh_session::SessionId;
use hoh_session_projection::SessionProjections;
use hoh_timeout::AbortController;
use hoh_tool_subagent::{
    ToolSubagent, model_selection_settings::SubagentModelSelectionSettings,
    model_selection_state::subagent_model_selection_projection_definition,
};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;
use support::*;

const DISCOVERY: &str = "list_subagent_models";

fn info(result: &hoh_tools::ToolExecutionResult) -> (String, String) {
    let info = result.error().unwrap().info.as_ref().unwrap();
    (info.name.clone(), info.code.clone())
}

fn default_routes() -> Value {
    json!([
        { "provider": "alpha", "model": "fast" },
        { "provider": "alpha", "model": "plain" },
        { "provider": "beta", "model": "fast" },
        { "provider": "beta", "model": "plain" },
    ])
}

/// `setupListTool`: recorded allowlist, discovery owner, then the calling parent Agent.
async fn list_env(routes: Value) -> (Env, Arc<dyn Agent>) {
    let env = Env::new().await;
    env.with_routes(routes).await;
    env.mount(json!({ "provider": "unused", "modelDiscovery": true }))
        .await;
    let agent = env
        .harness
        .create(SessionId::from("discovery-parent"))
        .await
        .unwrap();
    (env, agent)
}

async fn list(env: &Env, agent: &Arc<dyn Agent>, args: Value) -> hoh_tools::ToolExecutionResult {
    env.call(DISCOVERY, args, Some(agent.clone())).await
}

#[tokio::test]
async fn a_failed_policy_registration_rejects_agent_publication_instead_of_hanging() {
    let env = Env::new().await;
    // The same key at another stateVersion makes the settings owner's registration fail.
    let mut other = subagent_model_selection_projection_definition();
    other.state_version = 2;
    let projections = env.ctx.get::<SessionProjections>().unwrap();
    let _held = projections.register(&env.ctx, other).await.unwrap();
    let plugin = env
        .ctx
        .plugin::<SubagentModelSelectionSettings>(json!({}))
        .unwrap();
    let created = tokio::time::timeout(
        Duration::from_secs(10),
        env.harness.create(SessionId::from("registration-window")),
    )
    .await
    .expect("agent publication hung behind a failed policy registration");
    let settled = tokio::time::timeout(Duration::from_secs(10), plugin.settled())
        .await
        .expect("the settings owner never settled");
    assert!(settled.is_err());
    drop(created);
    env.close().await;
}

// ---- item 1: error order (agent, then parent route, then llm) ----

#[tokio::test]
async fn list_without_agent_and_without_llm_reports_agent_first() {
    let env = NoLlmEnv::new().await;
    let result = env.call(None).await;
    assert!(result.is_error());
    assert!(
        text(&result).contains("requires a calling agent"),
        "{}",
        text(&result)
    );
    assert_eq!(
        info(&result),
        (
            "ToolContextUnavailableError".into(),
            "TOOL_CONTEXT_UNAVAILABLE".into()
        )
    );
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn list_without_parent_route_and_without_llm_reports_the_route_first() {
    let with_llm = Env::new().await;
    // No Session policy and a parent without provider/model.
    let agent = with_llm
        .harness
        .create(SessionId::from("no-route-parent"))
        .await
        .unwrap();
    let env = NoLlmEnv::new().await;
    let result = env.call(Some(agent)).await;
    assert!(result.is_error());
    assert!(
        text(&result).contains("without a parent provider and model"),
        "{}",
        text(&result)
    );
    assert_eq!(
        info(&result),
        (
            "ToolContextUnavailableError".into(),
            "TOOL_CONTEXT_UNAVAILABLE".into()
        )
    );
    env.kernel.shutdown().await;
    with_llm.close().await;
}

#[tokio::test]
async fn discovery_stays_registered_without_llm_and_rejects_calls() {
    let with_llm = Env::new().await;
    let agent = with_llm
        .harness
        .create_with(
            SessionId::from("parent-with-route"),
            AgentOptions {
                provider: Some(Live::Value("alpha".into())),
                model: Some(Live::Value("fast".into())),
                ..Default::default()
            },
            AgentSessionMeta::default(),
        )
        .await
        .unwrap();
    let env = NoLlmEnv::new().await;
    assert!(env.tools().get(DISCOVERY, None, None).is_some());
    let result = env.call(Some(agent)).await;
    assert!(result.is_error());
    assert!(
        text(&result).contains("`llm` service is unavailable"),
        "{}",
        text(&result)
    );
    assert_eq!(
        info(&result),
        (
            "ToolContextUnavailableError".into(),
            "TOOL_CONTEXT_UNAVAILABLE".into()
        )
    );
    env.kernel.shutdown().await;
    with_llm.close().await;
}

// ---- item 3: list-models.spec.ts cases through the registered tool ----

#[tokio::test]
async fn reads_the_recorded_session_allowlist() {
    let (env, agent) = list_env(default_routes()).await;
    let _alpha = env.catalog(&["alpha", "secret"], CatalogAdapter::new());
    let result = list(&env, &agent, json!({})).await;
    assert!(!result.is_error(), "{}", text(&result));
    assert_eq!(text(&result), "alpha — ALPHA API");
    env.close().await;
}

#[tokio::test]
async fn lists_registered_providers_and_follows_live_registration_changes() {
    let (env, agent) = list_env(default_routes()).await;
    let empty = list(&env, &agent, json!({})).await;
    assert!(!empty.is_error());
    assert_eq!(text(&empty), "(no LLM providers)");
    let registration = env.catalog(&["alpha"], CatalogAdapter::new());
    assert_eq!(
        text(&list(&env, &agent, json!({})).await),
        "alpha — ALPHA API"
    );
    registration.replace(vec!["beta".into()]).unwrap();
    assert_eq!(
        text(&list(&env, &agent, json!({})).await),
        "beta — BETA API"
    );
    env.close().await;
}

#[tokio::test]
async fn lists_one_providers_advertised_models_without_treating_the_catalog_as_a_whitelist() {
    let (env, agent) = list_env(default_routes()).await;
    let _alpha = env.catalog(&["alpha"], CatalogAdapter::new());
    let result = list(&env, &agent, json!({ "provider": "alpha" })).await;
    assert!(!result.is_error());
    assert_eq!(
        text(&result),
        "alpha/fast — Fast: Focused work.\nalpha/plain — Plain"
    );
    env.close().await;
}

#[tokio::test]
async fn intersects_provider_and_model_discovery_with_the_session_allowlist() {
    let (env, agent) = list_env(json!([
        { "provider": "alpha", "model": "fast" },
        { "provider": "alpha", "model": "unlisted" },
        { "provider": "missing", "model": "hidden" },
    ]))
    .await;
    let _both = env.catalog(&["alpha", "beta"], CatalogAdapter::new());
    assert_eq!(
        text(&list(&env, &agent, json!({})).await),
        "alpha — ALPHA API"
    );
    assert_eq!(
        text(&list(&env, &agent, json!({ "provider": "alpha" })).await),
        "alpha/fast — Fast: Focused work."
    );
    assert!(
        text(
            &list(
                &env,
                &agent,
                json!({ "provider": "alpha", "model": "unlisted" })
            )
            .await
        )
        .contains("alpha/unlisted — Fast")
    );
    let denied = list(
        &env,
        &agent,
        json!({ "provider": "alpha", "model": "plain" }),
    )
    .await;
    assert!(denied.is_error());
    assert!(text(&denied).contains("is not allowed for this Session"));
    assert_eq!(
        info(&denied),
        ("SubagentError".into(), "MODEL_ROUTE_NOT_ALLOWED".into())
    );
    env.close().await;
}

#[tokio::test]
async fn rejects_an_unauthorized_provider_before_calling_its_adapter_catalog() {
    let (env, agent) = list_env(json!([{ "provider": "alpha", "model": "fast" }])).await;
    let adapter = CatalogAdapter::new();
    let _both = env.catalog(&["alpha", "secret"], adapter.clone());
    let result = list(&env, &agent, json!({ "provider": "secret" })).await;
    assert!(result.is_error());
    assert!(text(&result).contains("provider \"secret\" is not allowed for this Session"));
    assert_eq!(
        info(&result),
        ("SubagentError".into(), "MODEL_ROUTE_NOT_ALLOWED".into())
    );
    assert_eq!(adapter.list_calls.load(Ordering::SeqCst), 0);
    env.close().await;
}

#[tokio::test]
async fn returns_an_empty_list_when_the_catalog_contains_no_authorized_model() {
    let (env, agent) = list_env(json!([{ "provider": "alpha", "model": "unlisted" }])).await;
    let _alpha = env.catalog(&["alpha"], CatalogAdapter::new());
    let result = list(&env, &agent, json!({ "provider": "alpha" })).await;
    assert!(!result.is_error());
    assert_eq!(text(&result), "(no advertised models for alpha)");
    env.close().await;
}

#[tokio::test]
async fn lists_authorized_catalog_entries_without_resolving_removed_or_unlisted_models() {
    let (env, agent) = list_env(json!([
        { "provider": "alpha", "model": "removed" },
        { "provider": "alpha", "model": "plain" },
    ]))
    .await;
    let adapter = CatalogAdapter::new();
    *adapter.fail_resolve.lock().unwrap() = Some("lookup unavailable".into());
    let _alpha = env.catalog(&["alpha"], adapter.clone());
    let result = list(&env, &agent, json!({ "provider": "alpha" })).await;
    assert!(!result.is_error(), "{}", text(&result));
    assert_eq!(text(&result), "alpha/plain — Plain");
    assert_eq!(adapter.resolve_calls.load(Ordering::SeqCst), 0);
    let exact = list(
        &env,
        &agent,
        json!({ "provider": "alpha", "model": "removed" }),
    )
    .await;
    assert!(exact.is_error());
    assert!(text(&exact).contains("lookup unavailable"));
    env.close().await;
}

#[tokio::test]
async fn rejects_a_failed_catalog_lookup() {
    let (env, agent) = list_env(default_routes()).await;
    let adapter = CatalogAdapter::new();
    *adapter.fail_list.lock().unwrap() = Some("catalog unavailable".into());
    let _alpha = env.catalog(&["alpha"], adapter);
    let result = list(&env, &agent, json!({ "provider": "alpha" })).await;
    assert!(result.is_error());
    assert!(text(&result).contains("catalog unavailable"));
    env.close().await;
}

#[tokio::test]
async fn rejects_cancellation_after_a_catalog_lookup() {
    let (env, agent) = list_env(default_routes()).await;
    let controller = Arc::new(AbortController::new());
    let adapter = CatalogAdapter::new();
    let hook = controller.clone();
    *adapter.on_list.lock().unwrap() = Some(Box::new(move || {
        hook.abort_with("discovery cancelled".to_owned())
    }));
    let _alpha = env.catalog(&["alpha"], adapter);
    let result = env
        .call_with(
            DISCOVERY,
            json!({ "provider": "alpha" }),
            Some(agent.clone()),
            controller.signal(),
        )
        .await;
    assert!(result.is_error());
    assert!(
        text(&result).contains("discovery cancelled"),
        "{}",
        text(&result)
    );
    env.close().await;
}

#[tokio::test]
async fn resolves_only_the_current_parent_model_without_a_session_policy() {
    for empty in [false, true] {
        let env = Env::new().await;
        env.mount(json!({ "provider": "unused", "modelDiscovery": true }))
            .await;
        let agent = env
            .harness
            .create_with(
                SessionId::from(format!("parent-only-{empty}")),
                AgentOptions {
                    provider: Some(Live::Value("alpha".into())),
                    model: Some(Live::Value("unlisted".into())),
                    ..Default::default()
                },
                AgentSessionMeta::default(),
            )
            .await
            .unwrap();
        let adapter = if empty {
            CatalogAdapter::empty()
        } else {
            CatalogAdapter::new()
        };
        let _alpha = env.catalog(&["alpha"], adapter.clone());
        let result = list(&env, &agent, json!({ "provider": "alpha" })).await;
        assert!(!result.is_error(), "{}", text(&result));
        assert_eq!(text(&result), "alpha/unlisted — Fast: Focused work.");
        assert_eq!(adapter.list_calls.load(Ordering::SeqCst), 0);
        env.close().await;
    }
}

#[tokio::test]
async fn inspects_exact_model_efforts_descriptions_and_defaults() {
    let (env, agent) = list_env(default_routes()).await;
    let _both = env.catalog(&["alpha", "secret"], CatalogAdapter::new());
    let result = list(
        &env,
        &agent,
        json!({ "provider": "alpha", "model": "fast" }),
    )
    .await;
    assert!(!result.is_error());
    assert_eq!(
        text(&result),
        "alpha/fast — Fast: Focused work.\nReasoning efforts:\nlow — Low\nhigh (default) — High: Quality first."
    );
    env.close().await;
}

#[tokio::test]
async fn renders_exact_models_without_reasoning_metadata() {
    let (env, agent) = list_env(default_routes()).await;
    let _alpha = env.catalog(&["alpha"], CatalogAdapter::new());
    let result = list(
        &env,
        &agent,
        json!({ "provider": "alpha", "model": "plain" }),
    )
    .await;
    assert!(!result.is_error());
    assert_eq!(
        text(&result),
        "alpha/plain — Plain\nReasoning efforts:\n(no advertised reasoning efforts)"
    );
    env.close().await;
}

#[tokio::test]
async fn rejects_incomplete_or_unavailable_provider_requests() {
    for (args, expected, name, code) in [
        (
            json!({ "model": "fast" }),
            "`model` requires `provider`",
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "provider": "" }),
            "`provider` must be non-empty",
            "ToolArgsError",
            "INVALID_ARGS",
        ),
        (
            json!({ "provider": "missing" }),
            "is not allowed for this Session",
            "SubagentError",
            "MODEL_ROUTE_NOT_ALLOWED",
        ),
    ] {
        let (env, agent) = list_env(default_routes()).await;
        let result = list(&env, &agent, args).await;
        assert!(result.is_error());
        assert!(text(&result).contains(expected), "{}", text(&result));
        assert_eq!(info(&result), (name.into(), code.into()));
        env.close().await;
    }
}

#[tokio::test]
async fn rejects_an_empty_exact_model_after_resolving_the_provider() {
    let (env, agent) = list_env(default_routes()).await;
    let _alpha = env.catalog(&["alpha"], CatalogAdapter::new());
    let result = list(&env, &agent, json!({ "provider": "alpha", "model": "" })).await;
    assert!(result.is_error());
    assert!(text(&result).contains("`model` must be non-empty"));
    assert_eq!(
        info(&result),
        ("ToolArgsError".into(), "INVALID_ARGS".into())
    );
    env.close().await;
}

#[tokio::test]
async fn reports_registered_alternatives_for_an_unavailable_provider() {
    let (env, agent) = list_env(json!([
        { "provider": "alpha", "model": "fast" },
        { "provider": "missing", "model": "fast" },
    ]))
    .await;
    let _alpha = env.catalog(&["alpha"], CatalogAdapter::new());
    let result = list(&env, &agent, json!({ "provider": "missing" })).await;
    assert!(result.is_error());
    assert!(text(&result).contains("available providers: alpha"));
    assert_eq!(
        info(&result),
        ("SubagentError".into(), "LLM_PROVIDER_NOT_FOUND".into())
    );
    assert!(!text(&result).contains("secret"));
    env.close().await;
}

#[tokio::test]
async fn reports_no_available_provider_when_the_authorized_registry_intersection_is_empty() {
    let (env, agent) = list_env(json!([{ "provider": "missing", "model": "fast" }])).await;
    let result = list(&env, &agent, json!({ "provider": "missing" })).await;
    assert!(result.is_error());
    assert!(text(&result).contains("available providers: (none)"));
    assert_eq!(
        info(&result),
        ("SubagentError".into(), "LLM_PROVIDER_NOT_FOUND".into())
    );
    env.close().await;
}

// ---- registration and the definition itself ----

#[tokio::test]
async fn registers_discovery_only_on_its_configured_owner_and_removes_it_on_unload() {
    let env = Env::new().await;
    let _mock = env
        .subagents()
        .register_provider(Arc::new(Scripted::new("mock")))
        .unwrap();
    env.mount(json!({ "provider": "mock", "modelSelection": true }))
        .await;
    let sibling = env
        .ctx
        .plugin::<ToolSubagent>(json!({
            "provider": "mock", "toolName": "other_subagent", "modelSelection": true
        }))
        .unwrap();
    sibling.settled().await.unwrap();
    assert!(env.tools().get(DISCOVERY, None, None).is_none());
    let owner_config =
        json!({ "provider": "mock", "toolName": "discovery_subagent", "modelDiscovery": true });
    let owner = env
        .ctx
        .plugin::<ToolSubagent>(owner_config.clone())
        .unwrap();
    owner.settled().await.unwrap();
    let schemas = env.tools().schemas(None, None).unwrap();
    assert_eq!(schemas.iter().filter(|s| s.name == DISCOVERY).count(), 1);
    let discovery_subagent = schemas
        .iter()
        .find(|s| s.name == "discovery_subagent")
        .unwrap();
    assert!(
        discovery_subagent.parameters["properties"]
            .get("model")
            .is_none()
    );
    sibling.dispose().await;
    let definition = env.tools().get(DISCOVERY, None, None).unwrap();
    assert!(definition.is_concurrency_safe.as_ref().unwrap()(&json!({})));
    owner.dispose().await;
    assert!(env.tools().get(DISCOVERY, None, None).is_none());
    assert!(env.tools().get("subagent", None, None).is_some());
    let reloaded = env.ctx.plugin::<ToolSubagent>(owner_config).unwrap();
    reloaded.settled().await.unwrap();
    let count = env
        .tools()
        .schemas(None, None)
        .unwrap()
        .iter()
        .filter(|s| s.name == DISCOVERY)
        .count();
    assert_eq!(count, 1);
    reloaded.dispose().await;
    assert!(env.tools().get(DISCOVERY, None, None).is_none());
    env.close().await;
}

#[tokio::test]
async fn rejects_a_second_discovery_owner_in_the_same_scope_without_removing_the_first() {
    let env = Env::new().await;
    let _mock = env
        .subagents()
        .register_provider(Arc::new(Scripted::new("mock")))
        .unwrap();
    env.mount(json!({ "provider": "mock", "modelDiscovery": true }))
        .await;
    let definition = env.tools().get(DISCOVERY, None, None).unwrap();
    let second = env
        .ctx
        .plugin::<ToolSubagent>(json!({
            "provider": "mock", "toolName": "other_subagent", "modelDiscovery": true
        }))
        .unwrap();
    let error = second.settled().await.unwrap_err();
    assert!(error.to_string().contains("already registered"), "{error}");
    assert!(Arc::ptr_eq(
        &env.tools().get(DISCOVERY, None, None).unwrap(),
        &definition
    ));
    assert!(env.tools().get("subagent", None, None).is_some());
    env.close().await;
}

#[tokio::test]
async fn keeps_discovery_registration_and_disposal_local_to_each_tool_scope() {
    let env = Env::new().await;
    let _mock = env
        .subagents()
        .register_provider(Arc::new(Scripted::new("mock")))
        .unwrap();
    let first = env
        .harness
        .create(SessionId::from("first-owner"))
        .await
        .unwrap();
    let second = env
        .harness
        .create(SessionId::from("second-owner"))
        .await
        .unwrap();
    let config = json!({ "provider": "mock", "modelDiscovery": true });
    let first_owner = first.ctx().plugin::<ToolSubagent>(config.clone()).unwrap();
    first_owner.settled().await.unwrap();
    let second_owner = second.ctx().plugin::<ToolSubagent>(config).unwrap();
    second_owner.settled().await.unwrap();
    let (first_key, second_key) = (first.scope_key(), second.scope_key());
    let first_definition = env.tools().get(DISCOVERY, Some(&first_key), None).unwrap();
    let second_definition = env.tools().get(DISCOVERY, Some(&second_key), None).unwrap();
    assert!(!Arc::ptr_eq(&first_definition, &second_definition));
    assert!(env.tools().get(DISCOVERY, None, None).is_none());
    first_owner.dispose().await;
    assert!(env.tools().get(DISCOVERY, Some(&first_key), None).is_none());
    assert!(Arc::ptr_eq(
        &env.tools().get(DISCOVERY, Some(&second_key), None).unwrap(),
        &second_definition
    ));
    second_owner.dispose().await;
    assert!(
        env.tools()
            .get(DISCOVERY, Some(&second_key), None)
            .is_none()
    );
    env.close().await;
}

/// Bytes of `registerListSubagentModels` in `src/list-models.ts`: description (concatenated
/// literal pieces) and the two parameters.
#[tokio::test]
async fn registered_definition_is_byte_identical_to_the_source() {
    let env = Env::new().await;
    let _mock = env
        .subagents()
        .register_provider(Arc::new(Scripted::new("mock")))
        .unwrap();
    env.mount(json!({ "provider": "mock", "modelDiscovery": true }))
        .await;
    let schema = env
        .tools()
        .schemas(None, None)
        .unwrap()
        .into_iter()
        .find(|schema| schema.name == DISCOVERY)
        .unwrap();
    assert_eq!(
        schema.description,
        concat!(
            "Inspect the routes available for explicit subagent model selection. No arguments: authorized providers. ",
            "`provider`: that provider's authorized models. `provider` + `model`: that model and its reasoning efforts. ",
            "Query when choosing an explicit child route or reasoning effort. ",
            "Omit optional fields rather than sending empty strings."
        )
    );
    assert_eq!(
        serde_json::to_string(&schema.parameters["properties"]).unwrap(),
        r#"{"provider":{"type":"string","description":"Registered LLM provider id."},"model":{"type":"string","description":"Exact model id; requires provider."}}"#
    );
    let definition = env.tools().get(DISCOVERY, None, None).unwrap();
    assert_eq!(
        serde_json::to_string(&definition.output.schema).unwrap(),
        r#"{"type":"string"}"#
    );
    env.close().await;
}

#[tokio::test]
async fn an_agent_preset_without_the_presets_service_fails_the_mount() {
    let env = Env::new().await;
    let plugin = env
        .ctx
        .plugin::<ToolSubagent>(json!({ "provider": "unused", "agentPreset": "reviewer" }))
        .unwrap();
    let error = plugin.settled().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("agent preset selection requires the agent-presets service"),
        "{error}"
    );
    env.close().await;
}
