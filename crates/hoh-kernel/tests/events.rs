mod support;
use hoh_kernel::*;
use serde_json::json;
use std::sync::{Arc, Mutex};
use support::*;
#[tokio::test(start_paused = true)]
async fn ctx_on() {
    let (k, r, t) = harness();
    let a = t.clone();
    let d = r.on::<Ping>(move |_| a.push("called")).unwrap();
    r.emit(&Ping);
    r.emit(&Ping);
    drop(d.dispose());
    r.emit(&Ping);
    assert_eq!(t.list(), ["called", "called"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn removes_empty_event_buckets_after_disposal() {
    let (k, r, t) = harness();
    let a = t.clone();
    let d = r.on::<Ping>(move |_| a.push("a")).unwrap();
    let a = t.clone();
    let e = r.on::<Ping>(move |_| a.push("b")).unwrap();
    d.dispose().await;
    r.emit(&Ping);
    e.dispose().await;
    r.emit(&Ping);
    assert_eq!(t.list(), ["b"]);
    k.shutdown().await;
}
async fn mutation(mode: Dispatch, adding: bool) {
    let (k, r, t) = harness();
    let slot = Arc::new(Mutex::new(None::<Disposer>));
    let slot2 = slot.clone();
    let ctx = r.clone();
    let a = t.clone();
    let mutate = move || {
        if adding {
            let t = a.clone();
            match mode {
                Dispatch::Emit => {
                    ctx.on::<Ping>(move |_| t.push("added")).unwrap();
                }
                Dispatch::Bail => {
                    ctx.on_bail::<Stop, i32>(move |_| {
                        t.push("added");
                        None
                    })
                    .unwrap();
                }
                Dispatch::Serial => {
                    ctx.on_serial::<Ser, i32, _>(move |_| {
                        t.push("added");
                        async { Ok(None) }
                    })
                    .unwrap();
                }
                Dispatch::Parallel => {
                    ctx.on_parallel::<Par, _>(move |_| {
                        t.push("added");
                        async { Ok(()) }
                    })
                    .unwrap();
                }
                _ => {}
            }
        } else if let Some(d) = lock(&slot2).take() {
            drop(d.dispose());
        }
    };
    let mutation: Arc<dyn Fn() + Send + Sync> = Arc::new(mutate);
    match mode {
        Dispatch::Emit => {
            let f = mutation.clone();
            r.on::<Ping>(move |_| f()).unwrap();
            let t = t.clone();
            *lock(&slot) = Some(r.on::<Ping>(move |_| t.push("removed")).unwrap());
        }
        Dispatch::Bail => {
            let f = mutation.clone();
            r.on_bail::<Stop, i32>(move |_| {
                f();
                None
            })
            .unwrap();
            let t = t.clone();
            *lock(&slot) = Some(
                r.on_bail::<Stop, i32>(move |_| {
                    t.push("removed");
                    None
                })
                .unwrap(),
            );
        }
        Dispatch::Serial => {
            let f = mutation.clone();
            r.on_serial::<Ser, i32, _>(move |_| {
                f();
                async { Ok(None) }
            })
            .unwrap();
            let t = t.clone();
            *lock(&slot) = Some(
                r.on_serial::<Ser, i32, _>(move |_| {
                    t.push("removed");
                    async { Ok(None) }
                })
                .unwrap(),
            );
        }
        Dispatch::Parallel => {
            let f = mutation.clone();
            r.on_parallel::<Par, _>(move |_| {
                f();
                async { Ok(()) }
            })
            .unwrap();
            let t = t.clone();
            *lock(&slot) = Some(
                r.on_parallel::<Par, _>(move |_| {
                    t.push("removed");
                    async { Ok(()) }
                })
                .unwrap(),
            );
        }
        _ => {}
    }
    for pass in 0..if adding { 2 } else { 1 } {
        match mode {
            Dispatch::Emit => r.emit(&Ping),
            Dispatch::Bail => {
                r.bail(&Stop);
            }
            Dispatch::Serial => {
                r.serial(Ser).await.unwrap();
            }
            Dispatch::Parallel => {
                r.parallel(Par).await.unwrap();
            }
            _ => {}
        }
        if adding {
            assert_eq!(
                t.list().iter().filter(|s| s.as_str() == "added").count(),
                pass
            );
        } else {
            assert!(t.list().is_empty());
        }
    }
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn skips_listeners_removed_during_dispatch() {
    for mode in [
        Dispatch::Emit,
        Dispatch::Parallel,
        Dispatch::Serial,
        Dispatch::Bail,
    ] {
        mutation(mode, false).await;
    }
}
#[tokio::test(start_paused = true)]
async fn defers_added_listeners_until_the_next_dispatch() {
    for mode in [
        Dispatch::Emit,
        Dispatch::Parallel,
        Dispatch::Serial,
        Dispatch::Bail,
    ] {
        mutation(mode, true).await;
    }
}
#[tokio::test(start_paused = true)]
async fn skips_a_listener_disposed_by_an_earlier_component_during_dispatch() {
    let (k, r, t) = harness();
    let slot = Arc::new(Mutex::new(None::<Fiber>));
    let x = slot.clone();
    r.on_serial::<Ser, i32, _>(move |_| {
        let child = lock(&x).clone().unwrap();
        async move {
            child.dispose().await;
            Ok(None)
        }
    })
    .unwrap();
    let a = t.clone();
    let child = r
        .inject(move |ctx, (): ()| {
            let t = a.clone();
            async move {
                ctx.on_serial::<Ser, i32, _>(move |_| {
                    t.push("removed");
                    async { Ok(None) }
                })?;
                Ok(())
            }
        })
        .unwrap();
    child.settled().await.unwrap();
    *lock(&slot) = Some(child);
    r.serial(Ser).await.unwrap();
    assert!(t.list().is_empty());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn waterfall_continues_past_a_removed_listener() {
    let (k, r, t) = harness();
    let slot = Arc::new(Mutex::new(None::<Disposer>));
    let x = slot.clone();
    r.on_waterfall::<Fall, i32, _>(move |_, next| {
        if let Some(d) = lock(&x).take() {
            drop(d.dispose());
        }
        async move { Ok(1 + next.run().await?) }
    })
    .unwrap();
    let a = t.clone();
    *lock(&slot) = Some(
        r.on_waterfall::<Fall, i32, _>(move |_, next| {
            a.push("removed");
            async move { next.run().await }
        })
        .unwrap(),
    );
    r.on_waterfall::<Fall, i32, _>(|_, next| async move { Ok(1 + next.run().await?) })
        .unwrap();
    assert_eq!(r.waterfall(Fall, |_| async { Ok(2) }).await.unwrap(), 4);
    assert!(t.list().is_empty());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn waterfall_defers_added_listeners_until_the_next_dispatch() {
    let (k, r, t) = harness();
    let ctx = r.clone();
    let a = t.clone();
    r.on_waterfall::<Fall, i32, _>(move |_, next| {
        let a = a.clone();
        ctx.on_waterfall::<Fall, i32, _>(move |_, next| {
            a.push("added");
            async move { Ok(1 + next.run().await?) }
        })
        .unwrap();
        async move { next.run().await }
    })
    .unwrap();
    assert_eq!(r.waterfall(Fall, |_| async { Ok(2) }).await.unwrap(), 2);
    assert!(t.list().is_empty());
    assert_eq!(r.waterfall(Fall, |_| async { Ok(2) }).await.unwrap(), 3);
    assert_eq!(t.list(), ["added"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn internal_update_continues_past_a_removed_fiber_listener() {
    let (k, r, t) = harness();
    let f = r
        .inject(|ctx, (): ()| async move {
            let t = ctx.tag::<Arc<Track>>().unwrap();
            t.push("load");
            let slot = Arc::new(Mutex::new(None::<Disposer>));
            let x = slot.clone();
            ctx.on_update::<NoConfig, _>(move |_, next| {
                if let Some(d) = lock(&x).take() {
                    drop(d.dispose());
                }
                async move { next.run().await }
            })?;
            let t2 = t.clone();
            *lock(&slot) = Some(ctx.on_update::<NoConfig, _>(move |_, next| {
                t2.push("removed");
                async move { next.run().await }
            })?);
            ctx.on_update::<NoConfig, _>(move |_, next| {
                t.push("remaining");
                async move { next.run().await }
            })?;
            Ok(())
        })
        .unwrap();
    f.settled().await.unwrap();
    f.update(json!({})).await.unwrap();
    assert_eq!(t.list(), ["load", "remaining", "load"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn ctx_parallel() {
    let (k, r, t) = harness();
    r.parallel(Par).await.unwrap();
    let a = t.clone();
    r.with_tag(true)
        .on_parallel::<Par, _>(move |_| {
            a.push("called");
            async { Ok(()) }
        })
        .unwrap();
    r.parallel(Par).await.unwrap();
    r.with_filter(|ctx| ctx.tag::<bool>().is_some_and(|v| !*v))
        .parallel(Par)
        .await
        .unwrap();
    r.with_filter(|ctx| ctx.tag::<bool>().is_some_and(|v| *v))
        .parallel(Par)
        .await
        .unwrap();
    assert_eq!(t.list(), ["called", "called"]);
    r.on_parallel::<Par, _>(|_| async { Err("test".into()) })
        .unwrap();
    r.on_parallel::<Par, _>(|_| async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        Err("async".into())
    })
    .unwrap();
    let error = r.parallel(Par).await.unwrap_err();
    let KernelError::Listeners { event, errors } = error else {
        panic!()
    };
    assert_eq!(event, "parallel");
    assert_eq!(
        errors.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["test", "async"]
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn ctx_emit() {
    let (k, r, t) = harness();
    let a = t.clone();
    r.with_tag(true)
        .on::<Ping>(move |_| a.push("called"))
        .unwrap();
    r.emit(&Ping);
    r.with_filter(|_| false).emit(&Ping);
    assert_eq!(t.list(), ["called"]);
    r.on::<Ping>(|_| panic!("test")).unwrap();
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.emit(&Ping))).is_err());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn ctx_serial() {
    let (k, r, t) = harness();
    let a = t.clone();
    r.on_serial::<Ser, i32, _>(move |_| {
        a.push("first");
        async { Ok(None) }
    })
    .unwrap();
    r.on_serial::<Ser, i32, _>(|_| async { Ok(Some(0)) })
        .unwrap();
    r.on_serial::<Ser, i32, _>(|_| async { panic!("must not run") })
        .unwrap();
    assert_eq!(r.serial(Ser).await.unwrap(), Some(0));
    assert_eq!(t.list(), ["first"]);
    assert_eq!(r.with_filter(|_| false).serial(Ser).await.unwrap(), None);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn ctx_bail() {
    let (k, r, t) = harness();
    let a = t.clone();
    r.on_bail::<Stop, i32>(move |_| {
        a.push("first");
        None
    })
    .unwrap();
    r.on_bail::<Stop, i32>(|_| Some(0)).unwrap();
    r.on_bail::<Stop, i32>(|_| panic!("must not run")).unwrap();
    assert_eq!(r.bail(&Stop), Some(0));
    assert_eq!(t.list(), ["first"]);
    assert_eq!(r.with_filter(|_| false).bail(&Stop), None);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn ctx_waterfall() {
    let (k, r, _) = harness();
    r.on_waterfall::<Fall, i32, _>(|_, next| async move { Ok(1 + next.run().await?) })
        .unwrap();
    r.on_waterfall::<Fall, i32, _>(|_, next| async move { Ok(1 + next.run().await?) })
        .unwrap();
    assert_eq!(r.waterfall(Fall, |_| async { Ok(2) }).await.unwrap(), 4);
    r.on_waterfall::<Fall, i32, _>(|_, _| async { Ok(1) })
        .unwrap();
    r.on_waterfall::<Fall, i32, _>(|_, _| async { panic!("must not run") })
        .unwrap();
    assert_eq!(r.waterfall(Fall, |_| async { Ok(2) }).await.unwrap(), 3);
    k.shutdown().await;
}
struct Nested(bool);
event!(Nested = "nested", Waterfall<i32>);
#[tokio::test(start_paused = true)]
async fn ctx_waterfall_supports_nested_async_calls() {
    let (k, r, _) = harness();
    let ctx = r.clone();
    r.on_waterfall::<Nested, i32, _>(move |e, next| {
        let ctx = ctx.clone();
        async move {
            let result = next.run().await?;
            if !e.0 {
                Ok(result + ctx.waterfall(Nested(true), |_| async { Ok(2) }).await?)
            } else {
                Ok(result)
            }
        }
    })
    .unwrap();
    assert_eq!(
        r.waterfall(Nested(false), |_| async { Ok(2) })
            .await
            .unwrap(),
        4
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn does_not_accumulate_across_reloads() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({"hook":true})).unwrap();
    f.settled().await.unwrap();
    for n in 1..=3 {
        f.update(json!({"value":n,"delegate":true,"hook":true}))
            .await
            .unwrap();
    }
    assert_eq!(
        lock(&t.updates)
            .iter()
            .map(|v| v["value"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn can_be_disposed_explicitly() {
    let (k, r, t) = harness();
    let slot = Arc::new(Mutex::new(None::<Disposer>));
    let x = slot.clone();
    let f = r
        .inject(move |ctx, (): ()| {
            let x = x.clone();
            async move {
                let t = ctx.tag::<Arc<Track>>().unwrap();
                *lock(&x) = Some(ctx.on_update::<NoConfig, _>(move |_, next| {
                    t.push("update");
                    async move { next.run().await }
                })?);
                Ok(())
            }
        })
        .unwrap();
    f.settled().await.unwrap();
    let disposer = lock(&slot).take().unwrap();
    disposer.dispose().await;
    f.update(json!({})).await.unwrap();
    assert!(t.list().is_empty());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn runs_prepended_listeners_first() {
    let (k, r, t) = harness();
    let slot = Arc::new(Mutex::new(None::<Disposer>));
    let slot2 = slot.clone();
    let f = r
        .inject(move |ctx, (): ()| {
            let slot = slot2.clone();
            async move {
                let t = ctx.tag::<Arc<Track>>().unwrap();
                for (name, prepend) in [("a", false), ("b", true), ("c", false), ("d", true)] {
                    let t = t.clone();
                    let listen = ctx.listen();
                    let listen = if prepend { listen.prepend() } else { listen };
                    let d = listen.on_update::<NoConfig, _>(move |_, next| {
                        t.push(name);
                        async move { next.run().await }
                    })?;
                    if name == "b" {
                        *lock(&slot) = Some(d);
                    }
                }
                Ok(())
            }
        })
        .unwrap();
    f.settled().await.unwrap();
    f.update(json!({})).await.unwrap();
    assert_eq!(t.list(), ["d", "b", "a", "c"]);
    lock(&t.log).clear();
    let d = lock(&slot).take().unwrap();
    d.dispose().await;
    f.update(json!({})).await.unwrap();
    assert_eq!(t.list(), ["d", "a", "c"]);
    k.shutdown().await;
}
