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
