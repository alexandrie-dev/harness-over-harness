//! Port of `tests/steer-wait.spec.ts` (with `scripted-adapter.ts`): next-step input during a
//! background result wait, driven through a real agent loop and a scripted model.
//!
//! The source observes the wait through `vi.spyOn(ctx.jobs, 'wait')`; the `Jobs` service here
//! has no such seam, so the registration of the wait and its release are observed through their
//! effects (the scripted request count, the tool result) after real-time pauses.

mod common;

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

use common::{
    Producer, ProducerOptions, completed_with, killed, lock, mount, producer_with, user_message,
};
use futures_util::stream;
use hoh_agent::{Agent, AgentOptions, AgentSessionMeta, InboxTarget, Live};
use hoh_agent_loop::{AgentLoopKey, AgentLoopPlugin};
use hoh_error::BoxError;
use hoh_jobs::{JobEvent, JobEventFilter, JobId, JobStatus, Jobs};
use hoh_kernel::{Context, Kernel};
use hoh_llm::{GenerateOptions, Llm, LlmAdapter, LlmStream, StreamChunk, UserMessage};
use hoh_session::{AgentCancelCause, SessionId};
use hoh_tool_jobs::ToolJobs;
use hoh_tool_testkit::{ContentToolFixtureOptions, define_content_tool_fixture};
use hoh_tools::{ToolRegistrationOptions, Tools};
use serde_json::{Value, json};
use tokio::sync::oneshot;

fn chunk(value: Value) -> StreamChunk {
    serde_json::from_value(value).expect("stream chunk")
}

/// One complete text response.
fn text_chunks(value: &str) -> Vec<StreamChunk> {
    vec![
        chunk(json!({"type":"block-start","index":0,"blockType":"text"})),
        chunk(json!({"type":"text-delta","index":0,"text":value})),
        chunk(json!({"type":"block-end","index":0,"block":{"type":"text","text":value}})),
        chunk(json!({"type":"finish","reason":{"kind":"stop"}})),
    ]
}

/// The non-finish chunks of one single-tool-call response, at the given block index.
fn tool_call_blocks(index: usize, call_id: &str, name: &str, args: &Value) -> Vec<StreamChunk> {
    let arguments = serde_json::to_string(args).unwrap();
    vec![
        chunk(json!({"type":"block-start","index":index,"blockType":"tool-call"})),
        chunk(
            json!({"type":"tool-call-delta","index":index,"id":call_id,"name":name,"argumentsDelta":arguments}),
        ),
        chunk(
            json!({"type":"block-end","index":index,"block":{"type":"tool-call","id":call_id,"name":name,"arguments":arguments}}),
        ),
    ]
}

fn finish(kind: &str) -> StreamChunk {
    chunk(json!({"type":"finish","reason":{"kind":kind}}))
}

/// One complete single-tool-call response, ending the model turn.
fn tool_call_chunks(call_id: &str, name: &str, args: &Value) -> Vec<StreamChunk> {
    let mut chunks = tool_call_blocks(0, call_id, name, args);
    chunks.push(finish("tool-calls"));
    chunks
}

/// Each model call consumes the next scripted stream; every request is retained.
struct ScriptedAdapter {
    script: Mutex<VecDeque<Vec<StreamChunk>>>,
    requests: Mutex<Vec<Value>>,
}

impl ScriptedAdapter {
    fn request_count(&self) -> usize {
        lock(&self.requests).len()
    }
    fn request(&self, index: usize) -> String {
        lock(&self.requests)[index]["messages"].to_string()
    }
    fn push(&self, calls: Vec<Vec<StreamChunk>>) {
        lock(&self.script).extend(calls);
    }
}

impl LlmAdapter for ScriptedAdapter {
    fn stream(&self, options: GenerateOptions) -> Result<LlmStream, BoxError> {
        lock(&self.requests).push(serde_json::to_value(&options)?);
        match lock(&self.script).pop_front() {
            Some(chunks) => Ok(LlmStream::chunks(chunks)),
            None => Ok(LlmStream::new(stream::once(async {
                Err::<StreamChunk, BoxError>("scripted adapter exhausted".into())
            }))),
        }
    }
}

/// The script of one run, built once the job's opaque id is known.
type Script<'a> = Option<&'a dyn Fn(&JobId) -> Vec<Vec<StreamChunk>>>;

struct Harness {
    _kernel: Kernel,
    ctx: Context,
    agent: Arc<dyn Agent>,
    adapter: Arc<ScriptedAdapter>,
    id: JobId,
    producer: Producer,
    _registration: hoh_llm::AdapterRegistrationHandle,
}

async fn pause(millis: u64) {
    tokio::time::sleep(Duration::from_millis(millis)).await;
}

/// Wait (bounded) for `condition`, polling in real time.
async fn until(what: &str, condition: impl Fn() -> bool) {
    for _ in 0..400 {
        if condition() {
            return;
        }
        pause(5).await;
    }
    panic!("timed out waiting for {what}");
}

impl Harness {
    /// `setup(build)`: the default script is a `job_output` wait followed by a text reply.
    async fn new(build: Script<'_>) -> Harness {
        let (kernel, ctx, _agents) = common::base(json!({})).await;
        mount::<AgentLoopPlugin>(&ctx, json!({ "agents": [] })).await;
        mount::<ToolJobs>(&ctx, json!({ "completionDelivery": "quiet" })).await;
        let adapter = Arc::new(ScriptedAdapter {
            script: Mutex::new(VecDeque::new()),
            requests: Mutex::new(Vec::new()),
        });
        let registration = ctx
            .get::<Llm>()
            .expect("llm service")
            .register_adapter(&ctx, vec!["mock".into()], adapter.clone())
            .expect("adapter registers");
        let agent = ctx
            .get::<AgentLoopKey>()
            .expect("agent loop service")
            .create(
                SessionId::from("steer-wait"),
                AgentOptions {
                    provider: Some(Live::Value("mock".into())),
                    model: Some(Live::Value("mock".into())),
                    ..AgentOptions::default()
                },
                AgentSessionMeta::default(),
            )
            .await
            .expect("agent creates");
        let mut producer = producer_with(ProducerOptions {
            label: Some("gated producer".to_owned()),
            owner: Some(agent.id().clone()),
            settle_on_cancel: Some(killed()),
            ..ProducerOptions::default()
        });
        let id = ctx
            .get::<Jobs>()
            .expect("jobs")
            .start(producer.spec())
            .expect("start");
        producer.append("ready output\n");
        adapter.push(match build {
            Some(build) => build(&id),
            None => vec![
                tool_call_chunks(
                    "wait",
                    "job_output",
                    &json!({ "job_id": id.to_string(), "wait": true, "timeout_ms": 600_000 }),
                ),
                text_chunks("input handled"),
            ],
        });
        Harness {
            _kernel: kernel,
            ctx,
            agent,
            adapter,
            id,
            producer,
            _registration: registration,
        }
    }

    fn status(&self) -> JobStatus {
        self.ctx
            .get::<Jobs>()
            .unwrap()
            .get(&self.id, Some(self.agent.id()))
            .expect("job")
            .status
    }

    fn next_step(&self) -> Vec<UserMessage> {
        self.agent.inbox().next_step()
    }

    fn events(&self) -> Vec<Arc<hoh_session::SessionEvent>> {
        self.agent.session().snapshot_events(None, None).to_vec()
    }

    /// The model turn has reached the wait: the first request was served and the tool call is
    /// registered with the registry.
    async fn wait_registered(&self) {
        until("the first model request", || {
            self.adapter.request_count() == 1
        })
        .await;
        pause(50).await;
    }

    async fn dispose(self) {
        self.producer.settle(completed_with(None, None));
        let _ = self
            .agent
            .cancel(AgentCancelCause::User, Default::default());
        self.agent.when_idle().await;
    }
}

fn seq_of(
    events: &[Arc<hoh_session::SessionEvent>],
    found: impl Fn(&hoh_session::SessionEvent) -> bool,
) -> u64 {
    u64::from(
        events
            .iter()
            .find(|e| found(e))
            .expect("event present")
            .seq(),
    )
}

#[tokio::test]
async fn releases_a_registered_wait_before_producer_settlement_and_admits_steering_after_step_end()
{
    let h = Harness::new(None).await;
    h.agent.followup(user_message("start")).unwrap();
    h.wait_registered().await;
    assert_eq!(h.status(), JobStatus::Running);
    assert_eq!(h.adapter.request_count(), 1);
    let steer = user_message("UNIQUE_STEER_5965");
    h.agent.steer(steer.clone()).unwrap();
    h.agent.when_idle().await;
    assert_eq!(h.status(), JobStatus::Running);
    assert!(h.producer.cancels().is_empty());
    assert_eq!(h.adapter.request_count(), 2);
    assert!(h.adapter.request(1).contains("UNIQUE_STEER_5965"));
    assert!(
        h.adapter
            .request(1)
            .contains("ready output\\n[status: running, user steered]")
    );

    let events = h.events();
    let steer_id = serde_json::to_value(&steer.id).unwrap();
    let receipt = seq_of(&events, |e| {
        e.event_type() == "agent/inbox/spliced"
            && e.data()["inserted"]
                .as_array()
                .is_some_and(|m| m.iter().any(|m| m["id"] == steer_id))
    });
    let result = seq_of(&events, |e| e.event_type() == "tool/result");
    let end = seq_of(&events, |e| e.event_type() == "step/end");
    let admitted = seq_of(&events, |e| {
        e.event_type() == "user/message" && e.data()["id"] == steer_id
    });
    assert!(receipt < result);
    assert!(result < end);
    assert!(end < admitted);
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type() == "turn/start")
            .count(),
        1
    );

    let (tx, rx) = oneshot::channel::<bool>();
    let tx = Mutex::new(Some(tx));
    let _subscription = h
        .ctx
        .get::<Jobs>()
        .unwrap()
        .events()
        .subscribe(
            &h.ctx,
            JobEventFilter::Owner {
                owner: h.agent.id().clone(),
            },
            Arc::new(move |event| {
                if let JobEvent::Settled { awaited, .. } = event
                    && let Some(tx) = lock(&tx).take()
                {
                    let _ = tx.send(*awaited);
                }
                Ok(())
            }),
        )
        .expect("subscribe");
    h.producer
        .settle(completed_with(None, Some("final output")));
    assert!(!rx.await.unwrap());
    // The settlement's notice carries the result inline, so the registry has nothing left to hand
    // out.
    let notices = h.next_step();
    assert_eq!(notices.len(), 1);
    assert!(
        serde_json::to_string(&notices[0].content)
            .unwrap()
            .contains("Output:\\nfinal output")
    );
    assert!(
        h.ctx
            .get::<Jobs>()
            .unwrap()
            .read(&h.id, Some(h.agent.id()))
            .unwrap()
            .result
            .is_none()
    );
    h.dispose().await;
}

#[tokio::test]
async fn does_not_release_a_wait_for_next_turn_queue_input() {
    let h = Harness::new(Some(&|id: &JobId| {
        vec![
            tool_call_chunks(
                "wait",
                "job_output",
                &json!({ "job_id": id.to_string(), "wait": true }),
            ),
            text_chunks("original done"),
            text_chunks("queued handled"),
        ]
    }))
    .await;
    h.agent.followup(user_message("start")).unwrap();
    h.wait_registered().await;
    h.agent.followup(user_message("UNIQUE_QUEUE_5965")).unwrap();
    pause(50).await;
    // Still waiting: no second model request.
    assert_eq!(h.adapter.request_count(), 1);
    h.producer.settle(completed_with(None, None));
    h.agent.when_idle().await;
    assert!(!h.adapter.request(1).contains("UNIQUE_QUEUE_5965"));
    assert!(h.adapter.request(2).contains("UNIQUE_QUEUE_5965"));
    assert_eq!(
        h.events()
            .iter()
            .filter(|e| e.event_type() == "turn/start")
            .count(),
        2
    );
    assert!(h.producer.cancels().is_empty());
    h.dispose().await;
}

/// A `job_output` wait through the tool pipeline directly, on behalf of the harness agent.
async fn direct_wait(h: &Harness, call_id: &str) -> hoh_tools::ToolExecutionResult {
    let mut input = hoh_tools::ToolExecutionInput::new(
        hoh_llm::ToolCallId::from(call_id.to_owned()),
        "job_output",
        json!({ "job_id": h.id.to_string(), "wait": true }),
        hoh_timeout::AbortController::new().signal(),
    );
    input.agent = Some(h.agent.clone());
    hoh_tool_testkit::execute_tool_fixture(&h.ctx, input)
        .await
        .expect("tool call")
}

#[tokio::test]
async fn releases_for_already_pending_input_without_waiting_for_another_insertion() {
    let h = Harness::new(Some(&|_: &JobId| vec![])).await;
    h.agent
        .inbox()
        .append(InboxTarget::NextStep, user_message("ALREADY_PENDING"))
        .unwrap();
    let result = direct_wait(&h, "pending").await;
    assert!(!result.is_error());
    assert!(common::text(&result).contains("[status: running, user steered]"));
    assert_eq!(h.next_step().len(), 1);
    assert!(h.producer.cancels().is_empty());
    h.dispose().await;
}

#[tokio::test]
async fn does_not_lose_input_inserted_during_waiter_registration() {
    let h = Harness::new(Some(&|_: &JobId| vec![])).await;
    // The insertion races the wait's registration: start the wait, insert while it is being set up.
    let call = direct_wait(&h, "race");
    let insert = async {
        tokio::task::yield_now().await;
        h.agent
            .inbox()
            .append(InboxTarget::NextStep, user_message("REGISTRATION_RACE"))
            .unwrap();
    };
    let (result, ()) = tokio::join!(call, insert);
    assert!(!result.is_error());
    assert!(common::text(&result).contains("[status: running, user steered]"));
    assert!(h.producer.cancels().is_empty());
    h.dispose().await;
}

#[tokio::test]
async fn releases_both_parallel_waits_but_drains_ordinary_work_before_admitting_edited_input() {
    let h = Harness::new(Some(&|id: &JobId| {
        let args = json!({ "job_id": id.to_string(), "wait": true });
        let mut blocks = tool_call_blocks(0, "wait-a", "job_output", &args);
        blocks.extend(tool_call_blocks(1, "wait-b", "job_output", &args));
        blocks.extend(tool_call_blocks(2, "ordinary", "ordinary", &json!({})));
        blocks.push(finish("tool-calls"));
        vec![blocks, text_chunks("both inputs handled")]
    }))
    .await;
    let entered = Arc::new(Mutex::new(None::<hoh_timeout::AbortSignal>));
    let (drain_tx, drain_rx) = tokio::sync::watch::channel(false);

    // `job_output` is not concurrency-safe by default; the agent scope re-registers it as safe.
    let tools = h.ctx.get::<Tools>().expect("tools");
    let mut job_output = (*tools.get("job_output", None, None).expect("job_output")).clone();
    job_output.is_concurrency_safe = Some(Arc::new(|_| true));
    tools
        .register(
            h.agent.ctx(),
            job_output,
            ToolRegistrationOptions::default(),
        )
        .expect("register job_output in the agent scope");
    let seen = entered.clone();
    let mut ordinary = ContentToolFixtureOptions::new(
        "ordinary",
        "gated execution",
        json!({}),
        move |_args, exec: Arc<hoh_tools::ToolExecution>| {
            let seen = seen.clone();
            let mut drain = drain_rx.clone();
            async move {
                *lock(&seen) = Some(exec.signal().clone());
                let _ = drain.wait_for(|open| *open).await;
                Ok(vec![hoh_llm::ContentBlock::text("ordinary actual result")])
            }
        },
    );
    ordinary.is_concurrency_safe = Some(Arc::new(|_| true));
    tools
        .register(
            &h.ctx,
            define_content_tool_fixture(ordinary).expect("define"),
            ToolRegistrationOptions::default(),
        )
        .expect("register ordinary");

    h.agent.followup(user_message("start")).unwrap();
    until("the ordinary tool", || lock(&entered).is_some()).await;
    pause(50).await;
    let signal = lock(&entered).clone().unwrap();

    let removed = user_message("WITHDRAWN");
    let edited = user_message("ORIGINAL_EDIT");
    h.agent.steer(removed.clone()).unwrap();
    h.agent.inbox().remove(&removed.id).unwrap();
    h.agent.steer(edited.clone()).unwrap();
    let replacement = user_message("EDITED_INPUT");
    h.agent
        .inbox()
        .replace(&edited.id, replacement.clone())
        .unwrap();
    h.agent.steer(user_message("SECOND_STEER")).unwrap();
    let duplicate = h.agent.steer(replacement.clone()).unwrap_err();
    assert!(duplicate.to_string().contains("already pending"));
    pause(50).await;
    assert!(!signal.aborted());
    assert_eq!(h.adapter.request_count(), 1);
    assert_eq!(h.status(), JobStatus::Running);
    drain_tx.send(true).unwrap();
    h.agent.when_idle().await;

    let request = h.adapter.request(1);
    assert!(request.contains("ordinary actual result"));
    assert!(request.contains("EDITED_INPUT"));
    assert!(request.contains("SECOND_STEER"));
    assert!(!request.contains("WITHDRAWN"));
    assert!(!request.contains("ORIGINAL_EDIT"));
    let events = h.events();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type() == "tool/result")
            .count(),
        3
    );
    let replacement_id = serde_json::to_value(&replacement.id).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type() == "user/message" && e.data()["id"] == replacement_id)
            .count(),
        1
    );
    assert!(h.producer.cancels().is_empty());
    drain_tx.send(true).ok();
    h.dispose().await;
}

#[tokio::test]
async fn keeps_explicit_cancel_separate_and_removes_the_input_listener_on_wait_exit() {
    let h = Harness::new(None).await;
    h.agent.followup(user_message("start")).unwrap();
    h.wait_registered().await;
    h.agent
        .cancel(AgentCancelCause::User, Default::default())
        .unwrap();
    h.agent.when_idle().await;
    assert_eq!(h.status(), JobStatus::Running);
    assert!(h.next_step().is_empty());
    assert!(h.producer.cancels().is_empty());
    assert!(
        serde_json::to_string(
            &h.events()
                .iter()
                .map(|e| e.data().clone())
                .collect::<Vec<_>>()
        )
        .unwrap()
        .contains("wait aborted")
    );
    // A queued input after the cancel must not reopen a claim on this job: a leaked listener
    // would open one here and suppress the notice below.
    h.agent
        .inbox()
        .append(InboxTarget::NextStep, user_message("AFTER_CANCEL"))
        .unwrap();
    assert_eq!(h.status(), JobStatus::Running);
    h.producer.settle(completed_with(None, None));
    pause(50).await;
    let notices: Vec<_> = h
        .next_step()
        .into_iter()
        .filter(|m| m.source.kind == "tool-jobs")
        .collect();
    assert_eq!(notices.len(), 1);
    assert!(
        serde_json::to_string(&notices[0])
            .unwrap()
            .contains("finished")
    );
    h.dispose().await;
}
