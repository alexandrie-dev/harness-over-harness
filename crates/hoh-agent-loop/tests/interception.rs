//! Port of interception.spec.ts: the interception points `agent/pre-step`, `agent/created`,
//! `agent/turn-stopping` and the `tools/pre-execute` / `tools/post-execute` split with
//! `additionalContexts` buffering, driven the way a native plugin programs against them.
//!
//! Not portable as such (see the per-case notes): `Object.isFrozen` and the `TypeError` on a
//! mutated frozen object have no Rust counterpart because every value handed to a listener is
//! immutable by ownership; `vi.spyOn(session, 'append')` has no counterpart because `Session` has
//! no seam to make one append fail.
mod common;

use common::*;
use hoh_agent::{
    Agent, AgentCreated, AgentError, AgentPreStep, AgentStatus, AgentStatusChanged,
    AgentTurnStopping, Agents, PreStepDecision,
};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, NoConfig, Plugin};
use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, UserMessage, create_user_message};
use hoh_session::{SessionEventEmitted, Sessions};
use hoh_tools::{PostToolDecision, PreToolDecision, ToolsPostExecute, ToolsPreExecute};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Notify, oneshot, watch};

fn message_of(source: &str, text: &str) -> UserMessage {
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new(source),
    })
}

fn text_of(message: &UserMessage) -> String {
    message
        .content
        .iter()
        .map(|block| {
            if block.r#type == "text" {
                block.text_value().unwrap_or_default()
            } else {
                ""
            }
        })
        .collect()
}

fn text_of_all(messages: &[UserMessage]) -> String {
    messages.iter().map(text_of).collect()
}

/// `messages.map(message => message.content[0])` as JSON blocks.
fn first_blocks(messages: &[UserMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| serde_json::to_value(&message.content[0]).expect("block"))
        .collect()
}

fn text_block(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

fn send(agent: &Arc<dyn Agent>, text: &str) {
    agent.followup(user_message(text)).expect("followup");
}

/// `waitForIdle(ctx, agent)`: armed before the work is sent, resolves on the next `idle` status.
struct Idle(watch::Receiver<bool>);

impl Idle {
    fn arm(env: &Env, agent: &Arc<dyn Agent>) -> Self {
        let (sender, receiver) = watch::channel(false);
        let subject = agent.clone();
        env.ctx
            .on_contained::<AgentStatusChanged>(move |event| {
                if event.agent.id() == subject.id() && event.status == AgentStatus::Idle {
                    let _ = sender.send(true);
                }
                Ok(())
            })
            .expect("status listener");
        Self(receiver)
    }
    async fn wait(mut self) {
        self.0.wait_for(|idle| *idle).await.expect("idle signal");
    }
}

/// A one-shot decision a pre-step listener waits for (`Promise.withResolvers`).
struct Held {
    entered: Arc<Notify>,
    receiver: Arc<Mutex<Option<oneshot::Receiver<PreStepDecision>>>>,
    sender: Mutex<Option<oneshot::Sender<PreStepDecision>>>,
}

impl Held {
    fn new() -> Self {
        let (sender, receiver) = oneshot::channel();
        Self {
            entered: Arc::new(Notify::new()),
            receiver: Arc::new(Mutex::new(Some(receiver))),
            sender: Mutex::new(Some(sender)),
        }
    }
    /// Signal entry and wait for the decision (only the first call waits).
    async fn hold(
        entered: Arc<Notify>,
        receiver: Arc<Mutex<Option<oneshot::Receiver<PreStepDecision>>>>,
    ) -> Result<Option<PreStepDecision>, BoxError> {
        entered.notify_one();
        let receiver = lock(&receiver).take();
        match receiver {
            Some(receiver) => Ok(Some(receiver.await?)),
            None => Ok(None),
        }
    }
    fn resolve(&self, decision: PreStepDecision) {
        let sender = lock(&self.sender).take().expect("decision resolved once");
        let _ = sender.send(decision);
    }
}

fn enter(messages: Vec<UserMessage>) -> PreStepDecision {
    PreStepDecision::Enter {
        messages,
        starts_request_series: false,
    }
}

fn events_json(agent: &Arc<dyn Agent>) -> Vec<Value> {
    agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .map(|event| event.as_value().clone())
        .collect()
}

fn user_message_events(agent: &Arc<dyn Agent>) -> Vec<Value> {
    events_of(agent, "user/message")
}

fn sourced_by(event: &Value, kind: &str) -> bool {
    event["data"]["source"]["kind"] == kind
}

fn turn_markers(agent: &Arc<dyn Agent>) -> Vec<String> {
    event_types(agent)
        .into_iter()
        .filter(|kind| kind == "turn/start" || kind == "turn/end")
        .collect()
}

fn request_text(env: &Env, index: usize) -> String {
    serde_json::to_value(&lock(&env.adapter.requests)[index].messages)
        .expect("messages")
        .to_string()
}

/// `ctx.on('session/event', …)` collecting every `turn/end` reason.
fn collect_turn_end_reasons(env: &Env) -> Arc<Mutex<Vec<Value>>> {
    let reasons = Arc::new(Mutex::new(Vec::new()));
    let sink = reasons.clone();
    env.ctx
        .on_contained::<SessionEventEmitted>(move |emitted| {
            if emitted.event.event_type() == "turn/end" {
                lock(&sink).push(emitted.event.data()["reason"].clone());
            }
            Ok(())
        })
        .expect("session/event listener");
    reasons
}

// ------------------------------------------------------------------------------ agent/pre-step

#[tokio::test]
async fn enter_default_via_next_records_the_user_message_unchanged() {
    let env = harness(MockAdapter::chunks(vec![text_response("ok")])).await;
    let agent = create_agent(&env, "a1").await;
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = seen.clone();
    env.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
            lock(&sink).push(text_of(&event.messages[0]));
            next.run()
        })
        .expect("pre-step listener");

    send_and_wait(&agent, "hello").await;

    assert_eq!(*lock(&seen), ["hello"]);
    let recorded = user_message_events(&agent);
    assert_eq!(recorded[0]["data"]["content"], json!([text_block("hello")]));
    env.close().await;
}

#[tokio::test]
async fn reports_the_request_coordinates_for_initial_and_tool_continuation_prompts() {
    let adapter = MockAdapter::chunks(vec![
        tool_call_response("c1", "echo", &json!({"text": "hi"}), None),
        text_response("done"),
    ]);
    let env = harness(adapter).await;
    register_tool(
        &env,
        content_tool(
            "echo",
            json!({"text": {"type": "string", "required": true}}),
            false,
            |args, _| async move {
                Ok(vec![ContentBlock::text(
                    args["text"].as_str().unwrap_or_default(),
                )])
            },
        ),
    );
    let agent = create_agent(&env, "prompt-coordinates").await;
    let seen = Arc::new(Mutex::new(Vec::<(u64, u64, usize)>::new()));
    let sink = seen.clone();
    env.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
            lock(&sink).push((event.turn, event.step, event.messages.len()));
            next.run()
        })
        .expect("pre-step listener");

    send_and_wait(&agent, "hello").await;

    assert_eq!(*lock(&seen), [(1, 1, 1), (1, 2, 0)]);
    env.close().await;
}

/// `Object.isFrozen(message)` and the `TypeError` on mutating a published or accepted message have
/// no Rust counterpart: a published message is an owned immutable value. What is kept is the
/// observable half: the listener sees the accepted content and source, the decision's message is
/// the one recorded, and the recorded event carries exactly the input's fields.
#[tokio::test]
async fn publishes_input_without_replacing_its_identity() {
    let env = harness(MockAdapter::chunks(vec![text_response("ok")])).await;
    let agent = create_agent(&env, "owned-input").await;
    let held = Held::new();
    let observed = Arc::new(Mutex::new(Vec::<UserMessage>::new()));
    {
        let (entered, receiver, sink, subject) = (
            held.entered.clone(),
            held.receiver.clone(),
            observed.clone(),
            agent.clone(),
        );
        env.ctx
            .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, _next| {
                let (entered, receiver, sink, subject) = (
                    entered.clone(),
                    receiver.clone(),
                    sink.clone(),
                    subject.clone(),
                );
                async move {
                    if event.agent.id() != subject.id() {
                        return Ok(enter(event.messages.clone()));
                    }
                    lock(&sink).push(event.messages[0].clone());
                    Ok(Held::hold(entered, receiver)
                        .await?
                        .unwrap_or_else(|| enter(event.messages.clone())))
                }
            })
            .expect("pre-step listener");
    }
    let input = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text("accepted text")],
        source: MessageSource::new("accepted source"),
    });

    let idle = Idle::arm(&env, &agent);
    agent.followup(input.clone()).expect("followup");
    held.entered.notified().await;
    held.resolve(enter(vec![input.clone()]));
    idle.wait().await;

    let observed_messages = lock(&observed).clone();
    assert_eq!(observed_messages.len(), 1);
    assert_eq!(
        observed_messages[0].content,
        vec![ContentBlock::text("accepted text")]
    );
    assert_eq!(
        observed_messages[0].source,
        MessageSource::new("accepted source")
    );
    let recorded = &user_message_events(&agent)[0]["data"];
    assert_eq!(recorded["content"], json!([text_block("accepted text")]));
    assert_eq!(recorded["source"], json!({"kind": "accepted source"}));
    assert_eq!(recorded["id"], json!(input.id.to_string()));
    env.close().await;
}

#[tokio::test]
async fn enter_with_content_rewrites_the_prompt_before_it_is_recorded() {
    let adapter = MockAdapter::chunks(vec![text_response("ok")]);
    let env = harness(adapter).await;
    let agent = create_agent(&env, "a1").await;
    env.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, _next| {
            let mut rewritten = event.messages[0].clone();
            rewritten.content = vec![ContentBlock::text("REWRITTEN")];
            async move { Ok(enter(vec![rewritten])) }
        })
        .expect("pre-step listener");

    send_and_wait(&agent, "original").await;

    let recorded = user_message_events(&agent);
    assert_eq!(
        recorded[0]["data"]["content"],
        json!([text_block("REWRITTEN")])
    );
    // the rewritten prompt is what reached the model
    assert!(request_text(&env, 0).contains("REWRITTEN"));
    assert!(!request_text(&env, 0).contains("original"));
    env.close().await;
}

#[tokio::test]
async fn enter_with_additional_messages_records_separately_sourced_context_in_the_turn() {
    let adapter = MockAdapter::chunks(vec![text_response("ok")]);
    let env = harness(adapter).await;
    let agent = create_agent(&env, "a1").await;
    env.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, _next| {
            let mut messages = event.messages.clone();
            messages.push(message_of(
                "test",
                "<system-reminder>extra ctx</system-reminder>",
            ));
            async move { Ok(enter(messages)) }
        })
        .expect("pre-step listener");

    send_and_wait(&agent, "go").await;

    let recorded = user_message_events(&agent);
    assert!(recorded.iter().any(|event| sourced_by(event, "user")));
    let context = recorded
        .iter()
        .find(|event| !sourced_by(event, "user"))
        .expect("context message recorded");
    assert_eq!(
        context["data"]["content"],
        json!([text_block("<system-reminder>extra ctx</system-reminder>")])
    );
    assert_eq!(context["data"]["source"], json!({"kind": "test"}));
    assert!(request_text(&env, 0).contains("extra ctx"));
    env.close().await;
}

#[tokio::test]
async fn does_not_open_another_step_when_a_completed_turn_rewrites_pending_input_to_empty() {
    let adapter = MockAdapter::chunks(vec![text_response("done")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "empty-completed-continuation").await;
    env.ctx
        .on_serial::<AgentTurnStopping, (), _>(move |event| {
            let subject = event.agent.clone();
            async move {
                subject.inject(message_of("test", "pending context"))?;
                Ok(None)
            }
        })
        .expect("turn-stopping listener");
    env.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| async move {
            let decision = next.run().await?;
            Ok(
                if event.step == 1 || matches!(decision, PreStepDecision::Reject) {
                    decision
                } else {
                    enter(vec![])
                },
            )
        })
        .expect("pre-step listener");

    send(&agent, "finish once");
    agent.when_idle().await;

    assert_eq!(adapter.request_count(), 1);
    assert_eq!(
        event_types(&agent)
            .iter()
            .filter(|kind| *kind == "step/start")
            .count(),
        1
    );
    env.close().await;
}

#[tokio::test]
async fn reject_closes_the_claimed_prompt_turn_without_a_step_or_model_call() {
    let adapter = MockAdapter::chunks(vec![text_response("should not run")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "a1").await;
    env.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(|_, _next| async {
            Ok(PreStepDecision::Reject)
        })
        .expect("pre-step listener");
    let reasons = collect_turn_end_reasons(&env);

    send(&agent, "do something");
    agent.when_idle().await;

    // the model was never called
    assert_eq!(adapter.request_count(), 0);
    assert_eq!(turn_markers(&agent), ["turn/start", "turn/end"]);
    let types = event_types(&agent);
    assert!(!types.iter().any(|kind| kind == "user/message"));
    assert!(!types.iter().any(|kind| kind == "step/start"));
    assert_eq!(*lock(&reasons), [json!({"kind": "blocked"})]);
    env.close().await;
}

#[tokio::test]
async fn stages_inject_and_steer_during_pre_step_for_the_entered_turn() {
    let adapter = MockAdapter::chunks(vec![text_response("ok")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "pre-step-outbox").await;
    let held = Held::new();
    let claimed = Arc::new(Mutex::new(Vec::<UserMessage>::new()));
    let first_proposal = Arc::new(AtomicBool::new(true));
    {
        let (entered, receiver, claimed, first) = (
            held.entered.clone(),
            held.receiver.clone(),
            claimed.clone(),
            first_proposal.clone(),
        );
        env.ctx
            .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, _next| {
                let (entered, receiver, claimed, first) = (
                    entered.clone(),
                    receiver.clone(),
                    claimed.clone(),
                    first.clone(),
                );
                async move {
                    if !first.swap(false, Ordering::SeqCst) {
                        return Ok(enter(event.messages.clone()));
                    }
                    *lock(&claimed) = event.messages.clone();
                    Ok(Held::hold(entered, receiver)
                        .await?
                        .unwrap_or_else(|| enter(event.messages.clone())))
                }
            })
            .expect("pre-step listener");
    }

    let idle = Idle::arm(&env, &agent);
    send(&agent, "entered prompt");
    held.entered.notified().await;
    assert_eq!(agent.status(), AgentStatus::Running);
    assert!(event_types(&agent).iter().any(|kind| kind == "turn/start"));

    agent
        .inject(message_of("test", "attached context"))
        .expect("inject");
    agent
        .steer(message_of("user", "pre-step steering"))
        .expect("steer");
    assert!(
        !event_types(&agent)
            .iter()
            .any(|kind| kind == "user/message")
    );
    assert_eq!(
        first_blocks(&agent.inbox().next_step()),
        [
            text_block("attached context"),
            text_block("pre-step steering")
        ]
    );

    held.resolve(enter(lock(&claimed).clone()));
    idle.wait().await;
    assert!(agent.inbox().next_turn().is_empty());
    assert!(agent.inbox().next_step().is_empty());

    let staged: Vec<Value> = events_json(&agent)
        .into_iter()
        .filter(|event| event["type"] == "turn/start" || event["type"] == "user/message")
        .collect();
    assert_eq!(
        staged
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["turn/start", "user/message", "user/message", "user/message"]
    );
    assert_eq!(
        staged[1]["data"]["content"],
        json!([text_block("entered prompt")])
    );
    assert_eq!(
        staged[2]["data"]["content"],
        json!([text_block("attached context")])
    );
    assert_eq!(
        staged[3]["data"]["content"],
        json!([text_block("pre-step steering")])
    );
    let first_request = request_text(&env, 0);
    assert!(first_request.contains("entered prompt"));
    assert!(!first_request.contains("attached context"));
    assert!(!first_request.contains("pre-step steering"));
    let next_request = request_text(&env, 1);
    assert!(next_request.contains("attached context"));
    assert!(next_request.contains("pre-step steering"));
    env.close().await;
}

#[tokio::test]
async fn preserves_input_staged_after_the_blocked_batch_was_claimed() {
    let adapter = MockAdapter::chunks(vec![text_response("retried")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "blocked-pre-step-outbox").await;
    let held = Held::new();
    let dispose_block = {
        let (entered, receiver) = (held.entered.clone(), held.receiver.clone());
        env.ctx
            .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |_, _next| {
                let (entered, receiver) = (entered.clone(), receiver.clone());
                async move {
                    Held::hold(entered, receiver)
                        .await?
                        .ok_or_else(|| "blocked pre-step listener entered twice".into())
                }
            })
            .expect("blocking listener")
    };

    let blocked_idle = Idle::arm(&env, &agent);
    send(&agent, "blocked prompt");
    held.entered.notified().await;
    agent
        .inject(message_of("test", "staged context"))
        .expect("inject");
    agent
        .steer(message_of("user", "staged steering"))
        .expect("steer");
    held.resolve(PreStepDecision::Reject);
    blocked_idle.wait().await;

    assert_eq!(
        first_blocks(&agent.inbox().next_step()),
        [text_block("staged context"), text_block("staged steering")]
    );
    assert_eq!(turn_markers(&agent), ["turn/start", "turn/end"]);
    assert_eq!(adapter.request_count(), 0);

    dispose_block.dispose().await;
    send_and_wait(&agent, "resume").await;

    let staged = user_message_events(&agent);
    assert_eq!(staged.len(), 3);
    assert!(!request_text(&env, 0).contains("blocked prompt"));
    assert!(request_text(&env, 0).contains("staged context"));
    assert!(request_text(&env, 0).contains("staged steering"));
    env.close().await;
}

#[tokio::test]
async fn preserves_later_queued_work_when_a_step_is_rejected() {
    let adapter = MockAdapter::chunks(vec![
        text_response("continued"),
        text_response("wake reply"),
    ]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "rejected-pre-step-order").await;
    env.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| async move {
            let decision = next.run().await?;
            Ok(if text_of_all(&event.messages).contains("blocked prompt") {
                PreStepDecision::Reject
            } else {
                decision
            })
        })
        .expect("rejecting listener");
    env.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| async move {
            if text_of_all(&event.messages).contains("blocked prompt") {
                event
                    .agent
                    .inject(message_of("test", "earlier state change"))?;
                event.agent.steer(message_of("user", "earlier steering"))?;
            }
            next.run().await
        })
        .expect("staging listener");

    let idle = Idle::arm(&env, &agent);
    send(&agent, "blocked prompt");
    send(&agent, "later prompt");
    idle.wait().await;

    assert_eq!(turn_markers(&agent), ["turn/start", "turn/end"]);
    assert_eq!(
        first_blocks(&agent.inbox().next_step()),
        [
            text_block("earlier state change"),
            text_block("earlier steering")
        ]
    );
    assert_eq!(
        first_blocks(&agent.inbox().next_turn()),
        [text_block("later prompt")]
    );
    assert_eq!(adapter.request_count(), 0);

    send_and_wait(&agent, "wake").await;
    let request = request_text(&env, 0);
    assert!(request.contains("earlier state change"));
    assert!(request.contains("earlier steering"));
    assert!(request.contains("later prompt"));
    assert!(!request.contains("blocked prompt"));
    env.close().await;
}

#[tokio::test]
async fn preserves_context_only_injection_staged_after_pre_step_began() {
    let adapter = MockAdapter::chunks(vec![text_response("continued")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "rejected-pre-step-context").await;
    let held = Held::new();
    let dispose_block = {
        let (entered, receiver) = (held.entered.clone(), held.receiver.clone());
        env.ctx
            .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |_, _next| {
                let (entered, receiver) = (entered.clone(), receiver.clone());
                async move {
                    Held::hold(entered, receiver)
                        .await?
                        .ok_or_else(|| "blocking pre-step listener entered twice".into())
                }
            })
            .expect("blocking listener")
    };

    let idle = Idle::arm(&env, &agent);
    send(&agent, "blocked prompt");
    held.entered.notified().await;
    agent
        .inject(message_of("test", "independent context"))
        .expect("inject");
    held.resolve(PreStepDecision::Reject);
    idle.wait().await;

    assert!(user_message_events(&agent).is_empty());
    assert_eq!(
        first_blocks(&agent.inbox().next_step()),
        [text_block("independent context")]
    );
    assert_eq!(adapter.request_count(), 0);

    dispose_block.dispose().await;
    send_and_wait(&agent, "wake").await;
    assert!(request_text(&env, 0).contains("independent context"));
    assert!(!request_text(&env, 0).contains("blocked prompt"));
    env.close().await;
}

/// `vi.spyOn(agent.session, 'append').mockImplementationOnce(() => { throw … })` has no Rust
/// counterpart: `Session` offers no seam to make the next append fail with a chosen error. The
/// durable-append failure that the source simulates is produced here by the one failure a real
/// `Session` raises on its own: an append attempted while another append of the same session is
/// still being published ("cannot reenter"). The follow-up is sent from inside the listener of a
/// probe event, so its inbox append fails; the error must reach the caller and leave the log
/// (only the probe), both inbox queues and the status untouched.
#[tokio::test]
async fn leaves_inbox_state_unchanged_when_its_durable_append_fails() {
    let env = harness(MockAdapter::chunks(vec![])).await;
    let agent = create_agent(&env, "rejected-pre-step-append-failure").await;
    let outcome = Arc::new(Mutex::new(None::<String>));
    {
        let (sink, subject) = (outcome.clone(), agent.clone());
        env.ctx
            .on_contained::<SessionEventEmitted>(move |emitted| {
                if emitted.event.event_type() == "test/probe" {
                    let error = subject
                        .followup(user_message("blocked prompt"))
                        .err()
                        .map(|error| error.to_string());
                    *lock(&sink) = Some(error.unwrap_or_default());
                }
                Ok(())
            })
            .expect("probe listener");
    }

    agent
        .session()
        .append("test/probe", json!({}), None)
        .expect("probe append");

    let error = lock(&outcome).clone().expect("the listener ran");
    assert!(error.contains("cannot reenter"), "{error}");
    assert_eq!(event_types(&agent), ["test/probe"]);
    assert!(agent.inbox().next_turn().is_empty());
    assert!(agent.inbox().next_step().is_empty());
    assert_eq!(agent.status(), AgentStatus::Idle);
    env.close().await;
}

#[tokio::test]
async fn a_blocked_prompt_preserves_adjacent_queued_prompts() {
    let adapter = MockAdapter::chunks(vec![
        text_response("safe reply"),
        text_response("wake reply"),
    ]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "a1").await;
    env.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| async move {
            if text_of_all(&event.messages) == "secret" {
                Ok(PreStepDecision::Reject)
            } else {
                next.run().await
            }
        })
        .expect("pre-step listener");
    let reasons = collect_turn_end_reasons(&env);

    let idle = Idle::arm(&env, &agent);
    send(&agent, "secret");
    send(&agent, "safe");
    idle.wait().await;

    let types = event_types(&agent);
    assert_eq!(
        types.iter().filter(|kind| *kind == "user/message").count(),
        0
    );
    assert_eq!(adapter.request_count(), 0);
    assert_eq!(types.iter().filter(|kind| *kind == "turn/start").count(), 1);
    assert_eq!(types.iter().filter(|kind| *kind == "turn/end").count(), 1);
    assert_eq!(*lock(&reasons), [json!({"kind": "blocked"})]);
    assert_eq!(
        first_blocks(&agent.inbox().next_turn()),
        [text_block("safe")]
    );

    send_and_wait(&agent, "wake").await;
    assert!(request_text(&env, 0).contains("safe"));
    assert!(!request_text(&env, 0).contains("secret"));
    env.close().await;
}

#[tokio::test]
async fn a_throwing_pre_step_listener_reports_the_driver_error_and_retains_adjacent_work() {
    let adapter = MockAdapter::chunks(vec![text_response("after")]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "a1").await;
    let threw = Arc::new(AtomicBool::new(false));
    env.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, _next| {
            let first = !threw.swap(true, Ordering::SeqCst);
            async move {
                if first {
                    return Err("prompt hook broke".into());
                }
                Ok(enter(event.messages.clone()))
            }
        })
        .expect("pre-step listener");
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let sink = errors.clone();
        env.ctx
            .on_contained::<AgentError>(move |event| {
                lock(&sink).push(event.error.to_string());
                Ok(())
            })
            .expect("error listener");
    }
    let statuses = Arc::new(Mutex::new(Vec::<AgentStatus>::new()));
    {
        let (sink, subject) = (statuses.clone(), agent.clone());
        env.ctx
            .on_contained::<AgentStatusChanged>(move |event| {
                if event.agent.id() == subject.id() {
                    lock(&sink).push(event.status);
                }
                Ok(())
            })
            .expect("status listener");
    }
    let reasons = Arc::new(Mutex::new(Vec::<Value>::new()));
    {
        let (sink, session) = (reasons.clone(), agent.session().clone());
        env.ctx
            .on_contained::<SessionEventEmitted>(move |emitted| {
                if emitted.session == session && emitted.event.event_type() == "turn/end" {
                    lock(&sink).push(emitted.event.data()["reason"].clone());
                }
                Ok(())
            })
            .expect("session/event listener");
    }

    let idle = Idle::arm(&env, &agent);
    send(&agent, "first");
    send(&agent, "second");
    idle.wait().await;

    assert_eq!(*lock(&errors), ["prompt hook broke"]);
    let types = event_types(&agent);
    assert_eq!(types.iter().filter(|kind| *kind == "turn/start").count(), 1);
    assert_eq!(types.iter().filter(|kind| *kind == "turn/end").count(), 1);
    assert_eq!(
        *lock(&reasons),
        [json!({
            "kind": "error",
            "error": { "message": "prompt hook broke", "code": "UNKNOWN" },
        })]
    );
    assert_eq!(*lock(&statuses), [AgentStatus::Running, AgentStatus::Idle]);
    assert_eq!(adapter.request_count(), 0);
    assert_eq!(
        first_blocks(&agent.inbox().next_turn()),
        [text_block("second")]
    );
    env.close().await;
}

// ----------------------------------------------------------------------------- agent/created

#[tokio::test]
async fn fires_once_with_source_startup_for_a_fresh_create_before_the_first_turn() {
    let env = harness(MockAdapter::chunks(vec![text_response("ok")])).await;
    let sources = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let sink = sources.clone();
    env.ctx
        .on_serial::<AgentCreated, (), _>(move |event| {
            lock(&sink).push(event.source.as_str());
            async { Ok(None) }
        })
        .expect("created listener");

    let agent = create_agent(&env, "a1").await;
    assert_eq!(*lock(&sources), ["startup"]);
    assert!(!event_types(&agent).iter().any(|kind| kind == "turn/start"));

    send_and_wait(&agent, "go").await;
    assert_eq!(*lock(&sources), ["startup"]);
    env.close().await;
}

#[tokio::test]
async fn a_creation_listener_can_inject_context_the_first_request_sees() {
    let adapter = MockAdapter::chunks(vec![text_response("ok")]);
    let env = harness(adapter).await;
    env.ctx
        .on_serial::<AgentCreated, (), _>(move |event| {
            let subject = event.agent.clone();
            async move {
                subject.inject(message_of("test", "session preamble"))?;
                Ok(None)
            }
        })
        .expect("created listener");

    let agent = create_agent(&env, "a1").await;
    send_and_wait(&agent, "go").await;

    // the injected context reached the model on the first (only) request
    assert!(request_text(&env, 0).contains("session preamble"));
    // and is recorded with the plugin source, never mislabeled as a user prompt
    let recorded = user_message_events(&agent);
    let context = recorded
        .iter()
        .find(|event| !sourced_by(event, "user"))
        .expect("context recorded");
    assert_eq!(context["data"]["source"], json!({"kind": "test"}));
    env.close().await;
}

#[tokio::test]
async fn a_throwing_creation_listener_rolls_back_the_agent_before_its_first_turn() {
    let adapter = MockAdapter::chunks(vec![text_response("ok")]);
    let env = harness(adapter.clone()).await;
    env.ctx
        .on_serial::<AgentCreated, (), _>(|_| async { Err("creation hook broke".into()) })
        .expect("created listener");

    let agent_loop = env
        .ctx
        .get::<hoh_agent_loop::AgentLoopKey>()
        .expect("agent loop service");
    let error = agent_loop
        .create(
            hoh_session::SessionId::from("a1"),
            mock_options(),
            hoh_agent::AgentSessionMeta::default(),
        )
        .await
        .err()
        .expect("creation rejects with the listener's reason");

    // `rejects.toBe(reason)`: the rejection is the listener's own error, unwrapped.
    assert_eq!(error.to_string(), "creation hook broke");
    assert!(env.ctx.get::<Agents>().expect("agents").list().is_empty());
    assert!(
        env.ctx
            .get::<Sessions>()
            .expect("sessions")
            .list()
            .is_empty()
    );
    assert_eq!(adapter.request_count(), 0);
    env.close().await;
}

// ----------------------------------------------- tool additionalContexts buffering across a step

#[tokio::test]
async fn appends_each_calls_contexts_only_after_all_tool_results_preserving_adjacency() {
    // One assistant step with TWO tool calls; the second model response stops.
    let two_calls = multi_call(&[
        ("c1", "echo", json!({"text": "a"})),
        ("c2", "echo", json!({"text": "b"})),
    ]);
    let env = harness(MockAdapter::chunks(vec![two_calls, text_response("done")])).await;
    register_tool(
        &env,
        content_tool(
            "echo",
            json!({"text": {"type": "string"}}),
            false,
            |args, _| async move {
                Ok(vec![ContentBlock::text(
                    args["text"].as_str().unwrap_or_default(),
                )])
            },
        ),
    );
    let agent = create_agent(&env, "a1").await;
    // Each call attaches one context naming itself.
    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |event, _next| {
            let text = format!("ctx-{}", event.exec.call_id);
            async move {
                Ok(PostToolDecision::Accept {
                    content: None,
                    value: None,
                    additional_contexts: Some(vec![message_of("p", &text)]),
                })
            }
        })
        .expect("post-execute listener");

    send_and_wait(&agent, "go").await;

    // Event order in the log: both tool/results, THEN both injected contexts — never interleaved
    // (which would break tool-call/result adjacency).
    let log = events_json(&agent);
    let kinds: Vec<&str> = log.iter().map(|e| e["type"].as_str().unwrap()).collect();
    let injected: Vec<&Value> = log
        .iter()
        .filter(|e| e["type"] == "user/message" && !sourced_by(e, "user"))
        .collect();
    let first_result = kinds.iter().position(|kind| *kind == "tool/result");
    let last_result = kinds.iter().rposition(|kind| *kind == "tool/result");
    let first_context = log
        .iter()
        .position(|e| std::ptr::eq(e, injected[0]))
        .expect("context in log");
    assert!(first_result.is_some());
    assert!(last_result > first_result); // two results
    assert!(first_context > last_result.unwrap()); // context only after ALL results
    let texts: Vec<&str> = injected
        .iter()
        .map(|e| e["data"]["content"][0]["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["ctx-c1", "ctx-c2"]);
    env.close().await;
}

#[tokio::test]
async fn appends_multiple_contexts_deferred_by_one_composite_tool_after_its_outer_result() {
    let adapter = MockAdapter::chunks(vec![
        tool_call_response("c1", "composite", &json!({}), None),
        text_response("done"),
    ]);
    let env = harness(adapter).await;
    register_tool(
        &env,
        content_tool("composite", json!({}), false, |_, exec| async move {
            exec.defer_context(message_of("a", "nested-a"));
            exec.defer_context(message_of("b", "nested-b"));
            Ok(vec![ContentBlock::text("outer result")])
        }),
    );
    let agent = create_agent(&env, "a1").await;

    send_and_wait(&agent, "go").await;

    let log = events_json(&agent);
    let result_index = log
        .iter()
        .position(|e| e["type"] == "tool/result")
        .expect("tool result");
    let contexts: Vec<(usize, &Value)> = log
        .iter()
        .enumerate()
        .filter(|(_, e)| e["type"] == "user/message" && !sourced_by(e, "user"))
        .collect();
    assert!(contexts[0].0 > result_index);
    assert_eq!(
        contexts
            .iter()
            .map(|(_, e)| e["data"]["source"].clone())
            .collect::<Vec<_>>(),
        [json!({"kind": "a"}), json!({"kind": "b"})]
    );
    env.close().await;
}

// ------------------------------------------------------------------- tools/pre-execute gate

#[tokio::test]
async fn deny_short_circuits_dispatch_into_an_is_error_result_the_model_sees() {
    let adapter = MockAdapter::chunks(vec![
        tool_call_response("c1", "danger", &json!({}), None),
        text_response("ok"),
    ]);
    let env = harness(adapter).await;
    let ran = Arc::new(AtomicBool::new(false));
    let flag = ran.clone();
    register_tool(
        &env,
        content_tool("danger", json!({}), false, move |_, _| {
            let flag = flag.clone();
            async move {
                flag.store(true, Ordering::SeqCst);
                Ok(vec![ContentBlock::text("should not run")])
            }
        }),
    );
    let agent = create_agent(&env, "a1").await;
    env.ctx
        .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |event, next| async move {
            if event.exec.name == "danger" {
                return Ok(PreToolDecision::Deny {
                    reason: "blocked dangerous tool".into(),
                    info: None,
                });
            }
            next.run().await
        })
        .expect("pre-execute listener");

    send_and_wait(&agent, "go").await;

    assert!(!ran.load(Ordering::SeqCst));
    let result = &events_of(&agent, "tool/result")[0];
    assert_eq!(result["data"]["message"]["isError"], true);
    assert!(
        result["data"]["message"]["content"]
            .as_array()
            .unwrap()
            .iter()
            .any(|block| block["type"] == "text"
                && block["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("blocked dangerous tool")))
    );
    env.close().await;
}

// ------------------------------------- worked example: a native hook plugin on the seams

/// The `NativeGuard` of the source: an ordinary plugin subscribing to the canonical events and
/// returning typed decisions — no hook protocol, no external command, no `hook/*` log.
struct NativeGuard;

impl Plugin for NativeGuard {
    const NAME: &'static str = "native-guard";
    type Config = NoConfig;
    type Deps = ();

    async fn apply(ctx: Context, _deps: (), _config: Cfg<NoConfig>) -> Result<(), BoxError> {
        // 1. SessionStart: seed a standing instruction.
        ctx.on_serial::<AgentCreated, (), _>(|event| {
            let (subject, source) = (event.agent.clone(), event.source);
            async move {
                subject.inject(message_of(
                    "native-guard",
                    &format!("policy active (started: {})", source.as_str()),
                ))?;
                Ok(None)
            }
        })?;
        // 2. PreStep: reject a forbidden prompt, pass the rest on.
        ctx.on_waterfall::<AgentPreStep, PreStepDecision, _>(|event, next| async move {
            if text_of_all(&event.messages).contains("rm -rf") {
                return Ok(PreStepDecision::Reject);
            }
            next.run().await
        })?;
        // 3. PreToolUse: deny a dangerous tool by name.
        ctx.on_waterfall::<ToolsPreExecute, PreToolDecision, _>(|event, next| async move {
            if event.exec.name == "danger" {
                return Ok(PreToolDecision::Deny {
                    reason: "danger tool denied".into(),
                    info: None,
                });
            }
            next.run().await
        })?;
        // 4. PostToolUse: attach context after a tool runs.
        ctx.on_waterfall::<ToolsPostExecute, PostToolDecision, _>(|_, next| async move {
            let decision = next.run().await?;
            if matches!(decision, PostToolDecision::Accept { .. }) {
                return Ok(PostToolDecision::Accept {
                    content: None,
                    value: None,
                    additional_contexts: Some(vec![message_of("native-guard", "audited")]),
                });
            }
            Ok(decision)
        })?;
        Ok(())
    }
}

async fn mount_guard(env: &Env) -> hoh_kernel::Fiber {
    let fiber = env
        .ctx
        .plugin::<NativeGuard>(json!({}))
        .expect("guard mounts");
    fiber.settled().await.expect("guard applies");
    fiber
}

fn context_text_event(log: &[Value], matches: impl Fn(&str) -> bool) -> bool {
    log.iter().any(|e| {
        e["type"] == "user/message"
            && !sourced_by(e, "user")
            && e["data"]["content"]
                .as_array()
                .unwrap()
                .iter()
                .any(|block| block["type"] == "text" && matches(block["text"].as_str().unwrap()))
    })
}

#[tokio::test]
async fn all_four_seams_fire_for_a_real_allowed_turn_with_a_tool_call() {
    let adapter = MockAdapter::chunks(vec![
        tool_call_response("c1", "echo", &json!({"text": "hi"}), None),
        text_response("done"),
    ]);
    let env = harness(adapter).await;
    mount_guard(&env).await;
    register_tool(
        &env,
        content_tool(
            "echo",
            json!({"text": {"type": "string"}}),
            false,
            |args, _| async move {
                Ok(vec![ContentBlock::text(
                    args["text"].as_str().unwrap_or_default(),
                )])
            },
        ),
    );
    let agent = create_agent(&env, "a1").await;

    send_and_wait(&agent, "please echo hi").await;

    let log = events_json(&agent);
    // session-start preamble injected
    assert!(context_text_event(&log, |text| text
        .contains("policy active (started: startup)")));
    // prompt allowed → user-sourced user/message recorded
    assert!(
        log.iter()
            .any(|e| e["type"] == "user/message" && sourced_by(e, "user"))
    );
    // tool ran (echo allowed) and post-execute attached "audited" context
    assert!(
        log.iter()
            .any(|e| e["type"] == "tool/result" && e["data"]["message"]["isError"] != true)
    );
    assert!(context_text_event(&log, |text| text == "audited"));
    // NO hook/* events — a native plugin needs none
    assert!(
        !log.iter()
            .any(|e| e["type"].as_str().unwrap().starts_with("hook/"))
    );
    env.close().await;
}

#[tokio::test]
async fn the_same_plugin_blocks_a_destructive_prompt_inside_a_no_step_turn() {
    let adapter = MockAdapter::chunks(vec![text_response("should not run")]);
    let env = harness(adapter.clone()).await;
    mount_guard(&env).await;
    let agent = create_agent(&env, "a2").await;
    let reasons = collect_turn_end_reasons(&env);

    send(&agent, "run rm -rf /");
    agent.when_idle().await;

    assert_eq!(adapter.request_count(), 0);
    assert_eq!(*lock(&reasons), [json!({"kind": "blocked"})]);
    env.close().await;
}

#[tokio::test]
async fn hmr_safety_disposing_the_plugin_fiber_removes_all_four_listeners() {
    let adapter = MockAdapter::chunks(vec![text_response("ok")]);
    let env = harness(adapter.clone()).await;
    let fiber = mount_guard(&env).await;
    fiber.dispose().await;

    // After disposal, a destructive prompt is NOT blocked (the listener is gone).
    let agent = create_agent(&env, "a3").await;
    send_and_wait(&agent, "run rm -rf /").await;
    // the prompt ran (not rejected) — proving the pre-step listener was disposed
    assert_eq!(adapter.request_count(), 1);
    assert!(
        event_types(&agent)
            .iter()
            .any(|kind| kind == "user/message")
    );
    env.close().await;
}
