//! The Claude harness end to end against the CLI stub (`tests/stub/claude.rs`): the real `claude`
//! is never run.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hoh_external_harness::{
    AccessMode, ApprovalDecision, ApprovalMode, AuthState, DiscoveryFailure, DiscoveryOperation,
    EndReason, ExternalHarness, HarnessAction, HarnessEvent, HarnessPolicy, HarnessResponse,
    HarnessSession, HarnessTelemetry, InteractionMode, ItemOutcome, PromptParams, QuestionAnswer,
    RespondParams, SandboxMode, StartParams, Started, code,
};
use hoh_harness_claude::{ClaudeConfig, ClaudeHarness, SettingSources};
use tokio::sync::mpsc::Receiver;

const STUB: &str = env!("CARGO_BIN_EXE_claude-stub");

struct Env {
    dir: tempfile::TempDir,
    mode: Option<&'static str>,
    version: Option<&'static str>,
    executable: PathBuf,
    version_timeout: Duration,
    init_timeout: Duration,
    access: AccessMode,
    setting_sources: SettingSources,
    event_queue: usize,
    interrupt_timeout: Duration,
}

impl Env {
    fn new() -> Env {
        Env {
            dir: tempfile::tempdir().unwrap(),
            mode: None,
            version: None,
            executable: PathBuf::from(STUB),
            version_timeout: Duration::from_secs(10),
            init_timeout: Duration::from_secs(10),
            access: AccessMode::Confirm,
            setting_sources: ClaudeConfig::default().setting_sources,
            event_queue: 64,
            interrupt_timeout: Duration::from_secs(10),
        }
    }

    fn args_file(&self) -> PathBuf {
        self.dir.path().join("args.txt")
    }

    fn harness(&self) -> ClaudeHarness {
        let mut env = vec![
            (
                "STUB_STATE".into(),
                self.dir.path().to_string_lossy().into_owned(),
            ),
            (
                "STUB_ARGS_FILE".into(),
                self.args_file().to_string_lossy().into_owned(),
            ),
        ];
        env.push((
            "CLAUDE_CONFIG_DIR".into(),
            "/synthetic/Claude config".into(),
        ));
        env.extend(
            self.mode
                .map(|mode| ("STUB_MODE".to_owned(), mode.to_owned())),
        );
        env.extend(
            self.version
                .map(|v| ("STUB_VERSION".to_owned(), v.to_owned())),
        );
        ClaudeHarness::new(ClaudeConfig {
            executable: self.executable.clone(),
            setting_sources: self.setting_sources,
            extra_env: env,
            version_timeout: self.version_timeout,
            init_timeout: self.init_timeout,
            event_queue: self.event_queue,
            interrupt_timeout: self.interrupt_timeout,
        })
    }

    fn args(&self) -> Vec<String> {
        std::fs::read_to_string(self.args_file())
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

fn params(dir: &Path, resume: Option<&str>, policy: HarnessPolicy) -> StartParams {
    StartParams {
        session_id: "hoh-1".into(),
        cwd: dir.to_string_lossy().into_owned(),
        cwd_identity: None,
        model: None,
        effort: None,
        interaction_mode: hoh_external_harness::InteractionMode::Default,
        policy,
        resume: resume.map(Into::into),
    }
}

async fn start(env: &Env, resume: Option<&str>) -> Started {
    env.harness()
        .start_with_access(
            params(env.dir.path(), resume, HarnessPolicy::default()),
            env.access,
        )
        .await
        .map_err(|e| e.message)
        .unwrap()
}

#[tokio::test]
async fn plan_mode_captures_exit_plan_without_asking_for_permission() {
    let env = Env::new();
    let mut parameters = params(env.dir.path(), None, HarnessPolicy::default());
    parameters.interaction_mode = InteractionMode::Plan;
    let mut started = env
        .harness()
        .start_with_access(parameters, AccessMode::Confirm)
        .await
        .unwrap();
    prompt(&started.session, "turn-plan", "plan");
    let events = until(&mut started.events, |event| {
        matches!(event, HarnessEvent::TurnCompleted { .. })
    })
    .await;
    assert!(events.iter().any(|event| matches!(event,
        HarnessEvent::ItemStarted { action: HarnessAction::Tool { name, input }, .. }
            if name == "ExitPlanMode" && input["plan"] == "# Plan\n\n1. Inspect\n2. Implement"
    )));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, HarnessEvent::ApprovalRequested { .. }))
    );
    let args = env.args();
    let at = args
        .iter()
        .position(|arg| arg == "--permission-mode")
        .unwrap();
    assert_eq!(args[at + 1], "plan");
}

#[tokio::test]
async fn context_usage_is_the_last_root_message_snapshot_not_aggregate_result_usage() {
    let env = Env::new();
    let mut started = start(&env, None).await;
    prompt(&started.session, "turn-usage", "usage");
    let events = until(&mut started.events, |event| {
        matches!(event, HarnessEvent::TurnCompleted { .. })
    })
    .await;
    let usage_events: Vec<&HarnessEvent> = events
        .iter()
        .filter(|event| matches!(event, HarnessEvent::Usage { .. }))
        .collect();
    assert_eq!(usage_events.len(), 5, "{usage_events:?}");
    assert!(matches!(
        usage_events[0],
        HarnessEvent::Usage {
            input_tokens: 100,
            output_tokens: 1,
            max_tokens: None,
            ..
        }
    ));
    assert!(matches!(
        usage_events[3],
        HarnessEvent::Usage {
            input_tokens: 200,
            cache_creation_input_tokens: 40,
            cache_read_input_tokens: 60,
            output_tokens: 9,
            max_tokens: None,
        }
    ));
    assert!(matches!(
        usage_events[4],
        HarnessEvent::Usage {
            input_tokens: 200,
            cache_creation_input_tokens: 40,
            cache_read_input_tokens: 60,
            output_tokens: 9,
            max_tokens: Some(200_000),
        }
    ));
}

#[tokio::test]
async fn title_generation_is_a_short_isolated_no_tools_call_outside_the_project() {
    let env = Env::new();
    let title = env
        .harness()
        .generate_title("title this conversation".into())
        .await
        .unwrap();
    assert_eq!(title, "Context Window Indicator");
    let args = env.args();
    let value = |flag: &str| {
        let at = args.iter().position(|arg| arg == flag).unwrap();
        args[at + 1].as_str()
    };
    assert_eq!(value("--model"), "claude-haiku-5-5");
    assert_eq!(value("--effort"), "low");
    assert_eq!(value("--tools"), "");
    let cwd = std::fs::read_to_string(env.dir.path().join("title-cwd")).unwrap();
    assert_ne!(Path::new(&cwd), env.dir.path());
    assert!(
        !Path::new(&cwd).exists(),
        "the temporary title directory is removed"
    );
}

fn prompt(session: &Arc<dyn HarnessSession>, turn: &str, text: &str) {
    let fut = session.prompt(PromptParams {
        session_id: "hoh-1".into(),
        turn_id: turn.into(),
        text: text.into(),
        ..Default::default()
    });
    tokio::spawn(fut);
}

/// Events up to and including the first one `done` accepts.
async fn until(
    events: &mut Receiver<HarnessEvent>,
    mut done: impl FnMut(&HarnessEvent) -> bool,
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

fn turn_end(event: &HarnessEvent) -> bool {
    matches!(
        event,
        HarnessEvent::TurnCompleted { .. }
            | HarnessEvent::TurnAborted { .. }
            | HarnessEvent::TurnFailed { .. }
            | HarnessEvent::SessionEnded { .. }
    )
}

fn respond(session: &Arc<dyn HarnessSession>, request: &str, response: HarnessResponse) {
    let fut = session.respond(RespondParams {
        session_id: "hoh-1".into(),
        request_id: request.into(),
        response,
    });
    tokio::spawn(fut);
}

#[tokio::test]
async fn discover_reports_install_version_sign_in_and_models() {
    let env = Env::new();
    let found = env.harness().discover().await.unwrap();
    assert!(found.installed);
    assert_eq!(found.version.as_deref(), Some("2.0.31"));
    assert_eq!(found.auth, AuthState::Authenticated);
    assert_eq!(found.detail, None);
    let ids: Vec<_> = found.models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["default", "sonnet", "opus"]);
    assert!(found.models[0].is_default);
    // The probe stores nothing and connects to no MCP server.
    let args = env.args();
    assert!(args.contains(&"--no-session-persistence".to_owned()));
    assert!(args.contains(&"--strict-mcp-config".to_owned()));
}

/// HOH-41: `/` and `$` are what the CLI itself declares for the folder — the commands of its
/// `initialize`, the skills of its `reload_skills` — trimmed and one per name, nothing added; the
/// probe runs in the folder with user settings but no project/local setting files, no hooks, no
/// MCP server, no stored session, and no turn. Commands and skills still come from the CLI.
#[tokio::test]
async fn commands_and_skills_are_what_the_cli_declares_for_the_folder() {
    let env = Env::new();
    let folder = env.dir.path().join("project");
    std::fs::create_dir(&folder).unwrap();
    let harness = env.harness();
    let found = harness.commands(folder.clone()).await.unwrap();
    let commands: Vec<_> = found
        .commands
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                c.description.as_deref(),
                c.argument_hint.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        commands,
        [
            ("review", Some("Review a pull request"), Some("[pr]")),
            ("init", None, None),
            ("release-notes", Some("Write the release notes"), None),
        ]
    );
    let skills: Vec<_> = found.skills.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(skills, ["release-notes"]);
    assert_eq!(found.skills[0].argument_hint, None);
    let args = env.args();
    for switch in [
        "--no-session-persistence",
        "--strict-mcp-config",
        "--setting-sources",
    ] {
        assert!(args.contains(&switch.to_owned()), "{switch}: {args:?}");
    }
    let at = args.iter().position(|arg| arg == "--settings").unwrap();
    assert_eq!(args[at + 1], "{\"disableAllHooks\":true}");
    let at = args
        .iter()
        .position(|arg| arg == "--setting-sources")
        .unwrap();
    assert_eq!(args[at + 1], "user");
    assert!(
        !args
            .iter()
            .any(|arg| arg == "--session-id" || arg == "--resume")
    );
    // No prompt was sent: nothing reached the stub's prompt log.
    assert!(!env.dir.path().join("prompt-sessions").exists());

    // A real personal-profile session is started only for the person's turn. Its project/local
    // sources then add the folder's commands and skills, and its snapshot replaces the safe
    // pre-turn probe without waiting for the window's short cache to expire.
    assert_eq!(harness.commands_revision(&folder), 0);
    assert!(
        !found
            .commands
            .iter()
            .any(|command| command.name == "project-command")
    );
    assert!(
        !found
            .skills
            .iter()
            .any(|skill| skill.name == "project-skill")
    );
    let started = harness
        .start_with_access(
            params(&folder, None, HarnessPolicy::default()),
            AccessMode::Confirm,
        )
        .await
        .unwrap();
    let after = harness.commands(folder.clone()).await.unwrap();
    assert!(harness.commands_revision(&folder) > 0);
    assert!(
        after
            .commands
            .iter()
            .any(|command| command.name == "project-command")
    );
    assert!(
        after
            .skills
            .iter()
            .any(|skill| skill.name == "project-skill")
    );
    started.session.terminate(Duration::from_millis(500)).await;

    // A CLI without `reload_skills` still gives its commands; a missing folder, nothing.
    let mut env = Env::new();
    env.mode = Some("no-skills");
    env.setting_sources = SettingSources::Isolated;
    let found = env.harness().commands(env.dir.path().into()).await.unwrap();
    assert_eq!(found.commands.len(), 3);
    assert!(found.skills.is_empty());
    let args = env.args();
    let at = args
        .iter()
        .position(|arg| arg == "--setting-sources")
        .unwrap();
    assert_eq!(
        args[at + 1],
        "",
        "the isolated probe uses the isolated profile's empty sources"
    );
    let missing = env.harness().commands(env.dir.path().join("gone")).await;
    assert_eq!(missing.unwrap_err().code, hoh_external_harness::code::IO);
}

#[tokio::test]
async fn discover_says_when_the_user_is_signed_out() {
    let mut env = Env::new();
    env.mode = Some("signed-out");
    let found = env.harness().discover().await.unwrap();
    assert!(found.installed);
    assert_eq!(found.auth, AuthState::Unauthenticated);
    assert!(
        found
            .detail
            .unwrap()
            .contains("Вход в Claude Code не выполнен")
    );
}

#[tokio::test]
async fn discover_refuses_a_version_outside_the_range_in_words_for_the_window() {
    let mut env = Env::new();
    env.version = Some("1.0.128 (Claude Code)");
    let found = env.harness().discover().await.unwrap();
    assert!(found.installed);
    assert_eq!(found.version.as_deref(), Some("1.0.128"));
    assert_eq!(found.auth, AuthState::Unknown);
    assert!(found.models.is_empty());
    assert_eq!(
        found.detail.as_deref(),
        Some("Claude Code версии 1.0.128 не поддерживается, нужна 2.0.0–2.x.")
    );
    env.version = Some("3.1.0 (Claude Code)");
    let found = env.harness().discover().await.unwrap();
    assert!(found.detail.unwrap().contains("3.1.0 не поддерживается"));
}

#[tokio::test]
async fn discover_names_an_unreadable_version() {
    let mut env = Env::new();
    env.version = Some("what is this");
    let found = env.harness().discover().await.unwrap();
    assert!(found.installed);
    assert!(
        found
            .detail
            .unwrap()
            .contains("Не удалось определить версию")
    );
}

#[tokio::test]
async fn discover_without_the_program_says_so() {
    let mut env = Env::new();
    env.executable = env.dir.path().join("no-such-claude");
    let found = env.harness().discover().await.unwrap();
    assert!(!found.installed);
    assert!(found.detail.unwrap().contains("Claude Code не найден"));
}

#[tokio::test]
async fn discover_reports_a_protocol_that_is_not_recognized() {
    let mut env = Env::new();
    env.mode = Some("init-error");
    let found = env.harness().discover().await.unwrap();
    assert!(found.installed);
    assert_eq!(found.auth, AuthState::Unknown);
    let detail = found.detail.unwrap();
    assert!(
        detail.contains("2.0.31") && detail.contains("поддерживается 2.0.0–2.x"),
        "{detail}"
    );
}

#[tokio::test]
async fn no_chat_with_an_unsupported_version_or_a_missing_program() {
    let mut env = Env::new();
    env.version = Some("1.0.1");
    let error = env
        .harness()
        .start_or_resume(params(env.dir.path(), None, HarnessPolicy::default()))
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, code::UNSUPPORTED_VERSION);
    assert!(error.message.contains("1.0.1 не поддерживается"));
    // The process never started: no argv was written.
    assert!(!env.args_file().exists());
    env.version = None;
    env.executable = env.dir.path().join("missing");
    let error = env
        .harness()
        .start_or_resume(params(env.dir.path(), None, HarnessPolicy::default()))
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, code::NOT_INSTALLED);
}

#[tokio::test]
async fn a_turn_streams_reasoning_and_text_and_completes() {
    let env = Env::new();
    let Started {
        result,
        session,
        mut events,
    } = start(&env, None).await;
    assert!(!result.resumed);
    prompt(&session, "t1", "hello");
    let seen = until(&mut events, turn_end).await;
    assert!(matches!(&seen[0], HarnessEvent::TurnStarted { turn_id } if turn_id == "t1"));
    let deltas: String = seen
        .iter()
        .filter_map(|e| match e {
            HarnessEvent::TextDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, "Hello");
    let delta_id = seen.iter().find_map(|e| match e {
        HarnessEvent::TextDelta { message_id, .. } => Some(message_id.clone()),
        _ => None,
    });
    let final_id = seen.iter().find_map(|e| match e {
        HarnessEvent::TextFinal {
            message_id, text, ..
        } if text == "Hello" => Some(message_id.clone()),
        _ => None,
    });
    assert!(delta_id.is_some());
    assert_eq!(delta_id, final_id, "the final text replaces its own deltas");
    let thinking = seen.iter().find_map(|e| match e {
        HarnessEvent::ReasoningDelta {
            message_id, text, ..
        } => Some((message_id.clone(), text.clone())),
        _ => None,
    });
    let thinking_final = seen.iter().find_map(|e| match e {
        HarnessEvent::ReasoningFinal { message_id, .. } => Some(message_id.clone()),
        _ => None,
    });
    assert_eq!(thinking.as_ref().map(|t| t.1.as_str()), Some("pondering"));
    assert_eq!(thinking.map(|t| t.0), thinking_final);
    assert!(
        matches!(seen.last(), Some(HarnessEvent::TurnCompleted { turn_id }) if turn_id == "t1")
    );
    // The safe defaults of the launch line.
    let args = env.args();
    for expected in [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "--verbose",
        "--permission-prompt-tool",
        "stdio",
    ] {
        assert!(args.contains(&expected.to_owned()), "{expected}: {args:?}");
    }
    let at = args.iter().position(|a| a == "--permission-mode").unwrap();
    assert_eq!(args[at + 1], "default");
    assert!(!args.iter().any(|a| a.contains("dangerously")));
    let at = args.iter().position(|a| a == "--session-id").unwrap();
    assert_eq!(args[at + 1], String::from(result.native_session_id));
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn telemetry_keeps_raw_usage_safe_mcp_and_background_provenance() {
    let env = Env::new();
    let Started {
        result,
        session,
        mut events,
    } = start(&env, None).await;
    prompt(&session, "t-telemetry", "telemetry");
    let seen = until(&mut events, turn_end).await;
    let records: Vec<_> = seen
        .iter()
        .filter_map(|event| match event {
            HarnessEvent::Telemetry { record } => Some(record),
            _ => None,
        })
        .collect();
    assert_eq!(records.len(), 9, "{seen:#?}");
    for rejected in ["tasks-mixed-telemetry", "tasks-malformed-telemetry"] {
        assert!(
            records
                .iter()
                .all(|record| record.provenance.source_id.as_deref() != Some(rejected)),
            "an incomplete roster must emit no snapshot: {seen:#?}"
        );
    }
    assert!(records.iter().all(|record| {
        !String::from(record.provenance.native_session_id.clone()).is_empty()
            && !record.provenance.launch_id.is_empty()
    }));
    for (index, record) in records.iter().enumerate() {
        let expected_turn = matches!(index, 1 | 2 | 3 | 8).then_some("t-telemetry");
        assert_eq!(
            record.provenance.turn_id.as_deref(),
            expected_turn,
            "system/background telemetry must not be attributed to the active prompt"
        );
    }
    assert!(
        records
            .iter()
            .all(|record| record.provenance.launch_id == records[0].provenance.launch_id)
    );

    match &records[0].payload {
        HarnessTelemetry::McpServers { servers } => {
            assert_eq!(servers.len(), 2);
            assert_eq!(servers[0].name, "codex");
            assert_eq!(servers[0].status.as_deref(), Some("connected"));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        &records[1].payload,
        HarnessTelemetry::AssistantUsage { usage }
            if usage.input_tokens == Some(11)
                && usage.output_tokens == Some(3)
                && usage.cache_read_input_tokens == Some(5)
                && usage.cache_creation_input_tokens.is_none()
    ));
    for record in &records[2..=3] {
        assert!(matches!(
            &record.payload,
            HarnessTelemetry::AssistantUsage { .. }
        ));
        assert_eq!(
            record.provenance.source_id, None,
            "an id-less assistant frame must not get an invented source id"
        );
    }
    assert!(matches!(
        &records[4].payload,
        HarnessTelemetry::BackgroundTask { update }
            if update.subtype == "task_started"
                && update.task.task_id == "task-1"
                && update.task.run_id.as_deref() == Some("run-main")
                && update.is_backgrounded == Some(true)
    ));
    assert!(matches!(
        &records[5].payload,
        HarnessTelemetry::BackgroundTask { update }
            if update.subtype == "task_progress"
                && update.workflow_progress.len() == 2
                && update.workflow_progress[1].agent_id.as_deref() == Some("agent-1")
    ));
    assert!(matches!(
        &records[6].payload,
        HarnessTelemetry::BackgroundTask { update }
            if update.subtype == "task_notification"
                && update.status.as_deref() == Some("completed")
    ));
    assert!(matches!(
        &records[7].payload,
        HarnessTelemetry::BackgroundTasksSnapshot { tasks } if tasks.is_empty()
    ));
    match &records[8].payload {
        HarnessTelemetry::ResultUsage {
            usage,
            model_usage,
            total_cost_usd,
            user_message_uuids,
            ..
        } => {
            assert_eq!(
                usage.as_ref().and_then(|usage| usage.input_tokens),
                Some(13)
            );
            let ledger = model_usage.as_ref().expect("modelUsage field was present");
            assert_eq!(ledger.len(), 1);
            assert_eq!(ledger[0].model, "stub-model");
            assert_eq!(ledger[0].input_tokens, Some(20));
            assert_eq!(ledger[0].context_window, Some(200_000));
            assert_eq!(ledger[0].cost_usd, Some(0.125));
            assert_eq!(*total_cost_usd, Some(0.25));
            assert_eq!(user_message_uuids.len(), 1);
        }
        other => panic!("{other:?}"),
    }

    let encoded = serde_json::to_string(&records).unwrap();
    for secret in [
        "secret-command",
        "secret-token",
        "secret-arg",
        "secret-output",
    ] {
        assert!(
            !encoded.contains(secret),
            "unsafe MCP/task field leaked: {encoded}"
        );
    }
    let first_launch = records[0].provenance.launch_id.clone();
    let native = String::from(result.native_session_id);
    session.terminate(Duration::from_millis(500)).await;

    let Started {
        result,
        session,
        mut events,
    } = start(&env, Some(&native)).await;
    assert!(result.resumed);
    assert_eq!(String::from(result.native_session_id), native);
    prompt(&session, "t-resumed-telemetry", "telemetry");
    let resumed = until(&mut events, turn_end).await;
    let resumed_record = resumed.iter().find_map(|event| match event {
        HarnessEvent::Telemetry { record } => Some(record),
        _ => None,
    });
    let resumed_record = resumed_record.expect("resumed telemetry");
    assert_ne!(resumed_record.provenance.launch_id, first_launch);
    assert_eq!(
        String::from(resumed_record.provenance.native_session_id.clone()),
        native
    );
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn late_parented_assistant_usage_keeps_its_origin_turn() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t-origin", "telemetry-parent");
    let origin = until(&mut events, turn_end).await;
    assert!(origin.iter().any(|event| matches!(
        event,
        HarnessEvent::Telemetry { record }
            if record.provenance.source_id.as_deref() == Some("parent-message")
                && record.provenance.turn_id.as_deref() == Some("t-origin")
    )));

    prompt(&session, "t-next", "telemetry-late-parent");
    let late = until(&mut events, turn_end).await;
    let turn_of = |source: &str| {
        late.iter().find_map(|event| match event {
            HarnessEvent::Telemetry { record }
                if record.provenance.source_id.as_deref() == Some(source) =>
            {
                Some(record.provenance.turn_id.as_deref())
            }
            _ => None,
        })
    };
    assert_eq!(turn_of("late-known"), Some(Some("t-origin")), "{late:#?}");
    assert_eq!(turn_of("late-unknown"), Some(None), "{late:#?}");
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn task_runs_and_typed_updates_remain_distinguishable() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t-tasks", "telemetry-tasks");
    let seen = until(&mut events, turn_end).await;
    let records: Vec<_> = seen
        .iter()
        .filter_map(|event| match event {
            HarnessEvent::Telemetry { record } => Some(record),
            _ => None,
        })
        .collect();
    assert_eq!(records.len(), 6, "{seen:#?}");
    let lifecycle_runs: Vec<_> = records[..3]
        .iter()
        .filter_map(|record| match &record.payload {
            HarnessTelemetry::BackgroundTask { update } => update.task.run_id.as_deref(),
            _ => None,
        })
        .collect();
    assert_eq!(lifecycle_runs, ["run-1", "run-1", "run-2"]);
    assert!(matches!(
        &records[3].payload,
        HarnessTelemetry::BackgroundTaskUpdated { task_id, run_id, patch }
            if task_id == "same-task"
                && run_id.as_deref() == Some("run-1")
                && patch.status.as_deref() == Some("completed")
                && patch.end_time == Some(17)
    ));
    assert!(matches!(
        &records[4].payload,
        HarnessTelemetry::BackgroundTaskUpdated { run_id, patch, .. }
            if run_id.as_deref() == Some("run-2") && patch.background == Some(false)
    ));
    assert!(matches!(
        &records[5].payload,
        HarnessTelemetry::BackgroundTaskUpdated { run_id, patch, .. }
            if run_id.as_deref() == Some("run-2") && patch.background == Some(true)
    ));
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn an_allowed_command_runs_and_a_denied_one_is_declined() {
    for allow in [true, false] {
        let env = Env::new();
        let Started {
            session,
            mut events,
            ..
        } = start(&env, None).await;
        prompt(&session, "t1", "run: echo hi");
        let seen = until(&mut events, |e| {
            matches!(e, HarnessEvent::ApprovalRequested { .. })
        })
        .await;
        let request = match seen.last() {
            Some(HarnessEvent::ApprovalRequested {
                request_id,
                item_id,
                action,
                ..
            }) => {
                assert_eq!(item_id.as_deref(), Some("toolu_1"));
                assert!(
                    matches!(action, HarnessAction::Command { command, .. } if command == "echo hi")
                );
                request_id.clone()
            }
            other => panic!("{other:?}"),
        };
        assert!(
            seen.iter().any(
                |e| matches!(e, HarnessEvent::ItemStarted { item_id, .. } if item_id == "toolu_1")
            ),
            "the step starts before it is asked about"
        );
        let decision = if allow {
            ApprovalDecision::Allow
        } else {
            ApprovalDecision::Deny
        };
        respond(&session, &request, HarnessResponse::Approval { decision });
        let rest = until(&mut events, turn_end).await;
        let completed = rest.iter().find_map(|e| match e {
            HarnessEvent::ItemCompleted {
                outcome, output, ..
            } => Some((*outcome, output.clone())),
            _ => None,
        });
        if allow {
            assert_eq!(
                completed,
                Some((ItemOutcome::Completed, Some("ran: echo hi".into())))
            );
        } else {
            let (outcome, output) = completed.unwrap();
            assert_eq!(outcome, ItemOutcome::Declined);
            assert_eq!(output.as_deref(), Some("User declined tool execution."));
        }
        assert!(matches!(
            rest.last(),
            Some(HarnessEvent::TurnCompleted { .. })
        ));
        session.terminate(Duration::from_millis(500)).await;
    }
}

#[tokio::test]
async fn a_file_edit_is_asked_about_as_a_file_change() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "edit: src/lib.rs");
    let seen = until(&mut events, |e| {
        matches!(e, HarnessEvent::ApprovalRequested { .. })
    })
    .await;
    match seen.last() {
        Some(HarnessEvent::ApprovalRequested {
            action: HarnessAction::FileChange { changes },
            ..
        }) => {
            assert_eq!(changes[0].path, "src/lib.rs");
        }
        other => panic!("{other:?}"),
    }
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn a_question_reaches_the_user_and_the_answer_goes_back() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "question");
    let seen = until(&mut events, |e| {
        matches!(e, HarnessEvent::QuestionAsked { .. })
    })
    .await;
    let (request, question) = match seen.last() {
        Some(HarnessEvent::QuestionAsked {
            request_id,
            questions,
            ..
        }) => (request_id.clone(), questions[0].clone()),
        other => panic!("{other:?}"),
    };
    assert_eq!(question.question, "Which?");
    assert_eq!(question.header.as_deref(), Some("Pick"));
    assert_eq!(question.options.len(), 2);
    respond(
        &session,
        &request,
        HarnessResponse::Answer {
            answers: vec![QuestionAnswer {
                id: question.id,
                selected: vec!["B".into()],
                custom: None,
            }],
        },
    );
    let rest = until(&mut events, turn_end).await;
    let text = rest.iter().find_map(|e| match e {
        HarnessEvent::TextFinal { text, .. } => Some(text.clone()),
        _ => None,
    });
    assert_eq!(text.as_deref(), Some("answers={\"Which?\":\"B\"}"));
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn interrupt_during_a_command_aborts_the_turn_and_closes_the_step() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "long");
    until(&mut events, |e| {
        matches!(e, HarnessEvent::ItemStarted { .. })
    })
    .await;
    session.interrupt().await.unwrap();
    let rest = until(&mut events, turn_end).await;
    assert!(rest.iter().any(|e| matches!(
        e,
        HarnessEvent::ItemCompleted {
            outcome: ItemOutcome::Interrupted,
            ..
        }
    )));
    assert!(matches!(rest.last(), Some(HarnessEvent::TurnAborted { turn_id }) if turn_id == "t1"));
    session.terminate(Duration::from_millis(500)).await;
}

/// Живой пилот 22.11 (i12b): a result of a turn the CLI ran on its own — the notification of a
/// background command lost with the old process, a wake — does not end the prompt's turn; the
/// turn ends with the prompt's own answer. Before, the first result closed the turn at once and
/// the answer came between turns.
#[tokio::test]
async fn a_result_of_another_turn_does_not_end_the_prompts_turn() {
    for scenario in ["wake-first", "wake-turn-first"] {
        let env = Env::new();
        let Started {
            session,
            mut events,
            ..
        } = start(&env, None).await;
        prompt(&session, "t1", scenario);
        let seen = until(&mut events, turn_end).await;
        let ends: Vec<&HarnessEvent> = seen.iter().filter(|event| turn_end(event)).collect();
        assert!(
            matches!(ends.as_slice(), [HarnessEvent::TurnCompleted { turn_id }] if turn_id == "t1"),
            "{scenario}: {seen:?}"
        );
        let answer = seen
            .iter()
            .any(|event| matches!(event, HarnessEvent::TextFinal { text, .. } if text == "Hello"));
        assert!(
            answer,
            "{scenario}: the turn ended before its answer: {seen:?}"
        );
        let background_usage = seen.iter().find_map(|event| match event {
            HarnessEvent::Telemetry { record }
                if matches!(
                    &record.payload,
                    HarnessTelemetry::ResultUsage { origin, .. }
                        if origin.as_deref() == Some("task-notification")
                ) =>
            {
                Some(record)
            }
            _ => None,
        });
        assert_eq!(
            background_usage.and_then(|record| record.provenance.turn_id.as_deref()),
            None,
            "{scenario}: a background result must not be attributed to t1"
        );
        // The next prompt is a turn of its own, answered and ended once.
        prompt(&session, "t2", "hello");
        let next = until(&mut events, turn_end).await;
        assert!(
            matches!(next.last(), Some(HarnessEvent::TurnCompleted { turn_id }) if turn_id == "t2"),
            "{scenario}: {next:?}"
        );
        session.terminate(Duration::from_secs(1)).await;
    }
}

#[tokio::test]
async fn a_failed_result_fails_the_turn() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "failure");
    let seen = until(&mut events, turn_end).await;
    match seen.last() {
        Some(HarnessEvent::TurnFailed { error, .. }) => assert_eq!(error.message, "boom"),
        other => panic!("{other:?}"),
    }
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn a_session_resumes_by_its_native_id_and_a_missing_one_is_reported() {
    let env = Env::new();
    let first = start(&env, None).await;
    let native = String::from(first.result.native_session_id.clone());
    first.session.terminate(Duration::from_millis(500)).await;

    let second = start(&env, Some(&native)).await;
    assert!(second.result.resumed);
    assert_eq!(
        String::from(second.result.native_session_id.clone()),
        native
    );
    let args = env.args();
    let at = args.iter().position(|a| a == "--resume").unwrap();
    assert_eq!(args[at + 1], native);
    assert!(!args.contains(&"--session-id".to_owned()));
    second.session.terminate(Duration::from_millis(500)).await;

    let error = env
        .harness()
        .start_or_resume(params(
            env.dir.path(),
            Some("00000000-0000-4000-8000-00000000105e"),
            HarnessPolicy::default(),
        ))
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, code::SESSION_NOT_FOUND);
}

#[tokio::test]
async fn garbage_on_stdout_ends_the_session_as_a_protocol_error() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "garbage");
    let seen = until(&mut events, turn_end).await;
    assert!(matches!(
        seen.last(),
        Some(HarnessEvent::SessionEnded {
            reason: EndReason::ProtocolError,
            ..
        })
    ));
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn garbage_before_initialize_fails_the_start() {
    let mut env = Env::new();
    env.mode = Some("garbage-start");
    let error = env
        .harness()
        .start_or_resume(params(env.dir.path(), None, HarnessPolicy::default()))
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, code::PROTOCOL);
    assert!(error.message.contains("поддерживается 2.0.0–2.x"));
    assert!(
        error.message.contains("stdout line is not a JSON object"),
        "the cause is named: {}",
        error.message
    );
}

#[tokio::test]
async fn a_cli_that_never_answers_initialize_fails_the_start() {
    let mut env = Env::new();
    env.mode = Some("silent");
    env.init_timeout = Duration::from_millis(300);
    let error = env
        .harness()
        .start_or_resume(params(env.dir.path(), None, HarnessPolicy::default()))
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, code::TIMEOUT);
    assert!(error.message.contains("нет ответа на initialize"));
}

#[tokio::test]
async fn discovery_names_the_bounded_initialize_timeout() {
    let mut env = Env::new();
    env.mode = Some("silent");
    env.init_timeout = Duration::from_millis(50);
    let found = env.harness().discover().await.unwrap();
    assert_eq!(
        found.failure,
        Some(DiscoveryFailure::Timeout {
            operation: DiscoveryOperation::Initialize,
            timeout_seconds: 1,
        })
    );
}

#[tokio::test]
async fn a_cli_that_dies_at_start_reports_its_stderr() {
    let mut env = Env::new();
    env.mode = Some("crash-start");
    let error = env
        .harness()
        .start_or_resume(params(env.dir.path(), None, HarnessPolicy::default()))
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, code::PROTOCOL);
    assert!(error.message.contains("boot failure"), "{}", error.message);
}

#[tokio::test]
async fn a_crash_in_a_turn_ends_the_session_with_the_exit_code() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "crash");
    let seen = until(&mut events, turn_end).await;
    match seen.last() {
        Some(HarnessEvent::SessionEnded {
            reason,
            exit_code,
            message,
        }) => {
            assert_eq!(*reason, EndReason::Crashed);
            assert_eq!(*exit_code, Some(3));
            assert!(
                message
                    .as_deref()
                    .unwrap_or_default()
                    .contains("stub crashed")
            );
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn stderr_is_kept_for_diagnostics() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "stderr");
    until(&mut events, turn_end).await;
    for _ in 0..50 {
        if session.diagnostics().contains("something odd") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        session
            .diagnostics()
            .contains("stub warning: something odd")
    );
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn approval_never_is_refused_and_no_mode_gets_a_way_around_the_questions() {
    let env = Env::new();
    for sandbox in [SandboxMode::WorkspaceWrite, SandboxMode::ReadOnly] {
        let error = env
            .harness()
            .start_or_resume(params(
                env.dir.path(),
                None,
                HarnessPolicy {
                    sandbox,
                    approval: ApprovalMode::Never,
                    ..HarnessPolicy::default()
                },
            ))
            .await
            .err()
            .expect("approval never is off");
        assert_eq!(error.code, code::ESCALATION_DENIED);
        assert!(
            error.message.contains("пока недоступен"),
            "{}",
            error.message
        );
    }
    assert!(!env.args_file().exists(), "the CLI was not even started");

    // Default: `default` mode and the callback over stdio.
    let started = start_with(&env, None, HarnessPolicy::default()).await;
    let args = env.args();
    let at = args.iter().position(|a| a == "--permission-mode").unwrap();
    assert_eq!(args[at + 1], "default");
    let at = args
        .iter()
        .position(|a| a == "--permission-prompt-tool")
        .unwrap();
    assert_eq!(args[at + 1], "stdio");
    assert!(
        !args
            .iter()
            .any(|a| a.contains("bypass") || a.contains("dangerously"))
    );
    // By default the human's settings and the working folder's, as T3 (HOH-40).
    let at = args.iter().position(|a| a == "--setting-sources").unwrap();
    assert_eq!(args[at + 1], "user,project,local");
    started.session.terminate(Duration::from_millis(500)).await;

    // Read-only: `default` (a read outside the folder is a question), only the read tools, and no
    // `--allowedTools` that would answer "yes" to any read.
    let started = start_with(
        &env,
        None,
        HarnessPolicy {
            sandbox: SandboxMode::ReadOnly,
            ..HarnessPolicy::default()
        },
    )
    .await;
    let args = env.args();
    let at = args.iter().position(|a| a == "--permission-mode").unwrap();
    assert_eq!(args[at + 1], "default");
    assert!(!args.contains(&"--allowedTools".to_owned()));
    assert!(args.contains(&"--disallowedTools".to_owned()));
    // The tools of writing are not merely unapproved: the CLI is not given them at all.
    let at = args.iter().position(|a| a == "--tools").expect("--tools");
    assert_eq!(args[at + 1], "Read,Glob,Grep,LS,NotebookRead,TodoWrite");
    assert!(
        !args
            .iter()
            .any(|a| a.contains("bypass") || a.contains("dangerously") || a == "dontAsk")
    );
    started.session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn a_missing_working_folder_is_not_reported_as_a_missing_program() {
    let env = Env::new();
    let gone = env.dir.path().join("deleted-folder");
    let error = env
        .harness()
        .start_or_resume(params(&gone, None, HarnessPolicy::default()))
        .await
        .err()
        .unwrap();
    assert_ne!(error.code, code::NOT_INSTALLED);
    assert!(
        !error.message.contains("Claude Code не найден"),
        "{}",
        error.message
    );
    assert!(error.message.contains("Рабочая папка"), "{}", error.message);
}

#[tokio::test]
async fn the_reason_of_a_request_is_the_decision_reason_not_the_title() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "run-reason");
    let seen = until(&mut events, |e| {
        matches!(e, HarnessEvent::ApprovalRequested { .. })
    })
    .await;
    match seen.last() {
        Some(HarnessEvent::ApprovalRequested { reason, .. }) => {
            assert_eq!(reason.as_deref(), Some("Not in allowlist"));
        }
        other => panic!("{other:?}"),
    }
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn a_withdrawn_request_is_resolved() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "withdraw");
    let seen = until(&mut events, turn_end).await;
    let asked = seen
        .iter()
        .position(|e| matches!(e, HarnessEvent::ApprovalRequested { .. }));
    let resolved = seen
        .iter()
        .position(|e| matches!(e, HarnessEvent::RequestResolved { .. }));
    assert!(asked.is_some() && resolved > asked, "{seen:?}");
    // The request is gone: answering it now is refused.
    let request = match &seen[asked.unwrap()] {
        HarnessEvent::ApprovalRequested { request_id, .. } => request_id.clone(),
        _ => unreachable!(),
    };
    let late = session
        .respond(RespondParams {
            session_id: "hoh-1".into(),
            request_id: request,
            response: HarnessResponse::Approval {
                decision: ApprovalDecision::Allow,
            },
        })
        .await;
    assert_eq!(late.unwrap_err().code, code::PROTOCOL);
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn a_cancelled_permission_request_denies_with_interrupt() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "run: echo hi");
    let seen = until(&mut events, |e| {
        matches!(e, HarnessEvent::ApprovalRequested { .. })
    })
    .await;
    let HarnessEvent::ApprovalRequested { request_id, .. } = seen.last().unwrap() else {
        panic!()
    };
    respond(&session, request_id, HarnessResponse::Cancelled);
    let rest = until(&mut events, turn_end).await;
    let output = rest.iter().find_map(|e| match e {
        HarnessEvent::ItemCompleted {
            outcome: ItemOutcome::Declined,
            output,
            ..
        } => output.clone(),
        _ => None,
    });
    assert_eq!(
        output.as_deref(),
        Some("User cancelled tool execution. [interrupt]")
    );
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn a_cancelled_question_is_a_plain_deny() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "question");
    let seen = until(&mut events, |e| {
        matches!(e, HarnessEvent::QuestionAsked { .. })
    })
    .await;
    let HarnessEvent::QuestionAsked { request_id, .. } = seen.last().unwrap() else {
        panic!()
    };
    respond(&session, request_id, HarnessResponse::Cancelled);
    let rest = until(&mut events, turn_end).await;
    assert!(matches!(
        rest.last(),
        Some(HarnessEvent::TurnCompleted { .. })
    ));
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn an_answer_of_the_wrong_kind_is_refused_and_the_request_stays_open() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "run: echo hi");
    let seen = until(&mut events, |e| {
        matches!(e, HarnessEvent::ApprovalRequested { .. })
    })
    .await;
    let HarnessEvent::ApprovalRequested { request_id, .. } = seen.last().unwrap() else {
        panic!()
    };
    let wrong = session
        .respond(RespondParams {
            session_id: "hoh-1".into(),
            request_id: request_id.clone(),
            response: HarnessResponse::Answer { answers: vec![] },
        })
        .await;
    assert_eq!(wrong.unwrap_err().code, code::PROTOCOL);
    let unknown = session
        .respond(RespondParams {
            session_id: "hoh-1".into(),
            request_id: "no-such-request".into(),
            response: HarnessResponse::Cancelled,
        })
        .await;
    assert_eq!(unknown.unwrap_err().code, code::PROTOCOL);
    respond(
        &session,
        request_id,
        HarnessResponse::Approval {
            decision: ApprovalDecision::Allow,
        },
    );
    let rest = until(&mut events, turn_end).await;
    assert!(matches!(
        rest.last(),
        Some(HarnessEvent::TurnCompleted { .. })
    ));
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn a_second_prompt_during_a_turn_is_refused() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "run: echo hi");
    until(&mut events, |e| {
        matches!(e, HarnessEvent::ApprovalRequested { .. })
    })
    .await;
    let second = session
        .prompt(PromptParams {
            session_id: "hoh-1".into(),
            turn_id: "t2".into(),
            text: "hello".into(),
            ..Default::default()
        })
        .await;
    assert_eq!(second.unwrap_err().code, code::HARNESS);
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn a_failed_tool_result_fails_the_step() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "toolfail");
    let seen = until(&mut events, turn_end).await;
    assert!(seen.iter().any(|e| matches!(
        e,
        HarnessEvent::ItemCompleted { outcome: ItemOutcome::Failed, output: Some(o), .. } if o == "exit 1"
    )));
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn text_of_a_subagent_is_a_parented_answer_not_root_text() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "subagent");
    let seen = until(&mut events, turn_end).await;
    let finals: Vec<_> = seen
        .iter()
        .filter_map(|e| match e {
            HarnessEvent::TextFinal {
                text,
                parent_item_id,
                ..
            } => Some((text.as_str(), parent_item_id.as_deref())),
            _ => None,
        })
        .collect();
    assert_eq!(finals, [("hidden", Some("toolu_p")), ("shown", None)]);
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn the_session_id_the_cli_reports_is_used_for_later_prompts() {
    let mut env = Env::new();
    env.mode = Some("rename");
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "hello");
    let seen = until(&mut events, turn_end).await;
    assert!(seen.iter().any(|e| matches!(
        e,
        HarnessEvent::NativeSession { native_session_id } if native_session_id.as_ref() == "renamed-session"
    )));
    prompt(&session, "t2", "hello");
    until(&mut events, turn_end).await;
    let log = std::fs::read_to_string(env.dir.path().join("prompt-sessions")).unwrap();
    let ids: Vec<_> = log.lines().collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[1], "renamed-session");
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn a_process_killed_by_a_signal_names_the_signal() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "kill-self");
    let seen = until(&mut events, turn_end).await;
    match seen.last() {
        Some(HarnessEvent::SessionEnded {
            reason,
            message,
            exit_code,
        }) => {
            assert_eq!(*reason, EndReason::Crashed);
            assert_eq!(*exit_code, None);
            assert_eq!(
                message.as_deref(),
                Some("Claude Code process terminated by signal SIGABRT")
            );
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_step_and_approval_of_a_subagent_keep_their_parent() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "subagent-tool");
    let seen = until(&mut events, |e| {
        matches!(e, HarnessEvent::ApprovalRequested { .. })
    })
    .await;
    let request = match seen.last() {
        Some(HarnessEvent::ApprovalRequested {
            request_id,
            item_id,
            parent_item_id,
            ..
        }) => {
            assert_eq!(item_id.as_deref(), Some("toolu_sub"));
            assert_eq!(parent_item_id.as_deref(), Some("toolu_p"));
            request_id.clone()
        }
        other => panic!("{other:?}"),
    };
    respond(
        &session,
        &request,
        HarnessResponse::Approval {
            decision: ApprovalDecision::Allow,
        },
    );
    let rest = until(&mut events, turn_end).await;
    let steps: Vec<_> = seen
        .iter()
        .chain(rest.iter())
        .filter_map(|event| match event {
            HarnessEvent::ItemStarted {
                item_id,
                parent_item_id,
                ..
            }
            | HarnessEvent::ItemCompleted {
                item_id,
                parent_item_id,
                ..
            } => Some((item_id.as_str(), parent_item_id.as_deref())),
            _ => None,
        })
        .collect();
    assert_eq!(
        steps,
        [
            ("toolu_sub", Some("toolu_p")),
            ("toolu_sub", Some("toolu_p"))
        ],
        "{seen:?} {rest:?}"
    );
    assert!(matches!(
        rest.last(),
        Some(HarnessEvent::TurnCompleted { .. })
    ));
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn ending_the_session_releases_the_open_requests() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "run: echo hi");
    let seen = until(&mut events, |e| {
        matches!(e, HarnessEvent::ApprovalRequested { .. })
    })
    .await;
    let request = match seen.last() {
        Some(HarnessEvent::ApprovalRequested { request_id, .. }) => request_id.clone(),
        other => panic!("{other:?}"),
    };
    session.terminate(Duration::from_millis(500)).await;
    let rest = until(&mut events, |e| {
        matches!(e, HarnessEvent::RequestResolved { .. })
    })
    .await;
    assert!(matches!(
        rest.last(),
        Some(HarnessEvent::RequestResolved { request_id }) if *request_id == request
    ));
}

#[tokio::test]
async fn a_process_that_dies_releases_the_open_requests_before_the_end() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "ask-abort");
    let seen = until(&mut events, turn_end).await;
    let resolved = seen
        .iter()
        .position(|e| matches!(e, HarnessEvent::RequestResolved { .. }));
    assert!(resolved.is_some(), "{seen:?}");
    assert!(matches!(
        seen.last(),
        Some(HarnessEvent::SessionEnded { .. })
    ));
}

#[tokio::test]
async fn the_answer_to_a_request_carries_an_object_and_the_tool_use_id() {
    for (decision, behavior, class) in [
        (ApprovalDecision::Allow, "allow", "user_temporary"),
        (ApprovalDecision::Deny, "deny", "user_reject"),
    ] {
        let env = Env::new();
        let Started {
            session,
            mut events,
            ..
        } = start(&env, None).await;
        prompt(&session, "t1", "echo-decision");
        let seen = until(&mut events, |e| {
            matches!(e, HarnessEvent::ApprovalRequested { .. })
        })
        .await;
        let request = match seen.last() {
            Some(HarnessEvent::ApprovalRequested { request_id, .. }) => request_id.clone(),
            other => panic!("{other:?}"),
        };
        respond(&session, &request, HarnessResponse::Approval { decision });
        let rest = until(&mut events, turn_end).await;
        let shown = rest
            .iter()
            .find_map(|e| match e {
                HarnessEvent::TextFinal { text, .. } => Some(text.clone()),
                _ => None,
            })
            .unwrap();
        let answer: serde_json::Value = serde_json::from_str(&shown).unwrap();
        assert_eq!(answer["behavior"], behavior);
        assert_eq!(answer["toolUseID"], "toolu_e");
        assert_eq!(answer["decisionClassification"], class);
        if behavior == "allow" {
            assert_eq!(answer["updatedInput"], serde_json::json!({}));
        }
        session.terminate(Duration::from_millis(500)).await;
    }
}

#[tokio::test]
async fn an_interrupted_step_keeps_the_output_of_its_own_result() {
    // The output is the tool's own `[Request interrupted by user]`: the step was closed by its
    // `tool_result` (with `tool_result_meta`), not by the end of the turn.
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "long");
    until(&mut events, |e| {
        matches!(e, HarnessEvent::ItemStarted { .. })
    })
    .await;
    session.interrupt().await.unwrap();
    let rest = until(&mut events, turn_end).await;
    let closed: Vec<_> = rest
        .iter()
        .filter_map(|e| match e {
            HarnessEvent::ItemCompleted {
                outcome, output, ..
            } => Some((*outcome, output.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        closed,
        [(
            ItemOutcome::Interrupted,
            Some("[Request interrupted by user]".to_owned())
        )]
    );
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn a_written_file_is_an_add_when_it_is_new_and_an_update_when_it_exists() {
    use hoh_external_harness::FileChangeKind;
    for (file, existing, kind) in [
        ("fresh.txt", false, FileChangeKind::Add),
        ("old.txt", true, FileChangeKind::Update),
    ] {
        let env = Env::new();
        if existing {
            std::fs::write(env.dir.path().join(file), "x").unwrap();
        }
        let Started {
            session,
            mut events,
            ..
        } = start(&env, None).await;
        prompt(&session, "t1", &format!("write: {file}"));
        let seen = until(&mut events, |e| {
            matches!(e, HarnessEvent::ApprovalRequested { .. })
        })
        .await;
        match seen.last() {
            Some(HarnessEvent::ApprovalRequested {
                action: HarnessAction::FileChange { changes },
                ..
            }) => assert_eq!(changes[0].kind, kind, "{file}"),
            other => panic!("{other:?}"),
        }
        session.terminate(Duration::from_millis(500)).await;
    }
}

#[test]
fn the_debug_of_the_config_hides_the_values_of_the_environment() {
    let config = ClaudeConfig {
        extra_env: vec![("ANTHROPIC_API_KEY".into(), "sk-secret-value".into())],
        ..ClaudeConfig::default()
    };
    let shown = format!("{config:?}");
    assert!(shown.contains("ANTHROPIC_API_KEY"), "{shown}");
    assert!(!shown.contains("sk-secret-value"), "{shown}");
}

async fn start_with(env: &Env, model: Option<&str>, policy: HarnessPolicy) -> Started {
    let mut p = params(env.dir.path(), None, policy);
    p.model = model.map(Into::into);
    env.harness()
        .start_with_access(p, env.access)
        .await
        .map_err(|e| e.message)
        .unwrap()
}

#[tokio::test]
async fn the_chosen_model_reaches_the_command_line() {
    let env = Env::new();
    let started = start_with(&env, Some("sonnet"), HarnessPolicy::default()).await;
    let args = env.args();
    let at = args.iter().position(|a| a == "--model").unwrap();
    assert_eq!(args[at + 1], "sonnet");
    started.session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn without_network_the_two_network_tools_are_disallowed() {
    use hoh_external_harness::NetworkAccess;
    let env = Env::new();
    let started = start_with(
        &env,
        None,
        HarnessPolicy {
            network: NetworkAccess::Denied,
            ..HarnessPolicy::default()
        },
    )
    .await;
    let args = env.args();
    let at = args.iter().position(|a| a == "--disallowedTools").unwrap();
    assert_eq!(args[at + 1], "WebFetch,WebSearch");
    started.session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn an_unknown_control_request_is_answered_with_an_error() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "unknown-request");
    let seen = until(&mut events, turn_end).await;
    let shown = seen
        .iter()
        .find_map(|e| match e {
            HarnessEvent::TextFinal { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap();
    let answer: serde_json::Value = serde_json::from_str(&shown).unwrap();
    assert_eq!(answer["subtype"], "error");
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn a_withdrawn_question_is_resolved() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "withdraw-question");
    let seen = until(&mut events, turn_end).await;
    let asked = seen
        .iter()
        .position(|e| matches!(e, HarnessEvent::QuestionAsked { .. }));
    let resolved = seen
        .iter()
        .position(|e| matches!(e, HarnessEvent::RequestResolved { .. }));
    assert!(asked.is_some() && resolved > asked, "{seen:?}");
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn a_request_left_open_at_the_end_of_the_turn_is_resolved() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "leave-open");
    let seen = until(&mut events, turn_end).await;
    let resolved = seen
        .iter()
        .position(|e| matches!(e, HarnessEvent::RequestResolved { .. }));
    assert!(resolved.is_some(), "{seen:?}");
    assert!(matches!(
        seen.last(),
        Some(HarnessEvent::TurnCompleted { .. })
    ));
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn a_stream_delta_of_a_subagent_keeps_the_parent() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "subagent-stream");
    let seen = until(&mut events, turn_end).await;
    assert!(seen.iter().any(|e| matches!(
        e,
        HarnessEvent::TextDelta { text, parent_item_id: Some(parent), .. }
            if text == "hidden-delta" && parent == "toolu_p"
    )));
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn two_parallel_subagents_keep_parent_order_text_steps_results_and_outcomes() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "parallel-subagents");
    let seen = until(&mut events, turn_end).await;
    let routed: Vec<String> = seen
        .iter()
        .filter_map(|event| match event {
            HarnessEvent::TextFinal {
                text,
                parent_item_id: Some(parent),
                ..
            } => Some(format!("{parent}:text:{text}")),
            HarnessEvent::ItemStarted {
                item_id,
                parent_item_id: Some(parent),
                ..
            } => Some(format!("{parent}:start:{item_id}")),
            HarnessEvent::ItemCompleted {
                item_id,
                output,
                parent_item_id: Some(parent),
                ..
            } => Some(format!(
                "{parent}:result:{item_id}:{}",
                output.as_deref().unwrap_or_default()
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        routed,
        [
            "toolu_alpha:text:alpha note",
            "toolu_alpha:start:toolu_alpha_read",
            "toolu_beta:text:beta note",
            "toolu_beta:start:toolu_beta_grep",
            "toolu_beta:result:toolu_beta_grep:beta match",
            "toolu_beta:text:beta final",
            "toolu_alpha:result:toolu_alpha_read:alpha contents",
            "toolu_alpha:text:alpha final",
        ],
        "{seen:#?}"
    );
    let parent_outcomes: Vec<_> = seen
        .iter()
        .filter_map(|event| match event {
            HarnessEvent::ItemCompleted {
                item_id,
                outcome,
                parent_item_id: None,
                ..
            } if matches!(item_id.as_str(), "toolu_alpha" | "toolu_beta") => {
                Some((item_id.as_str(), *outcome))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        parent_outcomes,
        [
            ("toolu_beta", ItemOutcome::Completed),
            ("toolu_alpha", ItemOutcome::Completed)
        ]
    );
    assert!(matches!(
        seen.last(),
        Some(HarnessEvent::TurnCompleted { .. })
    ));
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn stdout_cut_in_the_middle_of_a_line_is_a_crash_even_with_exit_zero() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "truncate");
    let seen = until(&mut events, turn_end).await;
    match seen.last() {
        Some(HarnessEvent::SessionEnded { reason, .. }) => {
            assert_eq!(*reason, EndReason::Crashed);
        }
        other => panic!("{other:?}"),
    }
}

/// What the slot would be given for a command: the action of the permission request.
async fn asked_command(env: &Env, text: &str) -> (String, Option<String>) {
    let Started {
        session,
        mut events,
        ..
    } = env
        .harness()
        .start_or_resume(params(env.dir.path(), None, HarnessPolicy::default()))
        .await
        .map_err(|e| e.message)
        .unwrap();
    prompt(&session, "t1", text);
    let seen = until(&mut events, |e| {
        matches!(e, HarnessEvent::ApprovalRequested { .. })
    })
    .await;
    session.terminate(Duration::from_millis(500)).await;
    match seen.last() {
        Some(HarnessEvent::ApprovalRequested {
            action: HarnessAction::Command { command, cwd },
            ..
        }) => (command.clone(), cwd.clone()),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_command_reaches_the_slot_as_a_question_and_its_rule_sees_the_danger() {
    use hoh_destructive_classifier::{DestructiveClassifierService, RealFs};
    let classifier = DestructiveClassifierService::new(Arc::new(RealFs));
    let env = Env::new();
    let work = env.dir.path().to_path_buf();

    // The request reaches the slot (no `bypassPermissions`): the classifier of the slot finds
    // `rm -rf ~/x` dangerous and `ls` in the folder harmless.
    let (command, cwd) = asked_command(&env, "run: rm -rf ~/x").await;
    assert_eq!(cwd.as_deref(), Some(work.to_str().unwrap()));
    assert!(
        classifier
            .command(&command, cwd.map(PathBuf::from))
            .await
            .is_some()
    );
    let (command, cwd) = asked_command(&env, "run: ls").await;
    assert_eq!(cwd.as_deref(), Some(work.to_str().unwrap()));
    assert!(
        classifier
            .command(&command, cwd.map(PathBuf::from))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn a_command_is_shown_with_the_folder_the_session_runs_in() {
    // Package (a): the slot asks about every command, and the window and the classifier read
    // the true folder, whatever quotes, pipes or `cd` the command has.
    let env = Env::new();
    let work = env.dir.path().to_str().unwrap().to_owned();
    for command in [
        "touch new.txt",
        "echo \"a\"",
        "ls | wc",
        "cd /tmp && rm x",
        "c''d /tmp && rm x",
    ] {
        let (_, cwd) = asked_command(&env, &format!("run: {command}")).await;
        assert_eq!(cwd.as_deref(), Some(work.as_str()), "{command}");
    }
}

#[tokio::test]
async fn the_setting_sources_are_the_same_for_every_policy() {
    for (setting_sources, value, strict) in [
        (SettingSources::Personal, "user,project,local", false),
        (SettingSources::Isolated, "", true),
    ] {
        let mut env = Env::new();
        env.setting_sources = setting_sources;
        for policy in [
            HarnessPolicy::default(),
            HarnessPolicy {
                sandbox: SandboxMode::ReadOnly,
                ..HarnessPolicy::default()
            },
        ] {
            let started = start_with(&env, None, policy).await;
            let args = env.args();
            let at = args.iter().position(|a| a == "--setting-sources").unwrap();
            assert_eq!(args[at + 1], value, "{setting_sources:?} {policy:?}");
            assert_eq!(
                args.iter().filter(|a| *a == "--setting-sources").count(),
                1,
                "{args:?}"
            );
            assert_eq!(args.contains(&"--strict-mcp-config".to_owned()), strict);
            started.session.terminate(Duration::from_millis(500)).await;
        }
    }
}

#[tokio::test]
async fn read_only_asks_about_a_read_outside_the_folder() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start_with(
        &env,
        None,
        HarnessPolicy {
            sandbox: SandboxMode::ReadOnly,
            ..HarnessPolicy::default()
        },
    )
    .await;
    prompt(&session, "t1", "read: /home/u/.ssh/id_rsa");
    let seen = until(&mut events, |e| {
        matches!(e, HarnessEvent::ApprovalRequested { .. })
    })
    .await;
    assert!(matches!(
        seen.last(),
        Some(HarnessEvent::ApprovalRequested {
            action: HarnessAction::Tool { name, .. },
            ..
        }) if name == "Read"
    ));
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn an_answer_to_a_closed_stdin_is_an_error_not_a_loss() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "close-stdin");
    let seen = until(&mut events, |e| {
        matches!(e, HarnessEvent::ApprovalRequested { .. })
    })
    .await;
    let request = match seen.last() {
        Some(HarnessEvent::ApprovalRequested { request_id, .. }) => request_id.clone(),
        other => panic!("{other:?}"),
    };
    // The first write into a dead pipe only finds out that it is dead (the queue is
    // asynchronous): the interrupt takes it. After that the stdin is known to be closed.
    tokio::time::sleep(Duration::from_millis(300)).await;
    session.interrupt().await.ok();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let error = session
        .respond(RespondParams {
            session_id: "hoh-1".into(),
            request_id: request.clone(),
            response: HarnessResponse::Approval {
                decision: ApprovalDecision::Allow,
            },
        })
        .await
        .expect_err("the answer cannot reach the CLI");
    assert_eq!(error.code, code::CLOSED);
    // The request is not answered: it stays open (a second try meets the same closed stdin, not
    // "no open request") and is released when the session ends.
    let again = session
        .respond(RespondParams {
            session_id: "hoh-1".into(),
            request_id: request.clone(),
            response: HarnessResponse::Approval {
                decision: ApprovalDecision::Deny,
            },
        })
        .await
        .expect_err("still no way to the CLI");
    assert_eq!(again.code, code::CLOSED);
    session.terminate(Duration::from_millis(500)).await;
    let rest = until(&mut events, |e| {
        matches!(e, HarnessEvent::RequestResolved { .. })
    })
    .await;
    assert!(matches!(
        rest.last(),
        Some(HarnessEvent::RequestResolved { request_id }) if *request_id == request
    ));
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn the_answer_to_a_question_has_the_body_of_t3() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "question-body");
    let seen = until(&mut events, |e| {
        matches!(e, HarnessEvent::QuestionAsked { .. })
    })
    .await;
    let (request, id) = match seen.last() {
        Some(HarnessEvent::QuestionAsked {
            request_id,
            questions,
            ..
        }) => (request_id.clone(), questions[0].id.clone()),
        other => panic!("{other:?}"),
    };
    respond(
        &session,
        &request,
        HarnessResponse::Answer {
            answers: vec![QuestionAnswer {
                id,
                selected: vec!["B".into()],
                custom: None,
            }],
        },
    );
    let rest = until(&mut events, turn_end).await;
    let shown = rest
        .iter()
        .find_map(|e| match e {
            HarnessEvent::TextFinal { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&shown).unwrap();
    // T3 `ClaudeAdapterV2.ts:12438-12445`: allow, the questions as asked, the answers by text,
    // and the tool use id.
    assert_eq!(
        body,
        serde_json::json!({
            "behavior": "allow",
            "updatedInput": {
                "questions": [{
                    "question": "Which?", "header": "Pick",
                    "options": [{ "label": "A" }, { "label": "B" }], "multiSelect": false,
                }],
                "answers": { "Which?": "B" },
            },
            "toolUseID": "toolu_qb",
        })
    );
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn a_version_program_that_fails_is_not_a_version() {
    let mut env = Env::new();
    env.mode = Some("version-fails");
    let found = env.harness().discover().await.unwrap();
    assert!(found.installed);
    assert_eq!(found.version, None);
    assert!(
        found
            .detail
            .unwrap()
            .contains("Не удалось определить версию")
    );
    assert_eq!(
        found.failure,
        Some(DiscoveryFailure::Exit {
            operation: DiscoveryOperation::Version,
            exit_code: Some(1),
        })
    );
}

#[tokio::test]
async fn a_slow_version_probe_reports_its_configured_timeout() {
    use std::os::unix::fs::PermissionsExt;

    let mut env = Env::new();
    let executable = env.dir.path().join("slow-claude");
    std::fs::write(&executable, "#!/bin/sh\nsleep 2\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    env.executable = executable;
    env.version_timeout = Duration::from_millis(50);
    let found = env.harness().discover().await.unwrap();
    assert_eq!(
        found.failure,
        Some(DiscoveryFailure::Timeout {
            operation: DiscoveryOperation::Version,
            timeout_seconds: 1,
        })
    );
}

#[tokio::test]
async fn terminate_does_not_wait_for_a_reader_that_does_not_read() {
    let mut env = Env::new();
    env.event_queue = 1;
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "two-open");
    let mut asked = 0;
    until(&mut events, |e| {
        asked += usize::from(matches!(e, HarnessEvent::ApprovalRequested { .. }));
        asked == 2
    })
    .await;
    // The two open requests are released into a queue of one that nobody reads.
    tokio::time::timeout(
        Duration::from_secs(3),
        session.terminate(Duration::from_millis(500)),
    )
    .await
    .expect("terminate is not held back by the events");
}

#[tokio::test]
async fn an_interrupt_the_cli_never_follows_is_bounded() {
    let mut env = Env::new();
    env.interrupt_timeout = Duration::from_millis(300);
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "silent-interrupt");
    until(&mut events, |e| {
        matches!(e, HarnessEvent::ItemStarted { .. })
    })
    .await;
    session.interrupt().await.unwrap();
    let seen = until(&mut events, |e| {
        matches!(e, HarnessEvent::SessionEnded { .. })
    })
    .await;
    assert!(seen.iter().any(|e| matches!(
        e,
        HarnessEvent::ItemCompleted {
            outcome: ItemOutcome::Interrupted,
            ..
        }
    )));
    let aborted = seen
        .iter()
        .filter(|e| matches!(e, HarnessEvent::TurnAborted { turn_id } if turn_id == "t1"))
        .count();
    assert_eq!(aborted, 1, "{seen:?}");
    assert!(matches!(
        seen.last(),
        Some(HarnessEvent::SessionEnded {
            reason: EndReason::Terminated,
            ..
        })
    ));
}

#[tokio::test]
async fn questions_are_projected_as_in_t3() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "question-odd");
    let seen = until(&mut events, |e| {
        matches!(e, HarnessEvent::QuestionAsked { .. })
    })
    .await;
    let (request, questions) = match seen.last() {
        Some(HarnessEvent::QuestionAsked {
            request_id,
            questions,
            ..
        }) => (request_id.clone(), questions.clone()),
        other => panic!("{other:?}"),
    };
    // The empty question is dropped; the second keeps its place in the header; the option
    // shows its label as the description; the empty option is dropped.
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0].question, "Q");
    assert_eq!(questions[0].header.as_deref(), Some("Question 2"));
    assert_eq!(questions[0].options.len(), 1);
    assert_eq!(questions[0].options[0].label, "B");
    assert_eq!(questions[0].options[0].description.as_deref(), Some("B"));
    respond(
        &session,
        &request,
        HarnessResponse::Answer {
            answers: vec![QuestionAnswer {
                id: questions[0].id.clone(),
                selected: vec!["B".into()],
                custom: None,
            }],
        },
    );
    let rest = until(&mut events, turn_end).await;
    let shown = rest
        .iter()
        .find_map(|e| match e {
            HarnessEvent::TextFinal { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&shown).unwrap();
    assert_eq!(
        body["updatedInput"]["answers"],
        serde_json::json!({ "Q": "B" })
    );
    assert_eq!(body["toolUseID"], "toolu_qo");
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn an_input_without_questions_has_no_questions_key_in_the_answer() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "question-bare");
    let seen = until(&mut events, |e| {
        matches!(e, HarnessEvent::QuestionAsked { .. })
    })
    .await;
    let request = match seen.last() {
        Some(HarnessEvent::QuestionAsked { request_id, .. }) => request_id.clone(),
        other => panic!("{other:?}"),
    };
    respond(
        &session,
        &request,
        HarnessResponse::Answer { answers: vec![] },
    );
    let rest = until(&mut events, turn_end).await;
    let shown = rest
        .iter()
        .find_map(|e| match e {
            HarnessEvent::TextFinal { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&shown).unwrap();
    assert_eq!(body["updatedInput"], serde_json::json!({ "answers": {} }));
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn the_argv_the_function_returns_is_the_one_the_process_gets() {
    for access in [
        AccessMode::Confirm,
        AccessMode::AcceptEdits,
        AccessMode::Auto,
        AccessMode::Full,
    ] {
        let mut env = Env::new();
        env.access = access;
        let policy = HarnessPolicy::default();
        let started = env
            .harness()
            .start_with_access(params(env.dir.path(), None, policy), env.access)
            .await
            .map_err(|e| e.message)
            .unwrap();
        let args = env.args();
        let at = args.iter().position(|a| a == "--session-id").unwrap();
        let native = args[at + 1].clone();
        let argv = env
            .harness()
            .argv(&policy, access, None, None, None, &native);
        assert_eq!(argv[0], STUB);
        assert_eq!(argv[1..], args[..], "{access:?}");
        let has = |flag: &str| args.iter().any(|a| a == flag);
        match access {
            AccessMode::Confirm
            | AccessMode::AutoAcceptEdits
            | AccessMode::AcceptEdits
            | AccessMode::Auto => {
                // CEO 23.11 00:07: the two modes in between are Claude's own, with confirm's
                // question channel.
                assert!(has("--permission-prompt-tool"));
                assert!(!has("--allow-dangerously-skip-permissions"));
                let at = args.iter().position(|a| a == "--permission-mode").unwrap();
                let mode = match access {
                    AccessMode::AutoAcceptEdits | AccessMode::AcceptEdits => "acceptEdits",
                    AccessMode::Auto => "auto",
                    _ => "default",
                };
                assert_eq!(args[at + 1], mode);
            }
            AccessMode::Full => {
                // The callback stays for what even bypass refuses.
                assert!(has("--permission-prompt-tool"));
                assert!(has("--allow-dangerously-skip-permissions"));
                let at = args.iter().position(|a| a == "--permission-mode").unwrap();
                assert_eq!(args[at + 1], "bypassPermissions");
            }
        }
        started.session.terminate(Duration::from_millis(500)).await;
    }
}

#[tokio::test]
async fn in_full_access_nothing_is_asked() {
    let mut env = Env::new();
    env.access = AccessMode::Full;
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "run: echo hi");
    let seen = until(&mut events, turn_end).await;
    assert!(!seen.iter().any(|e| matches!(
        e,
        HarnessEvent::ApprovalRequested { .. } | HarnessEvent::QuestionAsked { .. }
    )));
    assert!(seen.iter().any(|e| matches!(
        e,
        HarnessEvent::ItemCompleted { outcome: ItemOutcome::Completed, output: Some(out), .. } if out == "ran: echo hi"
    )));
    assert!(matches!(
        seen.last(),
        Some(HarnessEvent::TurnCompleted { .. })
    ));
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn approval_never_is_refused_in_confirm_and_changes_nothing_in_full() {
    let never = HarnessPolicy {
        approval: ApprovalMode::Never,
        ..HarnessPolicy::default()
    };
    let env = Env::new();
    let error = env
        .harness()
        .start_or_resume(params(env.dir.path(), None, never))
        .await
        .err()
        .expect("never stays off in confirm");
    assert_eq!(error.code, code::ESCALATION_DENIED);
    assert_eq!(
        error.message,
        "Этот режим пока недоступен для Claude: действия идут через подтверждение"
    );

    let mut env = Env::new();
    env.access = AccessMode::Full;
    let started = env
        .harness()
        .start_with_access(params(env.dir.path(), None, never), env.access)
        .await
        .map_err(|e| e.message)
        .expect("in full nothing is asked, so never changes nothing");
    let args = env.args();
    let at = args.iter().position(|a| a == "--permission-mode").unwrap();
    assert_eq!(args[at + 1], "bypassPermissions");
    started.session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn full_does_not_widen_a_read_only_session_and_does_not_narrow_by_network() {
    use hoh_external_harness::NetworkAccess;
    let mut env = Env::new();
    env.access = AccessMode::Full;
    let flag = |args: &[String], name: &str| args.iter().position(|a| a == name);

    // Full + read-only + on-request: the read tools only, no questions, no bypass.
    let started = start_with(
        &env,
        None,
        HarnessPolicy {
            sandbox: SandboxMode::ReadOnly,
            approval: ApprovalMode::OnRequest,
            ..HarnessPolicy::default()
        },
    )
    .await;
    let args = env.args();
    let at = flag(&args, "--permission-mode").unwrap();
    assert_eq!(args[at + 1], "dontAsk");
    let at = flag(&args, "--tools").expect("the read tools stay");
    assert_eq!(args[at + 1], "Read,Glob,Grep,LS,NotebookRead,TodoWrite");
    assert!(flag(&args, "--permission-prompt-tool").is_none());
    assert!(flag(&args, "--allowedTools").is_none());
    assert!(
        !args
            .iter()
            .any(|a| a.contains("dangerously") || a == "bypassPermissions")
    );
    started.session.terminate(Duration::from_millis(500)).await;

    // Full + network denied: the web tools are allowed too (the human's choice).
    let started = start_with(
        &env,
        None,
        HarnessPolicy {
            network: NetworkAccess::Denied,
            ..HarnessPolicy::default()
        },
    )
    .await;
    let args = env.args();
    assert!(flag(&args, "--disallowedTools").is_none());
    let at = flag(&args, "--permission-mode").unwrap();
    assert_eq!(args[at + 1], "bypassPermissions");
    started.session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn full_access_resumes_a_session_with_the_same_flags() {
    let mut env = Env::new();
    env.access = AccessMode::Full;
    let first = start(&env, None).await;
    let native: String = first.result.native_session_id.clone().into();
    first.session.terminate(Duration::from_millis(500)).await;
    let again = start(&env, Some(&native)).await;
    let args = env.args();
    let at = args.iter().position(|a| a == "--resume").unwrap();
    assert_eq!(args[at + 1], native);
    assert!(!args.iter().any(|a| a == "--session-id"));
    let at = args.iter().position(|a| a == "--permission-mode").unwrap();
    assert_eq!(args[at + 1], "bypassPermissions");
    assert!(args.contains(&"--allow-dangerously-skip-permissions".to_owned()));
    again.session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn the_refusal_of_bypass_as_root_is_named() {
    let mut env = Env::new();
    env.access = AccessMode::Full;
    env.mode = Some("root-refusal");
    let error = env
        .harness()
        .start_with_access(
            params(env.dir.path(), None, HarnessPolicy::default()),
            env.access,
        )
        .await
        .err()
        .unwrap();
    assert!(
        error
            .message
            .contains("Claude Code exited before it started: --dangerously-skip-permissions cannot be used with root/sudo"),
        "{}",
        error.message
    );
    assert!(error.message.contains("обычным пользователем"));
}

#[tokio::test]
async fn a_secret_in_the_stderr_of_a_failed_start_is_not_shown() {
    let mut env = Env::new();
    env.mode = Some("secret-start");
    let error = env
        .harness()
        .start_or_resume(params(env.dir.path(), None, HarnessPolicy::default()))
        .await
        .err()
        .unwrap();
    assert!(error.message.contains("boot failure"), "{}", error.message);
    assert!(!error.message.contains("sk-ant-api03"), "{}", error.message);
}

#[tokio::test]
async fn in_full_access_a_question_is_not_asked() {
    // Without the callback the CLI sends no `can_use_tool` (the stub answers by itself): no
    // `QuestionAsked` reaches the window, and the turn goes on.
    let mut env = Env::new();
    env.access = AccessMode::Full;
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "question");
    let seen = until(&mut events, turn_end).await;
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, HarnessEvent::QuestionAsked { .. }))
    );
    assert!(matches!(
        seen.last(),
        Some(HarnessEvent::TurnCompleted { .. })
    ));
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn an_expired_login_is_named_instead_of_the_api_error() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "login-expired");
    let seen = until(&mut events, turn_end).await;
    match seen.last() {
        Some(HarnessEvent::TurnFailed { error, .. }) => {
            assert!(
                error.message.contains("run `claude auth login`"),
                "{}",
                error.message
            );
            assert!(error.message.contains("/synthetic/Claude config"));
            assert!(
                error
                    .message
                    .contains(&format!("{:?}", env.dir.path().to_str().unwrap()))
            );
            assert!(!error.message.contains("Credentials expired"));
        }
        other => panic!("{other:?}"),
    }
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn a_403_shows_the_text_of_the_cli_not_the_login_hint() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "forbidden");
    let seen = until(&mut events, turn_end).await;
    match seen.last() {
        Some(HarnessEvent::TurnFailed { error, .. }) => {
            assert_eq!(
                error.message,
                "API Error: 403 Your organization has no access to this model."
            );
        }
        other => panic!("{other:?}"),
    }
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn a_notebook_edit_names_the_notebook() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "notebook: analysis.ipynb");
    let seen = until(&mut events, |e| {
        matches!(e, HarnessEvent::ApprovalRequested { .. })
    })
    .await;
    match seen.last() {
        Some(HarnessEvent::ApprovalRequested {
            action: HarnessAction::FileChange { changes },
            ..
        }) => assert_eq!(changes[0].path, "analysis.ipynb"),
        other => panic!("{other:?}"),
    }
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn the_stderr_of_a_dying_process_is_masked_in_the_end_and_in_the_diagnostics() {
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "secret-crash");
    let seen = until(&mut events, turn_end).await;
    match seen.last() {
        Some(HarnessEvent::SessionEnded { message, .. }) => {
            let message = message.as_deref().unwrap_or_default();
            assert!(message.contains("fatal"), "{message}");
            assert!(!message.contains("sk-ant"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert!(!session.diagnostics().contains("sk-ant"));
}

#[tokio::test]
async fn in_full_access_what_bypass_refuses_still_reaches_the_human() {
    let mut env = Env::new();
    env.access = AccessMode::Full;
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "protected: .git/config");
    let seen = until(&mut events, |e| {
        matches!(e, HarnessEvent::ApprovalRequested { .. })
    })
    .await;
    let request = match seen.last() {
        Some(HarnessEvent::ApprovalRequested {
            request_id,
            action: HarnessAction::FileChange { changes },
            reason,
            ..
        }) => {
            assert_eq!(changes[0].path, ".git/config");
            assert_eq!(
                reason.as_deref(),
                Some("Path is protected: writes there need approval")
            );
            request_id.clone()
        }
        other => panic!("{other:?}"),
    };
    respond(
        &session,
        &request,
        HarnessResponse::Approval {
            decision: ApprovalDecision::Allow,
        },
    );
    let rest = until(&mut events, turn_end).await;
    assert!(rest.iter().any(|e| matches!(
        e,
        HarnessEvent::ItemCompleted {
            outcome: ItemOutcome::Completed,
            ..
        }
    )));
    session.terminate(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn under_bypass_the_removal_check_of_the_cli_is_not_overridden_by_a_question() {
    // Full access: T3 denies, in the model's words, and does not ask the human.
    let mut env = Env::new();
    env.access = AccessMode::Full;
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "dangerous-rm");
    let seen = until(&mut events, turn_end).await;
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, HarnessEvent::ApprovalRequested { .. }))
    );
    let output = seen.iter().find_map(|e| match e {
        HarnessEvent::ItemCompleted { output, .. } => output.clone(),
        _ => None,
    });
    let output = output.expect("the step is closed with the denial");
    assert!(
        output.starts_with(
            "Claude Code's removal safety check denied this command; the user did not reject it."
        ),
        "{output}"
    );
    assert!(
        output.ends_with("Warning: Dangerous rm operation detected"),
        "{output}"
    );
    session.terminate(Duration::from_millis(500)).await;

    // Confirm: the same request is an ordinary question.
    let env = Env::new();
    let Started {
        session,
        mut events,
        ..
    } = start(&env, None).await;
    prompt(&session, "t1", "dangerous-rm");
    let seen = until(&mut events, |e| {
        matches!(e, HarnessEvent::ApprovalRequested { .. })
    })
    .await;
    assert!(matches!(
        seen.last(),
        Some(HarnessEvent::ApprovalRequested { .. })
    ));
    session.terminate(Duration::from_millis(500)).await;
}

fn started_count(seen: &[HarnessEvent]) -> usize {
    seen.iter()
        .filter(|e| matches!(e, HarnessEvent::TurnStarted { .. }))
        .count()
}

/// The first event of a turn whose first line from the CLI is a permission request.
async fn first_event_of_an_asking_turn(
    session: &Arc<dyn HarnessSession>,
    events: &mut Receiver<HarnessEvent>,
    turn: &str,
) -> (Vec<HarnessEvent>, String) {
    prompt(session, turn, "ask-first");
    let seen = until(events, |e| {
        matches!(e, HarnessEvent::ApprovalRequested { .. })
    })
    .await;
    let request = match seen.last() {
        Some(HarnessEvent::ApprovalRequested { request_id, .. }) => request_id.clone(),
        other => panic!("{other:?}"),
    };
    (seen, request)
}

#[tokio::test]
async fn turn_started_comes_before_the_first_request_of_the_turn_in_both_modes_and_after_resume() {
    for access in [AccessMode::Confirm, AccessMode::Full] {
        let mut env = Env::new();
        env.access = access;
        let first = start(&env, None).await;
        let native: String = first.result.native_session_id.clone().into();
        let (session, mut events) = (first.session.clone(), first.events);
        let (seen, request) = first_event_of_an_asking_turn(&session, &mut events, "t1").await;
        assert!(
            matches!(seen.first(), Some(HarnessEvent::TurnStarted { turn_id }) if turn_id == "t1"),
            "{access:?}: {seen:?}"
        );
        assert_eq!(started_count(&seen), 1, "{seen:?}");
        respond(
            &session,
            &request,
            HarnessResponse::Approval {
                decision: ApprovalDecision::Allow,
            },
        );
        until(&mut events, turn_end).await;
        session.terminate(Duration::from_millis(500)).await;

        // A later turn of a resumed session: the same order.
        let again = start(&env, Some(&native)).await;
        let (session, mut events) = (again.session.clone(), again.events);
        let (seen, _) = first_event_of_an_asking_turn(&session, &mut events, "t2").await;
        assert!(
            matches!(seen.first(), Some(HarnessEvent::TurnStarted { turn_id }) if turn_id == "t2"),
            "{access:?} resumed: {seen:?}"
        );
        assert_eq!(started_count(&seen), 1, "{seen:?}");
        session.terminate(Duration::from_millis(500)).await;
    }
}
