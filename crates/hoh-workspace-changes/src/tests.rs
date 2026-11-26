use super::*;
use crate::{
    capture::{Capture, capture_file, same_capture},
    git::{
        GitLimits, GitRunner, blob_text, diff_trees, ignored_paths, locate_git_workspace,
        snapshot_tree, tree_blob,
    },
    recorder::{GitResolver, RecorderEnvironment, create_recorder},
};
use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
    MaintenanceTask, Model,
};
use hoh_error::BoxError;
use hoh_fs::{FsActor, FsTarget, FsWriteIntent, FsWriteIntentEvent};
use hoh_kernel::{Context, Kernel};
use hoh_llm::UserMessage;
use hoh_scope::ScopeKey;
use hoh_session::{
    AgentCancelCause, CreateSessionOptions, Session, SessionEvent, SessionPlugin, Sessions,
    SurfaceIntent,
};
use hoh_subprocess::Subprocess;
use hoh_subprocess_local::LocalSubprocessRuntime;
use hoh_timeout::AbortController;
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, ScheduledToolDispatch, ScheduledToolPreparation,
    ToolExecutionInput, ToolExecutionPipeline, define_tool,
};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
};
use tempfile::TempDir;

struct TestAgent {
    key: ScopeKey,
    session: Session,
    ctx: Context,
    options: AgentOptions,
}

impl Agent for TestAgent {
    fn id(&self) -> &hoh_session::SessionId {
        self.session.id()
    }

    fn scope_key(&self) -> ScopeKey {
        self.key.clone()
    }

    fn options(&self) -> &AgentOptions {
        &self.options
    }

    fn session(&self) -> &Session {
        &self.session
    }

    fn model(&self) -> &dyn Model {
        panic!("workspace-changes fixture has no model")
    }

    fn inbox(&self) -> &dyn Inbox {
        panic!("workspace-changes fixture has no inbox")
    }

    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        panic!("workspace-changes fixture does not cancel")
    }

    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance_erased(&self, _: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        panic!("workspace-changes fixture has no maintenance")
    }

    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        panic!("workspace-changes fixture has no inbox")
    }

    fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("workspace-changes fixture has no inbox")
    }

    fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("workspace-changes fixture has no inbox")
    }

    fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("workspace-changes fixture has no inbox")
    }
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git fixture");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git UTF-8 output")
}

fn repository() -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    git(directory.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(directory.path().join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(directory.path().join("same.txt"), "same\n").unwrap();
    std::fs::write(
        directory.path().join(".gitignore"),
        ".env*\n*.log\n!keep.log\n",
    )
    .unwrap();
    git(directory.path(), &["add", "-A"]);
    git(directory.path(), &["commit", "-q", "-m", "init"]);
    directory
}

async fn context_with_runtime() -> (Kernel, Context) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<LocalSubprocessRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    (kernel, ctx)
}

fn session(ctx: &Context, cwd: &Path, id: &str) -> Session {
    ctx.get::<Sessions>()
        .unwrap()
        .create(
            ctx,
            Some(id.into()),
            CreateSessionOptions {
                meta: Some(json!({ "cwd": cwd })),
                ..Default::default()
            },
        )
        .unwrap()
}

fn tool_result(turn: u64, seq: u64) -> SessionEvent {
    SessionEvent::from_value(json!({
        "type": "tool/result",
        "seq": seq,
        "time": seq,
        "data": { "turn": turn }
    }))
}

fn announced(session: &Session) -> u64 {
    session
        .snapshot_events(None, None)
        .iter()
        .rev()
        .find(|event| event.event_type() == "workspace/changes")
        .expect("workspace/changes event")
        .seq()
        .get()
}

fn append_tool_result(session: &Session, turn: u64) {
    session
        .append(
            "tool/result",
            json!({
                "turn": turn,
                "step": 1,
                "message": {
                    "id": format!("result-{turn}"),
                    "role": "tool",
                    "content": [],
                    "source": { "kind": "tool", "callId": format!("call-{turn}") },
                    "isError": false
                }
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
}

#[test]
fn parses_plain_binary_rename_and_unusual_numstat_names() {
    let output = [
        "3\t1\tsrc/a.ts",
        "-\t-\timg.png",
        "2\t0\t",
        "old\tx",
        "new\ty",
        "",
    ]
    .join("\0");
    assert_eq!(
        parse_numstat(&output).unwrap(),
        vec![
            NumstatEntry {
                path: "src/a.ts".into(),
                old_path: None,
                added: 3,
                deleted: 1,
                binary: false,
            },
            NumstatEntry {
                path: "img.png".into(),
                old_path: None,
                added: 0,
                deleted: 0,
                binary: true,
            },
            NumstatEntry {
                path: "new\ty".into(),
                old_path: Some("old\tx".into()),
                added: 2,
                deleted: 0,
                binary: false,
            },
        ]
    );
    assert!(
        parse_numstat("1\t1\ta.txt")
            .unwrap_err()
            .to_string()
            .contains("NUL")
    );
    assert!(
        parse_numstat("garbage\0")
            .unwrap_err()
            .to_string()
            .contains("malformed")
    );
    assert!(
        parse_numstat("1\t0\t\0old\0")
            .unwrap_err()
            .to_string()
            .contains("rename")
    );
    assert!(parse_numstat("").unwrap().is_empty());
}

#[test]
fn compares_text_with_context_counts_and_missing_final_newline_semantics() {
    let before = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"].join("\n");
    let after = ["a", "b", "c", "D", "e", "f", "g", "h", "i", "j", "k"].join("\n");
    let result = compare_text(Some(&before), Some(&after), 100);
    assert!(!result.coarse);
    assert_eq!((result.added, result.deleted), (2, 1));
    assert_eq!(result.hunks.len(), 1);
    assert_eq!(
        result.hunks[0],
        WorkspaceDiffHunk {
            old_start: 1,
            old_lines: 10,
            new_start: 1,
            new_lines: 11,
            lines: [
                " a", " b", " c", "-d", "+D", " e", " f", " g", " h", " i", " j", "+k",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        }
    );
    assert_eq!(
        compare_text(Some("same"), Some("same\n"), 100),
        Comparison {
            hunks: vec![],
            coarse: false,
            added: 0,
            deleted: 0,
        }
    );
    let created = compare_text(None, Some("x\ny\n"), 100);
    assert_eq!((created.added, created.deleted), (2, 0));
    assert_eq!(
        (created.hunks[0].old_lines, created.hunks[0].new_lines),
        (0, 2)
    );
    assert_eq!(
        (
            compare_text(Some("x\n"), None, 100).added,
            compare_text(Some("x\n"), None, 100).deleted
        ),
        (0, 1)
    );
    assert_eq!(
        compare_text(None, None, 100),
        Comparison {
            hunks: vec![],
            coarse: false,
            added: 0,
            deleted: 0
        }
    );
}

#[test]
fn splits_far_changes_and_degrades_expensive_comparison_to_whole_file() {
    let stable = (0..20).map(|i| format!("line {i}")).collect::<Vec<_>>();
    let mut changed = stable.clone();
    changed[2] = "changed first".into();
    changed[18] = "changed last".into();
    let exact = compare_text(Some(&stable.join("\n")), Some(&changed.join("\n")), 100);
    assert_eq!(exact.hunks.len(), 2);

    let before = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"].join("\n");
    let after = ["a", "b", "c", "D", "e", "f", "g", "h", "i", "j", "k"].join("\n");
    let far_before = format!("{before}\n{}end", "z\n".repeat(10));
    let far_after = format!("{after}\n{}END", "z\n".repeat(10));
    assert_eq!(
        compare_text(Some(&far_before), Some(&far_after), 100)
            .hunks
            .iter()
            .map(|hunk| (
                hunk.old_start,
                hunk.old_lines,
                hunk.new_start,
                hunk.new_lines
            ))
            .collect::<Vec<_>>(),
        [(1, 13, 1, 14), (18, 4, 19, 4)]
    );

    let before = (0..4_000)
        .map(|i| format!("old {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let after = (0..4_000)
        .map(|i| format!("new {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let coarse = compare_text(Some(&before), Some(&after), 1);
    assert!(coarse.coarse);
    assert_eq!((coarse.added, coarse.deleted), (4_000, 4_000));
    assert_eq!(coarse.hunks.len(), 1);
    assert_eq!(coarse.hunks[0].lines.first().unwrap(), "-old 0");
    assert_eq!(coarse.hunks[0].lines.last().unwrap(), "+new 3999");
    let created = compare_text(None, Some(&format!("{after}\n{before}")), 1);
    assert!(created.coarse);
    assert_eq!((created.added, created.deleted), (8_000, 0));
}

#[test]
fn comparison_degrades_before_a_large_edit_distance_can_exhaust_memory() {
    let before = (0..50_000)
        .map(|index| format!("old-{index}\n"))
        .collect::<String>();
    let after = (0..50_000)
        .map(|index| format!("new-{index}\n"))
        .collect::<String>();
    let comparison = compare_text(Some(&before), Some(&after), 60_000);
    assert!(comparison.coarse);
    assert_eq!((comparison.added, comparison.deleted), (50_000, 50_000));
}

#[test]
fn comparison_keeps_exact_hunks_for_a_moderate_edit_in_a_large_file() {
    let before = (0..10_000)
        .map(|index| format!("line-{index}"))
        .collect::<Vec<_>>();
    let mut after = before.clone();
    for (offset, line) in after[4_500..4_560].iter_mut().enumerate() {
        *line = format!("replacement-{offset}");
    }
    let comparison = compare_text(Some(&before.join("\n")), Some(&after.join("\n")), 60_000);
    assert!(!comparison.coarse);
    assert_eq!((comparison.added, comparison.deleted), (60, 60));
    assert_eq!(comparison.hunks.len(), 1);
}

#[test]
fn computes_durable_and_display_paths_in_source_order() {
    let cwd = Path::new("/home/u/proj/pkg");
    let root = Path::new("/home/u/proj");
    let home = Path::new("/home/u");
    assert_eq!(
        display_path_of(
            Path::new("/home/u/proj/pkg/src/a.ts"),
            cwd,
            root,
            Some(home)
        ),
        "src/a.ts"
    );
    assert_eq!(
        display_path_of(Path::new("/home/u/proj/other/b.ts"), cwd, root, Some(home)),
        "../other/b.ts"
    );
    assert_eq!(
        display_path_of(Path::new("/home/u/.zshrc"), cwd, root, Some(home)),
        "~/.zshrc"
    );
    assert_eq!(
        display_path_of(Path::new("/etc/hosts"), cwd, root, Some(home)),
        "/etc/hosts"
    );
    assert_eq!(
        durable_path_of(Path::new("/home/u/proj/pkg/a.ts"), cwd),
        "a.ts"
    );
    assert_eq!(
        durable_path_of(Path::new("/home/u/proj/a.ts"), cwd),
        "/home/u/proj/a.ts"
    );

    let mut displays = ["~/x", "src/b", "/etc/h", "../a", "src/a"];
    displays.sort_by(|left, right| compare_display(left, right));
    assert_eq!(displays, ["../a", "/etc/h", "src/a", "src/b", "~/x"]);
    assert_eq!(compare_display("same", "same"), std::cmp::Ordering::Equal);
    assert_eq!(
        compare_display("\u{10000}", "\u{e000}"),
        std::cmp::Ordering::Less
    );
}

#[tokio::test]
async fn canonicalizes_missing_paths_through_a_directory_symlink_and_classifies_temp() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real");
    tokio::fs::create_dir_all(real.join("nested"))
        .await
        .unwrap();
    #[cfg(unix)]
    tokio::fs::symlink(&real, root.path().join("link"))
        .await
        .unwrap();
    #[cfg(unix)]
    assert_eq!(
        canonical_path(&root.path().join("link/nested/new.txt")).await,
        tokio::fs::canonicalize(&real)
            .await
            .unwrap()
            .join("nested/new.txt")
    );
    assert_eq!(
        canonical_path(Path::new("/definitely/missing/root/file")).await,
        PathBuf::from("/definitely/missing/root/file")
    );
    let roots = temporary_roots(&[PathBuf::from("/tmp"), std::env::temp_dir()]).await;
    assert!(is_temporary_path(&std::env::temp_dir().join("x"), &roots));
    assert!(!is_temporary_path(Path::new("/tmpfoo/x"), &roots));
}

#[tokio::test]
async fn captures_content_once_and_classifies_absent_oversized_binary_and_directory() {
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("captures");
    tokio::fs::write(root.path().join("a"), b"one\ntwo\n")
        .await
        .unwrap();
    tokio::fs::write(root.path().join("same"), b"one\ntwo\n")
        .await
        .unwrap();
    tokio::fs::write(root.path().join("big"), vec![b'x'; 17])
        .await
        .unwrap();
    tokio::fs::write(root.path().join("bin"), [65, 0, 66])
        .await
        .unwrap();
    tokio::fs::write(root.path().join("short-a"), "a\n")
        .await
        .unwrap();
    tokio::fs::write(root.path().join("short-b"), "b\n")
        .await
        .unwrap();
    tokio::fs::create_dir(root.path().join("dir"))
        .await
        .unwrap();
    let a = capture_file(&root.path().join("a"), &store, 16)
        .await
        .unwrap()
        .unwrap();
    let same = capture_file(&root.path().join("same"), &store, 16)
        .await
        .unwrap()
        .unwrap();
    assert!(same_capture(&a, &same));
    let mut entries = tokio::fs::read_dir(&store).await.unwrap();
    let mut count = 0;
    while entries.next_entry().await.unwrap().is_some() {
        count += 1;
    }
    assert_eq!(count, 1);
    assert_eq!(
        capture_file(&root.path().join("big"), &store, 16)
            .await
            .unwrap(),
        Some(Capture::Oversized)
    );
    assert_eq!(
        capture_file(&root.path().join("missing"), &store, 16)
            .await
            .unwrap(),
        Some(Capture::Absent)
    );
    assert!(matches!(
        capture_file(&root.path().join("bin"), &store, 16)
            .await
            .unwrap(),
        Some(Capture::File { binary: true, .. })
    ));
    let short_a = capture_file(&root.path().join("short-a"), &store, 16)
        .await
        .unwrap()
        .unwrap();
    let short_b = capture_file(&root.path().join("short-b"), &store, 16)
        .await
        .unwrap()
        .unwrap();
    assert!(!same_capture(&short_a, &short_b));
    assert_eq!(
        capture_file(&root.path().join("dir"), &store, 16)
            .await
            .unwrap(),
        None
    );
    assert!(!same_capture(&Capture::Oversized, &Capture::Oversized));
}

#[tokio::test]
async fn private_git_snapshots_diff_blobs_ignores_and_renames_without_touching_status() {
    let repo = repository();
    let (kernel, ctx) = context_with_runtime().await;
    let runner = GitRunner::new(
        ctx.get::<Subprocess>().unwrap(),
        "git".into(),
        GitLimits {
            timeout_ms: 30_000,
            output_max_bytes: 1024 * 1024,
        },
    );
    let private = tempfile::tempdir().unwrap();
    let scratch = private.path().join("store");
    tokio::fs::create_dir(&scratch).await.unwrap();
    let signal = AbortController::new().signal();
    let workspace = locate_git_workspace(
        &runner,
        repo.path(),
        || async { Ok(scratch.clone()) },
        signal.clone(),
    )
    .await
    .unwrap()
    .unwrap();
    let before = snapshot_tree(&runner, &workspace, signal.clone())
        .await
        .unwrap();
    tokio::fs::write(repo.path().join("a.txt"), "one\nTWO\nthree\n")
        .await
        .unwrap();
    tokio::fs::rename(repo.path().join("same.txt"), repo.path().join("moved.txt"))
        .await
        .unwrap();
    tokio::fs::write(repo.path().join(".env"), "secret\n")
        .await
        .unwrap();
    let after = snapshot_tree(&runner, &workspace, signal.clone())
        .await
        .unwrap();
    let changed = diff_trees(&runner, &workspace, &before, &after, signal.clone())
        .await
        .unwrap();
    assert_eq!(changed.len(), 2);
    assert!(
        changed
            .iter()
            .any(|entry| entry.path == "a.txt" && entry.added == 2 && entry.deleted == 1)
    );
    assert!(changed.iter().any(|entry| entry.path == "moved.txt" && entry.old_path.as_deref() == Some("same.txt")));
    assert_eq!(
        ignored_paths(
            &runner,
            &workspace,
            &[".env".into(), "a.txt".into()],
            signal.clone()
        )
        .await
        .unwrap(),
        [".env".to_owned()].into_iter().collect()
    );
    assert_eq!(
        ignored_paths(
            &runner,
            &workspace,
            &["keep.log".into(), "x.log".into()],
            signal.clone()
        )
        .await
        .unwrap(),
        ["x.log".to_owned()].into_iter().collect()
    );
    let mut candidates: Vec<String> = (0..300).map(|index| format!("visible-{index}")).collect();
    candidates.extend([".env-0".into(), ".env-128".into(), ".env-299".into()]);
    let ignored = ignored_paths(&runner, &workspace, &candidates, signal.clone())
        .await
        .unwrap();
    assert_eq!(
        ignored,
        [".env-0".into(), ".env-128".into(), ".env-299".into()]
            .into_iter()
            .collect()
    );
    let blob = tree_blob(&runner, &workspace, &after, "a.txt", signal.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        blob_text(&runner, &workspace, &blob.oid, 1024, signal)
            .await
            .unwrap(),
        "one\nTWO\nthree\n"
    );
    assert!(git(repo.path(), &["status", "--porcelain"]).contains(" M a.txt"));
    kernel.shutdown().await;
}

fn no_git_environment(temp_root: &Path, warnings: Arc<Mutex<Vec<String>>>) -> RecorderEnvironment {
    let git: GitResolver = Arc::new(|| Box::pin(async { Ok(None) }));
    RecorderEnvironment {
        git,
        temp_root: temp_root.to_path_buf(),
        max_files: 10,
        max_file_bytes: 16,
        diff_timeout_ms: 100,
        warn: Arc::new(move |warning| warnings.lock().unwrap().push(warning)),
    }
}

fn git_environment(
    ctx: &Context,
    temp_root: &Path,
    warnings: Arc<Mutex<Vec<String>>>,
) -> RecorderEnvironment {
    let runner = GitRunner::new(
        ctx.get::<Subprocess>().unwrap(),
        "git".into(),
        GitLimits {
            timeout_ms: 30_000,
            output_max_bytes: 1024 * 1024,
        },
    );
    let git: GitResolver = Arc::new(move || {
        let runner = runner.clone();
        Box::pin(async move { Ok(Some(runner)) })
    });
    RecorderEnvironment {
        git,
        temp_root: temp_root.to_path_buf(),
        max_files: 10,
        max_file_bytes: 1024,
        diff_timeout_ms: 1_000,
        warn: Arc::new(move |warning| warnings.lock().unwrap().push(warning)),
    }
}

#[tokio::test]
async fn negated_ignore_pattern_does_not_resurface_a_snapshot_covered_capture() {
    let repo = repository();
    tokio::fs::write(repo.path().join("keep.log"), "original\n")
        .await
        .unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let (kernel, ctx) = context_with_runtime().await;
    let subject = session(&ctx, repo.path(), "negated-ignore");
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let recorder = create_recorder(
        subject.clone(),
        repo.path().to_string_lossy().into_owned(),
        git_environment(&ctx, scratch.path(), warnings.clone()),
    );
    recorder.start(1);
    recorder.settled().await;
    tokio::fs::write(repo.path().join("keep.log"), "bash change\n")
        .await
        .unwrap();
    recorder.capture(repo.path().join("keep.log"));
    recorder.settled().await;
    tokio::fs::write(repo.path().join("keep.log"), "original\n")
        .await
        .unwrap();
    recorder.observe(&tool_result(1, 10));
    recorder.stopping(1).await;
    assert!(
        subject
            .snapshot_events(None, None)
            .iter()
            .all(|event| event.event_type() != "workspace/changes")
    );
    assert!(warnings.lock().unwrap().is_empty());
    recorder.dispose().await;
    kernel.shutdown().await;
}

#[tokio::test]
async fn diff_returns_none_when_lifetime_aborts_after_both_reads() {
    let root = tempfile::tempdir().unwrap();
    tokio::fs::write(root.path().join("text"), "before\n")
        .await
        .unwrap();
    let (kernel, ctx) = context_with_runtime().await;
    let subject = session(&ctx, root.path(), "diff-dispose-race");
    let recorder = create_recorder(
        subject.clone(),
        root.path().to_string_lossy().into_owned(),
        no_git_environment(root.path(), Arc::new(Mutex::new(Vec::new()))),
    );
    recorder.start(1);
    recorder.settled().await;
    recorder.capture(root.path().join("text"));
    recorder.settled().await;
    tokio::fs::write(root.path().join("text"), "after\n")
        .await
        .unwrap();
    recorder.observe(&tool_result(1, 10));
    recorder.stopping(1).await;
    let seq = announced(&subject);
    assert!(
        recorder
            .diff_aborting_after_read_for_test(seq, 0, AbortController::new().signal())
            .await
            .unwrap()
            .is_none()
    );
    recorder.dispose().await;
    kernel.shutdown().await;
}

#[tokio::test]
async fn recorder_without_git_tracks_first_baseline_net_change_binary_and_oversized() {
    let root = tempfile::tempdir().unwrap();
    tokio::fs::write(root.path().join("text"), "before\n")
        .await
        .unwrap();
    tokio::fs::write(root.path().join("binary"), [65, 0, 66])
        .await
        .unwrap();
    tokio::fs::write(root.path().join("large"), vec![b'a'; 20])
        .await
        .unwrap();
    tokio::fs::write(root.path().join("invalid"), [0xff, b'a', b'\n'])
        .await
        .unwrap();
    let (kernel, ctx) = context_with_runtime().await;
    let session = session(&ctx, root.path(), "plain");
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let recorder = create_recorder(
        session.clone(),
        root.path().to_string_lossy().into_owned(),
        no_git_environment(root.path(), warnings.clone()),
    );
    recorder.start(1);
    recorder.settled().await;
    for name in ["text", "binary", "invalid", "large", "new"] {
        recorder.capture(root.path().join(name));
        recorder.settled().await;
    }
    tokio::fs::write(root.path().join("text"), "after\nmore\n")
        .await
        .unwrap();
    tokio::fs::write(root.path().join("binary"), [66, 0, 67])
        .await
        .unwrap();
    tokio::fs::write(root.path().join("large"), vec![b'b'; 20])
        .await
        .unwrap();
    tokio::fs::write(root.path().join("invalid"), [0xfe, b'b', b'\n'])
        .await
        .unwrap();
    tokio::fs::write(root.path().join("new"), "created\n")
        .await
        .unwrap();
    recorder.observe(&tool_result(1, 10));
    recorder.stopping(1).await;
    let seq = announced(&session);
    let summary = recorder.summary(seq).unwrap();
    assert_eq!(summary.total, 5);
    assert_eq!(
        summary
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["binary", "invalid", "large", "new", "text"]
    );
    assert!(summary.files[0].binary);
    assert_eq!((summary.files[1].added, summary.files[1].deleted), (1, 1));
    assert!(summary.files[2].oversized);
    assert_eq!((summary.files[3].added, summary.files[3].deleted), (1, 0));
    assert_eq!((summary.files[4].added, summary.files[4].deleted), (2, 1));
    assert!(matches!(
        recorder
            .diff(seq, 0, AbortController::new().signal())
            .await
            .unwrap(),
        Some(WorkspaceFileDiff::Binary { .. })
    ));
    assert!(matches!(
        recorder
            .diff(seq, 2, AbortController::new().signal())
            .await
            .unwrap(),
        Some(WorkspaceFileDiff::Oversized { .. })
    ));
    assert!(matches!(
        recorder
            .diff(seq, 1, AbortController::new().signal())
            .await
            .unwrap(),
        Some(WorkspaceFileDiff::Text { .. })
    ));
    assert!(matches!(
        recorder
            .diff(seq, 4, AbortController::new().signal())
            .await
            .unwrap(),
        Some(WorkspaceFileDiff::Text {
            before: true,
            after: true,
            ..
        })
    ));
    assert!(warnings.lock().unwrap().is_empty());
    recorder.dispose().await;
    kernel.shutdown().await;
}

#[tokio::test]
async fn recorder_omits_restored_and_created_then_deleted_files_and_caps_only_the_file_list() {
    let root = tempfile::tempdir().unwrap();
    tokio::fs::write(root.path().join("restored"), "same\n")
        .await
        .unwrap();
    let (kernel, ctx) = context_with_runtime().await;
    let session = session(&ctx, root.path(), "net");
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let mut environment = no_git_environment(root.path(), warnings);
    environment.max_files = 1;
    let recorder = create_recorder(
        session.clone(),
        root.path().to_string_lossy().into_owned(),
        environment,
    );
    recorder.start(1);
    recorder.settled().await;
    for name in ["restored", "temporary", "a", "b"] {
        recorder.capture(root.path().join(name));
        recorder.settled().await;
    }
    tokio::fs::write(root.path().join("restored"), "same\n")
        .await
        .unwrap();
    tokio::fs::write(root.path().join("a"), "a\n")
        .await
        .unwrap();
    tokio::fs::write(root.path().join("b"), "b\n")
        .await
        .unwrap();
    recorder.observe(&tool_result(1, 1));
    recorder.stopping(1).await;
    let summary = recorder.summary(announced(&session)).unwrap();
    assert_eq!(summary.total, 2);
    assert_eq!(summary.files.len(), 1);
    assert_eq!(summary.files[0].path, "a");
    assert_eq!((summary.added, summary.deleted), (2, 0));

    recorder.start(2);
    recorder.settled().await;
    recorder.capture(root.path().join("a"));
    recorder.settled().await;
    tokio::fs::write(root.path().join("a"), "changed after disposal\n")
        .await
        .unwrap();
    recorder.observe(&tool_result(2, 2));
    recorder.end(2);
    recorder.dispose().await;
    assert!(recorder.summary(announced(&session)).is_none());
    kernel.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn recorder_scratch_directory_is_private() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let scratch_root = tempfile::tempdir().unwrap();
    tokio::fs::write(root.path().join("secret"), "value\n")
        .await
        .unwrap();
    let (kernel, ctx) = context_with_runtime().await;
    let subject = session(&ctx, root.path(), "permissions");
    let recorder = create_recorder(
        subject,
        root.path().to_string_lossy().into_owned(),
        no_git_environment(scratch_root.path(), Arc::new(Mutex::new(Vec::new()))),
    );
    recorder.start(1);
    recorder.settled().await;
    recorder.capture(root.path().join("secret"));
    recorder.settled().await;
    let mut entries = tokio::fs::read_dir(scratch_root.path()).await.unwrap();
    let directory = entries.next_entry().await.unwrap().unwrap().path();
    assert!(entries.next_entry().await.unwrap().is_none());
    assert_eq!(
        tokio::fs::metadata(directory)
            .await
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    recorder.dispose().await;
    kernel.shutdown().await;
}

#[tokio::test]
async fn plugin_registers_service_ignores_ineligible_sessions_and_rejects_zero_bounds() {
    let root = tempfile::tempdir().unwrap();
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<LocalSubprocessRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<hoh_fs_local::FsLocalPlugin>(json!({ "cwd": root.path() }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<WorkspaceChangesPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    assert!(ctx.get::<WorkspaceChanges>().is_some());
    let child = ctx
        .get::<Sessions>()
        .unwrap()
        .create(
            &ctx,
            Some("child".into()),
            CreateSessionOptions {
                meta: Some(
                    json!({ "cwd": root.path(), "origin": "subagent", "delegationDepth": 1 }),
                ),
                ..Default::default()
            },
        )
        .unwrap();
    child.append("turn/start", json!({"turn":1}), None).unwrap();
    assert!(
        child
            .snapshot_events(None, None)
            .iter()
            .all(|event| event.event_type() != "workspace/changes")
    );
    kernel.shutdown().await;

    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<LocalSubprocessRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<hoh_fs_local::FsLocalPlugin>(json!({ "cwd": root.path() }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let error = ctx
        .plugin::<WorkspaceChangesPlugin>(json!({ "maxFiles": 0 }))
        .unwrap()
        .settled()
        .await
        .unwrap_err();
    assert!(error.to_string().contains("positive integer maxFiles"));
    let error = ctx
        .plugin::<WorkspaceChangesPlugin>(json!({
            "outputMaxBytes": 9_007_199_254_740_992_u64
        }))
        .unwrap()
        .settled()
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("positive integer outputMaxBytes")
    );
    assert!(serde_json::from_value::<Config>(json!({ "diffTimeoutMs": 1.5 })).is_err());
    kernel.shutdown().await;
}

#[tokio::test]
async fn tool_actor_write_intent_captures_before_the_mutation() {
    let root = tempfile::tempdir().unwrap();
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<LocalSubprocessRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<hoh_system_prompt::SystemPrompt>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<hoh_tools::ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<hoh_fs_local::FsLocalPlugin>(json!({ "cwd": root.path() }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<WorkspaceChangesPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();

    let subject = session(&ctx, root.path(), "actor-write");
    subject
        .append("turn/start", json!({ "turn": 1 }), None)
        .unwrap();
    let agent: Arc<dyn Agent> = Arc::new(TestAgent {
        key: ScopeKey::new(),
        session: subject.clone(),
        ctx: ctx.clone(),
        options: AgentOptions::default(),
    });
    let action_ctx = ctx.clone();
    let action_path = root.path().join("actor.txt");
    let definition = Arc::new(
        define_tool(DefineToolOptions::new(
            "workspace-changes-test",
            "write through a real tool body",
            json!({}),
            DefineToolOutput {
                schema: json!({ "type": "null" }),
                render: Arc::new(|_, _| Vec::new()),
                presentation_meta: None,
            },
            move |_, exec| {
                let action_ctx = action_ctx.clone();
                let action_path = action_path.clone();
                Box::pin(async move {
                    let actor: FsActor = exec;
                    let _: Option<FsWriteIntent> = action_ctx
                        .waterfall(
                            FsWriteIntentEvent {
                                target: FsTarget {
                                    target_key: action_path.to_string_lossy().into_owned().into(),
                                    display_path: "actor.txt".into(),
                                },
                                actor: Some(actor),
                            },
                            |_| async { Ok(None) },
                        )
                        .await?;
                    tokio::fs::write(action_path, "created by tool\n").await?;
                    Ok(serde_json::Value::Null)
                })
            },
        ))
        .unwrap(),
    );
    let source = hoh_tools::ToolExecutionSource {
        get: Some(Arc::new(move |name, _, _| {
            (name == "workspace-changes-test").then(|| definition.clone())
        })),
        reject: None,
    };
    let pipeline = ToolExecutionPipeline::with_source(ctx.clone(), source).unwrap();
    let mut input = ToolExecutionInput::new(
        "actor-write-call".into(),
        "workspace-changes-test",
        json!({}),
        AbortController::new().signal(),
    );
    input.agent = Some(agent);
    let prepared = pipeline.prepare(input).await.unwrap();
    let exec = prepared.exec().clone();
    let outcome = match prepared {
        ScheduledToolPreparation::Dispatch(exec) => pipeline.dispatch(exec).await,
        ScheduledToolPreparation::PostResult { result, .. } => {
            ScheduledToolDispatch::PostResult(result)
        }
        ScheduledToolPreparation::FinalResult { result, .. } => {
            ScheduledToolDispatch::FinalResult(result)
        }
    };
    let result = pipeline.complete(&exec, outcome).await;
    assert!(!result.is_error(), "{result:?}");

    append_tool_result(&subject, 1);
    subject
        .append(
            "turn/end",
            json!({ "turn": 1, "reason": { "kind": "completed" } }),
            None,
        )
        .unwrap();
    let seq = {
        let mut found = None;
        for _ in 0..200 {
            found = subject
                .snapshot_events(None, None)
                .iter()
                .rev()
                .find(|event| event.event_type() == "workspace/changes")
                .map(|event| event.seq().get());
            if found.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        found.expect("workspace/changes announcement")
    };
    let service = ctx.get::<WorkspaceChanges>().unwrap();
    let summary = service.summary(subject.id(), seq).unwrap();
    assert_eq!(summary.total, 1);
    assert_eq!(summary.files[0].path, "actor.txt");
    assert_eq!((summary.files[0].added, summary.files[0].deleted), (1, 0));
    assert!(matches!(
        service
            .diff(subject.id(), seq, 0, AbortController::new().signal())
            .await
            .unwrap(),
        Some(WorkspaceFileDiff::Text {
            before: false,
            after: true,
            ..
        })
    ));
    kernel.shutdown().await;
}

#[tokio::test]
async fn plugin_records_real_git_turn_and_serves_its_snapshot_comparison() {
    let repo = repository();
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<LocalSubprocessRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<hoh_system_prompt::SystemPrompt>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<hoh_tools::ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<hoh_fs_local::FsLocalPlugin>(json!({ "cwd": repo.path() }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<WorkspaceChangesPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let subject = session(&ctx, repo.path(), "plugin-real");
    subject
        .append("turn/start", json!({ "turn": 1 }), None)
        .unwrap();
    // Wait for the Git baseline through the same pre-execute gate a shell tool uses.
    // The direct write below has no filesystem actor, so only Git can report the change.
    let agent = Arc::new(TestAgent {
        key: ScopeKey::new(),
        session: subject.clone(),
        ctx: ctx.clone(),
        options: AgentOptions::default(),
    });
    let definition = Arc::new(
        define_tool(DefineToolOptions::new(
            "workspace-changes-shell",
            "simulate a command that changes the worktree",
            json!({}),
            DefineToolOutput {
                schema: json!({ "type": "null" }),
                render: Arc::new(|_, _| Vec::new()),
                presentation_meta: None,
            },
            |_, _| Box::pin(async { Ok(serde_json::Value::Null) }),
        ))
        .unwrap(),
    );
    let source = hoh_tools::ToolExecutionSource {
        get: Some(Arc::new(move |name, _, _| {
            (name == "workspace-changes-shell").then(|| definition.clone())
        })),
        reject: None,
    };
    let pipeline = ToolExecutionPipeline::with_source(ctx.clone(), source).unwrap();
    let mut input = ToolExecutionInput::new(
        "shell-edit".into(),
        "workspace-changes-shell",
        json!({}),
        AbortController::new().signal(),
    );
    input.agent = Some(agent);
    assert!(matches!(
        pipeline.prepare(input).await.unwrap(),
        ScheduledToolPreparation::Dispatch(_)
    ));
    tokio::fs::write(repo.path().join("a.txt"), "one\nTWO\nthree\n")
        .await
        .unwrap();
    append_tool_result(&subject, 1);
    subject
        .append(
            "turn/end",
            json!({ "turn": 1, "reason": { "kind": "completed" } }),
            None,
        )
        .unwrap();
    let seq = {
        let mut found = None;
        for _ in 0..200 {
            found = subject
                .snapshot_events(None, None)
                .iter()
                .rev()
                .find(|event| event.event_type() == "workspace/changes")
                .map(|event| event.seq().get());
            if found.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        found.expect("workspace/changes announcement")
    };
    let service = ctx.get::<WorkspaceChanges>().unwrap();
    let summary = service.summary(subject.id(), seq).unwrap();
    assert_eq!(summary.turn, 1);
    assert_eq!(summary.total, 1);
    assert_eq!(summary.files[0].path, "a.txt");
    assert_eq!((summary.files[0].added, summary.files[0].deleted), (2, 1));
    assert!(summary.snapshot.is_some());
    let comparison = service
        .diff(subject.id(), seq, 0, AbortController::new().signal())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        comparison,
        WorkspaceFileDiff::Text {
            before: true,
            after: true,
            ..
        }
    ));
    kernel.shutdown().await;
}
