use super::*;

tokio::task_local! { static CALLER: String; }

#[tokio::test]
async fn disposing_a_fiber_that_is_already_gone_leaves_no_capture_record() {
    let kernel = Kernel::start();
    let root = kernel.root();
    let gone = root.child("gone").unwrap().fiber();
    gone.dispose().await;
    // The actor drops the node once its disposal settles.
    kernel.snapshot().await;

    // A teardown that carries a captured value disposes the fiber that no longer exists.
    let holder = root.child("holder").unwrap();
    holder
        .capture_on_dispose::<String>(|| CALLER.try_with(|name| Arc::new(name.clone())).ok())
        .unwrap();
    let target = gone.clone();
    holder
        .effect("dispose-the-gone", move || {
            Undo::future(move || async move {
                for _ in 0..3 {
                    target.dispose().await;
                }
                Ok(())
            })
        })
        .unwrap();
    let holder = holder.fiber();
    CALLER
        .scope("x".to_owned(), async { holder.dispose().await })
        .await;
    kernel.snapshot().await;
    // Only the root's set may remain: neither the holder's nor the gone fiber's.
    assert!(kernel.capture_set_count().await <= 1);
    kernel.shutdown().await;
}
