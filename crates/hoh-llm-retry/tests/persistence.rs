mod support;

use hoh_llm::LlmFailure;
use hoh_llm_retry::{LlmRetryEventData, RetryId};
use hoh_session::SessionHeader;
use hoh_session_persistence::{SessionAccess, SessionPersistence};
use hoh_session_persistence_jsonl::{Config, JsonlSessionPersistence};
use serde_json::json;
use support::Env;

#[tokio::test]
async fn round_trips_the_event_losslessly_without_adding_a_model_message() {
    let env = Env::new("retry-jsonl").await;
    env.agent
        .session()
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    env.agent
        .session()
        .append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    env.agent
        .session()
        .append(
            "request/header",
            json!({
                "header":{"config":{"provider":"mock","model":"mock"}},
                "reason":"initial"
            }),
            None,
        )
        .unwrap();
    let event = env
        .agent
        .session()
        .append(
            "llm/retry",
            serde_json::to_value(LlmRetryEventData::Always {
                retry_id: RetryId::from("retry-jsonl-chain"),
                turn: 1,
                step: 1,
                provider: "mock".into(),
                policy_key: r#"["always",500,10000,0.1]"#.into(),
                retry: 1,
                delay_ms: 750.0,
                failure: {
                    let mut failure = LlmFailure::new("provider busy", "RATE_LIMIT");
                    failure.status = Some(429);
                    failure
                },
            })
            .unwrap(),
            None,
        )
        .unwrap();
    env.agent
        .session()
        .append("step/end", json!({"turn":1,"step":1}), None)
        .unwrap();
    env.agent
        .session()
        .append(
            "turn/end",
            json!({
                "turn":1,
                "reason":{
                    "kind":"error",
                    "error":{"message":"provider busy","code":"RATE_LIMIT","status":429}
                }
            }),
            None,
        )
        .unwrap();
    assert_eq!(env.agent.session().derive_messages().unwrap(), []);
    let events = env.agent.session().snapshot_events(None, None).to_vec();

    let directory = tempfile::tempdir().unwrap();
    let backend =
        JsonlSessionPersistence::new(&env.ctx, Config::new(directory.path().to_string_lossy()))
            .await
            .unwrap();
    let header: SessionHeader =
        serde_json::from_value((*env.agent.session().header()).clone()).unwrap();
    let writer = backend.create(&header, None).await.unwrap();
    writer.append(&events, None).await.unwrap();
    writer.flush(None).await.unwrap();
    writer.close().await.unwrap();

    let reader = backend
        .open(env.agent.id(), SessionAccess::Read, None)
        .await
        .unwrap();
    let loaded = reader.read(None, None, None).await.unwrap();
    assert_eq!(loaded.events.len(), events.len());
    let loaded_retry = loaded
        .events
        .iter()
        .find(|item| item.event_type() == "llm/retry")
        .unwrap();
    assert_eq!(loaded_retry.seq(), event.seq());
    assert_eq!(
        hoh_util_values::js_json_stringify(loaded_retry.data()),
        hoh_util_values::js_json_stringify(event.data())
    );
    assert_eq!(
        loaded_retry.data(),
        &json!({
            "retryId":"retry-jsonl-chain",
            "turn":1,
            "step":1,
            "provider":"mock",
            "mode":"always",
            "policyKey":"[\"always\",500,10000,0.1]",
            "retry":1,
            "delayMs":750,
            "failure":{"message":"provider busy","code":"RATE_LIMIT","status":429}
        })
    );
    reader.close().await.unwrap();
    backend.close().await.unwrap();
    env.close().await;
}
