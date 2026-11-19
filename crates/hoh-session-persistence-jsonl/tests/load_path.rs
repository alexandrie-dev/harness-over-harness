use hoh_kernel::{Kernel, Plugin};
use hoh_session::{CreateSessionOptions, SessionStore};
use hoh_session_persistence::SessionPersistenceService;
use hoh_session_persistence_jsonl::{Config, JsonlCompression, JsonlSessionPersistencePlugin};
use serde_json::json;
#[tokio::test]
async fn retains_the_jsonl_namespace_and_configuration_through_the_kernel() {
    assert_eq!(
        JsonlSessionPersistencePlugin::NAME,
        "session-persistence-jsonl"
    );
    let c: Config = serde_json::from_value(json!({"root":"logs"})).unwrap();
    assert_eq!(c.compression, JsonlCompression::Zstd);
    assert_eq!(c.list_concurrency, 3);
    let root = tempfile::tempdir().unwrap();
    let k = Kernel::start();
    let ctx = k.root();
    let p = ctx
        .plugin::<JsonlSessionPersistencePlugin>(
            json!({"root":root.path().to_string_lossy(),"compression":"none"}),
        )
        .unwrap();
    p.settled().await.unwrap();
    assert!(ctx.get::<SessionPersistenceService>().is_some());
    p.dispose().await;
    assert!(ctx.get::<SessionPersistenceService>().is_none());
    k.shutdown().await;
}
#[tokio::test]
async fn routes_live_events_and_joins_the_session_flush_barrier_before_teardown() {
    let root = tempfile::tempdir().unwrap();
    let k = Kernel::start();
    let ctx = k.root();
    let plugin = ctx
        .plugin::<JsonlSessionPersistencePlugin>(
            json!({"root":root.path().to_string_lossy(),"compression":"none"}),
        )
        .unwrap();
    plugin.settled().await.unwrap();
    let backend = ctx.get::<SessionPersistenceService>().unwrap();
    let store = SessionStore::new();
    let session = store
        .create(&ctx, Some("live".into()), CreateSessionOptions::default())
        .unwrap();
    let header = serde_json::from_value((*session.header()).clone()).unwrap();
    let writer = backend.create(&header, None).await.unwrap();
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
    store.flush(&session).await.unwrap();
    assert_eq!(
        writer.read(None, None, None).await.unwrap().events,
        session.snapshot_events(None, None).to_vec()
    );
    plugin.dispose().await;
    assert!(writer.read(None, None, None).await.is_err());
    k.shutdown().await;
}
