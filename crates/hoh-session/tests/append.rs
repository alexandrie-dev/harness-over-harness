mod common;
use common::*;
use hoh_session::*;
use serde_json::json;
use std::sync::{Arc, Mutex};
#[test]
fn reads_only_surface_metadata_from_event_options() {
    let s = session();
    let old = user(&s, "old");
    let next = data("user", "new");
    let events = s
        .append_batch(&[SessionEventInput::new("user/message", next.clone())
            .with_options(old.as_value().clone())])
        .unwrap();
    assert_eq!(events[0].data(), &next);
    assert_eq!(texts(&s), vec!["old", "new"]);
}
#[test]
fn leaves_the_log_unchanged_for_empty_input_and_accepts_a_single_array_member() {
    let s = session();
    assert!(s.append_batch(&[]).unwrap().is_empty());
    assert_eq!(s.seq(), offset(0));
    let events = s
        .append_batch(&[SessionEventInput::new("turn/start", json!({"turn":1}))])
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq(), seq(0));
}
#[tokio::test(start_paused = true)]
async fn publishes_each_member_after_the_complete_log_and_surface_are_committed() {
    let (k, ctx, store) = mounted().await;
    let s = store.create(&ctx, None, Default::default()).unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let out = seen.clone();
    ctx.on_contained::<SessionEventEmitted>(move |e| {
        assert_eq!(e.session.seq(), offset(2));
        assert_eq!(nodes(&e.session), vec![1]);
        assert_eq!(texts(&e.session), vec!["visible"]);
        assert!(Arc::ptr_eq(
            &e.event,
            &e.session.current_batch_publication().unwrap()
        ));
        lock(&out).push(e.event.seq());
        Ok(())
    })
    .unwrap();
    s.append_batch(&[
        SessionEventInput::new("turn/start", json!({"turn":1})),
        SessionEventInput::new("user/message", data("user", "visible"))
            .with_surface(SurfaceIntent::append()),
    ])
    .unwrap();
    assert_eq!(*lock(&seen), vec![seq(0), seq(1)]);
    assert!(s.current_batch_publication().is_none());
    k.shutdown().await;
}
#[test]
fn replaces_an_earlier_member_in_the_same_append() {
    let s = session();
    s.append_batch(&[
        SessionEventInput::new("user/message", data("user", "original"))
            .with_surface(SurfaceIntent::append()),
        SessionEventInput::new("user/message", data("user", "replacement"))
            .with_surface(SurfaceIntent::replace(seq(0), seq(0), vec![seq(0)])),
    ])
    .unwrap();
    assert_eq!(nodes(&s), vec![1]);
    assert_eq!(texts(&s), vec!["replacement"]);
}
#[test]
fn rejects_non_json_data_before_publishing_any_member() {
    let s = session();
    error(
        s.append_batch(&[
            SessionEventInput::new("turn/start", json!({"turn":1})),
            SessionEventInput::new("turn/start", json!({"turn":-0.0})),
        ]),
        "session batch member 1 (\"turn/start\") carries non-JSON-serializable data",
    );
    assert_eq!(s.seq(), offset(0));
}
#[tokio::test(start_paused = true)]
async fn contains_a_reentrant_append_while_publishing_the_remaining_members() {
    let (k, ctx, store) = mounted().await;
    let s = store.create(&ctx, None, Default::default()).unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    ctx.on_contained::<SessionEventEmitted>(|e| {
        if e.event.seq() == seq(0) {
            e.session.append(
                "request/context",
                json!({"provider":"mock","model":"mock"}),
                None,
            )?;
        }
        Ok(())
    })
    .unwrap();
    let out = seen.clone();
    ctx.on_contained::<SessionEventEmitted>(move |e| {
        lock(&out).push(e.event.seq());
        Ok(())
    })
    .unwrap();
    s.append_batch(&[
        SessionEventInput::new("turn/start", json!({"turn":1})),
        SessionEventInput::new("step/start", json!({"turn":1,"step":1})),
    ])
    .unwrap();
    assert_eq!(s.seq(), offset(2));
    assert_eq!(*lock(&seen), vec![seq(0), seq(1)]);
    let warnings = ctx
        .logger()
        .buffer()
        .into_iter()
        .filter(|record| record.message.contains("session append cannot reenter"))
        .collect::<Vec<_>>();
    assert_eq!(warnings.len(), 1);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn detaches_only_after_every_member_has_been_observed() {
    let (k, ctx, store) = mounted().await;
    let s = store.prepare(None, Default::default()).unwrap();
    let detach = store.enter(&ctx, s.clone()).unwrap();
    store.announce(&s).unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let out = seen.clone();
    let st = store.clone();
    ctx.on_contained::<SessionEventEmitted>(move |e| {
        assert_eq!(st.get(e.session.id()), Some(e.session.clone()));
        detach.detach();
        lock(&out).push(e.event.seq());
        Ok(())
    })
    .unwrap();
    s.append_batch(&[
        SessionEventInput::new("turn/start", json!({"turn":1})),
        SessionEventInput::new("step/start", json!({"turn":1,"step":1})),
    ])
    .unwrap();
    assert_eq!(*lock(&seen), vec![seq(0), seq(1)]);
    assert!(store.get(s.id()).is_none());
    k.shutdown().await;
}
#[test]
fn folds_preceding_single_appends_before_admitting_an_array() {
    let s = session();
    user(&s, "before");
    s.append_batch(&[
        SessionEventInput::new("user/message", data("user", "after"))
            .with_surface(SurfaceIntent::append()),
    ])
    .unwrap();
    assert_eq!(nodes(&s), vec![0, 1]);
    assert_eq!(s.surface().unwrap().replace_generation().unwrap(), 0);
}
#[test]
fn publishes_a_complete_group_with_its_final_surface() {
    let s = session();
    let e = s
        .append_batch(&[
            SessionEventInput::new("turn/start", json!({"turn":1})),
            SessionEventInput::new("user/message", data("user", "hello"))
                .with_surface(SurfaceIntent::append()),
        ])
        .unwrap();
    assert_eq!(
        e.iter().map(|e| e.seq()).collect::<Vec<_>>(),
        vec![seq(0), seq(1)]
    );
    assert_eq!(nodes(&s), vec![1]);
    assert_eq!(*s.derive_messages().unwrap()[0], *e[1].data());
}
#[test]
fn retains_the_committed_log_and_surface_when_a_later_candidate_is_invalid() {
    let s = session();
    user(&s, "before");
    let before = s.snapshot_events(None, None);
    let derived = s.derive_messages().unwrap();
    assert!(
        s.append_batch(&[
            SessionEventInput::new("user/message", data("user", "candidate"))
                .with_surface(SurfaceIntent::append()),
            SessionEventInput::new("user/message", data("user", "invalid"))
                .with_surface(SurfaceIntent::replace(seq(40), seq(41), vec![seq(0)]))
        ])
        .is_err()
    );
    assert!(Arc::ptr_eq(&before, &s.snapshot_events(None, None)));
    assert_eq!(s.derive_messages().unwrap(), derived);
    assert_eq!(nodes(&s), vec![0]);
}
#[tokio::test(start_paused = true)]
async fn rejects_the_entire_group_when_dispatch_validation_vetoes_its_last_event() {
    let (k, ctx, store) = mounted().await;
    let s = store.create(&ctx, None, Default::default()).unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let out = seen.clone();
    ctx.on_contained::<SessionEventEmitted>(move |e| {
        lock(&out).push(e.event.seq());
        Ok(())
    })
    .unwrap();
    let stop = ctx
        .on_dispatch(|_, name, event| {
            if name == "session/event" {
                let e = event.downcast_ref::<SessionEventEmitted>().unwrap();
                if e.event.seq() == seq(1) {
                    e.validation.reject("veto final event".into());
                }
            }
        })
        .unwrap();
    error(
        s.append_batch(&[
            SessionEventInput::new("turn/start", json!({"turn":1})),
            SessionEventInput::new("user/message", data("user", "candidate"))
                .with_surface(SurfaceIntent::append()),
        ]),
        "veto final event",
    );
    assert_eq!(s.seq(), offset(0));
    assert!(nodes(&s).is_empty());
    assert!(lock(&seen).is_empty());
    stop.dispose().await;
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    assert_eq!(*lock(&seen), vec![seq(0)]);
    k.shutdown().await;
}
#[test]
fn an_atomic_run_of_appends_is_not_interleaved_with_another_threads_append() {
    let s = session();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let other = s.clone();
    let intruder = std::thread::spawn(move || {
        entered_rx.recv().unwrap();
        other.append("turn/start", json!({"turn":2}), None).unwrap();
    });
    s.atomic(|| {
        s.append("turn/start", json!({"turn":0}), None).unwrap();
        entered_tx.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        s.append("turn/start", json!({"turn":1}), None).unwrap();
    });
    intruder.join().unwrap();
    let turns: Vec<_> = s
        .snapshot_events(None, None)
        .iter()
        .map(|event| event.data()["turn"].clone())
        .collect();
    assert_eq!(turns, vec![json!(0), json!(1), json!(2)]);
}
