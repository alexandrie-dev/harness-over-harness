use hoh_agent::{AgentOptions, Live};
use hoh_error::HarnessError;
use hoh_subagent::{SubagentCapabilities, SubagentError, SubagentProvider};
use hoh_tool_subagent::model_selection::{
    AllowedModelRoute, DelegationModelRequest, ModelSelectionPolicy, assert_allowed_model_routes,
    assert_allowed_model_selection, has_configured_llm_selection, has_delegation_model_request,
    model_route_key, requested_agent_options,
};
use hoh_tool_subagent::model_selection_state::subagent_model_selection_projection_definition;
use hoh_tool_subagent::preset_selection::{PresetSelectionConfig, SubagentPresetSelection};
use hoh_tool_subagent::{Config, provider_wording};
use hoh_tools::ToolArgsError;
use serde_json::{Value, json};
use std::sync::Arc;

fn route(provider: &str, model: &str) -> AllowedModelRoute {
    AllowedModelRoute {
        provider: provider.into(),
        model: model.into(),
    }
}

fn options(provider: &str, model: &str, effort: Option<&str>) -> AgentOptions {
    AgentOptions {
        provider: Some(Live::Value(provider.into())),
        model: Some(Live::Value(model.into())),
        reasoning_effort: effort.map(|value| Live::Value(value.into())),
        max_tokens: Some(Live::Value(777)),
    }
}

fn harness(error: &hoh_error::BoxError) -> &HarnessError {
    HarnessError::of(error.as_ref()).expect("HarnessError")
}

fn selection_error(
    result: Result<Option<AgentOptions>, hoh_error::BoxError>,
) -> hoh_error::BoxError {
    match result {
        Ok(_) => panic!("expected selection failure"),
        Err(error) => error,
    }
}

#[test]
fn route_key_is_unambiguous() {
    assert_eq!(model_route_key(&route("a/b", "c")), "a/b\0c");
    assert_ne!(
        model_route_key(&route("a", "b/c")),
        model_route_key(&route("a/b", "c"))
    );
}

#[test]
fn route_policy_rejects_empty_provider_and_model() {
    for candidate in [route("", "m"), route("p", "")] {
        assert_eq!(
            assert_allowed_model_routes(&[candidate])
                .unwrap_err()
                .to_string(),
            "subagent model selection requires non-empty provider and model ids"
        );
    }
}

#[test]
fn route_policy_rejects_exact_duplicates_only() {
    let duplicate = [route("p", "m"), route("p", "m")];
    assert_eq!(
        assert_allowed_model_routes(&duplicate)
            .unwrap_err()
            .to_string(),
        "subagent model selection repeats route \"p/m\""
    );
    assert_allowed_model_routes(&[route("p", "m"), route("p", "n"), route("q", "m")]).unwrap();
}

#[test]
fn delegation_request_presence_counts_each_field() {
    assert!(!has_delegation_model_request(&Default::default()));
    for request in [
        DelegationModelRequest {
            provider: Some("p".into()),
            ..Default::default()
        },
        DelegationModelRequest {
            model: Some("m".into()),
            ..Default::default()
        },
        DelegationModelRequest {
            reasoning_effort: Some("high".into()),
            ..Default::default()
        },
    ] {
        assert!(has_delegation_model_request(&request));
    }
}

#[test]
fn omitted_request_preserves_configured_options() {
    let configured = options("child", "m", Some("high"));
    let result = requested_agent_options(
        Some(&configured),
        &Default::default(),
        false,
        None,
        &options("parent", "p", None),
    )
    .unwrap()
    .unwrap();
    assert_eq!(result.provider().as_deref(), Some("child"));
    assert_eq!(result.model().as_deref(), Some("m"));
    assert_eq!(
        result
            .reasoning_effort()
            .map(|value| value.to_string())
            .as_deref(),
        Some("high")
    );
    assert_eq!(result.max_tokens(), Some(777));
}

#[test]
fn explicit_selection_requires_enabled_instance() {
    let error = selection_error(requested_agent_options(
        None,
        &DelegationModelRequest {
            provider: Some("p".into()),
            model: Some("m".into()),
            reasoning_effort: None,
        },
        false,
        None,
        &options("p", "m", None),
    ));
    assert!(harness(&error).is_named(SubagentError::NAME));
    assert_eq!(harness(&error).code(), "MODEL_SELECTION_DISABLED");
}

#[test]
fn explicit_selection_rejects_empty_values() {
    for (request, field) in [
        (
            DelegationModelRequest {
                provider: Some("".into()),
                model: Some("m".into()),
                reasoning_effort: None,
            },
            "provider",
        ),
        (
            DelegationModelRequest {
                provider: Some("p".into()),
                model: Some("".into()),
                reasoning_effort: None,
            },
            "model",
        ),
        (
            DelegationModelRequest {
                provider: Some("p".into()),
                model: Some("m".into()),
                reasoning_effort: Some("".into()),
            },
            "reasoning_effort",
        ),
    ] {
        let error = selection_error(requested_agent_options(
            None,
            &request,
            true,
            None,
            &options("p", "m", None),
        ));
        assert_eq!(harness(&error).code(), ToolArgsError::CODE);
        assert_eq!(
            harness(&error).message(),
            format!("invalid arguments: child LLM `{field}` must be non-empty")
        );
    }
}

#[test]
fn explicit_selection_requires_provider_and_model_together() {
    let error = selection_error(requested_agent_options(
        None,
        &DelegationModelRequest {
            provider: Some("p".into()),
            ..Default::default()
        },
        true,
        None,
        &options("p", "m", None),
    ));
    assert_eq!(harness(&error).code(), ToolArgsError::CODE);
    assert_eq!(
        harness(&error).message(),
        "invalid arguments: child LLM `provider` and `model` must be supplied together"
    );
}

#[test]
fn effort_without_complete_route_has_specific_diagnostic() {
    let error = selection_error(requested_agent_options(
        None,
        &DelegationModelRequest {
            reasoning_effort: Some("high".into()),
            ..Default::default()
        },
        true,
        None,
        &options("p", "m", None),
    ));
    assert_eq!(
        harness(&error).message(),
        "invalid arguments: child LLM `reasoning_effort` requires both `provider` and `model`; supply both or omit all three fields"
    );
}

#[test]
fn absent_policy_allows_only_parent_route() {
    let parent = options("p", "m", Some("medium"));
    assert_allowed_model_selection(None, &parent, &route("p", "m")).unwrap();
    let error = assert_allowed_model_selection(None, &parent, &route("p", "other")).unwrap_err();
    assert_eq!(harness(&error).code(), "MODEL_ROUTE_NOT_ALLOWED");
    assert!(
        harness(&error)
            .message()
            .contains("limited to the parent route")
    );
}

#[test]
fn recorded_policy_replaces_parent_fallback() {
    let policy = ModelSelectionPolicy {
        routes: vec![route("child", "m2")],
    };
    assert_allowed_model_selection(
        Some(&policy),
        &options("p", "m", None),
        &route("child", "m2"),
    )
    .unwrap();
    let error =
        assert_allowed_model_selection(Some(&policy), &options("p", "m", None), &route("p", "m"))
            .unwrap_err();
    assert_eq!(
        harness(&error).message(),
        "child LLM route \"p/m\" is not allowed for this Session"
    );
}

#[test]
fn explicit_route_overlays_config_and_clears_effort() {
    let configured = options("old", "old", Some("high"));
    let parent = options("parent", "parent", Some("medium"));
    let policy = ModelSelectionPolicy {
        routes: vec![route("new", "new")],
    };
    let result = requested_agent_options(
        Some(&configured),
        &DelegationModelRequest {
            provider: Some("new".into()),
            model: Some("new".into()),
            reasoning_effort: None,
        },
        true,
        Some(&policy),
        &parent,
    )
    .unwrap()
    .unwrap();
    assert_eq!(result.provider().as_deref(), Some("new"));
    assert_eq!(result.model().as_deref(), Some("new"));
    assert_eq!(result.reasoning_effort(), None);
    assert_eq!(result.max_tokens(), Some(777));
}

#[test]
fn configured_llm_selection_detects_route_or_effort_but_not_token_limit() {
    assert!(!has_configured_llm_selection(None));
    assert!(!has_configured_llm_selection(Some(&AgentOptions {
        max_tokens: Some(Live::Value(10)),
        ..Default::default()
    })));
    assert!(has_configured_llm_selection(Some(&AgentOptions {
        model: Some(Live::Value("m".into())),
        ..Default::default()
    })));
}

#[derive(Clone)]
struct Provider {
    capabilities: SubagentCapabilities,
    inherits: bool,
}

impl SubagentProvider for Provider {
    fn name(&self) -> &str {
        "test"
    }
    fn capabilities(&self) -> SubagentCapabilities {
        self.capabilities
    }
    fn inherits_parent_context(&self) -> bool {
        self.inherits
    }
}

fn preset(
    default: Option<&str>,
    allowed: Option<&[&str]>,
) -> Result<SubagentPresetSelection, hoh_error::BoxError> {
    SubagentPresetSelection::new(PresetSelectionConfig {
        agent_preset: default.map(str::to_owned),
        allowed_agent_presets: allowed
            .map(|values| values.iter().map(|value| (*value).to_owned()).collect()),
    })
}

#[test]
fn preset_configuration_rejects_duplicates_and_missing_default() {
    assert_eq!(
        preset(None, Some(&["a", "a"])).unwrap_err().to_string(),
        "tool-subagent: allowedAgentPresets contains duplicate preset ids"
    );
    assert_eq!(
        preset(Some("a"), Some(&["b"])).unwrap_err().to_string(),
        "tool-subagent: agentPreset must be included in allowedAgentPresets"
    );
}

#[test]
fn preset_metadata_matches_omission_behavior() {
    let inherited = preset(None, None).unwrap();
    assert!(!inherited.configured());
    assert_eq!(inherited.choices(), None);
    assert_eq!(
        inherited.description(),
        "Agent preset for this child. Omit to inherit the parent agent's current preset."
    );
    let configured = preset(Some("worker"), Some(&["worker", "reviewer"])).unwrap();
    assert!(configured.configured());
    assert_eq!(configured.choices().unwrap(), ["worker", "reviewer"]);
    assert_eq!(
        configured.description(),
        "Agent preset for this child. Omit to use worker."
    );
}

#[test]
fn preset_requires_independent_supported_provider() {
    let selection = preset(Some("worker"), None).unwrap();
    for provider in [
        Provider {
            capabilities: SubagentCapabilities::default(),
            inherits: false,
        },
        Provider {
            capabilities: SubagentCapabilities {
                agent_preset: true,
                ..Default::default()
            },
            inherits: true,
        },
    ] {
        assert_eq!(
            selection
                .assert_provider(&provider)
                .unwrap_err()
                .to_string(),
            "tool-subagent: provider \"test\" does not support independent agent preset selection"
        );
    }
    selection
        .assert_provider(&Provider {
            capabilities: SubagentCapabilities {
                agent_preset: true,
                ..Default::default()
            },
            inherits: false,
        })
        .unwrap();
}

#[test]
fn preset_resolution_applies_default_and_allowlist() {
    let selection = preset(Some("worker"), Some(&["worker", "reviewer"])).unwrap();
    assert_eq!(selection.resolve(None).unwrap().as_deref(), Some("worker"));
    assert_eq!(
        selection
            .resolve(Some(&json!("reviewer")))
            .unwrap()
            .as_deref(),
        Some("reviewer")
    );
    let error = selection.resolve(Some(&json!("other"))).unwrap_err();
    assert_eq!(harness(&error).code(), "PRESET_NOT_ALLOWED");
    assert_eq!(
        harness(&error).message(),
        "agent_preset must be one of: worker, reviewer"
    );
}

#[test]
fn model_preset_selection_disabled_is_distinct_from_default() {
    let selection = preset(Some("worker"), None).unwrap();
    let error = selection.resolve(Some(&json!("worker"))).unwrap_err();
    assert_eq!(harness(&error).code(), "PRESET_SELECTION_DISABLED");
}

#[test]
fn provider_wording_distinguishes_fresh_and_inherited_history() {
    let (fresh, fresh_prompt) = provider_wording(false);
    assert!(fresh.contains("own context"));
    assert!(fresh_prompt.contains("does not share"));
    let (forked, forked_prompt) = provider_wording(true);
    assert!(forked.contains("inherits this conversation"));
    assert!(forked.contains("does not see the current in-flight turn"));
    assert!(forked_prompt.contains("already sees"));
}

#[test]
fn removed_config_fields_fail_with_migration_text() {
    for (field, text) in [("persona", "persona"), ("toolFilter", "toolFilter")] {
        let error = serde_json::from_value::<Config>(json!({ "provider": "spawn", field: null }))
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("tool-subagent: {text} is no longer supported"))
        );
    }
}

#[test]
fn config_defaults_tool_and_selection_switches() {
    let config: Config = serde_json::from_value(json!({ "provider": "spawn" })).unwrap();
    assert_eq!(config.tool_name, "subagent");
    assert!(!config.model_selection);
    assert!(!config.model_discovery);
}

#[test]
fn policy_projection_is_first_event_wins_and_rejects_empty() {
    let definition = subagent_model_selection_projection_definition();
    let initial = Arc::new(None);
    let first = hoh_session::SessionEvent::from_value(json!({
        "seq": 0, "time": 0, "type": "subagent/model-selection-policy",
        "data": { "allowedModels": [{ "provider": "p", "model": "m" }] }
    }));
    let selected = (definition.apply)(&initial, &first).unwrap();
    assert_eq!(selected.as_ref(), &Some(vec![route("p", "m")]));
    let second = hoh_session::SessionEvent::from_value(json!({
        "seq": 1, "time": 0, "type": "subagent/model-selection-policy",
        "data": { "allowedModels": [{ "provider": "q", "model": "n" }] }
    }));
    assert_eq!(
        (definition.apply)(&selected, &second).unwrap().as_ref(),
        selected.as_ref()
    );
    let empty = hoh_session::SessionEvent::from_value(json!({
        "seq": 0, "time": 0, "type": "subagent/model-selection-policy",
        "data": { "allowedModels": [] }
    }));
    assert_eq!(
        (definition.apply)(&initial, &empty)
            .unwrap_err()
            .to_string(),
        "subagent/model-selection-policy requires at least one route"
    );
}

#[test]
fn policy_projection_schema_preserves_null_and_validates_routes() {
    let definition = subagent_model_selection_projection_definition();
    assert_eq!(
        (definition.state_schema)(Some(&Value::Null))
            .unwrap()
            .as_ref(),
        &None
    );
    // The source's zod state schema does not look for duplicates; only `apply` does.
    let duplicate = json!([{ "provider": "p", "model": "m" }, { "provider": "p", "model": "m" }]);
    assert!((definition.state_schema)(Some(&duplicate)).is_ok());
    let event = hoh_session::SessionEvent::from_value(json!({
        "seq": 0, "time": 0, "type": "subagent/model-selection-policy",
        "data": { "allowedModels": duplicate }
    }));
    assert_eq!(
        (definition.apply)(&Arc::new(None), &event)
            .unwrap_err()
            .to_string(),
        "subagent model selection repeats route \"p/m\""
    );
}
