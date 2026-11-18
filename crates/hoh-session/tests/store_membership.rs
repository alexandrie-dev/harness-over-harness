mod common;
use common::*;
use hoh_session::*;
use serde_json::json;
use std::sync::{Arc, Mutex};
fn write(s: &Session, record: bool) -> Result<()> {
    if record {
        append_plugin_record(s, "plugin:test/store-membership", json!({"value":1}))?;
    } else {
        s.append("turn/start", json!({"turn":1}), None)?;
    }
    Ok(())
}
#[tokio::test(start_paused = true)]
async fn keeps_independent_events_and_plugin_records_local_and_adopts_history_without_replaying_notifications()
 {
    for record in [false, true] {
        let (k, ctx, store) = mounted().await;
        let s = session();
        let calls = Arc::new(Mutex::new(0));
        let out = calls.clone();
        ctx.on_contained::<SessionEventEmitted>(move |_| {
            *lock(&out) += 1;
            Ok(())
        })
        .unwrap();
        write(&s, record).unwrap();
        let history = s.snapshot_events(None, None);
        assert_eq!(*lock(&calls), 0);
        let d = store.enter(&ctx, s.clone()).unwrap();
        error(
            write(&s, record),
            "cannot append before session/created completes",
        );
        store.announce(&s).unwrap();
        assert_eq!(*lock(&calls), 0);
        write(&s, record).unwrap();
        assert_eq!(*lock(&calls), 1);
        assert_eq!(history.len(), 1);
        d.detach();
        error(write(&s, record), "cannot append outside a SessionStore");
        k.shutdown().await;
    }
}
#[tokio::test(start_paused = true)]
async fn allows_events_and_plugin_records_only_after_creation_announcement_and_before_removal() {
    for record in [false, true] {
        let (k, ctx, store) = mounted().await;
        let s = store
            .prepare(Some("membership".into()), Default::default())
            .unwrap();
        let before = s.snapshot_events(None, None);
        error(write(&s, record), "cannot append outside a SessionStore");
        let d = store.enter(&ctx, s.clone()).unwrap();
        error(
            write(&s, record),
            "cannot append before session/created completes",
        );
        assert!(Arc::ptr_eq(&before, &s.snapshot_events(None, None)));
        store.announce(&s).unwrap();
        write(&s, record).unwrap();
        let committed = s.snapshot_events(None, None);
        d.detach();
        let replacement = store
            .create(&ctx, Some(s.id().clone()), Default::default())
            .unwrap();
        assert_eq!(replacement.seq(), offset(0));
        error(write(&s, record), "cannot append outside a SessionStore");
        assert!(Arc::ptr_eq(&committed, &s.snapshot_events(None, None)));
        k.shutdown().await;
    }
}
#[tokio::test(start_paused = true)]
async fn keeps_writes_disabled_after_a_failed_announcement() {
    let (k, ctx, store) = mounted().await;
    let s = store.prepare(None, Default::default()).unwrap();
    let d = store.enter(&ctx, s.clone()).unwrap();
    SessionStore::on_created(&ctx, |_| Err("creation observer failed".into())).unwrap();
    error(store.announce(&s), "creation observer failed");
    error(write(&s, false), "before session/created completes");
    error(store.announce(&s), "already announced");
    assert_eq!(s.seq(), offset(0));
    d.detach();
    k.shutdown().await;
}
#[test]
fn appends_to_independent_constructor_restore_and_fork_history_without_publication() {
    let seed = vec![event("turn/start", 0, json!({"turn":1}))];
    let created = Session::create("history".into(), Some(&seed), None, None, None).unwrap();
    let restored = Session::from_restore(
        "history".into(),
        &seed,
        (*created.header()).clone(),
        offset(0),
        SessionSeedEventState::SharedFrozen,
        None,
    )
    .unwrap();
    let mut h = header("child");
    h["isSeeded"] = json!(true);
    let forked =
        Session::create("child".into(), Some(&seed), Some(h), Some(offset(1)), None).unwrap();
    for s in [created, restored, forked] {
        assert_eq!(
            s.snapshot_events(None, None)[1].event_type(),
            "session/end-seed"
        );
        let history = s.snapshot_events(None, None);
        s.append(
            "turn/end",
            json!({"turn":1,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(s.seq(), offset(3));
    }
}
#[tokio::test(start_paused = true)]
async fn keeps_prepared_restored_history_read_only() {
    let (k, _ctx, store) = mounted().await;
    let source = session();
    write(&source, false).unwrap();
    for state in [
        SessionSeedEventState::Detached,
        SessionSeedEventState::SharedFrozen,
    ] {
        let s = store
            .prepare(
                Some(source.id().clone()),
                PrepareSessionOptions::Restored(RestoredSessionOptions {
                    seed: source.snapshot_events(None, None).to_vec(),
                    meta: (*source.header()).clone(),
                    inherited_event_count: offset(0),
                    event_state: state,
                }),
            )
            .unwrap();
        let history = s.snapshot_events(None, None);
        error(write(&s, false), "outside a SessionStore");
        assert!(Arc::ptr_eq(&history, &s.snapshot_events(None, None)));
    }
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn revokes_writes_when_the_owning_fiber_releases_a_session() {
    let (k, ctx, store) = mounted().await;
    let owner = ctx.child("session-owner").unwrap();
    let s = store.create(&owner, None, Default::default()).unwrap();
    write(&s, false).unwrap();
    owner.fiber().dispose().await;
    assert!(store.get(s.id()).is_none());
    error(write(&s, false), "outside a SessionStore");
    assert_eq!(s.seq(), offset(1));
    k.shutdown().await;
}
