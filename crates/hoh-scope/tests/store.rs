use hoh_kernel::{Context, Error, Kernel};
use hoh_scope::{
    store::{EffectOptions, EntryUndo},
    *,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct TestLayer {
    named: NamedEntries<u32>,
    anonymous: AnonymousEntries<String>,
}
impl TestLayer {
    fn new(scope: Option<&ScopeKey>) -> Self {
        let label = if scope.is_none() { "global" } else { "scoped" };
        Self {
            named: NamedEntries::new(move |name| -> Error {
                format!("{label} duplicate: {name}").into()
            }),
            anonymous: AnonymousEntries::new(),
        }
    }
}
impl ScopeLayer for TestLayer {
    fn is_empty(&self) -> bool {
        self.named.is_empty() && self.anonymous.is_empty()
    }
}
async fn mint(ctx: &Context, key: &ScopeKey) -> Scope {
    create_scope(ctx, key.clone(), CreateScopeOptions::default())
        .await
        .unwrap()
}
fn quiet(label: &'static str) -> EffectOptions {
    EffectOptions {
        label,
        notify: false,
    }
}
fn entries(layers: &ScopedLayers<TestLayer>, scope: Option<&ScopeKey>) -> Vec<(String, u32)> {
    layers
        .merge(scope, |layer| &layer.named)
        .into_iter()
        .collect()
}

#[test]
fn owns_duplicate_diagnostics_lookup_insertion_order_live_iteration_and_exact_idempotent_undo() {
    let duplicate: Arc<str> = "caller duplicate".into();
    let expected = duplicate.clone();
    let called = Arc::new(Mutex::new(Vec::new()));
    let c = called.clone();
    let entries = NamedEntries::new(move |name| {
        c.lock().unwrap().push(name.to_owned());
        duplicate.clone()
    });
    let undo_a = entries.insert("a", 1).unwrap();
    let mut values = entries.values();
    assert_eq!(values.next(), Some(1));
    let undo_b = entries.insert("b", 2).unwrap();
    assert_eq!(values.collect::<Vec<_>>(), [2]);
    assert_eq!(entries.keys().collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(
        entries.entries().collect::<Vec<_>>(),
        [("a".into(), 1), ("b".into(), 2)]
    );
    assert_eq!(entries.get("a"), Some(1));
    assert_eq!(entries.get("missing"), None);
    assert!(entries.has("b"));
    assert!(!entries.has("missing"));
    assert!(!entries.is_empty());
    let actual = entries.insert("a", 3).err().unwrap();
    assert!(Arc::ptr_eq(&expected, &actual));
    assert_eq!(*called.lock().unwrap(), ["a"]);
    undo_a.undo();
    entries.insert("a", 3).unwrap();
    undo_a.undo();
    assert_eq!(entries.get("a"), Some(3));
    undo_b.undo();
    assert_eq!(entries.entries().collect::<Vec<_>>(), [("a".into(), 3)]);
}

#[test]
fn named_starts_a_fresh_iterator_generation_after_the_table_drains() {
    let entries = NamedEntries::new(|name| format!("duplicate: {name}"));
    let undo = entries.insert("first", 1).unwrap();
    let mut values = entries.values();
    assert_eq!(values.next(), Some(1));
    undo.undo();
    entries.insert("replacement", 2).unwrap();
    assert_eq!(values.next(), None);
    assert_eq!(entries.values().collect::<Vec<_>>(), [2]);
}

#[test]
fn owns_equal_values_independently_with_live_insertion_ordered_iteration_and_idempotent_undo() {
    let entries = AnonymousEntries::new();
    let value = Arc::new(());
    let first = entries.append(value.clone());
    let mut values = entries.values();
    assert!(Arc::ptr_eq(&values.next().unwrap(), &value));
    let second = entries.append(value.clone());
    let rest: Vec<_> = values.collect();
    assert_eq!(rest.len(), 1);
    assert!(Arc::ptr_eq(&rest[0], &value));
    let all: Vec<_> = entries.values().collect();
    assert_eq!(all.len(), 2);
    assert!(all.iter().all(|v| Arc::ptr_eq(v, &value)));
    first.undo();
    first.undo();
    let all: Vec<_> = entries.values().collect();
    assert_eq!(all.len(), 1);
    assert!(Arc::ptr_eq(&all[0], &value));
    second.undo();
    assert!(entries.is_empty());
}

#[test]
fn anonymous_starts_a_fresh_iterator_generation_after_the_table_drains() {
    let entries = AnonymousEntries::new();
    let undo = entries.append(1);
    let mut values = entries.values();
    assert_eq!(values.next(), Some(1));
    undo.undo();
    entries.append(2);
    assert_eq!(values.next(), None);
    assert_eq!(entries.values().collect::<Vec<_>>(), [2]);
}

#[test]
fn constructs_global_state_eagerly_while_reads_stay_non_creating_and_merge_named_shadows_in_order()
{
    let created = Arc::new(Mutex::new(Vec::new()));
    let c = created.clone();
    let layers = ScopedLayers::new(
        move |scope| {
            c.lock().unwrap().push(scope.cloned());
            TestLayer::new(scope)
        },
        || {},
    );
    let key = ScopeKey::new();
    layers.global.named.insert("a", 1).unwrap();
    layers.global.named.insert("shared", 2).unwrap();
    assert_eq!(*created.lock().unwrap(), [None]);
    assert!(layers.peek(None).is_none());
    assert!(layers.peek(Some(&key)).is_none());
    assert_eq!(
        entries(&layers, Some(&key)),
        [("a".into(), 1), ("shared".into(), 2)]
    );
    assert_eq!(*created.lock().unwrap(), [None]);
}

#[tokio::test(start_paused = true)]
async fn uses_the_same_scoped_context_for_lazy_visibility_and_ownership_and_reclaims_only_an_empty_aggregate()
 {
    let kernel = Kernel::start();
    let key = ScopeKey::new();
    let scope = mint(&kernel.root(), &key).await;
    let changed = Arc::new(AtomicUsize::new(0));
    let created = Arc::new(Mutex::new(Vec::new()));
    let c = created.clone();
    let count = changed.clone();
    let layers = ScopedLayers::new(
        move |selected| {
            c.lock().unwrap().push(selected.cloned());
            TestLayer::new(selected)
        },
        move || {
            count.fetch_add(1, Ordering::SeqCst);
        },
    );
    layers.global.named.insert("a", 1).unwrap();
    layers.global.named.insert("shared", 1).unwrap();
    let named = layers
        .effect(
            &scope.ctx,
            |layer| layer.named.insert("shared", 2),
            quiet("test.named"),
        )
        .unwrap();
    let tail = layers
        .effect(
            &scope.ctx,
            |layer| layer.named.insert("c", 3),
            quiet("test.tail"),
        )
        .unwrap();
    let anonymous = layers
        .effect(
            &scope.ctx,
            |layer| Ok(layer.anonymous.append("kept".into())),
            quiet("test.anonymous"),
        )
        .unwrap();
    assert_eq!(*created.lock().unwrap(), [None, Some(key.clone())]);
    assert_eq!(
        entries(&layers, Some(&key)),
        [("a".into(), 1), ("shared".into(), 2), ("c".into(), 3)]
    );
    assert_eq!(changed.load(Ordering::SeqCst), 0);
    named.dispose().await;
    assert!(layers.peek(Some(&key)).is_some());
    assert_eq!(
        entries(&layers, Some(&key)),
        [("a".into(), 1), ("shared".into(), 1), ("c".into(), 3)]
    );
    tail.dispose().await;
    assert!(layers.peek(Some(&key)).is_some());
    anonymous.dispose().await;
    assert!(layers.peek(Some(&key)).is_none());
    scope.dispose().await;
    assert_eq!(changed.load(Ordering::SeqCst), 0);
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn runs_action_notification_undo_and_disposal_notification_in_order_with_cordis_idempotence_and_labels()
 {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let events = Arc::new(Mutex::new(Vec::new()));
    let e = events.clone();
    let layers = ScopedLayers::new(TestLayer::new, move || e.lock().unwrap().push("notify"));
    let e = events.clone();
    let dispose = layers
        .effect(
            &ctx,
            move |layer| {
                e.lock().unwrap().push("action");
                let undo = layer.named.insert("x", 1)?;
                Ok(EntryUndo::new(move || {
                    e.lock().unwrap().push("undo");
                    undo.undo();
                }))
            },
            EffectOptions::new("store.order"),
        )
        .unwrap();
    assert_eq!(*events.lock().unwrap(), ["action", "notify"]);
    dispose.dispose().await;
    kernel.shutdown().await;
    assert_eq!(
        *events.lock().unwrap(),
        ["action", "notify", "undo", "notify"]
    );
    assert!(layers.global.is_empty());
    // Effect labels are verified through the kernel's public error log in a separate test.
}

#[tokio::test(start_paused = true)]
async fn cleans_up_failed_factories_and_empty_failed_actions_without_discarding_an_existing_layer()
{
    let kernel = Kernel::start();
    let key = ScopeKey::new();
    let scope = mint(&kernel.root(), &key).await;
    let fail_factory = Arc::new(AtomicBool::new(true));
    let fail = fail_factory.clone();
    let layers = ScopedLayers::try_new(
        move |selected| {
            if selected.is_some() && fail.load(Ordering::SeqCst) {
                return Err("factory failed".into());
            }
            Ok(TestLayer::new(selected))
        },
        || Ok(()),
    )
    .unwrap();
    let result = layers.effect(
        &scope.ctx,
        |layer| layer.named.insert("never", 1),
        quiet("store.factory"),
    );
    assert_eq!(result.err().unwrap().to_string(), "factory failed");
    assert!(layers.peek(Some(&key)).is_none());
    fail_factory.store(false, Ordering::SeqCst);
    let result = layers.effect(
        &scope.ctx,
        |_| Err("action failed".into()),
        quiet("store.action"),
    );
    assert_eq!(result.err().unwrap().to_string(), "action failed");
    assert!(layers.peek(Some(&key)).is_none());
    let dispose = layers
        .effect(
            &scope.ctx,
            |layer| layer.named.insert("kept", 1),
            quiet("store.kept"),
        )
        .unwrap();
    let result = layers.effect(
        &scope.ctx,
        |_| Err("second action failed".into()),
        quiet("store.existing-action"),
    );
    assert_eq!(result.err().unwrap().to_string(), "second action failed");
    assert_eq!(layers.peek(Some(&key)).unwrap().named.get("kept"), Some(1));
    dispose.dispose().await;
    scope.dispose().await;
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn rolls_back_a_scoped_insertion_when_notification_throws() {
    let kernel = Kernel::start();
    let key = ScopeKey::new();
    let scope = mint(&kernel.root(), &key).await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let notifications = Arc::new(AtomicUsize::new(0));
    let e = events.clone();
    let n = notifications.clone();
    let layers = ScopedLayers::try_new(
        |selected| Ok(TestLayer::new(selected)),
        move || {
            e.lock().unwrap().push("notify");
            if n.fetch_add(1, Ordering::SeqCst) == 0 {
                Err("change failed".into())
            } else {
                Ok(())
            }
        },
    )
    .unwrap();
    let e = events.clone();
    let result = layers.effect(
        &scope.ctx,
        move |layer| {
            let undo = layer.named.insert("rollback", 1)?;
            Ok(EntryUndo::new(move || {
                e.lock().unwrap().push("undo");
                undo.undo();
            }))
        },
        EffectOptions::new("store.rollback"),
    );
    assert_eq!(result.err().unwrap().to_string(), "change failed");
    assert_eq!(*events.lock().unwrap(), ["notify", "undo", "notify"]);
    assert!(layers.peek(Some(&key)).is_none());
    scope.dispose().await;
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn merges_ancestor_layers_nearest_last_with_stable_name_order_and_live_rebinding() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let layers = ScopedLayers::new(TestLayer::new, || {});
    let root_key = ScopeKey::new();
    let missing = ScopeKey::new();
    let child_key = ScopeKey::new();
    bind_scope_parent(&missing, root_key.clone()).unwrap();
    let binding = bind_scope_parent(&child_key, missing.clone()).unwrap();
    let root = mint(&ctx, &root_key).await;
    let child = mint(&ctx, &child_key).await;
    layers.global.named.insert("shared", 0).unwrap();
    layers.global.named.insert("global", 1).unwrap();
    layers
        .effect(
            &root.ctx,
            |l| l.named.insert("shared", 2),
            quiet("root.shared"),
        )
        .unwrap();
    layers
        .effect(&root.ctx, |l| l.named.insert("root", 3), quiet("root.own"))
        .unwrap();
    layers
        .effect(
            &child.ctx,
            |l| l.named.insert("shared", 4),
            quiet("child.shared"),
        )
        .unwrap();
    layers
        .effect(
            &child.ctx,
            |l| l.named.insert("child", 5),
            quiet("child.own"),
        )
        .unwrap();
    assert!(layers.peek(Some(&missing)).is_none());
    let chain = layers.chain_layers(Some(&child_key));
    assert_eq!(chain.len(), 2);
    assert!(Arc::ptr_eq(
        &chain[0],
        &layers.peek(Some(&root_key)).unwrap()
    ));
    assert!(Arc::ptr_eq(
        &chain[1],
        &layers.peek(Some(&child_key)).unwrap()
    ));
    assert_eq!(
        entries(&layers, Some(&child_key)),
        [
            ("shared".into(), 4),
            ("global".into(), 1),
            ("root".into(), 3),
            ("child".into(), 5)
        ]
    );
    assert_eq!(
        entries(&layers, Some(&root_key)),
        [
            ("shared".into(), 2),
            ("global".into(), 1),
            ("root".into(), 3)
        ]
    );
    assert_eq!(layers.chain_layers(None).len(), 0);
    binding.rebind(ScopeKey::new()).unwrap();
    assert_eq!(
        entries(&layers, Some(&child_key)),
        [
            ("shared".into(), 4),
            ("global".into(), 1),
            ("child".into(), 5)
        ]
    );
    child.dispose().await;
    assert!(layers.peek(Some(&child_key)).is_none());
    root.dispose().await;
    assert!(layers.peek(Some(&root_key)).is_none());
    assert_eq!(
        entries(&layers, Some(&root_key)),
        [("shared".into(), 0), ("global".into(), 1)]
    );
    kernel.shutdown().await;
}

#[test]
fn live_keys_and_entries_skip_deletions_observe_reinsertions_and_stay_exhausted() {
    let entries = NamedEntries::new(|name| format!("duplicate: {name}"));
    entries.insert("a", 1).unwrap();
    let b = entries.insert("b", 2).unwrap();
    let mut keys = entries.keys();
    let mut pairs = entries.entries();
    assert_eq!(keys.next().as_deref(), Some("a"));
    assert_eq!(pairs.next(), Some(("a".into(), 1)));
    b.undo();
    entries.insert("b", 3).unwrap();
    assert_eq!(keys.next().as_deref(), Some("b"));
    assert_eq!(pairs.next(), Some(("b".into(), 3)));
    assert_eq!(keys.next(), None);
    assert_eq!(pairs.next(), None);
    entries.insert("c", 4).unwrap();
    assert_eq!(keys.next(), None);
    assert_eq!(pairs.next(), None);
}

#[tokio::test(start_paused = true)]
async fn actions_can_reenter_the_registry() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let layers = ScopedLayers::new(TestLayer::new, || {});
    let scope = mint(&ctx, &ScopeKey::new()).await;
    let nested = layers.clone();
    let outer = layers
        .effect(
            &scope.ctx,
            |layer| {
                let selected = scope_of(&scope.ctx).unwrap();
                assert!(nested.peek(Some(&selected)).is_some());
                let inner = nested.effect(
                    &scope.ctx,
                    |layer| layer.named.insert("inner", 1),
                    quiet("inner"),
                )?;
                let undo = layer.named.insert("outer", 2)?;
                assert_eq!(nested.merge(Some(&selected), |layer| &layer.named).len(), 2);
                // Keep the independently owned nested effect on the scope.
                drop(inner);
                Ok(undo)
            },
            quiet("outer"),
        )
        .unwrap();
    outer.dispose().await;
    assert_eq!(
        entries(&layers, scope_of(&scope.ctx).as_ref()),
        [("inner".into(), 1)]
    );
    scope.dispose().await;
    assert!(layers.peek(scope_of(&scope.ctx).as_ref()).is_none());
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn kernel_retains_the_effect_label_for_cleanup_diagnostics() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let layers = ScopedLayers::new(TestLayer::new, || {});
    let dispose = layers
        .effect(
            &ctx,
            |_| Ok(EntryUndo::new(|| panic!("test cleanup panic"))),
            quiet("store.label"),
        )
        .unwrap();
    dispose.dispose().await;
    assert_eq!(
        ctx.logger().buffer().last().unwrap().message,
        "effect \"store.label\": cleanup panicked"
    );
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn inactive_contexts_do_not_call_the_factory_or_action() {
    let kernel = Kernel::start();
    let scope = mint(&kernel.root(), &ScopeKey::new()).await;
    let created = Arc::new(AtomicUsize::new(0));
    let count = created.clone();
    let layers = ScopedLayers::new(
        move |scope| {
            count.fetch_add(1, Ordering::SeqCst);
            TestLayer::new(scope)
        },
        || {},
    );
    scope.dispose().await;
    let result = layers.effect(
        &scope.ctx,
        |_| panic!("inactive action must not run"),
        quiet("inactive"),
    );
    assert!(result.is_err());
    assert_eq!(created.load(Ordering::SeqCst), 1);
    assert!(layers.peek(scope_of(&scope.ctx).as_ref()).is_none());
    kernel.shutdown().await;
}
