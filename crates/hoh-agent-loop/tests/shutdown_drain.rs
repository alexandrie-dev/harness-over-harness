//! Port of shutdown-drain.spec.ts: root-fiber shutdown drains buffered session events durably, in both
//! mount orders of the JSONL backend and the loop.
mod support_s17;

use hoh_agent::{Agents, CreateAgentOptions};
use hoh_kernel::Kernel;
use hoh_session::SessionId;
use hoh_session_persistence::{SessionAccess, SessionPersistenceService};
use hoh_session_persistence_jsonl::JsonlSessionPersistencePlugin;
use serde_json::json;
use support_s17::*;

async fn persists_buffered_turn_events_without_an_explicit_flush_before_dispose(before_loop: bool) {
    let root = tempfile::tempdir().expect("temp dir");
    let stack = stack(
        MockAdapter::chunks(vec![text_response("done")]),
        StackOptions {
            persistence: Some(Persistence {
                config: json!({ "root": root.path().to_str().expect("utf-8 root") }),
                before_loop,
            }),
            ..StackOptions::default()
        },
    )
    .await;
    let session_id = SessionId::from("shutdown-drain");
    let mut options = CreateAgentOptions::new(session_id.clone());
    options.agent_options = Some(mock_agent_options());
    let agents = stack.ctx.get::<Agents>().expect("agents");
    let handle = agents
        .create(&stack.ctx, options)
        .await
        .expect("agent creates");
    send_and_wait(&handle.agent, "q").await;
    // No explicit flush and no agent dispose: root teardown must drain.
    stack.close().await;

    let verify = Kernel::start();
    let ctx = verify.root();
    let fiber = ctx
        .plugin::<JsonlSessionPersistencePlugin>(
            json!({ "root": root.path().to_str().expect("utf-8 root") }),
        )
        .expect("plugin");
    fiber.settled().await.expect("persistence mounts");
    let persistence = ctx.get::<SessionPersistenceService>().expect("persistence");
    let reader = persistence
        .open(&session_id, SessionAccess::Read, None)
        .await
        .expect("open");
    let events = reader.read(None, None, None).await.expect("read").events;
    reader.close().await.expect("close");
    let last = events.last().expect("events");
    assert_eq!(last.event_type(), "turn/end");
    assert_match_object(last.data(), &json!({ "reason": { "kind": "completed" } }));
    verify.shutdown().await;
}

#[tokio::test]
async fn root_shutdown_drain_backend_first_persists_buffered_turn_events_without_an_explicit_flush_before_dispose()
 {
    persists_buffered_turn_events_without_an_explicit_flush_before_dispose(true).await;
}

#[tokio::test]
async fn root_shutdown_drain_loop_first_persists_buffered_turn_events_without_an_explicit_flush_before_dispose()
 {
    persists_buffered_turn_events_without_an_explicit_flush_before_dispose(false).await;
}
