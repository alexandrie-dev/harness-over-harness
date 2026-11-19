use crate::{test_support::*, *};
use hoh_error::SharedError;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Serialize, Debug, PartialEq)]
#[serde(untagged)]
enum HostState {
    Turn { turn: u64 },
    Null,
}
#[tokio::test]
async fn binds_typed_host_states_to_the_exact_session_without_reading_unrelated_units() {
    let h = harness().await;
    let (fiber, ctx) = owner(&h.ctx).await;
    let unrelated_init = Arc::new(AtomicUsize::new(0));
    let counted = unrelated_init.clone();
    let mut unrelated = count_unit();
    unrelated.key = "test/bound-unrelated".into();
    unrelated.init = Arc::new(move |_, _| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(0))
    });
    unrelated.apply = Arc::new(|state, _| Ok(state.clone()));
    h.registry.register(&h.ctx, unrelated).await.unwrap();
    let count_init = Arc::new(AtomicUsize::new(0));
    let counted = count_init.clone();
    let mut definition = reader_unit();
    definition.init = Arc::new(move |_, _| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(0))
    });
    let count = h.registry.register(&ctx, definition).await.unwrap();
    let host_init = Arc::new(AtomicUsize::new(0));
    let counted = host_init.clone();
    let host = h
        .registry
        .register(
            &ctx,
            ProjectionDefinition {
                key: "test/bound-host".into(),
                state_version: 1,
                state_schema: Arc::new(|value| match value {
                    Some(Value::Null) => Ok(Arc::new(HostState::Null)),
                    Some(value) => Ok(Arc::new(HostState::Turn {
                        turn: *number_schema(value.get("turn"))?,
                    })),
                    None => Err(error("missing host state")),
                }),
                init: Arc::new(move |_, _| {
                    counted.fetch_add(1, Ordering::SeqCst);
                    Ok(Arc::new(HostState::Null))
                }),
                apply: Arc::new(|state: &Arc<HostState>, event| {
                    if event.event_type() == "turn/start" {
                        Ok(Arc::new(HostState::Turn {
                            turn: *number_schema(event.data().get("turn"))?,
                        }))
                    } else {
                        Ok(state.clone())
                    }
                }),
                wire: None,
            },
        )
        .await
        .unwrap();
    let read: ProjectionReader<u64> = count.reader_for(h.session.clone());
    let host_read: ProjectionReader<HostState> = host.reader_for(h.session.clone());
    assert_eq!(count_init.load(Ordering::SeqCst), 0);
    assert_eq!(host_init.load(Ordering::SeqCst), 0);
    assert_eq!(*read().unwrap(), 0);
    assert_eq!(*host_read().unwrap(), HostState::Null);
    assert_eq!(unrelated_init.load(Ordering::SeqCst), 0);
    let other = h.another("other");
    let other_read = count.reader_for(other.clone());
    turn(&other, 1);
    assert_eq!(*other_read().unwrap(), 1);
    assert_eq!(*read().unwrap(), 0);
    turn(&h.session, 2);
    assert_eq!(*read().unwrap(), 1);
    assert_eq!(*host_read().unwrap(), HostState::Turn { turn: 2 });
    assert!(Arc::ptr_eq(
        &host_read().unwrap(),
        &host.state_of(&h.session).unwrap()
    ));
    fiber.dispose().await;
    h.close().await;
}
#[tokio::test]
async fn retains_the_bound_unit_after_owner_disposal_and_version_replacement() {
    let h = harness().await;
    let (fiber, ctx) = owner(&h.ctx).await;
    let original = h.registry.register(&ctx, reader_unit()).await.unwrap();
    let read = original.reader_for(h.session.clone());
    fiber.dispose().await;
    assert!(h.registry.checkpoint(&h.session).unwrap().is_empty());
    let mut definition = reader_unit();
    definition.state_version = 2;
    definition.apply = Arc::new(|state, _| Ok(Arc::new(**state + 10)));
    let replacement = h.registry.register(&h.ctx, definition).await.unwrap();
    let changed = Arc::new(AtomicUsize::new(0));
    let counted = changed.clone();
    h.registry
        .on_changed(
            &h.ctx,
            Arc::new(move |_, _, _, _| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
        )
        .unwrap();
    turn(&h.session, 1);
    assert_eq!(*read().unwrap(), 1);
    assert_eq!(*original.reader_for(h.session.clone())().unwrap(), 1);
    assert_eq!(*replacement.state_of(&h.session).unwrap(), 10);
    assert_eq!(
        values(h.registry.snapshot(&h.session, None).unwrap()),
        json!({"test/reader":10})
    );
    assert_eq!(changed.load(Ordering::SeqCst), 1);
    assert_eq!(
        h.registry.checkpoint(&h.session).unwrap()["test/reader"].ver,
        2
    );
    h.close().await;
}
#[tokio::test]
async fn does_not_read_during_binding_and_preserves_the_original_synchronous_failure() {
    let h = harness().await;
    let failure = SharedError::new(std::io::Error::other("host state cannot initialize"));
    let cause = failure.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let mut unit = count_unit();
    unit.key = "test/bound-failure".into();
    unit.init = Arc::new(move |_, _| {
        counted.fetch_add(1, Ordering::SeqCst);
        Err(cause.clone().into())
    });
    let handle = h.registry.register(&h.ctx, unit).await.unwrap();
    let reader = handle.reader_for(h.session.clone());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let caught = reader().unwrap_err();
    assert!(SharedError::of(caught.as_ref()).unwrap().ptr_eq(&failure));
    h.close().await;
}
#[tokio::test]
async fn preserves_undefined_state_without_substituting_a_value() {
    let h = harness().await;
    let unit = ProjectionDefinition {
        key: "test/bound-undefined".into(),
        state_version: 1,
        state_schema: Arc::new(|value: Option<&Value>| match value {
            None => Ok(Arc::new(None::<u64>)),
            Some(value) => Ok(Arc::new(Some(*number_schema(Some(value))?))),
        }),
        init: Arc::new(|_, _| Ok(Arc::new(None::<u64>))),
        apply: Arc::new(|state: &Arc<Option<u64>>, event| {
            if event.event_type() == "turn/start" {
                Ok(Arc::new(Some(*number_schema(event.data().get("turn"))?)))
            } else {
                Ok(state.clone())
            }
        }),
        wire: None,
    };
    let handle = h.registry.register(&h.ctx, unit).await.unwrap();
    let reader: ProjectionReader<Option<u64>> = handle.reader_for(h.session.clone());
    assert_eq!(*reader().unwrap(), None);
    turn(&h.session, 1);
    assert_eq!(*reader().unwrap(), Some(1));
    h.close().await;
}
#[tokio::test]
async fn reads_new_events_after_handle_or_fiber_disposal_without_rejoining_snapshots_or_notifications()
 {
    for via_handle in [true, false] {
        let h = harness().await;
        let (fiber, ctx) = owner(&h.ctx).await;
        let apply = Arc::new(AtomicUsize::new(0));
        let counted = apply.clone();
        let mut unit = reader_unit();
        unit.apply = Arc::new(move |state, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(**state + 1))
        });
        let handle = h.registry.register(&ctx, unit).await.unwrap();
        let reader = handle.reader_for(h.session.clone());
        let changes = Arc::new(AtomicUsize::new(0));
        let counted = changes.clone();
        h.registry
            .on_changed(
                &h.ctx,
                Arc::new(move |_, _, _, _| {
                    counted.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }),
            )
            .unwrap();
        assert_eq!(*handle.state_of(&h.session).unwrap(), 0);
        turn(&h.session, 1);
        assert_eq!(*handle.state_of(&h.session).unwrap(), 1);
        assert_eq!(changes.load(Ordering::SeqCst), 1);
        if via_handle {
            handle.dispose().await;
        } else {
            fiber.dispose().await;
        }
        turn(&h.session, 2);
        assert_eq!(apply.load(Ordering::SeqCst), 1);
        assert_eq!(*handle.state_of(&h.session).unwrap(), 2);
        assert_eq!(*reader().unwrap(), 2);
        assert_eq!(apply.load(Ordering::SeqCst), 2);
        assert_eq!(changes.load(Ordering::SeqCst), 1);
        assert!(
            h.registry
                .snapshot(&h.session, None)
                .unwrap()
                .values
                .is_empty()
        );
        assert_eq!(
            json(&h.registry.checkpoint(&h.session).unwrap()),
            json!({"test/reader":{"ver":1,"seq":1,"val":2}})
        );
        let later = h.another("later");
        turn(&later, 1);
        assert_eq!(*handle.state_of(&later).unwrap(), 1);
        h.close().await;
    }
}
#[tokio::test]
async fn keeps_the_original_unit_when_the_same_key_is_registered_with_a_new_version() {
    let h = harness().await;
    let original = h.registry.register(&h.ctx, reader_unit()).await.unwrap();
    assert_eq!(*original.state_of(&h.session).unwrap(), 0);
    original.dispose().await;
    let mut unit = reader_unit();
    unit.state_version = 2;
    unit.apply = Arc::new(|state, _| Ok(Arc::new(**state + 10)));
    let replacement = h.registry.register(&h.ctx, unit).await.unwrap();
    turn(&h.session, 1);
    assert_eq!(*original.state_of(&h.session).unwrap(), 1);
    assert_eq!(*replacement.state_of(&h.session).unwrap(), 10);
    assert_eq!(
        values(h.registry.snapshot(&h.session, None).unwrap()),
        json!({"test/reader":10})
    );
    h.close().await;
}
#[tokio::test]
async fn reads_only_the_selected_unit_without_materializing_unrelated_registrations() {
    let h = harness().await;
    let initialized = Arc::new(AtomicUsize::new(0));
    let counted = initialized.clone();
    let mut unit = count_unit();
    unit.key = "test/unrelated-reader".into();
    unit.init = Arc::new(move |_, _| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(0))
    });
    h.registry.register(&h.ctx, unit).await.unwrap();
    let handle = h.registry.register(&h.ctx, reader_unit()).await.unwrap();
    assert_eq!(*handle.state_of(&h.session).unwrap(), 0);
    assert_eq!(initialized.load(Ordering::SeqCst), 0);
    h.close().await;
}
#[tokio::test]
async fn rejects_initialization_when_the_owner_unloads_before_the_handle_is_returned() {
    let h = harness().await;
    let (fiber, ctx) = owner(&h.ctx).await;
    let registration = h.registry.register(&ctx, reader_unit());
    fiber.dispose().await;
    assert!(registration.await.is_err());
    assert!(h.registry.checkpoint(&h.session).unwrap().is_empty());
    h.close().await;
}
#[tokio::test]
async fn releases_one_reference_once_under_twice_or_owner_unload_disposal() {
    for owner_unload in [false, true] {
        let h = harness().await;
        let (fiber, ctx) = owner(&h.ctx).await;
        let retained = h.registry.register(&h.ctx, reader_unit()).await.unwrap();
        let handle = h.registry.register(&ctx, reader_unit()).await.unwrap();
        if owner_unload {
            tokio::join!(fiber.dispose(), handle.dispose());
            handle.dispose().await;
        } else {
            handle.dispose().await;
            handle.dispose().await;
        }
        turn(&h.session, 1);
        assert_eq!(
            values(h.registry.snapshot(&h.session, None).unwrap()),
            json!({"test/reader":1})
        );
        assert_eq!(*handle.state_of(&h.session).unwrap(), 1);
        retained.dispose().await;
        assert!(
            h.registry
                .snapshot(&h.session, None)
                .unwrap()
                .values
                .is_empty()
        );
        h.close().await;
    }
}
#[tokio::test]
async fn a_registration_handed_off_while_the_owner_was_active_stays_fulfilled_after_unload() {
    // Awaiting completes the Rust future's handoff while the owner is active.
    let h = harness().await;
    let (fiber, ctx) = owner(&h.ctx).await;
    let handle = h.registry.register(&ctx, reader_unit()).await.unwrap();
    fiber.dispose().await;
    assert_eq!(*handle.state_of(&h.session).unwrap(), 0);
    h.close().await;
}
#[tokio::test]
async fn readers_accept_session_headers_with_integral_float_counters() {
    let h = harness().await;
    let handle = h.registry.register(&h.ctx, reader_unit()).await.unwrap();
    for header in [
        json!({"version":4,"id":"floaty","createdAt":1.0,"isSeeded":false}),
        json!({"version":4,"id":"floaty","createdAt":1.0,"isSeeded":false,"origin":"subagent","delegationDepth":2.0}),
    ] {
        let session =
            hoh_session::Session::create("floaty".into(), None, Some(header), None, None).unwrap();
        assert_eq!(*handle.state_of(&session).unwrap(), 0);
        assert_eq!(h.registry.checkpoint(&session).unwrap().len(), 1);
    }
    h.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fix2_unpolled_registration_cannot_handoff_on_another_worker() {
    let h = harness().await;
    let (fiber, ctx) = owner(&h.ctx).await;
    // This test's block_on future is not a spawned task. Drain setup first,
    // then let every task spawned by register finish on the other workers,
    // while keeping the caller's synchronous stack (no await) intact.
    tokio::task::yield_now().await;
    let metrics = tokio::runtime::Handle::current().metrics();
    let baseline = metrics.num_alive_tasks();
    let registration = h.registry.register(&ctx, reader_unit());
    while metrics.num_alive_tasks() > baseline {
        std::thread::yield_now();
    }
    fiber.dispose().await;
    assert!(
        registration.await.is_err(),
        "unload before polling must reject"
    );
    assert!(h.registry.checkpoint(&h.session).unwrap().is_empty());

    let (fiber, ctx) = owner(&h.ctx).await;
    let handle = h.registry.register(&ctx, reader_unit()).await.unwrap();
    fiber.dispose().await;
    assert_eq!(*handle.state_of(&h.session).unwrap(), 0);
    h.close().await;
}

#[tokio::test]
async fn fix2_restore_omits_proto_from_the_refreshed_checkpoint() {
    let h = harness().await;
    let mut definition = count_unit();
    definition.key = "__proto__".into();
    h.registry.register(&h.ctx, definition).await.unwrap();
    let restored = h
        .registry
        .restore(
            &rows(vec![("__proto__", row(1, -1, json!(0)))]),
            &[],
            offset(0),
            &header(),
            offset(0),
        )
        .unwrap();
    assert_eq!(json(&restored.checkpoint), json!({}));
    assert!(restored.snapshot.values.is_empty());
    h.close().await;
}
#[tokio::test]
async fn checkpoint_omits_the_proto_key_like_the_plain_object_source() {
    // PORTING §1: `rows["__proto__"] = row` sets the prototype in JS, so the row never serializes.
    let h = harness().await;
    let mut proto = count_unit();
    proto.key = "__proto__".into();
    h.registry.register(&h.ctx, proto).await.unwrap();
    assert_eq!(
        serde_json::to_value(h.registry.checkpoint(&h.session).unwrap()).unwrap(),
        json!({})
    );
    h.registry.register(&h.ctx, reader_unit()).await.unwrap();
    assert_eq!(
        serde_json::to_value(h.registry.checkpoint(&h.session).unwrap()).unwrap(),
        json!({"test/reader":{"ver":1,"seq":-1,"val":0}})
    );
    h.close().await;
}
