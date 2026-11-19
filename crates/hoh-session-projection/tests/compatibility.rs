use crate::{test_support::*, *};
use futures_util::FutureExt;
use hoh_kernel::{Context, FiberState, Level, Levels, Undo};
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

fn definition() -> ProjectionDefinition<u64> {
    let mut unit = reader_unit();
    unit.key = "test/legacy".into();
    unit
}
fn invalid() -> ProjectionDefinition<u64> {
    let mut unit = definition();
    unit.state_version = hoh_session::MAX_SAFE_INTEGER + 1;
    unit
}
fn warnings(ctx: &Context) -> Vec<String> {
    ctx.logger()
        .buffer()
        .into_iter()
        .filter(|entry| entry.level == Level::Warn)
        .map(|entry| entry.message)
        .collect()
}
fn errors(ctx: &Context) -> Vec<String> {
    ctx.logger()
        .buffer()
        .into_iter()
        .filter(|entry| entry.level == Level::Error)
        .map(|entry| entry.message)
        .collect()
}
async fn settle() {
    for _ in 0..12 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn awaits_modern_registration_keeps_promise_chaining_and_never_warns_for_handle_cleanup() {
    let h = harness().await;
    let (fiber, ctx) = owner(&h.ctx).await;
    let settled = Arc::new(AtomicUsize::new(0));
    let mut registration = h.registry.register(&ctx, definition());
    let handle = (&mut registration).await.unwrap();
    let counted = settled.clone();
    let again = (&mut registration)
        .inspect(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
        })
        .await
        .unwrap();
    assert!(Arc::ptr_eq(
        &handle.state_of(&h.session).unwrap(),
        &again.state_of(&h.session).unwrap()
    ));
    assert_eq!(*handle.state_of(&h.session).unwrap(), 0);
    handle.dispose().await;
    turn(&h.session, 1);
    assert_eq!(*handle.state_of(&h.session).unwrap(), 1);
    assert_eq!(settled.load(Ordering::SeqCst), 1);
    assert!(
        h.registry
            .snapshot(&h.session, None)
            .unwrap()
            .values
            .is_empty()
    );
    fiber.dispose().await;
    assert!(warnings(&h.ctx).is_empty());
    h.close().await;
}
#[tokio::test]
async fn lets_an_asynchronous_plugin_await_registration_later_in_its_initialization() {
    let h = harness().await;
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let entered = Arc::new(Mutex::new(Some(entered_tx)));
    let release = Arc::new(Mutex::new(Some(release_rx)));
    let registry = h.registry.clone();
    let fiber = h
        .ctx
        .inject::<(), _>(move |ctx, ()| {
            let registry = registry.clone();
            let entered = lock(&entered).take().unwrap();
            let release = lock(&release).take().unwrap();
            async move {
                let registration = registry.register(&ctx, definition());
                entered.send(()).unwrap();
                release.await.unwrap();
                registration.await?;
                Ok(())
            }
        })
        .unwrap();
    entered_rx.await.unwrap();
    release_tx.send(()).unwrap();
    fiber.settled().await.unwrap();
    assert!(warnings(&h.ctx).is_empty());
    h.close().await;
}
#[tokio::test]
async fn loads_an_unawaited_legacy_registration_and_warns_once_per_key_for_keyed_reads() {
    let h = harness().await;
    let (first, one) = owner(&h.ctx).await;
    let (second, two) = owner(&h.ctx).await;
    drop(h.registry.register(&one, definition()));
    drop(h.registry.register(&two, definition()));
    turn(&h.session, 1);
    assert_eq!(
        *h.registry
            .state_of::<u64>(&h.ctx, &h.session, "test/legacy")
            .unwrap()
            .unwrap(),
        1
    );
    assert_eq!(
        *h.registry
            .state_of::<u64>(&h.ctx, &h.session, "test/legacy")
            .unwrap()
            .unwrap(),
        1
    );
    assert!(
        h.registry
            .state_of::<u64>(&h.ctx, &h.session, "test/missing-legacy")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        warnings(&h.ctx),
        vec![
            "sessionProjections.stateOf(session, \"test/legacy\") is deprecated; read handle.stateOf(session) from an awaited registration, or use the owning domain service",
            "sessionProjections.stateOf(session, \"test/missing-legacy\") is deprecated; read handle.stateOf(session) from an awaited registration, or use the owning domain service"
        ]
    );
    first.dispose().await;
    assert_eq!(
        *h.registry
            .state_of::<u64>(&h.ctx, &h.session, "test/legacy")
            .unwrap()
            .unwrap(),
        1
    );
    second.dispose().await;
    assert!(
        h.registry
            .state_of::<u64>(&h.ctx, &h.session, "test/legacy")
            .unwrap()
            .is_none()
    );
    assert_eq!(warnings(&h.ctx).len(), 2);
    h.close().await;
}
#[tokio::test]
async fn materializes_every_registered_unit_on_a_legacy_keyed_read_without_producing_unrelated_wire_views()
 {
    let h = harness().await;
    turn(&h.session, 1);
    h.session
        .append(
            "turn/end",
            json!({"turn":1,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
    h.registry.register(&h.ctx, definition()).await.unwrap();
    let views = Arc::new(AtomicUsize::new(0));
    let counted = views.clone();
    let mut wire = definition();
    wire.key = "test/legacy-unread-wire".into();
    wire.wire.as_mut().unwrap().view = Arc::new(move |state| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(Some(Arc::new(json!(**state))))
    });
    h.registry.register(&h.ctx, wire).await.unwrap();
    let inits = Arc::new(AtomicUsize::new(0));
    let counted = inits.clone();
    let mut host = count_unit();
    host.key = "test/legacy-unread-host".into();
    host.init = Arc::new(move |_, _| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(0))
    });
    h.registry.register(&h.ctx, host).await.unwrap();
    assert!(
        h.registry
            .state_of::<u64>(&h.ctx, &h.session, "test/missing-legacy")
            .unwrap()
            .is_none()
    );
    assert!(
        h.registry
            .cached_snapshot(&h.session, None)
            .unwrap()
            .is_none()
    );
    assert_eq!(inits.load(Ordering::SeqCst), 0);
    assert_eq!(
        *h.registry
            .state_of::<u64>(&h.ctx, &h.session, "test/legacy")
            .unwrap()
            .unwrap(),
        2
    );
    assert_eq!(views.load(Ordering::SeqCst), 0);
    assert_eq!(inits.load(Ordering::SeqCst), 1);
    assert_eq!(
        json(
            &h.registry
                .cached_snapshot(&h.session, None)
                .unwrap()
                .unwrap()
        ),
        json!({"asOfSeq":1,"values":{"test/legacy":2,"test/legacy-unread-wire":2}})
    );
    h.close().await;
}
#[tokio::test]
async fn keeps_callable_early_disposal_idempotent_without_invalidating_an_awaited_reader() {
    let h = harness().await;
    let mut registration = h.registry.register(&h.ctx, definition());
    let handle = (&mut registration).await.unwrap();
    registration.dispose().await;
    registration.dispose().await;
    handle.dispose().await;
    turn(&h.session, 1);
    assert_eq!(*handle.state_of(&h.session).unwrap(), 1);
    assert!(
        h.registry
            .snapshot(&h.session, None)
            .unwrap()
            .values
            .is_empty()
    );
    assert!(warnings(&h.ctx).is_empty());
    h.close().await;
}
#[tokio::test]
async fn releases_a_registration_called_before_the_asynchronous_handle_handoff() {
    let h = harness().await;
    let registration = h.registry.register(&h.ctx, definition());
    registration.dispose().await;
    let handle = registration.await.unwrap();
    assert_eq!(*handle.state_of(&h.session).unwrap(), 0);
    assert!(
        h.registry
            .snapshot(&h.session, None)
            .unwrap()
            .values
            .is_empty()
    );
    assert!(warnings(&h.ctx).is_empty());
    h.close().await;
}
#[tokio::test]
async fn unloads_a_legacy_return_effect_or_generator_registration() {
    for style in ["return", "effect", "generator"] {
        let h = harness().await;
        let (fiber, ctx) = owner(&h.ctx).await;
        match style {
            "return" => {
                let registration = h.registry.register(&ctx, definition());
                ctx.effect("legacy return", move || registration.into_undo())
                    .unwrap();
            }
            "effect" => {
                ctx.effect("legacy effect", || {
                    h.registry.register(&ctx, definition()).into_undo()
                })
                .unwrap();
            }
            _ => {
                ctx.try_effect::<hoh_error::BoxError>("legacy generator", |undos| {
                    undos.push(h.registry.register(&ctx, definition()).into_undo())?;
                    Ok(())
                })
                .unwrap();
            }
        }
        assert_eq!(
            values(h.registry.snapshot(&h.session, None).unwrap()),
            json!({"test/legacy":0})
        );
        fiber.dispose().await;
        assert!(
            h.registry
                .snapshot(&h.session, None)
                .unwrap()
                .values
                .is_empty()
        );
        assert!(warnings(&h.ctx).is_empty());
        h.close().await;
    }
}
#[tokio::test]
async fn transfers_the_original_disposer_into_a_composed_effect_and_retains_it_until_earlier_cleanup_settles()
 {
    let h = harness().await;
    let (fiber, ctx) = owner(&h.ctx).await;
    let (closing_tx, closing_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    ctx.try_effect::<hoh_error::BoxError>("legacy composed", |undos| {
        undos.push(h.registry.register(&ctx, definition()).into_undo())?;
        undos.push(Undo::future(move || async move {
            closing_tx.send(()).unwrap();
            release_rx.await.unwrap();
            Ok(())
        }))?;
        Ok(())
    })
    .unwrap();
    let disposed = fiber.dispose();
    tokio::pin!(disposed);
    tokio::select! { _=&mut disposed=>panic!("cleanup must wait"), _=closing_rx=>{} }
    turn(&h.session, 1);
    assert_eq!(
        values(h.registry.snapshot(&h.session, None).unwrap()),
        json!({"test/legacy":1})
    );
    release_tx.send(()).unwrap();
    disposed.await;
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
async fn preserves_explicit_rejection_handling_and_returns_a_harmless_legacy_disposer_for_failed_acquisition()
 {
    let h = harness().await;
    let mut registration = h.registry.register(&h.ctx, invalid());
    assert!((&mut registration).await.is_err());
    registration.dispose().await;
    settle().await;
    assert!(
        h.registry
            .snapshot(&h.session, None)
            .unwrap()
            .values
            .is_empty()
    );
    assert!(errors(&h.ctx).is_empty());
    assert!(warnings(&h.ctx).is_empty());
    h.close().await;
}
#[tokio::test(start_paused = true)]
async fn logs_an_ignored_legacy_registration_rejection_and_keeps_the_plugin_active() {
    let h = harness().await;
    let (fiber, ctx) = owner(&h.ctx).await;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let reported = Arc::new(Mutex::new(Some(tx)));
    h.ctx
        .logger()
        .exporter(
            Levels {
                default: Some(Level::Error),
                ..Default::default()
            },
            move |record| {
                if let Some(tx) = lock(&reported).take() {
                    let _ = tx.send(record.message.clone());
                }
            },
        )
        .unwrap();
    drop(h.registry.register(&ctx, definition()));
    let mut conflict = definition();
    conflict.state_version = 2;
    drop(h.registry.register(&ctx, conflict));
    let message = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
        .await
        .unwrap()
        .unwrap();
    assert!(message.contains("already registered at stateVersion 1"));
    assert_eq!(errors(&h.ctx).len(), 1);
    assert_eq!(fiber.state(), FiberState::Active);
    assert_eq!(
        values(h.registry.snapshot(&h.session, None).unwrap()),
        json!({"test/legacy":0})
    );
    h.close().await;
}
#[tokio::test]
async fn lets_a_modern_plugin_handle_rejected_registration_and_finish_activation() {
    let h = harness().await;
    let (fiber, ctx) = owner(&h.ctx).await;
    assert!(h.registry.register(&ctx, invalid()).await.is_err());
    h.registry.register(&ctx, definition()).await.unwrap();
    assert_eq!(fiber.state(), FiberState::Active);
    assert_eq!(
        values(h.registry.snapshot(&h.session, None).unwrap()),
        json!({"test/legacy":0})
    );
    settle().await;
    assert!(warnings(&h.ctx).is_empty());
    assert!(errors(&h.ctx).is_empty());
    h.close().await;
}
#[tokio::test]
async fn warns_again_for_a_new_registry_instance_without_sharing_warning_state_between_applications()
 {
    let h = harness().await;
    h.registry.register(&h.ctx, definition()).await.unwrap();
    h.registry
        .state_of::<u64>(&h.ctx, &h.session, "test/legacy")
        .unwrap();
    h.registry
        .state_of::<u64>(&h.ctx, &h.session, "test/legacy")
        .unwrap();
    assert_eq!(warnings(&h.ctx).len(), 1);
    let second = hoh_kernel::Kernel::start();
    let ctx = second.root();
    let registry = SessionProjectionRegistry::new(&ctx).unwrap();
    assert!(
        registry
            .state_of::<u64>(&ctx, &h.session, "test/legacy")
            .unwrap()
            .is_none()
    );
    assert_eq!(warnings(&ctx).len(), 1);
    assert_eq!(warnings(&h.ctx).len(), 1);
    second.shutdown().await;
    h.close().await;
}
#[tokio::test]
async fn drops_an_ignored_rejection_when_its_plugin_unloads_before_handoff() {
    let h = harness().await;
    let (fiber, ctx) = owner(&h.ctx).await;
    drop(h.registry.register(&ctx, definition()));
    fiber.dispose().await;
    settle().await;
    assert_eq!(fiber.state(), FiberState::Disposed);
    assert!(
        h.registry
            .snapshot(&h.session, None)
            .unwrap()
            .values
            .is_empty()
    );
    assert!(errors(&h.ctx).is_empty());
    assert!(warnings(&h.ctx).is_empty());
    h.close().await;
}
#[tokio::test]
async fn keeps_an_observed_registration_owned_when_its_plugin_unloads_before_handoff() {
    let h = harness().await;
    let (fiber, ctx) = owner(&h.ctx).await;
    let registration = h.registry.register(&ctx, definition());
    fiber.dispose().await;
    assert_eq!(
        registration.await.err().unwrap().to_string(),
        "inactive context"
    );
    assert!(
        h.registry
            .snapshot(&h.session, None)
            .unwrap()
            .values
            .is_empty()
    );
    assert!(warnings(&h.ctx).is_empty());
    h.close().await;
}
