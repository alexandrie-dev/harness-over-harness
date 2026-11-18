mod support;
use hoh_kernel::{Cfg, Context, Error, Plugin, Volatile, VolatileUpdate};
use hoh_loader::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use support::live::*;

async fn update(world: &World, config: Value) -> Vec<EntryChange> {
    world
        .patch(&format!("- id: x\n  config: {}\n", config))
        .await
        .unwrap()
}
fn live(world: &World) -> Value {
    (*world.row("x").fiber.unwrap().effective_config().unwrap()).clone()
}

#[tokio::test(start_paused = true)]
async fn updates_a_whole_config_reference_and_reports_the_root_path() {
    struct Whole;
    impl Plugin for Whole {
        const NAME: &'static str = "whole";
        type Config = Volatile<Value>;
        type Deps = ();
        async fn apply(ctx: Context, _: (), cfg: Cfg<Self::Config>) -> Result<(), Error> {
            let observed = ctx.tag::<Observed>().unwrap();
            let id = ctx.fiber().uid();
            ctx.on::<VolatileUpdate>(move |event| {
                locked(&observed.events).push((
                    id,
                    event.paths.clone(),
                    serde_json::to_value(&*cfg.get()).unwrap(),
                ))
            })?;
            Ok(())
        }
    }
    let world = World::registry("- {id: x, name: whole, config: {title: first}}", |r| {
        r.add::<Whole>("whole");
    })
    .await;
    let uid = world.row("x").fiber.unwrap().uid();
    update(&world, json!({"title":"second"})).await;
    assert_eq!(world.row("x").fiber.unwrap().uid(), uid);
    assert_eq!(
        *locked(&world.observed.events),
        vec![(uid, vec!["".into()], json!({"title":"second"}))]
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn commits_all_references_before_one_local_event_without_remounting() {
    let world = World::new(
        "- {id: x, name: objects, config: {}}\n- {id: other, name: objects, config: {}}",
    )
    .await;
    let first = locked(&world.observed.configs)[0].clone();
    let other = locked(&world.observed.configs)[1].clone();
    let uid = world.row("x").fiber.unwrap().uid();
    update(&world, json!({"title":"hello"})).await;
    // R53 replaces the Cfg snapshot and emits even when its changed-path list is empty.
    assert_eq!(locked(&world.observed.events)[0].1, Vec::<String>::new());
    locked(&world.observed.events).clear();
    update(&world, json!({"title":"next","nested":{"count":2}})).await;
    assert_eq!(locked(&world.observed.configs).len(), 2);
    assert_eq!(first.get().title.0, "next");
    assert_eq!(other.get().title.0, "hello");
    assert_eq!(
        locked(&world.observed.events)[0],
        (
            uid,
            vec!["title".into(), "nested.count".into()],
            live(&world)
        )
    );
    update(&world, json!({"title":"next","nested":{"count":2}})).await;
    assert_eq!(locked(&world.observed.events).len(), 1);
    update(&world, json!({"optional":"added"})).await;
    assert_eq!(first.get().optional.0.as_deref(), Some("added"));
    update(&world, json!({})).await;
    assert_eq!(first.get().optional.0, None);
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn keeps_references_on_invalid_updates_and_replaces_them_on_mixed_updates() {
    let world = World::new("- {id: x, name: objects, config: {}}").await;
    let old = locked(&world.observed.configs)[0].clone();
    update(
        &world,
        json!({"title":"invalid candidate","nested":{"count":-1}}),
    )
    .await;
    assert_eq!(old.get().title.0, "hello");
    assert!(locked(&world.observed.events).is_empty());
    update(&world, json!({"title":"new","fixed":"restart"})).await;
    assert_eq!(locked(&world.observed.configs).len(), 2);
    assert_eq!(locked(&world.observed.disposed).len(), 1);
    assert!(locked(&world.observed.events).is_empty());
    assert_eq!(old.get().title.0, "hello");
    assert_eq!(live(&world)["title"], "new");
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn bypasses_custom_update_hooks_for_volatile_changes_and_retains_them_for_mixed_changes() {
    let world = World::new("- {id: x, name: custom, config: {}}").await;
    update(&world, json!({"title":"accepted"})).await;
    update(&world, json!({"title":"accepted"})).await;
    assert!(locked(&world.observed.hooks).is_empty());
    let previous = locked(&world.observed.configs)[0].get();
    update(&world, json!({"title":"custom","fixed":"changed"})).await;
    assert_eq!(locked(&world.observed.hooks).len(), 1);
    assert_eq!(locked(&world.observed.hooks)[0]["title"], "custom");
    assert_eq!(previous.title.0, "accepted");
    assert_eq!(live(&world)["title"], "custom");
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn compares_ordinary_dates_and_arrays_and_retains_the_existing_nonvolatile_restart_behavior()
{
    #[derive(Serialize, Deserialize, schemars::JsonSchema)]
    struct Config {
        live: Volatile<String>,
        date: String,
        list: Vec<String>,
    }
    struct Dates;
    impl Plugin for Dates {
        const NAME: &'static str = "dates";
        type Config = Config;
        type Deps = ();
        async fn apply(ctx: Context, _: (), _: Cfg<Config>) -> Result<(), Error> {
            let state = ctx.tag::<Observed>().unwrap();
            locked(&state.values).push(json!("mounted"));
            Ok(())
        }
    }
    let world = World::registry(
        "- {id: x, name: dates, config: {live: a, date: '2026-01-01', list: [a]}}",
        |r| {
            r.add::<Dates>("dates");
        },
    )
    .await;
    update(&world, json!({"live":"b","date":"2026-01-01","list":["a"]})).await;
    assert_eq!(locked(&world.observed.values).len(), 1);
    update(&world, json!({"live":"c","date":"2026-01-02","list":["a"]})).await;
    update(&world, json!({"live":"d","date":"2026-01-02","list":["b"]})).await;
    assert_eq!(locked(&world.observed.values).len(), 3);
    let fiber = world.row("x").fiber.unwrap();
    fiber.update((*fiber.raw_config()).clone()).await.unwrap();
    assert_eq!(locked(&world.observed.values).len(), 4);
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn resolves_pending_updates_and_creates_fresh_references_after_dependency_replacement() {
    let world = World::new("- {id: x, name: objects, inject: [ready], config: {}}").await;
    assert!(locked(&world.observed.configs).is_empty());
    update(&world, json!({"title":"pending"})).await;
    hoh_kernel::service!(Ready: () = "ready");
    let provider = world.ctx.provide::<Ready>(Arc::new(())).await.unwrap();
    world.loader.settled().await;
    let old = locked(&world.observed.configs)[0].clone();
    assert_eq!(old.get().title.0, "pending");
    provider.dispose().await;
    world.loader.settled().await;
    update(&world, json!({"title":"restored"})).await;
    world.ctx.provide::<Ready>(Arc::new(())).await.unwrap();
    world.loader.settled().await;
    assert_eq!(locked(&world.observed.configs).len(), 2);
    assert_eq!(old.get().title.0, "pending");
    assert_eq!(live(&world)["title"], "restored");
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn reloads_real_yaml_and_persists_plain_values_independently_of_custom_update_hooks() {
    let world = World::new("- {id: x, name: custom, config: {title: old}}").await;
    std::fs::write(
        world.dir.path().join("cordis.yml"),
        "- {id: x, name: custom, config: {title: yaml}}\n",
    )
    .unwrap();
    world.loader.refresh("include").await.unwrap();
    assert_eq!(live(&world)["title"], "yaml");
    assert!(locked(&world.observed.hooks).is_empty());
    assert!(
        std::fs::read_to_string(world.dir.path().join("cordis.yml"))
            .unwrap()
            .contains("title: yaml")
    );
    world.close().await;
}
async fn subtree(kind: &str) {
    let world = World::new("[]").await;
    std::fs::write(
        world.dir.path().join("nested.yml"),
        "- {id: x, name: objects, config: {title: initial}}\n",
    )
    .unwrap();
    let config = |title: &str, fixed: &str| {
        if kind == "group" {
            json!([{"id":"x","name":"objects","config":{"title":title,"fixed":fixed}}])
        } else {
            json!({"path":"./nested.yml","patches":[{"id":"x","config":{"title":title,"fixed":fixed}}]})
        }
    };
    let inserted = |title: &str, fixed: &str| {
        format!(
            "- insert: [{}]\n",
            json!({"id":"parent","name":format!("cordis:{kind}"),"group":kind=="group","config":config(title,fixed)})
        )
    };
    world.patch(&inserted("initial", "fixed")).await.unwrap();
    let parent = world.row("parent").fiber.unwrap().uid();
    let child = world.row("x").fiber.unwrap().uid();
    world.patch(&inserted("first", "fixed")).await.unwrap();
    world.patch(&inserted("second", "fixed")).await.unwrap();
    assert_eq!(world.row("parent").fiber.unwrap().uid(), parent);
    assert_eq!(world.row("x").fiber.unwrap().uid(), child);
    assert_eq!(locked(&world.observed.configs).len(), 1);
    assert_eq!(locked(&world.observed.events).len(), 2);
    world.patch(&inserted("mixed", "changed")).await.unwrap();
    assert_eq!(locked(&world.observed.configs).len(), 2);
    assert_eq!(locked(&world.observed.disposed).len(), 1);
    assert_eq!(locked(&world.observed.events).len(), 2);
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn updates_volatile_children_through_group_and_include_custom_updates_and_saves_ordinary_config()
 {
    subtree("group").await;
    subtree("include").await;
}
#[tokio::test(start_paused = true)]
async fn applies_profile_patches_to_a_group_subtree_without_hmr_and_keeps_mixed_updates_on_the_normal_lifecycle()
 {
    subtree("group").await;
}
#[tokio::test(start_paused = true)]
async fn applies_profile_patches_to_an_include_subtree_without_hmr_and_keeps_mixed_updates_on_the_normal_lifecycle()
 {
    subtree("include").await;
}
#[tokio::test(start_paused = true)]
async fn commits_volatile_edits_in_place_and_retains_direct_fiber_updates() {
    let world = World::new("- {id: x, name: objects, config: {}}").await;
    let fiber = world.row("x").fiber.unwrap();
    update(&world, json!({"title":"enabled"})).await;
    assert_eq!(locked(&world.observed.configs).len(), 1);
    update(&world, json!({"title":"mixed","fixed":"changed"})).await;
    assert_eq!(locked(&world.observed.configs).len(), 2);
    fiber.update(json!({"title":"direct"})).await.unwrap();
    world.loader.settled().await;
    assert_eq!(locked(&world.observed.configs).len(), 3);
    assert_eq!(live(&world)["title"], "direct");
    assert_eq!(locked(&world.observed.events).len(), 1);
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn parses_a_volatile_only_update_through_the_config_hook_without_patching_the_entry_context()
{
    struct Hooked;
    impl Plugin for Hooked {
        const NAME: &'static str = "hooked";
        type Config = LiveConfig;
        type Deps = ();
        async fn apply(ctx: Context, _: (), config: Cfg<LiveConfig>) -> Result<(), Error> {
            Objects::apply(ctx.clone(), (), config).await?;
            let state = ctx.tag::<Observed>().unwrap();
            ctx.on_candidate::<LiveConfig>(move |config| {
                locked(&state.hooks).push(json!(config.title.0));
                Ok(())
            })?;
            Ok(())
        }
    }
    let world = World::registry("- {id: x, name: hooked, config: {title: initial}}", |r| {
        r.add::<Hooked>("hooked");
    })
    .await;
    let uid = world.row("x").fiber.unwrap().uid();
    update(&world, json!({"title":"changed"})).await;
    assert_eq!(*locked(&world.observed.hooks), vec![json!("changed")]);
    assert_eq!(locked(&world.observed.events).len(), 1);
    assert_eq!(world.row("x").fiber.unwrap().uid(), uid);
    world.close().await;
}
async fn isolation(available: bool) {
    hoh_kernel::service!(Ready: () = "ready");
    struct Provider;
    impl Plugin for Provider {
        const NAME: &'static str = "provider";
        type Config = Value;
        type Deps = ();
        async fn apply(ctx: Context, _: (), _: Cfg<Value>) -> Result<(), Error> {
            ctx.provide::<Ready>(Arc::new(())).await?;
            Ok(())
        }
    }
    let world=World::registry("- {id: p, name: provider, isolate: {ready: a}}\n- {id: x, name: objects, inject: [ready], isolate: {ready: a}, config: {title: old}}",|r|{r.add::<Provider>("provider");}).await;
    let mut text = "- {id: x, isolate: {ready: b}, config: {title: new}}\n".to_owned();
    if available {
        text.push_str("- insert: [{id: p2, name: provider, isolate: {ready: b}}]\n");
    }
    let old = locked(&world.observed.configs)[0].clone();
    world.patch(&text).await.unwrap();
    assert_eq!(old.get().title.0, "old");
    assert!(locked(&world.observed.events).is_empty());
    assert_eq!(
        locked(&world.observed.configs).len(),
        if available { 2 } else { 1 }
    );
    if !available {
        text.push_str("- insert: [{id: p2, name: provider, isolate: {ready: b}}]\n");
        world.patch(&text).await.unwrap();
    }
    assert_eq!(locked(&world.observed.configs).len(), 2);
    assert_eq!(live(&world)["title"], "new");
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn keeps_old_references_untouched_when_isolation_replaces_a_dependency_available_true() {
    isolation(true).await;
}
#[tokio::test(start_paused = true)]
async fn keeps_old_references_untouched_when_isolation_replaces_a_dependency_available_false() {
    isolation(false).await;
}
#[tokio::test(start_paused = true)]
async fn logs_an_invalid_candidate_and_keeps_its_references_while_other_entries_commit() {
    let world = World::new(
        "- {id: x, name: objects, config: {}}\n- {id: healthy, name: objects, config: {}}",
    )
    .await;
    world.patch("- {id: x, config: {title: invalid, nested: {count: -1}}}\n- {id: healthy, config: {title: healthy}}").await.unwrap();
    assert_eq!(live(&world)["title"], "hello");
    assert_eq!(
        *world.row("x").fiber.unwrap().raw_config(),
        json!({"title":"invalid","nested":{"count":-1}})
    );
    assert_eq!(locked(&world.observed.events).len(), 1);
    assert_eq!(locked(&world.observed.events)[0].2["title"], "healthy");
    assert!(
        world
            .ctx
            .logger()
            .buffer()
            .iter()
            .any(|r| r.message == "volatile config update failed for x")
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn keeps_an_ordinary_plugin_mounted_when_an_absent_object_becomes_its_explicit_default() {
    #[derive(Serialize, Deserialize, schemars::JsonSchema)]
    struct OrdinaryNested {
        #[serde(default = "ordinary_default")]
        fixed: String,
    }
    fn ordinary_default() -> String {
        "default".into()
    }
    impl OrdinaryNested {
        fn source_default() -> Self {
            Self {
                fixed: ordinary_default(),
            }
        }
    }
    #[derive(Serialize, Deserialize, schemars::JsonSchema)]
    struct Ordinary {
        #[serde(default = "OrdinaryNested::source_default")]
        #[schemars(schema_with = "inline_ordinary_schema")]
        // Source inline z.object(...).default(...).
        nested: OrdinaryNested,
    }
    // The source object has default({fixed:'default'}).
    impl Default for OrdinaryNested {
        fn default() -> Self {
            Self::source_default()
        }
    }
    fn inline_ordinary_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <OrdinaryNested as schemars::JsonSchema>::json_schema(generator)
    }
    struct OrdinaryPlugin;
    impl Plugin for OrdinaryPlugin {
        const NAME: &'static str = "ordinary-default";
        type Config = Ordinary;
        type Deps = ();
        async fn apply(ctx: Context, _: (), _: Cfg<Ordinary>) -> Result<(), Error> {
            locked(&ctx.tag::<Observed>().unwrap().values).push(json!("mounted"));
            Ok(())
        }
    }
    let world = World::registry("- {id: x, name: ordinary-default, config: {}}", |r| {
        r.add::<OrdinaryPlugin>("ordinary-default");
    })
    .await;
    let fiber = world.row("x").fiber.unwrap();
    let original = fiber.effective_config().unwrap();
    update(&world, json!({"nested":{"fixed":"default"}})).await;
    assert_eq!(locked(&world.observed.values).len(), 1);
    assert_eq!(*fiber.effective_config().unwrap(), *original);
    update(&world, json!({"nested":{"fixed":"changed"}})).await;
    assert_eq!(locked(&world.observed.values).len(), 2);
    assert_eq!(
        *fiber.effective_config().unwrap(),
        json!({"nested":{"fixed":"changed"}})
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn keeps_a_nosave_volatile_value_as_the_pending_raw_config_across_a_noop_entry_update() {
    let world = World::new("- {id: x, name: objects, config: {title: saved}}").await;
    let fiber = world.row("x").fiber.unwrap();
    fiber.update(json!({"title":"ephemeral"})).await.unwrap();
    update(&world, json!({"title":"saved"})).await;
    assert_eq!(*fiber.raw_config(), json!({"title":"ephemeral"}));
    assert_eq!(live(&world)["title"], "ephemeral");
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn isolates_a_failing_notification_listener_and_dispatches_with_a_context_receiver() {
    struct Bad;
    impl Plugin for Bad {
        const NAME: &'static str = "bad";
        type Config = LiveConfig;
        type Deps = ();
        async fn apply(ctx: Context, _: (), cfg: Cfg<LiveConfig>) -> Result<(), Error> {
            Objects::apply(ctx.clone(), (), cfg).await?;
            ctx.on::<VolatileUpdate>(|_| panic!("listener boom"))?;
            Ok(())
        }
    }
    let world = World::registry(
        "- {id: x, name: bad, config: {}}\n- {id: other, name: objects, config: {}}",
        |r| {
            r.add::<Bad>("bad");
        },
    )
    .await;
    update(&world, json!({"title":"committed"})).await;
    assert_eq!(live(&world)["title"], "committed");
    assert_eq!(locked(&world.observed.events).len(), 1);
    world
        .patch("- {id: x, config: {title: committed}}\n- {id: other, config: {title: unaffected}}")
        .await
        .unwrap();
    assert_eq!(locked(&world.observed.events).len(), 2);
    assert!(
        world
            .ctx
            .logger()
            .buffer()
            .iter()
            .any(|r| r.message.contains("listener panicked"))
    );
    world.close().await;
}

struct SourceKey;
impl hoh_kernel::ServiceKey for SourceKey {
    type Value = Mutex<Value>;
    const NAME: &'static str = "volatile-expression-source";
    fn view(value: &Self::Value) -> Option<Value> {
        Some(locked(value).clone())
    }
}
async fn raw_expression_edits(volatile: bool) {
    #[derive(Serialize, Deserialize, schemars::JsonSchema)]
    struct Ordinary {
        value: String,
    }
    #[derive(Serialize, Deserialize, schemars::JsonSchema)]
    struct Live {
        value: Volatile<String>,
    }
    struct OrdinaryPlugin;
    struct LivePlugin;
    impl Plugin for OrdinaryPlugin {
        const NAME: &'static str = "ordinary";
        type Config = Ordinary;
        type Deps = ();
        async fn apply(ctx: Context, _: (), cfg: Cfg<Ordinary>) -> Result<(), Error> {
            locked(&ctx.tag::<Observed>().unwrap().values).push(json!(cfg.get().value));
            Ok(())
        }
    }
    impl Plugin for LivePlugin {
        const NAME: &'static str = "live";
        type Config = Live;
        type Deps = ();
        async fn apply(ctx: Context, _: (), cfg: Cfg<Live>) -> Result<(), Error> {
            locked(&ctx.tag::<Observed>().unwrap().values).push(json!(cfg.get().value.0));
            Ok(())
        }
    }
    let world = World::registry("[]", |r| {
        r.add::<OrdinaryPlugin>("ordinary")
            .add::<LivePlugin>("live");
    })
    .await;
    let provider = world
        .ctx
        .provide::<SourceKey>(Arc::new(Mutex::new(json!({"a":"same","b":"same"}))))
        .await
        .unwrap();
    let inserted = |field: &str| {
        format!(
            "- insert: [{}]",
            json!({"id":"x","name":if volatile{"live"}else{"ordinary"},"inject":["volatile-expression-source"],"config":{"value":{"__jsExpr":format!("ctx.get('volatile-expression-source')?.{field}")}}})
        )
    };
    world.patch(&inserted("a")).await.unwrap();
    world.patch(&inserted("b")).await.unwrap();
    assert_eq!(
        locked(&world.observed.values).len(),
        if volatile { 1 } else { 2 }
    );
    assert_eq!(
        world.row("x").fiber.unwrap().raw_config()["value"]["__jsExpr"],
        "ctx.get('volatile-expression-source')?.b"
    );
    provider.dispose().await;
    world.loader.settled().await;
    world
        .ctx
        .provide::<SourceKey>(Arc::new(Mutex::new(
            json!({"a":"old-route","b":"new-route"}),
        )))
        .await
        .unwrap();
    world.loader.settled().await;
    assert_eq!(
        locked(&world.observed.values).last(),
        Some(&json!("new-route"))
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn retains_raw_expression_edits_for_dependency_restoration_volatile_false() {
    raw_expression_edits(false).await;
}
#[tokio::test(start_paused = true)]
async fn retains_raw_expression_edits_for_dependency_restoration_volatile_true() {
    raw_expression_edits(true).await;
}
#[tokio::test(start_paused = true)]
async fn remounts_when_changed_expressions_resolve_to_ordinary_values_different_from_the_live_config()
 {
    let world = World::new("[]").await;
    let data = Arc::new(Mutex::new(json!({"a":"initial","b":"initial"})));
    world.ctx.provide::<SourceKey>(data.clone()).await.unwrap();
    let config = |field: &str, title: &str| {
        format!(
            "- insert: [{}]",
            json!({"id":"x","name":"objects","config":{"title":title,"fixed":{"__jsExpr":format!("ctx.get('volatile-expression-source')?.{field}")}}})
        )
    };
    world.patch(&config("a", "old")).await.unwrap();
    *locked(&data) = json!({"a":"changed","b":"changed"});
    world.patch(&config("b", "new")).await.unwrap();
    assert_eq!(locked(&world.observed.configs).len(), 2);
    assert!(locked(&world.observed.events).is_empty());
    assert_eq!(live(&world)["fixed"], "changed");
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn remounts_when_an_ordinary_parent_expression_removes_a_live_reference_path() {
    #[derive(Serialize, Deserialize, schemars::JsonSchema)]
    struct Block {
        value: Volatile<String>,
    }
    #[derive(Serialize, Deserialize, schemars::JsonSchema)]
    struct Config {
        #[serde(default)]
        block: Option<Block>,
        live: Volatile<String>,
    }
    struct Consumer;
    impl Plugin for Consumer {
        const NAME: &'static str = "block";
        type Config = Config;
        type Deps = ();
        async fn apply(ctx: Context, _: (), cfg: Cfg<Config>) -> Result<(), Error> {
            let state = ctx.tag::<Observed>().unwrap();
            locked(&state.values).push(serde_json::to_value(&*cfg.get()).unwrap());
            ctx.on::<VolatileUpdate>(move |_| {
                locked(&state.hooks).push(json!("event"));
            })?;
            Ok(())
        }
    }
    let world = World::registry("[]", |r| {
        r.add::<Consumer>("block");
    })
    .await;
    let data = Arc::new(Mutex::new(json!({"block":{"value":"old"}})));
    world.ctx.provide::<SourceKey>(data.clone()).await.unwrap();
    let config = |live: &str| {
        format!(
            "- insert: [{}]",
            json!({"id":"x","name":"block","config":{"block":{"__jsExpr":"ctx.get('volatile-expression-source')?.block"},"live":live}})
        )
    };
    world.patch(&config("old")).await.unwrap();
    *locked(&data) = json!({});
    world.patch(&config("new")).await.unwrap();
    assert_eq!(locked(&world.observed.values).len(), 2);
    assert!(locked(&world.observed.hooks).is_empty());
    assert!(live(&world)["block"].is_null());
    world.close().await;
}
