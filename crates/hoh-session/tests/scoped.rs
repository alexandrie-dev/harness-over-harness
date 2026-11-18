mod common;
use common::*;
use hoh_scope::{CreateScopeOptions, ScopeKey, create_scope};
use hoh_session::*;
use serde_json::json;
use std::sync::{Arc, Mutex};
#[tokio::test(start_paused = true)]
async fn a_session_entered_through_a_scoped_context_dispatches_without_a_receiver_filter() {
    let (k, ctx, store) = mounted().await;
    let a = create_scope(&ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    let b = create_scope(&ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    let heard = Arc::new(Mutex::new(vec![]));
    for (name, c) in [
        ("global", ctx.clone()),
        ("owner", a.ctx.clone()),
        ("other", b.ctx.clone()),
    ] {
        let out = heard.clone();
        c.on_contained::<SessionEventEmitted>(move |_| {
            lock(&out).push(name);
            Ok(())
        })
        .unwrap();
    }
    let s = store.create(&a.ctx, None, Default::default()).unwrap();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    assert_eq!(*lock(&heard), vec!["global", "owner", "other"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn a_bare_session_dispatches_without_a_receiver_filter() {
    let (k, ctx, store) = mounted().await;
    let scoped = ctx.with_filter(|_| false);
    let n = Arc::new(Mutex::new(0));
    let out = n.clone();
    scoped
        .on_contained::<SessionEventEmitted>(move |_| {
            *lock(&out) += 1;
            Ok(())
        })
        .unwrap();
    let s = store.create(&scoped, None, Default::default()).unwrap();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    assert_eq!(*lock(&n), 1);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn reuses_the_enter_context_for_the_paired_disposal_notification() {
    let (k, ctx, store) = mounted().await;
    let owner = ctx.child("owner").unwrap();
    let heard = Arc::new(Mutex::new(vec![]));
    let out = heard.clone();
    owner
        .on_contained::<SessionDisposed>(move |e| {
            lock(&out).push(e.session.id().clone());
            Ok(())
        })
        .unwrap();
    let s = store.prepare(None, Default::default()).unwrap();
    let detach = store.enter(&owner, s.clone()).unwrap();
    store.announce(&s).unwrap();
    detach.detach();
    assert_eq!(*lock(&heard), vec![s.id().clone()]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn allows_an_ordinary_flush_with_no_listeners() {
    let (k, ctx, store) = mounted().await;
    let s = store.create(&ctx, None, Default::default()).unwrap();
    assert!(!store.flush(&s).await.unwrap());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn reports_a_participating_listener_after_it_succeeds() {
    let (k, ctx, store) = mounted().await;
    let calls = Arc::new(Mutex::new(vec![]));
    let out = calls.clone();
    SessionStore::on_flush(&ctx, move |s| {
        let out = out.clone();
        async move {
            lock(&out).push(s);
            Ok(())
        }
    })
    .unwrap();
    let s = store.create(&ctx, None, Default::default()).unwrap();
    assert!(store.flush(&s).await.unwrap());
    assert_eq!(*lock(&calls), vec![s]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn dispatches_session_flush_without_a_receiver_filter_and_awaits_all_listeners() {
    let (k, ctx, store) = mounted().await;
    let calls = Arc::new(Mutex::new(vec![]));
    for name in ["global", "owner"] {
        let out = calls.clone();
        SessionStore::on_flush(&ctx.with_filter(|_| false), move |_| {
            let out = out.clone();
            async move {
                tokio::task::yield_now().await;
                lock(&out).push(name);
                Ok(())
            }
        })
        .unwrap();
    }
    let s = store.create(&ctx, None, Default::default()).unwrap();
    assert!(store.flush(&s).await.unwrap());
    let mut heard = lock(&calls).clone();
    heard.sort();
    assert_eq!(heard, vec!["global", "owner"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn propagates_a_rejecting_flush_listener() {
    let (k, ctx, store) = mounted().await;
    SessionStore::on_flush(&ctx, |_| async { Err("disk full".into()) }).unwrap();
    let s = store.create(&ctx, None, Default::default()).unwrap();
    error(store.flush(&s).await, "disk full");
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn does_not_let_a_synchronous_flush_failure_starve_later_listeners() {
    let (k, ctx, store) = mounted().await;
    ctx.on_contained::<SessionFlush>(|_| Err("disk full".into()))
        .unwrap();
    let calls = Arc::new(Mutex::new(0));
    let out = calls.clone();
    SessionStore::on_flush(&ctx, move |_| {
        let out = out.clone();
        async move {
            *lock(&out) += 1;
            Ok(())
        }
    })
    .unwrap();
    let s = store.create(&ctx, None, Default::default()).unwrap();
    error(store.flush(&s).await, "disk full");
    assert_eq!(*lock(&calls), 1);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn waits_for_slower_flush_listeners_before_reporting_another_listener_failure() {
    let (k, ctx, store) = mounted().await;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let rx = Arc::new(Mutex::new(Some(rx)));
    SessionStore::on_flush(&ctx, |_| async { Err("disk full".into()) }).unwrap();
    let pending = rx.clone();
    let started = Arc::new(Mutex::new(false));
    let flag = started.clone();
    SessionStore::on_flush(&ctx, move |_| {
        let rx = lock(&pending).take().unwrap();
        let flag = flag.clone();
        async move {
            *lock(&flag) = true;
            rx.await.unwrap();
            Ok(())
        }
    })
    .unwrap();
    let s = store.create(&ctx, None, Default::default()).unwrap();
    let future = store.flush(&s);
    tokio::pin!(future);
    tokio::select! {biased;result=&mut future=>panic!("settled before gate: {result:?}"),_=tokio::task::yield_now()=>{}}
    assert!(*lock(&started));
    tx.send(()).unwrap();
    error(future.await, "disk full");
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rejects_a_never_entered_session_instead_of_inventing_a_carrier() {
    let (k, _ctx, store) = mounted().await;
    let s = store.prepare(None, Default::default()).unwrap();
    error(store.flush(&s).await, "not live");
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn clears_a_detached_carrier_and_rejects_stale_flushes() {
    let (k, ctx, store) = mounted().await;
    let s = store.prepare(None, Default::default()).unwrap();
    let d = store.enter(&ctx, s.clone()).unwrap();
    assert!(!store.flush(&s).await.unwrap());
    d.detach();
    error(store.flush(&s).await, "not live");
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn distinct_scopes_carry_distinct_keys() {
    let (k, ctx, _) = mounted().await;
    let a = create_scope(&ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    let b = create_scope(&ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    assert_ne!(hoh_scope::scope_of(&a.ctx), hoh_scope::scope_of(&b.ctx));
    k.shutdown().await;
}
