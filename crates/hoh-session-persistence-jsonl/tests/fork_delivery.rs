mod common;
use common::*;
use hoh_session::{CreateSessionOptions, Session, SessionForkSource, SessionHeader, SessionStore};
use hoh_session_persistence::*;
use hoh_session_persistence_jsonl::JsonlCompression;
use serde_json::json;
fn delivered(s: &Session) {
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    s.append(
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
        None,
    )
    .unwrap();
    s.append(
        "session-log-deepseek/delivery-accepted",
        json!({"sessionId":s.id(),"sessionFormatVersion":4,"throughSeq":0}),
        None,
    )
    .unwrap();
}
async fn persist(f: &Fixture, s: &Session) -> (String, Vec<u8>) {
    let h: SessionHeader = serde_json::from_value((*s.header()).clone()).unwrap();
    let w = f
        .backend
        .create(
            &h,
            Some(&SessionPersistenceCreateOptions {
                inherited_event_count: Some(s.inherited_event_count()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    w.append(&s.snapshot_events(None, None), None)
        .await
        .unwrap();
    w.close().await.unwrap();
    let path = hoh_session_persistence_jsonl::format::log_path(
        &f.root.path().to_string_lossy(),
        h.cwd.as_deref(),
        s.id(),
        f.backend.compression(),
    )
    .unwrap();
    let bytes = tokio::fs::read(&path).await.unwrap();
    (path, bytes)
}
#[tokio::test]
async fn restores_ordinary_nested_subagent_and_legacy_fork_history_through_read_and_write_access() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        for kind in ["ordinary", "nested", "subagent", "legacy"] {
            let f = Fixture::new(compression).await;
            let ctx = f.kernel.root();
            let store = SessionStore::new();
            let source = store
                .create(&ctx, Some("source".into()), CreateSessionOptions::default())
                .unwrap();
            delivered(&source);
            let ordinary = store
                .fork(
                    &ctx,
                    SessionForkSource::Session(source.clone()),
                    None,
                    Some("ordinary".into()),
                )
                .unwrap();
            let child = match kind {
                "ordinary" => ordinary,
                "nested" => store
                    .fork(
                        &ctx,
                        SessionForkSource::Session(ordinary),
                        None,
                        Some("nested".into()),
                    )
                    .unwrap(),
                _ => {
                    let mut metadata = json!({"parentSession":source.id(),"isSeeded":true});
                    if kind == "subagent" {
                        metadata["origin"] = "subagent".into();
                    }
                    store
                        .create(
                            &ctx,
                            Some(kind.into()),
                            CreateSessionOptions {
                                seed: Some(source.snapshot_events(None, None).to_vec()),
                                inherited_event_count: Some(source.seq()),
                                meta: Some(metadata),
                            },
                        )
                        .unwrap()
                }
            };
            let (path, bytes) = persist(&f, &child).await;
            for access in [SessionAccess::Read, SessionAccess::Write] {
                let h = f.backend.open(child.id(), access, None).await.unwrap();
                let read = h.read(None, None, None).await.unwrap();
                let restored = Session::from_restore(
                    child.id().clone(),
                    &read.events,
                    serde_json::to_value(h.header()).unwrap(),
                    h.inherited_event_count(),
                    read.event_state,
                    None,
                )
                .unwrap();
                assert_eq!(
                    restored.snapshot_events(None, None),
                    child.snapshot_events(None, None)
                );
                assert_eq!(
                    restored.inherited_event_count(),
                    child.inherited_event_count()
                );
                assert_eq!(
                    h.header().parent_session.as_ref(),
                    if matches!(kind, "subagent" | "legacy") {
                        Some(source.id())
                    } else {
                        None
                    }
                );
                h.close().await.unwrap();
                assert_eq!(tokio::fs::read(&path).await.unwrap(), bytes);
            }
            f.finish().await;
        }
    }
}
#[tokio::test]
async fn refuses_foreign_own_invalid_through_and_inherited_through_delivery_without_modifying_the_file()
 {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        for damage in ["foreign-own", "invalid-through", "inherited-through"] {
            let f = Fixture::new(compression).await;
            let ctx = f.kernel.root();
            let store = SessionStore::new();
            let source = store
                .create(&ctx, Some("source".into()), CreateSessionOptions::default())
                .unwrap();
            delivered(&source);
            let target = if damage == "inherited-through" {
                source.clone()
            } else {
                store
                    .fork(
                        &ctx,
                        SessionForkSource::Session(source.clone()),
                        None,
                        Some("child".into()),
                    )
                    .unwrap()
            };
            target.append("session-log-deepseek/delivery-accepted",json!({"sessionId":if damage=="foreign-own" {source.id()}else{target.id()},"sessionFormatVersion":4,"throughSeq":if damage=="foreign-own" {0}else{target.seq().get()}}),None).unwrap();
            let child = if damage == "inherited-through" {
                store
                    .fork(&ctx, SessionForkSource::Session(source), None, None)
                    .unwrap()
            } else {
                target
            };
            let (path, bytes) = persist(&f, &child).await;
            for access in [SessionAccess::Read, SessionAccess::Write] {
                fail(
                    f.backend.open(child.id(), access, None).await,
                    if damage == "foreign-own" {
                        "wrong Session"
                    } else {
                        "throughSeq must precede"
                    },
                );
                assert_eq!(tokio::fs::read(&path).await.unwrap(), bytes);
            }
            f.finish().await;
        }
    }
}
