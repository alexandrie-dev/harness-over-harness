use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hoh_external_harness::{
    AccessMode, ApprovalDecision, AuthState, ExternalHarness, HarnessEvent, HarnessPolicy,
    HarnessResponse, InteractionMode, PromptImage, PromptParams, QuestionAnswer, RespondParams,
    StartParams, TitleRequest, TitleWorkspace, VersionStatus,
};
use hoh_harness_codex::{CodexConfig, CodexHarness};
use serde_json::Value;
use tempfile::TempDir;

fn stub() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_codex-stub"))
}

fn harness(temp: &TempDir, scenario: &str) -> (CodexHarness, PathBuf) {
    let log = temp.path().join(format!("{scenario}.jsonl"));
    let release = log.with_extension("release");
    let mut config = CodexConfig {
        executable: stub(),
        request_timeout: Duration::from_secs(3),
        extra_env: vec![
            ("CODEX_STUB_SCENARIO".into(), scenario.into()),
            ("CODEX_STUB_LOG".into(), log.to_string_lossy().into_owned()),
            (
                "CODEX_STUB_RELEASE".into(),
                release.to_string_lossy().into_owned(),
            ),
        ],
        ..CodexConfig::default()
    };
    if scenario == "title-credential-override" {
        config
            .extra_env
            .push(("CODEX_API_KEY".into(), "stub-override".into()));
    }
    (CodexHarness::new(config), log)
}

#[tokio::test]
async fn codex_title_refuses_exec_only_credential_overrides() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "title-credential-override");
    let started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    assert_eq!(
        started
            .session
            .generate_title(title_request(&temp))
            .await
            .unwrap_err()
            .code,
        hoh_external_harness::code::POLICY_UNAVAILABLE,
    );
    assert!(
        !lines(&log)
            .iter()
            .any(|entry| entry["stubEvent"] == "title-exec")
    );
}

#[tokio::test]
async fn account_limits_read_real_windows_without_starting_a_thread() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "account-limits");

    let windows = harness.rate_limits().await.unwrap().unwrap();
    assert_eq!(windows.len(), 3);
    assert_eq!(windows[0].remaining_percent, 84);
    assert_eq!(windows[0].window_minutes, Some(300));
    assert_eq!(windows[0].resets_at, Some(1795672800));
    assert_eq!(windows[1].remaining_percent, 43);
    assert_eq!(windows[1].window_minutes, Some(10080));
    assert_eq!(windows[2].name.as_deref(), Some("Code review"));
    assert_eq!(windows[2].remaining_percent, 0);
    assert_eq!(windows[2].resets_at, None);

    let requests = lines(&log);
    let methods: Vec<_> = requests
        .iter()
        .filter_map(|entry| entry["method"].as_str())
        .collect();
    assert_eq!(
        methods,
        ["initialize", "initialized", "account/rateLimits/read"]
    );
}

#[tokio::test]
async fn workspace_skills_are_listed_without_starting_a_thread() {
    let temp = TempDir::new().unwrap();
    let cwd = temp.path().canonicalize().unwrap();
    let (harness, log) = harness(&temp, "workspace-skills");

    let catalog = harness.commands(cwd.clone()).await.unwrap();
    assert!(catalog.commands.is_empty());
    assert_eq!(catalog.skills.len(), 1);
    assert_eq!(catalog.skills[0].name, "review");
    assert_eq!(
        catalog.skills[0].description.as_deref(),
        Some("Review changes")
    );
    let requests = lines(&log);
    let methods: Vec<_> = requests
        .iter()
        .filter_map(|entry| entry["method"].as_str())
        .collect();
    assert_eq!(methods, ["initialize", "initialized", "skills/list"]);
    assert_eq!(
        requests.last().unwrap()["params"]["cwds"][0],
        cwd.to_str().unwrap()
    );
    assert!(requests[0]["args"].as_array().unwrap().iter().any(|arg| {
        arg.as_str().is_some_and(|arg| {
            arg.starts_with("projects={")
                && arg.contains(cwd.to_str().unwrap())
                && arg.contains("trust_level=\"untrusted\"")
        })
    }));
}

fn start_params(temp: &TempDir, resume: Option<String>) -> StartParams {
    StartParams {
        session_id: "hoh-session".into(),
        cwd: temp.path().to_string_lossy().into_owned(),
        cwd_identity: None,
        model: Some("gpt-stub".into()),
        effort: Some("high".into()),
        interaction_mode: InteractionMode::Default,
        policy: HarnessPolicy::default(),
        resume: resume.map(Into::into),
    }
}

fn title_request(temp: &TempDir) -> TitleRequest {
    use std::os::unix::fs::MetadataExt;

    let cwd = temp.path().canonicalize().unwrap();
    let metadata = cwd.metadata().unwrap();
    TitleRequest {
        prompt: "Generate a short title for: У CEO не открывается кабинет\n".into(),
        model: Some("gpt-stub".into()),
        workspace: Some(TitleWorkspace::new(
            cwd,
            [metadata.dev(), metadata.ino()],
            temp.path().to_owned(),
        )),
    }
}

#[tokio::test]
async fn codex_title_uses_the_verified_thread_route_and_isolated_json_output() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "title-default");
    let started = harness
        .start_or_resume(start_params(&temp, None))
        .await
        .unwrap();
    let request = title_request(&temp);
    let prompt = request.prompt.clone();
    assert_eq!(
        started.session.generate_title(request).await.unwrap(),
        "Доступ к кабинету CEO"
    );
    let requests = lines(&log);
    let methods: Vec<_> = requests
        .iter()
        .filter_map(|entry| entry["method"].as_str())
        .collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "initialized",
            "config/read",
            "configRequirements/read",
            "thread/start",
            "initialize",
            "initialized",
            "config/read",
            "configRequirements/read",
            "account/read",
            "account/read"
        ]
    );
    let start = requests
        .iter()
        .find(|entry| entry["method"] == "thread/start")
        .unwrap();
    assert_eq!(start["params"]["config"]["openai_base_url"], "");
    assert_eq!(
        start["params"]["config"]["cli_auth_credentials_store"],
        "keyring"
    );
    let exec = requests
        .iter()
        .find(|entry| entry["stubEvent"] == "title-exec")
        .unwrap();
    assert_eq!(exec["prompt"], prompt);
    let cwd = Path::new(exec["cwd"].as_str().unwrap());
    assert_ne!(cwd, temp.path());
    assert!(!cwd.exists());
    let args: Vec<_> = exec["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect();
    for required in [
        "--ephemeral",
        "--ignore-user-config",
        "--output-schema",
        "--model=gpt-stub",
        "cli_auth_credentials_store=\"keyring\"",
        "openai_base_url=\"\"",
        "chatgpt_base_url=\"https://chatgpt.com/backend-api/\"",
        "shell_tool",
        "hooks",
        "read-only",
    ] {
        assert!(args.contains(&required), "missing {required}: {args:?}");
    }
    assert!(!args.iter().any(|arg| arg.contains(prompt.as_str())));
}

#[tokio::test]
async fn codex_title_requires_a_matching_workspace_identity() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "title-default");
    let started = harness
        .start_or_resume(start_params(&temp, None))
        .await
        .unwrap();
    let mut request = title_request(&temp);
    request.workspace = Some(TitleWorkspace::new(
        temp.path().canonicalize().unwrap(),
        [0, 0],
        temp.path().to_owned(),
    ));

    let before = lines(&log);
    let error = started.session.generate_title(request).await.unwrap_err();
    assert!(error.message.contains("conversation folder has changed"));
    assert_eq!(lines(&log), before);
}

#[tokio::test]
async fn codex_title_refuses_custom_routes_and_non_schema_responses() {
    let temp = TempDir::new().unwrap();
    for scenario in [
        "title-custom-config",
        "title-custom-thread",
        "title-managed",
    ] {
        let (harness, log) = harness(&temp, scenario);
        let started = harness
            .start_or_resume(start_params(&temp, None))
            .await
            .unwrap();
        let error = started
            .session
            .generate_title(title_request(&temp))
            .await
            .unwrap_err();
        assert_eq!(error.code, hoh_external_harness::code::POLICY_UNAVAILABLE);
        assert!(
            !lines(&log)
                .iter()
                .any(|entry| entry["stubEvent"] == "title-exec")
        );
    }
    let (harness, _) = harness(&temp, "title-invalid-output");
    let started = harness
        .start_or_resume(start_params(&temp, None))
        .await
        .unwrap();
    let error = started
        .session
        .generate_title(title_request(&temp))
        .await
        .unwrap_err();
    assert_eq!(error.code, hoh_external_harness::code::PROTOCOL);
}

#[tokio::test]
async fn codex_title_refuses_route_changes_after_start_or_resume() {
    for resume in [None, Some("thread-stub".to_owned())] {
        let temp = TempDir::new().unwrap();
        let (harness, log) = harness(&temp, "title-changing-route");
        let marker = temp.path().join("custom-route");
        let initially_custom = resume.is_some();
        if initially_custom {
            fs::write(&marker, "").unwrap();
        }
        let started = harness
            .start_or_resume(start_params(&temp, resume))
            .await
            .unwrap();
        if initially_custom {
            fs::remove_file(&marker).unwrap();
        } else {
            fs::write(&marker, "").unwrap();
        }
        let error = started
            .session
            .generate_title(title_request(&temp))
            .await
            .unwrap_err();
        assert_eq!(error.code, hoh_external_harness::code::POLICY_UNAVAILABLE);
        assert!(
            !lines(&log)
                .iter()
                .any(|entry| entry["stubEvent"] == "title-exec")
        );
    }
}

#[tokio::test]
async fn codex_title_requires_the_current_account_to_match_the_live_thread() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "title-changing-account");
    let started = harness
        .start_or_resume(start_params(&temp, None))
        .await
        .unwrap();
    fs::write(temp.path().join("changed-account"), "").unwrap();

    let error = started
        .session
        .generate_title(title_request(&temp))
        .await
        .unwrap_err();
    assert_eq!(error.code, hoh_external_harness::code::POLICY_UNAVAILABLE);
    assert!(
        !lines(&log)
            .iter()
            .any(|entry| entry["stubEvent"] == "title-exec")
    );
}

#[tokio::test]
async fn codex_title_requires_the_probe_version_to_match_the_live_server() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "title-changing-version");
    let started = harness
        .start_or_resume(start_params(&temp, None))
        .await
        .unwrap();
    fs::write(temp.path().join("changed-version"), "").unwrap();

    let error = started
        .session
        .generate_title(title_request(&temp))
        .await
        .unwrap_err();
    assert_eq!(error.code, hoh_external_harness::code::POLICY_UNAVAILABLE);
    assert!(
        !lines(&log)
            .iter()
            .any(|entry| entry["stubEvent"] == "title-exec")
    );
}

fn lines(path: &Path) -> Vec<Value> {
    let contents = fs::read_to_string(path).unwrap_or_default();
    let complete = contents
        .strip_suffix('\n')
        .unwrap_or_else(|| contents.rsplit_once('\n').map_or("", |(prefix, _)| prefix));
    complete
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn child_is_running(pid: u32) -> bool {
    assert_ne!(pid, 0, "the fixture did not start a child");
    let probe = std::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "state="])
        .env_clear()
        .output()
        .expect("read process state");
    assert!(probe.stderr.is_empty(), "ps failed for {pid}: {probe:?}");
    let state = probe
        .stdout
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace());
    match (probe.status.code(), state) {
        (Some(1), None) => false,
        (Some(0), Some(state)) => !matches!(state, b'Z' | b'X'),
        _ => panic!("ps failed for {pid}: {probe:?}"),
    }
}

async fn running_child(log: &Path) -> u32 {
    let pid = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(pid) = lines(log).iter().find_map(|line| line["childPid"].as_u64()) {
                return u32::try_from(pid).expect("fixture child PID fits in u32");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the fixture reported a child PID");
    assert!(child_is_running(pid), "fixture child {pid} never ran");
    pid
}

async fn assert_child_stopped(pid: u32, message: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while child_is_running(pid) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{message}: child {pid} survived"));
}

async fn event(events: &mut tokio::sync::mpsc::Receiver<HarnessEvent>) -> HarnessEvent {
    tokio::time::timeout(Duration::from_secs(3), events.recv())
        .await
        .unwrap()
        .unwrap()
}

async fn turn_aborted(events: &mut tokio::sync::mpsc::Receiver<HarnessEvent>, expected: &str) {
    loop {
        match event(events).await {
            HarnessEvent::TurnAborted { turn_id } => {
                assert_eq!(turn_id, expected);
                return;
            }
            HarnessEvent::SessionEnded {
                reason, message, ..
            } => panic!("session ended before {expected} was aborted: {reason:?}: {message:?}"),
            _ => {}
        }
    }
}

async fn protocol_end_without_a_request_card(
    events: &mut tokio::sync::mpsc::Receiver<HarnessEvent>,
    scenario: &str,
) {
    loop {
        match event(events).await {
            HarnessEvent::ApprovalRequested { .. } | HarnessEvent::QuestionAsked { .. } => {
                panic!("{scenario}: a stale or foreign request was shown to the user")
            }
            HarnessEvent::SessionEnded {
                reason, message, ..
            } => {
                assert_eq!(reason, hoh_external_harness::EndReason::ProtocolError);
                assert!(message.as_deref().is_some_and(|text| !text.is_empty()));
                return;
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn plan_is_refused_before_codex_process_can_start_even_with_full_access() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "plan-refused");
    let mut params = start_params(&temp, None);
    params.interaction_mode = InteractionMode::Plan;
    let failure = match harness.start_with_access(params, AccessMode::Full).await {
        Ok(_) => panic!("Codex must not run a plan as an ordinary full-access turn"),
        Err(failure) => failure,
    };
    assert!(failure.message.contains("coming soon"));
    assert!(lines(&log).is_empty());
}

#[tokio::test]
async fn discovery_uses_app_server_models_and_account() {
    let temp = TempDir::new().unwrap();
    let (harness, _) = harness(&temp, "discover");
    let (found, status) = harness.discover_with_status().await.unwrap();
    assert_eq!(status, VersionStatus::Supported);
    assert_eq!(found.auth, AuthState::Authenticated);
    assert_eq!(found.models.len(), 1);
    assert_eq!(found.models[0].id, "gpt-stub");
    assert_eq!(found.models[0].default_effort.as_deref(), Some("medium"));
    assert_eq!(found.models[0].effort_levels.len(), 3);
}

#[tokio::test]
async fn folder_trust_is_explicit_in_thread_start_and_retained_for_turn() {
    for trusted in [false, true] {
        let temp = TempDir::new().unwrap();
        let (harness, log) = harness(&temp, "trust");
        let mut started = harness
            .start_with_folder_trust(start_params(&temp, None), AccessMode::Confirm, trusted)
            .await
            .unwrap();
        started
            .session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "turn-trust".into(),
                text: "hello".into(),
                images: Vec::new(),
            })
            .await
            .unwrap();
        loop {
            if matches!(
                event(&mut started.events).await,
                HarnessEvent::ApprovalRequested { .. }
            ) {
                break;
            }
        }
        started.session.terminate(Duration::from_millis(100)).await;
        let wire = lines(&log);
        let thread = wire
            .iter()
            .find(|line| line["method"] == "thread/start")
            .unwrap();
        let turn = wire
            .iter()
            .find(|line| line["method"] == "turn/start")
            .unwrap();
        let cwd = temp.path().to_str().unwrap();
        assert_eq!(
            thread["params"]["config"]["projects"][cwd]["trust_level"],
            if trusted { "trusted" } else { "untrusted" }
        );
        assert_eq!(turn["params"]["cwd"], cwd);
        // Codex 0.166's generated TurnStartParams has no config field; the thread config applies.
        assert!(turn["params"].get("config").is_none());
    }
}

#[tokio::test]
async fn codex_turns_deliver_attached_images_with_or_without_text() {
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGP4z8AARAwQCgAf7gP9i18U1AAAAABJRU5ErkJggg==";
    for text in ["Describe these screenshots", ""] {
        let temp = TempDir::new().unwrap();
        let (harness, log) = harness(&temp, "trust");
        let started = harness
            .start_with_folder_trust(start_params(&temp, None), AccessMode::Confirm, true)
            .await
            .unwrap();
        started
            .session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "turn-images".into(),
                text: text.into(),
                images: ["first.png", "second.png"]
                    .into_iter()
                    .map(|name| PromptImage {
                        media_type: "image/png".into(),
                        data: PNG.into(),
                        name: Some(name.into()),
                    })
                    .collect(),
            })
            .await
            .unwrap();
        started.session.terminate(Duration::from_millis(100)).await;
        let wire = lines(&log);
        let turn = wire
            .iter()
            .find(|line| line["method"] == "turn/start")
            .unwrap();
        let image =
            serde_json::json!({"type":"image","url":format!("data:image/png;base64,{PNG}")});
        let mut expected = vec![];
        if !text.is_empty() {
            expected.push(serde_json::json!({"type":"text","text":text}));
        }
        expected.extend([image.clone(), image]);
        assert_eq!(turn["params"]["input"], serde_json::json!(expected));
    }
}

#[tokio::test]
async fn credential_tool_routes_into_a_dedicated_request_and_refuses_native_secret_questions() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "credential-card");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "turn-secret".into(),
            text: "Request test credential".into(),
            images: vec![],
        })
        .await
        .unwrap();
    let request = loop {
        if let HarnessEvent::QuestionAsked {
            request_id,
            questions,
            ..
        } = event(&mut started.events).await
        {
            assert_eq!(questions.len(), 1);
            assert!(questions[0].secret_file);
            assert_eq!(questions[0].header.as_deref(), Some("Test key"));
            break request_id;
        }
    };
    let path = temp.path().join(format!("turn-{}", "0".repeat(64)));
    let path = path.to_str().unwrap().to_owned();
    started
        .session
        .respond(RespondParams {
            session_id: "hoh-session".into(),
            request_id: request,
            response: HarnessResponse::Answer {
                answers: vec![QuestionAnswer {
                    id: "credential".into(),
                    selected: vec![],
                    custom: Some(path.clone()),
                }],
            },
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if lines(&log).iter().any(|line| line["id"] == "file-secret") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    started.session.terminate(Duration::from_millis(100)).await;
    let wire = lines(&log);
    assert!(wire.iter().any(|line| line["method"] == "thread/start"
        && line["params"]["dynamicTools"][0]["name"] == "request_secret"));
    assert!(
        wire.iter()
            .any(|line| line["id"] == "native-secret" && line["error"]["code"] == -32602)
    );
    let result = wire
        .iter()
        .find(|line| line["id"] == "file-secret")
        .unwrap();
    assert_eq!(result["result"]["success"], true);
    let content: Value = serde_json::from_str(
        result["result"]["contentItems"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(content["status"], "saved");
    assert_eq!(content["file"], path);
}
#[tokio::test]
async fn every_mode_is_present_on_thread_and_first_turn() {
    let expected = [
        (
            AccessMode::Confirm,
            "untrusted",
            "user",
            "workspace-write",
            "workspaceWrite",
        ),
        (
            AccessMode::AutoAcceptEdits,
            "on-request",
            "user",
            "workspace-write",
            "workspaceWrite",
        ),
        (
            AccessMode::Auto,
            "on-request",
            "auto_review",
            "workspace-write",
            "workspaceWrite",
        ),
        (
            AccessMode::Full,
            "granular",
            "user",
            "danger-full-access",
            "dangerFullAccess",
        ),
    ];
    for (index, (mode, approval, reviewer, sandbox, turn_sandbox)) in
        expected.into_iter().enumerate()
    {
        let temp = TempDir::new().unwrap();
        let (harness, log) = harness(&temp, &format!("mode-{index}"));
        let mut started = harness
            .start_with_access(start_params(&temp, None), mode)
            .await
            .unwrap();
        started
            .session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "turn-hoh".into(),
                text: "hello".into(),
                images: Vec::new(),
            })
            .await
            .unwrap();
        loop {
            if matches!(
                event(&mut started.events).await,
                HarnessEvent::ApprovalRequested { .. }
            ) {
                break;
            }
        }
        started.session.terminate(Duration::from_millis(100)).await;
        let wire = lines(&log);
        let thread = wire
            .iter()
            .find(|line| line.get("method") == Some(&Value::String("thread/start".into())))
            .unwrap();
        let turn = wire
            .iter()
            .find(|line| line.get("method") == Some(&Value::String("turn/start".into())))
            .unwrap();
        assert_eq!(
            thread
                .pointer("/params/approvalsReviewer")
                .and_then(Value::as_str),
            Some(reviewer)
        );
        assert_eq!(
            thread.pointer("/params/sandbox").and_then(Value::as_str),
            Some(sandbox)
        );
        assert_eq!(
            turn.pointer("/params/approvalsReviewer")
                .and_then(Value::as_str),
            Some(reviewer)
        );
        assert_eq!(
            turn.pointer("/params/sandboxPolicy/type")
                .and_then(Value::as_str),
            Some(turn_sandbox)
        );
        if mode == AccessMode::Confirm {
            assert_eq!(thread["params"]["approvalPolicy"], "untrusted");
            assert_eq!(
                turn["params"]["sandboxPolicy"]["writableRoots"],
                serde_json::json!([temp.path().to_str().unwrap()])
            );
        } else {
            assert!(
                turn["params"]["sandboxPolicy"]
                    .get("writableRoots")
                    .is_none()
            );
        }
        if approval == "granular" {
            assert_eq!(
                turn.pointer("/params/approvalPolicy/granular/sandbox_approval")
                    .and_then(Value::as_bool),
                Some(false)
            );
        } else {
            assert_eq!(
                turn.pointer("/params/approvalPolicy")
                    .and_then(Value::as_str),
                Some(approval)
            );
        }
    }
}

#[tokio::test]
async fn a_future_model_effort_is_forwarded_without_a_hard_coded_allowlist() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "future-effort");
    let mut params = start_params(&temp, None);
    params.effort = Some("super-high".into());
    let mut started = harness
        .start_with_access(params, AccessMode::Confirm)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "turn-hoh".into(),
            text: "hello".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    loop {
        if matches!(
            event(&mut started.events).await,
            HarnessEvent::ApprovalRequested { .. }
        ) {
            break;
        }
    }
    started.session.terminate(Duration::from_millis(100)).await;
    let wire = lines(&log);
    let turn = wire
        .iter()
        .find(|line| line.get("method") == Some(&Value::String("turn/start".into())))
        .unwrap();
    assert_eq!(
        turn.pointer("/params/effort").and_then(Value::as_str),
        Some("super-high")
    );
}

#[tokio::test]
async fn supervised_single_approval_uses_writable_project_and_single_use_accept() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "supervised-write");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "turn-hoh".into(),
            text: "create a file".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    let request_id = loop {
        if let HarnessEvent::ApprovalRequested { request_id, .. } = event(&mut started.events).await
        {
            break request_id;
        }
    };
    started
        .session
        .respond(RespondParams {
            session_id: "hoh-session".into(),
            request_id,
            response: HarnessResponse::Approval {
                decision: ApprovalDecision::Allow,
            },
        })
        .await
        .unwrap();
    loop {
        match event(&mut started.events).await {
            HarnessEvent::ApprovalRequested { .. } => panic!("unexpected second approval"),
            HarnessEvent::TurnCompleted { .. } => break,
            HarnessEvent::SessionEnded {
                reason, message, ..
            } => panic!("stub failed: {reason:?}: {message:?}"),
            _ => {}
        }
    }
    assert_eq!(
        fs::read_to_string(temp.path().join("approved.txt")).unwrap(),
        "approved"
    );
    started.session.terminate(Duration::from_millis(100)).await;
    let wire = lines(&log);
    let argv = wire.iter().find(|v| v["stubEvent"] == "argv").unwrap();
    assert_eq!(argv["args"][0], "app-server");
    let answer = wire.iter().find(|v| v["id"] == "approval-command").unwrap();
    assert_eq!(answer["result"]["decision"], "accept");
}

#[tokio::test]
async fn approving_file_change_is_single_use_accept() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "approvals");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "turn-hoh".into(),
            text: "edit a file".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    for _ in 0..2 {
        let request_id = loop {
            if let HarnessEvent::ApprovalRequested { request_id, .. } =
                event(&mut started.events).await
            {
                break request_id;
            }
        };
        started
            .session
            .respond(RespondParams {
                session_id: "hoh-session".into(),
                request_id,
                response: HarnessResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            })
            .await
            .unwrap();
    }
    while !matches!(
        event(&mut started.events).await,
        HarnessEvent::TurnCompleted { .. }
    ) {}
    started.session.terminate(Duration::from_millis(100)).await;
    let wire = lines(&log);
    let answer = wire.iter().find(|v| v["id"] == 71).unwrap();
    assert_eq!(answer["result"]["decision"], "accept");
}

#[tokio::test]
async fn stream_steps_and_exact_approval_ids_round_trip() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "approvals");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "turn-hoh".into(),
            text: "hello".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    let mut saw_step = false;
    let first = loop {
        match event(&mut started.events).await {
            HarnessEvent::ItemStarted { .. } => saw_step = true,
            HarnessEvent::ApprovalRequested { request_id, .. } => break request_id,
            _ => {}
        }
    };
    assert!(saw_step);
    started
        .session
        .respond(RespondParams {
            session_id: "hoh-session".into(),
            request_id: first,
            response: HarnessResponse::Approval {
                decision: ApprovalDecision::Allow,
            },
        })
        .await
        .unwrap();
    let second = loop {
        if let HarnessEvent::ApprovalRequested { request_id, .. } = event(&mut started.events).await
        {
            break request_id;
        }
    };
    started
        .session
        .respond(RespondParams {
            session_id: "hoh-session".into(),
            request_id: second,
            response: HarnessResponse::Approval {
                decision: ApprovalDecision::Deny,
            },
        })
        .await
        .unwrap();
    let mut deltas = String::new();
    let mut reasoning = String::new();
    let mut reasoning_final = None;
    let mut tool_started = false;
    let mut tool_output = None;
    let mut tool_completed = false;
    loop {
        match event(&mut started.events).await {
            HarnessEvent::TextDelta { text, .. } => deltas.push_str(&text),
            HarnessEvent::ReasoningDelta { text, .. } => reasoning.push_str(&text),
            HarnessEvent::ReasoningFinal { text, .. } => reasoning_final = Some(text),
            HarnessEvent::ItemStarted {
                item_id, action, ..
            } if item_id == "tool-1"
                && matches!(action, hoh_external_harness::HarnessAction::Tool { .. }) =>
            {
                tool_started = true;
            }
            HarnessEvent::ItemOutput { item_id, text, .. } if item_id == "tool-1" => {
                tool_output = Some(text);
            }
            HarnessEvent::ItemCompleted { item_id, .. } if item_id == "tool-1" => {
                tool_completed = true;
            }
            HarnessEvent::TurnCompleted { .. } => break,
            _ => {}
        }
    }
    assert_eq!(deltas, "part done");
    assert_eq!(reasoning, "checking the result");
    assert_eq!(reasoning_final.as_deref(), Some("checking the result"));
    assert!(tool_started);
    assert_eq!(tool_output.as_deref(), Some("inspected src/lib.rs"));
    assert!(tool_completed);
    started.session.terminate(Duration::from_millis(100)).await;
    let wire = lines(&log);
    assert!(wire.iter().any(|line| line.get("id")
        == Some(&Value::String("approval-command".into()))
        && line.pointer("/result/decision").and_then(Value::as_str) == Some("accept")));
    assert!(
        wire.iter()
            .any(|line| line.get("id").and_then(Value::as_u64) == Some(71)
                && line.pointer("/result/decision").and_then(Value::as_str) == Some("decline"))
    );
}

#[tokio::test]
async fn nested_codex_events_keep_their_parent_through_the_item_lifecycle() {
    let temp = TempDir::new().unwrap();
    let (harness, _) = harness(&temp, "nested-parent");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "turn-hoh".into(),
            text: "nested".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();

    let mut command_started = false;
    let mut command_approval = false;
    let mut command_output = false;
    let mut command_completed = false;
    let mut text_delta = false;
    let mut text_final = false;
    let mut reasoning_delta = false;
    let mut reasoning_final = false;
    loop {
        match event(&mut started.events).await {
            HarnessEvent::ItemStarted {
                item_id,
                parent_item_id,
                ..
            } if item_id == "cmd-1" => {
                assert_eq!(parent_item_id.as_deref(), Some("collab-parent"));
                command_started = true;
            }
            HarnessEvent::ApprovalRequested {
                request_id,
                item_id,
                parent_item_id,
                ..
            } if item_id.as_deref() == Some("cmd-1") => {
                assert_eq!(parent_item_id.as_deref(), Some("collab-parent"));
                command_approval = true;
                started
                    .session
                    .respond(RespondParams {
                        session_id: "hoh-session".into(),
                        request_id,
                        response: HarnessResponse::Approval {
                            decision: ApprovalDecision::Allow,
                        },
                    })
                    .await
                    .unwrap();
            }
            HarnessEvent::ItemOutput {
                item_id,
                parent_item_id,
                ..
            } if item_id == "cmd-1" => {
                assert_eq!(parent_item_id.as_deref(), Some("collab-parent"));
                command_output = true;
            }
            HarnessEvent::ItemCompleted {
                item_id,
                parent_item_id,
                ..
            } if item_id == "cmd-1" => {
                assert_eq!(parent_item_id.as_deref(), Some("collab-parent"));
                command_completed = true;
            }
            HarnessEvent::TextDelta { parent_item_id, .. } => {
                assert_eq!(parent_item_id.as_deref(), Some("collab-parent"));
                text_delta = true;
            }
            HarnessEvent::TextFinal { parent_item_id, .. } => {
                assert_eq!(parent_item_id.as_deref(), Some("collab-parent"));
                text_final = true;
            }
            HarnessEvent::ReasoningDelta { parent_item_id, .. } => {
                assert_eq!(parent_item_id.as_deref(), Some("collab-parent"));
                reasoning_delta = true;
            }
            HarnessEvent::ReasoningFinal { parent_item_id, .. } => {
                assert_eq!(parent_item_id.as_deref(), Some("collab-parent"));
                reasoning_final = true;
            }
            HarnessEvent::TurnCompleted { .. } => break,
            _ => {}
        }
    }
    assert!(command_started && command_approval && command_output && command_completed);
    assert!(text_delta && text_final && reasoning_delta && reasoning_final);
}

#[tokio::test]
async fn resume_uses_the_same_native_thread() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "resume");
    let first = harness
        .start_with_access(start_params(&temp, None), AccessMode::AutoAcceptEdits)
        .await
        .unwrap();
    let native = String::from(first.result.native_session_id.clone());
    first.session.terminate(Duration::from_millis(100)).await;
    let mut second = harness
        .start_with_access(
            start_params(&temp, Some(native.clone())),
            AccessMode::AutoAcceptEdits,
        )
        .await
        .unwrap();
    assert!(second.result.resumed);
    assert_eq!(
        String::from(second.result.native_session_id.clone()),
        native
    );
    second
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "turn-after-restart".into(),
            text: "continue".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    let request_id = loop {
        if let HarnessEvent::ApprovalRequested { request_id, .. } = event(&mut second.events).await
        {
            break request_id;
        }
    };
    second
        .session
        .respond(RespondParams {
            session_id: "hoh-session".into(),
            request_id,
            response: HarnessResponse::Approval {
                decision: ApprovalDecision::Allow,
            },
        })
        .await
        .unwrap();
    loop {
        if matches!(
            event(&mut second.events).await,
            HarnessEvent::TurnCompleted { .. }
        ) {
            break;
        }
    }
    second.session.terminate(Duration::from_millis(100)).await;
    assert!(lines(&log).iter().any(|line| line.get("method")
        == Some(&Value::String("thread/resume".into()))
        && line.pointer("/params/threadId").and_then(Value::as_str) == Some("thread-stub")));
    let wire = lines(&log);
    let resumed_turn = wire
        .iter()
        .rev()
        .find(|line| line.get("method") == Some(&Value::String("turn/start".into())))
        .unwrap();
    assert_eq!(
        resumed_turn
            .pointer("/params/approvalPolicy")
            .and_then(Value::as_str),
        Some("on-request")
    );
    assert_eq!(
        resumed_turn
            .pointer("/params/sandboxPolicy/type")
            .and_then(Value::as_str),
        Some("workspaceWrite")
    );
}

#[tokio::test]
async fn resume_rejects_a_different_returned_thread_without_rebinding() {
    let temp = TempDir::new().unwrap();
    let (harness, _) = harness(&temp, "resume-mismatch");
    let error = harness
        .start_with_access(
            start_params(&temp, Some("thread-stub".into())),
            AccessMode::Confirm,
        )
        .await
        .err()
        .expect("a mismatched resume id must fail");
    assert_eq!(error.code, hoh_external_harness::code::PROTOCOL);
    assert!(error.message.contains("different thread.id"), "{error:?}");
}

#[tokio::test]
async fn duplicate_native_request_ids_close_fail_closed_without_answering_another_action() {
    for scenario in ["duplicate-string", "duplicate-number"] {
        let temp = TempDir::new().unwrap();
        let (harness, log) = harness(&temp, scenario);
        let mut started = harness
            .start_with_access(start_params(&temp, None), AccessMode::Confirm)
            .await
            .unwrap();
        started
            .session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "turn-hoh".into(),
                text: "duplicate".into(),
                images: Vec::new(),
            })
            .await
            .unwrap();
        let first = loop {
            match event(&mut started.events).await {
                HarnessEvent::ApprovalRequested {
                    request_id, action, ..
                } => {
                    assert!(matches!(
                        action,
                        hoh_external_harness::HarnessAction::Command { .. }
                    ));
                    break request_id;
                }
                HarnessEvent::SessionEnded {
                    reason, message, ..
                } => {
                    panic!(
                        "duplicate closed before first request was observable: {reason:?} {message:?}"
                    )
                }
                _ => {}
            }
        };
        let response = started
            .session
            .respond(RespondParams {
                session_id: "hoh-session".into(),
                request_id: first,
                response: HarnessResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            })
            .await;
        assert!(
            response.is_ok(),
            "{scenario}: the first action's answer should win its race"
        );
        let ended = loop {
            if let HarnessEvent::SessionEnded {
                reason, message, ..
            } = event(&mut started.events).await
            {
                break (reason, message);
            }
        };
        assert_eq!(ended.0, hoh_external_harness::EndReason::ProtocolError);
        assert!(
            ended
                .1
                .as_deref()
                .is_some_and(|text| text.contains("reused JSON-RPC"))
        );
        let duplicate = if scenario == "duplicate-string" {
            Value::String("duplicate-approval".into())
        } else {
            Value::from(404)
        };
        let wire = lines(&log);
        assert_eq!(
            wire.iter()
                .filter(|line| line.get("id") == Some(&duplicate) && line.get("result").is_some())
                .count(),
            1,
            "{scenario}: a second action received the first action's approval"
        );
        assert!(
            wire.iter()
                .any(|line| line["stubEvent"] == "second-duplicate-action")
        );
    }
}

#[tokio::test]
async fn subagent_notifications_leave_the_parent_turn_and_its_output_intact() {
    let temp = TempDir::new().unwrap();
    let (harness, _) = harness(&temp, "subagent-notifications");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Full)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "parent-turn".into(),
            text: "fixture".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();

    let mut turns_started = 0;
    let mut text = String::new();
    let mut usage = Vec::new();
    loop {
        match event(&mut started.events).await {
            HarnessEvent::TurnStarted { turn_id } => {
                assert_eq!(turn_id, "parent-turn");
                turns_started += 1;
            }
            HarnessEvent::TextDelta { text: delta, .. } => text.push_str(&delta),
            HarnessEvent::Usage {
                input_tokens,
                cache_creation_input_tokens,
                cache_read_input_tokens,
                output_tokens,
                max_tokens,
            } => usage.push((
                input_tokens,
                cache_creation_input_tokens,
                cache_read_input_tokens,
                output_tokens,
                max_tokens,
            )),
            HarnessEvent::TurnCompleted { turn_id } => {
                assert_eq!(turn_id, "parent-turn");
                break;
            }
            HarnessEvent::SessionEnded {
                reason, message, ..
            } => {
                panic!("child notification stopped the parent: {reason:?}: {message:?}");
            }
            _ => {}
        }
    }
    assert_eq!(turns_started, 1);
    assert_eq!(text, "part done");
    assert_eq!(usage, [(120, 0, 200, 30, Some(200_000))]);
}

#[tokio::test]
async fn foreign_thread_requests_and_resolutions_fail_before_showing_a_card() {
    for (scenario, forbidden_id) in [
        ("foreign-thread-request", Some("foreign-approval")),
        ("foreign-resolved", Some("ghost")),
        ("unknown-resolved", Some("ghost")),
        ("untyped-resolved", None),
    ] {
        let temp = TempDir::new().unwrap();
        let (harness, log) = harness(&temp, scenario);
        let mut started = harness
            .start_with_access(start_params(&temp, None), AccessMode::Confirm)
            .await
            .unwrap();
        started
            .session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "turn-hoh".into(),
                text: scenario.into(),
                images: Vec::new(),
            })
            .await
            .unwrap();
        let child = if scenario == "foreign-thread-request" {
            let child = running_child(&log).await;
            fs::write(log.with_extension("release"), b"send the foreign request").unwrap();
            Some(child)
        } else {
            None
        };
        protocol_end_without_a_request_card(&mut started.events, scenario).await;
        if let Some(forbidden_id) = forbidden_id {
            assert!(
                !lines(&log).iter().any(|line| {
                    line.get("id") == Some(&Value::String(forbidden_id.into()))
                        && line.get("result").is_some()
                }),
                "{scenario}: a result escaped for an invalid request"
            );
        }
        if let Some(child) = child {
            assert_child_stopped(child, "foreign request protocol failure").await;
        }
    }
}

#[tokio::test]
async fn canonical_resolved_without_a_turn_id_removes_the_exact_pending_request() {
    let temp = TempDir::new().unwrap();
    let (harness, _) = harness(&temp, "canonical-resolved");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "turn-hoh".into(),
            text: "resolve canonically".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    let request_id = loop {
        if let HarnessEvent::ApprovalRequested { request_id, .. } = event(&mut started.events).await
        {
            break request_id;
        }
    };
    assert_eq!(request_id, "codex:\"canonical-resolved\"");
    loop {
        match event(&mut started.events).await {
            HarnessEvent::RequestResolved {
                request_id: resolved,
            } => {
                assert_eq!(resolved, request_id);
                break;
            }
            HarnessEvent::SessionEnded {
                reason, message, ..
            } => {
                panic!("canonical resolved failed: {reason:?} {message:?}")
            }
            _ => {}
        }
    }
    started.session.terminate(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn resolved_ids_are_unambiguous_for_the_lifetime_of_the_app_server_process() {
    for scenario in ["stale-resolved", "cross-turn-request-id"] {
        let temp = TempDir::new().unwrap();
        let (harness, log) = harness(&temp, scenario);
        let mut started = harness
            .start_with_access(start_params(&temp, None), AccessMode::Confirm)
            .await
            .unwrap();
        started
            .session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "hoh-first".into(),
                text: "first".into(),
                images: Vec::new(),
            })
            .await
            .unwrap();
        loop {
            if matches!(
                event(&mut started.events).await,
                HarnessEvent::TurnCompleted { .. }
            ) {
                break;
            }
        }
        started
            .session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "hoh-second".into(),
                text: "second".into(),
                images: Vec::new(),
            })
            .await
            .unwrap();
        protocol_end_without_a_request_card(&mut started.events, scenario).await;
        assert!(!lines(&log).iter().any(|line| {
            line.get("id") == Some(&Value::String("resolved-old".into()))
                && line.get("result").is_some()
        }));
    }
}

#[tokio::test]
async fn mcp_elicitation_accepts_schema_exact_omitted_and_null_turn_ids() {
    for (scenario, native_id) in [
        ("mcp-omitted", Value::String("mcp-omitted".into())),
        ("mcp-null", Value::from(909)),
    ] {
        let temp = TempDir::new().unwrap();
        let (harness, log) = harness(&temp, scenario);
        let mut started = harness
            .start_with_access(start_params(&temp, None), AccessMode::Confirm)
            .await
            .unwrap();
        started
            .session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "turn-hoh".into(),
                text: scenario.into(),
                images: Vec::new(),
            })
            .await
            .unwrap();
        let request_id = loop {
            if let HarnessEvent::ApprovalRequested {
                request_id,
                action: hoh_external_harness::HarnessAction::Tool { name, .. },
                ..
            } = event(&mut started.events).await
            {
                assert_eq!(name, "mcp-elicitation");
                break request_id;
            }
        };
        started
            .session
            .respond(RespondParams {
                session_id: "hoh-session".into(),
                request_id,
                response: HarnessResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            })
            .await
            .unwrap();
        loop {
            if matches!(
                event(&mut started.events).await,
                HarnessEvent::TurnCompleted { .. }
            ) {
                break;
            }
        }
        assert!(lines(&log).iter().any(|line| {
            line.get("id") == Some(&native_id)
                && line.pointer("/result/action").and_then(Value::as_str) == Some("accept")
        }));
    }
}

#[tokio::test]
async fn standalone_mcp_elicitation_without_an_active_turn_is_cancelled_on_its_exact_id() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "mcp-no-active");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "turn-hoh".into(),
            text: "finish before standalone elicitation".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    loop {
        match event(&mut started.events).await {
            HarnessEvent::ApprovalRequested { request_id, .. }
                if request_id == "codex:\"mcp-no-active\"" =>
            {
                panic!("standalone elicitation without a turn was shown as a turn approval")
            }
            HarnessEvent::TurnCompleted { .. } => break,
            _ => {}
        }
    }
    let start = Instant::now();
    while !lines(&log).iter().any(|line| {
        line.get("id") == Some(&Value::String("mcp-no-active".into()))
            && line.pointer("/result/action").and_then(Value::as_str) == Some("cancel")
    }) && start.elapsed() < Duration::from_secs(3)
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(lines(&log).iter().any(|line| {
        line.get("id") == Some(&Value::String("mcp-no-active".into()))
            && line.pointer("/result/action").and_then(Value::as_str) == Some("cancel")
    }));
}

#[tokio::test]
async fn standalone_mcp_elicitation_rejects_foreign_thread_and_stale_supplied_turn() {
    for scenario in ["mcp-foreign", "mcp-stale-turn"] {
        let temp = TempDir::new().unwrap();
        let (harness, log) = harness(&temp, scenario);
        let mut started = harness
            .start_with_access(start_params(&temp, None), AccessMode::Confirm)
            .await
            .unwrap();
        started
            .session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "turn-hoh".into(),
                text: scenario.into(),
                images: Vec::new(),
            })
            .await
            .unwrap();
        protocol_end_without_a_request_card(&mut started.events, scenario).await;
        assert!(!lines(&log).iter().any(|line| line.get("result").is_some()));
    }
}

#[tokio::test]
async fn a_request_from_the_prior_turn_cannot_rebind_a_reused_item_id() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "stale-prior-turn");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "hoh-first".into(),
            text: "first".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    loop {
        if matches!(
            event(&mut started.events).await,
            HarnessEvent::TurnCompleted { .. }
        ) {
            break;
        }
    }
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "hoh-second".into(),
            text: "second".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    protocol_end_without_a_request_card(&mut started.events, "stale-prior-turn").await;
    assert!(!lines(&log).iter().any(|line| {
        line.get("id") == Some(&Value::String("stale-approval".into()))
            && line.get("result").is_some()
    }));
}

#[tokio::test]
async fn reusing_an_item_id_for_a_different_action_fails_before_approval() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "reused-item");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "turn-hoh".into(),
            text: "reuse item".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    protocol_end_without_a_request_card(&mut started.events, "reused-item").await;
    assert!(!lines(&log).iter().any(|line| line.get("result").is_some()));
}

#[tokio::test]
async fn parallel_sessions_keep_their_first_turn_access_isolated() {
    let temp = TempDir::new().unwrap();
    let (harness_a, log_a) = harness(&temp, "parallel-auto-accept");
    let (harness_b, log_b) = harness(&temp, "parallel-auto");
    let mut a = harness_a
        .start_with_access(start_params(&temp, None), AccessMode::AutoAcceptEdits)
        .await
        .unwrap();
    let mut b = harness_b
        .start_with_access(start_params(&temp, None), AccessMode::Auto)
        .await
        .unwrap();

    let prompt_a = a.session.prompt(PromptParams {
        session_id: "chat-a".into(),
        turn_id: "turn-a".into(),
        text: "a".into(),
        images: Vec::new(),
    });
    let prompt_b = b.session.prompt(PromptParams {
        session_id: "chat-b".into(),
        turn_id: "turn-b".into(),
        text: "b".into(),
        images: Vec::new(),
    });
    let (result_a, result_b) = tokio::join!(prompt_a, prompt_b);
    result_a.unwrap();
    result_b.unwrap();

    for (started, session_id) in [(&mut a, "chat-a"), (&mut b, "chat-b")] {
        let request_id = loop {
            if let HarnessEvent::ApprovalRequested { request_id, .. } =
                event(&mut started.events).await
            {
                break request_id;
            }
        };
        started
            .session
            .respond(RespondParams {
                session_id: session_id.into(),
                request_id,
                response: HarnessResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            })
            .await
            .unwrap();
        loop {
            if matches!(
                event(&mut started.events).await,
                HarnessEvent::TurnCompleted { .. }
            ) {
                break;
            }
        }
        started.session.terminate(Duration::from_millis(100)).await;
    }

    let a_turn = lines(&log_a)
        .into_iter()
        .find(|line| line.get("method") == Some(&Value::String("turn/start".into())))
        .unwrap();
    let b_turn = lines(&log_b)
        .into_iter()
        .find(|line| line.get("method") == Some(&Value::String("turn/start".into())))
        .unwrap();
    assert_eq!(
        a_turn
            .pointer("/params/approvalsReviewer")
            .and_then(Value::as_str),
        Some("user")
    );
    assert_eq!(
        b_turn
            .pointer("/params/approvalsReviewer")
            .and_then(Value::as_str),
        Some("auto_review")
    );
    assert_eq!(
        a_turn
            .pointer("/params/sandboxPolicy/type")
            .and_then(Value::as_str),
        Some("workspaceWrite")
    );
    assert_eq!(
        b_turn
            .pointer("/params/sandboxPolicy/type")
            .and_then(Value::as_str),
        Some("workspaceWrite")
    );
}

#[tokio::test]
async fn terminate_removes_the_process_tree_without_a_native_interrupt() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "interrupt");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "turn-hoh".into(),
            text: "sleep".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    let child = running_child(&log).await;
    let start = Instant::now();
    started.session.terminate(Duration::from_millis(200)).await;
    assert_child_stopped(child, "Codex termination").await;
    assert!(start.elapsed() < Duration::from_secs(3));
    let _ = event(&mut started.events).await;
}

#[tokio::test]
async fn early_interrupt_waits_for_turn_started_after_result_and_reaps_process_tree() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "result-before-turn-started");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    let session = started.session.clone();
    let prompt = tokio::spawn(async move {
        session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "turn-early-stop".into(),
                text: "sleep".into(),
                images: Vec::new(),
            })
            .await
    });
    let child = running_child(&log).await;

    started.session.interrupt().await.unwrap();
    prompt.await.unwrap().unwrap();
    assert!(
        !lines(&log)
            .iter()
            .any(|line| line.get("method") == Some(&Value::String("turn/interrupt".into()))),
        "turn/start result alone must not make the native turn interruptible"
    );
    assert!(
        !lines(&log).iter().any(|line| line.get("stubEvent")
            == Some(&Value::String("interrupt-before-turn-started".into()))),
        "HoH sent Stop before the canonical turn/started notification"
    );
    fs::write(log.with_extension("release"), b"start native turn").unwrap();

    let mut started_seen = false;
    loop {
        match event(&mut started.events).await {
            HarnessEvent::TurnStarted { turn_id } => {
                assert_eq!(turn_id, "turn-early-stop");
                started_seen = true;
            }
            HarnessEvent::TurnAborted { turn_id } => {
                assert_eq!(turn_id, "turn-early-stop");
                break;
            }
            _ => {}
        }
    }
    assert!(started_seen);
    let interrupts: Vec<_> = lines(&log)
        .into_iter()
        .filter(|line| line.get("method") == Some(&Value::String("turn/interrupt".into())))
        .collect();
    assert_eq!(
        interrupts.len(),
        1,
        "early Stop must send one native interrupt"
    );
    assert_eq!(
        interrupts[0].get("params"),
        Some(&serde_json::json!({"threadId":"thread-stub","turnId":"turn-stub"}))
    );

    started.session.terminate(Duration::from_millis(200)).await;
    assert_child_stopped(child, "early Stop").await;
}

#[tokio::test]
async fn late_turn_start_error_does_not_clear_replacement_turn() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "late-start-error");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    let first_session = started.session.clone();
    let first = tokio::spawn(async move {
        first_session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "turn-old-start".into(),
                text: "old".into(),
                images: Vec::new(),
            })
            .await
    });
    while !lines(&log)
        .iter()
        .any(|line| line.get("method") == Some(&Value::String("turn/start".into())))
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    started.session.interrupt().await.unwrap();
    turn_aborted(&mut started.events, "turn-old-start").await;
    assert_eq!(
        lines(&log)
            .iter()
            .filter(|line| line.get("method") == Some(&Value::String("turn/interrupt".into())))
            .count(),
        1
    );

    let second_session = started.session.clone();
    let second = tokio::spawn(async move {
        second_session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "turn-after-late-start".into(),
                text: "replacement".into(),
                images: Vec::new(),
            })
            .await
    });
    second.await.unwrap().unwrap();
    assert!(first.await.unwrap().is_err());
    started.session.interrupt().await.unwrap();
    turn_aborted(&mut started.events, "turn-after-late-start").await;
    assert_eq!(
        lines(&log)
            .iter()
            .filter(|line| line.get("method") == Some(&Value::String("turn/interrupt".into())))
            .count(),
        2,
        "each generation must receive exactly one native interrupt"
    );
    started.session.terminate(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn late_interrupt_error_does_not_close_replacement_turn() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "late-interrupt-error");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    let first_session = started.session.clone();
    let first = tokio::spawn(async move {
        first_session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "turn-old-interrupt".into(),
                text: "old".into(),
                images: Vec::new(),
            })
            .await
    });
    while !lines(&log)
        .iter()
        .any(|line| line.get("method") == Some(&Value::String("turn/start".into())))
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    started.session.interrupt().await.unwrap();
    turn_aborted(&mut started.events, "turn-old-interrupt").await;
    assert_eq!(
        lines(&log)
            .iter()
            .filter(|line| line.get("method") == Some(&Value::String("turn/interrupt".into())))
            .count(),
        1
    );

    let second_session = started.session.clone();
    let second = tokio::spawn(async move {
        second_session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "turn-after-late-interrupt".into(),
                text: "replacement".into(),
                images: Vec::new(),
            })
            .await
    });
    second.await.unwrap().unwrap();
    assert!(first.await.unwrap().is_err());
    started.session.interrupt().await.unwrap();
    turn_aborted(&mut started.events, "turn-after-late-interrupt").await;
    assert_eq!(
        lines(&log)
            .iter()
            .filter(|line| line.get("method") == Some(&Value::String("turn/interrupt".into())))
            .count(),
        2,
        "a stale interrupt failure must not duplicate or erase the replacement Stop"
    );
    started.session.terminate(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn compatible_app_servers_start_regardless_of_version_metadata() {
    for (banner, expected) in [
        ("codex-cli 0.110.0", Some("0.110.0")),
        ("codex-cli 0.172.0", Some("0.172.0")),
        ("codex-cli 1.100.0-alpha.7", Some("1.100.0-alpha.7")),
        ("not a version", None),
    ] {
        let temp = TempDir::new().unwrap();
        let harness = CodexHarness::new(CodexConfig {
            executable: stub(),
            extra_env: vec![("CODEX_STUB_VERSION".into(), banner.into())],
            ..CodexConfig::default()
        });
        let (discovery, status) = harness.discover_with_status().await.unwrap();
        assert_eq!(status, VersionStatus::Supported);
        assert_eq!(discovery.version.as_deref(), expected);
        let started = harness
            .start_with_access(start_params(&temp, None), AccessMode::Confirm)
            .await
            .unwrap();
        started.session.terminate(Duration::ZERO).await;
    }
}

#[tokio::test]
async fn an_invalid_initialize_response_fails_discovery_and_startup() {
    let temp = TempDir::new().unwrap();
    let (harness, _) = harness(&temp, "bad-initialize");
    let (discovery, status) = harness.discover_with_status().await.unwrap();
    assert_eq!(status, VersionStatus::Unknown);
    assert!(discovery.detail.unwrap().contains("initialize"));
    let Err(error) = harness.start_or_resume(start_params(&temp, None)).await else {
        panic!("invalid handshake accepted");
    };
    assert_eq!(error.code, hoh_external_harness::code::PROTOCOL);
}
