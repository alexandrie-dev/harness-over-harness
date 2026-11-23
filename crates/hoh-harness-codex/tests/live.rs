//! Opt-in smoke test for the founder's installed and authenticated `codex`.
//!
//! Nothing here locates or reads Codex's home. The child owns authentication exactly as it does
//! when started in a terminal. `scripts/cx-live-check.sh` is the supported entry point.

use std::time::{Duration, Instant};

use hoh_external_harness::{
    AccessMode, ApprovalDecision, AuthState, ExternalHarness, HarnessEvent, HarnessResponse,
    PromptParams, RespondParams, StartParams,
};
use hoh_harness_codex::{CodexConfig, CodexHarness};
use tempfile::TempDir;

fn params(
    temp: &TempDir,
    model: String,
    effort: Option<String>,
    resume: Option<String>,
) -> StartParams {
    StartParams {
        session_id: "cx-live".into(),
        cwd: temp.path().to_string_lossy().into_owned(),
        model: Some(model),
        effort,
        policy: Default::default(),
        resume: resume.map(Into::into),
    }
}

async fn finish_turn(
    session: &std::sync::Arc<dyn hoh_external_harness::HarnessSession>,
    events: &mut tokio::sync::mpsc::Receiver<HarnessEvent>,
) -> (String, usize, usize) {
    let mut answer = String::new();
    let mut steps = 0;
    let mut approvals = 0;
    loop {
        let event = tokio::time::timeout(Duration::from_secs(180), events.recv())
            .await
            .expect("codex did not finish the live turn")
            .expect("codex event stream ended during the live turn");
        match event {
            HarnessEvent::ItemStarted { .. } => steps += 1,
            HarnessEvent::TextDelta { text, .. } => answer.push_str(&text),
            HarnessEvent::ApprovalRequested { request_id, .. } => {
                approvals += 1;
                session
                    .respond(RespondParams {
                        session_id: "cx-live".into(),
                        request_id,
                        response: HarnessResponse::Approval {
                            decision: ApprovalDecision::Allow,
                        },
                    })
                    .await
                    .expect("answer Codex approval");
            }
            HarnessEvent::TurnCompleted { .. } => return (answer, steps, approvals),
            HarnessEvent::SessionEnded {
                reason, message, ..
            } => {
                panic!("codex ended during the live turn: {reason:?}: {message:?}")
            }
            _ => {}
        }
    }
}

#[tokio::test]
#[ignore = "uses the installed authenticated codex; run scripts/cx-live-check.sh"]
async fn real_codex_streams_approves_resumes_and_stops() {
    assert_eq!(std::env::var("HOH_CODEX_LIVE").as_deref(), Ok("1"));
    let temp = TempDir::new().expect("live workspace");
    let mut config = CodexConfig::default();
    config.request_timeout = Duration::from_secs(45);
    let harness = CodexHarness::new(config);
    let discovery = harness.discover().await.expect("discover installed Codex");
    assert!(discovery.installed, "Codex is not installed: {discovery:?}");
    assert_eq!(discovery.auth, AuthState::Authenticated, "{discovery:?}");
    let model = discovery
        .models
        .iter()
        .find(|model| model.is_default)
        .or_else(|| discovery.models.first())
        .expect("Codex app-server returned no models");
    let effort = model.default_effort.clone();

    let mut first = harness
        .start_with_access(
            params(&temp, model.id.clone(), effort.clone(), None),
            AccessMode::Confirm,
        )
        .await
        .expect("start a real Codex thread");
    first
        .session
        .prompt(PromptParams {
            session_id: "cx-live".into(),
            turn_id: "cx-live-first".into(),
            text: "Use a shell command to create cx-live.txt containing LIVE, then reply exactly LIVE_OK.".into(),
            images: Vec::new(),
        })
        .await
        .expect("send the first live prompt");
    let (answer, steps, approvals) = finish_turn(&first.session, &mut first.events).await;
    assert!(steps > 0, "no Codex steps were streamed");
    assert!(approvals > 0, "supervised Codex made no approval request");
    assert!(
        !answer.trim().is_empty(),
        "no Codex answer deltas were streamed"
    );
    let native = String::from(first.result.native_session_id.clone());
    first.session.terminate(Duration::from_secs(1)).await;

    let mut resumed = harness
        .start_with_access(
            params(&temp, model.id.clone(), effort, Some(native.clone())),
            AccessMode::Confirm,
        )
        .await
        .expect("resume the real Codex thread after app-server restart");
    assert!(resumed.result.resumed);
    assert_eq!(
        String::from(resumed.result.native_session_id.clone()),
        native
    );
    resumed
        .session
        .prompt(PromptParams {
            session_id: "cx-live".into(),
            turn_id: "cx-live-resume".into(),
            text: "Reply exactly CONTINUED.".into(),
            images: Vec::new(),
        })
        .await
        .expect("continue the resumed Codex thread");
    let (answer, _, _) = finish_turn(&resumed.session, &mut resumed.events).await;
    assert!(
        !answer.trim().is_empty(),
        "resumed Codex streamed no answer"
    );

    resumed
        .session
        .prompt(PromptParams {
            session_id: "cx-live".into(),
            turn_id: "cx-live-stop".into(),
            text: "Run the shell command `sleep 30`, then answer DONE.".into(),
            images: Vec::new(),
        })
        .await
        .expect("start the turn that will be stopped");
    let stopped_at = Instant::now();
    resumed
        .session
        .interrupt()
        .await
        .expect("interrupt Codex turn");
    resumed.session.terminate(Duration::from_millis(500)).await;
    assert!(
        stopped_at.elapsed() < Duration::from_secs(5),
        "Codex stop took {:?}",
        stopped_at.elapsed()
    );
    eprintln!(
        "LIVE_OK model={} steps={steps} approvals={approvals} stop_ms={}",
        model.id,
        stopped_at.elapsed().as_millis()
    );
}
