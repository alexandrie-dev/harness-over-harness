use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
    MaintenanceTask, Model, ModelSelection,
};
use hoh_agent_instructions::{
    config::{
        Config, DEFAULT_MAX_SOURCE_BYTES, resolve_discovery_config, workspace_baseline_identity,
    },
    digest::{instruction_content_sha1, trimmed_instruction_digest},
    files::{
        DiscoverOptions, LoadOptions, LoadedInstructionFile, ancestor_chain,
        dedup_instruction_files_by_candidate_group, descendant_dirs_between,
        discover_baseline_instruction_files, load_baseline_instruction_set,
    },
    render::{
        AgentInstructionChange, ChangeRenderItem, InstructionAction, candidate_scope_key,
        decode_scope_key, render_agent_instruction_set, render_agent_instructions,
        render_instruction_changes,
    },
    state::{
        InstructionVersionCache, ReconcileOptions, baseline_context_message,
        baseline_instruction_state, reconcile_instruction_context,
        retained_instruction_version_updates, workspace_instruction_changes,
    },
};
use hoh_error::BoxError;
use hoh_fs::Fs;
use hoh_fs_local::FsLocalPlugin;
use hoh_kernel::{Context, Kernel};
use hoh_llm::{MessageId, UserMessage};
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, Session, SessionId};
use serde_json::json;
use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tempfile::TempDir;

struct NoopModel;

impl Model for NoopModel {
    fn current(&self) -> Option<ModelSelection> {
        None
    }

    fn select(&self, _: &ModelSelection) -> Result<(), BoxError> {
        Ok(())
    }
}

struct NoopInbox;

impl Inbox for NoopInbox {
    fn next_turn(&self) -> Vec<UserMessage> {
        vec![]
    }
    fn next_step(&self) -> Vec<UserMessage> {
        vec![]
    }
    fn clear(&self) -> Result<(), BoxError> {
        Ok(())
    }
    fn append(&self, _: InboxTarget, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
    fn prepend(&self, _: InboxTarget, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
    fn replace(&self, _: &MessageId, _: UserMessage) -> Result<bool, BoxError> {
        Ok(false)
    }
    fn remove(&self, _: &MessageId) -> Result<bool, BoxError> {
        Ok(false)
    }
    fn splice(
        &self,
        _: InboxTarget,
        _: f64,
        _: f64,
        _: Vec<UserMessage>,
    ) -> Result<Vec<UserMessage>, BoxError> {
        Ok(vec![])
    }
}

struct TestAgent {
    session: Session,
    context: Context,
    scope: ScopeKey,
    options: AgentOptions,
}

impl Agent for TestAgent {
    fn id(&self) -> &SessionId {
        self.session.id()
    }
    fn scope_key(&self) -> ScopeKey {
        self.scope.clone()
    }
    fn options(&self) -> &AgentOptions {
        &self.options
    }
    fn session(&self) -> &Session {
        &self.session
    }
    fn model(&self) -> &dyn Model {
        &NoopModel
    }
    fn inbox(&self) -> &dyn Inbox {
        &NoopInbox
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }
    fn ctx(&self) -> &Context {
        &self.context
    }
    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        Ok(())
    }
    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance_erased(&self, _: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        Err("unused maintenance".into())
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        Ok(())
    }
    fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
    fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
    fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
}

fn test_agent(context: Context, cwd: &Path) -> Arc<TestAgent> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let id: SessionId = format!(
        "agent-instructions-test-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
    .into();
    let session = Session::create(
        id.clone(),
        None,
        Some(json!({
            "version": 4,
            "id": id,
            "createdAt": 0,
            "isSeeded": false,
            "cwd": cwd,
        })),
        None,
        None,
    )
    .unwrap();
    Arc::new(TestAgent {
        session,
        context,
        scope: ScopeKey::new(),
        options: AgentOptions::default(),
    })
}

fn file(path: &str, content: &str) -> LoadedInstructionFile {
    LoadedInstructionFile {
        absolute_path: format!("/repo/{path}"),
        display_path: path.into(),
        content: content.into(),
        version: None,
    }
}

fn config(max_bytes: f64) -> Config {
    Config {
        project_root_markers: vec![".git".into()],
        max_bytes,
        max_source_bytes: DEFAULT_MAX_SOURCE_BYTES,
        instruction_file_candidates: vec!["AGENTS.md".into(), "CLAUDE.md".into()],
        local_instruction_file_candidates: vec!["AGENTS.local.md".into(), "CLAUDE.local.md".into()],
    }
}

fn discovery(cwd: &Path, home: &Path) -> DiscoverOptions {
    DiscoverOptions {
        cwd: cwd.to_string_lossy().into_owned(),
        dsh_home: Some(home.to_string_lossy().into_owned()),
        agents_home: Some(home.join("agents").to_string_lossy().into_owned()),
        project_root_markers: None,
        instruction_file_candidates: None,
        local_instruction_file_candidates: None,
        project_root: None,
        signal: None,
    }
}

fn resolved_for(
    dsh: &Path,
    agents: &Path,
    max_source_bytes: u64,
) -> hoh_agent_instructions::config::ResolvedConfig {
    let input = config(65_536.0);
    hoh_agent_instructions::config::ResolvedConfig {
        discovery: resolve_discovery_config(
            &input,
            Some(&dsh.to_string_lossy()),
            Some(&agents.to_string_lossy()),
        )
        .unwrap(),
        max_bytes: input.max_bytes,
        max_source_bytes,
    }
}

async fn local_fs(root: &Path) -> (Kernel, Context, hoh_kernel::Service<Fs>) {
    let kernel = Kernel::start();
    let context = kernel.root();
    let fiber = context
        .plugin::<FsLocalPlugin>(json!({ "cwd": root }))
        .unwrap();
    fiber.settled().await.unwrap();
    let file_system = context.get::<Fs>().unwrap();
    (kernel, context, file_system)
}

fn set_change(path: &str, content: &str) -> AgentInstructionChange {
    AgentInstructionChange {
        action: InstructionAction::Set,
        scope: hoh_agent_instructions::render::instruction_scope_key(path),
        path: path.into(),
        digest: Some(instruction_content_sha1(content)),
    }
}

#[test]
fn sha1_matches_standard_vectors_and_trimmed_identity() {
    assert_eq!(
        instruction_content_sha1(""),
        "da39a3ee5e6b4b0d3255bfef95601890afd80709"
    );
    assert_eq!(
        instruction_content_sha1("abc"),
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
    assert_eq!(
        trimmed_instruction_digest(" \tabc\r\n"),
        instruction_content_sha1("abc")
    );
}

#[test]
fn invalid_candidate_path_segments_are_filtered() {
    let mut input = config(1000.0);
    input.instruction_file_candidates = vec![
        "AGENTS.md".into(),
        "".into(),
        ".".into(),
        "..".into(),
        "nested/AGENTS.md".into(),
        "nested\\AGENTS.md".into(),
        "CLAUDE.md".into(),
    ];
    let resolved = resolve_discovery_config(&input, Some("/tmp/dsh"), Some("/tmp/agents")).unwrap();
    assert_eq!(
        resolved.instruction_file_candidates,
        ["AGENTS.md", "CLAUDE.md"]
    );
}

#[test]
fn baseline_identity_changes_with_discovery_or_budget_semantics() {
    let left = hoh_agent_instructions::config::resolve_config(&config(1000.0)).unwrap();
    let right = hoh_agent_instructions::config::resolve_config(&config(999.0)).unwrap();
    assert_ne!(
        workspace_baseline_identity(&left, "/repo/pkg", "/repo"),
        workspace_baseline_identity(&right, "/repo/pkg", "/repo")
    );
}

#[test]
fn scope_keys_round_trip_nul_separated_candidates() {
    let key = candidate_scope_key("pkg", "AGENTS.md");
    assert_eq!(decode_scope_key(&key), ("pkg".into(), "AGENTS.md".into()));
    assert_eq!(decode_scope_key("legacy"), ("legacy".into(), String::new()));
}

#[test]
fn config_requires_max_bytes_and_applies_array_defaults() {
    assert!(serde_json::from_value::<Config>(serde_json::json!({})).is_err());
    let parsed: Config = serde_json::from_value(serde_json::json!({ "maxBytes": 4096 })).unwrap();
    assert_eq!(parsed.project_root_markers, [".git"]);
    assert_eq!(
        parsed.instruction_file_candidates,
        ["AGENTS.md", "CLAUDE.md"]
    );
    assert_eq!(parsed.max_source_bytes, DEFAULT_MAX_SOURCE_BYTES);
}

#[test]
fn project_paths_colliding_with_reserved_global_labels_are_escaped() {
    assert_eq!(
        hoh_agent_instructions::render::escape_project_display_path("user-global/AGENTS.md"),
        "./user-global/AGENTS.md"
    );
    assert_eq!(
        hoh_agent_instructions::render::escape_project_display_path("agents-global/AGENTS.md"),
        "./agents-global/AGENTS.md"
    );
}

#[test]
fn two_user_global_roots_share_one_trimmed_content_group() {
    let kept = dedup_instruction_files_by_candidate_group(&[
        file("$DSH_HOME/AGENTS.md", " same\n"),
        file("$DSH_AGENTS_HOME/AGENTS.md", "same"),
    ]);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].display_path, "$DSH_HOME/AGENTS.md");
}

#[test]
fn separate_global_and_project_chains_each_receive_the_full_budget() {
    let rendered = render_agent_instructions(
        &[
            file("$DSH_HOME/AGENTS.md", &"g".repeat(500)),
            file("AGENTS.md", &"p".repeat(500)),
        ],
        400.0,
        false,
    );
    assert!(rendered.text.len() <= 800);
    assert!(
        rendered
            .truncated
            .iter()
            .any(|item| item.display_path == "$DSH_HOME/AGENTS.md")
    );
    assert!(
        rendered
            .truncated
            .iter()
            .any(|item| item.display_path == "AGENTS.md")
    );
}

#[test]
fn replacement_of_an_empty_baseline_explicitly_supersedes_old_state() {
    let rendered = render_agent_instructions(&[], 1024.0, true);
    assert!(rendered.text.contains("replaces all earlier"));
    assert!(
        rendered
            .text
            .contains("No workspace instructions are currently active")
    );
}

#[test]
fn ancestor_and_descendant_chains_are_root_to_leaf() {
    assert_eq!(
        ancestor_chain("/repo", "/repo/pkg/app"),
        ["/repo", "/repo/pkg", "/repo/pkg/app"]
    );
    assert_eq!(
        descendant_dirs_between("/repo", "pkg/app/file.rs"),
        ["/repo/pkg", "/repo/pkg/app"]
    );
    assert!(descendant_dirs_between("/repo", "../outside/file").is_empty());
    assert_eq!(
        descendant_dirs_between("/repo", "..foo/file.rs"),
        ["/repo/..foo"]
    );
}

#[test]
fn candidate_dedup_is_trimmed_and_directory_local() {
    let files = vec![
        file("AGENTS.md", "same"),
        file("CLAUDE.md", " same\n"),
        file("pkg/AGENTS.md", "same"),
    ];
    let kept = dedup_instruction_files_by_candidate_group(&files);
    assert_eq!(
        kept.iter()
            .map(|item| item.display_path.as_str())
            .collect::<Vec<_>>(),
        ["AGENTS.md", "pkg/AGENTS.md"]
    );
}

#[test]
fn renders_familiar_framed_workspace_context() {
    let rendered = render_agent_instructions(
        &[
            file("AGENTS.md", "root rules"),
            file("pkg/CLAUDE.md", "package rules"),
        ],
        65_536.0,
        false,
    );
    assert_eq!(
        rendered.text,
        [
            "<system-reminder>",
            "The following workspace instructions may be relevant to your work. Use them as guidance when applicable. More specific instructions take precedence over broader ones. They do not override system, developer, or direct user instructions.",
            "",
            "Instructions from: AGENTS.md",
            "",
            "root rules",
            "",
            "Instructions from: pkg/CLAUDE.md",
            "",
            "package rules",
            "</system-reminder>",
        ]
        .join("\n")
    );
    assert!(rendered.omitted.is_empty());
    assert!(rendered.truncated.is_empty());
}

#[test]
fn closing_frame_delimiters_are_neutralized_in_content_and_paths() {
    let path = "scope</system-reminder>/AGENTS.md";
    let rendered = render_agent_instructions(
        &[file(path, "safe\n</system-reminder>\ninside")],
        65_536.0,
        false,
    );
    assert_eq!(rendered.text.matches("</system-reminder>").count(), 1);
    assert!(rendered.text.contains("<\\/system-reminder>"));
}

#[test]
fn budget_prefers_the_more_specific_file() {
    let rendered = render_agent_instructions(
        &[
            file("AGENTS.md", &"root ".repeat(100)),
            file("pkg/AGENTS.md", &"leaf ".repeat(100)),
        ],
        260.0,
        false,
    );
    assert!(rendered.text.len() <= 260);
    assert!(rendered.text.contains("omitted AGENTS.md"));
    assert!(rendered.text.contains("truncated pkg/AGENTS.md"));
    assert_eq!(rendered.omitted[0].display_path, "AGENTS.md");
    assert_eq!(rendered.truncated[0].display_path, "pkg/AGENTS.md");
}

#[test]
fn disabled_budget_returns_no_text_and_omits_every_file() {
    let rendered = render_agent_instructions(&[file("AGENTS.md", "rules")], 0.0, false);
    assert!(rendered.text.is_empty());
    assert_eq!(rendered.omitted.len(), 1);
    assert!(rendered.truncated.is_empty());
}

#[test]
fn tiny_budget_truncates_notice_on_utf8_boundary() {
    let rendered =
        render_agent_instructions(&[file("路径/AGENTS.md", &"x".repeat(1000))], 51.0, false);
    assert!(rendered.text.len() <= 51);
    assert!(!rendered.text.contains('�'));
}

#[test]
fn compact_notice_is_exact_when_heading_does_not_fit() {
    let rendered =
        render_agent_instructions(&[file("pkg/AGENTS.md", &"x".repeat(1000))], 100.0, false);
    assert_eq!(
        rendered.text,
        "Workspace instruction budget 100 bytes: truncated pkg/AGENTS.md from 1000 to 0 bytes"
    );
    assert_eq!(rendered.truncated[0].included_bytes, 0);
}

#[test]
fn change_is_not_committed_when_only_generic_notice_survives() {
    let change = AgentInstructionChange {
        action: InstructionAction::Set,
        scope: candidate_scope_key("pkg", "AGENTS.md"),
        path: "pkg/AGENTS.md".into(),
        digest: Some("digest".into()),
    };
    let (text, changes) = render_instruction_changes(
        &[ChangeRenderItem {
            change,
            file: file("pkg/AGENTS.md", &"x".repeat(1000)),
        }],
        20.0,
    );
    assert_eq!(text, "Workspace instructio");
    assert!(changes.is_empty());
}

#[test]
fn replacement_change_commits_when_semantic_section_survives() {
    let change = AgentInstructionChange {
        action: InstructionAction::Replace,
        scope: candidate_scope_key("pkg", "AGENTS.md"),
        path: "pkg/AGENTS.md".into(),
        digest: Some("digest".into()),
    };
    let (text, changes) = render_instruction_changes(
        &[ChangeRenderItem {
            change: change.clone(),
            file: file("pkg/AGENTS.md", &"x".repeat(1000)),
        }],
        400.0,
    );
    assert!(text.contains("Updated instructions from: pkg/AGENTS.md"));
    assert_eq!(changes, [change]);
}

#[test]
fn dynamic_replace_budget_counts_only_instruction_body_bytes() {
    let change = AgentInstructionChange {
        action: InstructionAction::Replace,
        scope: candidate_scope_key("pkg", "AGENTS.md"),
        path: "pkg/AGENTS.md".into(),
        digest: Some("digest".into()),
    };
    let (text, changes) = render_instruction_changes(
        &[ChangeRenderItem {
            change: change.clone(),
            file: file("pkg/AGENTS.md", &"x".repeat(5000)),
        }],
        600.0,
    );
    assert!(text.contains("Updated instructions from: pkg/AGENTS.md"));
    assert!(text.contains("truncated pkg/AGENTS.md from 5000 to "));
    assert_eq!(changes, [change]);
}

#[test]
fn dynamic_remove_is_never_committed_as_a_partial_sentence() {
    let change = AgentInstructionChange {
        action: InstructionAction::Remove,
        scope: candidate_scope_key("pkg", "AGENTS.md"),
        path: "pkg/AGENTS.md".into(),
        digest: None,
    };
    let complete = "Instructions removed: pkg/AGENTS.md\n\nThe previously loaded instructions from this file no longer apply.";
    for budget in 1..complete.len() {
        let (text, changes) = render_instruction_changes(
            &[ChangeRenderItem {
                change: change.clone(),
                file: file("pkg/AGENTS.md", ""),
            }],
            budget as f64,
        );
        if !changes.is_empty() {
            assert!(text.contains(complete));
        }
    }
}

#[test]
fn baseline_source_fields_keep_recorded_session_order() {
    let message = hoh_agent_instructions::state::baseline_context_message(
        vec![hoh_llm::ContentBlock::text("rules")],
        &[],
        "identity",
    );
    assert_eq!(
        message
            .source
            .data
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["form", "baseline", "baselineIdentity", "changes"]
    );
}

#[test]
fn empty_instruction_is_still_represented_when_heading_survives() {
    let input = file("pkg/AGENTS.md", "");
    let (rendered, included) =
        render_agent_instruction_set(std::slice::from_ref(&input), 117.0, false);
    assert!(rendered.text.contains("Instructions from: pkg/AGENTS.md"));
    assert_eq!(included, [input]);
}

#[test]
fn baseline_state_has_digests_but_no_invented_provider_versions() {
    let input = file("AGENTS.md", "rules");
    let (changes, versions) = baseline_instruction_state(&[input]);
    let change = changes.values().next().unwrap();
    assert_eq!(change.action, InstructionAction::Set);
    assert_eq!(
        change.digest.as_deref(),
        Some(instruction_content_sha1("rules").as_str())
    );
    assert!(versions.is_empty());
}

#[test]
fn version_updates_are_retained_only_for_exact_rendered_transition() {
    let change = AgentInstructionChange {
        action: InstructionAction::Remove,
        scope: "scope".into(),
        path: "AGENTS.md".into(),
        digest: None,
    };
    let update = hoh_agent_instructions::state::InstructionVersionUpdate {
        change: change.clone(),
        state: None,
    };
    let retained = retained_instruction_version_updates(std::slice::from_ref(&update), &[change]);
    assert_eq!(retained.first(), Some(&update));
    assert_eq!(retained.len(), 1);
    assert!(retained_instruction_version_updates(&[update], &[]).is_empty());
}

#[tokio::test]
async fn host_discovery_loads_global_then_root_to_cwd_candidates() {
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    fs::create_dir(project.path().join(".git")).unwrap();
    fs::create_dir(project.path().join("pkg")).unwrap();
    fs::write(home.path().join("AGENTS.md"), "global").unwrap();
    fs::write(project.path().join("AGENTS.md"), "root").unwrap();
    fs::write(project.path().join("CLAUDE.md"), "root fallback").unwrap();
    fs::write(project.path().join("pkg/AGENTS.md"), "package").unwrap();
    let found = discover_baseline_instruction_files(
        &discovery(&project.path().join("pkg"), home.path()),
        None,
    )
    .await
    .unwrap();
    let names: Vec<_> = found
        .iter()
        .map(|item| item.display_path.as_str())
        .collect();
    assert_eq!(names[1..], ["AGENTS.md", "CLAUDE.md", "pkg/AGENTS.md"]);
    assert!(names[0].ends_with("/AGENTS.md"));
}

#[tokio::test]
async fn agents_home_only_uses_agents_md_not_project_fallback_names() {
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let agents_home = home.path().join("agents");
    fs::create_dir(project.path().join(".git")).unwrap();
    fs::create_dir(&agents_home).unwrap();
    fs::write(agents_home.join("CLAUDE.md"), "must stay ignored").unwrap();
    fs::write(agents_home.join("AGENTS.md"), "global agents rules").unwrap();

    let found = discover_baseline_instruction_files(&discovery(project.path(), home.path()), None)
        .await
        .unwrap();

    assert_eq!(found.len(), 1);
    assert!(found[0].display_path.ends_with("/AGENTS.md"));
}

#[cfg(unix)]
#[tokio::test]
async fn instruction_file_symlinks_are_loaded_like_their_targets() {
    use std::os::unix::fs::symlink;

    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    fs::create_dir(project.path().join(".git")).unwrap();
    fs::write(project.path().join("rules.md"), "linked rules").unwrap();
    symlink("rules.md", project.path().join("AGENTS.md")).unwrap();

    let loaded = load_baseline_instruction_set(
        &LoadOptions {
            discovery: discovery(project.path(), home.path()),
            max_bytes: 1024.0,
            max_source_bytes: 1024,
            replace_previous_baseline: false,
        },
        None,
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(loaded.included.len(), 1);
    assert_eq!(loaded.included[0].content, "linked rules");
}

#[tokio::test]
async fn default_local_overlay_loads_after_base_file() {
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    fs::create_dir(project.path().join(".git")).unwrap();
    fs::write(project.path().join("AGENTS.md"), "base").unwrap();
    fs::write(project.path().join("AGENTS.local.md"), "local").unwrap();
    let found = discover_baseline_instruction_files(&discovery(project.path(), home.path()), None)
        .await
        .unwrap();
    assert_eq!(
        found
            .iter()
            .map(|item| item.display_path.as_str())
            .collect::<Vec<_>>(),
        ["AGENTS.md", "AGENTS.local.md"]
    );
}

#[tokio::test]
async fn empty_local_candidate_list_disables_overlay_loading() {
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    fs::create_dir(project.path().join(".git")).unwrap();
    fs::write(project.path().join("AGENTS.md"), "base").unwrap();
    fs::write(project.path().join("AGENTS.local.md"), "local").unwrap();
    let mut options = discovery(project.path(), home.path());
    options.local_instruction_file_candidates = Some(vec![]);
    let found = discover_baseline_instruction_files(&options, None)
        .await
        .unwrap();
    assert_eq!(
        found
            .iter()
            .map(|item| item.display_path.as_str())
            .collect::<Vec<_>>(),
        ["AGENTS.md"]
    );
}

#[tokio::test]
async fn configured_candidates_exclude_unlisted_fallbacks() {
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    fs::create_dir(project.path().join(".git")).unwrap();
    fs::write(project.path().join("AGENTS.md"), "agents").unwrap();
    fs::write(project.path().join("CLAUDE.md"), "claude").unwrap();
    let mut options = discovery(project.path(), home.path());
    options.instruction_file_candidates = Some(vec!["CLAUDE.md".into()]);
    options.local_instruction_file_candidates = Some(vec![]);
    let found = discover_baseline_instruction_files(&options, None)
        .await
        .unwrap();
    assert_eq!(found[0].display_path, "CLAUDE.md");
    assert_eq!(found.len(), 1);
}

#[tokio::test]
async fn git_file_is_a_project_root_marker() {
    let outer = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let root = outer.path().join("root");
    let child = root.join("pkg");
    fs::create_dir_all(&child).unwrap();
    fs::write(root.join(".git"), "gitdir: elsewhere").unwrap();
    fs::write(outer.path().join("AGENTS.md"), "outside").unwrap();
    fs::write(root.join("AGENTS.md"), "inside").unwrap();
    let found = discover_baseline_instruction_files(&discovery(&child, home.path()), None)
        .await
        .unwrap();
    assert_eq!(
        found
            .iter()
            .map(|item| item.display_path.as_str())
            .collect::<Vec<_>>(),
        ["AGENTS.md"]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn inaccessible_host_root_marker_is_an_error_not_absence() {
    use std::os::unix::fs::PermissionsExt;

    let project = TempDir::new().unwrap();
    let locked = project.path().join("locked");
    let cwd = locked.join("child");
    fs::create_dir_all(&cwd).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let result = hoh_agent_instructions::files::find_project_root(
        &cwd.to_string_lossy(),
        &[".git".into()],
        None,
        None,
    )
    .await;
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
}

#[tokio::test]
async fn candidate_directories_are_not_instruction_files() {
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    fs::create_dir(project.path().join(".git")).unwrap();
    fs::create_dir(project.path().join("AGENTS.md")).unwrap();
    let found = discover_baseline_instruction_files(&discovery(project.path(), home.path()), None)
        .await
        .unwrap();
    assert!(found.is_empty());
}

#[tokio::test]
async fn both_user_global_roots_precede_project_files() {
    let project = TempDir::new().unwrap();
    let dsh = TempDir::new().unwrap();
    let agents = dsh.path().join("agents");
    fs::create_dir(&agents).unwrap();
    fs::create_dir(project.path().join(".git")).unwrap();
    fs::write(dsh.path().join("AGENTS.md"), "dsh").unwrap();
    fs::write(agents.join("AGENTS.md"), "agents").unwrap();
    fs::write(project.path().join("AGENTS.md"), "project").unwrap();
    let found = discover_baseline_instruction_files(&discovery(project.path(), dsh.path()), None)
        .await
        .unwrap();
    assert_eq!(found.len(), 3);
    assert!(found[0].display_path.ends_with("/AGENTS.md"));
    assert!(found[1].display_path.ends_with("/AGENTS.md"));
    assert_eq!(found[2].display_path, "AGENTS.md");
}

#[tokio::test]
async fn oversized_source_is_ignored_before_rendering() {
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    fs::create_dir(project.path().join(".git")).unwrap();
    fs::write(project.path().join("AGENTS.md"), "12345").unwrap();
    let loaded = load_baseline_instruction_set(
        &LoadOptions {
            discovery: discovery(project.path(), home.path()),
            max_bytes: 1000.0,
            max_source_bytes: 4,
            replace_previous_baseline: false,
        },
        None,
    )
    .await
    .unwrap();
    assert!(loaded.is_none());
}

#[tokio::test]
async fn zero_render_budget_disables_all_reads_and_output() {
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    fs::write(project.path().join("AGENTS.md"), "rules").unwrap();
    let loaded = load_baseline_instruction_set(
        &LoadOptions {
            discovery: discovery(project.path(), home.path()),
            max_bytes: 0.0,
            max_source_bytes: 100,
            replace_previous_baseline: false,
        },
        None,
    )
    .await
    .unwrap();
    assert!(loaded.is_none());
}

#[tokio::test]
async fn agents_global_duplicate_is_promoted_after_the_dsh_winner_changes() {
    let root = TempDir::new().unwrap();
    let project = root.path().join("project");
    let dsh = root.path().join("dsh");
    let agents = root.path().join("agents");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&dsh).unwrap();
    fs::create_dir_all(&agents).unwrap();
    fs::write(dsh.join("AGENTS.md"), "same").unwrap();
    fs::write(agents.join("AGENTS.md"), "same").unwrap();
    let (kernel, context, file_system) = local_fs(root.path()).await;
    let agent = test_agent(context, &project);
    let resolved = resolved_for(&dsh, &agents, 1024);
    let cache = InstructionVersionCache::default();
    let baseline = baseline_context_message(
        vec![],
        &[set_change("$DSH_HOME/AGENTS.md", "same")],
        "baseline",
    );
    let options = || ReconcileOptions {
        cwd: project.to_str().unwrap(),
        authority_messages: std::slice::from_ref(&baseline),
        scope_messages: &[],
        touched_paths: &[],
        include_baseline_scopes: true,
        excluded_baseline_scopes: None,
        project_root: Some(project.to_str().unwrap()),
        signal: None,
    };
    assert!(
        reconcile_instruction_context(agent.as_ref(), &resolved, &cache, &*file_system, options())
            .await
            .unwrap()
            .is_none()
    );
    fs::write(dsh.join("AGENTS.md"), "changed winner").unwrap();
    let update =
        reconcile_instruction_context(agent.as_ref(), &resolved, &cache, &*file_system, options())
            .await
            .unwrap()
            .unwrap();
    let changes = workspace_instruction_changes(&update.context.source);
    assert!(changes.iter().any(|change| {
        change.action == InstructionAction::Set
            && change.scope == candidate_scope_key("agents-global", "AGENTS.md")
    }));
    kernel.shutdown().await;
}

#[tokio::test]
async fn reconciliation_uses_the_ensured_cwd_instead_of_the_session_header() {
    let root = TempDir::new().unwrap();
    let project = root.path().join("project");
    let nested = project.join("pkg");
    let dsh = root.path().join("dsh");
    let agents = root.path().join("agents");
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir_all(&dsh).unwrap();
    fs::create_dir_all(&agents).unwrap();
    fs::write(project.join("AGENTS.md"), "root").unwrap();
    fs::write(nested.join("AGENTS.md"), "nested").unwrap();
    let (kernel, context, file_system) = local_fs(root.path()).await;
    let agent = test_agent(context, &project);
    let resolved = resolved_for(&dsh, &agents, 1024);
    let baseline = baseline_context_message(vec![], &[set_change("AGENTS.md", "root")], "baseline");
    let update = reconcile_instruction_context(
        agent.as_ref(),
        &resolved,
        &InstructionVersionCache::default(),
        &*file_system,
        ReconcileOptions {
            cwd: nested.to_str().unwrap(),
            authority_messages: &[baseline],
            scope_messages: &[],
            touched_paths: &[],
            include_baseline_scopes: true,
            excluded_baseline_scopes: None,
            project_root: Some(project.to_str().unwrap()),
            signal: None,
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        workspace_instruction_changes(&update.context.source)
            .iter()
            .any(|change| change.path == "pkg/AGENTS.md")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn unreadable_candidate_rolls_back_its_whole_deduplication_group() {
    let root = TempDir::new().unwrap();
    let project = root.path().join("project");
    let dsh = root.path().join("dsh");
    let agents = root.path().join("agents");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&dsh).unwrap();
    fs::create_dir_all(&agents).unwrap();
    fs::write(project.join("AGENTS.md"), "too large").unwrap();
    fs::write(project.join("CLAUDE.md"), "small").unwrap();
    let (kernel, context, file_system) = local_fs(root.path()).await;
    let agent = test_agent(context, &project);
    let resolved = resolved_for(&dsh, &agents, 5);
    let update = reconcile_instruction_context(
        agent.as_ref(),
        &resolved,
        &InstructionVersionCache::default(),
        &*file_system,
        ReconcileOptions {
            cwd: project.to_str().unwrap(),
            authority_messages: &[],
            scope_messages: &[],
            touched_paths: &[project.join("file.rs").to_string_lossy().into_owned()],
            include_baseline_scopes: true,
            excluded_baseline_scopes: None,
            project_root: Some(project.to_str().unwrap()),
            signal: None,
        },
    )
    .await
    .unwrap();
    assert!(update.is_none());
    kernel.shutdown().await;
}

#[tokio::test]
async fn hidden_duplicate_keeps_provider_metadata_in_the_version_cache() {
    let root = TempDir::new().unwrap();
    let project = root.path().join("project");
    let dsh = root.path().join("dsh");
    let agents = root.path().join("agents");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&dsh).unwrap();
    fs::create_dir_all(&agents).unwrap();
    fs::write(project.join("AGENTS.md"), "same").unwrap();
    fs::write(project.join("CLAUDE.md"), " same\n").unwrap();
    let (kernel, context, file_system) = local_fs(root.path()).await;
    let agent = test_agent(context, &project);
    let resolved = resolved_for(&dsh, &agents, 1024);
    let cache = InstructionVersionCache::default();
    let _ = reconcile_instruction_context(
        agent.as_ref(),
        &resolved,
        &cache,
        &*file_system,
        ReconcileOptions {
            cwd: project.to_str().unwrap(),
            authority_messages: &[],
            scope_messages: &[],
            touched_paths: &[project.join("file.rs").to_string_lossy().into_owned()],
            include_baseline_scopes: true,
            excluded_baseline_scopes: None,
            project_root: Some(project.to_str().unwrap()),
            signal: None,
        },
    )
    .await
    .unwrap();
    assert!(
        cache
            .states(agent.session())
            .contains_key(&candidate_scope_key(".", "CLAUDE.md"))
    );
    kernel.shutdown().await;
}
