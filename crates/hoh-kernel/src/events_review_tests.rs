use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
struct Ping;
event!(Ping = "review/ping", Emit);
#[tokio::test(start_paused = true)]
async fn e2_empty_buckets_are_removed() {
    let k = Kernel::start();
    let ctx = k.root();
    let first = ctx.on::<Ping>(|_| {}).unwrap();
    let second = ctx.on::<Ping>(|_| {}).unwrap();
    first.dispose().await;
    assert!(read(&ctx.inner.bus.listeners).contains_key(&TypeId::of::<Ping>()));
    second.dispose().await;
    assert!(!read(&ctx.inner.bus.listeners).contains_key(&TypeId::of::<Ping>()));
    k.shutdown().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c12_prepend_is_one_atomic_registration() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let k = Kernel::start();
        let ctx = k.root();
        let anchor = ctx
            .on_update::<NoConfig, _>(|_, _| async { Ok(()) })
            .unwrap();
        let done = Arc::new(AtomicBool::new(false));
        let stop = done.clone();
        let writer_ctx = ctx.clone();
        let writer = std::thread::spawn(move || {
            for _ in 0..30000 {
                let d = writer_ctx
                    .listen()
                    .prepend()
                    .on_update::<NoConfig, _>(|_, _| async { Ok(()) })
                    .unwrap();
                drop(d.dispose());
            }
            stop.store(true, Ordering::Release);
        });
        let mut wrong = false;
        while !done.load(Ordering::Acquire) {
            let hooks = read(&ctx.data.hooks);
            if let Some(last) = hooks.updates.last() {
                wrong |= last
                    .effect
                    .upgrade()
                    .is_some_and(|e| !Arc::ptr_eq(&e, &anchor.effect));
            }
            drop(hooks);
            std::thread::yield_now();
        }
        writer.join().unwrap();
        assert!(
            !wrong,
            "snapshot observed the prepended handler appended after the anchor"
        );
        k.shutdown().await;
    })
    .await
    .unwrap();
}
struct Fall;
event!(Fall = "review/forward", Waterfall<u32>);
#[test]
fn c14_forwarding_uses_default_stack() {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "events::review_tests::c14_stack_probe",
            "--ignored",
            "--nocapture",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
#[test]
#[ignore = "isolated subprocess: catches a stack overflow without aborting the test runner"]
fn c14_stack_probe() {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let k = Kernel::start();
                    for _ in 0..2000 {
                        k.root()
                            .on_waterfall::<Fall, u32, _>(|_, next| async move { next.run().await })
                            .unwrap();
                    }
                    assert_eq!(
                        k.root().waterfall(Fall, |_| async { Ok(7) }).await.unwrap(),
                        7
                    );
                    let f = k
                        .root()
                        .inject(|ctx, (): ()| async move {
                            for _ in 0..2000 {
                                ctx.on_update::<NoConfig, _>(
                                    |_, next| async move { next.run().await },
                                )?;
                            }
                            Ok(())
                        })
                        .unwrap();
                    f.settled().await.unwrap();
                    f.update(serde_json::json!({})).await.unwrap();
                    assert_eq!(f.state(), FiberState::Active);
                    k.shutdown().await;
                });
        })
        .unwrap()
        .join()
        .unwrap();
}
