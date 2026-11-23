mod support;

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentAssistantStream, AgentOptions, AgentStatus, AssistantStreamFrame, CancelOptions,
    Inbox, InboxTarget, MaintenanceFuture, MaintenanceTask, Model, agent_events,
};
use hoh_error::BoxError;
use hoh_kernel::Context;
use hoh_llm::{LlmAttemptId, StreamChunk, UserMessage};
use hoh_scope::ScopeKey;
use hoh_sdk_jsonrpc_server::{HarnessSdkJsonRpcServer, HarnessSdkJsonRpcServerOptions};
use hoh_session::{AgentCancelCause, Session};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use support::{Env, Peer, lock};

struct SessionReadProbe {
    inner: Arc<dyn Agent>,
    reads: Arc<AtomicUsize>,
}

impl Agent for SessionReadProbe {
    fn id(&self) -> &hoh_session::SessionId {
        self.inner.id()
    }

    fn scope_key(&self) -> ScopeKey {
        self.inner.scope_key()
    }

    fn options(&self) -> &AgentOptions {
        self.inner.options()
    }

    fn session(&self) -> &Session {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.session()
    }

    fn model(&self) -> &dyn Model {
        self.inner.model()
    }

    fn inbox(&self) -> &dyn Inbox {
        self.inner.inbox()
    }

    fn status(&self) -> AgentStatus {
        self.inner.status()
    }

    fn ctx(&self) -> &Context {
        self.inner.ctx()
    }

    fn cancel(&self, cause: AgentCancelCause, options: CancelOptions) -> Result<(), BoxError> {
        self.inner.cancel(cause, options)
    }

    fn when_idle(&self) -> BoxFuture<'static, ()> {
        self.inner.when_idle()
    }

    fn run_maintenance_erased(&self, task: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        self.inner.run_maintenance_erased(task)
    }

    fn send(
        &self,
        message: UserMessage,
        target: InboxTarget,
        wakeup: bool,
    ) -> Result<(), BoxError> {
        self.inner.send(message, target, wakeup)
    }

    fn followup(&self, message: UserMessage) -> Result<(), BoxError> {
        self.inner.followup(message)
    }

    fn steer(&self, message: UserMessage) -> Result<(), BoxError> {
        self.inner.steer(message)
    }

    fn inject(&self, message: UserMessage) -> Result<(), BoxError> {
        self.inner.inject(message)
    }
}

fn emit_start(env: &Env, agent: &Arc<dyn Agent>, attempt: u64) {
    agent_events(&env.ctx, agent).emit(|agent| {
        let attempt_id = LlmAttemptId::from(format!("{}:{attempt}", agent.id()));
        AgentAssistantStream {
            agent,
            frame: AssistantStreamFrame::Start {
                attempt_id,
                revision: attempt,
                turn: attempt,
                step: 1,
            },
        }
    });
}

fn emit_chunk(env: &Env, agent: &Arc<dyn Agent>, attempt: u64, chunk: StreamChunk) {
    agent_events(&env.ctx, agent).emit(|agent| {
        let attempt_id = LlmAttemptId::from(format!("{}:{attempt}", agent.id()));
        AgentAssistantStream {
            agent,
            frame: AssistantStreamFrame::Chunk {
                attempt_id,
                revision: attempt,
                index: 0,
                time: 123,
                chunk,
            },
        }
    });
}

fn chunks() -> Vec<StreamChunk> {
    [
        json!({"type":"block-start","index":0,"blockType":"text"}),
        json!({"type":"text-delta","index":0,"text":"first "}),
        json!({"type":"text-delta","index":0,"text":"second"}),
        json!({"type":"block-end","index":0,"block":{"type":"text","text":"first second"}}),
        json!({"type":"finish","reason":{"kind":"stop"}}),
    ]
    .into_iter()
    .map(|value| serde_json::from_value(value).unwrap())
    .collect()
}

fn unfinished_tool_call() -> Vec<StreamChunk> {
    [
        json!({"type":"block-start","index":0,"blockType":"tool-call"}),
        json!({"type":"tool-call-delta","index":0,"id":"call-1","name":"read","argumentsDelta":"{\"pa"}),
        json!({"type":"block-end","index":0,"block":{"type":"tool-call","id":"call-1","name":"read","arguments":"{}"}}),
        json!({"type":"finish","reason":{"kind":"stop"}}),
    ]
    .into_iter()
    .map(|value| serde_json::from_value(value).unwrap())
    .collect()
}

fn is_message(params: &Value) -> bool {
    params["event"]["type"] == "assistant/message"
}

fn positions(notices: &[(String, Value)]) -> (Vec<usize>, usize, usize) {
    let chunk_positions = notices
        .iter()
        .enumerate()
        .filter_map(|(position, (method, params))| {
            (method == "session.assistant-stream" && params["frame"]["type"] == "chunk")
                .then_some(position)
        })
        .collect::<Vec<_>>();
    let message = notices
        .iter()
        .position(|(method, params)| method == "session.event" && is_message(params))
        .expect("durable assistant/message notification");
    let end = notices
        .iter()
        .position(|(method, params)| {
            method == "session.assistant-stream" && params["frame"]["type"] == "end"
        })
        .expect("terminal assistant stream frame");
    (chunk_positions, message, end)
}

async fn delayed() -> (Env, Arc<tokio::sync::Semaphore>) {
    let env = Env::new().await;
    env.initialize().await;
    env.open("stream").await;
    lock(&env.peer.notices).clear();
    *lock(&env.model.scripted_chunks) = Some(chunks());
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    *lock(&env.model.pause_before) = Some((3, gate.clone()));
    env.prompt("stream", "go").await;
    env.model.entered.notified().await;
    (env, gate)
}

#[tokio::test]
async fn sends_each_text_delta_before_the_durable_message_and_end_after_it() {
    let (env, gate) = delayed().await;
    {
        let notices = lock(&env.peer.notices);
        assert_eq!(
            notices
                .iter()
                .filter(|(method, params)| method == "session.assistant-stream"
                    && params["frame"]["type"] == "chunk")
                .count(),
            2
        );
        assert!(
            !notices
                .iter()
                .any(|(method, params)| { method == "session.event" && is_message(params) })
        );
    }
    gate.add_permits(1);
    env.wait("stream").await;

    let notices = lock(&env.peer.notices).clone();
    let (chunk_positions, message, end) = positions(&notices);
    assert_eq!(chunk_positions.len(), 2);
    assert!(chunk_positions.iter().all(|position| *position < message));
    assert!(message < end);
    assert_eq!(notices[end].1["frame"]["outcome"], "committed");
    assert_eq!(
        chunk_positions
            .iter()
            .map(|position| notices[*position].1["frame"]["chunk"]["text"]
                .as_str()
                .unwrap())
            .collect::<Vec<_>>(),
        ["first ", "second"]
    );
    env.close().await;
}

#[tokio::test]
async fn sends_reasoning_deltas_including_the_empty_block_opener() {
    let env = Env::new().await;
    env.initialize().await;
    let agent = env.open("reasoning").await;
    lock(&env.peer.notices).clear();

    emit_start(&env, &agent, 1);
    emit_chunk(
        &env,
        &agent,
        1,
        serde_json::from_value(json!({
            "type": "reasoning-delta",
            "index": 0,
            "text": ""
        }))
        .unwrap(),
    );

    let stream = env.peer.events("session.assistant-stream");
    assert_eq!(stream.len(), 2);
    assert_eq!(stream[1]["frame"]["type"], "chunk");
    assert_eq!(
        stream[1]["frame"]["chunk"],
        json!({"type": "reasoning-delta", "text": ""})
    );
    env.close().await;
}

#[tokio::test]
async fn cancel_keeps_sent_text_and_ends_the_attempt_as_abandoned() {
    let (env, _gate) = delayed().await;
    assert_eq!(
        env.peer
            .events("session.assistant-stream")
            .iter()
            .filter(|params| params["frame"]["type"] == "chunk")
            .count(),
        2
    );

    env.server
        .handle_request("session/cancel", json!({"sessionId":"stream"}))
        .await
        .unwrap();
    env.wait("stream").await;

    let notices = lock(&env.peer.notices).clone();
    let (chunk_positions, message, end) = positions(&notices);
    assert_eq!(chunk_positions.len(), 2);
    assert!(chunk_positions.iter().all(|position| *position < message));
    assert!(message < end);
    assert_eq!(notices[end].1["frame"]["outcome"], "abandoned");
    assert_eq!(
        notices[message].1["event"]["data"]["message"]["content"][0]["text"],
        "first second"
    );
    env.close().await;
}

#[tokio::test]
async fn cancel_before_visible_text_commits_an_empty_attempt_like_dsh() {
    let env = Env::new().await;
    env.initialize().await;
    env.open("empty").await;
    lock(&env.peer.notices).clear();
    *lock(&env.model.scripted_chunks) = Some(unfinished_tool_call());
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    *lock(&env.model.pause_before) = Some((2, gate));
    env.prompt("empty", "go").await;
    env.model.entered.notified().await;

    env.server
        .handle_request("session/cancel", json!({"sessionId":"empty"}))
        .await
        .unwrap();
    env.wait("empty").await;

    let stream = env.peer.events("session.assistant-stream");
    assert!(
        !stream
            .iter()
            .any(|params| params["frame"]["type"] == "chunk")
    );
    assert_eq!(stream.last().unwrap()["frame"]["type"], "end");
    assert_eq!(stream.last().unwrap()["frame"]["outcome"], "committed");
    let session_events = env.peer.events("session.event");
    assert!(
        session_events
            .iter()
            .any(|params| params["event"]["type"] == "assistant/attempt")
    );
    assert!(
        !session_events
            .iter()
            .any(|params| params["event"]["type"] == "assistant/message")
    );
    env.close().await;
}

#[tokio::test]
async fn isolates_simultaneous_sessions_owned_by_two_servers_on_one_runtime() {
    let env = Env::new().await;
    let other_peer = Arc::new(Peer::default());
    let other = HarnessSdkJsonRpcServer::new(
        env.ctx.clone(),
        other_peer.clone(),
        HarnessSdkJsonRpcServerOptions::default(),
    )
    .unwrap();
    env.initialize().await;
    other.initialize(env.params()).await.unwrap();
    let left = env.open("left").await;
    other
        .handle_request(
            "session/working-directory/get",
            json!({"sessionId":"right"}),
        )
        .await
        .unwrap();
    let right = env.agent("right");
    lock(&env.peer.notices).clear();
    lock(&other_peer.notices).clear();

    emit_start(&env, &left, 1);
    assert_eq!(
        env.peer.events("session.assistant-stream")[0]["sessionId"],
        "left"
    );
    assert!(other_peer.events("session.assistant-stream").is_empty());

    emit_start(&env, &right, 2);
    assert_eq!(env.peer.events("session.assistant-stream").len(), 1);
    let other_notices = other_peer.events("session.assistant-stream");
    assert_eq!(other_notices.len(), 1);
    assert_eq!(other_notices[0]["sessionId"], "right");
    assert_eq!(other_notices[0]["frame"]["attemptId"], "right:2");

    other.shutdown().await.unwrap();
    env.close().await;
}

#[tokio::test]
async fn ignores_unopened_sessions_and_a_foreign_agent_with_the_same_session_id() {
    let env = Env::new().await;
    env.initialize().await;
    env.open("owned").await;

    let foreign = Env::new().await;
    foreign.initialize().await;
    let replacement = foreign.open("owned").await;
    let unopened = foreign.open("unopened").await;
    lock(&env.peer.notices).clear();

    emit_start(&env, &replacement, 1);
    emit_start(&env, &unopened, 1);

    assert!(env.peer.events("session.assistant-stream").is_empty());
    foreign.close().await;
    env.close().await;
}

#[tokio::test]
async fn ignores_frames_after_the_owned_agent_is_disposed_outside_the_server() {
    let env = Env::new().await;
    env.initialize().await;
    let agent = env.open("closed").await;
    lock(&env.peer.notices).clear();

    env.loop_fiber.dispose().await;
    emit_start(&env, &agent, 1);

    assert!(env.peer.events("session.assistant-stream").is_empty());
    env.close().await;
}

#[tokio::test]
async fn removes_the_stream_subscription_when_the_server_stops() {
    let env = Env::new().await;
    env.initialize().await;
    let agent = env.open("stopped").await;
    let reads = Arc::new(AtomicUsize::new(0));
    let probe: Arc<dyn Agent> = Arc::new(SessionReadProbe {
        inner: agent,
        reads: reads.clone(),
    });
    lock(&env.peer.notices).clear();

    emit_start(&env, &probe, 1);
    assert_eq!(reads.load(Ordering::SeqCst), 1, "listener precondition");
    reads.store(0, Ordering::SeqCst);

    env.server.shutdown().await.unwrap();
    let before = lock(&env.peer.notices).len();
    emit_start(&env, &probe, 2);

    assert_eq!(reads.load(Ordering::SeqCst), 0, "listener was not removed");
    assert_eq!(lock(&env.peer.notices).len(), before);
    env.close().await;
}
