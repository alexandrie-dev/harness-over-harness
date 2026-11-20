use futures_util::future::BoxFuture;
use hoh_commander::Output;
use hoh_headless::{
    HeadlessRuntime, HeadlessRuntimeKey, HeadlessStartupKey, HeadlessStartupPlugin,
};
use hoh_kernel::ServiceKey;
use hoh_kernel::{Context, Fiber, Kernel};
use hoh_loader::cmdline::{AppExit, CmdlineHost, provide_cmdline};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, MutexGuard};

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct Fixture {
    kernel: Kernel,
    ctx: Context,
    _fiber: Fiber,
    exits: Arc<Mutex<Vec<i32>>>,
    stdout: Arc<Mutex<String>>,
    stderr: Arc<Mutex<String>>,
}

async fn start(args: &[&str], tty: bool) -> Fixture {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let exits = Arc::new(Mutex::new(Vec::new()));
    let observed_exits = exits.clone();
    provide_cmdline(
        &ctx,
        CmdlineHost {
            args: args.iter().map(|argument| (*argument).into()).collect(),
            exit: AppExit::new(move |code| lock(&observed_exits).push(code)),
            ready: None,
        },
    )
    .await
    .unwrap();
    let stdout = Arc::new(Mutex::new(String::new()));
    let stderr = Arc::new(Mutex::new(String::new()));
    let command_out = stdout.clone();
    let command_err = stderr.clone();
    let sink_out = stdout.clone();
    let sink_err = stderr.clone();
    ctx.provide::<HeadlessRuntimeKey>(Arc::new(HeadlessRuntime {
        output: Output::new(
            move |text| lock(&command_out).push_str(text),
            move |text| lock(&command_err).push_str(text),
        ),
        stdout: Arc::new(move |text: &str| lock(&sink_out).push_str(text)),
        stderr: Arc::new(move |text: &str| lock(&sink_err).push_str(text)),
        stdin_is_tty: Arc::new(move || tty),
        read_stdin: Arc::new(|| {
            Box::pin(async { Ok(String::new()) }) as BoxFuture<'static, std::io::Result<String>>
        }),
    }))
    .await
    .unwrap();
    let fiber = ctx.plugin::<HeadlessStartupPlugin>(json!({})).unwrap();
    fiber.settled().await.unwrap();
    Fixture {
        kernel,
        ctx,
        _fiber: fiber,
        exits,
        stdout,
        stderr,
    }
}

#[tokio::test]
async fn publishes_joined_task_and_all_options() {
    let fixture = start(
        &[
            "--session-id",
            " session ",
            "--permission-mode",
            " workspace-write ",
            "--approval-policy",
            "never",
            "--add-dir",
            "/one",
            "--add-dir",
            "/one",
            "hello",
            "world",
        ],
        false,
    )
    .await;
    let startup = fixture.ctx.get::<HeadlessStartupKey>().unwrap();
    assert_eq!(startup.task.as_deref(), Some("hello world"));
    assert_eq!(startup.session_id.as_deref(), Some(" session "));
    assert_eq!(startup.permission_mode.as_deref(), Some("workspace-write"));
    assert_eq!(startup.approval_policy.as_deref(), Some("never"));
    assert_eq!(startup.additional_writable_roots, Some(vec!["/one".into()]));
    assert!(lock(&fixture.exits).is_empty());
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn absent_task_is_published_for_non_terminal_stdin() {
    let fixture = start(&[], false).await;
    let startup = fixture.ctx.get::<HeadlessStartupKey>().unwrap();
    assert_eq!(startup.task, None);
    let view = HeadlessStartupKey::view(&startup).unwrap();
    assert!(view.get("task").is_none());
    assert!(view.get("sessionId").is_none());
    assert!(view.get("additionalWritableRoots").is_none());
    fixture.kernel.shutdown().await;
}

async fn assert_usage_error(args: &[&str], tty: bool, expected: &str) {
    let fixture = start(args, tty).await;
    assert!(fixture.ctx.get::<HeadlessStartupKey>().is_none());
    assert_eq!(*lock(&fixture.exits), [1]);
    assert_eq!(lock(&fixture.stderr).trim_end(), expected);
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn terminal_stdin_requires_a_task() {
    assert_usage_error(
        &[],
        true,
        "error: a task is required, for example: dsh --profile headless \"run the tests\"",
    )
    .await;
}

#[tokio::test]
async fn stdin_marker_must_be_the_only_task_argument() {
    assert_usage_error(
        &["-", "other"],
        false,
        "error: `-` must be the only task argument",
    )
    .await;
}

#[tokio::test]
async fn whitespace_task_is_rejected() {
    assert_usage_error(
        &[" ", "  "],
        false,
        "error: a task is required, for example: dsh --profile headless \"run the tests\"",
    )
    .await;
}

#[tokio::test]
async fn whitespace_session_id_is_rejected_without_trimming_valid_ids() {
    assert_usage_error(
        &["--session-id", " ", "task"],
        false,
        "error: --session-id requires a non-empty session id",
    )
    .await;
}

#[tokio::test]
async fn whitespace_permission_mode_is_rejected() {
    assert_usage_error(
        &["--permission-mode", " ", "task"],
        false,
        "error: --permission-mode requires a non-empty preset name",
    )
    .await;
}

#[tokio::test]
async fn relative_additional_directory_is_rejected() {
    assert_usage_error(
        &["--add-dir", "relative", "task"],
        false,
        "error: --add-dir requires an absolute directory path in the execution environment",
    )
    .await;
}

#[tokio::test]
async fn json_grammar_error_is_an_stdout_event_without_commander_prefix() {
    let fixture = start(&["--json", "--unknown"], false).await;
    assert!(fixture.ctx.get::<HeadlessStartupKey>().is_none());
    assert_eq!(*lock(&fixture.exits), [1]);
    assert!(lock(&fixture.stderr).is_empty());
    let event: Value = serde_json::from_str(lock(&fixture.stdout).trim()).unwrap();
    assert_eq!(event["type"], "error");
    assert!(
        event["message"]
            .as_str()
            .unwrap()
            .contains("unknown option")
    );
    assert!(!event["message"].as_str().unwrap().starts_with("error: "));
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn json_usage_errors_cover_blank_identity_and_invalid_approval_policy() {
    for args in [
        ["--json", "--session-id", " ", "task"].as_slice(),
        ["--json", "--approval-policy", "sometimes", "task"].as_slice(),
    ] {
        let fixture = start(args, false).await;
        assert!(fixture.ctx.get::<HeadlessStartupKey>().is_none());
        assert_eq!(*lock(&fixture.exits), [1]);
        assert!(lock(&fixture.stderr).is_empty());
        let event: Value = serde_json::from_str(lock(&fixture.stdout).trim()).unwrap();
        assert_eq!(event["type"], "error");
        fixture.kernel.shutdown().await;
    }
}

#[tokio::test]
async fn separator_keeps_a_later_json_token_positional() {
    let fixture = start(&["--", "--json"], false).await;
    let startup = fixture.ctx.get::<HeadlessStartupKey>().unwrap();
    assert_eq!(startup.task.as_deref(), Some("--json"));
    assert!(!startup.json);
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn json_used_as_an_option_operand_does_not_change_error_channel() {
    let fixture = start(&["--session-id", "--json"], true).await;
    assert!(lock(&fixture.stdout).is_empty());
    assert!(!lock(&fixture.stderr).is_empty());
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn help_exits_without_publishing_startup() {
    let fixture = start(&["--help"], false).await;
    assert!(fixture.ctx.get::<HeadlessStartupKey>().is_none());
    assert_eq!(*lock(&fixture.exits), [0]);
    assert!(lock(&fixture.stdout).contains("dsh --profile headless"));
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn invalid_approval_policy_keeps_exact_recommended_values() {
    let fixture = start(&["--approval-policy", "sometimes", "task"], false).await;
    let stderr = lock(&fixture.stderr).clone();
    assert!(
        stderr.contains("Allowed choices are ask, never"),
        "{stderr}"
    );
    assert_eq!(*lock(&fixture.exits), [1]);
    fixture.kernel.shutdown().await;
}
