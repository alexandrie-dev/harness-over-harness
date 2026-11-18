mod support;
use hoh_kernel::{FiberState, Kernel};
use hoh_loader::*;
use serde_json::json;
use std::sync::Arc;
use support::{live::*, *};

#[tokio::test(start_paused = true)]
async fn mounts_rows_disabled_and_unresolved_siblings_and_retains_raw_options() {
    let world = World::new("- {id: good, name: noop, config: {value: 1}}\n- {id: off, name: missing, disabled: true}\n- {id: bad, name: missing}\n").await;
    assert!(world.active("good"));
    assert_eq!(world.row("off").status, EntryStatus::Disabled);
    assert_eq!(world.row("bad").status, EntryStatus::Unresolved);
    let audit = world.loader.audit(&["good", "absent"]);
    assert_eq!(audit.missing, ["absent"]);
    assert_eq!(audit.unresolved, ["include:bad"]);
    assert!(audit.inactive.is_empty());
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn duplicate_explicit_ids_reuse_one_entry_and_the_last_options_win() {
    let world = World::new("- {id: same, name: noop, config: {value: first}}\n- {id: same, name: noop, config: {value: last}}\n").await;
    let same: Vec<_> = world
        .loader
        .entries()
        .into_iter()
        .filter(|e| e.options.id == "same")
        .collect();
    assert_eq!(same.len(), 1);
    assert_eq!(same[0].options.config, json!({"value":"last"}));
    assert_eq!(locked(&world.observed.values).len(), 1);
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn literal_colons_do_not_collide_with_an_included_tree_path() {
    let world = World::new("- {id: 'nested:leaf', name: noop, config: {value: literal}}\n").await;
    std::fs::write(
        world.dir.path().join("child.yml"),
        "- {id: leaf, name: noop, config: {value: nested}}\n",
    )
    .unwrap();
    world
        .patch("- insert: [{id: nested, name: cordis:include, config: {path: ./child.yml}}]\n")
        .await
        .unwrap();
    let leaves: Vec<_> = world
        .loader
        .entries()
        .into_iter()
        .filter(|entry| entry.id == "include:nested:leaf")
        .collect();
    assert_eq!(leaves.len(), 2);
    let literal = leaves
        .iter()
        .find(|entry| entry.options.id == "nested:leaf")
        .unwrap()
        .fiber
        .clone()
        .unwrap();
    let nested = world
        .loader
        .resolve("include:nested:leaf")
        .unwrap()
        .fiber
        .unwrap();
    assert_ne!(literal.uid(), nested.uid());
    assert_eq!(literal.state(), FiberState::Active);
    assert_eq!(nested.state(), FiberState::Active);
    world.loader.remove("include:nested:leaf").await.unwrap();
    assert!(nested.is_disposed());
    assert_eq!(literal.state(), FiberState::Active);
    assert!(world.loader.resolve("include:nested:leaf").is_none());
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn removes_an_addressable_child_of_a_group_with_a_literal_colon_id() {
    let world = World::new("- id: 'literal:group'\n  name: cordis:group\n  group: true\n  config: [{id: child, name: noop}]\n").await;
    assert!(world.loader.resolve("include:literal:group").is_none());
    let child = world
        .loader
        .resolve("include:child")
        .unwrap()
        .fiber
        .unwrap();
    world.loader.remove("include:child").await.unwrap();
    assert!(child.is_disposed());
    assert!(world.loader.resolve("include:child").is_none());
    assert_eq!(world.loader.entries().len(), 2);
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn group_reconciliation_keeps_the_fiber_and_updates_children() {
    let world = World::new("- id: group\n  name: cordis:group\n  group: true\n  config:\n    - {id: x, name: noop, config: {value: old}}\n").await;
    let parent = world.row("group").fiber.unwrap().uid();
    let child = world.row("x").fiber.unwrap().uid();
    world
        .patch("- id: x\n  config: {value: new}\n")
        .await
        .unwrap();
    assert_eq!(world.row("group").fiber.unwrap().uid(), parent);
    assert_eq!(world.row("x").fiber.unwrap().uid(), child);
    assert_eq!(world.row("x").options.config, json!({"value":"new"}));
    world.patch("- id: group\n  config: []\n").await.unwrap();
    assert!(world.loader.resolve("include:x").is_none());
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn standalone_group_accepts_builtin_groups_and_stops_with_its_owner() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let group = Group::mount(
        &ctx,
        Arc::new(PluginRegistry::default()),
        env(std::path::Path::new("/fixtures")),
    )
    .unwrap();
    group
        .update(entries(
            "- {id: g, name: cordis:group, group: true, config: []}",
        ))
        .await;
    let fiber = group.entries()[0].fiber.clone().unwrap();
    fiber.settled().await.unwrap();
    assert_eq!(fiber.state(), FiberState::Active);
    kernel.shutdown().await;
    assert!(group.entries().is_empty());
}
#[tokio::test(start_paused = true)]
async fn name_and_context_changes_rebuild_but_configuration_updates_reuse_fibers() {
    let world = World::new("- {id: x, name: noop, config: {value: 1}}\n").await;
    let old = world.row("x").fiber.unwrap();
    let changes = world
        .patch("- {id: x, config: {value: 2}}\n")
        .await
        .unwrap();
    assert!(changes.contains(&EntryChange::Updated(
        "include:x".into(),
        UpdateAction::Config
    )));
    assert_eq!(world.row("x").fiber.unwrap().uid(), old.uid());
    let changes = world
        .patch("- {id: x, isolate: {agents: private}, config: {value: 3}}\n")
        .await
        .unwrap();
    assert!(changes.contains(&EntryChange::Updated(
        "include:x".into(),
        UpdateAction::Rebuilt
    )));
    assert!(old.is_disposed());
    assert_ne!(world.row("x").fiber.unwrap().uid(), old.uid());
    world.close().await;
}

#[derive(Default)]
struct Gate {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
hoh_kernel::service!(GateKey: Gate = "loadingGate");
struct Loading;
impl hoh_kernel::Plugin for Loading {
    const NAME: &'static str = "loading";
    type Config = serde_json::Value;
    type Deps = ();
    async fn apply(
        ctx: hoh_kernel::Context,
        _: (),
        config: hoh_kernel::Cfg<Self::Config>,
    ) -> Result<(), hoh_kernel::Error> {
        if config.get()["value"] == "first" {
            let gate = ctx.get::<GateKey>().unwrap();
            gate.entered.notify_one();
            gate.release.notified().await;
        }
        Ok(())
    }
}
async fn overlap() {
    let world = World::registry("- {id: x, name: loading, config: {value: base}}", |r| {
        r.add::<Loading>("loading");
    })
    .await;
    let gate = Arc::new(Gate::default());
    world.ctx.provide::<GateKey>(gate.clone()).await.unwrap();
    let original = world.row("x").fiber.unwrap().uid();
    let first = world.patch("- {id: x, config: {value: first}}");
    tokio::pin!(first);
    tokio::select! {result=&mut first=>panic!("activation was not held: {result:?}"),_=gate.entered.notified()=>{}}
    let second = world.patch("- {id: x, config: {value: second}}");
    tokio::pin!(second);
    tokio::select! {result=&mut second=>panic!("second update finished before the pending activation drained: {result:?}"),_=async{loop{if world.row("x").options.config==json!({"value":"second"}){break;}tokio::task::yield_now().await;}}=>{}}
    gate.release.notify_one();
    let (first, second) = tokio::join!(first, second);
    first.unwrap();
    second.unwrap();
    world.loader.settled().await;
    let row = world.row("x");
    assert_eq!(row.fiber.as_ref().unwrap().uid(), original);
    assert_eq!(row.options.config, json!({"value":"second"}));
    assert_eq!(
        *row.fiber.unwrap().effective_config().unwrap(),
        json!({"value":"second"})
    );
    assert!(world.loader.dump().contains("second"));
    assert!(world.loader.audit(&[]).inactive.is_empty());
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn recomposition_while_rows_are_loading_keeps_the_latest_generation() {
    overlap().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recomposition_while_rows_are_loading_keeps_the_latest_generation_multithreaded() {
    tokio::time::timeout(std::time::Duration::from_secs(10), overlap())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_creates_are_atomic_and_do_not_drop_sibling_rows() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let world = World::new("[]").await;
        let barrier = Arc::new(tokio::sync::Barrier::new(33));
        let futures: Vec<_> = (0..32)
            .map(|i| {
                let loader = world.loader.clone();
                let barrier = barrier.clone();
                tokio::spawn(async move {
                    barrier.wait().await;
                    loader
                        .create(
                            entries(&format!("- {{id: row-{i}, name: noop}}\n")).remove(0),
                            None,
                        )
                        .await
                })
            })
            .collect();
        barrier.wait().await;
        for result in futures_util::future::join_all(futures).await {
            result.unwrap().unwrap();
        }
        world.loader.settled().await;
        assert_eq!(world.loader.entries().len(), 33);
        assert!(world.loader.audit(&[]).inactive.is_empty());
        world.close().await;
    })
    .await
    .unwrap();
}
