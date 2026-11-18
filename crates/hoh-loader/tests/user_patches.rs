mod support;
use hoh_kernel::{Cfg, Context, Error, FiberState, Plugin, Undo};
use hoh_loader::*;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use support::live::*;
use support::*;
#[test]
fn returns_undefined_when_no_user_patch_file_exists() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &[]);
    let p = Profile::load(
        &dir,
        &Source(vec![]),
        &root.path().join("home"),
        &[],
        &env(root.path()),
    )
    .unwrap();
    assert!(p.layers.is_empty());
}
#[test]
fn parses_a_patch_list_and_preserves_js_expressions_as_loader_expression_nodes() {
    let p = patches(
        "- id: agent-loop\n  name: '@deepseek-ai/dsh-agent-loop'\n  config:\n    model: !!js process.env.DSH_SPEC_MODEL\n- insert: [{id: llm, name: '@deepseek-ai/dsh-llm-pi-ai'}]\n",
    );
    assert_eq!(p.len(), 2);
    assert_eq!(
        p[0].set["config"]["model"],
        json!({"__jsExpr":"process.env.DSH_SPEC_MODEL"})
    );
    assert_eq!(p[1].insert.as_ref().unwrap().len(), 1);
}
#[test]
fn anchors_inserted_relative_plugins_to_the_patch_file_and_keeps_assertion_names_literal() {
    let p = patches(
        "- id: existing\n  name: ./assertion.mjs\n- insert:\n    - id: rule\n      name: ./rule.mjs\n    - id: nested\n      name: cordis:group\n      group: true\n      config: [{id: child, name: ../child.mjs}]\n",
    );
    assert_eq!(p[0].name.as_deref(), Some("./assertion.mjs"));
    let rows = p[1].insert.as_ref().unwrap();
    assert_eq!(rows[0].name, "file:///fixtures/rule.mjs");
    assert_eq!(rows[1].config[0]["name"], json!("file:///child.mjs"));
}
#[tokio::test(start_paused = true)]
async fn loads_absolute_plugin_paths_from_patch_files_as_file_urls() {
    let p = patches(
        "- id: existing\n  name: '/absolute #100%.mjs'\n- insert: [{id: a, name: '/absolute #100%.mjs'}, {id: b, name: 'file:///a%20b.mjs'}, {id: c, name: '@dsh/p'}]\n",
    );
    assert_eq!(p[0].name.as_deref(), Some("/absolute #100%.mjs"));
    let rows = p[1].insert.as_ref().unwrap();
    assert_eq!(rows[0].name, "file:///absolute%20%23100%25.mjs");
    assert_eq!(rows[1].name, "file:///a%20b.mjs");
    assert_eq!(rows[2].name, "@dsh/p");
    let world = World::registry("[]", |r| {
        r.add::<Noop>("file:///absolute%20%23100%25.mjs")
            .add::<Noop>("file:///a%20b.mjs")
            .add::<Noop>("@dsh/p");
    })
    .await;
    world
        .loader
        .recompose(vec![Layer {
            source: LayerSource::Profile,
            file: None,
            patches: vec![p[1].clone()],
        }])
        .await
        .unwrap();
    for id in ["a", "b", "c"] {
        assert!(world.active(id));
    }
    world.close().await;
}
#[test]
fn fails_loud_on_an_unreadable_file_a_present_user_patch_layer_is_never_skipped() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &[]);
    std::fs::create_dir(dir.join("cordis.patch.yml")).unwrap();
    assert!(
        Profile::load(
            &dir,
            &Source(vec![]),
            &root.path().join("home"),
            &[],
            &env(root.path())
        )
        .is_err()
    );
}
#[test]
fn fails_loud_on_unparsable_yaml_and_on_a_js_tag_with_no_expression_body() {
    for (text, wanted_line, wanted_column, wanted_message) in [
        (
            "invalid: [unclosed\n",
            2,
            1,
            "while parsing a flow sequence, expected ',' or ']'",
        ),
        (
            "- id: x\n  config:\n    a: !!js\n",
            3,
            8,
            "!!js requires an expression body",
        ),
    ] {
        let error = parse_patches(text, Path::new("/bad.yml")).unwrap_err();
        match error {
            LoadError::Yaml {
                file,
                line,
                column,
                message,
            } => {
                assert_eq!(file, Path::new("/bad.yml"));
                assert_eq!((line, column), (wanted_line, wanted_column));
                assert_eq!(message, wanted_message);
            }
            _ => panic!("{error}"),
        }
    }
}
#[test]
fn fails_loud_when_the_file_is_not_a_top_level_array_or_an_entry_is_not_an_object() {
    for (text, wanted_column, wanted_message) in [
        (
            "id: not-a-list\n",
            1,
            "config file must be a top-level YAML array",
        ),
        ("- just-a-string\n", 3, "entry must be a mapping"),
        ("- []\n", 3, "entry must be a mapping"),
        ("- null\n", 3, "entry must be a mapping"),
    ] {
        let error = parse_patches(text, Path::new("/bad.yml")).unwrap_err();
        match error {
            LoadError::Shape {
                file,
                line,
                column,
                message,
            } => {
                assert_eq!(file, Path::new("/bad.yml"));
                assert_eq!((line, column), (1, wanted_column));
                assert_eq!(message, wanted_message);
            }
            _ => panic!("{error}"),
        }
    }
}
#[test]
fn missing_explicit_overlays_fail_instead_of_being_optional() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &[]);
    assert!(
        Profile::load(
            &dir,
            &Source(vec![]),
            &root.path().join("home"),
            &[root.path().join("missing.yml")],
            &env(root.path())
        )
        .is_err()
    );
}

#[tokio::test(start_paused = true)]
async fn keeps_includes_config_literal_a_nested_rows_js_belongs_to_that_rows_fiber() {
    let world=World::new("- id: carrier\n  name: cordis:include\n  config:\n    path: ./nested.yml\n    initial:\n      - id: child\n        name: noop\n        inject: [late]\n        config:\n          value: !!js ctx.get('late')?.value\n").await;
    assert_eq!(
        world.row("child").fiber.unwrap().state(),
        FiberState::Pending
    );
    struct Late;
    impl hoh_kernel::ServiceKey for Late {
        type Value = serde_json::Value;
        const NAME: &'static str = "late";
        fn view(value: &Self::Value) -> Option<Self::Value> {
            Some(value.clone())
        }
    }
    world
        .ctx
        .provide::<Late>(Arc::new(json!({"value":"ready"})))
        .await
        .unwrap();
    world.loader.settled().await;
    assert_eq!(
        world
            .row("child")
            .fiber
            .unwrap()
            .effective_config()
            .unwrap()["value"],
        "ready"
    );
    assert_eq!(
        world.row("carrier").options.config["initial"][0]["config"]["value"],
        json!({"__jsExpr":"ctx.get('late')?.value"})
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn waits_for_row_injections_before_resolving_js_and_resolves_again_after_provider_replacement()
 {
    struct Source;
    impl hoh_kernel::ServiceKey for Source {
        type Value = serde_json::Value;
        const NAME: &'static str = "source";
        fn view(v: &Self::Value) -> Option<Self::Value> {
            Some(v.clone())
        }
    }
    struct Reader;
    impl Plugin for Reader {
        const NAME: &'static str = "reader";
        type Config = serde_json::Value;
        type Deps = ();
        async fn apply(ctx: Context, _: (), cfg: Cfg<Self::Config>) -> Result<(), Error> {
            if cfg.get()["value"] == "rejected" {
                return Err("rejected provider".into());
            }
            Noop::apply(ctx, (), cfg).await
        }
    }
    let world=World::registry("- id: reader\n  name: reader\n  inject: [source]\n  config:\n    value: !!js ctx.get('source')?.value\n",|r|{r.add::<Reader>("reader");}).await;
    assert!(locked(&world.observed.values).is_empty());
    let first = world
        .ctx
        .provide::<Source>(Arc::new(json!({"value":"first"})))
        .await
        .unwrap();
    world.loader.settled().await;
    assert_eq!(
        locked(&world.observed.values).last().unwrap()["value"],
        "first"
    );
    first.dispose().await;
    world.loader.settled().await;
    let second = world
        .ctx
        .provide::<Source>(Arc::new(json!({"value":"second"})))
        .await
        .unwrap();
    world.loader.settled().await;
    assert_eq!(
        locked(&world.observed.values).last().unwrap()["value"],
        "second"
    );
    second.dispose().await;
    world.loader.settled().await;
    let rejected = world
        .ctx
        .provide::<Source>(Arc::new(json!({"value":"rejected"})))
        .await
        .unwrap();
    world.loader.settled().await;
    let fiber = world.row("reader").fiber.unwrap();
    assert!(
        fiber
            .settled()
            .await
            .unwrap_err()
            .to_string()
            .contains("rejected provider")
    );
    rejected.dispose().await;
    world
        .ctx
        .provide::<Source>(Arc::new(json!({"value":"recovered"})))
        .await
        .unwrap();
    world.loader.settled().await;
    assert_eq!(fiber.state(), FiberState::Failed);
    fiber.update((*fiber.raw_config()).clone()).await.unwrap();
    assert_eq!(
        locked(&world.observed.values).last().unwrap()["value"],
        "recovered"
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn evaluates_a_js_disabled_expression_against_the_loader_context() {
    let world=World::new("- id: off\n  name: noop\n  disabled: !!js process.platform === 'linux'\n- id: on\n  name: noop\n  disabled: !!js process.platform === 'win32'\n").await;
    assert!(world.row("off").disabled);
    assert!(world.row("off").fiber.is_none());
    assert!(world.active("on"));
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn keeps_the_raw_expression_in_the_options_so_writeback_preserves_the_js_form() {
    let world =
        World::new("- id: expr\n  name: noop\n  disabled: !!js process.platform === 'win32'\n")
            .await;
    assert_eq!(
        world.row("expr").options.disabled,
        json!({"__jsExpr":"process.platform === 'win32'"})
    );
    assert!(!world.row("expr").disabled);
    let dump = world.loader.dump();
    assert!(dump.contains("!!js"));
    assert_eq!(
        parse_entries(&dump, Path::new("/dump.yml")).unwrap()[0].disabled,
        world.row("expr").options.disabled
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn reevaluates_when_update_replaces_the_expression_mounting_and_unmounting() {
    let world = World::new("- {id: expr, name: noop}\n").await;
    let first = world.row("expr").fiber.unwrap();
    world
        .patch("- id: expr\n  disabled: !!js process.platform === 'linux'\n")
        .await
        .unwrap();
    assert!(first.is_disposed());
    assert!(world.row("expr").disabled);
    world
        .patch("- id: expr\n  disabled: !!js process.platform === 'win32'\n")
        .await
        .unwrap();
    assert!(world.active("expr"));
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn retains_unchanged_import_diagnostics_across_profile_reconciliation() {
    let world = World::new("- {id: missing, name: absent}\n").await;
    world.patch("[]").await.unwrap();
    assert!(
        world.ctx.logger().buffer().iter().any(
            |r| r.message == "missing (absent): failed to import: cannot resolve plugin absent"
        )
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn rejects_a_context_without_the_launcher_root_include() {
    let world = World::new("[]").await;
    world.loader.remove("include").await.unwrap();
    let error = world.patch("[]").await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "hoh: profile reload requires the root Include entry"
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn removes_a_previously_failed_entry_without_reporting_its_old_activation_error() {
    let world = World::new("[]").await;
    assert!(
        world
            .patch("- insert: [{id: candidate, name: fails}]")
            .await
            .is_err()
    );
    world.patch("[]").await.unwrap();
    world.patch("[]").await.unwrap();
    assert!(world.loader.resolve("include:candidate").is_none());
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn reports_an_unchanged_failed_entry_as_a_warning_but_rejects_a_changed_configuration_with_the_same_failure()
 {
    let world = World::new("- {id: candidate, name: fails, config: {revision: 1}}\n").await;
    world.patch("[]").await.unwrap();
    assert!(
        world
            .ctx
            .logger()
            .buffer()
            .iter()
            .any(|r| r.message == "candidate (fails): activation boom")
    );
    assert!(
        world
            .patch("- {id: candidate, config: {revision: 2}}")
            .await
            .unwrap_err()
            .to_string()
            .contains("activation boom")
    );
    world.close().await;
}
#[derive(Default)]
struct Probe {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
hoh_kernel::service!(ProbeKey: Probe = "reloadProbe");
struct WaitingFailure;
impl Plugin for WaitingFailure {
    const NAME: &'static str = "waiting-failure";
    type Config = serde_json::Value;
    type Deps = ();
    async fn apply(ctx: Context, _: (), cfg: Cfg<Self::Config>) -> Result<(), Error> {
        if cfg.get()["fail"] == true {
            let probe = ctx.get::<ProbeKey>().unwrap();
            probe.entered.notify_one();
            probe.release.notified().await;
            return Err("in-flight failure".into());
        }
        Ok(())
    }
}
struct Held;
impl Plugin for Held {
    const NAME: &'static str = "held";
    type Config = serde_json::Value;
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<Self::Config>) -> Result<(), Error> {
        let probe = ctx.get::<ProbeKey>().unwrap();
        ctx.effect("held resource", move || {
            Undo::future(move || async move {
                probe.entered.notify_one();
                probe.release.notified().await;
                Ok(())
            })
        })?;
        Ok(())
    }
}
async fn superseded() {
    let world = World::registry("[]", |r| {
        r.add::<WaitingFailure>("waiting-failure");
    })
    .await;
    let probe = Arc::new(Probe::default());
    world.ctx.provide::<ProbeKey>(probe.clone()).await.unwrap();
    world
        .patch("- insert: [{id: candidate, name: waiting-failure, config: {fail: false}}]")
        .await
        .unwrap();
    let fiber = world.row("candidate").fiber.unwrap();
    let updating = fiber.update(json!({"fail":true}));
    let updating = tokio::spawn(updating);
    probe.entered.notified().await;
    let removing = world.patch("[]");
    tokio::pin!(removing);
    tokio::select! {result=&mut removing=>panic!("removal unexpectedly finished before activation: {result:?}"),_ = async {loop {if world.loader.resolve("include:candidate").is_none(){break;}tokio::task::yield_now().await;}}=>{}}
    probe.release.notify_one();
    updating.await.unwrap().unwrap();
    removing.await.unwrap();
    assert!(world.loader.resolve("include:candidate").is_none());
    assert!(
        world
            .ctx
            .logger()
            .buffer()
            .iter()
            .any(|r| r.message.contains("in-flight failure"))
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn logs_a_superseded_activation_failure_without_failing_removal() {
    superseded().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logs_a_superseded_activation_failure_without_failing_removal_multithreaded() {
    tokio::time::timeout(std::time::Duration::from_secs(5), superseded())
        .await
        .unwrap();
}
async fn held() {
    let world = World::registry("[]", |r| {
        r.add::<Held>("held");
    })
    .await;
    let probe = Arc::new(Probe::default());
    world.ctx.provide::<ProbeKey>(probe.clone()).await.unwrap();
    world
        .patch("- insert: [{id: held, name: held}]")
        .await
        .unwrap();
    let removing = world.patch("[]");
    tokio::pin!(removing);
    tokio::select! {result=&mut removing=>panic!("removal finished before releasing the resource: {result:?}"),_=probe.entered.notified()=>{}}
    assert!(world.loader.resolve("include:held").is_none());
    probe.release.notify_one();
    removing.await.unwrap();
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn waits_for_a_removed_plugin_to_release_its_resources() {
    held().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waits_for_a_removed_plugin_to_release_its_resources_multithreaded() {
    tokio::time::timeout(std::time::Duration::from_secs(5), held())
        .await
        .unwrap();
}
#[tokio::test(start_paused = true)]
async fn applies_id_targeted_overrides_inserts_and_interpolates_js_from_the_environment() {
    let world = World::registry_env(
        "- {id: noop, name: noop, config: {value: base}}\n",
        |_| {},
        |root| EvalEnv {
            var: Arc::new(|name| (name == "DSH_APP_BOOT_USER_SPEC").then(|| "user-value".into())),
            ..env(root)
        },
    )
    .await;
    world.patch("- id: noop\n  config:\n    value: !!js process.env.DSH_APP_BOOT_USER_SPEC\n- insert: [{id: extra, name: noop}]\n").await.unwrap();
    assert_eq!(
        world.row("noop").fiber.unwrap().effective_config().unwrap()["value"],
        "user-value"
    );
    assert!(world.active("extra"));
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn mounts_no_patch_layer_for_an_absent_or_empty_user_layer() {
    let world = World::new("- {id: noop, name: noop, config: {value: base}}\n").await;
    world.patch("[]").await.unwrap();
    assert_eq!(world.row("noop").options.config, json!({"value":"base"}));
    assert_eq!(world.loader.entries().len(), 2);
    world.close().await;
}
