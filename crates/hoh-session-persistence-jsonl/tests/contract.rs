mod common;
use common::*;
use hoh_session_persistence::*;
use hoh_session_persistence_jsonl::{Config, JsonlCompression, JsonlSessionPersistence};
use serde_json::json;
async fn remount(f: &Fixture) -> JsonlSessionPersistence {
    let mut c = Config::new(f.root.path().to_string_lossy());
    c.compression = f.backend.compression();
    JsonlSessionPersistence::new(&f.kernel.root(), c)
        .await
        .unwrap()
}
fn turn2() -> Vec<serde_json::Value> {
    vec![
        event("turn/start", 6, json!({"turn":2})),
        event(
            "turn/end",
            7,
            json!({"turn":2,"reason":{"kind":"completed"}}),
        ),
    ]
}
async fn run(case: &str) {
    for mode in [JsonlCompression::None, JsonlCompression::Zstd] {
        let f = Fixture::new(mode).await;
        let m = meta("contract");
        match case {
            "round_trip" => {
                let h = f.backend.create(&m, None).await.unwrap();
                assert_eq!(h.access(), SessionAccess::Write);
                assert_eq!(h.id(), &m.id);
                h.append(&arcs(&log()), None).await.unwrap();
                h.append(&[], None).await.unwrap();
                assert_eq!(read(h.as_ref()).await, log());
                for (o, l, expected) in [
                    (Some(3), None, log()[3..].to_vec()),
                    (Some(0), Some(2), log()[..2].to_vec()),
                    (Some(1), Some(3), log()[1..4].to_vec()),
                    (Some(6), None, vec![]),
                    (Some(106), None, vec![]),
                ] {
                    assert_eq!(values(&h.read(o, l, None).await.unwrap().events), expected);
                }
                h.flush(None).await.unwrap();
                h.close().await.unwrap();
            }
            "duplicate_pending" => {
                let a = f.backend.create(&m, None).await.unwrap();
                assert!(has_class::<SessionAlreadyExistsError>(
                    fail(f.backend.create(&m, None).await, "already exists").as_ref()
                ));
                a.close().await.unwrap();
                f.backend
                    .create(&m, None)
                    .await
                    .unwrap()
                    .close()
                    .await
                    .unwrap();
            }
            "concurrent_create" => {
                let (a, b) = tokio::join!(f.backend.create(&m, None), f.backend.create(&m, None));
                match (a, b) {
                    (Ok(a), Err(e)) | (Err(e), Ok(a)) => {
                        assert!(has_class::<SessionAlreadyExistsError>(e.as_ref()));
                        a.close().await.unwrap();
                    }
                    _ => panic!("exactly one create must win"),
                }
            }
            "duplicate_stored" => {
                let h = f.backend.create(&m, None).await.unwrap();
                h.append(&arcs(&log()), None).await.unwrap();
                h.close().await.unwrap();
                let other = remount(&f).await;
                assert!(has_class::<SessionAlreadyExistsError>(
                    fail(other.create(&m, None).await, "already exists").as_ref()
                ));
                other.close().await.unwrap();
            }
            "absent" => {
                for a in [SessionAccess::Read, SessionAccess::Write] {
                    assert!(has_class::<SessionPersistenceNotFoundError>(
                        fail(f.backend.open(&m.id, a, None).await, "not found").as_ref()
                    ));
                }
            }
            "single_writer" => {
                let h = f.backend.create(&m, None).await.unwrap();
                assert!(has_class::<SessionAlreadyOwnedError>(
                    fail(
                        f.backend.open(&m.id, SessionAccess::Write, None).await,
                        "already owned"
                    )
                    .as_ref()
                ));
                h.append(&arcs(&log()), None).await.unwrap();
                assert!(has_class::<SessionAlreadyOwnedError>(
                    fail(
                        f.backend.open(&m.id, SessionAccess::Write, None).await,
                        "already owned"
                    )
                    .as_ref()
                ));
                h.close().await.unwrap();
                let h = f
                    .backend
                    .open(&m.id, SessionAccess::Write, None)
                    .await
                    .unwrap();
                h.append(&arcs(&turn2()), None).await.unwrap();
                assert_eq!(read(h.as_ref()).await.len(), 8);
                h.close().await.unwrap();
            }
            "read_only" => {
                f.store(&m, &log()).await;
                let h = f
                    .backend
                    .open(&m.id, SessionAccess::Read, None)
                    .await
                    .unwrap();
                for e in [h.append(&arcs(&turn2()), None).await, h.flush(None).await] {
                    assert!(has_class::<SessionReadOnlyError>(
                        fail(e, "read handle").as_ref()
                    ));
                }
                assert_eq!(read(h.as_ref()).await, log());
                h.close().await.unwrap();
            }
            "closed" => {
                let h = f.backend.create(&m, None).await.unwrap();
                h.append(&arcs(&log()), None).await.unwrap();
                h.close().await.unwrap();
                h.close().await.unwrap();
                assert!(has_class::<SessionHandleClosedError>(
                    fail(h.read(None, None, None).await, "closed handle").as_ref()
                ));
                for e in [h.append(&arcs(&turn2()), None).await, h.flush(None).await] {
                    assert!(has_class::<SessionHandleClosedError>(
                        fail(e, "closed handle").as_ref()
                    ));
                }
                let w = f
                    .backend
                    .open(&m.id, SessionAccess::Write, None)
                    .await
                    .unwrap();
                w.append(&arcs(&turn2()), None).await.unwrap();
                w.close().await.unwrap();
                f.backend
                    .open(&m.id, SessionAccess::Write, None)
                    .await
                    .unwrap()
                    .close()
                    .await
                    .unwrap();
            }
            "flush_all" => {
                let a = f.backend.create(&m, None).await.unwrap();
                let b = f.backend.create(&meta("closing"), None).await.unwrap();
                let closing = b.close();
                f.backend.flush().await.unwrap();
                closing.await.unwrap();
                let other = remount(&f).await;
                assert!(other.stat(&m.id, None).await.unwrap().is_some());
                assert!(other.stat(&"closing".into(), None).await.unwrap().is_none());
                other.close().await.unwrap();
                a.close().await.unwrap();
            }
            "pending_visibility" => {
                let h = f.backend.create(&m, None).await.unwrap();
                assert!(read(h.as_ref()).await.is_empty());
                assert!(f.backend.stat(&m.id, None).await.unwrap().is_some());
                assert_eq!(f.backend.list(None).await.unwrap().len(), 1);
                let r = f
                    .backend
                    .open(&m.id, SessionAccess::Read, None)
                    .await
                    .unwrap();
                assert!(read(r.as_ref()).await.is_empty());
                r.close().await.unwrap();
                let other = remount(&f).await;
                assert!(other.stat(&m.id, None).await.unwrap().is_none());
                assert!(other.list(None).await.unwrap().is_empty());
                assert!(has_class::<SessionPersistenceNotFoundError>(
                    fail(
                        other.open(&m.id, SessionAccess::Read, None).await,
                        "not found"
                    )
                    .as_ref()
                ));
                other.close().await.unwrap();
                h.close().await.unwrap();
            }
            "erasing_close" => {
                let h = f.backend.create(&m, None).await.unwrap();
                h.close().await.unwrap();
                assert!(f.backend.stat(&m.id, None).await.unwrap().is_none());
                assert!(f.backend.list(None).await.unwrap().is_empty());
                assert!(has_class::<SessionPersistenceNotFoundError>(
                    fail(
                        f.backend.open(&m.id, SessionAccess::Read, None).await,
                        "not found"
                    )
                    .as_ref()
                ));
            }
            "durable_empty" => {
                let h = f.backend.create(&m, None).await.unwrap();
                h.flush(None).await.unwrap();
                h.close().await.unwrap();
                let other = remount(&f).await;
                assert_eq!(other.list(None).await.unwrap().len(), 1);
                let r = other.open(&m.id, SessionAccess::Read, None).await.unwrap();
                assert!(read(r.as_ref()).await.is_empty());
                r.close().await.unwrap();
                other.close().await.unwrap();
            }
            "freshness" => {
                let w = f.backend.create(&m, None).await.unwrap();
                w.append(&arcs(&log()), None).await.unwrap();
                let before = f
                    .backend
                    .open(&m.id, SessionAccess::Read, None)
                    .await
                    .unwrap();
                assert_eq!(read(before.as_ref()).await, log());
                w.append(&arcs(&turn2()), None).await.unwrap();
                assert_eq!(read(before.as_ref()).await.len(), 8);
                let after = f
                    .backend
                    .open(&m.id, SessionAccess::Read, None)
                    .await
                    .unwrap();
                assert_eq!(read(after.as_ref()).await.len(), 8);
                before.close().await.unwrap();
                after.close().await.unwrap();
                w.close().await.unwrap();
            }
            "fresh_instance" => {
                f.store(&m, &log()).await;
                let other = remount(&f).await;
                let w = other.open(&m.id, SessionAccess::Write, None).await.unwrap();
                w.append(&arcs(&turn2()), None).await.unwrap();
                assert_eq!(read(w.as_ref()).await.len(), 8);
                w.close().await.unwrap();
                other.close().await.unwrap();
            }
            "contiguity" => {
                let h = f.backend.create(&m, None).await.unwrap();
                h.append(&arcs(&log()), None).await.unwrap();
                fail(h.append(&arcs(&log()), None).await, "expected 6");
                let mut gap = turn2();
                gap[1]["seq"] = 8.into();
                fail(h.append(&arcs(&gap), None).await, "expected 7");
                assert_eq!(read(h.as_ref()).await, log());
                h.close().await.unwrap();
            }
            "vocabulary" => {
                let h = f.backend.create(&m, None).await.unwrap();
                h.append(&arcs(&log()), None).await.unwrap();
                h.append(
                    &arcs(&[event("mystery/event", 6, json!({"payload":true}))]),
                    None,
                )
                .await
                .unwrap();
                h.close().await.unwrap();
                for a in [
                    SessionAccess::Write,
                    SessionAccess::Write,
                    SessionAccess::Read,
                ] {
                    assert!(has_class::<SessionFormatUnsupportedError>(
                        fail(f.backend.open(&m.id, a, None).await, "mystery/event").as_ref()
                    ));
                }
            }
            "revisions" => {
                let h = f.backend.create(&m, None).await.unwrap();
                h.append(&arcs(&log()), None).await.unwrap();
                let a = f.backend.stat(&m.id, None).await.unwrap().unwrap();
                assert_eq!(
                    a.revision,
                    f.backend.stat(&m.id, None).await.unwrap().unwrap().revision
                );
                assert_eq!(a.revision, f.backend.list(None).await.unwrap()[0].revision);
                h.append(&arcs(&turn2()), None).await.unwrap();
                let b = f.backend.stat(&m.id, None).await.unwrap().unwrap();
                assert_ne!(a.revision, b.revision);
                assert_eq!(b.revision, f.backend.list(None).await.unwrap()[0].revision);
                let r = f
                    .backend
                    .open(&m.id, SessionAccess::Read, None)
                    .await
                    .unwrap();
                assert_eq!(&b.header, r.header());
                r.close().await.unwrap();
                h.close().await.unwrap();
                assert!(
                    f.backend
                        .stat(&"absent-stat".into(), None)
                        .await
                        .unwrap()
                        .is_none()
                );
            }
            _ => panic!("unknown contract case"),
        }
        f.finish().await;
    }
}

#[tokio::test]
async fn round_trips_through_one_write_handle_append_self_read_offset_and_length_defaults() {
    run("round_trip").await;
}

#[tokio::test]
async fn duplicate_create_rejects_a_live_pending_session_and_allows_the_id_after_erasing_close() {
    run("duplicate_pending").await;
}

#[tokio::test]
async fn concurrent_duplicate_creates_have_one_winner_and_one_already_exists() {
    run("concurrent_create").await;
}

#[tokio::test]
async fn duplicate_create_rejects_a_materialized_artifact_seen_by_a_fresh_instance() {
    run("duplicate_stored").await;
}

#[tokio::test]
async fn open_of_an_absent_session_rejects_not_found_for_both_accesses() {
    run("absent").await;
}

#[tokio::test]
async fn write_ownership_is_single_holder_per_instance_and_released_by_close() {
    run("single_writer").await;
}

#[tokio::test]
async fn a_read_handle_refuses_append_and_flush_with_read_only_error() {
    run("read_only").await;
}

#[tokio::test]
async fn closed_handle_operations_reject_and_explicit_close_releases_ownership() {
    run("closed").await;
}

#[tokio::test]
async fn service_flush_materializes_every_active_writer_and_counts_a_closing_one_as_flushed() {
    run("flush_all").await;
}

#[tokio::test]
async fn a_created_but_unappended_session_is_visible_only_to_its_own_instance() {
    run("pending_visibility").await;
}

#[tokio::test]
async fn close_without_an_append_erases_the_created_session() {
    run("erasing_close").await;
}

#[tokio::test]
async fn flush_materializes_an_empty_session_durably_for_a_fresh_instance() {
    run("durable_empty").await;
}

#[tokio::test]
async fn reads_started_after_an_append_resolves_observe_that_prefix_on_any_handle() {
    run("freshness").await;
}

#[tokio::test]
async fn a_fresh_instance_continues_at_the_committed_next_sequence() {
    run("fresh_instance").await;
}

#[tokio::test]
async fn append_rejects_a_noncontiguous_batch_naming_the_expected_sequence() {
    run("contiguity").await;
}

#[tokio::test]
async fn an_unknown_stored_type_refuses_read_and_write_opens() {
    run("vocabulary").await;
}

#[tokio::test]
async fn stat_and_list_agree_on_stable_revisions_that_change_after_append() {
    run("revisions").await;
}
