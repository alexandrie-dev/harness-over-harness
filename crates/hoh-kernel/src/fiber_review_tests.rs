use super::*;
#[tokio::test(start_paused = true)]
async fn ready_replies_survive_shutdown() {
    let k = Kernel::start();
    let f = k
        .root()
        .inject(|ctx, (): ()| async move {
            ctx.on_update::<NoConfig, _>(|_, _| async { Ok(()) })?;
            Ok(())
        })
        .unwrap();
    f.settled().await.unwrap();
    let settled = f.settled();
    k.snapshot().await;
    let update = f.update(serde_json::json!({"revision":1}));
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    assert_eq!(f.effective_config().unwrap()["revision"], 1);
    let restart = f.restart_from_update();
    f.settled().await.unwrap();
    k.shutdown().await;
    let replies = [settled.await, update.await, restart.await];
    assert!(replies.iter().all(Result::is_ok), "{replies:?}");
}
#[tokio::test(start_paused = true)]
async fn type_release_skips_unrelated_commands() {
    async fn scans(k: &Kernel) -> usize {
        let (tx, rx) = oneshot::channel();
        k.inner.tx.send(Command::TypeScans(tx)).unwrap();
        rx.await.unwrap()
    }
    let k = Kernel::start();
    service!(Key:u32="type-scan");
    let binding = k.root().provide::<Key>(Arc::new(1)).await.unwrap();
    let before = scans(&k).await;
    for _ in 0..10 {
        k.snapshot().await;
    }
    assert_eq!(
        scans(&k).await,
        before,
        "read-only commands scanned every service type"
    );
    binding.dispose().await;
    assert_eq!(scans(&k).await, before + 1);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn c6_actor_failure_cancels_activations_and_update_waiters() {
    let k = Kernel::start();
    let slot = Arc::new(Mutex::new(None::<Context>));
    let s = slot.clone();
    let f = k
        .root()
        .inject(move |ctx, (): ()| {
            let s = s.clone();
            async move {
                *lock(&s) = Some(ctx);
                Ok(())
            }
        })
        .unwrap();
    f.settled().await.unwrap();
    let ctx = lock(&slot).clone().unwrap();
    let token = ctx.token();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let g = gate.clone();
    ctx.on_update::<NoConfig, _>(move |_, _| {
        let g = g.clone();
        async move {
            let _ = g.acquire().await;
            Ok(())
        }
    })
    .unwrap();
    let update = f.update(serde_json::json!({}));
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    k.inner.tx.send(Command::Fault).ok();
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), update).await;
    assert!(matches!(result, Ok(Err(KernelError::Closed))));
    assert!(token.is_cancelled());
    gate.add_permits(1);
}
