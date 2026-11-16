use super::*;
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
