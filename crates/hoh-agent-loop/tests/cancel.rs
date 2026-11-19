//! Port of cancel.spec.ts: cancellation mid-turn, mid-stream, from synchronous listeners, with
//! `keepInbox`, the latched wake and the typed cause. Cases that mutate the JS cause object or need
//! `Error.captureStackTrace` have no counterpart (see PORT_NOTES.md); the typed cause is checked per
//! variant instead.
mod common;

use common::*;
use hoh_agent::{
    Agent, AgentAssistantStream, AgentStatus, AgentStatusChanged, AgentTurnStopping, Agents,
    AssistantStreamFrame, AssistantStreamOutcome, CancelCauseError, CancelOptions,
    CreateAgentOptions,
};
use hoh_llm::StreamChunk;
use hoh_session::{AgentCancelCause, SessionEventEmitted};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

fn cancel(agent: &Arc<dyn Agent>, cause: AgentCancelCause) {
    agent
        .cancel(cause, CancelOptions::default())
        .expect("cancel");
}

fn user_cancel(agent: &Arc<dyn Agent>) {
    cancel(agent, AgentCancelCause::User);
}

fn keep_inbox_cancel(agent: &Arc<dyn Agent>) {
    agent
        .cancel(AgentCancelCause::User, CancelOptions { keep_inbox: true })
        .expect("cancel");
}

fn send(agent: &Arc<dyn Agent>, text: &str) {
    agent.followup(user_message(text)).expect("followup");
}

fn endings(agent: &Arc<dyn Agent>) -> Vec<Value> {
    events_of(agent, "turn/end")
        .iter()
        .map(|event| event["data"]["reason"].clone())
        .collect()
}

fn count(agent: &Arc<dyn Agent>, event_type: &str) -> usize {
    events_of(agent, event_type).len()
}

fn aborted(cause: Value) -> Value {
    json!({ "kind": "aborted", "reason": cause })
}

fn completed() -> Value {
    json!({ "kind": "completed" })
}

fn pending_turns(agent: &Arc<dyn Agent>) -> usize {
    agent.inbox().next_turn().len()
}

async fn promptly(job: impl std::future::Future<Output = ()>, what: &str) {
    tokio::time::timeout(Duration::from_secs(1), job)
        .await
        .unwrap_or_else(|_| panic!("{what}"));
}

// ---------------------------------------------------------------- the typed cause

#[tokio::test]
async fn records_each_typed_cancellation_cause_and_closes_the_turn() {
    let causes = [
        (AgentCancelCause::User, json!({"kind": "user"})),
        (AgentCancelCause::Parent, json!({"kind": "parent"})),
        (
            AgentCancelCause::Hook {
                reason: "policy stopped this turn".into(),
            },
            json!({"kind": "hook", "reason": "policy stopped this turn"}),
        ),
        (AgentCancelCause::Disposed, json!({"kind": "disposed"})),
    ];
    for (cause, expected) in causes {
        let adapter =
            MockAdapter::new(vec![Entry::Hang, Entry::Chunks(text_response("next turn"))]);
        let env = harness(adapter.clone()).await;
        let agent = create_agent(&env, "cancel-cause").await;
        send(&agent, "cancel this turn");
        until(|| adapter.request_count() == 1).await;
        cancel(&agent, cause);
        agent.when_idle().await;
        // A second turn proves the cancelled one closed: an unclosed turn would make the next
        // `turn/start` fail the session's turn-bracket invariant.
        send_and_wait(&agent, "continue").await;

        assert_eq!(endings(&agent), [aborted(expected.clone()), completed()]);
        assert_eq!(adapter.request_count(), 2, "{expected}");
        env.close().await;
    }
}

#[tokio::test]
async fn keeps_the_first_typed_cause_for_an_active_turn() {
    let adapter = MockAdapter::new(vec![Entry::Hang]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "typed-first-wins").await;

    send(&agent, "go");
    until(|| adapter.request_count() == 1).await;
    cancel(&agent, AgentCancelCause::Parent);
    cancel(&agent, AgentCancelCause::User);
    agent.when_idle().await;

    let signal = lock(&adapter.requests)[0].signal.clone().expect("signal");
    let reason = signal.reason().expect("aborted with a reason");
    assert_eq!(
        reason.downcast_ref::<CancelCauseError>(),
        Some(&CancelCauseError(AgentCancelCause::Parent))
    );
    assert_eq!(endings(&agent), [aborted(json!({"kind": "parent"}))]);
}

// ---------------------------------------------------------------- idle and keepInbox

#[tokio::test]
async fn cancel_on_an_idle_agent_with_nothing_queued_is_a_no_op() {
    let adapter = MockAdapter::chunks(vec![text_response("reply")]);
    let env = harness(adapter).await;
    let agent = create_agent(&env, "a1").await;

    // The loop is parked at the idle wait with nothing queued. A cancel here must NOT arm the
    // marker, or the next legitimate prompt would be dropped.
    user_cancel(&agent);
    send_and_wait(&agent, "real prompt").await;

    assert_eq!(user_texts(&agent), ["real prompt"]);
    assert_eq!(count(&agent, "turn/end"), 1);
}

#[tokio::test]
async fn keep_inbox_does_not_restore_work_already_claimed_by_a_waking_send() {
    let adapter = MockAdapter::chunks(vec![text_response("wake reply")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "a1").await;

    send(&agent, "preserved");
    // A waking send starts and claims synchronously, so keepInbox has no pending item to
    // preserve by the time this cancellation runs.
    keep_inbox_cancel(&agent);
    assert!(
        !events_of(&agent, "agent/inbox/spliced")
            .iter()
            .any(|event| event["data"]["outcome"] == "canceled")
    );
    agent.when_idle().await;
    assert_eq!(pending_turns(&agent), 0);
    assert!(user_texts(&agent).is_empty());
    assert_eq!(adapter.request_count(), 0);
    assert_eq!(
        endings(&agent).last(),
        Some(&aborted(json!({"kind": "user"})))
    );

    send_and_wait(&agent, "wake it").await;
    assert_eq!(user_texts(&agent), ["wake it"]);
    assert_eq!(adapter.request_count(), 1);
}

#[tokio::test]
async fn keep_inbox_parks_queued_work_after_an_active_turn_aborts() {
    let adapter = MockAdapter::new(vec![
        Entry::Hang,
        Entry::Chunks(text_response("preserved reply")),
        Entry::Chunks(text_response("wake reply")),
    ]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "keep-after-abort").await;

    send(&agent, "active");
    until(|| adapter.request_count() == 1).await;
    send(&agent, "preserved");
    keep_inbox_cancel(&agent);
    agent.when_idle().await;

    assert_eq!(user_texts(&agent), ["active"]);
    assert_eq!(pending_turns(&agent), 1);
    assert_eq!(adapter.request_count(), 1);

    send_and_wait(&agent, "wake it").await;
    assert_eq!(user_texts(&agent), ["active", "preserved", "wake it"]);
    assert_eq!(adapter.request_count(), 3);
}

#[tokio::test]
async fn keep_inbox_latches_a_waking_send_landing_in_the_abort_to_idle_window() {
    let adapter = MockAdapter::new(vec![Entry::Hang, Entry::Chunks(text_response("B reply"))]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "latch-window").await;

    send(&agent, "active");
    until(|| adapter.request_count() == 1).await;

    // The abort signal is set but the driver has not converged to idle yet: the waking send
    // must be latched, not parked until another wake.
    keep_inbox_cancel(&agent);
    send(&agent, "B");
    agent.when_idle().await;

    assert_eq!(user_texts(&agent), ["active", "B"]);
    assert_eq!(adapter.request_count(), 2);
    assert_eq!(pending_turns(&agent), 0);
    assert_eq!(
        endings(&agent),
        [aborted(json!({"kind": "user"})), completed()]
    );
}

#[tokio::test]
async fn cancel_without_keep_inbox_clears_a_latched_wake_with_the_inbox() {
    let adapter = MockAdapter::new(vec![Entry::Hang, Entry::Chunks(text_response("C reply"))]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "latch-cleared").await;

    send(&agent, "active");
    until(|| adapter.request_count() == 1).await;
    keep_inbox_cancel(&agent);
    send(&agent, "B"); // latched behind the aborted activity
    user_cancel(&agent); // drops the inbox and the latch with it
    agent.when_idle().await;

    assert_eq!(user_texts(&agent), ["active"]);
    assert_eq!(pending_turns(&agent), 0);
    assert_eq!(adapter.request_count(), 1);

    send_and_wait(&agent, "C").await;
    assert_eq!(user_texts(&agent), ["active", "C"]);
    assert_eq!(adapter.request_count(), 2);
}

#[tokio::test]
async fn removing_the_latched_wake_before_convergence_suppresses_the_replay() {
    let adapter = MockAdapter::new(vec![Entry::Hang]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "removed-latched-wake").await;

    send(&agent, "active");
    until(|| adapter.request_count() == 1).await;

    keep_inbox_cancel(&agent);
    let steer = user_message("steer me");
    agent.steer(steer.clone()).expect("steer"); // latched behind the aborted activity
    agent.inbox().remove(&steer.id).expect("remove"); // the wake is retracted before convergence
    agent.when_idle().await;

    assert_eq!(user_texts(&agent), ["active"]);
    assert_eq!(adapter.request_count(), 1);
    assert_eq!(pending_turns(&agent), 0);
    assert_eq!(agent.status(), AgentStatus::Idle);
    // No replay with nothing to run: no empty follow-up turn is recorded.
    assert_eq!(count(&agent, "turn/start"), 1);
}

#[tokio::test]
async fn latches_a_wake_arriving_deep_into_a_slow_abort_convergence() {
    // The stream notices the abort only after 50ms, so the driver stays in the abort-to-idle
    // window long after `cancel()` returned: the wake must be latched across the whole window.
    let adapter = MockAdapter::new(vec![
        Entry::HangSlow(Duration::from_millis(50)),
        Entry::Chunks(text_response("B reply")),
    ]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "slow-convergence").await;

    send(&agent, "A");
    until(|| adapter.request_count() == 1).await;
    keep_inbox_cancel(&agent);
    settle(10).await;
    send(&agent, "B");
    agent.when_idle().await;

    assert_eq!(user_texts(&agent), ["A", "B"]);
    assert_eq!(adapter.request_count(), 2);
    assert_eq!(pending_turns(&agent), 0);
}

// ---------------------------------------------------------------- disposal windows

async fn create_handle(env: &Env, id: &str) -> hoh_agent::AgentHandle {
    let agents = env.ctx.get::<Agents>().expect("agents service");
    let mut options = CreateAgentOptions::new(id.into());
    options.agent_options = Some(mock_options());
    agents.create(&env.ctx, options).await.expect("agent")
}

#[tokio::test]
async fn does_not_latch_a_wake_landing_after_disposal_begins() {
    let adapter = MockAdapter::new(vec![
        Entry::HangSlow(Duration::from_millis(50)),
        Entry::Chunks(text_response("late reply")),
    ]);
    let env = harness(adapter.clone()).await;
    let handle = create_handle(&env, "dispose-window-wake").await;
    let agent = handle.agent.clone();

    let streaming = Arc::new(AtomicBool::new(false));
    let flag = streaming.clone();
    env.ctx
        .on_contained::<AgentAssistantStream>(move |event| {
            if matches!(event.frame, hoh_agent::AssistantStreamFrame::Chunk { .. }) {
                flag.store(true, Ordering::SeqCst);
            }
            Ok(())
        })
        .expect("listener");
    send(&agent, "active");
    until(|| streaming.load(Ordering::SeqCst)).await;
    let mut disposal = Box::pin(handle.dispose());
    // Disposal begins at its first poll: abort first, then the late wake.
    let early = tokio::time::timeout(Duration::from_millis(1), &mut disposal)
        .await
        .ok();
    let _ = agent.followup(user_message("late wake"));
    match early {
        Some(done) => done.expect("disposal"),
        None => disposal.await.expect("disposal"),
    }

    assert_eq!(adapter.request_count(), 1);
    assert_eq!(user_texts(&agent), ["active"]);
}

#[tokio::test]
async fn does_not_start_an_idle_agent_after_disposal_begins() {
    let adapter = MockAdapter::chunks(vec![text_response("late reply")]);
    let env = harness(adapter.clone()).await;
    let handle = create_handle(&env, "idle-disposal-wake").await;
    let agent = handle.agent.clone();

    let mut disposal = Box::pin(handle.dispose());
    let early = tokio::time::timeout(Duration::from_millis(1), &mut disposal)
        .await
        .ok();
    let _ = agent.followup(user_message("late wake"));
    agent.when_idle().await;
    match early {
        Some(done) => done.expect("disposal"),
        None => disposal.await.expect("disposal"),
    }

    assert_eq!(count(&agent, "turn/start"), 0);
    assert_eq!(adapter.request_count(), 0);
}

#[tokio::test]
async fn disposal_from_the_running_notification_drops_queued_work_before_turn_start() {
    let adapter = MockAdapter::chunks(vec![text_response("should not run")]);
    let env = harness(adapter.clone()).await;
    let handle = Arc::new(create_handle(&env, "dispose-running-session").await);
    let agent = handle.agent.clone();

    let disposal = Arc::new(Mutex::new(None));
    {
        let (slot, handle, subject) = (disposal.clone(), handle.clone(), agent.clone());
        env.ctx
            .on_contained::<AgentStatusChanged>(move |event| {
                if event.agent.id() == subject.id() && event.status == AgentStatus::Running {
                    let mut slot = lock(&slot);
                    if slot.is_none() {
                        // Disposal begins at its first poll (the source starts it synchronously).
                        let mut job = Box::pin(handle.dispose());
                        let mut prefix =
                            std::task::Context::from_waker(futures_util::task::noop_waker_ref());
                        let first = std::future::Future::poll(job.as_mut(), &mut prefix);
                        *slot = Some(tokio::spawn(async move {
                            match first {
                                std::task::Poll::Ready(done) => done,
                                std::task::Poll::Pending => job.await,
                            }
                        }));
                    }
                }
                Ok(())
            })
            .expect("listener");
    }
    send(&agent, "drop before claim");
    until(|| lock(&disposal).is_some()).await;
    let job = lock(&disposal).take().expect("disposal started");
    job.await.expect("join").expect("disposal");
    agent.when_idle().await;

    assert_eq!(agent.status(), AgentStatus::Idle);
    assert_eq!(count(&agent, "turn/start"), 0);
    assert!(user_texts(&agent).is_empty());
    assert_eq!(adapter.request_count(), 0);
}

// ---------------------------------------------------------------- pre-step cancellation

#[tokio::test]
async fn cancel_after_a_waking_send_closes_its_synchronously_opened_turn_without_a_step() {
    let adapter = MockAdapter::chunks(vec![text_response("should not run")]);
    let env = harness(adapter).await;
    let agent = create_agent(&env, "a1").await;

    send(&agent, "drop me first");
    send(&agent, "drop me second");
    user_cancel(&agent);
    settle(30).await;

    assert!(user_texts(&agent).is_empty());
    assert_eq!(count(&agent, "turn/start"), 1);
    assert_eq!(count(&agent, "step/start"), 0);
    assert_eq!(
        endings(&agent).last(),
        Some(&aborted(json!({"kind": "user"})))
    );
    assert_eq!(agent.status(), AgentStatus::Idle);
}

#[tokio::test]
async fn a_when_idle_waiter_registered_before_a_pre_step_cancel_resolves() {
    let adapter = MockAdapter::chunks(vec![text_response("x")]);
    let env = harness(adapter).await;
    let agent = create_agent(&env, "a1").await;

    // This waiter cannot rely on a running -> idle transition because cancellation drops the
    // turn before it runs; the skip path must settle it directly.
    send(&agent, "q");
    let idle = agent.when_idle();
    user_cancel(&agent);

    promptly(idle, "when_idle hung after pre-step cancel").await;
    assert_eq!(agent.status(), AgentStatus::Idle);
}

#[tokio::test]
async fn idle_listener_cancellation_settles_its_waiter_without_cancelling_later_work() {
    let adapter = MockAdapter::chunks(vec![
        text_response("first reply"),
        text_response("later reply"),
    ]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "idle-listener-cancel").await;

    let observation = Arc::new(Mutex::new(
        None::<tokio::task::JoinHandle<(AgentStatus, usize, usize)>>,
    ));
    {
        let (slot, subject, probe) = (observation.clone(), agent.clone(), adapter.clone());
        let fired = Arc::new(AtomicBool::new(false));
        env.ctx
            .on_contained::<AgentStatusChanged>(move |event| {
                // Never hold a lock across `send`: its synchronous prefix re-enters this listener.
                if event.agent.id() != subject.id()
                    || event.status != AgentStatus::Idle
                    || fired.swap(true, Ordering::SeqCst)
                {
                    return Ok(());
                }
                send(&subject, "cancelled replacement");
                let idle = subject.when_idle();
                let (subject_for_task, probe) = (subject.clone(), probe.clone());
                let task = tokio::spawn(async move {
                    idle.await;
                    (
                        subject_for_task.status(),
                        probe.request_count(),
                        events_of(&subject_for_task, "turn/start").len(),
                    )
                });
                user_cancel(&subject);
                *lock(&slot) = Some(task);
                Ok(())
            })
            .expect("listener");
    }

    send(&agent, "first");
    until(|| lock(&observation).is_some()).await;
    let job = lock(&observation).take().expect("observation");
    let seen = tokio::time::timeout(Duration::from_secs(1), job)
        .await
        .expect("when_idle hung after idle-listener cancel")
        .expect("join");
    assert_eq!(seen, (AgentStatus::Idle, 1, 2));

    send_and_wait(&agent, "later").await;
    assert_eq!(adapter.request_count(), 2);
    assert_eq!(user_texts(&agent), ["first", "later"]);
}

#[tokio::test]
async fn replacement_work_queued_after_idle_listener_cancellation_replays_at_convergence() {
    let adapter = MockAdapter::chunks(vec![
        text_response("first reply"),
        text_response("replacement reply"),
        text_response("wake reply"),
    ]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "idle-listener-post-cancel-send").await;

    let replacement = Arc::new(Mutex::new(
        None::<futures_util::future::BoxFuture<'static, ()>>,
    ));
    {
        let (slot, subject) = (replacement.clone(), agent.clone());
        let fired = Arc::new(AtomicBool::new(false));
        env.ctx
            .on_contained::<AgentStatusChanged>(move |event| {
                if event.agent.id() != subject.id()
                    || event.status != AgentStatus::Idle
                    || fired.swap(true, Ordering::SeqCst)
                {
                    return Ok(());
                }
                send(&subject, "cancelled replacement");
                user_cancel(&subject);
                send(&subject, "surviving replacement");
                *lock(&slot) = Some(subject.when_idle());
                Ok(())
            })
            .expect("listener");
    }

    send(&agent, "first");
    until(|| lock(&replacement).is_some()).await;
    let idle = lock(&replacement).take().expect("replacement idle");
    idle.await;

    // The wake sent after the cancel fired is latched: the surviving replacement runs at
    // convergence without a third message.
    assert_eq!(adapter.request_count(), 2);
    assert_eq!(user_texts(&agent), ["first", "surviving replacement"]);
    assert_eq!(pending_turns(&agent), 0);

    send_and_wait(&agent, "wake it").await;
    assert_eq!(adapter.request_count(), 3);
    assert_eq!(
        user_texts(&agent),
        ["first", "surviving replacement", "wake it"]
    );
}

// ---------------------------------------------------------------- mid-turn cancellation

#[tokio::test]
async fn cancel_mid_step_aborts_the_active_turn_and_drops_every_queued_tail_item() {
    let adapter = MockAdapter::new(vec![Entry::Hang]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "a1").await;

    send(&agent, "go");
    until(|| adapter.request_count() == 1).await;
    assert_eq!(agent.status(), AgentStatus::Running);
    send(&agent, "queued tail");
    user_cancel(&agent);
    agent.when_idle().await;

    assert_eq!(endings(&agent), [aborted(json!({"kind": "user"}))]);
    assert_eq!(user_texts(&agent), ["go"]);
    assert_eq!(count(&agent, "turn/start"), 1);
    assert_eq!(adapter.request_count(), 1);
}

#[tokio::test]
async fn cancel_from_an_assistant_message_observer_skips_execution_but_balances_replay() {
    let adapter = MockAdapter::chunks(vec![
        tool_call_response("c1", "danger", &json!({}), None),
        text_response("recovered after cancellation"),
    ]);
    let env = harness(adapter.clone()).await;
    let executions = Arc::new(AtomicUsize::new(0));
    let counter = executions.clone();
    register_tool(
        &env,
        content_tool("danger", json!({}), false, move |_, _| {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(vec![hoh_llm::ContentBlock::text("ran")])
            }
        }),
    );
    let agent = create_agent(&env, "cancel-after-assistant-message").await;
    let armed = Arc::new(AtomicBool::new(true));
    {
        let (session, victim, armed) = (agent.session().clone(), agent.clone(), armed.clone());
        env.ctx
            .on_contained::<SessionEventEmitted>(move |emitted| {
                if armed.load(Ordering::SeqCst)
                    && emitted.session == session
                    && emitted.event.event_type() == "assistant/message"
                {
                    user_cancel(&victim);
                }
                Ok(())
            })
            .expect("listener");
    }

    send_and_wait(&agent, "go").await;
    armed.store(false, Ordering::SeqCst);

    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(endings(&agent), [aborted(json!({"kind": "user"}))]);
    assert_eq!(call_ids(&agent, "tool/call"), ["c1"]);
    let result = events_of(&agent, "tool/result").remove(0);
    assert_eq!(result["data"]["message"]["role"], "tool");
    assert_eq!(result["data"]["message"]["source"]["callId"], "c1");
    assert_eq!(result["data"]["message"]["isError"], true);
    assert_eq!(
        result["data"]["error"],
        json!({"name": "AbortError", "code": hoh_tools::TOOL_ABORTED_BEFORE_DISPATCH})
    );

    send_and_wait(&agent, "continue safely").await;
    let second = lock(&adapter.requests)[1].messages.clone();
    let replayed = serde_json::to_value(&second).expect("messages");
    let tool = replayed
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("replayed tool result");
    assert_eq!(tool["toolCallId"], "c1");
    assert_eq!(tool["isError"], true);
    assert_eq!(
        endings(&agent),
        [aborted(json!({"kind": "user"})), completed()]
    );
}

#[tokio::test]
async fn cancel_mid_tool_call_commits_the_started_call_and_aborts_the_turn() {
    let adapter = MockAdapter::chunks(vec![
        multi_call(&[("c1", "slow", json!({"id": "1"}))]),
        text_response("after"),
    ]);
    let env = harness(adapter.clone()).await;
    let observed = Arc::new(AtomicBool::new(false));
    let flag = observed.clone();
    register_tool(
        &env,
        content_tool("slow", id_param(), false, move |_, exec| {
            let flag = flag.clone();
            async move {
                // The body observes the turn's abort signal and finishes cooperatively.
                exec.signal().cancelled().await;
                flag.store(true, Ordering::SeqCst);
                Ok(vec![hoh_llm::ContentBlock::text("interrupted")])
            }
        }),
    );
    let agent = create_agent(&env, "cancel-mid-tool").await;

    send(&agent, "go");
    until(|| count(&agent, "tool/call") == 1).await;
    settle(10).await;
    user_cancel(&agent);
    agent.when_idle().await;

    assert!(
        observed.load(Ordering::SeqCst),
        "the tool body never saw the abort"
    );
    assert_eq!(count(&agent, "tool/result"), 1);
    assert_eq!(endings(&agent), [aborted(json!({"kind": "user"}))]);
    assert_eq!(adapter.request_count(), 1);
    // The paired history lets the next prompt continue.
    send_and_wait(&agent, "again").await;
    assert_eq!(endings(&agent).last(), Some(&completed()));
}

#[tokio::test]
async fn a_prompt_sent_after_a_cancelled_turn_runs_normally() {
    let adapter = MockAdapter::new(vec![
        Entry::Hang,
        Entry::Chunks(text_response("second reply")),
    ]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "a1").await;

    send(&agent, "first");
    until(|| adapter.request_count() == 1).await;
    user_cancel(&agent);
    agent.when_idle().await;

    // The marker must have been reset after the cancelled turn: a fresh prompt runs to
    // completion rather than being dropped by a stale marker.
    send_and_wait(&agent, "second").await;

    assert!(user_texts(&agent).contains(&"second".to_owned()));
    assert_eq!(count(&agent, "turn/end"), 2);
}

#[tokio::test]
async fn cancel_mid_stream_finalizes_the_streamed_prefix() {
    let adapter = MockAdapter::new(vec![Entry::Hang, Entry::Chunks(text_response("after"))]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "partial-finalize").await;

    send(&agent, "go");
    until(|| adapter.request_count() == 1).await;
    settle(30).await;
    user_cancel(&agent);
    agent.when_idle().await;

    let message = events_of(&agent, "assistant/message").remove(0);
    assert_eq!(
        message["data"]["message"]["content"],
        json!([{"type": "text", "text": "partial"}])
    );
}

fn hang_after(chunks: Vec<StreamChunk>) -> Entry {
    Entry::HangAfter(chunks)
}

#[tokio::test]
async fn cancel_drops_a_half_streamed_tool_call_and_keeps_the_completed_text_before_it() {
    let adapter = MockAdapter::new(vec![hang_after(vec![
        chunk(json!({"type":"block-start","index":0,"blockType":"text"})),
        chunk(json!({"type":"text-delta","index":0,"text":"reading the file"})),
        chunk(
            json!({"type":"block-end","index":0,"block":{"type":"text","text":"reading the file"}}),
        ),
        chunk(json!({"type":"block-start","index":1,"blockType":"tool-call"})),
        chunk(
            json!({"type":"tool-call-delta","index":1,"id":"c1","name":"read","argumentsDelta":"{\"pa"}),
        ),
    ])]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "tool-call-drop").await;

    send(&agent, "go");
    until(|| adapter.request_count() == 1).await;
    settle(30).await;
    user_cancel(&agent);
    agent.when_idle().await;

    // The undispatched call is dropped whole: no dangling tool_use to pair.
    let message = events_of(&agent, "assistant/message").remove(0);
    assert_eq!(
        message["data"]["message"]["content"],
        json!([{"type": "text", "text": "reading the file"}])
    );
    assert_eq!(count(&agent, "tool/call"), 0);
}

#[tokio::test]
async fn cancel_before_any_visible_content_finalizes_nothing() {
    let adapter = MockAdapter::new(vec![hang_after(vec![
        chunk(json!({"type":"block-start","index":0,"blockType":"tool-call"})),
        chunk(
            json!({"type":"tool-call-delta","index":0,"id":"c1","name":"read","argumentsDelta":"{\"pa"}),
        ),
    ])]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "nothing-to-finalize").await;
    let frames = Arc::new(Mutex::new(Vec::new()));
    let sink = frames.clone();
    let id = agent.id().clone();
    env.ctx
        .on_contained::<AgentAssistantStream>(move |event| {
            if event.agent.id() == &id {
                lock(&sink).push(event.frame.clone());
            }
            Ok(())
        })
        .expect("listener");

    send(&agent, "go");
    until(|| adapter.request_count() == 1).await;
    settle(30).await;
    user_cancel(&agent);
    agent.when_idle().await;

    assert_eq!(count(&agent, "assistant/message"), 0);
    let attempt = last_event(&agent, "assistant/attempt").expect("durable empty attempt");
    match lock(&frames).last() {
        Some(AssistantStreamFrame::End { outcome, .. }) => match outcome {
            AssistantStreamOutcome::Committed { event_type, seq } => {
                assert_eq!(*event_type, "assistant/attempt");
                assert_eq!(json!(seq), attempt["seq"]);
            }
            AssistantStreamOutcome::Abandoned => panic!("dsh commits the empty attempt"),
        },
        frame => panic!("expected terminal frame, got {frame:?}"),
    }
}

// ---------------------------------------------------------------- synchronous listeners

#[tokio::test]
async fn cancel_from_a_synchronous_step_start_listener_drops_the_step() {
    let adapter = MockAdapter::chunks(vec![text_response("should not stream")]);
    let env = harness(adapter).await;
    let agent = create_agent(&env, "a1").await;

    // A step/start session-event listener fires AFTER step/start is appended, so cancelling
    // there lands in the second cancel check (the one that must close the already-open step).
    let streamed = Arc::new(AtomicBool::new(false));
    {
        let flag = streamed.clone();
        env.ctx
            .on_contained::<AgentAssistantStream>(move |event| {
                if matches!(event.frame, hoh_agent::AssistantStreamFrame::Chunk { .. }) {
                    flag.store(true, Ordering::SeqCst);
                }
                Ok(())
            })
            .expect("listener");
    }
    {
        let (session, victim) = (agent.session().clone(), agent.clone());
        env.ctx
            .on_contained::<SessionEventEmitted>(move |emitted| {
                if emitted.session == session && emitted.event.event_type() == "step/start" {
                    user_cancel(&victim);
                }
                Ok(())
            })
            .expect("listener");
    }

    send_and_wait(&agent, "go").await;

    assert!(!streamed.load(Ordering::SeqCst));
    assert_eq!(endings(&agent), [aborted(json!({"kind": "user"}))]);
    assert_eq!(count(&agent, "step/start"), count(&agent, "step/end"));
}

#[tokio::test]
async fn cancel_during_the_stopping_window_ends_the_turn_aborted_and_runs_no_further_step() {
    let adapter = MockAdapter::chunks(vec![text_response("one"), text_response("two")]);
    let env = harness(adapter).await;
    let agent = create_agent(&env, "a1").await;

    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let (subject, cancelled) = (agent.clone(), cancelled.clone());
        env.ctx
            .on_serial::<AgentTurnStopping, (), _>(move |event| {
                let (subject, cancelled) = (subject.clone(), cancelled.clone());
                let same = event.agent.id() == subject.id();
                async move {
                    if same && !cancelled.swap(true, Ordering::SeqCst) {
                        user_cancel(&subject);
                    }
                    Ok(None)
                }
            })
            .expect("listener");
    }

    send_and_wait(&agent, "go").await;

    // Only one step ran, and the shared turn signal classified the durable outcome as aborted.
    assert_eq!(count(&agent, "step/start"), 1);
    assert_eq!(endings(&agent), [aborted(json!({"kind": "user"}))]);
}

#[tokio::test]
async fn cancel_from_a_synchronous_running_listener_drops_the_turn() {
    let adapter = MockAdapter::chunks(vec![text_response("should not run")]);
    let env = harness(adapter).await;
    let agent = create_agent(&env, "a1").await;

    // `agent/status` is synchronous, so cancellation can land before the durable turn-start
    // commit and must drop the reserved work.
    let streamed = Arc::new(AtomicBool::new(false));
    {
        let flag = streamed.clone();
        env.ctx
            .on_contained::<AgentAssistantStream>(move |event| {
                if matches!(event.frame, hoh_agent::AssistantStreamFrame::Chunk { .. }) {
                    flag.store(true, Ordering::SeqCst);
                }
                Ok(())
            })
            .expect("listener");
    }
    let armed = Arc::new(AtomicBool::new(true));
    {
        let (subject, armed) = (agent.clone(), armed.clone());
        env.ctx
            .on_contained::<AgentStatusChanged>(move |event| {
                if armed.load(Ordering::SeqCst)
                    && event.agent.id() == subject.id()
                    && event.status == AgentStatus::Running
                {
                    user_cancel(&subject);
                }
                Ok(())
            })
            .expect("listener");
    }

    send_and_wait(&agent, "go").await;
    armed.store(false, Ordering::SeqCst);

    assert!(!streamed.load(Ordering::SeqCst));
    assert_eq!(count(&agent, "turn/start"), 0);
}

#[tokio::test]
async fn a_running_listener_cancellation_replays_replacement_work_at_convergence() {
    let adapter = MockAdapter::chunks(vec![text_response("A reply"), text_response("B reply")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "a1").await;

    let replaced = Arc::new(AtomicBool::new(false));
    {
        let (subject, replaced) = (agent.clone(), replaced.clone());
        env.ctx
            .on_contained::<AgentStatusChanged>(move |event| {
                if event.agent.id() == subject.id()
                    && event.status == AgentStatus::Running
                    && !replaced.swap(true, Ordering::SeqCst)
                {
                    user_cancel(&subject);
                    send(&subject, "B");
                }
                Ok(())
            })
            .expect("listener");
    }

    send_and_wait(&agent, "A").await;

    // B's wake was latched behind the cancelled driver: it runs on its own.
    assert_eq!(user_texts(&agent), ["B"]);
    assert_eq!(pending_turns(&agent), 0);
    assert_eq!(adapter.request_count(), 1);

    send_and_wait(&agent, "C").await;
    assert_eq!(user_texts(&agent), ["B", "C"]);
    assert_eq!(adapter.request_count(), 2);
    assert_eq!(count(&agent, "turn/end"), 2);
}

#[tokio::test]
async fn a_prompt_queued_during_pre_step_cancellation_replays_at_convergence() {
    let adapter = MockAdapter::chunks(vec![text_response("A reply"), text_response("B reply")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "a1").await;

    send(&agent, "A");
    let idle = agent.when_idle();
    user_cancel(&agent);
    send(&agent, "B");
    idle.await;

    assert_eq!(user_texts(&agent), ["B"]);
    assert_eq!(pending_turns(&agent), 0);
    assert_eq!(adapter.request_count(), 1);

    send_and_wait(&agent, "C").await;
    assert_eq!(user_texts(&agent), ["B", "C"]);
    assert_eq!(adapter.request_count(), 2);
    assert_eq!(count(&agent, "turn/end"), 3);
}

#[tokio::test]
async fn cancel_clears_the_turns_steering_instead_of_re_enqueueing_it() {
    let adapter = MockAdapter::new(vec![Entry::Hang]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "a1").await;

    send(&agent, "go");
    until(|| adapter.request_count() == 1).await;
    assert_eq!(agent.status(), AgentStatus::Running);
    // Steer joins the running turn's steering FIFO; cancel must drop it, not re-enqueue it as a
    // new queued turn.
    agent.steer(user_message("steer text")).expect("steer");
    user_cancel(&agent);
    agent.when_idle().await;

    settle(30).await;
    assert_eq!(agent.status(), AgentStatus::Idle);
    assert_eq!(count(&agent, "turn/start"), 1);
    assert!(!user_texts(&agent).contains(&"steer text".to_owned()));
}

#[tokio::test]
async fn replays_replacement_work_queued_synchronously_by_an_abort_observer() {
    let adapter = MockAdapter::new(vec![
        Entry::Hang,
        Entry::Chunks(text_response("replacement reply")),
        Entry::Chunks(text_response("wake reply")),
    ]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "abort-observer-replacement").await;

    send(&agent, "original");
    until(|| adapter.request_count() == 1).await;
    let signal = lock(&adapter.requests)[0]
        .signal
        .clone()
        .expect("model request omitted its turn signal");
    {
        let observer = agent.clone();
        let sent = Arc::new(AtomicBool::new(false));
        let watched = signal.clone();
        tokio::spawn(async move {
            watched.cancelled().await;
            if !sent.swap(true, Ordering::SeqCst) {
                send(&observer, "replacement");
            }
        });
    }
    let idle = agent.when_idle();
    user_cancel(&agent);
    promptly(idle, "replacement did not settle").await;
    agent.when_idle().await;

    // The abort-observer wake was latched: replacement runs at convergence, so the original
    // turn is followed by a completed replacement turn.
    assert_eq!(adapter.request_count(), 2);
    assert_eq!(user_texts(&agent), ["original", "replacement"]);
    assert_eq!(pending_turns(&agent), 0);
    assert_eq!(
        endings(&agent),
        [aborted(json!({"kind": "user"})), completed()]
    );

    send_and_wait(&agent, "wake it").await;
    assert_eq!(adapter.request_count(), 3);
    assert_eq!(user_texts(&agent), ["original", "replacement", "wake it"]);
}

#[tokio::test]
async fn preserves_the_first_user_cancellation_when_lifecycle_teardown_races_it() {
    let adapter = MockAdapter::new(vec![Entry::Hang]);
    let env = harness(adapter.clone()).await;
    let handle = create_handle(&env, "cancel-dispose-race").await;
    let agent = handle.agent.clone();

    send(&agent, "go");
    until(|| adapter.request_count() == 1).await;
    user_cancel(&agent);
    handle.dispose().await.expect("disposal");

    assert_eq!(
        endings(&agent).last(),
        Some(&aborted(json!({"kind": "user"})))
    );
}

// ---------------------------------------------------------------- review rv-agentloop regressions

/// Two disposals of one handle on a single-threaded runtime: the second must join the first
/// instead of waiting on a lock held across the first's `.await` (hang guarded by the external
/// `timeout` around the test run).
#[tokio::test]
async fn two_concurrent_handle_disposals_both_complete_on_a_current_thread_runtime() {
    let env = harness(MockAdapter::chunks(vec![])).await;
    let handle = create_handle(&env, "double-dispose").await;
    let (first, second) = tokio::join!(handle.dispose(), handle.dispose());
    first.expect("first disposal");
    second.expect("second disposal");
    env.close().await;
}

/// A followup racing a cancel that clears the inbox: whatever the interleaving, the agent never
/// settles idle with the followup still pending (it was either cleared by the cancel or runs).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_followup_racing_a_cancel_never_leaves_pending_work_on_an_idle_agent() {
    for round in 0..60 {
        let adapter = MockAdapter::new(vec![
            Entry::Hang,
            Entry::Chunks(text_response("reply")),
            Entry::Chunks(text_response("reply")),
        ]);
        let env = harness(adapter.clone()).await;
        let agent = create_agent(&env, &format!("race-{round}")).await;
        send(&agent, "active");
        until(|| adapter.request_count() == 1).await;

        let gate = Arc::new(tokio::sync::Barrier::new(2));
        let canceller = {
            let (agent, gate) = (agent.clone(), gate.clone());
            tokio::spawn(async move {
                gate.wait().await;
                user_cancel(&agent);
            })
        };
        let sender = {
            let (agent, gate) = (agent.clone(), gate.clone());
            tokio::spawn(async move {
                gate.wait().await;
                send(&agent, "racing");
            })
        };
        canceller.await.expect("cancel task");
        sender.await.expect("send task");
        promptly(agent.when_idle(), "the agent must settle idle").await;

        assert_eq!(
            pending_turns(&agent),
            0,
            "round {round}: idle with a pending followup"
        );
        env.close().await;
    }
}
