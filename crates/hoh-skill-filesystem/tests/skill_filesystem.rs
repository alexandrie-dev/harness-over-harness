use std::{path::Path, sync::Arc, time::Duration};

use hoh_error::HarnessError;
use hoh_fs_local::FsLocalPlugin;
use hoh_kernel::{Context, Kernel, Plugin};
use hoh_skill::{
    SkillLookupOptions, SkillProvider, SkillProviderOutput, SkillRegistration, SkillRegistry,
    SkillViewOptions, Skills,
};
use hoh_skill_filesystem::{Config, FileSystemSkillProvider, NAME, SkillFileSystemPlugin};
use hoh_timeout::AbortController;
use serde_json::json;
use tempfile::TempDir;

fn write_skill(root: &Path, name: &str, description: &str, body: &str) {
    let directory = root.join(name);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n"),
    )
    .unwrap();
}

fn write_flat(root: &Path, name: &str, frontmatter: &str, body: &str) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(
        root.join(format!("{name}.md")),
        format!("---\nname: {name}\n{frontmatter}\n---\n\n{body}\n"),
    )
    .unwrap();
}

async fn setup(home: &Path, extra: serde_json::Value) -> (Kernel, Context) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SkillRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let mut config = json!({
        "dshHome": home.join(".dsh"),
        "agentsHome": home.join(".agents"),
        "watch": false,
    });
    config
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    ctx.plugin::<SkillFileSystemPlugin>(config)
        .unwrap()
        .settled()
        .await
        .unwrap();
    (kernel, ctx)
}

async fn list(ctx: &Context, cwd: Option<&Path>) -> Vec<hoh_skill::SkillSummary> {
    ctx.get::<Skills>()
        .unwrap()
        .list(Arc::new(SkillViewOptions {
            cwd: cwd.map(|path| path.to_string_lossy().into_owned()),
            ..Default::default()
        }))
        .await
        .unwrap()
}

async fn get(ctx: &Context, name: &str) -> Option<hoh_skill::SkillDefinition> {
    ctx.get::<Skills>()
        .unwrap()
        .get(name, Arc::new(SkillViewOptions::default()))
        .await
        .unwrap()
}

fn candidate_count(output: SkillProviderOutput) -> usize {
    match output {
        SkillProviderOutput::Candidates(candidates) => candidates.len(),
        SkillProviderOutput::Observation(observation) => observation.candidates.len(),
        SkillProviderOutput::Raw(_) => panic!("unexpected raw provider output"),
    }
}

async fn setup_with_fs(home: &Path) -> (Kernel, Context) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SkillRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<FsLocalPlugin>(json!({"cwd":home}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SkillFileSystemPlugin>(json!({
        "dshHome":home.join(".dsh"),
        "agentsHome":home.join(".agents"),
        "watch":false,
    }))
    .unwrap()
    .settled()
    .await
    .unwrap();
    (kernel, ctx)
}

#[tokio::test]
async fn plugin_metadata_and_watcher_config_validation_are_exact() {
    assert_eq!(SkillFileSystemPlugin::NAME, NAME);
    assert_eq!(NAME, "skill-filesystem");
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SkillRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    for (field, value) in [
        ("watchMaxProjects", json!(0)),
        ("watchPollIntervalMs", json!(1.5)),
        ("watchStabilityThresholdMs", json!(0)),
    ] {
        let handle = ctx
            .plugin::<SkillFileSystemPlugin>(json!({field:value}))
            .unwrap();
        assert!(
            handle
                .settled()
                .await
                .unwrap_err()
                .to_string()
                .contains(field)
        );
    }
    kernel.shutdown().await;
}

#[tokio::test]
async fn discovers_all_roots_in_rank_order_and_loads_exact_body() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let custom = TempDir::new().unwrap();
    let bundled = TempDir::new().unwrap();
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    for (root, description) in [
        (home.path().join(".agents/skills"), "user agents"),
        (home.path().join(".dsh/skills"), "user dsh"),
        (custom.path().to_path_buf(), "custom"),
        (project.path().join(".agents/skills"), "project agents"),
        (project.path().join(".dsh/skills"), "project dsh"),
        (bundled.path().to_path_buf(), "bundled"),
    ] {
        write_skill(&root, "same", description, description);
    }
    write_skill(custom.path(), "custom-only", "custom only", "Custom body.");
    write_skill(
        bundled.path(),
        "bundled-only",
        "bundled only",
        "Bundled body.",
    );
    write_skill(
        &home.path().join(".dsh/skills/.system"),
        "hidden-system",
        "hidden",
        "Hidden.",
    );
    let (kernel, ctx) = setup(
        home.path(),
        json!({
            "customSkillDirs":[custom.path()],
            "bundledSkillDir":bundled.path(),
        }),
    )
    .await;
    let skills = list(&ctx, Some(project.path())).await;
    assert_eq!(
        skills
            .iter()
            .map(|skill| skill.text("name").unwrap())
            .collect::<Vec<_>>(),
        ["bundled-only", "custom-only", "same"]
    );
    let same = skills
        .iter()
        .find(|skill| skill.text("name") == Some("same"))
        .unwrap();
    assert_eq!(same.text("description"), Some("project dsh"));
    assert_eq!(same.text("source"), Some("project-dsh"));
    assert_eq!(
        get(&ctx, "bundled-only").await.unwrap().text("content"),
        Some("Bundled body.")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn parses_flat_crlf_block_metadata_and_invocation_values() {
    let home = TempDir::new().unwrap();
    let root = home.path().join(".dsh/skills");
    write_flat(
        &root,
        "rich-skill",
        "description: rich description\nwhenToUse: For local parsing\ndisable-model-invocation: off\nuser-invocable: YES\nmetadata:\n  owner: tests",
        "Rich body.",
    );
    std::fs::write(
        root.join("block-skill.md"),
        "---\r\nname: block-skill\r\ndescription: |\r\n  Includes a ---- marker.\r\nmetadata:\r\n  marker: \"----\"\r\n---\r\n\r\nBlock body.\r\n",
    )
    .unwrap();
    write_flat(
        &root,
        "user-only",
        "description: user only\ndisable-model-invocation: true",
        "User body.",
    );
    write_flat(
        &root,
        "model-only",
        "description: model only\nuser-invocable: false",
        "Model body.",
    );
    write_flat(
        &root,
        "numeric-bools",
        "description: numeric bools\ndisable-model-invocation: 1.0\nuser-invocable: 0.0",
        "Numeric body.",
    );
    let (kernel, ctx) = setup(home.path(), json!({})).await;
    let rich = get(&ctx, "rich-skill").await.unwrap();
    assert_eq!(rich.text("whenToUse"), Some("For local parsing"));
    assert_eq!(rich.get("metadata"), Some(&json!({"owner":"tests"})));
    assert_eq!(
        rich.get("invocation"),
        Some(&json!({"modelInvocable":true,"userInvocable":true}))
    );
    assert_eq!(
        get(&ctx, "block-skill").await.unwrap().text("description"),
        Some("Includes a ---- marker.\n")
    );
    assert_eq!(
        get(&ctx, "user-only").await.unwrap().get("invocation"),
        Some(&json!({"modelInvocable":false,"userInvocable":true}))
    );
    assert_eq!(
        get(&ctx, "model-only").await.unwrap().get("invocation"),
        Some(&json!({"modelInvocable":true,"userInvocable":false}))
    );
    assert_eq!(
        get(&ctx, "numeric-bools").await.unwrap().get("invocation"),
        Some(&json!({"modelInvocable":false,"userInvocable":false}))
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn parses_flow_metadata_but_filters_collection_typed_required_fields() {
    let home = TempDir::new().unwrap();
    let root = home.path().join(".dsh/skills");
    write_flat(
        &root,
        "flow-skill",
        "description: useful # trailing YAML comment\nmetadata: {owner: tests, flags: [one, two]}",
        "Flow body.",
    );
    write_flat(
        &root,
        "array-description",
        "description: [not, text]",
        "Invalid.",
    );
    let (kernel, ctx) = setup(home.path(), json!({})).await;
    assert_eq!(
        list(&ctx, None)
            .await
            .iter()
            .map(|skill| skill.text("name").unwrap())
            .collect::<Vec<_>>(),
        ["flow-skill"]
    );
    assert_eq!(
        get(&ctx, "flow-skill").await.unwrap().get("metadata"),
        Some(&json!({"owner":"tests","flags":["one","two"]}))
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn definition_field_order_matches_the_source_contract() {
    let home = TempDir::new().unwrap();
    let root = home.path().join(".dsh/skills");
    write_flat(
        &root,
        "ordered-skill",
        "description: Ordered\nwhenToUse: Now\nmetadata: {owner: tests}",
        "Ordered body.",
    );
    let (kernel, ctx) = setup(home.path(), json!({})).await;
    let value = get(&ctx, "ordered-skill").await.unwrap().to_value();
    assert_eq!(
        value.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "name",
            "description",
            "whenToUse",
            "invocation",
            "source",
            "provider",
            "resourceBase",
            "path",
            "metadata",
            "content",
        ]
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn invalid_frontmatter_is_filtered_without_hiding_valid_siblings() {
    let home = TempDir::new().unwrap();
    let root = home.path().join(".dsh/skills");
    write_skill(&root, "good-skill", "Good skill", "Good.");
    for (name, field) in [
        ("legacy-model", "disableModelInvocation: true"),
        ("legacy-positive", "modelInvocable: false"),
        ("legacy-user", "userInvocable: false"),
        ("bad-string", "disable-model-invocation: maybe"),
        ("bad-value", "user-invocable: null"),
    ] {
        write_flat(
            &root,
            name,
            &format!("description: {name}\n{field}"),
            "Bad.",
        );
    }
    write_flat(&root, "Bad_Name", "description: invalid", "Bad.");
    write_flat(&root, "missing-description", "metadata:\n  x: y", "Bad.");
    std::fs::write(
        root.join("bad-yaml.md"),
        "---\nname: bad-yaml\ndescription: [unclosed\n---\nBad.",
    )
    .unwrap();
    std::fs::write(root.join("no-frontmatter.md"), "# Notes").unwrap();
    std::fs::write(
        root.join("open-frontmatter.md"),
        "---\nname: open-frontmatter\ndescription: open",
    )
    .unwrap();
    std::fs::write(
        root.join("no-trailing-body.md"),
        "---\nname: no-trailing-body\ndescription: No trailing body\n---",
    )
    .unwrap();
    std::fs::write(root.join("notes.txt"), "not a skill").unwrap();
    std::fs::create_dir_all(root.join("not-a-skill")).unwrap();
    let (kernel, ctx) = setup(home.path(), json!({})).await;
    assert_eq!(
        list(&ctx, None)
            .await
            .iter()
            .map(|skill| skill.text("name").unwrap())
            .collect::<Vec<_>>(),
        ["good-skill", "no-trailing-body"]
    );
    assert_eq!(
        get(&ctx, "no-trailing-body").await.unwrap().text("content"),
        Some("")
    );
    assert!(get(&ctx, "Bad_Name").await.is_none());
    std::fs::remove_file(root.join("good-skill/SKILL.md")).unwrap();
    assert!(get(&ctx, "good-skill").await.is_none());
    kernel.shutdown().await;
}

#[tokio::test]
async fn runtime_rank_sits_between_project_and_custom_skills() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let custom = TempDir::new().unwrap();
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    write_skill(
        &project.path().join(".dsh/skills"),
        "project-name",
        "Project wins",
        "Project.",
    );
    write_skill(custom.path(), "runtime-name", "Custom loses", "Custom.");
    let (kernel, ctx) = setup(home.path(), json!({"customSkillDirs":[custom.path()]})).await;
    let skills = ctx.get::<Skills>().unwrap();
    for (name, description) in [
        ("project-name", "Runtime loses"),
        ("runtime-name", "Runtime wins"),
    ] {
        skills
            .register(
                &ctx,
                SkillRegistration::from_value(json!({
                    "name":name,
                    "description":description,
                    "content":"Runtime body.",
                    "source":"runtime",
                })),
            )
            .unwrap();
    }
    let options = Arc::new(SkillViewOptions {
        cwd: Some(project.path().to_string_lossy().into_owned()),
        ..Default::default()
    });
    assert_eq!(
        skills
            .get("project-name", options.clone())
            .await
            .unwrap()
            .unwrap()
            .text("description"),
        Some("Project wins")
    );
    assert_eq!(
        skills
            .get("runtime-name", options)
            .await
            .unwrap()
            .unwrap()
            .text("description"),
        Some("Runtime wins")
    );
    kernel.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn follows_skill_symlinks_but_preserves_the_resource_directory() {
    let home = TempDir::new().unwrap();
    let external = TempDir::new().unwrap();
    let root = home.path().join(".dsh/skills");
    write_skill(external.path(), "linked-skill", "Linked", "Linked body.");
    std::fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::symlink(
        external.path().join("linked-skill"),
        root.join("linked-skill"),
    )
    .unwrap();
    let (kernel, ctx) = setup(home.path(), json!({})).await;
    let candidate = list(&ctx, None).await.remove(0);
    assert_eq!(
        candidate.text("path"),
        Some(
            external
                .path()
                .join("linked-skill/SKILL.md")
                .to_string_lossy()
                .as_ref()
        )
    );
    assert_eq!(
        get(&ctx, "linked-skill").await.unwrap().get("resourceBase"),
        Some(&json!({"kind":"directory","path":root.join("linked-skill")}))
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn filesystem_service_handles_discovery_reads_project_roots_and_cancellation() {
    let home = TempDir::new().unwrap();
    let project = home.path().join("workspace/project");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    write_skill(
        &project.join(".dsh/skills"),
        "project-fs-skill",
        "Through fs",
        "Filesystem service body.",
    );
    let (kernel, ctx) = setup_with_fs(home.path()).await;
    let skills = list(&ctx, Some(&project.join("nested"))).await;
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].text("name"), Some("project-fs-skill"));
    assert_eq!(
        ctx.get::<Skills>()
            .unwrap()
            .get(
                "project-fs-skill",
                Arc::new(SkillViewOptions {
                    cwd: Some(project.to_string_lossy().into_owned()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap()
            .unwrap()
            .text("content"),
        Some("Filesystem service body.")
    );

    let abort = AbortController::new();
    abort.abort();
    assert!(
        ctx.get::<Skills>()
            .unwrap()
            .get(
                "project-fs-skill",
                Arc::new(SkillViewOptions {
                    cwd: Some(project.to_string_lossy().into_owned()),
                    signal: Some(abort.signal()),
                    ..Default::default()
                }),
            )
            .await
            .is_err()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn unreadable_bundled_skill_reports_skill_read_failed() {
    let home = TempDir::new().unwrap();
    let bundled = TempDir::new().unwrap();
    write_skill(bundled.path(), "bundled-only", "Bundled", "Body.");
    let (kernel, ctx) = setup(home.path(), json!({"bundledSkillDir":bundled.path()})).await;
    assert_eq!(list(&ctx, None).await.len(), 1);
    let file = bundled.path().join("bundled-only/SKILL.md");
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    let error = ctx
        .get::<Skills>()
        .unwrap()
        .get("bundled-only", Arc::new(SkillViewOptions::default()))
        .await
        .unwrap_err();
    let error = HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.name(), "SkillError");
    assert_eq!(error.code(), "SKILL_READ_FAILED");
    kernel.shutdown().await;
}

#[tokio::test]
async fn watcher_refreshes_add_change_delete_and_missing_root_recreation() {
    let home = TempDir::new().unwrap();
    let root = home.path().join(".agents/skills");
    let (kernel, ctx) = setup(
        home.path(),
        json!({
            "watch":true,
            "watchStabilityThresholdMs":20,
            "watchPollIntervalMs":10,
        }),
    )
    .await;
    assert!(list(&ctx, None).await.is_empty());
    write_skill(&root, "watched-skill", "First", "First body.");
    wait_until(&ctx, |skills| {
        skills
            .iter()
            .any(|skill| skill.text("description") == Some("First"))
    })
    .await;
    write_skill(&root, "watched-skill", "Second", "Second body.");
    wait_until(&ctx, |skills| {
        skills
            .iter()
            .any(|skill| skill.text("description") == Some("Second"))
    })
    .await;
    std::fs::remove_dir_all(&root).unwrap();
    wait_until(&ctx, Vec::is_empty).await;
    write_skill(&root, "recreated-skill", "Recreated", "Recreated.");
    wait_until(&ctx, |skills| {
        skills
            .iter()
            .any(|skill| skill.text("name") == Some("recreated-skill"))
    })
    .await;
    kernel.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn watcher_filters_events_against_the_canonical_symlink_root() {
    let home = TempDir::new().unwrap();
    let external = TempDir::new().unwrap();
    let dsh_home = home.path().join(".dsh");
    std::fs::create_dir_all(&dsh_home).unwrap();
    write_skill(external.path(), "linked-root-skill", "First", "First.");
    std::os::unix::fs::symlink(external.path(), dsh_home.join("skills")).unwrap();
    let (kernel, ctx) = setup(
        home.path(),
        json!({
            "watch":true,
            "watchFollowSymlinks":true,
            "watchStabilityThresholdMs":20,
            "watchPollIntervalMs":10,
        }),
    )
    .await;
    assert_eq!(list(&ctx, None).await[0].text("description"), Some("First"));
    write_skill(external.path(), "linked-root-skill", "Second", "Second.");
    wait_until(&ctx, |skills| {
        skills
            .first()
            .is_some_and(|skill| skill.text("description") == Some("Second"))
    })
    .await;
    kernel.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn watcher_refreshes_a_followed_child_skill_symlink() {
    let home = TempDir::new().unwrap();
    let external = TempDir::new().unwrap();
    let root = home.path().join(".agents/skills");
    write_skill(external.path(), "linked-child", "First", "First.");
    std::fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::symlink(
        external.path().join("linked-child"),
        root.join("linked-child"),
    )
    .unwrap();
    let (kernel, ctx) = setup(
        home.path(),
        json!({
            "watch":true,
            "watchFollowSymlinks":true,
            "watchStabilityThresholdMs":20,
            "watchPollIntervalMs":10,
        }),
    )
    .await;
    assert_eq!(list(&ctx, None).await[0].text("description"), Some("First"));
    write_skill(external.path(), "linked-child", "Second", "Second.");
    wait_until(&ctx, |skills| {
        skills
            .first()
            .is_some_and(|skill| skill.text("description") == Some("Second"))
    })
    .await;
    kernel.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn watcher_refreshes_when_a_child_skill_symlink_is_added_and_removed() {
    let home = TempDir::new().unwrap();
    let external = TempDir::new().unwrap();
    let root = home.path().join(".agents/skills");
    let target = external.path().join("linked-child");
    write_skill(external.path(), "linked-child", "Linked", "Linked.");
    std::fs::create_dir_all(&root).unwrap();
    let (kernel, ctx) = setup(
        home.path(),
        json!({
            "watch":true,
            "watchFollowSymlinks":true,
            "watchStabilityThresholdMs":20,
            "watchPollIntervalMs":10,
        }),
    )
    .await;
    assert!(list(&ctx, None).await.is_empty());

    let link = root.join("linked-child");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    wait_until(&ctx, |skills| {
        skills
            .iter()
            .any(|skill| skill.text("name") == Some("linked-child"))
    })
    .await;

    std::fs::remove_file(&link).unwrap();
    wait_until(&ctx, Vec::is_empty).await;
    kernel.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn polling_watcher_ignores_child_skill_symlink_when_following_is_disabled() {
    let home = TempDir::new().unwrap();
    let external = TempDir::new().unwrap();
    let root = home.path().join(".agents/skills");
    let target = external.path().join("linked-child");
    write_skill(external.path(), "linked-child", "Linked", "Linked.");
    std::fs::create_dir_all(&root).unwrap();
    let (kernel, ctx) = setup(
        home.path(),
        json!({
            "watch":true,
            "watchUsePolling":true,
            "watchFollowSymlinks":false,
            "watchStabilityThresholdMs":20,
            "watchPollIntervalMs":10,
        }),
    )
    .await;
    assert!(list(&ctx, None).await.is_empty());

    let link = root.join("linked-child");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(list(&ctx, None).await.is_empty());

    std::fs::remove_file(link).unwrap();
    kernel.shutdown().await;
}

#[tokio::test]
async fn watcher_stability_window_restarts_after_each_write_event() {
    let home = TempDir::new().unwrap();
    let root = home.path().join(".agents/skills");
    write_skill(&root, "settling-skill", "First", "First.");
    let (kernel, ctx) = setup(
        home.path(),
        json!({
            "watch":true,
            "watchStabilityThresholdMs":200,
            "watchPollIntervalMs":10,
        }),
    )
    .await;
    assert_eq!(list(&ctx, None).await[0].text("description"), Some("First"));
    std::fs::write(
        root.join("settling-skill/SKILL.md"),
        "---\nname: settling-skill\ndescription: [unfinished\n",
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    write_skill(&root, "settling-skill", "Second", "Second.");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(list(&ctx, None).await[0].text("description"), Some("First"));
    wait_until(&ctx, |skills| {
        skills
            .first()
            .is_some_and(|skill| skill.text("description") == Some("Second"))
    })
    .await;
    kernel.shutdown().await;
}

#[tokio::test]
async fn provider_control_abort_disposes_watchers_but_keeps_direct_reads_contained() {
    let home = TempDir::new().unwrap();
    write_skill(
        &home.path().join(".agents/skills"),
        "disposed-skill",
        "Disposed",
        "Still readable.",
    );
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SkillRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let holder = Arc::new(std::sync::Mutex::new(None));
    let output = holder.clone();
    let provider_ctx = ctx.clone();
    let config = Config {
        provider_name: "lifecycle-test".into(),
        dsh_home: Some(home.path().join(".dsh").to_string_lossy().into_owned()),
        agents_home: Some(home.path().join(".agents").to_string_lossy().into_owned()),
        watch_stability_threshold_ms: 10.0,
        watch_poll_interval_ms: 10.0,
        ..Config::default()
    };
    let disposer = ctx
        .get::<Skills>()
        .unwrap()
        .register_provider(&ctx, move |control| {
            let provider = Arc::new(FileSystemSkillProvider::new(provider_ctx, control, config)?);
            *output.lock().unwrap() = Some(provider.clone());
            Ok(provider)
        })
        .unwrap();
    let provider = holder.lock().unwrap().clone().unwrap();
    assert_eq!(
        candidate_count(
            provider
                .list(Arc::new(SkillLookupOptions::default()))
                .await
                .unwrap()
        ),
        1
    );
    disposer.dispose().await;
    tokio::task::yield_now().await;
    provider.dispose();
    provider.dispose();
    assert_eq!(
        candidate_count(
            provider
                .list(Arc::new(SkillLookupOptions::default()))
                .await
                .unwrap()
        ),
        1
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn host_mutation_invalidates_cached_catalog_when_watch_is_disabled() {
    let home = TempDir::new().unwrap();
    let root = home.path().join(".agents/skills");
    write_skill(&root, "host-skill", "First", "First.");
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SkillRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let holder = Arc::new(std::sync::Mutex::new(None));
    let output = holder.clone();
    let provider_ctx = ctx.clone();
    let config = Config {
        dsh_home: Some(home.path().join(".dsh").to_string_lossy().into_owned()),
        agents_home: Some(home.path().join(".agents").to_string_lossy().into_owned()),
        watch: false,
        ..Config::default()
    };
    ctx.get::<Skills>()
        .unwrap()
        .register_provider(&ctx, move |control| {
            let provider = Arc::new(FileSystemSkillProvider::new(provider_ctx, control, config)?);
            *output.lock().unwrap() = Some(provider.clone());
            Ok(provider)
        })
        .unwrap();
    let provider = holder.lock().unwrap().clone().unwrap();
    assert_eq!(list(&ctx, None).await[0].text("description"), Some("First"));
    write_skill(&root, "host-skill", "Second", "Second.");
    provider.observe_host_mutation(root.join("host-skill/SKILL.md").to_string_lossy().as_ref());
    assert_eq!(
        list(&ctx, None).await[0].text("description"),
        Some("Second")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn invalid_utf8_native_skill_does_not_hide_valid_siblings() {
    let home = TempDir::new().unwrap();
    let bundled = TempDir::new().unwrap();
    write_skill(bundled.path(), "ok-skill", "Valid", "Body.");
    std::fs::write(
        bundled.path().join("bad.md"),
        b"---\nname: bad\ndescription\xff invalid\n---\n",
    )
    .unwrap();
    let (kernel, ctx) = setup(home.path(), json!({"bundledSkillDir":bundled.path()})).await;
    let skills = list(&ctx, None).await;
    assert_eq!(
        skills
            .iter()
            .map(|skill| skill.text("name").unwrap())
            .collect::<Vec<_>>(),
        ["ok-skill"]
    );
    kernel.shutdown().await;
}

async fn wait_until(ctx: &Context, accept: impl Fn(&Vec<hoh_skill::SkillSummary>) -> bool) {
    let started = std::time::Instant::now();
    loop {
        let skills = list(ctx, None).await;
        if accept(&skills) {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "watch timed out"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[test]
fn config_defaults_match_the_source() {
    assert_eq!(Config::default().provider_name, "filesystem");
    assert!(Config::default().include_default_roots);
    assert!(Config::default().watch);
    assert_eq!(Config::default().watch_max_projects, 128.0);
}
