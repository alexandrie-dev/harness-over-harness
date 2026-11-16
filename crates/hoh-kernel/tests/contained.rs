use hoh_kernel::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap()
}
async fn turn() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}
struct Notice(u32);
event!(Notice = "contained/notice", Contained);
#[tokio::test(start_paused = true)]
async fn deprecated_dispatch_returns_bound_callbacks_that_respect_later_removal() {
    // DSH freezes bound callbacks (vendor/events.ts:170-184); upstream's newer
    // removed-hook guard is absent there. Keep that difference explicit.
    let k = Kernel::start();
    let values = Arc::new(Mutex::new(vec![]));
    let out = values.clone();
    let disposer = k
        .root()
        .on_contained::<Notice>(move |e| {
            lock(&out).push(e.0);
            Ok(())
        })
        .unwrap();
    let first = k.root().dispatch(Notice(1));
    let second = k.root().dispatch(Notice(2));
    first.invoke(|e| panic!("unexpected failure: {e:?}"));
    disposer.dispose().await;
    second.invoke(|e| panic!("unexpected failure: {e:?}"));
    assert_eq!(*lock(&values), [1, 2]);
    assert!(k.root().dispatch(Notice(3)).is_empty());
    k.shutdown().await;
}
struct LegacyFall;
event!(LegacyFall = "legacy/waterfall", Waterfall<u32>);
#[tokio::test(start_paused = true)]
async fn deprecated_waterfall_dispatch_continues_past_a_removed_callback() {
    // A raw waterfall callback is deliberately unavailable for Contained. The
    // public waterfall takes its snapshot synchronously, before its task runs.
    let k = Kernel::start();
    let calls = Arc::new(AtomicBool::new(false));
    let called = calls.clone();
    let disposer = k
        .root()
        .on_waterfall::<LegacyFall, u32, _>(move |_, next| {
            called.store(true, Ordering::SeqCst);
            async move { next.run().await }
        })
        .unwrap();
    let next = Arc::new(AtomicBool::new(false));
    let tail = next.clone();
    let pending = k.root().waterfall(LegacyFall, move |_| async move {
        tail.store(true, Ordering::SeqCst);
        Ok(7)
    });
    disposer.dispose().await;
    assert_eq!(pending.await.unwrap(), 7);
    assert!(!calls.load(Ordering::SeqCst));
    assert!(next.load(Ordering::SeqCst));
    k.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn reports_threw_before_next_call_and_rejected_from_async_task() {
    let k = Kernel::start();
    let ctx = k.root();
    let log = Arc::new(Mutex::new(vec![]));
    let s = log.clone();
    ctx.on_contained::<Notice>(move |e| {
        assert_eq!(e.0, 7);
        lock(&s).push("sync call".to_owned());
        Err("sync error".into())
    })
    .unwrap();
    let s = log.clone();
    ctx.on_contained_async::<Notice, _>(move |_| {
        lock(&s).push("async call".into());
        async { Err("async error".into()) }
    })
    .unwrap();
    let s = log.clone();
    ctx.on_contained::<Notice>(move |_| {
        lock(&s).push("success".into());
        Ok(())
    })
    .unwrap();
    let snapshot = ctx.dispatch(Notice(7));
    assert_eq!(snapshot.len(), 3);
    assert!(!snapshot.is_empty());
    assert!(lock(&log).is_empty());
    let s = log.clone();
    snapshot.invoke(move |failure| {
        lock(&s).push(match failure {
            ListenerFailure::Threw(e) => format!("threw: {e}"),
            ListenerFailure::Rejected(e) => format!("rejected: {e}"),
        })
    });
    assert_eq!(
        *lock(&log),
        ["sync call", "threw: sync error", "async call", "success"]
    );
    turn().await;
    assert_eq!(lock(&log).last().unwrap(), "rejected: async error");
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn synchronous_panics_are_contained_and_do_not_starve_other_listeners() {
    let k = Kernel::start();
    let calls = Arc::new(Mutex::new(vec![]));
    k.root()
        .on_contained::<Notice>(|_| panic!("listener panic"))
        .unwrap();
    let c = calls.clone();
    k.root()
        .on_contained::<Notice>(move |_| {
            lock(&c).push("success");
            Ok(())
        })
        .unwrap();
    let c = calls.clone();
    k.root().dispatch(Notice(0)).invoke(move |failure| {
        assert!(matches!(failure, ListenerFailure::Threw(_)));
        lock(&c).push("threw");
    });
    assert_eq!(*lock(&calls), ["threw", "success"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn asynchronous_panics_are_rejections() {
    let k = Kernel::start();
    let failures = Arc::new(Mutex::new(vec![]));
    k.root()
        .on_contained_async::<Notice, _>(|_| async {
            panic!("async panic");
            #[allow(unreachable_code)]
            Ok(())
        })
        .unwrap();
    let f = failures.clone();
    k.root()
        .dispatch(Notice(0))
        .invoke(move |failure| lock(&f).push(matches!(failure, ListenerFailure::Rejected(_))));
    turn().await;
    assert_eq!(*lock(&failures), [true]);
    k.shutdown().await;
}
#[derive(Clone)]
struct Scope(u32);
#[tokio::test(start_paused = true)]
async fn dispatch_observes_filters_prepend_global_tags_and_snapshot_timing() {
    let k = Kernel::start();
    let ctx = k.root();
    let calls = Arc::new(Mutex::new(vec![]));
    let observed = Arc::new(Mutex::new(vec![]));
    let o = observed.clone();
    ctx.on_dispatch(move |mode, name, event| {
        if name == Notice::NAME {
            assert!(event.downcast_ref::<Notice>().is_some());
            lock(&o).push(mode);
        }
    })
    .unwrap();
    for (tag, name) in [(1, "first"), (2, "filtered")] {
        let c = calls.clone();
        ctx.with_tag(Scope(tag))
            .on_contained::<Notice>(move |_| {
                lock(&c).push(name);
                Ok(())
            })
            .unwrap();
    }
    let c = calls.clone();
    ctx.with_tag(Scope(1))
        .listen()
        .prepend()
        .on_contained::<Notice>(move |_| {
            lock(&c).push("prepend");
            Ok(())
        })
        .unwrap();
    let c = calls.clone();
    ctx.with_tag(Scope(2))
        .listen()
        .global()
        .on_contained::<Notice>(move |_| {
            lock(&c).push("global");
            Ok(())
        })
        .unwrap();
    let target = ctx.with_filter(|listener| listener.tag::<Scope>().is_some_and(|tag| tag.0 == 1));
    let snapshot = target.dispatch(Notice(0));
    assert_eq!(*lock(&observed), [Dispatch::Emit]);
    assert_eq!(snapshot.len(), 3);
    assert!(lock(&calls).is_empty());
    let c = calls.clone();
    ctx.with_tag(Scope(1))
        .on_contained::<Notice>(move |_| {
            lock(&c).push("late");
            Ok(())
        })
        .unwrap();
    snapshot.invoke(|e| panic!("unexpected failure: {e:?}"));
    assert_eq!(*lock(&calls), ["prepend", "first", "global"]);
    target
        .dispatch(Notice(0))
        .invoke(|e| panic!("unexpected failure: {e:?}"));
    assert_eq!(lock(&calls).last(), Some(&"late"));
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn disposed_listener_is_not_in_a_new_snapshot() {
    let k = Kernel::start();
    let d = k.root().on_contained::<Notice>(|_| Ok(())).unwrap();
    d.dispose().await;
    assert!(k.root().dispatch(Notice(0)).is_empty());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn asynchronous_listener_is_cancelled_with_its_fiber_without_reporting() {
    struct Guard(Arc<AtomicBool>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let k = Kernel::start();
    let started = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let a = started.clone();
    let b = dropped.clone();
    let g = gate.clone();
    let f = k
        .root()
        .inject(move |ctx, (): ()| {
            let a = a.clone();
            let b = b.clone();
            let g = g.clone();
            async move {
                ctx.on_contained_async::<Notice, _>(move |_| {
                    let a = a.clone();
                    let b = b.clone();
                    let g = g.clone();
                    async move {
                        let _guard = Guard(b);
                        a.store(true, Ordering::SeqCst);
                        g.acquire().await.unwrap().forget();
                        Err("late error".into())
                    }
                })?;
                Ok(())
            }
        })
        .unwrap();
    f.settled().await.unwrap();
    let failures = Arc::new(Mutex::new(vec![]));
    let out = failures.clone();
    k.root()
        .dispatch(Notice(0))
        .invoke(move |e| lock(&out).push(e));
    turn().await;
    assert!(started.load(Ordering::SeqCst));
    f.dispose().await;
    assert!(dropped.load(Ordering::SeqCst));
    assert!(lock(&failures).is_empty());
    gate.add_permits(1);
    turn().await;
    assert!(lock(&failures).is_empty());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn contained_async_tasks_carry_the_listener_fiber_mark() {
    let k = Kernel::start();
    let out = Arc::new(Mutex::new(None));
    let send = out.clone();
    let f = k
        .root()
        .inject(move |ctx, (): ()| {
            let send = send.clone();
            async move {
                let own = ctx.fiber();
                ctx.on_contained_async::<Notice, _>(move |_| {
                    let own = own.clone();
                    let send = send.clone();
                    async move {
                        *lock(&send) = Some(own.restart().await);
                        Ok(())
                    }
                })?;
                Ok(())
            }
        })
        .unwrap();
    f.settled().await.unwrap();
    k.root().dispatch(Notice(0)).invoke(|_| {});
    turn().await;
    assert!(matches!(
        lock(&out).as_ref(),
        Some(Err(KernelError::SelfWait))
    ));
    tokio::time::timeout(Duration::from_secs(1), f.settled())
        .await
        .unwrap()
        .unwrap();
    k.shutdown().await;
}
