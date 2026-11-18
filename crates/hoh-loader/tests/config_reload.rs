mod support;
use hoh_kernel::{Cfg, Context, Error, Plugin};
use hoh_loader::*;
use serde_json::{Value, json};
use std::sync::Arc;
use support::live::*;
#[tokio::test(start_paused = true)]
async fn writes_and_activates_initial_entries_when_an_included_file_is_missing() {
    let world=World::new("- id: initialized\n  name: cordis:include\n  config:\n    path: ./created.yml\n    initial:\n      - {id: noop, name: noop, config: {value: initial}}\n").await;
    assert_eq!(world.row("noop").options.config, json!({"value":"initial"}));
    assert!(world.active("noop"));
    assert!(
        std::fs::read_to_string(world.dir.path().join("created.yml"))
            .unwrap()
            .contains("id: noop")
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn keeps_the_last_good_tree_instead_of_throwing_then_applies_the_next_valid_edit() {
    let world = World::new("- {id: noop, name: noop, config: {value: 1}}\n").await;
    let path = world.dir.path().join("cordis.yml");
    for text in ["invalid: [unclosed\n", "", "null\n", "{}\n"] {
        std::fs::write(&path, text).unwrap();
        world.loader.refresh("include").await.unwrap();
        assert_eq!(world.row("noop").options.config, json!({"value":1}));
    }
    std::fs::write(&path, "- {id: noop, name: noop, config: {value: 2}}\n").unwrap();
    world.loader.refresh("include").await.unwrap();
    assert_eq!(world.row("noop").options.config, json!({"value":2}));
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn reapplies_entry_patches_and_inserted_entries_on_every_reread_parity_with_initial_load() {
    let world = World::new("[]").await;
    let path = world.dir.path().join("base.yml");
    std::fs::write(&path, "- {id: noop, name: noop, config: {value: base}}\n").unwrap();
    world.patch("- insert:\n    - id: base\n      name: cordis:include\n      config:\n        path: ./base.yml\n        patches:\n          - {id: noop, config: {value: patched}}\n          - insert: [{id: extra, name: noop}]\n").await.unwrap();
    for value in ["edited", "again"] {
        std::fs::write(
            &path,
            format!("- {{id: noop, name: noop, config: {{value: {value}}}}}\n"),
        )
        .unwrap();
        world.loader.refresh("include:base").await.unwrap();
        assert_eq!(world.row("noop").options.config, json!({"value":"patched"}));
        assert!(world.active("extra"));
    }
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn lets_a_later_patch_configure_or_disable_a_row_an_earlier_patch_inserted() {
    let world = World::new("[]").await;
    let insert = "- insert: [{id: x, name: noop, config: {value: inserted}}]\n";
    world
        .patch(&format!(
            "{insert}- {{id: x, config: {{value: configured}}}}\n"
        ))
        .await
        .unwrap();
    assert_eq!(world.row("x").options.config, json!({"value":"configured"}));
    let old = world.row("x").fiber.unwrap();
    world
        .patch(&format!("{insert}- {{id: x, disabled: true}}\n"))
        .await
        .unwrap();
    assert!(old.is_disposed());
    assert_eq!(world.row("x").status, EntryStatus::Disabled);
    world.patch(insert).await.unwrap();
    assert!(world.active("x"));
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn keeps_the_previous_fiber_config_after_schema_rejection_and_retries_a_valid_edit() {
    let world = World::new("- {id: x, name: objects, config: {title: first}}\n").await;
    let old = world.row("x").fiber.unwrap();
    assert!(
        world
            .patch("- {id: x, config: {title: next, fixed: 5}}")
            .await
            .is_err()
    );
    assert_eq!(old.effective_config().unwrap()["title"], "first");
    world
        .patch("- {id: x, config: {title: next, fixed: valid}}")
        .await
        .unwrap();
    assert_eq!(old.effective_config().unwrap()["title"], "next");
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn lets_a_booted_composition_share_one_isolate_realm_across_a_group_of_rows() {
    struct Shared;
    impl hoh_kernel::ServiceKey for Shared {
        type Value = Value;
        const NAME: &'static str = "shared";
        fn view(value: &Value) -> Option<Value> {
            Some(value.clone())
        }
    }
    struct Provider;
    impl Plugin for Provider {
        const NAME: &'static str = "provider";
        type Config = Value;
        type Deps = ();
        async fn apply(ctx: Context, _: (), _: Cfg<Value>) -> Result<(), Error> {
            ctx.provide::<Shared>(Arc::new(json!({"value":"inside"})))
                .await?;
            Ok(())
        }
    }
    let world=World::registry("- id: g\n  name: cordis:group\n  group: true\n  isolate: {shared: true}\n  config:\n    - {id: p, name: provider}\n    - id: c\n      name: noop\n      inject: [shared]\n      config:\n        value: !!js ctx.get('shared')?.value\n- {id: outside, name: noop, inject: [shared]}\n",|r|{r.add::<Provider>("provider");}).await;
    assert!(world.active("c"));
    assert_eq!(
        world.row("c").fiber.unwrap().effective_config().unwrap()["value"],
        "inside"
    );
    assert!(!world.active("outside"));
    assert!(!world.ctx.has("shared"));
    world.close().await;
}

#[tokio::test(start_paused = true)]
async fn anonymous_rows_keep_their_ids_when_new_file_rows_appear_before_them() {
    let world=World::new("- {name: noop, config: {value: root}}\n- id: g\n  name: cordis:group\n  group: true\n  config:\n    - {name: noop, config: {value: child}}\n").await;
    let root = world
        .loader
        .entries()
        .into_iter()
        .find(|row| row.options.config == json!({"value":"root"}))
        .unwrap();
    let child = world
        .loader
        .entries()
        .into_iter()
        .find(|row| row.options.config == json!({"value":"child"}))
        .unwrap();
    std::fs::write(world.dir.path().join("cordis.yml"),"- {name: noop, config: {value: inserted-root}}\n- {name: noop, config: {value: root}}\n- id: g\n  name: cordis:group\n  group: true\n  config:\n    - {name: noop, config: {value: inserted-child}}\n    - {name: noop, config: {value: child}}\n").unwrap();
    world.loader.refresh("include").await.unwrap();
    for old in [root, child] {
        let current = world.loader.resolve(&old.id).unwrap();
        assert_eq!(current.options.config, old.options.config);
        assert_eq!(current.fiber.unwrap().uid(), old.fiber.unwrap().uid());
    }
    assert!(world.loader.dump().contains("inserted-root"));
    assert!(world.loader.dump().contains("inserted-child"));
    world.close().await;
}

#[tokio::test(start_paused = true)]
async fn keeps_siblings_after_a_required_id_failure_during_hmr() {
    hoh_kernel::service!(Ready: () = "reloadMissing");
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
    struct Conditional;
    impl Plugin for Conditional {
        const NAME: &'static str = "conditional";
        type Config = Value;
        type Deps = ();
        async fn apply(_: Context, _: (), cfg: Cfg<Value>) -> Result<(), Error> {
            if cfg.get()["fail"] == true {
                Err("reload sync failure".into())
            } else {
                Ok(())
            }
        }
    }
    struct AsyncConditional;
    impl Plugin for AsyncConditional {
        const NAME: &'static str = "async-conditional";
        type Config = Value;
        type Deps = ();
        async fn apply(_: Context, _: (), cfg: Cfg<Value>) -> Result<(), Error> {
            tokio::task::yield_now().await;
            if cfg.get()["fail"] == true {
                Err("reload async failure".into())
            } else {
                Ok(())
            }
        }
    }
    for (name, state, inject) in [
        ("missing", None, ""),
        ("conditional", Some(hoh_kernel::FiberState::Failed), ""),
        ("async", Some(hoh_kernel::FiberState::Failed), ""),
        (
            "noop",
            Some(hoh_kernel::FiberState::Pending),
            ", inject: [reloadMissing]",
        ),
    ] {
        let world = World::registry("- {id: good, name: noop}\n", |registry| {
            registry
                .add::<Conditional>("conditional")
                .add::<AsyncConditional>("async")
                .add::<Provider>("provider");
        })
        .await;
        let good = world.row("good").fiber.unwrap();
        let path = world.dir.path().join("cordis.yml");
        std::fs::write(&path,format!("- {{id: good, name: noop}}\n- {{id: webserver, name: {name}{inject}, config: {{fail: true}}}}\n")).unwrap();
        world.loader.refresh("include").await.unwrap();
        assert_eq!(
            world
                .row("webserver")
                .fiber
                .as_ref()
                .map(|fiber| fiber.state()),
            state
        );
        assert_eq!(good.state(), hoh_kernel::FiberState::Active);
        let recovery = if name == "missing" { "noop" } else { name };
        std::fs::write(&path,format!("- {{id: good, name: noop}}\n- {{id: webserver, name: {recovery}{inject}, config: {{fail: false}}}}\n- {{id: provider, name: provider}}\n")).unwrap();
        world.loader.refresh("include").await.unwrap();
        assert!(world.active("webserver"));
        assert_eq!(world.row("good").fiber.unwrap().uid(), good.uid());
        world.close().await;
    }
}
