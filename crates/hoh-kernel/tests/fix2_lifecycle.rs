mod support;
use hoh_kernel::*;
use serde_json::json;
use std::sync::{Arc, Mutex};
use support::*;

#[tokio::test(start_paused = true)]
async fn hidden_registration_activation_withdrawal_sequence() {
    let (k, r, t) = harness();
    let seen = Arc::new(Mutex::new(vec![]));
    let s = seen.clone();
    r.on::<InternalService>(move |e| {
        if e.name.as_ref() == "foo" {
            lock(&s).push(e.available);
        }
    })
    .unwrap();
    let f = r
        .inject(|ctx, (): ()| async move {
            ctx.provide::<A>(Arc::new(1)).await?;
            let t = ctx.tag::<Arc<Track>>().unwrap();
            t.gate.acquire().await.unwrap().forget();
            Ok(())
        })
        .unwrap();
    turn().await;
    assert_eq!(f.state(), FiberState::Loading);
    assert!(lock(&seen).is_empty());
    t.gate.add_permits(1);
    f.settled().await.unwrap();
    assert_eq!(*lock(&seen), [true]);
    f.dispose().await;
    assert_eq!(*lock(&seen), [true, false]);
    k.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn settled_active_fiber_allows_marked_serial_wait() {
    let (k, r, t) = harness();
    let a = r.plugin::<Recorded>(json!({})).unwrap();
    a.settled().await.unwrap();
    let owner = a.clone();
    let b = r
        .inject(move |ctx, (): ()| {
            let a = owner.clone();
            async move {
                ctx.on_serial::<Ser, i32, _>(move |_| {
                    let a = a.clone();
                    async move {
                        a.settled().await?;
                        Ok(None)
                    }
                })?;
                Ok(())
            }
        })
        .unwrap();
    b.settled().await.unwrap();
    let (out, received) = tokio::sync::oneshot::channel();
    t.context()
        .spawn("serial", move |_| async move {
            let result = t.context().serial(Ser).await;
            out.send(result).ok();
            Ok(())
        })
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), received)
            .await
            .unwrap()
            .unwrap()
            .is_ok()
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn loading_spawn_wait_still_reports_self_wait() {
    let (k, r, t) = harness();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let out = Arc::new(Mutex::new(Some(tx)));
    let f = r
        .inject(move |ctx, (): ()| {
            let out = out.clone();
            async move {
                let own = ctx.fiber();
                let sender = lock(&out).take().unwrap();
                ctx.spawn("self-wait", move |_| async move {
                    sender.send(own.settled().await).ok();
                    Ok(())
                })?;
                let t = ctx.tag::<Arc<Track>>().unwrap();
                t.gate.acquire().await.unwrap().forget();
                Ok(())
            }
        })
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(1), rx)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(KernelError::SelfWait)));
    assert_eq!(f.state(), FiberState::Loading);
    t.gate.add_permits(1);
    f.settled().await.unwrap();
    k.shutdown().await;
}
struct Forward;
event!(Forward = "fix2/forward", Waterfall<u32>);
#[tokio::test(start_paused = true)]
async fn chain_steps_continue_after_caller_drop() {
    use futures_util::FutureExt;
    let (k, r, t) = harness();
    let gate = t.clone();
    r.on_waterfall::<Forward, u32, _>(move |_, next| {
        let t = gate.clone();
        async move {
            t.gate.acquire().await.unwrap().forget();
            let value = next.run().await?;
            t.push("event continued");
            Ok(value)
        }
    })
    .unwrap();
    assert!(
        r.waterfall(Forward, |_| async { Ok(7) })
            .now_or_never()
            .is_none()
    );
    turn().await;
    assert!(t.list().is_empty());
    t.gate.add_permits(1);
    turn().await;
    assert_eq!(t.list(), ["event continued"]);
    let f = r.plugin::<Recorded>(json!({})).unwrap();
    f.settled().await.unwrap();
    let gate = t.clone();
    t.context()
        .on_update::<serde_json::Value, _>(move |_, _| {
            let t = gate.clone();
            async move {
                t.gate.acquire().await.unwrap().forget();
                t.push("update continued");
                Ok(())
            }
        })
        .unwrap();
    drop(f.update(json!({"value":1})));
    turn().await;
    assert!(!t.list().contains(&"update continued".into()));
    t.gate.add_permits(1);
    turn().await;
    assert_eq!(t.list().last().unwrap(), "update continued");
    assert_eq!(f.effective_config().unwrap()["value"], 1);
    k.shutdown().await;
}
