mod support;

use hoh_system_prompt::render_context_snapshot;
use serde_json::json;

use support::{Env, error_code, text};

#[tokio::test]
async fn registers_the_exact_send_and_interrupt_schemas() {
    let env = Env::new(json!({})).await;
    let schemas = env.tools().schemas(None, None).unwrap();

    let send = schemas
        .iter()
        .find(|schema| schema.name == "send_message")
        .unwrap();
    let send_properties = send.parameters["properties"].as_object().unwrap();
    assert_eq!(
        send_properties.keys().cloned().collect::<Vec<_>>(),
        ["agent", "message"]
    );
    assert!(send.description.contains("receives it at its next step"));
    assert!(!send.description.contains("job id"));
    assert_eq!(
        send_properties["agent"]["description"],
        "The name of another agent in your subagent tree, as shown by list_agents."
    );

    let interrupt = schemas
        .iter()
        .find(|schema| schema.name == "interrupt_agent")
        .unwrap();
    let interrupt_properties = interrupt.parameters["properties"].as_object().unwrap();
    assert_eq!(
        interrupt_properties.keys().cloned().collect::<Vec<_>>(),
        ["agent"]
    );
    assert!(interrupt.description.contains("current work"));
    assert!(interrupt.description.contains("without waiting"));
    env.close().await;
}

#[tokio::test]
async fn interrupt_can_be_disabled_without_removing_other_controls() {
    let env = Env::new(json!({ "interrupt": false })).await;
    assert!(env.tools().get("interrupt_agent", None, None).is_none());
    for name in ["send_message", "list_agents", "wait_agent"] {
        assert!(
            env.tools().get(name, None, None).is_some(),
            "missing {name}"
        );
    }
    env.close().await;
}

#[tokio::test]
async fn root_identity_precedes_collaboration_guidance() {
    let env = Env::new(json!({})).await;
    let assembled = env.assembly(&env.parent).await;
    let snapshot = render_context_snapshot(&assembled).unwrap();
    let identity = snapshot
        .find("You are the root agent. Your name is \"lead\".")
        .unwrap();
    let send = snapshot.find("To message other agents").unwrap();
    let list = snapshot.find("Use list_agents({})").unwrap();
    assert!(identity < send && send < list);
    env.close().await;
}

#[tokio::test]
async fn calls_without_an_agent_fail_loud() {
    let env = Env::new(json!({})).await;
    for (name, arguments) in [
        (
            "send_message",
            json!({ "agent": "nobody", "message": "hi" }),
        ),
        ("interrupt_agent", json!({ "agent": "nobody" })),
    ] {
        let result = env.call(name, arguments, None).await;
        assert!(result.is_error());
        assert_eq!(error_code(&result), Some("TOOL_CONTEXT_UNAVAILABLE"));
        assert!(text(&result).contains("requires a calling agent"));
    }
    env.close().await;
}

#[tokio::test]
async fn message_resolution_and_self_delivery_fail_with_subagent_codes() {
    let env = Env::new(json!({})).await;
    let missing = env
        .call(
            "send_message",
            json!({ "agent": "missing/child", "message": "hello" }),
            Some(env.parent.clone()),
        )
        .await;
    assert_eq!(error_code(&missing), Some("AGENT_NAME_NOT_FOUND"));
    assert!(text(&missing).contains("use list_agents"));

    let own_root = env
        .call(
            "send_message",
            json!({ "agent": "lead", "message": "hello" }),
            Some(env.parent.clone()),
        )
        .await;
    assert_eq!(error_code(&own_root), Some("UNAUTHORIZED"));
    assert!(text(&own_root).contains("requires another agent"));
    env.close().await;
}

#[tokio::test]
async fn interrupting_an_absent_target_is_an_accepted_no_op() {
    let env = Env::new(json!({})).await;
    let result = env
        .call(
            "interrupt_agent",
            json!({ "agent": "absent" }),
            Some(env.parent.clone()),
        )
        .await;
    assert_eq!(result.value(), Some(&json!({ "accepted": true })));
    assert_eq!(text(&result), "interrupt requested for agent absent");
    env.close().await;
}

#[test]
fn config_defaults_interruption_on_and_rejects_non_boolean_values() {
    assert!(hoh_tool_subagent_control::Config::default().interrupt);

    let error = serde_json::from_value::<hoh_tool_subagent_control::Config>(json!({
        "interrupt": "yes"
    }))
    .unwrap_err();
    assert!(error.to_string().contains("boolean"));
}
