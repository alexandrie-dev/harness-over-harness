//! Port of the declarative-config rules of index.ts (config-session-id.spec.ts validation cases,
//! settings.spec.ts defaults).
use hoh_agent_loop::{
    Config, ConfiguredAgent, DEFAULT_MAX_PARALLEL_TOOL_CALLS, LauncherAgentIdentity,
    apply_launcher_identities, validate_configured_agents,
};
use hoh_session::SessionId;
use std::collections::HashMap;

fn agent(id: &str) -> ConfiguredAgent {
    ConfiguredAgent {
        id: id.into(),
        ..ConfiguredAgent::default()
    }
}

#[test]
fn defaults_the_scheduler_budget_to_ten() {
    assert_eq!(DEFAULT_MAX_PARALLEL_TOOL_CALLS, 10);
    assert_eq!(Config::default().max_parallel_tool_calls.0, 10);
    let parsed: Config = serde_json::from_value(serde_json::json!({ "agents": [] })).unwrap();
    assert_eq!(parsed.max_parallel_tool_calls.0, 10);
}

#[test]
fn rejects_session_id_together_with_resume_session_id() {
    let mut both = agent("a");
    both.session_id = Some("x".into());
    both.resume_session_id = Some("y".into());
    let error = validate_configured_agents(&[both]).unwrap_err();
    assert_eq!(
        error.to_string(),
        "agent \"a\": sessionId and resumeSessionId are mutually exclusive"
    );
}

#[test]
fn rejects_duplicate_exact_identities_across_both_keys() {
    let mut first = agent("first");
    first.session_id = Some("same".into());
    let mut second = agent("second");
    second.resume_session_id = Some("same".into());
    let error = validate_configured_agents(&[first, second]).unwrap_err();
    assert_eq!(
        error.to_string(),
        "agents \"first\" and \"second\" use duplicate exact session identity \"same\""
    );
}

#[test]
fn treats_an_empty_resume_id_as_absent() {
    let mut a = agent("a");
    a.session_id = Some("x".into());
    a.resume_session_id = Some(String::new());
    validate_configured_agents(&[a]).unwrap();
}

#[test]
fn launcher_identities_replace_both_config_identity_keys() {
    let mut configured = agent("a");
    configured.session_id = Some("config-id".into());
    let untouched = agent("b");
    let identities = HashMap::from([(
        "a".to_owned(),
        LauncherAgentIdentity {
            id: SessionId::from("launcher-id"),
            resume: true,
        },
    )]);
    let applied = apply_launcher_identities(&[configured, untouched.clone()], Some(&identities));
    assert_eq!(applied[0].session_id, None);
    assert_eq!(applied[0].resume_session_id.as_deref(), Some("launcher-id"));
    assert_eq!(applied[1], untouched);

    let fresh = HashMap::from([(
        "a".to_owned(),
        LauncherAgentIdentity {
            id: SessionId::from("fresh-id"),
            resume: false,
        },
    )]);
    let mut resumed = agent("a");
    resumed.resume_session_id = Some("old".into());
    let applied = apply_launcher_identities(&[resumed], Some(&fresh));
    assert_eq!(applied[0].resume_session_id, None);
    assert_eq!(applied[0].session_id.as_deref(), Some("fresh-id"));
}
