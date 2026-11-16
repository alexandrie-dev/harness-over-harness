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
    create_scope(ctx, key.clone(), CreateScopeOptions::default())
        .await
        .unwrap()
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
    let raw = scope.raw_dispose.dispose();
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
            undos.push(scope.raw_dispose)?;
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
    scope_target(&ctx, Some(key_a))
        .context(&ctx)
        .emit(&Ping("a"));
    scope_target(&ctx, Some(key_b))
        .context(&ctx)
        .emit(&Ping("b"));
    scope_target(&ctx, None).context(&ctx).emit(&Ping("none"));
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
    let base = Arc::new(());
    let expected = base.clone();
    let receiver = base.clone();
    let matches = Arc::new(AtomicBool::new(false));
    let m = matches.clone();
    let carrier = scope_target(&base, Some(key)).with_base_filter(move |_| {
        m.store(Arc::ptr_eq(&receiver, &expected), Ordering::SeqCst);
        false
    });
    carrier.context(&ctx).emit(&Ping("vetoed"));
    assert!(heard.lock().unwrap().is_empty());
    assert!(matches.load(Ordering::SeqCst));
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
    scope_target(&ctx, Some(ScopeKey::new()))
        .context(&ctx)
        .emit(&Ping("foreign"));
    scope_target(&ctx, None).context(&ctx).emit(&Ping("none"));
    assert_eq!(*heard.lock().unwrap(), ["foreign", "none"]);
    scope.dispose().await;
    kernel.shutdown().await;
}

#[test]
fn uses_an_opaque_branded_carrier_with_a_separately_tracked_key() {
    struct Subject {
        _value: u32,
    }
    let key = ScopeKey::new();
    let subject = Subject { _value: 1 };
    let carrier: Scoped<Subject> = scope_target(&subject, Some(key.clone()));
    assert!(is_scope_carrier(&carrier));
    assert_eq!(carrier_key_of(&carrier), Some(key));
    let unkeyed = scope_target(&subject, None);
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
    .await
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
    .await
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
    scope_target(&(), Some(agent)).context(&ctx).emit(&Probe);
    seen.lock().unwrap().sort();
    assert_eq!(*seen.lock().unwrap(), ["agent", "preset", "untagged"]);
    seen.lock().unwrap().clear();
    scope_target(&(), Some(preset)).context(&ctx).emit(&Probe);
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
    let carrier = scope_target(&(), Some(child));
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
