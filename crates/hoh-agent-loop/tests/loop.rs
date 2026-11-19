//! Port of loop.spec.ts (the cases runnable against the mock adapter).
mod common;

use common::*;
use hoh_agent::{
    Agent, AgentAssistantStream, AgentError, AgentRequestError, AssistantStreamFrame,
    AssistantStreamOutcome, CancelOptions, RequestErrorAction, run_maintenance,
};
use hoh_session::{AgentCancelCause, SessionEventEmitted};
use hoh_timeout::AbortSignal;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn frames_of(env: &Env, agent: &Arc<dyn Agent>) -> Arc<Mutex<Vec<AssistantStreamFrame>>> {
    let frames = Arc::new(Mutex::new(Vec::new()));
    let (sink, id) = (frames.clone(), agent.id().clone());
    env.ctx
        .on_contained::<AgentAssistantStream>(move |event| {
            if event.agent.id() == &id {
                lock(&sink).push(event.frame.clone());
            }
            Ok(())
        })
        .expect("listener");
    frames
}

fn turn_end_reasons(env: &Env) -> Arc<Mutex<Vec<Value>>> {
    let reasons = Arc::new(Mutex::new(Vec::new()));
    let sink = reasons.clone();
    env.ctx
        .on_contained::<SessionEventEmitted>(move |emitted| {
            if emitted.event.event_type() == "turn/end" {
                lock(&sink).push(emitted.event.data()["reason"].clone());
            }
            Ok(())
        })
        .expect("listener");
    reasons
}

fn revision(frame: &AssistantStreamFrame) -> u64 {
    match frame {
        AssistantStreamFrame::Start { revision, .. }
        | AssistantStreamFrame::Chunk { revision, .. }
        | AssistantStreamFrame::End { revision, .. } => *revision,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn publishes_one_dense_live_attempt_and_commits_one_message_with_the_exact_embedded_stream() {
    let env = harness(MockAdapter::chunks(vec![text_response("live")])).await;
    let agent = create_agent(&env, "live-assistant-attempt").await;
    let frames = frames_of(&env, &agent);

    send_and_wait(&agent, "stream this").await;

    let frames = lock(&frames).clone();
    assert!(matches!(
        frames.first(),
        Some(AssistantStreamFrame::Start { .. })
    ));
    assert!(matches!(
        frames.last(),
        Some(AssistantStreamFrame::End { .. })
    ));
    let revisions: Vec<u64> = frames.iter().map(revision).collect();
    assert_eq!(revisions, (1..=frames.len() as u64).collect::<Vec<_>>());
    let chunk_indexes: Vec<u64> = frames
        .iter()
        .filter_map(|frame| match frame {
            AssistantStreamFrame::Chunk { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(
        chunk_indexes,
        (0..chunk_indexes.len() as u64).collect::<Vec<_>>()
    );
    assert!(!event_types(&agent).iter().any(|t| t == "assistant/chunk"));
    let message = last_event(&agent, "assistant/message").expect("assistant message");
    match frames.last() {
        Some(AssistantStreamFrame::End { index, outcome, .. }) => {
            assert_eq!(*index, chunk_indexes.len() as u64);
            match outcome {
                AssistantStreamOutcome::Committed { event_type, seq } => {
                    assert_eq!(*event_type, "assistant/message");
                    assert_eq!(json!(seq), message["seq"]);
                }
                AssistantStreamOutcome::Abandoned => panic!("expected a committed outcome"),
            }
        }
        _ => unreachable!(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn settles_a_failed_attempt_before_retrying_with_a_new_dense_attempt() {
    let failed = vec![
        chunk(json!({"type":"usage","usage":{"inputTokens":1,"outputTokens":0}})),
        chunk(
            json!({"type":"finish","reason":{"kind":"error","failure":{"message":"retry","code":"SERVER"}}}),
        ),
    ];
    let env = harness(MockAdapter::chunks(vec![
        failed,
        text_response("recovered"),
    ]))
    .await;
    let agent = create_agent(&env, "assistant-retry-attempt").await;
    let frames = frames_of(&env, &agent);
    env.ctx
        .on_waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(|_, _| async {
            Ok(Some(RequestErrorAction::Retry))
        })
        .expect("listener");

    send_and_wait(&agent, "retry once").await;

    let frames = lock(&frames).clone();
    let revisions: Vec<u64> = frames.iter().map(revision).collect();
    assert_eq!(revisions, (1..=frames.len() as u64).collect::<Vec<_>>());
    assert_eq!(
        frames
            .iter()
            .filter(|f| matches!(f, AssistantStreamFrame::Start { .. }))
            .count(),
        2
    );
    let ends: Vec<&str> = frames
        .iter()
        .filter_map(|frame| match frame {
            AssistantStreamFrame::End { outcome, .. } => Some(match outcome {
                AssistantStreamOutcome::Committed { event_type, .. } => *event_type,
                AssistantStreamOutcome::Abandoned => "abandoned",
            }),
            _ => None,
        })
        .collect();
    assert_eq!(ends, ["assistant/attempt", "assistant/message"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_concurrent_maintenance_while_one_job_owns_the_agent() {
    let env = harness(MockAdapter::chunks(vec![text_response("unused")])).await;
    let agent = create_agent(&env, "concurrent-maintenance").await;
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let (finish_tx, finish_rx) = tokio::sync::oneshot::channel::<()>();
    let active = run_maintenance(&*agent, move |_signal: AbortSignal| async move {
        let _ = started_tx.send(());
        let _ = finish_rx.await;
        Ok(())
    })
    .expect("maintenance starts");
    let active = tokio::spawn(active);
    started_rx.await.expect("started");

    let second = run_maintenance(&*agent, |_signal: AbortSignal| async { Ok(()) });
    let error = second.err().expect("second maintenance is rejected");
    assert!(error.to_string().contains("already has active work"));

    let _ = finish_tx.send(());
    active.await.expect("joins").expect("maintenance result");
    agent.when_idle().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_maintenance_future_still_returns_the_agent_to_idle() {
    let env = harness(MockAdapter::chunks(vec![text_response("after")])).await;
    let agent = create_agent(&env, "maintenance-dropped").await;
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let (finish_tx, finish_rx) = tokio::sync::oneshot::channel::<()>();
    let work = run_maintenance(&*agent, move |_signal: AbortSignal| async move {
        let _ = started_tx.send(());
        let _ = finish_rx.await;
        Ok(())
    })
    .expect("maintenance starts");
    // The caller abandons the returned future without ever polling it.
    drop(work);
    started_rx
        .await
        .expect("the work runs without the caller polling it");
    let _ = finish_tx.send(());
    tokio::time::timeout(std::time::Duration::from_secs(5), agent.when_idle())
        .await
        .expect("the agent leaves the maintenance phase");

    // The agent is usable again: a turn runs and a new maintenance is admitted.
    send_and_wait(&agent, "next").await;
    run_maintenance(&*agent, |_signal: AbortSignal| async { Ok(()) })
        .expect("a new maintenance is admitted")
        .await
        .expect("it completes");
}

#[tokio::test(flavor = "multi_thread")]
async fn replays_a_wake_latched_behind_maintenance_at_convergence() {
    let adapter = MockAdapter::chunks(vec![text_response("wake reply")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "maintenance-wake-replay").await;
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let (finish_tx, finish_rx) = tokio::sync::oneshot::channel::<()>();
    let maintenance = tokio::spawn(
        run_maintenance(&*agent, move |_signal: AbortSignal| async move {
            let _ = started_tx.send(());
            let _ = finish_rx.await;
            Ok(())
        })
        .expect("maintenance starts"),
    );
    started_rx.await.expect("started");

    agent
        .followup(user_message("wake behind maintenance"))
        .expect("followup");
    let _ = finish_tx.send(());
    maintenance
        .await
        .expect("joins")
        .expect("maintenance result");
    agent.when_idle().await;

    assert_eq!(user_texts(&agent), ["wake behind maintenance"]);
    assert_eq!(adapter.request_count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn suppresses_the_replay_when_a_latched_maintenance_wake_is_removed() {
    let adapter = MockAdapter::chunks(vec![]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "maintenance-wake-removed").await;
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let (finish_tx, finish_rx) = tokio::sync::oneshot::channel::<()>();
    let maintenance = tokio::spawn(
        run_maintenance(&*agent, move |_signal: AbortSignal| async move {
            let _ = started_tx.send(());
            let _ = finish_rx.await;
            Ok(())
        })
        .expect("maintenance starts"),
    );
    started_rx.await.expect("started");

    let wake = user_message("removed wake");
    agent.followup(wake.clone()).expect("followup");
    assert!(agent.inbox().remove(&wake.id).expect("remove"));
    let _ = finish_tx.send(());
    maintenance
        .await
        .expect("joins")
        .expect("maintenance result");
    agent.when_idle().await;

    assert!(user_texts(&agent).is_empty());
    assert_eq!(adapter.request_count(), 0);
    assert!(!event_types(&agent).iter().any(|t| t == "turn/start"));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancels_queued_wakeup_work_together_with_an_active_maintenance_task() {
    let adapter = MockAdapter::chunks(vec![text_response("park reply")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "cancel-maintenance-wakeup").await;
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let maintenance = tokio::spawn(
        run_maintenance(&*agent, move |signal: AbortSignal| async move {
            let _ = started_tx.send(());
            let reason = signal.cancelled().await;
            Err::<(), _>(format!("maintenance aborted: {reason}").into())
        })
        .expect("maintenance starts"),
    );
    started_rx.await.expect("started");

    agent
        .followup(user_message("discard this wakeup"))
        .expect("latched");
    agent
        .cancel(AgentCancelCause::User, CancelOptions::default())
        .expect("cancel");
    agent
        .followup(user_message("park after cancellation"))
        .expect("re-latched");

    let error = maintenance
        .await
        .expect("joins")
        .expect_err("maintenance rejects");
    assert!(error.to_string().contains("maintenance aborted"));
    agent.when_idle().await;

    // The pre-cancel wakeup is gone; the post-cancel wake replays at convergence.
    assert_eq!(user_texts(&agent), ["park after cancellation"]);
    assert!(agent.inbox().next_turn().is_empty());
    assert_eq!(adapter.request_count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn runs_a_simple_turn_queued_message_model_idle_with_ordered_events() {
    let adapter = MockAdapter::chunks(vec![text_response("hello there")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "a1").await;
    let order = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = order.clone();
    env.ctx
        .on_contained::<SessionEventEmitted>(move |emitted| {
            let kind = emitted.event.event_type();
            if ["turn/start", "step/start", "step/end", "turn/end"].contains(&kind) {
                lock(&sink).push(kind.to_owned());
            }
            Ok(())
        })
        .expect("listener");

    send_and_wait(&agent, "hi").await;

    assert_eq!(
        *lock(&order),
        ["turn/start", "step/start", "step/end", "turn/end"]
    );
    let types = event_types(&agent);
    // Durable inbox receipt precedes the turn-owned transcript.
    assert_eq!(types[0], "agent/inbox/spliced");
    for expected in ["turn/start", "user/message", "assistant/message"] {
        assert!(
            types.iter().any(|t| t == expected),
            "{expected} in {types:?}"
        );
    }
    let message = last_event(&agent, "assistant/message").expect("assistant message");
    assert_eq!(
        message["data"]["usage"],
        json!({"inputTokens": 10, "outputTokens": "hello there".len()})
    );
    assert_eq!(types.last().map(String::as_str), Some("turn/end"));

    // derived history: system prompt (surface node 0) + user + assistant
    let messages = agent.session().derive_messages().expect("derive");
    let roles: Vec<&str> = messages
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["system", "user", "assistant"]);
    assert_eq!(
        messages[2]["content"],
        json!([{"type": "text", "text": "hello there"}])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_mid_stream_ends_the_turn_with_reason_aborted() {
    let env = harness(MockAdapter::new(vec![Entry::Hang])).await;
    let agent = create_agent(&env, "a1").await;
    let reasons = turn_end_reasons(&env);
    let frames = frames_of(&env, &agent);

    agent.followup(user_message("go")).expect("followup");
    // wait until the stream is hanging, then cancel
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(agent.status(), hoh_agent::AgentStatus::Running);
    agent
        .cancel(AgentCancelCause::User, CancelOptions::default())
        .expect("cancel");
    agent.when_idle().await;

    assert_eq!(
        *lock(&reasons),
        [json!({"kind": "aborted", "reason": {"kind": "user"}})]
    );
    let frames = lock(&frames).clone();
    let chunks = frames
        .iter()
        .filter(|f| matches!(f, AssistantStreamFrame::Chunk { .. }))
        .count() as u64;
    match frames.last() {
        Some(AssistantStreamFrame::End { index, outcome, .. }) => {
            assert_eq!(*index, chunks);
            assert!(matches!(outcome, AssistantStreamOutcome::Abandoned));
            assert!(last_event(&agent, "assistant/message").is_some());
        }
        other => panic!("expected an end frame, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn surfaces_max_tokens_as_the_turn_end_reason_when_the_last_step_is_cut_off() {
    let adapter = MockAdapter::chunks(vec![max_tokens_response("truncat")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "a1").await;
    let reasons = turn_end_reasons(&env);

    send_and_wait(&agent, "go").await;

    assert_eq!(adapter.request_count(), 1);
    assert_eq!(*lock(&reasons), [json!({"kind": "max-tokens"})]);
    // Assert the durable row, not only the live listener.
    let turn_end = last_event(&agent, "turn/end").expect("turn end");
    assert_eq!(turn_end["data"]["reason"], json!({"kind": "max-tokens"}));
}

#[tokio::test(flavor = "multi_thread")]
async fn records_normalized_model_errors_on_the_turn_boundary() {
    let env = harness(MockAdapter::chunks(vec![])).await; // script exhausted -> fails
    let agent = create_agent(&env, "a1").await;
    let errors = Arc::new(Mutex::new(Vec::<hoh_error::SharedError>::new()));
    let sink = errors.clone();
    env.ctx
        .on_contained::<AgentError>(move |event| {
            lock(&sink).push(event.error.clone());
            Ok(())
        })
        .expect("listener");
    let reasons = turn_end_reasons(&env);

    send_and_wait(&agent, "hi").await;

    let errors = lock(&errors).clone();
    assert_eq!(errors.len(), 1);
    let harness_error = hoh_error::HarnessError::of(&errors[0]).expect("an LlmError");
    assert!(harness_error.is_named(hoh_llm::LlmError::NAME));
    let failure = harness_error
        .detail::<hoh_llm::LlmFailure>()
        .expect("failure detail");
    assert_eq!(failure.message, "MockAdapter: script exhausted");
    assert_eq!(failure.code, "UNKNOWN");
    assert_eq!(lock(&reasons)[0]["kind"], "error");
    // The durable failure and live relay describe the same failed turn.
    let turn_end = last_event(&agent, "turn/end").expect("turn end");
    assert_eq!(turn_end["data"]["reason"]["kind"], "error");
}

#[tokio::test(flavor = "multi_thread")]
async fn creates_agents_from_config_on_startup_and_attaches_cwd_to_the_fresh_header() {
    let env = harness_with(
        MockAdapter::chunks(vec![text_response("from config")]),
        json!({"agents": [{"id": "config-agent", "provider": "mock", "model": "mock", "cwd": "/work/project"}]}),
    )
    .await;
    let agents = env.ctx.get::<hoh_agent::Agents>().expect("agents");
    for _ in 0..200 {
        if agents.list().len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let agent = agents.list().first().cloned().expect("config agent exists");
    assert_eq!(agent.id(), agent.session().id());
    assert!(agent.id().to_string().starts_with("config-agent-session-"));
    assert_eq!(agent.options().model().as_deref(), Some("mock"));
    assert_eq!(agent.session().header()["cwd"], "/work/project");

    // the agent is alive: send triggers a turn
    send_and_wait(&agent, "hi").await;
    assert_eq!(env.adapter.request_count(), 1);
}

/// Review item 3: a broken persisted splice is an error at the read, not an empty inbox.
#[tokio::test]
async fn reading_the_inbox_reports_an_invalid_persisted_splice() {
    let env = harness(MockAdapter::chunks(vec![text_response("ok")])).await;
    let agent = create_agent(&env, "bad-splice").await;
    let appended = agent.session().append(
        "agent/inbox/spliced",
        json!({"target": "next-turn", "start": 7, "inserted": []}),
        None,
    );
    // The eager fold may already refuse the append; either way the read must not hide it.
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| agent.inbox().next_turn()));
    let message = match (&appended, read) {
        (_, Err(panic)) => panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default(),
        (Err(error), Ok(_)) => error.to_string(),
        (Ok(_), Ok(pending)) => panic!("inbox read hid the broken splice: {pending:?}"),
    };
    assert!(
        message.contains("invalid persisted inbox splice at session seq"),
        "unexpected message: {message}"
    );
}

/// Regression: the request handed to the adapter carries the derived history, not just the system prompt.
#[tokio::test]
async fn the_adapter_request_carries_the_user_message_and_the_history() {
    let adapter = MockAdapter::chunks(vec![text_response("first"), text_response("second")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "request-history").await;

    send_and_wait(&agent, "the question").await;
    send_and_wait(&agent, "the follow-up").await;

    let requests = lock(&adapter.requests);
    let roles = |index: usize| -> Vec<String> {
        requests[index]
            .messages
            .iter()
            .map(|m| {
                serde_json::to_value(m).unwrap()["role"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    };
    let dump = |index: usize| serde_json::to_string(&requests[index].messages).unwrap();
    assert_eq!(roles(0), ["system", "user"], "{}", dump(0));
    assert!(dump(0).contains("the question"), "{}", dump(0));
    assert_eq!(
        roles(1),
        ["system", "user", "assistant", "user"],
        "{}",
        dump(1)
    );
    assert!(dump(1).contains("first") && dump(1).contains("the follow-up"));
}

/// The runtime hands the adapter a user message as a user message (not a developer message).
#[tokio::test]
async fn the_adapter_receives_user_messages_with_the_user_role() {
    let adapter = MockAdapter::chunks(vec![text_response("ok")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "request-roles").await;
    send_and_wait(&agent, "hello").await;
    let requests = lock(&adapter.requests);
    let roles: Vec<&str> = requests[0].messages.iter().map(|m| m.role()).collect();
    assert_eq!(roles, ["system", "user"]);
}

/// A message projection that rejects every `step/end` event at planning, so the append that
/// closes a step fails.
struct StepEndTrap;

impl hoh_session::SessionMessageProjection for StepEndTrap {
    fn event_type(&self) -> &str {
        "step/end"
    }
    fn project(
        &self,
        _event: &hoh_session::SessionEvent,
        _context: hoh_session::SessionMessageProjectionContext<'_>,
    ) -> hoh_session::Result<indexmap::IndexMap<hoh_session::SessionSeq, Arc<Value>>> {
        Err("step/end rejected by the trap".into())
    }
}

#[tokio::test]
async fn a_failure_to_close_the_step_supersedes_the_steps_own_error() {
    // The script is empty, so the step fails; a trap projection then rejects the `step/end`
    // append that closes it. The source's `finally` makes the closing failure the thrown one.
    let env = harness(MockAdapter::chunks(vec![])).await;
    let _trap = env
        .ctx
        .get::<hoh_session::Sessions>()
        .expect("sessions")
        .register_message_projection(&env.ctx, Arc::new(StepEndTrap))
        .expect("trap registers");
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = errors.clone();
    env.ctx
        .on_contained::<AgentError>(move |event| {
            lock(&sink).push(format!("{:?}", event.error));
            Ok(())
        })
        .expect("listener");
    let agent = create_agent(&env, "step-end-priority").await;

    send_and_wait(&agent, "hi").await;

    let errors = lock(&errors).clone();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("step/end rejected by the trap"),
        "the closing failure must win over the step's own failure: {errors:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wake_delayed_after_its_message_was_handled_opens_no_empty_turn() {
    let adapter = MockAdapter::chunks(vec![
        text_response("first reply"),
        text_response("late reply"),
    ]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "late-wake").await;

    // The first model step waits at a gate, so the delayed sender's message is queued meanwhile.
    let gate = Arc::new(tokio::sync::Notify::new());
    let first_step = Arc::new(std::sync::atomic::AtomicBool::new(true));
    {
        let (gate, first_step) = (gate.clone(), first_step.clone());
        env.ctx
            .on_waterfall::<hoh_agent::AgentPreStep, hoh_agent::PreStepDecision, _>(
                move |_event, next| {
                    let (gate, first_step) = (gate.clone(), first_step.clone());
                    async move {
                        if first_step.swap(false, std::sync::atomic::Ordering::SeqCst) {
                            gate.notified().await;
                        }
                        next.run().await
                    }
                },
            )
            .expect("gate listener");
    }
    // A synchronous listener holds the sender of "late" after its message is already inserted.
    let (inserted_tx, inserted_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = Mutex::new(Some(release_rx));
    env.ctx
        .on_contained::<hoh_agent::AgentInboxInserted>(move |event| {
            if format!("{:?}", event.message.content).contains("late") {
                let _ = inserted_tx.send(());
                if let Some(release) = lock(&release_rx).take() {
                    let _ = release.recv();
                }
            }
            Ok(())
        })
        .expect("holding listener");

    send_and_wait_started(&agent, "first");
    let sender = {
        let (agent, handle) = (agent.clone(), tokio::runtime::Handle::current());
        std::thread::spawn(move || {
            let _runtime = handle.enter();
            agent.followup(user_message("late")).expect("late followup");
        })
    };
    tokio::task::spawn_blocking(move || inserted_rx.recv().expect("late was inserted"))
        .await
        .expect("join");

    // Release the first step: the driver handles "first", then "late" (already queued), and idles.
    gate.notify_one();
    until(|| {
        agent.status() == hoh_agent::AgentStatus::Idle && events_of(&agent, "turn/end").len() == 2
    })
    .await;
    // Now the delayed sender proceeds to its wake: its message is long handled.
    release_tx.send(()).expect("release the sender");
    tokio::task::spawn_blocking(move || sender.join().expect("sender joined"))
        .await
        .expect("join");
    agent.when_idle().await;

    assert_eq!(events_of(&agent, "turn/start").len(), 2);
    assert_eq!(adapter.request_count(), 2);
    assert_eq!(user_texts(&agent), ["first", "late"]);
}

fn send_and_wait_started(agent: &Arc<dyn Agent>, text: &str) {
    agent.followup(user_message(text)).expect("followup");
}

/// The same delayed sender while the driver is still running: its wake must not count either,
/// or the final lap would claim work (`note`) that the source leaves queued.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wake_delayed_past_its_handled_message_leaves_later_work_queued_while_running() {
    let adapter = MockAdapter::new(vec![
        Entry::Chunks(text_response("first reply")),
        Entry::Fail("boom"),
        Entry::Chunks(text_response("must not be asked")),
    ]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "late-wake-running").await;

    let gate = Arc::new(tokio::sync::Notify::new());
    let first_step = Arc::new(std::sync::atomic::AtomicBool::new(true));
    {
        let (gate, first_step) = (gate.clone(), first_step.clone());
        env.ctx
            .on_waterfall::<hoh_agent::AgentPreStep, hoh_agent::PreStepDecision, _>(
                move |_event, next| {
                    let (gate, first_step) = (gate.clone(), first_step.clone());
                    async move {
                        if first_step.swap(false, std::sync::atomic::Ordering::SeqCst) {
                            gate.notified().await;
                        }
                        next.run().await
                    }
                },
            )
            .expect("gate listener");
    }
    let (inserted_tx, inserted_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = Mutex::new(Some(release_rx));
    env.ctx
        .on_contained::<hoh_agent::AgentInboxInserted>(move |event| {
            if format!("{:?}", event.message.content).contains("late") {
                let _ = inserted_tx.send(());
                if let Some(release) = lock(&release_rx).take() {
                    let _ = release.recv();
                }
            }
            Ok(())
        })
        .expect("holding listener");
    // The second request fails; inside that failure report the delayed sender is released and
    // waited for while the driver is still running, and a plain `inject` is queued.
    let (sent_tx, sent_rx) = std::sync::mpsc::channel::<()>();
    let sent_rx = Mutex::new(Some(sent_rx));
    let release_tx = Mutex::new(Some(release_tx));
    env.ctx
        .on_contained::<AgentError>(move |event| {
            event.agent.inject(user_message("note"))?;
            if let Some(release) = lock(&release_tx).take() {
                let _ = release.send(());
                if let Some(sent) = lock(&sent_rx).take() {
                    let _ = sent.recv_timeout(Duration::from_secs(5));
                }
            }
            Ok(())
        })
        .expect("error listener");

    send_and_wait_started(&agent, "first");
    let sender = {
        let (agent, handle) = (agent.clone(), tokio::runtime::Handle::current());
        std::thread::spawn(move || {
            let _runtime = handle.enter();
            agent.followup(user_message("late")).expect("late followup");
            let _ = sent_tx.send(());
        })
    };
    tokio::task::spawn_blocking(move || inserted_rx.recv().expect("late was inserted"))
        .await
        .expect("join");

    gate.notify_one();
    tokio::task::spawn_blocking(move || sender.join().expect("sender joined"))
        .await
        .expect("join");
    agent.when_idle().await;

    assert_eq!(events_of(&agent, "turn/start").len(), 2);
    assert_eq!(adapter.request_count(), 2);
    assert_eq!(user_texts(&agent), ["first", "late"]);
    // Left queued, as in the source: nothing woke the agent after the failure.
    assert_eq!(agent.inbox().next_step().len(), 1);
}
