mod support;

use hoh_agent::AgentRegistryPlugin;
use hoh_kernel::Kernel;
use hoh_session::{CreateSessionOptions, SessionHeader, SessionPlugin, Sessions};
use hoh_session_persistence::{SessionAccess, SessionPersistenceService};
use hoh_session_persistence_jsonl::JsonlSessionPersistencePlugin;
use hoh_session_projection::SessionProjectionRegistry;
use hoh_session_title::{SessionTitle, SessionTitlePlugin, fold_session_title};
use serde_json::json;

use support::{append_human, config};

#[tokio::test]
async fn round_trips_through_a_remounted_jsonl_backend() {
    let root = tempfile::tempdir().unwrap();
    let backend_config = json!({
        "root": root.path().to_string_lossy(),
        "compression": "none",
    });

    let writer = Kernel::start();
    let writer_ctx = writer.root();
    writer_ctx
        .plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    writer_ctx
        .plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    writer_ctx
        .plugin::<AgentRegistryPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    writer_ctx
        .plugin::<JsonlSessionPersistencePlugin>(backend_config.clone())
        .unwrap()
        .settled()
        .await
        .unwrap();
    writer_ctx
        .plugin::<SessionTitlePlugin>(config())
        .unwrap()
        .settled()
        .await
        .unwrap();

    let sessions = writer_ctx.get::<Sessions>().unwrap();
    let titles = writer_ctx.get::<SessionTitle>().unwrap();
    let persistence = writer_ctx.get::<SessionPersistenceService>().unwrap();
    let session = sessions
        .create(
            &writer_ctx,
            Some("title-jsonl".into()),
            CreateSessionOptions::default(),
        )
        .unwrap();
    let header: SessionHeader = serde_json::from_value((*session.header()).clone()).unwrap();
    let handle = persistence.create(&header, None).await.unwrap();
    session
        .append("turn/start", json!({"turn": 1}), None)
        .unwrap();
    let source = append_human(&session, "Persist this session title");
    session
        .append(
            "turn/end",
            json!({"turn":1,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
    let accepted = titles.refresh(&session, None).await.unwrap().unwrap();
    assert_eq!(accepted.message_seqs, vec![source]);
    sessions.flush(&session).await.unwrap();
    handle.close().await.unwrap();
    writer.shutdown().await;

    let reader = Kernel::start();
    let reader_ctx = reader.root();
    reader_ctx
        .plugin::<JsonlSessionPersistencePlugin>(backend_config)
        .unwrap()
        .settled()
        .await
        .unwrap();
    let persistence = reader_ctx.get::<SessionPersistenceService>().unwrap();
    let handle = persistence
        .open(&"title-jsonl".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    let events = handle.read(None, None, None).await.unwrap().events;
    let folded = fold_session_title(&events).unwrap();
    assert_eq!(folded.title, "Persist this session title");
    assert_eq!(folded.message_seqs, vec![source]);
    assert_eq!(u64::from(folded.event_seq), 3);
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type())
            .collect::<Vec<_>>(),
        ["turn/start", "user/message", "turn/end", "session/title"],
    );
    handle.close().await.unwrap();
    reader.shutdown().await;
}
