mod support;

use hoh_kernel::Plugin;
use hoh_session::{SessionHeader, SessionId};
use hoh_tool_subagent_control::ToolSubagentControl;
use hoh_tools::ToolExecutionResult;
use serde_json::json;

use support::{Env, error_code, text};

#[tokio::test]
async fn registers_one_parameterless_concurrency_safe_tool() {
    let env = Env::new(json!({})).await;
    let schemas = env.tools().schemas(None, None).unwrap();
    let matching = schemas
        .iter()
        .filter(|schema| schema.name == "list_agents")
        .collect::<Vec<_>>();
    assert_eq!(matching.len(), 1);
    assert!(
        matching[0]
            .parameters
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .is_none_or(serde_json::Map::is_empty)
    );
    assert!(
        matching[0]
            .parameters
            .get("required")
            .and_then(serde_json::Value::as_array)
            .is_none_or(Vec::is_empty)
    );
    assert!(
        matching[0]
            .description
            .contains("You can message any other agent in the subagent tree")
    );

    let definition = env.tools().get("list_agents", None, None).unwrap();
    assert!(definition.is_concurrency_safe.as_ref().unwrap()(&json!({})));
    let vocabulary = &definition.output.schema["items"]["properties"]["status"]["enum"];
    assert_eq!(
        vocabulary,
        &json!([
            "running",
            "inactive",
            "corrupt",
            "unsupported",
            "unavailable"
        ])
    );
    env.close().await;
}

#[tokio::test]
async fn renders_the_root_when_the_tree_has_no_children() {
    let env = Env::new(json!({})).await;
    let result = env
        .call("list_agents", json!({}), Some(env.parent.clone()))
        .await;
    assert!(!result.is_error());
    assert_eq!(
        result.value(),
        Some(&json!([{
            "name": "lead",
            "status": "inactive",
            "description": "root agent"
        }]))
    );
    assert_eq!(text(&result), "lead [inactive] — root agent");
    env.close().await;
}

#[tokio::test]
async fn projects_continuable_and_diagnostic_rows_but_omits_non_messageable_children() {
    let env = Env::new(json!({})).await;
    let worker = env
        .harness
        .create(SessionId::from("worker-session"))
        .await
        .unwrap();
    let worker_header: SessionHeader =
        serde_json::from_value((*worker.session().header()).clone()).unwrap();
    hoh_subagent::catalog::establish_catalog_child(
        env.parent.session(),
        &worker_header,
        "reviewer",
        "worker",
    )
    .unwrap();

    let one_shot = env
        .harness
        .create(SessionId::from("one-shot-session"))
        .await
        .unwrap();
    let one_shot_header: SessionHeader =
        serde_json::from_value((*one_shot.session().header()).clone()).unwrap();
    env.parent
        .session()
        .append(
            "subagent/catalog",
            json!({
                "version": 1,
                "childId": one_shot_header.id,
                "childCreatedAt": one_shot_header.created_at,
                "mode": "one-shot",
                "label": "finished once"
            }),
            None,
        )
        .unwrap();
    hoh_subagent::catalog::establish_external_catalog_child(
        env.parent.session(),
        &SessionId::from("external-session"),
        "external leaf",
        "external",
    )
    .unwrap();
    env.parent
        .session()
        .append(
            "subagent/catalog",
            json!({
                "version": 3,
                "childId": "missing-session",
                "childCreatedAt": 1,
                "mode": "continuable",
                "label": "unreadable branch",
                "name": "missing"
            }),
            None,
        )
        .unwrap();

    let result = env
        .call("list_agents", json!({}), Some(env.parent.clone()))
        .await;
    assert_eq!(
        result.value(),
        Some(&json!([
            { "name": "lead", "status": "inactive", "description": "root agent" },
            { "name": "worker", "status": "inactive", "description": "reviewer" },
            { "name": "missing", "status": "unavailable" }
        ]))
    );
    assert_eq!(
        text(&result),
        "lead [inactive] — root agent\nworker [inactive] — reviewer\nmissing [unavailable]"
    );
    env.close().await;
}

#[tokio::test]
async fn rejects_a_call_without_an_agent() {
    let env = Env::new(json!({})).await;
    let result = env.call("list_agents", json!({}), None).await;
    assert!(matches!(result, ToolExecutionResult::Failure(_)));
    assert_eq!(error_code(&result), Some("TOOL_CONTEXT_UNAVAILABLE"));
    assert!(text(&result).contains("requires a calling agent"));
    env.close().await;
}

#[tokio::test]
async fn unregisters_with_the_plugin_fiber() {
    let env = Env::new(json!({})).await;
    assert!(env.tools().get("list_agents", None, None).is_some());
    env.control.dispose().await;
    assert!(env.tools().get("list_agents", None, None).is_none());
    env.kernel.shutdown().await;
}

#[test]
fn exports_the_source_plugin_identity() {
    assert_eq!(ToolSubagentControl::NAME, "tool-subagent-control");
    assert_eq!(hoh_tool_subagent_control::NAME, "tool-subagent-control");
    assert_eq!(
        hoh_tool_subagent_control::INJECT,
        ["tools", "subagents", "agents", "systemPrompt"]
    );
}
