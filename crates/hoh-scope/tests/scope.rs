use hoh_kernel::{Context, Emit, Kernel, Undo};
use hoh_scope::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Notify;

struct Ping(&'static str);
hoh_kernel::event!(Ping = "scope-test/ping", Emit);
struct Probe;
hoh_kernel::event!(Probe = "probe/event", Emit);

async fn mint(ctx: &Context, key: &ScopeKey) -> Scope {
    let scope = create_scope(ctx, key.clone(), CreateScopeOptions::default()).unwrap();
    scope.ctx.fiber().settled().await.unwrap();
    scope
}
type Log = Arc<Mutex<Vec<String>>>;
fn listen(ctx: &Context, label: &'static str, heard: &Log) {
    let heard = heard.clone();
    ctx.on::<Ping>(move |event| heard.lock().unwrap().push(format!("{label}:{}", event.0)))
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn tags_contexts_and_derived_contexts_with_the_nearest_tag_winning() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let outer_key = ScopeKey::new();
    let inner_key = ScopeKey::new();
    let outer = mint(&ctx, &outer_key).await;
    let inner = mint(&outer.ctx, &inner_key).await;
    assert_eq!(scope_of(&ctx), None);
    assert_eq!(scope_of(&outer.ctx), Some(outer_key.clone()));
    assert_eq!(scope_of(&outer.ctx.with_tag(())), Some(outer_key));
    assert_eq!(scope_of(&inner.ctx), Some(inner_key));
    assert_eq!(scope_parent_of(&scope_of(&inner.ctx).unwrap()), None);
    inner.dispose().await;
    outer.dispose().await;
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn is_usable_synchronously_before_the_backing_fiber_activates() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let events = Arc::new(Mutex::new(Vec::new()));
    let scope = create_scope(&ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    assert_eq!(scope.ctx.fiber().state(), hoh_kernel::FiberState::Pending);
    let e = events.clone();
    scope
        .ctx
        .effect("early", || {
            Undo::sync(move || e.lock().unwrap().push("disposed"))
        })
        .unwrap();
    events.lock().unwrap().push("registered");
    assert_eq!(*events.lock().unwrap(), ["registered"]);
    scope.dispose().await;
    assert_eq!(*events.lock().unwrap(), ["registered", "disposed"]);
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn disposal_unwinds_early_effect_listener_and_store_in_reverse_and_rejects_late_registration()
{
    use hoh_scope::store::{EffectOptions, EntryUndo};
    struct Layer(AnonymousEntries<u32>);
    impl ScopeLayer for Layer {
        fn is_empty(&self) -> bool {
            self.0.is_empty()
        }
    }
    struct ListenerDrop(Arc<Mutex<Vec<&'static str>>>);
    impl Drop for ListenerDrop {
        fn drop(&mut self) {
            self.0.lock().unwrap().push("listener");
        }
    }
    let kernel = Kernel::start();
    let scope = create_scope(&kernel.root(), ScopeKey::new(), Default::default()).unwrap();
    assert_eq!(scope.ctx.fiber().state(), hoh_kernel::FiberState::Pending);
    let log = Arc::new(Mutex::new(Vec::new()));
    let l = log.clone();
    scope
        .ctx
        .effect("early", || {
            Undo::sync(move || l.lock().unwrap().push("early"))
        })
        .unwrap();
    let marker = ListenerDrop(log.clone());
    scope
        .ctx
        .on::<Ping>(move |_| {
            let _ = &marker;
        })
        .unwrap();
    let layers = ScopedLayers::new(|_| Ok(Layer(AnonymousEntries::new())), || Ok(())).unwrap();
    let l = log.clone();
    layers
        .effect(
            &scope.ctx,
            |layer| {
                let undo = layer.0.append(1);
                Ok(EntryUndo::new(move || {
                    undo.undo();
                    l.lock().unwrap().push("store");
                }))
            },
            EffectOptions::new("store"),
        )
        .unwrap();
    scope.dispose().await;
    assert_eq!(*log.lock().unwrap(), ["store", "listener", "early"]);
    assert!(matches!(
        scope.ctx.on::<Ping>(|_| {}),
        Err(hoh_kernel::KernelError::Inactive)
    ));
    assert!(matches!(
        scope.ctx.effect("late", || panic!("must not run")),
        Err(hoh_kernel::KernelError::Inactive)
    ));
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn registrations_during_unloading_are_rejected() {
    let kernel = Kernel::start();
    let scope = create_scope(&kernel.root(), ScopeKey::new(), Default::default()).unwrap();
    let started = Arc::new(Notify::new());
    let gate = Arc::new(Notify::new());
    let s = started.clone();
    let g = gate.clone();
    scope
        .ctx
        .effect("blocked", || {
            Undo::future(move || async move {
                s.notify_one();
                g.notified().await;
                Ok(())
            })
        })
        .unwrap();
    let dispose = scope.dispose();
    started.notified().await;
    assert_eq!(scope.ctx.fiber().state(), hoh_kernel::FiberState::Unloading);
    assert!(matches!(
        scope.ctx.on::<Ping>(|_| {}),
        Err(hoh_kernel::KernelError::Inactive)
    ));
    assert!(matches!(
        scope.ctx.effect("late", || panic!("must not run")),
        Err(hoh_kernel::KernelError::Inactive)
    ));
    gate.notify_one();
    dispose.await;
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn shares_quiescence_across_repeat_and_raw_disposer_first_calls() {
    let kernel = Kernel::start();
    let scope = mint(&kernel.root(), &ScopeKey::new()).await;
    let gate = Arc::new(Notify::new());
    let finished = Arc::new(AtomicBool::new(false));
    let g = gate.clone();
    let f = finished.clone();
    scope
        .ctx
        .effect("gate", move || {
            Undo::future(move || async move {
                g.notified().await;
                f.store(true, Ordering::SeqCst);
                Ok(())
            })
        })
        .unwrap();
    let raw = scope.raw_dispose().dispose();
    let public_dispose = scope.dispose();
    tokio::pin!(public_dispose);
    tokio::select! {
        biased;
        () = &mut public_dispose => panic!("disposal finished before releasing the gate"),
        () = std::future::ready(()) => {}
    }
    assert!(!finished.load(Ordering::SeqCst));
    gate.notify_one();
    tokio::join!(raw, public_dispose, scope.dispose());
    assert!(finished.load(Ordering::SeqCst));
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn exposes_the_exact_raw_disposer_for_ordered_composite_teardown() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let order = Arc::new(Mutex::new(Vec::new()));
    let o = order.clone();
    let dispose = ctx
        .clone()
        .effect_async::<hoh_kernel::Error, _>("ordered", |undos| async move {
            let outer = o.clone();
            undos.push(Undo::sync(move || outer.lock().unwrap().push("outer")))?;
            let scope = mint(&ctx, &ScopeKey::new()).await;
            let scoped = o.clone();
            scope.ctx.effect("scope", || {
                Undo::sync(move || scoped.lock().unwrap().push("scope"))
            })?;
            undos.push(scope.raw_dispose())?;
            undos.push(Undo::sync(move || o.lock().unwrap().push("inner")))?;
            Ok(())
        })
        .await
        .unwrap();
    dispose.dispose().await;
    assert_eq!(*order.lock().unwrap(), ["inner", "scope", "outer"]);
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn routes_scoped_listeners_by_key_while_untagged_listeners_remain_global() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let key_a = ScopeKey::new();
    let key_b = ScopeKey::new();
    let a = mint(&ctx, &key_a).await;
    let b = mint(&ctx, &key_b).await;
    let heard = Log::default();
    listen(&ctx, "global", &heard);
    listen(&a.ctx, "A", &heard);
    listen(&b.ctx, "B", &heard);
    scope_target(Arc::new(ctx.clone()), Some(key_a))
        .context(&ctx)
        .emit(&Ping("a"));
    scope_target(Arc::new(ctx.clone()), Some(key_b))
        .context(&ctx)
        .emit(&Ping("b"));
    scope_target(Arc::new(ctx.clone()), None)
        .context(&ctx)
        .emit(&Ping("none"));
    assert_eq!(
        *heard.lock().unwrap(),
        ["global:a", "A:a", "global:b", "B:b", "global:none"]
    );
    tokio::join!(a.dispose(), b.dispose());
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn preserves_a_base_cordis_filter_and_its_receiver() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let key = ScopeKey::new();
    let scope = mint(&ctx, &key).await;
    let heard = Log::default();
    listen(&ctx, "global", &heard);
    listen(&scope.ctx, "A", &heard);
    struct Base {
        matches: AtomicBool,
    }
    impl ScopeFilter for Base {
        fn matches_filter(&self, _: &Context) -> bool {
            self.matches.store(true, Ordering::SeqCst);
            false
        }
    }
    let base = Arc::new(Base {
        matches: AtomicBool::new(false),
    });
    let carrier = scope_target(base.clone(), Some(key));
    carrier.context(&ctx).emit(&Ping("vetoed"));
    assert!(heard.lock().unwrap().is_empty());
    assert!(base.matches.load(Ordering::SeqCst));
    scope.dispose().await;
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn global_true_listeners_retain_cordis_global_listener_semantics() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let scope = mint(&ctx, &ScopeKey::new()).await;
    let heard = Log::default();
    let h = heard.clone();
    scope
        .ctx
        .listen()
        .global()
        .on::<Ping>(move |event| h.lock().unwrap().push(event.0.into()))
        .unwrap();
    scope_target(Arc::new(ctx.clone()), Some(ScopeKey::new()))
        .context(&ctx)
        .emit(&Ping("foreign"));
    scope_target(Arc::new(ctx.clone()), None)
        .context(&ctx)
        .emit(&Ping("none"));
    assert_eq!(*heard.lock().unwrap(), ["foreign", "none"]);
    scope.dispose().await;
    kernel.shutdown().await;
}

#[test]
fn uses_an_opaque_branded_carrier_with_a_separately_tracked_key() {
    struct Subject {
        _value: u32,
    }
    impl ScopeFilter for Subject {
        fn matches_filter(&self, _: &Context) -> bool {
            true
        }
    }
    let key = ScopeKey::new();
    let subject = Arc::new(Subject { _value: 1 });
    let carrier: Scoped<Subject> = scope_target(subject.clone(), Some(key.clone()));
    assert!(is_scope_carrier(&carrier));
    assert_eq!(carrier_key_of(&carrier), Some(key));
    let unkeyed = scope_target(subject, None);
    assert!(is_scope_carrier(&unkeyed));
    assert_eq!(carrier_key_of(&unkeyed), None);
    // Scoped<Subject> has no Subject fields; non-carriers cannot be passed to the helpers.
}

#[tokio::test(start_paused = true)]
async fn links_at_mint_walks_to_the_root_and_rejects_cycles() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let preset = ScopeKey::new();
    let agent = ScopeKey::new();
    mint(&ctx, &preset).await;
    create_scope(
        &ctx,
        agent.clone(),
        CreateScopeOptions {
            parent: Some(preset.clone()),
        },
    )
    .unwrap();
    assert_eq!(scope_parent_of(&agent), Some(preset.clone()));
    assert_eq!(scope_parent_of(&preset), None);
    assert_eq!(
        scope_chain_of(Some(&agent)),
        [agent.clone(), preset.clone()]
    );
    assert_eq!(scope_chain_of(None), []);
    assert_eq!(
        bind_scope_parent(&preset, agent).err().unwrap().to_string(),
        "dsh-scope: scope parent link would form a cycle"
    );
    assert!(matches!(
        bind_scope_parent(&preset, preset.clone()),
        Err(ScopeError::Cycle)
    ));
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn re_links_only_through_the_binding_held_by_the_original_binder() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let a = ScopeKey::new();
    let b = ScopeKey::new();
    let agent = ScopeKey::new();
    mint(&ctx, &a).await;
    mint(&ctx, &b).await;
    let binding = bind_scope_parent(&agent, a).unwrap();
    mint(&ctx, &agent).await;
    assert_eq!(
        bind_scope_parent(&agent, b.clone())
            .err()
            .unwrap()
            .to_string(),
        "dsh-scope: scope key is already bound to a parent; re-linking requires the binding returned by the original bind"
    );
    binding.rebind(b.clone()).unwrap();
    assert_eq!(scope_chain_of(Some(&agent)), [agent.clone(), b]);
    let child = ScopeKey::new();
    bind_scope_parent(&child, agent.clone()).unwrap();
    assert!(matches!(binding.rebind(child), Err(ScopeError::Cycle)));
    assert_eq!(
        scope_parent_of(&agent),
        scope_chain_of(Some(&agent)).get(1).cloned()
    );
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn admits_an_ancestor_tagged_listener_for_a_descendant_dispatch_never_the_reverse() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let preset = ScopeKey::new();
    let agent = ScopeKey::new();
    let preset_scope = mint(&ctx, &preset).await;
    let agent_scope = create_scope(
        &ctx,
        agent.clone(),
        CreateScopeOptions {
            parent: Some(preset.clone()),
        },
    )
    .unwrap();
    let other_scope = mint(&ctx, &ScopeKey::new()).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    for (listener, label) in [
        (&ctx, "untagged"),
        (&preset_scope.ctx, "preset"),
        (&agent_scope.ctx, "agent"),
        (&other_scope.ctx, "other"),
    ] {
        let seen = seen.clone();
        listener
            .on::<Probe>(move |_| seen.lock().unwrap().push(label))
            .unwrap();
    }
    scope_target(Arc::new(()), Some(agent))
        .context(&ctx)
        .emit(&Probe);
    seen.lock().unwrap().sort();
    assert_eq!(*seen.lock().unwrap(), ["agent", "preset", "untagged"]);
    seen.lock().unwrap().clear();
    scope_target(Arc::new(()), Some(preset))
        .context(&ctx)
        .emit(&Probe);
    seen.lock().unwrap().sort();
    assert_eq!(*seen.lock().unwrap(), ["preset", "untagged"]);
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn existing_carriers_follow_rebinds_and_scopes_have_the_source_plugin_name() {
    let kernel = Kernel::start();
    let ctx = kernel
        .root()
        .intercept("logger", serde_json::json!({ "name": "inherited" }));
    let a = ScopeKey::new();
    let b = ScopeKey::new();
    let child = ScopeKey::new();
    let binding = bind_scope_parent(&child, a.clone()).unwrap();
    let a_scope = mint(&ctx, &a).await;
    let b_scope = mint(&ctx, &b).await;
    let scope = mint(&ctx, &child).await;
    assert_eq!(scope.ctx.fiber().name().as_ref(), "scope");
    scope.ctx.logger().info("inherited");
    assert_eq!(
        scope.ctx.logger().buffer().last().unwrap().name.as_ref(),
        "inherited"
    );
    let carrier = scope_target(Arc::new(()), Some(child));
    let dispatch = carrier.context(&ctx);
    let heard = Log::default();
    listen(&a_scope.ctx, "A", &heard);
    listen(&b_scope.ctx, "B", &heard);
    dispatch.emit(&Ping("before"));
    binding.rebind(b).unwrap();
    dispatch.emit(&Ping("after"));
    assert_eq!(*heard.lock().unwrap(), ["A:before", "B:after"]);
    kernel.shutdown().await;
}

#[test]
fn concurrent_parent_writes_cannot_close_a_cycle() {
    for _ in 0..16 {
        let a = ScopeKey::new();
        let b = ScopeKey::new();
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|threads| {
            let first = threads.spawn(|| {
                barrier.wait();
                bind_scope_parent(&a, b.clone())
            });
            let second = threads.spawn(|| {
                barrier.wait();
                bind_scope_parent(&b, a.clone())
            });
            let first = first.join().unwrap();
            let second = second.join().unwrap();
            assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
            assert!(matches!(
                first.err().or(second.err()),
                Some(ScopeError::Cycle)
            ));
        });
    }
}

#[tokio::test(start_paused = true)]
async fn context_base_filter_runs_first_independently_of_the_emitters_filter() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let key = ScopeKey::new();
    let scope = create_scope(&ctx, key.clone(), CreateScopeOptions::default()).unwrap();
    let unrelated = create_scope(&ctx, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    let heard = Log::default();
    listen(&ctx, "global", &heard);
    listen(&scope.ctx, "scoped", &heard);
    listen(&unrelated.ctx, "unrelated", &heard);
    let order = Arc::new(Mutex::new(Vec::new()));
    let seen = order.clone();
    let base = ctx.with_filter(move |listener| {
        let key = scope_of(listener);
        seen.lock().unwrap().push(key.clone());
        key.is_some()
    });
    let carrier = scope_target(Arc::new(base), Some(key.clone()));
    let sender = ctx.with_filter(|_| panic!("emitter filter is not the receiver filter"));
    carrier.context(&sender).emit(&Ping("first"));
    assert_eq!(*heard.lock().unwrap(), ["scoped:first"]);
    assert_eq!(
        *order.lock().unwrap(),
        [None, Some(key), scope_of(&unrelated.ctx)]
    );
    // A rejected base short-circuits before walking scope ancestry.
    let veto = ctx.with_filter(|_| false);
    scope_target(Arc::new(veto), None)
        .context(&ctx)
        .emit(&Ping("vetoed"));
    assert_eq!(*heard.lock().unwrap(), ["scoped:first"]);
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_carrier_used_as_a_base_preserves_its_scope_filter() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let first = ScopeKey::new();
    let second = ScopeKey::new();
    let a = create_scope(&ctx, first.clone(), CreateScopeOptions::default()).unwrap();
    let b = create_scope(&ctx, second.clone(), CreateScopeOptions::default()).unwrap();
    let heard = Log::default();
    listen(&ctx, "global", &heard);
    listen(&a.ctx, "A", &heard);
    listen(&b.ctx, "B", &heard);
    let base = Arc::new(scope_target(Arc::new(()), Some(first)));
    scope_target(base, Some(second))
        .context(&ctx)
        .emit(&Ping("both"));
    assert_eq!(*heard.lock().unwrap(), ["global:both"]);
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn raw_adoption_preserves_async_order_during_parent_teardown_and_public_dispose_joins() {
    let kernel = Kernel::start();
    let parent = kernel.root().child("parent").unwrap();
    let trace = Arc::new(Mutex::new(Vec::new()));
    let inner_started = Arc::new(Notify::new());
    let scope_started = Arc::new(Notify::new());
    let inner_gate = Arc::new(Notify::new());
    let scope_gate = Arc::new(Notify::new());
    let mut held = None;
    parent
        .try_effect::<hoh_error::BoxError>("ordered", |undos| {
            let t = trace.clone();
            undos.push(Undo::sync(move || t.lock().unwrap().push("outer")))?;
            let scope = create_scope(&parent, ScopeKey::new(), CreateScopeOptions::default())?;
            let t = trace.clone();
            let started = scope_started.clone();
            let gate = scope_gate.clone();
            scope.ctx.effect("scope", || {
                Undo::future(move || async move {
                    t.lock().unwrap().push("scope begin");
                    started.notify_one();
                    gate.notified().await;
                    t.lock().unwrap().push("scope end");
                    Ok(())
                })
            })?;
            undos.push(scope.raw_dispose())?;
            let t = trace.clone();
            let started = inner_started.clone();
            let gate = inner_gate.clone();
            undos.push(Undo::future(move || async move {
                t.lock().unwrap().push("inner begin");
                started.notify_one();
                gate.notified().await;
                t.lock().unwrap().push("inner end");
                Ok(())
            }))?;
            held = Some(scope);
            Ok(())
        })
        .unwrap();
    parent.fiber().settled().await.unwrap();
    let dispose_parent = parent.fiber().dispose();
    inner_started.notified().await;
    assert_eq!(*trace.lock().unwrap(), ["inner begin"]);
    inner_gate.notify_one();
    scope_started.notified().await;
    assert_eq!(
        *trace.lock().unwrap(),
        ["inner begin", "inner end", "scope begin"]
    );
    let scope = held.unwrap();
    let wait_scope = scope.dispose();
    tokio::pin!(wait_scope);
    tokio::select! {
        biased;
        () = &mut wait_scope => panic!("scope quiesced while its cleanup was blocked"),
        () = std::future::ready(()) => {}
    }
    scope_gate.notify_one();
    tokio::join!(dispose_parent, wait_scope, scope.dispose());
    assert_eq!(
        *trace.lock().unwrap(),
        [
            "inner begin",
            "inner end",
            "scope begin",
            "scope end",
            "outer"
        ]
    );
    scope.raw_dispose().dispose().await;
    kernel.shutdown().await;
}

#[test]
fn scope_errors_remain_plain_errors_with_the_source_messages() {
    let key = ScopeKey::new();
    let error: hoh_error::BoxError = bind_scope_parent(&key, key.clone()).err().unwrap().into();
    assert!(error.downcast_ref::<ScopeError>().is_some());
    assert!(hoh_error::HarnessError::of(error.as_ref()).is_none());
    assert_eq!(
        error.to_string(),
        "dsh-scope: scope parent link would form a cycle"
    );
}

hoh_kernel::service!(Resource: String = "scope-test/resource");
#[derive(hoh_kernel::Inject)]
struct NeedsResource {
    resource: hoh_kernel::Service<Resource>,
}

#[tokio::test(start_paused = true)]
async fn scoped_registrations_resolve_the_minters_committed_services_through_cleanup() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let resource = Arc::new("original".to_owned());
    let weak = Arc::downgrade(&resource);
    let provision = ctx.provide::<Resource>(resource).await.unwrap();
    let held = Arc::new(Mutex::new(None));
    let out = held.clone();
    let entered = Arc::new(Notify::new());
    let cleanup_entered = entered.clone();
    let gate = Arc::new(Notify::new());
    let cleanup_gate = gate.clone();
    let read = Arc::new(AtomicBool::new(false));
    let cleanup_read = read.clone();
    let owner = ctx
        .inject(move |ctx, deps: NeedsResource| {
            let out = out.clone();
            let entered = cleanup_entered.clone();
            let gate = cleanup_gate.clone();
            let read = cleanup_read.clone();
            async move {
                assert_eq!(deps.resource.as_str(), "original");
                let scope = create_scope(&ctx, ScopeKey::new(), CreateScopeOptions::default())?;
                assert_eq!(scope.ctx.get::<Resource>().unwrap().as_str(), "original");
                let reader = scope.ctx.clone();
                scope.ctx.effect("read on cleanup", || {
                    Undo::future(move || async move {
                        entered.notify_one();
                        gate.notified().await;
                        assert_eq!(reader.get::<Resource>().unwrap().as_str(), "original");
                        assert!(
                            reader
                                .isolate("scope-test/resource", hoh_kernel::Realm::ROOT)
                                .get::<Resource>()
                                .is_none()
                        );
                        read.store(true, Ordering::SeqCst);
                        Ok(())
                    })
                })?;
                *out.lock().unwrap() = Some(scope);
                Ok(())
            }
        })
        .unwrap();
    owner.settled().await.unwrap();
    let scope = held.lock().unwrap().take().unwrap();
    scope.ctx.fiber().settled().await.unwrap();
    let withdrawal = provision.dispose();
    entered.notified().await;
    assert!(ctx.get::<Resource>().is_none());
    assert!(weak.upgrade().is_some());
    assert!(!read.load(Ordering::SeqCst));
    gate.notify_one();
    withdrawal.await;
    assert!(read.load(Ordering::SeqCst));
    assert!(scope.ctx.get::<Resource>().is_none());
    assert!(weak.upgrade().is_none());
    kernel.shutdown().await;
}
