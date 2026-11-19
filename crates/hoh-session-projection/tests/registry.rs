use crate::{test_support::*, *};
use hoh_session::{CreateSessionOptions, Session};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn supplies_the_exact_inherited_cut_to_live_restored_and_hydrated_projection_initialization()
{
    let h = harness().await;
    let handle = h.registry.register(&h.ctx, cut_unit()).await.unwrap();
    let inherited = vec![
        raw("turn/start", 0, json!({"turn":1})),
        raw(
            "turn/end",
            1,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ];
    let session = h
        .sessions
        .create(
            &h.ctx,
            Some("projection-cut".into()),
            CreateSessionOptions {
                seed: Some(inherited.clone()),
                inherited_event_count: Some(offset(2)),
                meta: Some(json!({"isSeeded":true})),
            },
        )
        .unwrap();
    assert_eq!(*handle.state_of(&session).unwrap(), 2);
    let header = serde_json::from_value((*session.header()).clone()).unwrap();
    let restored = h
        .registry
        .restore(
            &Default::default(),
            &inherited,
            offset(0),
            &header,
            session.inherited_event_count(),
        )
        .unwrap();
    assert_eq!(restored.checkpoint["test/cut"].val, Some(json!(2)));
    let mut raw_header = (*session.header()).clone();
    raw_header["id"] = json!("projection-cut-prepared");
    let prepared = Session::create(
        "projection-cut-prepared".into(),
        Some(&inherited),
        Some(raw_header),
        Some(offset(2)),
        None,
    )
    .unwrap();
    assert_eq!(
        h.registry
            .hydrate(&prepared, &Default::default(), &inherited, offset(0))
            .unwrap()
            .as_of_seq,
        seq(1)
    );
    assert_eq!(*handle.state_of(&prepared).unwrap(), 2);
    h.close().await;
}
#[tokio::test]
async fn drives_a_registered_unit_over_committed_events_and_snapshots_the_current_value() {
    let h = harness().await;
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    mark(&h.session, &["a"]);
    mark(&h.session, &["a", "b"]);
    let snapshot = h.registry.snapshot(&h.session, None).unwrap();
    assert_eq!(snapshot.as_of_seq, seq(h.session.seq().get() as i64 - 1));
    assert_eq!(values(snapshot), json!({"test/marks":{"marks":["a","b"]}}));
    h.close().await;
}
#[tokio::test]
async fn builds_the_cell_lazily_from_the_full_log_for_a_unit_registered_after_events_flowed() {
    let h = harness().await;
    mark(&h.session, &["pre-registration"]);
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    assert_eq!(
        values(h.registry.snapshot(&h.session, None).unwrap()),
        json!({"test/marks":{"marks":["pre-registration"]}})
    );
    mark(&h.session, &["after"]);
    assert_eq!(
        values(h.registry.snapshot(&h.session, None).unwrap()),
        json!({"test/marks":{"marks":["after"]}})
    );
    h.close().await;
}
#[tokio::test]
async fn serves_init_derived_state_and_as_of_seq_minus_one_for_an_empty_log() {
    let h = harness().await;
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    let snapshot = h.registry.snapshot(&h.session, None).unwrap();
    assert_eq!(snapshot.as_of_seq, seq(-1));
    assert_eq!(values(snapshot), json!({"test/marks":{"marks":[]}}));
    h.close().await;
}
#[tokio::test]
async fn notifies_on_changed_with_the_validated_view_and_the_causing_seq_and_skips_same_reference_applies()
 {
    let h = harness().await;
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let sink = seen.clone();
    h.registry
        .on_changed(
            &h.ctx,
            Arc::new(move |session, key, value, seq| {
                lock(&sink).push((
                    session.id().to_string(),
                    key.to_owned(),
                    (*value).clone(),
                    seq.get(),
                ));
                Ok(())
            }),
        )
        .unwrap();
    let event = mark(&h.session, &["a"]);
    turn(&h.session, 1);
    assert_eq!(
        *lock(&seen),
        vec![(
            h.session.id().to_string(),
            "test/marks".into(),
            json!({"marks":["a"]}),
            event.seq().get()
        )]
    );
    h.close().await;
}
#[tokio::test]
async fn notifies_on_event_changed_once_per_event_with_every_changed_key_in_registration_order() {
    let h = harness().await;
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    h.registry.register(&h.ctx, count_unit()).await.unwrap();
    let cache = Arc::new(Mutex::new(std::collections::HashMap::<
        usize,
        (Arc<Value>, Arc<Value>),
    >::new()));
    h.registry.register(&h.ctx,stable_unit(Arc::new(move |state| {
        let key=Arc::as_ptr(&state.value) as usize;let mut cache=lock(&cache);
        let entry=cache.entry(key).or_insert_with(||(state.value.clone(),Arc::new(json!({"marks":state.value["marks"].as_array().unwrap().iter().map(|s|s.as_str().unwrap().to_uppercase()).collect::<Vec<_>>()}))));Ok(Some(entry.1.clone()))
    }))).await.unwrap();
    let order = Arc::new(Mutex::new(vec![]));
    let sink = order.clone();
    h.registry
        .on_changed(
            &h.ctx,
            Arc::new(move |_, key, _, _| {
                lock(&sink).push(format!("key:{key}"));
                Ok(())
            }),
        )
        .unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let sink = seen.clone();
    let ordered = order.clone();
    h.registry
        .on_event_changed(
            &h.ctx,
            Arc::new(move |session, changes, seq| {
                lock(&ordered).push("event".into());
                lock(&sink).push((session.id().to_string(), json(&changes), seq.get()));
                Ok(())
            }),
        )
        .unwrap();
    let event = mark(&h.session, &["a"]);
    turn(&h.session, 1);
    assert_eq!(
        *lock(&order),
        vec!["key:test/marks", "key:test/stable-view", "event"]
    );
    assert_eq!(
        *lock(&seen),
        vec![(
            h.session.id().to_string(),
            json!([{"key":"test/marks","val":{"marks":["a"]}},{"key":"test/stable-view","val":{"marks":["A"]}}]),
            event.seq().get()
        )]
    );
    h.close().await;
}
#[tokio::test]
async fn still_delivers_the_changes_made_before_a_later_unit_throws() {
    let h = harness().await;
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    h.registry
        .register(
            &h.ctx,
            stable_unit(Arc::new(|_| Ok(Some(Arc::new(json!({"marks":1})))))),
        )
        .await
        .unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let sink = seen.clone();
    h.registry
        .on_event_changed(
            &h.ctx,
            Arc::new(move |_, changes, seq| {
                lock(&sink).push((json(&changes), seq.get()));
                Ok(())
            }),
        )
        .unwrap();
    let event = mark(&h.session, &["a"]);
    assert_eq!(
        *lock(&seen),
        vec![(
            json!([{"key":"test/marks","val":{"marks":["a"]}}]),
            event.seq().get()
        )]
    );
    h.close().await;
}
#[tokio::test]
async fn computes_views_for_an_event_listener_alone_and_stops_after_its_disposer_runs() {
    let h = harness().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    h.registry
        .register(
            &h.ctx,
            stable_unit(Arc::new(move |state| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(Some(state.value.clone()))
            })),
        )
        .await
        .unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let sink = seen.clone();
    let stop = h
        .registry
        .on_event_changed(
            &h.ctx,
            Arc::new(move |_, changes, seq| {
                lock(&sink).push((
                    changes
                        .iter()
                        .map(|change| change.key.clone())
                        .collect::<Vec<_>>(),
                    seq.get(),
                ));
                Ok(())
            }),
        )
        .unwrap();
    let event = turn(&h.session, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        *lock(&seen),
        vec![(vec!["test/stable-view".into()], event.seq().get())]
    );
    drop(stop.dispose());
    mark(&h.session, &["after-stop"]);
    assert_eq!(lock(&seen).len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    h.close().await;
}
#[tokio::test]
async fn removes_event_level_change_listeners_when_their_owning_fiber_unloads_hmr_safety() {
    let h = harness().await;
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    let (fiber, ctx) = owner(&h.ctx).await;
    let seen = Arc::new(Mutex::new(vec![]));
    let sink = seen.clone();
    h.registry
        .on_event_changed(
            &ctx,
            Arc::new(move |_, changes, _| {
                lock(&sink).push(
                    changes
                        .iter()
                        .map(|change| change.key.clone())
                        .collect::<Vec<_>>(),
                );
                Ok(())
            }),
        )
        .unwrap();
    mark(&h.session, &["live"]);
    fiber.dispose().await;
    mark(&h.session, &["after-dispose"]);
    assert_eq!(*lock(&seen), vec![vec!["test/marks"]]);
    assert_eq!(
        values(h.registry.snapshot(&h.session, None).unwrap()),
        json!({"test/marks":{"marks":["after-dispose"]}})
    );
    h.close().await;
}
#[tokio::test]
async fn does_not_compute_a_view_while_no_change_listener_exists() {
    let h = harness().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let handle = h
        .registry
        .register(
            &h.ctx,
            stable_unit(Arc::new(move |state| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(Some(state.value.clone()))
            })),
        )
        .await
        .unwrap();
    turn(&h.session, 1);
    turn(&h.session, 2);
    assert_eq!(handle.state_of(&h.session).unwrap().revision, 2);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    h.close().await;
}
#[tokio::test]
async fn publishes_the_first_observed_view_and_suppresses_later_same_reference_views() {
    let h = harness().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    h.registry
        .register(
            &h.ctx,
            stable_unit(Arc::new(move |state| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(Some(state.value.clone()))
            })),
        )
        .await
        .unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let sink = seen.clone();
    h.registry
        .on_changed(
            &h.ctx,
            Arc::new(move |_, _, value, _| {
                lock(&sink).push((*value).clone());
                Ok(())
            }),
        )
        .unwrap();
    turn(&h.session, 1);
    turn(&h.session, 2);
    assert_eq!(*lock(&seen), vec![json!({"marks":[]})]);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    mark(&h.session, &["changed"]);
    assert_eq!(
        *lock(&seen),
        vec![json!({"marks":[]}), json!({"marks":["changed"]})]
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    h.close().await;
}
#[tokio::test]
async fn publishes_the_first_view_after_an_unobserved_state_change() {
    let h = harness().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    h.registry
        .register(
            &h.ctx,
            stable_unit(Arc::new(move |state| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(Some(state.value.clone()))
            })),
        )
        .await
        .unwrap();
    let first = Arc::new(Mutex::new(vec![]));
    let sink = first.clone();
    let stop = h
        .registry
        .on_changed(
            &h.ctx,
            Arc::new(move |_, _, value, _| {
                lock(&sink).push((*value).clone());
                Ok(())
            }),
        )
        .unwrap();
    turn(&h.session, 1);
    drop(stop.dispose());
    turn(&h.session, 2);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let resumed = Arc::new(Mutex::new(vec![]));
    let sink = resumed.clone();
    h.registry
        .on_changed(
            &h.ctx,
            Arc::new(move |_, _, value, _| {
                lock(&sink).push((*value).clone());
                Ok(())
            }),
        )
        .unwrap();
    turn(&h.session, 3);
    assert_eq!(*lock(&first), vec![json!({"marks":[]})]);
    assert_eq!(*lock(&resumed), vec![json!({"marks":[]})]);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    h.close().await;
}
#[tokio::test]
async fn drives_independently_per_session_cells_are_per_session_watermarks() {
    let h = harness().await;
    let other = h.another("other");
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    mark(&h.session, &["one"]);
    mark(&other, &["two"]);
    assert_eq!(
        values(h.registry.snapshot(&h.session, None).unwrap()),
        json!({"test/marks":{"marks":["one"]}})
    );
    assert_eq!(
        values(h.registry.snapshot(&other, None).unwrap()),
        json!({"test/marks":{"marks":["two"]}})
    );
    h.close().await;
}
#[tokio::test]
async fn updates_host_only_units_without_publishing_them_to_wire_listeners() {
    let h = harness().await;
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    let count = h.registry.register(&h.ctx, count_unit()).await.unwrap();
    let keys = Arc::new(Mutex::new(vec![]));
    let sink = keys.clone();
    h.registry
        .on_changed(
            &h.ctx,
            Arc::new(move |_, key, _, _| {
                lock(&sink).push(key.to_owned());
                Ok(())
            }),
        )
        .unwrap();
    turn(&h.session, 1);
    assert!(lock(&keys).is_empty());
    assert_eq!(*count.state_of(&h.session).unwrap(), 1);
    assert_eq!(
        values(h.registry.snapshot(&h.session, None).unwrap()),
        json!({"test/marks":{"marks":[]}})
    );
    h.close().await;
}
#[tokio::test]
async fn shares_one_unit_between_registrants_of_the_same_key() {
    let h = harness().await;
    let first = h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    let second = h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    mark(&h.session, &["kept"]);
    assert!(Arc::ptr_eq(
        &first.state_of(&h.session).unwrap(),
        &second.state_of(&h.session).unwrap()
    ));
    assert_eq!(
        values(h.registry.snapshot(&h.session, None).unwrap()),
        json!({"test/marks":{"marks":["kept"]}})
    );
    h.close().await;
}
#[tokio::test]
async fn keeps_the_unit_until_the_last_registrant_releases_it() {
    let h = harness().await;
    let first = h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    let second = h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    mark(&h.session, &["kept"]);
    first.dispose().await;
    assert_eq!(
        values(h.registry.snapshot(&h.session, None).unwrap()),
        json!({"test/marks":{"marks":["kept"]}})
    );
    second.dispose().await;
    assert!(
        h.registry
            .snapshot(&h.session, None)
            .unwrap()
            .values
            .is_empty()
    );
    h.close().await;
}
#[tokio::test]
async fn refuses_to_share_a_key_across_a_state_version_change() {
    let h = harness().await;
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    let mut unit = marks_unit();
    unit.state_version = 9;
    let error = h.registry.register(&h.ctx, unit).await.err().unwrap();
    assert!(error.to_string().contains(
        "already registered at stateVersion 1; refusing to share it with stateVersion 9"
    ));
    h.close().await;
}
#[tokio::test]
async fn rejects_a_non_integer_or_negative_state_version_at_register_time() {
    // Rust's public version field is u64. Exercise the same admission checker
    // for the source's two otherwise-unrepresentable values as well.
    for bad in [-1., 1.5] {
        assert!(
            crate::registry::checked_version("test/marks", bad)
                .unwrap_err()
                .to_string()
                .contains("stateVersion")
        );
    }
    let h = harness().await;
    let mut unit = marks_unit();
    unit.state_version = hoh_session::MAX_SAFE_INTEGER + 1;
    assert!(
        h.registry
            .register(&h.ctx, unit)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("stateVersion")
    );
    h.close().await;
}
#[tokio::test]
async fn register_disposer_releases_the_key_from_snapshots_and_frees_it_for_re_registration() {
    let h = harness().await;
    let handle = h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    mark(&h.session, &["cached"]);
    handle.dispose().await;
    assert!(
        h.registry
            .snapshot(&h.session, None)
            .unwrap()
            .values
            .is_empty()
    );
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    assert_eq!(
        values(h.registry.snapshot(&h.session, None).unwrap()),
        json!({"test/marks":{"marks":["cached"]}})
    );
    h.close().await;
}

#[tokio::test]
async fn freezes_a_released_unit_checkpoints_keep_its_last_cell_while_every_live_face_skips_it() {
    let h = harness().await;
    let (fiber, ctx) = owner(&h.ctx).await;
    h.registry.register(&ctx, marks_unit()).await.unwrap();
    h.registry.register(&h.ctx, count_unit()).await.unwrap();
    let released = mark(&h.session, &["released"]);
    assert!(h.registry.summary_keys().contains("test/marks"));
    fiber.dispose().await;
    let after = mark(&h.session, &["after"]);
    let fresh = h.another("fresh");
    assert!(!h.registry.summary_keys().contains("test/marks"));
    assert!(
        h.registry
            .snapshot(&h.session, None)
            .unwrap()
            .values
            .is_empty()
    );
    assert!(
        h.registry
            .cached_snapshot(&h.session, None)
            .unwrap()
            .is_none()
    );
    assert!(
        h.registry
            .state_of::<Value>(&h.ctx, &h.session, "test/marks")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        *h.registry
            .state_of::<u64>(&h.ctx, &h.session, "test/count")
            .unwrap()
            .unwrap(),
        2
    );
    let checkpoint = h.registry.checkpoint(&h.session).unwrap();
    assert_eq!(
        json(&checkpoint),
        json!({"test/marks":{"ver":1,"seq":released.seq().get(),"val":{"marks":["released"]}},"test/count":{"ver":1,"seq":after.seq().get(),"val":2}})
    );
    assert_eq!(
        json(&h.registry.checkpoint(&fresh).unwrap()),
        json!({"test/count":{"ver":1,"seq":-1,"val":0}})
    );
    assert_eq!(
        h.registry.restore_floor(&checkpoint).unwrap(),
        Some(offset(after.seq().get()))
    );
    assert!(
        h.registry
            .view_checkpoint(&checkpoint, None)
            .unwrap()
            .is_empty()
    );
    let events = vec![released, after];
    let restored = h
        .registry
        .restore(&checkpoint, &events, offset(0), &header(), offset(0))
        .unwrap();
    assert!(restored.snapshot.values.is_empty());
    assert_eq!(
        json(&restored.checkpoint),
        json!({"test/count":{"ver":1,"seq":1,"val":2}})
    );
    let prepared = h.prepared("hydrate-released", events.clone());
    let first = h
        .registry
        .hydrate(&prepared, &checkpoint, &events, offset(0))
        .unwrap();
    let again = h
        .registry
        .hydrate(&prepared, &checkpoint, &events, offset(0))
        .unwrap();
    assert_eq!(json(&first), json(&again));
    assert!(first.values.is_empty());
    assert!(
        !h.registry
            .checkpoint(&prepared)
            .unwrap()
            .contains_key("test/marks")
    );
    h.close().await;
}
#[tokio::test]
async fn re_registering_a_released_key_refolds_the_session_instead_of_continuing_the_frozen_cell() {
    let h = harness().await;
    let (fiber, ctx) = owner(&h.ctx).await;
    h.registry.register(&ctx, marks_unit()).await.unwrap();
    let released = mark(&h.session, &["released"]);
    fiber.dispose().await;
    let after = mark(&h.session, &["after"]);
    assert_eq!(
        json(&h.registry.checkpoint(&h.session).unwrap()["test/marks"]),
        json!({"ver":1,"seq":released.seq().get(),"val":{"marks":["released"]}})
    );
    let replacement = h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    assert_eq!(
        *replacement.state_of(&h.session).unwrap(),
        json!({"marks":["after"]})
    );
    assert_eq!(
        json(&h.registry.checkpoint(&h.session).unwrap()["test/marks"]),
        json!({"ver":1,"seq":after.seq().get(),"val":{"marks":["after"]}})
    );
    h.close().await;
}
#[tokio::test]
async fn removes_registrations_and_change_listeners_when_their_owning_fiber_unloads_hmr_safety() {
    let h = harness().await;
    let (fiber, ctx) = owner(&h.ctx).await;
    h.registry.register(&ctx, marks_unit()).await.unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let sink = seen.clone();
    h.registry
        .on_changed(
            &ctx,
            Arc::new(move |_, key, _, _| {
                lock(&sink).push(key.to_owned());
                Ok(())
            }),
        )
        .unwrap();
    mark(&h.session, &["live"]);
    fiber.dispose().await;
    mark(&h.session, &["after-dispose"]);
    assert_eq!(*lock(&seen), vec!["test/marks"]);
    assert!(
        h.registry
            .snapshot(&h.session, None)
            .unwrap()
            .values
            .is_empty()
    );
    h.close().await;
}
#[tokio::test]
async fn retains_readable_state_during_owner_teardown() {
    let h = harness().await;
    let (fiber, ctx) = owner(&h.ctx).await;
    let handle = h.registry.register(&ctx, marks_unit()).await.unwrap();
    let seen = Arc::new(Mutex::new(None));
    let sink = seen.clone();
    let session = h.session.clone();
    ctx.effect("read during teardown", move || {
        hoh_kernel::Undo::sync(move || {
            *lock(&sink) = Some(handle.state_of(&session).unwrap());
        })
    })
    .unwrap();
    mark(&h.session, &["live"]);
    fiber.dispose().await;
    assert_eq!(
        lock(&seen).as_ref().map(|value| value.as_ref()),
        Some(&json!({"marks":["live"]}))
    );
    assert!(
        h.registry
            .snapshot(&h.session, None)
            .unwrap()
            .values
            .is_empty()
    );
    h.close().await;
}
#[tokio::test]
async fn snapshot_serves_client_views_and_excludes_host_only_state() {
    let h = harness().await;
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    let count = h.registry.register(&h.ctx, count_unit()).await.unwrap();
    mark(&h.session, &["a", "b"]);
    let snapshot = h.registry.snapshot(&h.session, None).unwrap();
    assert_eq!(values(snapshot), json!({"test/marks":{"marks":["a","b"]}}));
    assert_eq!(*count.state_of(&h.session).unwrap(), 1);
    h.close().await;
}
#[tokio::test]
async fn snapshot_with_keys_computes_only_the_selected_units() {
    let h = harness().await;
    mark(&h.session, &["a"]);
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let mut unit = stable();
    unit.apply = Arc::new(move |state, _| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(state.clone())
    });
    h.registry.register(&h.ctx, unit).await.unwrap();
    let keys = key_selection(&["test/marks"]);
    assert_eq!(
        values(h.registry.snapshot(&h.session, selected(&keys)).unwrap()),
        json!({"test/marks":{"marks":["a"]}})
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    h.close().await;
}
#[tokio::test]
async fn snapshot_without_keys_does_not_compute_host_only_units() {
    let h = harness().await;
    mark(&h.session, &["a"]);
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let mut unit = count_unit();
    unit.apply = Arc::new(move |state, _| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(**state + 1))
    });
    h.registry.register(&h.ctx, unit).await.unwrap();
    assert_eq!(
        values(h.registry.snapshot(&h.session, None).unwrap()),
        json!({"test/marks":{"marks":["a"]}})
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    h.close().await;
}
#[tokio::test]
async fn snapshot_with_keys_succeeds_while_an_unselected_unit_fails_to_apply() {
    let h = harness().await;
    mark(&h.session, &["a"]);
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    let mut unit = stable();
    unit.apply = Arc::new(|_, _| Err(error("unselected unit failed")));
    h.registry.register(&h.ctx, unit).await.unwrap();
    let keys = key_selection(&["test/marks"]);
    assert_eq!(
        values(h.registry.snapshot(&h.session, selected(&keys)).unwrap()),
        json!({"test/marks":{"marks":["a"]}})
    );
    assert_eq!(
        h.registry
            .snapshot(&h.session, None)
            .unwrap_err()
            .to_string(),
        "unselected unit failed"
    );
    h.close().await;
}
#[tokio::test]
async fn checkpoints_every_persisted_unit_with_its_state_version_and_per_cell_watermark() {
    let h = harness().await;
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    let mut count = count_unit();
    count.state_version = 7;
    h.registry.register(&h.ctx, count).await.unwrap();
    let event = mark(&h.session, &["a"]);
    assert_eq!(
        json(&h.registry.checkpoint(&h.session).unwrap()),
        json!({"test/marks":{"ver":1,"seq":event.seq().get(),"val":{"marks":["a"]}},"test/count":{"ver":7,"seq":event.seq().get(),"val":1}})
    );
    let fresh = h.another("fresh");
    assert_eq!(
        json(&h.registry.checkpoint(&fresh).unwrap()["test/marks"]),
        json!({"ver":1,"seq":-1,"val":null})
    );
    h.close().await;
}
#[tokio::test]
async fn checkpoint_states_are_detached_clones_mutating_them_cannot_corrupt_the_watermark_cache() {
    let h = harness().await;
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    mark(&h.session, &["a"]);
    let mut checkpoint = h.registry.checkpoint(&h.session).unwrap();
    checkpoint["test/marks"].val.as_mut().unwrap()["marks"]
        .as_array_mut()
        .unwrap()
        .push(json!("INJECTED"));
    assert_eq!(
        values(h.registry.snapshot(&h.session, None).unwrap()),
        json!({"test/marks":{"marks":["a"]}})
    );
    assert_eq!(
        h.registry.checkpoint(&h.session).unwrap()["test/marks"].val,
        Some(json!({"marks":["a"]}))
    );
    h.close().await;
}
#[tokio::test]
async fn restore_floor_anchors_one_below_the_lowest_usable_watermark_and_at_zero_for_missing_or_mismatched_rows()
 {
    let h = harness().await;
    assert_eq!(h.registry.restore_floor(&Default::default()).unwrap(), None);
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    h.registry.register(&h.ctx, count_unit()).await.unwrap();
    assert_eq!(
        h.registry.restore_floor(&Default::default()).unwrap(),
        Some(offset(0))
    );
    for (marks_version, marks_seq, count_seq, expected) in
        [(1, 10, 5, 5), (2, 10, 5, 0), (1, -1, -1, 0)]
    {
        let checkpoint = rows(vec![
            ("test/marks", row(marks_version, marks_seq, Value::Null)),
            ("test/count", row(1, count_seq, json!(0))),
        ]);
        assert_eq!(
            h.registry.restore_floor(&checkpoint).unwrap(),
            Some(offset(expected))
        );
    }
    h.close().await;
}
#[tokio::test]
async fn restore_folds_the_tail_past_each_usable_row_and_refolds_from_init_on_version_mismatch() {
    let h = harness().await;
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    h.registry.register(&h.ctx, count_unit()).await.unwrap();
    let tail = vec![
        raw("test/mark", 3, json!({"marks":["new"]})),
        raw(
            "turn/end",
            4,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ];
    let checkpoint = rows(vec![
        ("test/marks", row(1, 2, json!({"marks":["old"]}))),
        ("test/count", row(99, 2, json!(3))),
    ]);
    assert!(
        h.registry
            .restore(&checkpoint, &tail, offset(3), &header(), offset(0))
            .unwrap_err()
            .to_string()
            .contains("re-read from seq 0")
    );
    let mut full = vec![
        raw("turn/start", 0, json!({"turn":1})),
        raw("test/mark", 1, json!({"marks":["old"]})),
        raw("test/mark", 2, json!({"marks":["old","2"]})),
    ];
    full.extend(tail);
    let restored = h
        .registry
        .restore(&checkpoint, &full, offset(0), &header(), offset(0))
        .unwrap();
    assert_eq!(restored.snapshot.as_of_seq, seq(4));
    assert_eq!(
        values(restored.snapshot),
        json!({"test/marks":{"marks":["new"]}})
    );
    assert_eq!(
        json(&restored.checkpoint),
        json!({"test/marks":{"ver":1,"seq":4,"val":{"marks":["new"]}},"test/count":{"ver":1,"seq":4,"val":5}})
    );
    h.close().await;
}
#[tokio::test]
async fn restore_over_a_suffix_folds_only_past_each_row_watermark_and_serves_an_exact_empty_tail_cut()
 {
    let h = harness().await;
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    h.registry.register(&h.ctx, count_unit()).await.unwrap();
    let checkpoint = rows(vec![
        ("test/marks", row(1, 4, json!({"marks":["done"]}))),
        ("test/count", row(1, 2, json!(3))),
    ]);
    let tail = vec![
        raw("turn/start", 3, json!({"turn":2})),
        raw(
            "turn/end",
            4,
            json!({"turn":2,"reason":{"kind":"completed"}}),
        ),
    ];
    let restored = h
        .registry
        .restore(&checkpoint, &tail, offset(3), &header(), offset(0))
        .unwrap();
    assert_eq!(restored.snapshot.as_of_seq, seq(4));
    assert_eq!(
        values(restored.snapshot),
        json!({"test/marks":{"marks":["done"]}})
    );
    assert_eq!(
        json(&restored.checkpoint["test/count"]),
        json!({"ver":1,"seq":4,"val":5})
    );
    let restored = h
        .registry
        .restore(&restored.checkpoint, &[], offset(5), &header(), offset(0))
        .unwrap();
    assert_eq!(restored.snapshot.as_of_seq, seq(4));
    assert!(!restored.snapshot.values.contains_key("test/count"));
    assert_eq!(
        json(&restored.checkpoint["test/count"]),
        json!({"ver":1,"seq":4,"val":5})
    );
    h.close().await;
}
#[tokio::test]
async fn view_checkpoint_serves_version_matching_rows_without_any_log_and_skips_mismatched_keys() {
    let h = harness().await;
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    h.registry.register(&h.ctx, count_unit()).await.unwrap();
    let checkpoint = rows(vec![
        ("test/marks", row(1, 4, json!({"marks":["stored"]}))),
        ("test/count", row(99, 4, json!(5))),
    ]);
    assert_eq!(
        json(&h.registry.view_checkpoint(&checkpoint, None).unwrap()),
        json!({"test/marks":{"marks":["stored"]}})
    );
    assert!(
        h.registry
            .view_checkpoint(&Default::default(), None)
            .unwrap()
            .is_empty()
    );
    h.close().await;
}
#[tokio::test]
async fn view_checkpoint_and_restore_exclude_host_only_state_while_retaining_its_checkpoint() {
    let h = harness().await;
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    h.registry.register(&h.ctx, count_unit()).await.unwrap();
    let checkpoint = rows(vec![
        ("test/marks", row(1, 4, json!({"marks":["stored"]}))),
        ("test/count", row(1, 4, json!(5))),
    ]);
    assert_eq!(
        json(&h.registry.view_checkpoint(&checkpoint, None).unwrap()),
        json!({"test/marks":{"marks":["stored"]}})
    );
    let restored = h
        .registry
        .restore(&checkpoint, &[], offset(5), &header(), offset(0))
        .unwrap();
    assert_eq!(
        values(restored.snapshot),
        json!({"test/marks":{"marks":["stored"]}})
    );
    assert_eq!(
        json(&restored.checkpoint["test/count"]),
        json(&checkpoint["test/count"])
    );
    h.close().await;
}

async fn assert_proto_wire_values_are_omitted<S>(
    mut definition: ProjectionDefinition<S>,
    checkpoint_value: Value,
) where
    S: Serialize + Send + Sync + 'static,
{
    definition.key = "__proto__".into();
    let h = harness().await;
    h.registry.register(&h.ctx, definition).await.unwrap();

    assert_eq!(
        values(h.registry.snapshot(&h.session, None).unwrap()),
        json!({})
    );
    assert_eq!(
        values(
            h.registry
                .cached_snapshot(&h.session, None)
                .unwrap()
                .unwrap()
        ),
        json!({})
    );

    let checkpoint = rows(vec![("__proto__", row(1, -1, checkpoint_value))]);
    assert_eq!(
        json(&h.registry.view_checkpoint(&checkpoint, None).unwrap()),
        json!({})
    );
    let prepared = h.prepared("proto-hydrate", vec![]);
    assert_eq!(
        values(
            h.registry
                .hydrate(&prepared, &checkpoint, &[], offset(0))
                .unwrap()
        ),
        json!({})
    );
    h.close().await;
}

#[tokio::test]
async fn proto_wire_values_are_omitted_from_every_plain_object_projection_face() {
    let mut primitive = reader_unit();
    primitive.init = Arc::new(|_, _| Ok(Arc::new(5)));
    assert_proto_wire_values_are_omitted(primitive, json!(5)).await;

    let object_value = json!({"marks":["object"]});
    let mut object = marks_unit();
    let initial = Arc::new(object_value.clone());
    object.init = Arc::new(move |_, _| Ok(initial.clone()));
    assert_proto_wire_values_are_omitted(object, object_value).await;
}

#[tokio::test]
async fn rejects_version_matching_rows_whose_state_no_longer_matches_the_registered_schema() {
    let h = harness().await;
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    let checkpoint = rows(vec![(
        "test/marks",
        row(1, 2, json!({"marks":"not-an-array"})),
    )]);
    assert!(
        h.registry
            .view_checkpoint(&checkpoint, None)
            .unwrap()
            .is_empty()
    );
    assert!(
        h.registry
            .restore(&checkpoint, &[], offset(3), &header(), offset(0))
            .is_err()
    );
    h.close().await;
}
#[tokio::test]
async fn restore_rejects_a_row_claiming_events_past_the_supplied_log_end_shrunk_log_requires_re_read()
 {
    let h = harness().await;
    h.registry.register(&h.ctx, count_unit()).await.unwrap();
    let checkpoint = rows(vec![("test/count", row(1, 9, json!(10)))]);
    assert_eq!(
        h.registry.restore_floor(&checkpoint).unwrap(),
        Some(offset(9))
    );
    let anchor = vec![raw(
        "turn/end",
        9,
        json!({"turn":2,"reason":{"kind":"completed"}}),
    )];
    let restored = h
        .registry
        .restore(&checkpoint, &anchor, offset(9), &header(), offset(0))
        .unwrap();
    assert_eq!(
        json(&restored.checkpoint["test/count"]),
        json!({"ver":1,"seq":9,"val":10})
    );
    assert!(restored.snapshot.values.is_empty());
    assert!(
        h.registry
            .restore(&checkpoint, &[], offset(9), &header(), offset(0))
            .unwrap_err()
            .to_string()
            .contains("re-read from seq 0")
    );
    let full = vec![
        raw("turn/start", 0, json!({"turn":1})),
        raw(
            "turn/end",
            1,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ];
    let restored = h
        .registry
        .restore(&checkpoint, &full, offset(0), &header(), offset(0))
        .unwrap();
    assert_eq!(restored.snapshot.as_of_seq, seq(1));
    assert!(restored.snapshot.values.is_empty());
    assert_eq!(
        json(&restored.checkpoint["test/count"]),
        json!({"ver":1,"seq":1,"val":2})
    );
    h.close().await;
}
#[tokio::test]
async fn fails_loud_when_a_unit_view_violates_its_own_schema_async_unit_output_is_unrepresentable()
{
    let h = harness().await;
    let mut unit = marks_unit();
    unit.wire.as_mut().unwrap().view = Arc::new(|_| Ok(Some(Arc::new(json!({"marks":1})))));
    h.registry.register(&h.ctx, unit).await.unwrap();
    assert!(h.registry.snapshot(&h.session, None).is_err());
    h.close().await;
}
#[tokio::test]
async fn keeps_a_cell_built_before_a_fresh_session_is_announced() {
    let h = harness().await;
    let handle = h.registry.register(&h.ctx, stable()).await.unwrap();
    let prepared = h
        .sessions
        .prepare(Some("early-cell".into()), Default::default())
        .unwrap();
    let before = handle.state_of(&prepared).unwrap();
    let detach = h.sessions.enter(&h.ctx, prepared.clone()).unwrap();
    h.sessions.announce(&prepared).unwrap();
    assert!(Arc::ptr_eq(&before, &handle.state_of(&prepared).unwrap()));
    detach.detach();
    h.close().await;
}
#[tokio::test]
async fn restore_rejects_events_that_are_not_contiguous_from_base_seq() {
    let h = harness().await;
    h.registry.register(&h.ctx, count_unit()).await.unwrap();
    let gapped = vec![
        raw("turn/start", 0, json!({"turn":1})),
        raw("turn/start", 2, json!({"turn":2})),
    ];
    assert!(
        h.registry
            .restore(
                &Default::default(),
                &gapped,
                offset(0),
                &header(),
                offset(0)
            )
            .unwrap_err()
            .to_string()
            .contains("cannot restore across missing seq 1")
    );
    h.close().await;
}
#[tokio::test]
async fn hydrate_with_an_empty_event_list_serves_the_cut_before_base_seq() {
    let h = harness().await;
    let handle = h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    let prepared = h
        .sessions
        .prepare(Some("hydrate-empty".into()), Default::default())
        .unwrap();
    let snapshot = h
        .registry
        .hydrate(&prepared, &Default::default(), &[], offset(0))
        .unwrap();
    assert_eq!(snapshot.as_of_seq, seq(-1));
    assert_eq!(values(snapshot), json!({"test/marks":{"marks":[]}}));
    assert!(handle.state_of(&prepared).unwrap().is_null());
    h.close().await;
}
#[tokio::test]
async fn hydrate_at_an_already_hydrated_cut_serves_the_installed_cells() {
    let h = harness().await;
    let mut unit = marks_unit();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let init = unit.init.clone();
    unit.init = Arc::new(move |header, cut| {
        count.fetch_add(1, Ordering::SeqCst);
        init(header, cut)
    });
    h.registry.register(&h.ctx, unit).await.unwrap();
    h.registry.register(&h.ctx, count_unit()).await.unwrap();
    let events = vec![
        raw("test/mark", 0, json!({"marks":["seed"]})),
        raw("turn/start", 1, json!({"turn":1})),
    ];
    let prepared = h.prepared("hydrate-twice", events.clone());
    let first = h
        .registry
        .hydrate(&prepared, &Default::default(), &events, offset(0))
        .unwrap();
    let initialized = calls.load(Ordering::SeqCst);
    let second = h
        .registry
        .hydrate(&prepared, &Default::default(), &events, offset(0))
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), initialized);
    assert_eq!(json(&first), json(&second));
    assert_eq!(
        json(&second),
        json!({"asOfSeq":1,"values":{"test/marks":{"marks":["seed"]}}})
    );
    h.close().await;
}
#[tokio::test]
async fn hydrate_keeps_a_cell_that_is_already_past_the_hydrated_cut() {
    let h = harness().await;
    let handle = h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    let events = vec![
        raw("test/mark", 0, json!({"marks":["first"]})),
        raw("test/mark", 1, json!({"marks":["second"]})),
    ];
    let prepared = h.prepared("hydrate-behind", events.clone());
    assert_eq!(
        *handle.state_of(&prepared).unwrap(),
        json!({"marks":["second"]})
    );
    let hydrated = h
        .registry
        .hydrate(&prepared, &Default::default(), &events[..1], offset(0))
        .unwrap();
    assert_eq!(
        json(&hydrated),
        json!({"asOfSeq":0,"values":{"test/marks":{"marks":["first"]}}})
    );
    assert_eq!(
        *handle.state_of(&prepared).unwrap(),
        json!({"marks":["second"]})
    );
    assert_eq!(
        h.registry.checkpoint(&prepared).unwrap()["test/marks"].seq,
        seq(prepared.seq().get() as i64 - 1)
    );
    h.close().await;
}
#[tokio::test]
async fn drive_skips_a_cell_that_an_earlier_session_event_listener_already_advanced() {
    let h = harness().await;
    let applied = Arc::new(Mutex::new(vec![]));
    let sink = applied.clone();
    let mut unit = count_unit();
    unit.apply = Arc::new(move |state, event| {
        lock(&sink).push(event.clone());
        Ok(Arc::new(**state + 1))
    });
    let count = h.registry.register(&h.ctx, unit).await.unwrap();
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let sink = seen.clone();
    let read = count.clone();
    h.ctx
        .listen()
        .prepend()
        .on_contained::<hoh_session::SessionEventEmitted>(move |event| {
            lock(&sink).push(*read.state_of(&event.session)?);
            Ok(())
        })
        .unwrap();
    let changed = Arc::new(Mutex::new(vec![]));
    let sink = changed.clone();
    h.registry
        .on_changed(
            &h.ctx,
            Arc::new(move |_, key, value, seq| {
                lock(&sink).push((key.to_owned(), (*value).clone(), seq.get()));
                Ok(())
            }),
        )
        .unwrap();
    let first = mark(&h.session, &["a"]);
    let second = mark(&h.session, &["b"]);
    assert_eq!(*lock(&seen), vec![1, 2]);
    assert_eq!(*lock(&applied), vec![(*first).clone(), (*second).clone()]);
    assert_eq!(*count.state_of(&h.session).unwrap(), 2);
    assert_eq!(
        *lock(&changed),
        vec![
            (
                "test/marks".into(),
                json!({"marks":["a"]}),
                first.seq().get()
            ),
            (
                "test/marks".into(),
                json!({"marks":["b"]}),
                second.seq().get()
            )
        ]
    );
    h.close().await;
}

fn identity_sequences(length: usize) -> Vec<Vec<usize>> {
    fn visit(sequence: Vec<usize>, highest: usize, length: usize, out: &mut Vec<Vec<usize>>) {
        if sequence.len() == length {
            out.push(sequence);
            return;
        }
        for value in 0..=highest + 1 {
            let mut next = sequence.clone();
            next.push(value);
            visit(next, highest.max(value), length, out);
        }
    }
    let mut result = vec![];
    visit(vec![0], 0, length, &mut result);
    result
}
fn listening(
    registry: &SessionProjectionRegistry,
    ctx: &hoh_kernel::Context,
    session: &Session,
    enabled: bool,
    stop: &mut Option<hoh_kernel::Disposer>,
    seen: &Arc<Mutex<Vec<u64>>>,
) {
    if enabled && stop.is_none() {
        let target = session.clone();
        let sink = seen.clone();
        *stop = Some(
            registry
                .on_changed(
                    ctx,
                    Arc::new(move |session, key, _, seq| {
                        if *session == target && key == "test/marks" {
                            lock(&sink).push(seq.get());
                        }
                        Ok(())
                    }),
                )
                .unwrap(),
        );
    } else if !enabled && let Some(stop) = stop.take() {
        drop(stop.dispose());
    }
}
#[tokio::test]
async fn matches_every_four_state_identity_sequence_across_listener_gaps_and_raw_view_identities() {
    let h = harness().await;
    let states = Arc::new(Mutex::new(
        std::collections::HashMap::<String, Arc<Value>>::new(),
    ));
    let views = Arc::new(Mutex::new(
        std::collections::HashMap::<usize, Arc<Value>>::new(),
    ));
    let computed = Arc::new(Mutex::new(Vec::<Arc<Value>>::new()));
    let initial = Arc::new(json!({"marks":["initial"]}));
    let mut unit = marks_unit();
    unit.init = Arc::new(move |_, _| Ok(initial.clone()));
    let state_by_event = states.clone();
    unit.apply = Arc::new(move |state, event| {
        if event.event_type() != "test/mark" {
            return Ok(state.clone());
        }
        Ok(event.data()["marks"]
            .get(0)
            .and_then(Value::as_str)
            .and_then(|token| lock(&state_by_event).get(token).cloned())
            .unwrap_or_else(|| state.clone()))
    });
    let view_by_state = views.clone();
    let calls = computed.clone();
    unit.wire.as_mut().unwrap().view = Arc::new(move |state| {
        let value = lock(&view_by_state)
            .get(&(Arc::as_ptr(state) as usize))
            .cloned()
            .ok_or_else(|| error("test state lacks a raw view"))?;
        lock(&calls).push(value.clone());
        Ok(Some(value))
    });
    h.registry.register(&h.ctx, unit).await.unwrap();
    let mut checked = 0;
    for state_sequence in identity_sequences(4) {
        let state_count = state_sequence.iter().max().unwrap() + 1;
        for view_sequence in identity_sequences(state_count) {
            for baseline in [false, true] {
                for mask in 0..8 {
                    let scenario = checked;
                    checked += 1;
                    let state_pool: Vec<_> = (0..state_count)
                        .map(|index| {
                            Arc::new(json!({"marks":[format!("state-{scenario}-{index}")]}))
                        })
                        .collect();
                    let view_pool: Vec<_> = (0..=(*view_sequence.iter().max().unwrap()))
                        .map(|_| Arc::new(json!({"marks":[]})))
                        .collect();
                    for (index, state) in state_pool.iter().enumerate() {
                        lock(&views).insert(
                            Arc::as_ptr(state) as usize,
                            view_pool[view_sequence[index]].clone(),
                        );
                    }
                    for (index, state) in state_sequence.iter().enumerate() {
                        lock(&states)
                            .insert(format!("{scenario}:{index}"), state_pool[*state].clone());
                    }
                    let session = h.another(&format!("identity-{scenario}"));
                    let notifications = Arc::new(Mutex::new(vec![]));
                    let mut stop = None;
                    listening(
                        &h.registry,
                        &h.ctx,
                        &session,
                        baseline,
                        &mut stop,
                        &notifications,
                    );
                    mark(&session, &[&format!("{scenario}:0")]);
                    lock(&computed).clear();
                    lock(&notifications).clear();
                    let mut expected_views = Vec::new();
                    let mut expected_notifications = vec![];
                    let mut comparable =
                        baseline.then(|| view_pool[view_sequence[state_sequence[0]]].clone());
                    for index in 1..4 {
                        let enabled = (mask & (1 << (index - 1))) != 0;
                        listening(
                            &h.registry,
                            &h.ctx,
                            &session,
                            enabled,
                            &mut stop,
                            &notifications,
                        );
                        if state_sequence[index] != state_sequence[index - 1] {
                            if enabled {
                                let current =
                                    view_pool[view_sequence[state_sequence[index]]].clone();
                                expected_views.push(current.clone());
                                if comparable
                                    .as_ref()
                                    .is_none_or(|old| !Arc::ptr_eq(old, &current))
                                {
                                    expected_notifications.push(index as u64);
                                }
                                comparable = Some(current);
                            } else {
                                comparable = None;
                            }
                        }
                        mark(&session, &[&format!("{scenario}:{index}")]);
                    }
                    listening(
                        &h.registry,
                        &h.ctx,
                        &session,
                        false,
                        &mut stop,
                        &notifications,
                    );
                    let actual = lock(&computed);
                    assert_eq!(
                        actual.len(),
                        expected_views.len(),
                        "state {state_sequence:?}, view {view_sequence:?}, baseline {baseline}, mask {mask}"
                    );
                    assert!(
                        actual
                            .iter()
                            .zip(&expected_views)
                            .all(|(actual, expected)| Arc::ptr_eq(actual, expected))
                    );
                    assert_eq!(
                        *lock(&notifications),
                        expected_notifications,
                        "state {state_sequence:?}, view {view_sequence:?}, baseline {baseline}, mask {mask}"
                    );
                }
            }
        }
    }
    assert_eq!(checked, 960);
    h.close().await;
}

#[tokio::test]
async fn primitive_identity_and_checkpoint_undefined_are_distinct_from_null() {
    let h = harness().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let mut unit = reader_unit();
    unit.apply = Arc::new(|state, _| Ok(Arc::new(**state)));
    unit.wire.as_mut().unwrap().view = Arc::new(move |state| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(Some(Arc::new(json!(**state))))
    });
    h.registry.register(&h.ctx, unit).await.unwrap();
    h.registry
        .on_changed(&h.ctx, Arc::new(|_, _, _, _| Ok(())))
        .unwrap();
    turn(&h.session, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let optional = ProjectionDefinition {
        key: "optional".into(),
        state_schema: Arc::new(|value: Option<&Value>| match value {
            None => Ok(Arc::new(None::<u64>)),
            Some(value) => Ok(Arc::new(Some(*number_schema(Some(value))?))),
        }),
        init: Arc::new(|_, _| Ok(Arc::new(None::<u64>))),
        apply: Arc::new(|state: &Arc<Option<u64>>, _| Ok(state.clone())),
        wire: None,
        state_version: 1,
    };
    h.registry.register(&h.ctx, optional).await.unwrap();
    h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    let checkpoint = h.registry.checkpoint(&h.session).unwrap();
    assert!(checkpoint["optional"].val.is_none());
    assert_eq!(checkpoint["test/marks"].val, Some(Value::Null));
    let encoded = json(&checkpoint);
    assert!(encoded["optional"].get("val").is_none());
    assert_eq!(encoded["test/marks"]["val"], Value::Null);
    let decoded: ProjectionCheckpoint = serde_json::from_value(encoded).unwrap();
    assert!(decoded["optional"].val.is_none());
    assert_eq!(decoded["test/marks"].val, Some(Value::Null));
    h.close().await;
}

#[tokio::test]
async fn session_cells_distinguish_prepared_lifecycles_with_identical_ids_and_headers() {
    let h = harness().await;
    let handle = h.registry.register(&h.ctx, marks_unit()).await.unwrap();
    let prepared = |text: &str| {
        h.sessions
            .prepare(
                Some("same-id".into()),
                hoh_session::PrepareSessionOptions::Create(CreateSessionOptions {
                    seed: Some(vec![raw("test/mark", 0, json!({"marks":[text]}))]),
                    meta: Some(json!({"createdAt":0})),
                    ..Default::default()
                }),
            )
            .unwrap()
    };
    let first = prepared("one");
    let second = prepared("two");
    assert_eq!(first.header(), second.header());
    assert_ne!(first, second);
    assert_eq!(
        *handle.reader_for(first)().unwrap(),
        json!({"marks":["one"]})
    );
    assert_eq!(
        *handle.reader_for(second)().unwrap(),
        json!({"marks":["two"]})
    );
    h.close().await;
}
#[tokio::test]
async fn snapshots_keep_one_consistent_cut_while_another_thread_commits_events() {
    let h = harness().await;
    let mut first = reader_unit();
    first.key = "left".into();
    let mut second = reader_unit();
    second.key = "right".into();
    h.registry.register(&h.ctx, first).await.unwrap();
    h.registry.register(&h.ctx, second).await.unwrap();
    let session = h.session.clone();
    let writer = tokio::task::spawn_blocking(move || {
        for index in 1..=300 {
            turn(&session, index);
        }
    });
    while !writer.is_finished() {
        let snapshot = h.registry.snapshot(&h.session, None).unwrap();
        let expected = match snapshot.as_of_seq {
            hoh_session::SessionSeqCursor::BeforeFirst => 0,
            hoh_session::SessionSeqCursor::Seq(seq) => seq.get() + 1,
        };
        assert_eq!(snapshot.values["left"].as_u64(), Some(expected));
        assert_eq!(snapshot.values["right"].as_u64(), Some(expected));
        tokio::task::yield_now().await;
    }
    writer.await.unwrap();
    let snapshot = h.registry.snapshot(&h.session, None).unwrap();
    assert_eq!(snapshot.as_of_seq, seq(299));
    assert_eq!(snapshot.values["left"].as_u64(), Some(300));
    assert_eq!(snapshot.values["right"].as_u64(), Some(300));
    h.close().await;
}
