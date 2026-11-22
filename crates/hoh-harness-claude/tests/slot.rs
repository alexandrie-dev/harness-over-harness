//! The access mode reaches Claude only from the slot: [`ExternalHarness::start_with_access`] with
//! the mode the human chose in the window (`hoh/executor/access/set`). The plugin's configuration
//! never decides it, even when it names one. Claude is mounted as a plugin in a kernel with the
//! slot and reached the way the slot and the window reach it, as a `dyn ExternalHarness` from
//! `ExternalHarnesses`, against the CLI stub (`tests/stub/claude.rs`).
//!
//! Only the slot's trait is used here (no function of this crate but the plugin), so the file
//! builds against any version of the crate: against one that took the mode from its
//! configuration, `the_slot_mode_decides_even_when_the_configuration_says_full` fails.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hoh_agent::AgentRegistryPlugin;
use hoh_destructive_classifier::DestructiveClassifierPlugin;
use hoh_external_harness::{
    AccessMode, ApprovalMode, AuthState, ExternalHarness, ExternalHarnessPlugin, ExternalHarnesses,
    HarnessEvent, HarnessPolicy, ItemOutcome, NetworkAccess, PromptParams, SandboxMode,
    StartParams, Started, VersionStatus,
};
use hoh_harness_claude::{HARNESS_NAME, HarnessClaudePlugin};
use hoh_kernel::{Context, Kernel, Plugin};
use hoh_session::{SessionStore, Sessions};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_user_approval::ApprovalService;
use hoh_user_questions::UserQuestionsPlugin;
use serde_json::{Value, json};
use tokio::sync::mpsc::Receiver;

const STUB: &str = env!("CARGO_BIN_EXE_claude-stub");

/// The switches before the permission flags, in every session's command line.
const STREAM: [&str; 7] = [
    "-p",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
    "--include-partial-messages",
];

/// What the slot starts a session with under each mode for the default configuration
/// (`launch_policy` of `hoh-external-harness`): confirm caps it to the folder without network,
/// full opens both.
const CONFIRM_POLICY: HarnessPolicy = HarnessPolicy {
    sandbox: SandboxMode::WorkspaceWrite,
    approval: ApprovalMode::OnRequest,
    network: NetworkAccess::Denied,
};
const FULL_POLICY: HarnessPolicy = HarnessPolicy {
    sandbox: SandboxMode::FullAccess,
    approval: ApprovalMode::OnRequest,
    network: NetworkAccess::Allowed,
};
const READ_ONLY_POLICY: HarnessPolicy = HarnessPolicy {
    sandbox: SandboxMode::ReadOnly,
    approval: ApprovalMode::OnRequest,
    network: NetworkAccess::Denied,
};

/// The tests of this file run one at a time: each writes a launcher script and runs it, and a
/// process another test forks while the script is still open for writing would make that run
/// fail with "text file busy".
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

struct Slot {
    kernel: Kernel,
    dir: tempfile::TempDir,
    harness: Arc<dyn ExternalHarness>,
    log: Arc<Mutex<Vec<String>>>,
    _exporter: hoh_kernel::Disposer,
}

async fn mount<P: Plugin>(ctx: &Context, config: Value) {
    let fiber = ctx.plugin::<P>(config).expect("plugin mounts");
    fiber
        .settled()
        .await
        .unwrap_or_else(|error| panic!("{}: {error}", P::NAME));
}

/// A launcher for the stub: the plugin's configuration names only the program, so the program
/// sets the stub's environment itself.
fn launcher(dir: &Path, version: Option<&str>) -> PathBuf {
    let path = dir.join("claude");
    let mut script = format!(
        "#!/bin/sh\nexport STUB_STATE='{}' STUB_ARGS_FILE='{}'\n",
        dir.display(),
        dir.join("args.txt").display()
    );
    if let Some(version) = version {
        script.push_str(&format!("export STUB_VERSION='{version}'\n"));
    }
    script.push_str(&format!("exec '{STUB}' \"$@\"\n"));
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// A kernel with the slot and Claude mounted with `config` (its `executable` set to the stub).
async fn mount_claude(mut config: Value, version: Option<&str>) -> Slot {
    let dir = tempfile::tempdir().unwrap();
    if config.get("executable").is_none() {
        config["executable"] = json!(launcher(dir.path(), version).to_str().unwrap());
    }
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let (log, exporter) = capture_logs(&ctx);
    ctx.provide::<Sessions>(Arc::new(SessionStore::new()))
        .await
        .expect("sessions");
    mount::<SessionProjectionRegistry>(&ctx, json!({})).await;
    mount::<AgentRegistryPlugin>(&ctx, json!({})).await;
    mount::<ApprovalService>(&ctx, json!({})).await;
    mount::<UserQuestionsPlugin>(&ctx, json!({})).await;
    mount::<DestructiveClassifierPlugin>(&ctx, json!({})).await;
    mount::<ExternalHarnessPlugin>(&ctx, json!({})).await;
    mount::<HarnessClaudePlugin>(&ctx, config).await;
    let harness = ctx
        .get::<ExternalHarnesses>()
        .expect("the slot's service")
        .get(HARNESS_NAME)
        .expect("Claude is registered");
    Slot {
        kernel,
        dir,
        harness,
        log,
        _exporter: exporter,
    }
}

fn capture_logs(ctx: &Context) -> (Arc<Mutex<Vec<String>>>, hoh_kernel::Disposer) {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
    let levels = hoh_kernel::Levels {
        default: Some(hoh_kernel::Level::Debug),
        ..hoh_kernel::Levels::default()
    };
    let exporter = ctx
        .logger()
        .exporter(levels, move |record: &hoh_kernel::Record| {
            sink.lock().unwrap().push(record.message.clone());
        })
        .expect("log exporter");
    (lines, exporter)
}

impl Slot {
    fn params(&self, policy: HarnessPolicy) -> StartParams {
        StartParams {
            session_id: "hoh-1".into(),
            cwd: self.dir.path().to_string_lossy().into_owned(),
            model: None,
            effort: None,
            policy,
            resume: None,
        }
    }

    async fn start(&self, policy: HarnessPolicy, access: AccessMode) -> Started {
        self.harness
            .start_with_access(self.params(policy), access)
            .await
            .map_err(|e| format!("{}: {}", e.code, e.message))
            .unwrap()
    }

    /// The command line the stub got, program excluded.
    fn args(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("args.txt"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    async fn close(self) {
        self.kernel.shutdown().await;
    }
}

fn flag_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let at = args.iter().position(|a| a == name)?;
    args.get(at + 1).map(String::as_str)
}

/// Events up to and including the first one `done` accepts.
async fn until(
    events: &mut Receiver<HarnessEvent>,
    done: impl Fn(&HarnessEvent) -> bool,
) -> Vec<HarnessEvent> {
    let mut seen = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(15), events.recv())
            .await
            .expect("an event in time")
            .expect("the stream stays open");
        let last = done(&event);
        seen.push(event);
        if last {
            return seen;
        }
    }
}

fn run_echo(started: &Started) {
    send(started, "run: echo hi");
}

fn send(started: &Started, text: &str) {
    tokio::spawn(started.session.prompt(PromptParams {
        session_id: "hoh-1".into(),
        turn_id: "t1".into(),
        text: text.into(),
    }));
}

/// (a) The configuration says `full`, the human chose confirm: confirm is what Claude runs, and
/// a command is a question.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn the_slot_mode_decides_even_when_the_configuration_says_full() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let slot = mount_claude(json!({ "access": "full" }), None).await;
    let mut started = slot.start(CONFIRM_POLICY, AccessMode::Confirm).await;
    let args = slot.args();
    assert_eq!(
        flag_value(&args, "--permission-mode"),
        Some("default"),
        "{args:?}"
    );
    assert_eq!(
        flag_value(&args, "--permission-prompt-tool"),
        Some("stdio"),
        "{args:?}"
    );
    assert_eq!(
        flag_value(&args, "--setting-sources"),
        Some("user,project,local"),
        "{args:?}"
    );
    assert!(
        !args
            .iter()
            .any(|a| a == "bypassPermissions" || a.contains("dangerously")),
        "{args:?}"
    );

    run_echo(&started);
    let seen = until(&mut started.events, |e| {
        matches!(
            e,
            HarnessEvent::ApprovalRequested { .. }
                | HarnessEvent::TurnCompleted { .. }
                | HarnessEvent::SessionEnded { .. }
        )
    })
    .await;
    match seen.last() {
        Some(HarnessEvent::ApprovalRequested { request_id, .. }) => assert!(!request_id.is_empty()),
        other => panic!("the command is a question to the human, got {other:?}: {seen:?}"),
    }
    assert!(!seen.iter().any(|e| matches!(
        e,
        HarnessEvent::ItemCompleted {
            outcome: ItemOutcome::Completed,
            ..
        }
    )));
    started.session.terminate(Duration::from_millis(500)).await;
    // The configured mode is not silently dropped: the log says it was ignored.
    assert!(
        slot.log
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains("names an access mode") && line.contains("ignored")),
        "the configured mode is reported as ignored: {:?}",
        slot.log.lock().unwrap()
    );
    slot.close().await;
}

/// (b) The human chose full: full is what Claude runs, whatever the configuration says.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn full_from_the_slot_runs_full_whatever_the_configuration_says() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for config in [json!({}), json!({ "access": "confirm" })] {
        let slot = mount_claude(config.clone(), None).await;
        let mut started = slot.start(FULL_POLICY, AccessMode::Full).await;
        let args = slot.args();
        assert_eq!(
            flag_value(&args, "--permission-mode"),
            Some("bypassPermissions"),
            "{config}: {args:?}"
        );
        assert!(args.contains(&"--allow-dangerously-skip-permissions".to_owned()));
        // The callback stays for what even bypass refuses.
        assert_eq!(flag_value(&args, "--permission-prompt-tool"), Some("stdio"));
        assert!(
            !args
                .iter()
                .any(|a| a == "--tools" || a == "--disallowedTools")
        );

        run_echo(&started);
        let seen = until(&mut started.events, |e| {
            matches!(
                e,
                HarnessEvent::TurnCompleted { .. } | HarnessEvent::SessionEnded { .. }
            )
        })
        .await;
        assert!(
            !seen
                .iter()
                .any(|e| matches!(e, HarnessEvent::ApprovalRequested { .. })),
            "{config}: {seen:?}"
        );
        assert!(seen.iter().any(|e| matches!(
            e,
            HarnessEvent::ItemCompleted { outcome: ItemOutcome::Completed, output: Some(out), .. }
                if out == "ran: echo hi"
        )));
        started.session.terminate(Duration::from_millis(500)).await;
        slot.close().await;
    }
}

/// (c) What the window shows as `effective.flags` is the permission part of the real command
/// line, in both modes, for each policy the slot starts with.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn the_launch_flags_are_the_flags_the_process_gets() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for config in [
        json!({ "access": "full" }),
        json!({ "settingSources": "isolated" }),
    ] {
        let slot = mount_claude(config.clone(), None).await;
        for access in [AccessMode::Confirm, AccessMode::Full] {
            for policy in [CONFIRM_POLICY, FULL_POLICY, READ_ONLY_POLICY] {
                let started = slot.start(policy, access).await;
                let args = slot.args();
                let flags = slot.harness.launch_flags(&policy, access);
                assert!(!flags.is_empty(), "{config} {access:?} {policy:?}");
                let native = flag_value(&args, "--session-id")
                    .expect("a new session")
                    .to_owned();
                let mut expected: Vec<String> = STREAM.map(str::to_owned).to_vec();
                expected.extend(flags.iter().cloned());
                expected.extend(["--session-id".to_owned(), native]);
                assert_eq!(args, expected, "{config} {access:?} {policy:?}");
                started.session.terminate(Duration::from_millis(500)).await;
            }
        }
        slot.close().await;
    }
    let slot = mount_claude(json!({}), None).await;
    let full = |policy| slot.harness.launch_flags(&policy, AccessMode::Full);
    let confirm = |policy| slot.harness.launch_flags(&policy, AccessMode::Confirm);
    let words = |flags: Vec<String>| flags.join(" ");
    assert_eq!(
        words(confirm(CONFIRM_POLICY)),
        "--permission-mode default --setting-sources user,project,local \
         --permission-prompt-tool stdio --disallowedTools WebFetch,WebSearch"
    );
    assert_eq!(
        words(full(FULL_POLICY)),
        "--permission-mode bypassPermissions --setting-sources user,project,local \
         --permission-prompt-tool stdio --allow-dangerously-skip-permissions"
    );
    // Full does not widen a read-only session.
    assert_eq!(
        words(full(READ_ONLY_POLICY)),
        "--permission-mode dontAsk --setting-sources user,project,local --tools \
         Read,Glob,Grep,LS,NotebookRead,TodoWrite"
    );
    slot.close().await;
}

/// (d) Installed, version and where it stands against `version.rs`, sign-in and models.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn discover_with_status_places_the_version_against_the_checked_range() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    let slot = mount_claude(json!({}), None).await;
    let (found, status) = slot.harness.discover_with_status().await.unwrap();
    assert_eq!(status, VersionStatus::Supported);
    assert!(found.installed);
    assert_eq!(found.version.as_deref(), Some("2.0.31"));
    assert_eq!(found.auth, AuthState::Authenticated);
    let ids: Vec<_> = found.models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["default", "sonnet", "opus"]);
    assert_eq!(found, slot.harness.discover().await.unwrap());
    slot.close().await;

    for version in ["3.1.0 (Claude Code)", "1.0.128 (Claude Code)"] {
        let slot = mount_claude(json!({}), Some(version)).await;
        let (found, status) = slot.harness.discover_with_status().await.unwrap();
        assert_eq!(status, VersionStatus::Unsupported, "{version}");
        assert!(found.installed);
        assert_eq!(
            found.version.as_deref(),
            version.split(' ').next(),
            "{version}"
        );
        slot.close().await;
    }

    let slot = mount_claude(json!({}), Some("what is this")).await;
    let (found, status) = slot.harness.discover_with_status().await.unwrap();
    assert_eq!(status, VersionStatus::Unknown);
    assert!(found.installed);
    assert_eq!(found.version, None);
    slot.close().await;

    let missing = tempfile::tempdir().unwrap();
    let program = missing.path().join("no-such-claude");
    let slot = mount_claude(json!({ "executable": program.to_str().unwrap() }), None).await;
    let (found, status) = slot.harness.discover_with_status().await.unwrap();
    assert_eq!(status, VersionStatus::Unknown);
    assert!(!found.installed);
    slot.close().await;
}

/// (1) Confirm from the slot with the network denied: the CLI's two network tools stay off, even
/// when the configuration says full.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn confirm_from_the_slot_keeps_the_network_tools_off_whatever_the_configuration_says() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let slot = mount_claude(json!({ "access": "full" }), None).await;
    let started = slot.start(CONFIRM_POLICY, AccessMode::Confirm).await;
    let args = slot.args();
    assert_eq!(
        flag_value(&args, "--disallowedTools"),
        Some("WebFetch,WebSearch"),
        "{args:?}"
    );
    started.session.terminate(Duration::from_millis(500)).await;
    slot.close().await;
}

/// (2) Confirm from the slot for a read-only session: the read tools only, and what they do
/// outside the folder is still a question (`default` with the callback, not `dontAsk`), even
/// when the configuration says full.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn confirm_from_the_slot_keeps_a_read_only_session_asking_whatever_the_configuration_says() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let slot = mount_claude(json!({ "access": "full" }), None).await;
    let started = slot.start(READ_ONLY_POLICY, AccessMode::Confirm).await;
    let args = slot.args();
    assert_eq!(
        flag_value(&args, "--permission-mode"),
        Some("default"),
        "{args:?}"
    );
    assert_eq!(
        flag_value(&args, "--permission-prompt-tool"),
        Some("stdio"),
        "{args:?}"
    );
    assert_eq!(
        flag_value(&args, "--tools"),
        Some("Read,Glob,Grep,LS,NotebookRead,TodoWrite"),
        "{args:?}"
    );
    assert!(!args.iter().any(|a| a == "dontAsk"), "{args:?}");
    started.session.terminate(Duration::from_millis(500)).await;
    slot.close().await;
}

/// (3) Confirm from the slot: the CLI's removal safety refusal is a question to the human, not
/// the automatic denial of bypass, even when the configuration says full.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn under_confirm_from_the_slot_the_removal_check_is_a_question_whatever_the_configuration_says()
 {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let slot = mount_claude(json!({ "access": "full" }), None).await;
    let mut started = slot.start(CONFIRM_POLICY, AccessMode::Confirm).await;
    send(&started, "dangerous-rm");
    let seen = until(&mut started.events, |e| {
        matches!(
            e,
            HarnessEvent::ApprovalRequested { .. }
                | HarnessEvent::ItemCompleted { .. }
                | HarnessEvent::TurnCompleted { .. }
                | HarnessEvent::SessionEnded { .. }
        )
    })
    .await;
    match seen.last() {
        Some(HarnessEvent::ApprovalRequested { action, .. }) => {
            let shown = format!("{action:?}");
            assert!(shown.contains("rm -rf /data"), "{shown}");
        }
        other => panic!("the removal is a question to the human, got {other:?}: {seen:?}"),
    }
    started.session.terminate(Duration::from_millis(500)).await;
    slot.close().await;
}

/// The window of the slot behind the real SDK server. No host: `hoh/executor/access/get` is not
/// a privileged command.
struct Window {
    server: hoh_sdk_jsonrpc_server::HarnessSdkJsonRpcServer,
    _adapter: hoh_llm::AdapterRegistrationHandle,
}

struct NoModel;

impl hoh_llm::LlmAdapter for NoModel {
    fn stream(
        &self,
        _options: hoh_llm::GenerateOptions,
    ) -> Result<hoh_llm::LlmStream, hoh_error::BoxError> {
        Err("no model in these tests".into())
    }
}

struct Peer;

impl hoh_sdk_jsonrpc_server::transport::JsonRpcTransportPeer for Peer {
    fn request(
        &self,
        _method: &str,
        _params: Value,
        _signal: Option<hoh_timeout::AbortSignal>,
    ) -> futures_util::future::BoxFuture<'static, Result<Value, hoh_error::BoxError>> {
        Box::pin(async { Err("the test window answers nothing".into()) })
    }

    fn notify(&self, _method: &str, _params: Option<Value>) -> Result<(), hoh_error::BoxError> {
        Ok(())
    }
}

/// The window plugin of the slot on the kernel of `slot`, as the engine serves it.
async fn open_window(slot: &Slot) -> Window {
    let ctx = slot.kernel.root();
    ctx.provide::<hoh_loader::ProfileContextKey>(Arc::new(hoh_loader::ProfileContext {
        name: "test".into(),
        dir: slot.dir.path().to_path_buf(),
        home: slot.dir.path().to_path_buf(),
    }))
    .await
    .expect("profile");
    mount::<hoh_llm::LlmPlugin>(&ctx, json!({})).await;
    let adapter = ctx
        .get::<hoh_llm::Llm>()
        .expect("llm")
        .register_adapter(&ctx, vec!["mock".into()], Arc::new(NoModel))
        .expect("adapter");
    ctx.provide::<hoh_sdk_jsonrpc_server::HohRpcMethods>(Arc::new(
        hoh_sdk_jsonrpc_server::HohRpcMethodRegistry::default(),
    ))
    .await
    .expect("method table");
    mount::<hoh_external_harness::ExternalHarnessWindowPlugin>(&ctx, json!({})).await;
    let server = hoh_sdk_jsonrpc_server::HarnessSdkJsonRpcServer::new(
        ctx.clone(),
        Arc::new(Peer),
        hoh_sdk_jsonrpc_server::HarnessSdkJsonRpcServerOptions::default(),
    )
    .expect("server");
    let params = json!({"cwd": slot.dir.path(), "provider": "mock", "model": "mock"});
    server
        .initialize(serde_json::from_value(params).expect("initialize params"))
        .await
        .expect("initialize");
    Window {
        server,
        _adapter: adapter,
    }
}

impl Window {
    async fn access(&self) -> Value {
        self.server
            .handle_request(
                "hoh/executor/access/get",
                json!({ "executor": HARNESS_NAME }),
            )
            .await
            .map_err(|error| error.to_string())
            .expect("access/get")
    }

    async fn close(self) {
        let _ = self.server.shutdown().await;
    }
}

/// The human's settings and the working folder's by default (as T3, HOH-40); none of them when
/// the profile says `isolated`.
const SOURCES: [(&str, &[&str]); 2] = [
    ("{}", &["--setting-sources", "user,project,local"]),
    (
        r#"{"settingSources":"isolated"}"#,
        &["--setting-sources", "", "--strict-mcp-config"],
    ),
];

/// The setting sources are the profile's choice and reach the command line as it says, in both
/// modes; the mode does not change them.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn the_setting_sources_of_the_profile_reach_the_command_line() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for (config, sources) in SOURCES {
        let slot = mount_claude(serde_json::from_str(config).unwrap(), None).await;
        for (policy, access) in [
            (CONFIRM_POLICY, AccessMode::Confirm),
            (FULL_POLICY, AccessMode::Full),
        ] {
            let started = slot.start(policy, access).await;
            let args = slot.args();
            let at = args
                .iter()
                .position(|a| a == "--setting-sources")
                .expect("the sources are always named");
            assert_eq!(args[at..at + sources.len()], *sources, "{config}: {args:?}");
            assert_eq!(
                args.iter().filter(|a| *a == "--setting-sources").count(),
                1,
                "{args:?}"
            );
            assert_eq!(
                args.iter().any(|a| a == "--strict-mcp-config"),
                sources.contains(&"--strict-mcp-config"),
                "{config}: {args:?}"
            );
            started.session.terminate(Duration::from_millis(500)).await;
        }
        slot.close().await;
    }
}

/// The window's `effective.flags` name the setting sources the next session starts with: the
/// flags shown are the slice of the command line that runs.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn the_window_shows_the_setting_sources_the_session_starts_with() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for (config, sources) in SOURCES {
        let slot = mount_claude(serde_json::from_str(config).unwrap(), None).await;
        let window = open_window(&slot).await;
        let shown = window.access().await;
        assert_eq!(shown["effective"]["mode"], "confirm", "{shown}");
        let flags: Vec<String> = serde_json::from_value(shown["effective"]["flags"].clone())
            .expect("the flags are a list of strings");
        let at = flags
            .iter()
            .position(|f| f == "--setting-sources")
            .unwrap_or_else(|| panic!("{config}: {flags:?}"));
        assert_eq!(
            flags[at..at + sources.len()],
            *sources,
            "{config}: {flags:?}"
        );

        // The default configuration of the slot under confirm: the policy the next start uses.
        let started = slot.start(CONFIRM_POLICY, AccessMode::Confirm).await;
        let args = slot.args();
        assert_eq!(
            args[STREAM.len()..STREAM.len() + flags.len()],
            flags[..],
            "{config}: {args:?}"
        );
        started.session.terminate(Duration::from_millis(500)).await;
        window.close().await;
        slot.close().await;
    }
}
