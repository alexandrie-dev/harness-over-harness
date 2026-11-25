use hoh_kernel::Kernel;
use hoh_plugin_turn_note::{AuthorPlugin, last_turn};
use hoh_session::{SessionStore, append_plugin_record};
use hoh_system_prompt::SystemPrompt;
use hoh_tools::ToolRuntime;
use serde_json::json;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mounts_and_records_finished_turn_without_reentry() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    for fiber in [ctx.plugin::<SystemPrompt>(json!({})).unwrap()] {
        fiber.settled().await.unwrap();
    }
    ctx.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<AuthorPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let store = SessionStore::new();
    let session = store
        .create(&ctx, Some("fixture".into()), Default::default())
        .unwrap();
    append_plugin_record(
        &session,
        "plugin:external-harness/session",
        json!({"harness":"claude", "nativeSessionId":null}),
    )
    .unwrap();
    session
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    session
        .append(
            "turn/end",
            json!({"turn":1,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while last_turn(&session).is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        last_turn(&session).unwrap(),
        json!({"turn":1,"engine":"claude","outcome":"completed"})
    );
    let events = session.snapshot_events(None, None);
    let record = events
        .iter()
        .find(|event| event.event_type() == "plugin:turn-note/turn-ended")
        .unwrap();
    assert!(record.ignorable());
    kernel.shutdown().await;
}
