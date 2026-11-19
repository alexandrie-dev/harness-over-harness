mod support;
use hoh_kernel::{Cfg, Context, Error, Plugin, Service};
use hoh_loader::config_editor::*;
use hoh_loader::*;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use support::{live::*, *};
struct Fixture {
    world: World,
    dir: PathBuf,
    editor: Service<ConfigEditorKey>,
}
#[derive(Debug, thiserror::Error)]
#[error("settings revision conflict: expected {expected}, actual {actual}")]
struct SettingsConflictError {
    expected: u64,
    actual: u64,
}
#[derive(Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
struct InheritedConfig {
    marker: String,
    title: String,
    #[serde(default = "default_attempts")]
    attempts: u32,
}
fn default_attempts() -> u32 {
    7
}
struct InheritedPlugin;
struct WebStartup;
impl hoh_kernel::ServiceKey for WebStartup {
    type Value = Value;
    const NAME: &'static str = "webStartup";
    fn view(value: &Value) -> Option<Value> {
        Some(value.clone())
    }
}
struct StartupProvider;
impl Plugin for StartupProvider {
    const NAME: &'static str = "startup-provider";
    type Config = Value;
    type Deps = ();
    async fn apply(ctx: Context, _: (), config: Cfg<Value>) -> Result<(), Error> {
        ctx.provide::<WebStartup>(Arc::new((*config.get()).clone()))
            .await?;
        Ok(())
    }
}
#[tokio::test(start_paused = true)]
async fn inherited_interpolation_uses_the_entry_isolation_realm() {
    let world = World::registry("- {id: outside, name: startup-provider, config: {port: 9}}\n- {id: local, name: startup-provider, isolate: {webStartup: private}, config: {port: 4813}}\n- {id: x, name: objects, inject: [webStartup], isolate: {webStartup: private}, config: {}}", |registry| {registry.add::<StartupProvider>("startup-provider");}).await;
    assert!(world.active("x"));
    let raw = json!({"title":{"__jsExpr":"'port:' + ctx.webStartup.port"}});
    assert_eq!(
        world
            .loader
            .interpolate_entry(&world.row("x"), &raw)
            .unwrap(),
        json!({"title":"port:4813"})
    );
    let inherited = world
        .loader
        .resolve_inherited(&world.row("x"), &raw)
        .await
        .unwrap();
    assert_eq!(inherited["title"], "port:4813");
    assert_eq!(inherited["fixed"], "fixed");
    world.close().await;
}
impl Plugin for InheritedPlugin {
    const NAME: &'static str = "inherited-fixture";
    type Config = InheritedConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), config: Cfg<InheritedConfig>) -> Result<(), Error> {
        tokio::fs::write(&config.get().marker, "applied").await?;
        let observed = ctx.tag::<Observed>().ok_or("missing observed")?;
        ctx.on_candidate::<InheritedConfig>(move |config| {
            locked(&observed.hooks).push(json!(config.title));
            Err("live candidate hook must not run for inherited data".into())
        })?;
        Ok(())
    }
}
#[tokio::test(start_paused = true)]
async fn inherited_resolution_uses_the_actual_codec_and_context_without_apply_or_candidate_hooks() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("applied");
    let text = format!(
        "- id: x\n  name: inherited-fixture\n  config: {}\n",
        json!({"marker":marker,"title":"live"})
    );
    let world = World::registry(&text, |r| {
        r.add::<InheritedPlugin>("inherited-fixture");
    })
    .await;
    assert!(marker.exists());
    std::fs::remove_file(&marker).unwrap();
    let entry = world.row("x");
    let before = world.kernel.snapshot().await.fibers.len();
    let raw = json!({"marker":marker,"title":{"__jsExpr":"process.cwd()"}});
    let resolved = world.loader.resolve_inherited(&entry, &raw).await.unwrap();
    assert_eq!(
        resolved,
        json!({"marker":marker,"title":world.dir.path(),"attempts":7})
    );
    assert!(
        !marker.exists(),
        "codec resolution must never apply a plugin"
    );
    assert!(locked(&world.observed.hooks).is_empty());
    assert_eq!(world.kernel.snapshot().await.fibers.len(), before);
    assert_eq!(
        entry.fiber.as_ref().unwrap().effective_config().unwrap()["title"],
        "live"
    );
    assert!(
        entry
            .fiber
            .as_ref()
            .unwrap()
            .resolve_config(&resolved)
            .is_err()
    );
    assert_eq!(locked(&world.observed.hooks).len(), 1);
    // Missing required values are a legitimate inherited base; settings can still display it.
    let incomplete = json!({"title":{"__jsExpr":"process.cwd()"}});
    assert_eq!(
        world
            .loader
            .resolve_inherited(&entry, &incomplete)
            .await
            .unwrap(),
        json!({"title":world.dir.path()})
    );
    assert!(
        world
            .loader
            .resolve_inherited(&entry, &json!({"title":{"__jsExpr":"JSON.parse('bad')"}}))
            .await
            .is_err()
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn fallible_callback_reads_reloaded_configuration_and_preserves_its_error_before_write() {
    let f = fixture("- id: x\n  config: {title: original}\n", None).await;
    let entry = f.world.row("x");
    let before = "- id: x\n  config: {title: external}\n";
    std::fs::write(f.editor.document_path(), before).unwrap();
    let error = Box::new(SettingsConflictError {
        expected: 0,
        actual: 1,
    });
    let address = (&*error as *const SettingsConflictError) as usize;
    let editor = f.editor.clone();
    let result = f
        .editor
        .try_edit(&entry, move |current, _| async move {
            assert_eq!(current, json!({"title":"external"}));
            let configuration = editor.configuration().await?;
            assert_eq!(
                configuration
                    .iter()
                    .find(|c| c.entry.options.id == "x")
                    .unwrap()
                    .r#override,
                current
            );
            Err(error as hoh_error::BoxError)
        })
        .await
        .unwrap_err();
    let actual = result.downcast::<SettingsConflictError>().unwrap();
    assert_eq!((&*actual as *const SettingsConflictError) as usize, address);
    assert_eq!((actual.expected, actual.actual), (0, 1));
    assert_eq!(
        std::fs::read_to_string(f.editor.document_path()).unwrap(),
        before
    );
    assert_eq!(
        f.world.row("x").fiber.unwrap().effective_config().unwrap()["title"],
        "external"
    );
    f.world.close().await;
}
#[tokio::test(start_paused = true)]
async fn active_profile_home_and_name_are_explicit_read_only_host_facts() {
    let f = fixture("[]\n", None).await;
    assert_eq!(f.editor.home(), f.world.dir.path().join("home"));
    assert_eq!(
        f.editor.profile_name(),
        f.dir.file_name().unwrap().to_str().unwrap()
    );
    f.world.close().await;
}
#[tokio::test(start_paused = true)]
async fn an_application_owned_profile_context_keeps_its_name_and_explicit_home() {
    let world = World::new("[]").await;
    let dir = write_profile(world.dir.path(), &[]);
    let home = world.dir.path().join("active-home");
    world
        .ctx
        .provide::<ProfileContextKey>(Arc::new(ProfileContext {
            name: "application profile".into(),
            dir: dir.clone(),
            home: home.clone(),
        }))
        .await
        .unwrap();
    let editor = ConfigEditor::mount(
        &world.ctx,
        dir,
        Arc::new(Source(vec![])),
        world.dir.path().join("unused-home"),
        vec![],
        env(world.dir.path()),
    )
    .await
    .unwrap();
    assert_eq!(editor.profile_name(), "application profile");
    assert_eq!(editor.home(), home);
    assert_eq!(
        Expr::parse("ctx.get('profileContext')?.name === 'application profile'")
            .unwrap()
            .eval(
                &env(world.dir.path()),
                &world.ctx,
                &PluginRegistry::default(),
                world.dir.path()
            )
            .unwrap(),
        Some(json!(true))
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn a_mounted_entry_exposes_its_actual_definition_schema_and_rejects_stale_fibers() {
    let f = fixture("[]\n", None).await;
    let entry = f.world.row("x");
    assert_eq!(
        f.editor.schema(&entry).unwrap(),
        Some(hoh_kernel::PluginDef::of::<Objects>().schema().clone())
    );
    assert_eq!(
        f.editor
            .resolve_inherited(&entry, &json!({}))
            .await
            .unwrap()["fixed"],
        "fixed"
    );
    assert_eq!(
        f.world
            .loader
            .interpolate_entry(&entry, &json!({"value":{"__jsExpr":"process.cwd()"}}))
            .unwrap(),
        json!({"value":f.world.dir.path().to_string_lossy()})
    );
    f.world
        .patch("- insert: [{id: replacement, name: noop}]")
        .await
        .unwrap();
    assert!(f.editor.schema(&entry).is_err());
    f.world.close().await;
}
async fn fixture(patch: &str, home_patch: Option<&str>) -> Fixture {
    let world = World::registry("[]", |r| {
        r.add::<Candidate>("candidate");
    })
    .await;
    let dir = write_profile(world.dir.path(), &["bundle"]);
    std::fs::write(dir.join("cordis.patch.yml"), patch).unwrap();
    let source: Arc<dyn BundleSource> = Arc::new(Source(vec![bundle(
        "bundle",
        "- insert: [{id: x, name: objects, config: {title: inherited}}, {id: candidate, name: candidate, config: {fail: false}}]\n",
    )]));
    let home = world.dir.path().join("home");
    if let Some(patch) = home_patch {
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("cordis.patch.yml"), patch).unwrap();
    }
    let profile = Profile::load(&dir, source.as_ref(), &home, &[], &env(world.dir.path())).unwrap();
    world.loader.recompose(profile.layers).await.unwrap();
    let editor = ConfigEditor::mount(
        &world.ctx,
        dir.clone(),
        source,
        home,
        vec![],
        env(world.dir.path()),
    )
    .await
    .unwrap();
    Fixture { world, dir, editor }
}
struct Candidate;
struct EmptyConfig;
impl Plugin for EmptyConfig {
    const NAME: &'static str = "empty-config";
    type Config = hoh_kernel::NoConfig;
    type Deps = ();
    async fn apply(_: Context, _: (), _: Cfg<hoh_kernel::NoConfig>) -> Result<(), Error> {
        Ok(())
    }
}
#[tokio::test(start_paused = true)]
async fn a_plugin_without_config_exposes_no_form_schema() {
    let world = World::registry("- id: empty\n  name: empty\n", |r| {
        r.add::<EmptyConfig>("empty");
    })
    .await;
    assert!(
        world
            .loader
            .entry_schema(&world.row("empty"))
            .unwrap()
            .is_none()
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn an_edit_reload_keeps_the_invocations_frozen_command_line_overlay() {
    let world = World::new("[]").await;
    let dir = write_profile(world.dir.path(), &["bundle"]);
    std::fs::write(dir.join("cordis.patch.yml"), "[]\n").unwrap();
    let source: Arc<dyn BundleSource> = Arc::new(Source(vec![bundle(
        "bundle",
        "- insert: [{id: x, name: objects}]\n",
    )]));
    let overlay = world.dir.path().join("overlay.yml");
    std::fs::write(&overlay, "- id: x\n  config: {title: pinned}\n").unwrap();
    let home = world.dir.path().join("home");
    let environment = env(world.dir.path());
    let profile = Profile::load(
        &dir,
        source.as_ref(),
        &home,
        std::slice::from_ref(&overlay),
        &environment,
    )
    .unwrap();
    world.loader.recompose(profile.layers).await.unwrap();
    let editor = ConfigEditor::mount(
        &world.ctx,
        dir.clone(),
        source,
        home,
        vec![overlay.clone()],
        environment,
    )
    .await
    .unwrap();
    std::fs::write(overlay, "- id: x\n  config: {title: changed-on-disk}\n").unwrap();
    let error = editor
        .try_edit(&world.row("x"), |current, _| async move {
            if current["title"] != "pinned" {
                return Err("command-line overlay was reread".into());
            }
            Err(Box::new(SettingsConflictError {
                expected: 0,
                actual: 1,
            }) as hoh_error::BoxError)
        })
        .await
        .unwrap_err();
    assert!(error.downcast_ref::<SettingsConflictError>().is_some());
    assert_eq!(
        world.row("x").fiber.unwrap().effective_config().unwrap()["title"],
        "pinned"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("cordis.patch.yml")).unwrap(),
        "[]\n"
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn missing_and_null_configs_are_empty_objects_and_an_empty_edit_is_a_noop() {
    let world = World::new("[]").await;
    let dir = write_profile(world.dir.path(), &["bundle"]);
    let patch = "- id: explicit-null\n  config: null\n";
    std::fs::write(dir.join("cordis.patch.yml"), patch).unwrap();
    let source: Arc<dyn BundleSource> = Arc::new(Source(vec![bundle(
        "bundle",
        "- insert: [{id: absent, name: objects}, {id: explicit-null, name: objects}]\n",
    )]));
    let home = world.dir.path().join("home");
    let environment = env(world.dir.path());
    let profile = Profile::load(&dir, source.as_ref(), &home, &[], &environment).unwrap();
    world.loader.recompose(profile.layers).await.unwrap();
    let editor = ConfigEditor::mount(&world.ctx, dir, source, home, vec![], environment)
        .await
        .unwrap();
    let configuration = editor.configuration().await.unwrap();
    for id in ["absent", "explicit-null"] {
        let row = configuration
            .iter()
            .find(|configuration| configuration.entry.options.id == id)
            .unwrap();
        assert_eq!(row.inherited, json!({}));
        assert_eq!(row.r#override, json!({}));
    }
    editor
        .edit(&world.row("absent"), |current, inherited| {
            assert_eq!(current, json!({}));
            assert_eq!(inherited, json!({}));
            json!({})
        })
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(editor.document_path()).unwrap(),
        patch
    );
    world.close().await;
}
impl Plugin for Candidate {
    const NAME: &'static str = "candidate";
    type Config = Value;
    type Deps = ();
    async fn apply(ctx: Context, _: (), cfg: Cfg<Value>) -> Result<(), Error> {
        let observed = ctx.tag::<Observed>().unwrap();
        if cfg.get()["armRollback"] == true {
            locked(&observed.hooks).push(json!("rollback armed"));
        } else if locked(&observed.hooks).contains(&json!("rollback armed")) {
            return Err("rollback reconcile failed".into());
        }
        if cfg.get()["fail"] == true {
            Err("candidate config failed".into())
        } else {
            Ok(())
        }
    }
}
const PRESERVED: &str = "# note — keep UTF-8 positions\n- id: other\n  disabled: true\n  config: &keep {flag: true}\n- insert: [{id: local, name: ./local.mjs, disabled: true, config: *keep}]\n";
#[tokio::test(start_paused = true)]
async fn setting_one_config_preserves_comments_anchors_flow_and_relative_names() {
    let f = fixture(
        &format!("{PRESERVED}- id: x\n  config:\n    title: explicit\n"),
        None,
    )
    .await;
    f.editor
        .edit(&f.world.row("x"), |_, _| json!({"title":"edited"}))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(f.editor.document_path()).unwrap(),
        format!("{PRESERVED}- id: x\n  config:\n    title: edited\n")
    );
    f.world.close().await;
}
#[tokio::test(start_paused = true)]
async fn adding_one_override_preserves_every_existing_byte() {
    let f = fixture(PRESERVED, None).await;
    f.editor
        .edit(&f.world.row("x"), |_, _| json!({"title":"edited"}))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(f.editor.document_path()).unwrap(),
        format!("{PRESERVED}- id: x\n  name: objects\n  config:\n    title: edited\n")
    );
    f.world.close().await;
}
#[tokio::test(start_paused = true)]
async fn removing_config_preserves_other_keys_and_every_other_row() {
    let f = fixture(
        &format!("{PRESERVED}- id: x\n  disabled: false\n  config: {{title: explicit}}\n"),
        None,
    )
    .await;
    f.editor
        .edit(&f.world.row("x"), |_, inherited| inherited)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(f.editor.document_path()).unwrap(),
        format!("{PRESERVED}- id: x\n  disabled: false\n")
    );
    f.world.close().await;
}
#[tokio::test(start_paused = true)]
async fn removing_a_redundant_override_deletes_only_that_row() {
    let f = fixture(
        &format!("{PRESERVED}- id: x\n  config: {{title: explicit}}\n"),
        None,
    )
    .await;
    f.editor
        .edit(&f.world.row("x"), |_, inherited| inherited)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(f.editor.document_path()).unwrap(),
        PRESERVED
    );
    f.world.close().await;
}
#[tokio::test(start_paused = true)]
async fn a_failed_rollback_reconcile_replaces_the_original_edit_error() {
    let f = fixture("[]\n", None).await;
    let error = f
        .editor
        .edit(
            &f.world.row("candidate"),
            |_, _| json!({"fail":true,"armRollback":true}),
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("rollback reconcile failed"),
        "{error}"
    );
    assert_eq!(
        std::fs::read_to_string(f.editor.document_path()).unwrap(),
        "[]\n"
    );
    f.world.close().await;
}
#[tokio::test(start_paused = true)]
async fn configuration_returns_detached_inherited_and_explicit_values() {
    let f = fixture("- {id: x, config: {title: explicit}}\n", None).await;
    assert_eq!(f.editor.document_path(), f.dir.join("cordis.patch.yml"));
    let mut configs = f.editor.configuration().await.unwrap();
    let config = configs
        .iter_mut()
        .find(|c| c.entry.options.id == "x")
        .unwrap();
    assert_eq!(config.inherited, json!({"title":"inherited"}));
    assert_eq!(config.r#override, json!({"title":"explicit"}));
    config.inherited["title"] = json!("mutated");
    assert_eq!(
        f.editor
            .configuration()
            .await
            .unwrap()
            .iter()
            .find(|c| c.entry.options.id == "x")
            .unwrap()
            .inherited,
        json!({"title":"inherited"})
    );
    f.world.close().await;
}
#[tokio::test(start_paused = true)]
async fn edit_validates_persists_raw_expressions_and_reconciles_volatile_values() {
    let f = fixture("[]\n", None).await;
    let entry = f.world.row("x");
    let uid = entry.fiber.as_ref().unwrap().uid();
    f.editor
        .edit(&entry, |_, _| json!({"title":{"__jsExpr":"'edited'"}}))
        .await
        .unwrap();
    let text = std::fs::read_to_string(f.editor.document_path()).unwrap();
    assert!(text.contains("!!js"));
    let patches = parse_patches(&text, &f.editor.document_path()).unwrap();
    assert_eq!(
        patches[0].set["config"],
        json!({"title":{"__jsExpr":"'edited'"}})
    );
    assert_eq!(f.world.row("x").fiber.as_ref().unwrap().uid(), uid);
    assert_eq!(
        f.world.row("x").fiber.unwrap().effective_config().unwrap()["title"],
        "edited"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(f.editor.document_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    f.world.close().await;
}
#[tokio::test(start_paused = true)]
async fn restoring_inherited_config_removes_every_redundant_override() {
    let f = fixture(
        "- {id: x, config: {title: first}}\n- {id: x, name: objects, config: {title: second}}\n",
        None,
    )
    .await;
    f.editor
        .edit(&f.world.row("x"), |_, inherited| inherited)
        .await
        .unwrap();
    assert!(
        parse_patches(
            &std::fs::read_to_string(f.editor.document_path()).unwrap(),
            &f.editor.document_path()
        )
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        f.world.row("x").fiber.unwrap().effective_config().unwrap()["title"],
        "inherited"
    );
    f.world.close().await;
}
#[tokio::test(start_paused = true)]
async fn invalid_config_is_rejected_before_the_file_changes() {
    let f = fixture("[]\n", None).await;
    assert!(
        f.editor
            .edit(&f.world.row("x"), |_, _| json!({"fixed":3}))
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(f.editor.document_path()).unwrap(),
        "[]\n"
    );
    assert_eq!(
        f.world.row("x").fiber.unwrap().effective_config().unwrap()["title"],
        "inherited"
    );
    f.world.close().await;
}
#[tokio::test(start_paused = true)]
async fn home_and_commandline_overrides_are_reported_before_persistence() {
    let f = fixture("[]\n", Some("- {id: x, config: {title: locked}}\n")).await;
    let error = f
        .editor
        .edit(&f.world.row("x"), |_, _| json!({"title":"edited"}))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Configuration for \"x\" is overridden by a home patch or command-line overlay"
    );
    assert_eq!(
        std::fs::read_to_string(f.editor.document_path()).unwrap(),
        "[]\n"
    );
    f.world.close().await;
}
#[tokio::test(start_paused = true)]
async fn activation_rejection_restores_the_file_and_previous_running_composition() {
    let f = fixture("[]\n", None).await;
    let error = f
        .editor
        .edit(&f.world.row("candidate"), |_, _| json!({"fail":true}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("candidate config failed"));
    assert_eq!(
        std::fs::read_to_string(f.editor.document_path()).unwrap(),
        "[]\n"
    );
    assert!(f.world.active("candidate"));
    assert_eq!(
        f.world.row("candidate").options.config,
        json!({"fail":false})
    );
    f.world.close().await;
}
#[tokio::test(start_paused = true)]
async fn stale_entries_are_rejected_and_concurrent_edits_observe_the_latest_raw_config() {
    let f = fixture("[]\n", None).await;
    let entry = f.world.row("x");
    let first = f.editor.edit(&entry, |mut current, _| {
        current["first"] = json!(true);
        current
    });
    let second = f.editor.edit(&entry, |mut current, _| {
        current["second"] = json!(true);
        current
    });
    let (a, b) = tokio::join!(first, second);
    a.unwrap();
    b.unwrap();
    assert_eq!(f.world.row("x").options.config["first"], true);
    assert_eq!(f.world.row("x").options.config["second"], true);
    f.world.loader.remove(&entry.id).await.unwrap();
    assert_eq!(
        f.editor
            .edit(&entry, |current, _| current)
            .await
            .unwrap_err()
            .to_string(),
        "Configuration entry is no longer available"
    );
    f.world.close().await;
}
