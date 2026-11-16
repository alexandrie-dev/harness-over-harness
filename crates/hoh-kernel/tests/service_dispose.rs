mod support;
use hoh_kernel::*;
use std::sync::{Arc, atomic::Ordering};
use support::resource::{self, Deps, Foo};
use support::*;
#[tokio::test(start_paused = true)]
async fn releases_an_injected_binding_after_providing_the_same_name_in_another_realm() {
    for action in 0..3 {
        let (k, r, _) = harness();
        let p = resource::provider(&r, false).await;
        let c = r
            .inject(|ctx, d: Deps| async move {
                ctx.isolate("foo", Realm::new())
                    .provide::<Foo>(Arc::new(resource::Resource {
                        live: std::sync::atomic::AtomicBool::new(true),
                        t: d.foo.t.clone(),
                    }))
                    .await?;
                Ok(())
            })
            .unwrap();
        c.settled().await.unwrap();
        match action {
            0 => {
                c.dispose().await;
                p.dispose().await;
            }
            1 => p.dispose().await,
            _ => k.shutdown().await,
        }
        k.shutdown().await;
    }
}
#[tokio::test(start_paused = true)]
async fn waits_for_a_disposing_consumer_that_has_already_left_the_registry() {
    let (k, r, t) = harness();
    let d = r.provide::<A>(Arc::new(1)).await.unwrap();
    let x = t.clone();
    let c = r
        .inject(move |ctx, _: ADeps| {
            let t = x.clone();
            async move {
                ctx.effect("consumer", move || {
                    Undo::future(move || async move {
                        t.gate.acquire().await.unwrap().forget();
                        t.push("consumer finished");
                        Ok(())
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    c.settled().await.unwrap();
    let disposing = c.dispose();
    let withdrawal = d.dispose();
    turn().await;
    assert_eq!(c.state(), FiberState::Unloading);
    assert!(t.list().is_empty());
    assert_eq!(k.snapshot().await.provisions[0].dependents, [c.uid()]);
    t.gate.add_permits(1);
    disposing.await;
    withdrawal.await;
    t.push("withdraw completed");
    assert_eq!(t.list(), ["consumer finished", "withdraw completed"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn withdraws_one_binding_without_waiting_for_another_binding_from_the_same_provider() {
    let (k, r, t) = harness();
    let a = r.provide::<A>(Arc::new(1)).await.unwrap();
    r.provide::<B>(Arc::new(2)).await.unwrap();
    let first = r.inject(|_, _: ADeps| async { Ok(()) }).unwrap();
    first.settled().await.unwrap();
    let x = t.clone();
    #[derive(Inject)]
    struct Bd {
        _b: Service<B>,
    }
    let second = r
        .inject(move |ctx, _: Bd| {
            let t = x.clone();
            async move {
                ctx.effect("bar", move || {
                    Undo::future(move || async move {
                        t.gate.acquire().await.unwrap().forget();
                        Ok(())
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    second.settled().await.unwrap();
    let disposing = second.dispose();
    turn().await;
    a.dispose().await;
    assert_eq!(second.state(), FiberState::Unloading);
    t.gate.add_permits(1);
    disposing.await;
    k.shutdown().await;
}
async fn resources(action: u8, composed: bool, isolated: bool) {
    let (k, r, t) = harness();
    let r = if isolated {
        r.isolate("foo", Realm::new())
    } else {
        r
    };
    let p = resource::provider(&r, composed).await;
    let value = r.get::<Foo>().unwrap();
    let c = resource::consumer(&r, true, false).await;
    let task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> = match action {
        0 => Box::pin(p.dispose()),
        1 => Box::pin(k.shutdown()),
        2 => {
            let a = p.dispose();
            let b = c.dispose();
            Box::pin(async move {
                a.await;
                b.await
            })
        }
        3 => {
            let b = c.dispose();
            let a = p.dispose();
            Box::pin(async move {
                a.await;
                b.await
            })
        }
        _ => {
            let a = p.restart();
            Box::pin(async move {
                a.await.unwrap();
            })
        }
    };
    turn().await;
    assert!(value.live.load(Ordering::Acquire));
    assert!(t.list().is_empty());
    t.gate.add_permits(10);
    task.await;
    assert_eq!(t.list(), ["consumer read: open", "provider closed"]);
    if action == 4 {
        c.settled().await.unwrap();
        assert!(r.get::<Foo>().unwrap().live.load(Ordering::Acquire));
    }
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn retains_resources_during_consumer_cleanup() {
    for a in 0..5 {
        resources(a, false, false).await;
    }
}
#[tokio::test(start_paused = true)]
async fn retains_transitive_dependencies_while_a_consumer_providing_another_service_drains() {
    let (k, r, t) = harness();
    let p = resource::provider(&r, false).await;
    let value = r.get::<Foo>().unwrap();
    let mid = r
        .inject(|ctx, d: Deps| async move {
            ctx.provide::<B>(Arc::new(1)).await?;
            ctx.effect("mid", move || Undo::sync(move || d.foo.read()))?;
            Ok(())
        })
        .unwrap();
    mid.settled().await.unwrap();
    #[derive(Inject)]
    struct Bd {
        _b: Service<B>,
    }
    let last = r
        .inject(|ctx, _: Bd| async move {
            let t = ctx.tag::<Arc<Track>>().unwrap();
            ctx.effect("last", move || {
                Undo::future(move || async move {
                    t.push("consumer draining");
                    t.gate.acquire().await.unwrap().forget();
                    t.push("consumer finished");
                    Ok(())
                })
            })?;
            Ok(())
        })
        .unwrap();
    last.settled().await.unwrap();
    let task = p.dispose();
    turn().await;
    assert!(value.live.load(Ordering::Acquire));
    assert_eq!(t.list(), ["consumer draining"]);
    t.gate.add_permits(1);
    task.await;
    assert_eq!(
        t.list(),
        [
            "consumer draining",
            "consumer finished",
            "consumer read: open",
            "provider closed"
        ]
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn retains_dependencies_through_in_flight_setup_and_rollback() {
    for fail in [false, true] {
        let (k, r, t) = harness();
        let p = resource::provider(&r, false).await;
        let value = r.get::<Foo>().unwrap();
        let c = r
            .inject(move |ctx, d: Deps| async move {
                let t = ctx.tag::<Arc<Track>>().unwrap();
                t.push("setup started");
                ctx.effect_async::<Error, _>("setup", move |u| async move {
                    let foo = d.foo.clone();
                    let tt = t.clone();
                    u.push(Undo::sync(move || {
                        foo.read();
                        tt.push("setup rolled back");
                    }))?;
                    t.gate.acquire().await.unwrap().forget();
                    d.foo.read();
                    if fail {
                        return Err("setup failed".into());
                    }
                    Ok(())
                })
                .await?;
                Ok(())
            })
            .unwrap();
        turn().await;
        let disposal = p.dispose();
        turn().await;
        assert!(value.live.load(Ordering::Acquire));
        t.gate.add_permits(1);
        disposal.await;
        c.dispose().await;
        assert_eq!(
            t.list(),
            [
                "setup started",
                "consumer read: open",
                "consumer read: open",
                "setup rolled back",
                "provider closed"
            ]
        );
        assert_eq!(
            r.logger()
                .buffer()
                .iter()
                .filter(|r| r.level == Level::Error)
                .count(),
            usize::from(fail)
        );
        k.shutdown().await;
    }
}
#[tokio::test(start_paused = true)]
async fn releases_dependencies_after_a_cleanup_error() {
    let (k, r, t) = harness();
    resource::provider(&r, false).await;
    resource::consumer(&r, false, true).await;
    k.shutdown().await;
    assert_eq!(t.list(), ["consumer read: open", "provider closed"]);
    assert_eq!(
        r.logger()
            .buffer()
            .iter()
            .filter(|r| r.level == Level::Error)
            .count(),
        1
    );
}
#[tokio::test(start_paused = true)]
async fn keeps_provider_listeners_alive_through_the_final_consumer_event() {
    let (k, r, t) = harness();
    let p = resource::provider(&r, false).await;
    let pctx = t.context();
    let x = t.clone();
    pctx.on::<Ping>(move |_| x.push("received")).unwrap();
    let c = r
        .inject(|ctx, _: Deps| async move {
            let origin = ctx.clone();
            ctx.effect("final event", move || {
                Undo::future(move || async move {
                    tokio::task::yield_now().await;
                    origin.emit(&Ping);
                    Ok(())
                })
            })?;
            Ok(())
        })
        .unwrap();
    c.settled().await.unwrap();
    p.dispose().await;
    assert_eq!(t.list(), ["received", "provider closed"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn allows_unrelated_providers_and_sibling_effects_to_clean_up_concurrently() {
    let (k, r, t) = harness();
    resource::provider(&r, false).await;
    let value = r.get::<Foo>().unwrap();
    let independent = r
        .inject(|ctx, (): ()| async move {
            ctx.provide::<B>(Arc::new(1)).await?;
            let t = ctx.tag::<Arc<Track>>().unwrap();
            ctx.effect("independent", move || {
                Undo::sync(move || t.push("independent"))
            })?;
            Ok(())
        })
        .unwrap();
    independent.settled().await.unwrap();
    let c = resource::consumer(&r, true, false).await;
    let ctx = t.context();
    let x = t.clone();
    ctx.effect("provider sibling", move || {
        Undo::sync(move || x.push("provider sibling"))
    })
    .unwrap();
    let x = t.clone();
    let sibling = r
        .effect("sibling", move || Undo::sync(move || x.push("sibling")))
        .unwrap();
    let shutdown = k.shutdown();
    turn().await;
    assert!(value.live.load(Ordering::Acquire));
    assert!(t.list().contains(&"independent".into()));
    assert_eq!(c.state(), FiberState::Unloading);
    drop(sibling.dispose());
    turn().await;
    assert!(t.list().contains(&"sibling".into()));
    t.gate.add_permits(1);
    shutdown.await;
}
#[tokio::test(start_paused = true)]
async fn supports_composed_provider_effects() {
    for root in [false, true] {
        resources(if root { 1 } else { 0 }, true, false).await;
    }
}
#[tokio::test(start_paused = true)]
async fn keeps_an_inherited_service_accessible_to_a_child_in_a_composed_effect() {
    let (k, r, t) = harness();
    let owner = r
        .inject(|ctx, (): ()| async move {
            let t = ctx.tag::<Arc<Track>>().unwrap();
            let service = Arc::new(resource::Resource {
                live: std::sync::atomic::AtomicBool::new(true),
                t: (*t).clone(),
            });
            ctx.provide::<Foo>(service.clone()).await?;
            let child = ctx.inject(move |ctx, (): ()| {
                let service = service.clone();
                async move {
                    ctx.effect("inherited", move || Undo::sync(move || service.read()))?;
                    Ok(())
                }
            })?;
            let child2 = child.clone();
            ctx.try_effect::<KernelError>("composed child", move |u| {
                u.push(Undo::future(move || async move {
                    child2.dispose().await;
                    Ok(())
                }))
            })?;
            child.settled().await?;
            Ok(())
        })
        .unwrap();
    owner.settled().await.unwrap();
    owner.dispose().await;
    assert_eq!(t.list(), ["consumer read: open"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn retains_an_ancestor_injection_until_its_child_finishes_cleanup() {
    let (k, r, t) = harness();
    let p = resource::provider(&r, false).await;
    let value = r.get::<Foo>().unwrap();
    let parent = r
        .inject(|ctx, d: Deps| async move {
            ctx.inject(move |ctx, (): ()| {
                let foo = d.foo.clone();
                async move {
                    let t = ctx.tag::<Arc<Track>>().unwrap();
                    ctx.effect("child", move || {
                        Undo::future(move || async move {
                            t.gate.acquire().await.unwrap().forget();
                            foo.read();
                            Ok(())
                        })
                    })?;
                    Ok(())
                }
            })?
            .settled()
            .await?;
            Ok(())
        })
        .unwrap();
    parent.settled().await.unwrap();
    let dispose = p.dispose();
    turn().await;
    assert!(value.live.load(Ordering::Acquire));
    t.gate.add_permits(1);
    dispose.await;
    assert_eq!(t.list(), ["consumer read: open", "provider closed"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn waits_for_a_child_that_explicitly_injects_its_parent_service() {
    let (k, r, t) = harness();
    let p = resource::provider(&r, false).await;
    let value = r.get::<Foo>().unwrap();
    resource::consumer(&t.context(), true, false).await;
    let dispose = p.dispose();
    turn().await;
    assert!(value.live.load(Ordering::Acquire));
    t.gate.add_permits(1);
    dispose.await;
    assert_eq!(t.list(), ["consumer read: open", "provider closed"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn tracks_same_name_services_in_separate_isolation_realms() {
    for shutdown in [false, true] {
        let (k, r, t) = harness();
        let first = Realm::new();
        let second = Realm::new();
        let p = resource::provider(&r.isolate("foo", first), false).await;
        let pctx = t.context();
        pctx.isolate("foo", second)
            .provide::<Foo>(Arc::new(resource::Resource {
                live: std::sync::atomic::AtomicBool::new(true),
                t: t.clone(),
            }))
            .await
            .unwrap();
        resource::consumer(&r.isolate("foo", first), true, false).await;
        let task = if shutdown {
            Box::pin(k.shutdown())
                as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        } else {
            Box::pin(p.dispose())
        };
        turn().await;
        assert!(t.list().is_empty());
        t.gate.add_permits(1);
        task.await;
        assert_eq!(t.list(), ["consumer read: open", "provider closed"]);
        k.shutdown().await;
    }
}
async fn replacement(during_setup: bool) {
    let (k, r, t) = harness();
    let old = r.provide::<A>(Arc::new(1)).await.unwrap();
    let x = t.clone();
    let c = r
        .inject(move |ctx, d: ADeps| {
            let t = x.clone();
            async move {
                let version = *d.a;
                t.push(format!("load {version}"));
                ctx.effect_async::<Error, _>("replacement", move |u| async move {
                    if during_setup || version == 2 {
                        t.gate.acquire().await.unwrap().forget();
                    }
                    let tt = t.clone();
                    let _ = u.push(Undo::future(move || async move {
                        if !during_setup {
                            tt.cleanup.acquire().await.unwrap().forget();
                        }
                        tt.push(format!("unload {version}"));
                        drop(d);
                        Ok(())
                    }));
                    Ok(())
                })
                .await?;
                Ok(())
            }
        })
        .unwrap();
    turn().await;
    if !during_setup {
        c.settled().await.unwrap();
    }
    let withdrawn = old.dispose();
    r.provide::<A>(Arc::new(2)).await.unwrap();
    turn().await;
    if during_setup {
        t.gate.add_permits(1);
    } else {
        t.cleanup.add_permits(1);
    }
    turn().await;
    withdrawn.await;
    assert_eq!(c.state(), FiberState::Loading);
    assert_eq!(*r.get::<A>().unwrap(), 2);
    assert_eq!(t.list(), ["load 1", "unload 1", "load 2"]);
    t.gate.add_permits(1);
    c.settled().await.unwrap();
    t.cleanup.add_permits(1);
    c.dispose().await;
    assert_eq!(t.list(), ["load 1", "unload 1", "load 2", "unload 2"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn withdraws_only_the_old_binding_without_waiting_for_the_next_load() {
    replacement(false).await;
}
#[tokio::test(start_paused = true)]
async fn replaces_a_binding_from_the_same_provider_during_consumer_initialization() {
    replacement(true).await;
}
#[tokio::test(start_paused = true)]
async fn waits_for_current_and_withdrawn_bindings_during_provider_disposal() {
    for old_first in [false, true] {
        let (k, r, t) = harness();
        let p = resource::provider(&r, false).await;
        let value = r.get::<Foo>().unwrap();
        let ctx = t.context();
        let old = ctx.provide::<B>(Arc::new(1)).await.unwrap();
        resource::consumer(&r, true, false).await;
        #[derive(Inject)]
        struct Bd {
            _b: Service<B>,
        }
        let last = r
            .inject(|ctx, _: Bd| async move {
                let t = ctx.tag::<Arc<Track>>().unwrap();
                ctx.effect("bar", move || {
                    Undo::future(move || async move {
                        t.cleanup.acquire().await.unwrap().forget();
                        t.push("consumer finished");
                        Ok(())
                    })
                })?;
                Ok(())
            })
            .unwrap();
        last.settled().await.unwrap();
        let withdrawal = old.dispose();
        ctx.provide::<B>(Arc::new(2)).await.unwrap();
        let dispose = p.dispose();
        turn().await;
        if old_first {
            t.cleanup.add_permits(1);
        } else {
            t.gate.add_permits(1);
        }
        turn().await;
        assert!(value.live.load(Ordering::Acquire));
        if old_first {
            t.gate.add_permits(1);
        } else {
            t.cleanup.add_permits(1);
        }
        withdrawal.await;
        dispose.await;
        let expected = if old_first {
            vec![
                "consumer finished",
                "consumer read: open",
                "provider closed",
            ]
        } else {
            vec![
                "consumer read: open",
                "consumer finished",
                "provider closed",
            ]
        };
        assert_eq!(t.list(), expected);
        k.shutdown().await;
    }
}
