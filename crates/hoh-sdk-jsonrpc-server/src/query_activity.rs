//! packages/sdk/server/src/query-activity.ts. Command completion through quiescence.
use crate::{lock, transport::JsonRpcTransportPeer};
use hoh_agent::{Agent, AgentDisposed, AgentError, AgentStatus, AgentStatusChanged};
use hoh_commands::{CommandExecution, CommandResult};
use hoh_error::BoxError;
use hoh_goal::{GoalActivation, GoalActivationChanged, GoalPhase, Goals};
use hoh_kernel::{Context, Disposer};
use hoh_session::SessionEventEmitted;
use hoh_timeout::AbortSignal;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

tokio::task_local! { pub(crate) static QUERY_INVOCATION: u64; }
#[derive(Default)]
struct State {
    command_id: Option<String>,
    result: Option<CommandResult>,
    closed: bool,
    input: bool,
    input_idle: bool,
    goal_id: Option<Value>,
    goal_outcome: Option<&'static str>,
    goal_stopped: bool,
    response: Option<String>,
    turn_reason: String,
}
pub struct QueryActivity {
    id: u64,
    ctx: Context,
    agent: Arc<dyn Agent>,
    transport: Arc<dyn JsonRpcTransportPeer>,
    state: Mutex<State>,
    disposers: Mutex<Vec<Disposer>>,
    on_close: Box<dyn Fn(u64) + Send + Sync>,
}
impl QueryActivity {
    pub fn new(
        ctx: Context,
        agent: Arc<dyn Agent>,
        transport: Arc<dyn JsonRpcTransportPeer>,
        signal: AbortSignal,
        on_close: impl Fn(u64) + Send + Sync + 'static,
    ) -> Result<Arc<Self>, BoxError> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let this = Arc::new(Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            ctx: ctx.clone(),
            agent,
            transport,
            state: Mutex::new(State {
                turn_reason: "completed".into(),
                ..State::default()
            }),
            disposers: Mutex::new(Vec::new()),
            on_close: Box::new(on_close),
        });
        let weak = Arc::downgrade(&this);
        lock(&this.disposers).push(ctx.on_contained::<SessionEventEmitted>(move |event| {
            let Some(this) = weak.upgrade() else {
                return Ok(());
            };
            if &event.session != this.agent.session() {
                return Ok(());
            }
            {
                let mut s = lock(&this.state);
                let own = s.result.is_none()
                    && QUERY_INVOCATION
                        .try_with(|id| *id == this.id)
                        .unwrap_or(false);
                let data = event.event.data();
                let tag = event.event.event_type();
                if own && tag == "command/run" && s.command_id.is_none() {
                    s.command_id = data["commandId"].as_str().map(str::to_owned);
                }
                if own
                    && tag == "agent/inbox/spliced"
                    && data["inserted"].as_array().is_some_and(|messages| {
                        messages.iter().any(|m| m["source"]["kind"] == "user")
                    })
                {
                    s.input = true;
                }
                if tag == "goal/change" {
                    if own && matches!(data["operation"].as_str(), Some("create" | "resume")) {
                        s.goal_id = data["goal"].get("id").cloned();
                    }
                    if data["operation"] == "clear" {
                        if data["cleared"].get("id") == s.goal_id.as_ref() && s.goal_id.is_some() {
                            s.goal_outcome.get_or_insert("aborted");
                        }
                    } else if data["goal"].get("id") == s.goal_id.as_ref()
                        && s.goal_id.is_some()
                        && data["goal"]["phase"] != "active"
                    {
                        let outcome = match data["goal"]["phase"].as_str() {
                            Some("complete") => "completed",
                            Some("paused") => "aborted",
                            _ => "error",
                        };
                        s.goal_outcome.get_or_insert(outcome);
                    }
                }
                if s.input || s.goal_id.is_some() {
                    if tag == "assistant/message" {
                        s.response = Some(
                            data["message"]["content"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter(|b| b["type"] == "text")
                                .filter_map(|b| b["text"].as_str())
                                .collect::<String>(),
                        );
                    }
                    if tag == "turn/end" {
                        s.turn_reason = data["reason"]["kind"].as_str().unwrap_or_default().into();
                    }
                }
            }
            this.check()
        })?);
        let weak = Arc::downgrade(&this);
        lock(&this.disposers).push(ctx.on_contained::<GoalActivationChanged>(move |event| {
            let Some(this) = weak.upgrade() else {
                return Ok(());
            };
            let matches = {
                let s = lock(&this.state);
                event.session_id == *this.agent.session().id()
                    && event.goal.as_ref().is_some_and(|goal| {
                        s.goal_id.as_ref().and_then(Value::as_str) == Some(goal.id.as_str())
                            && goal.activation == GoalActivation::Disarmed
                    })
            };
            let active = if matches {
                match this.ctx.get::<Goals>() {
                    Some(goals) => goals
                        .get(&this.agent)?
                        .is_some_and(|goal| goal.goal.phase == GoalPhase::Active),
                    None => false,
                }
            } else {
                false
            };
            if active {
                lock(&this.state).goal_stopped = true;
                this.check()?;
            }
            Ok(())
        })?);
        let weak = Arc::downgrade(&this);
        lock(&this.disposers).push(ctx.on_contained::<AgentStatusChanged>(move |event| {
            if let Some(this) = weak.upgrade()
                && Arc::ptr_eq(&this.agent, &event.agent)
            {
                this.check()?;
            }
            Ok(())
        })?);
        let weak = Arc::downgrade(&this);
        lock(&this.disposers).push(ctx.on_contained::<AgentError>(move |event| {
            if let Some(this) = weak.upgrade()
                && Arc::ptr_eq(&this.agent, &event.agent)
            {
                let mut s = lock(&this.state);
                if s.input || s.goal_id.is_some() {
                    s.turn_reason = "error".into();
                }
            }
            Ok(())
        })?);
        let weak = Arc::downgrade(&this);
        lock(&this.disposers).push(ctx.on_contained::<AgentDisposed>(move |event| {
            if let Some(this) = weak.upgrade()
                && Arc::ptr_eq(&this.agent, &event.agent)
            {
                this.finish(
                    "error",
                    "The Session Agent was disposed before the input finished.",
                )?;
            }
            Ok(())
        })?);
        let weak = Arc::downgrade(&this);
        lock(&this.disposers).push(ctx.spawn("jsonrpc.queryActivity.abort", move |token| async move {
            tokio::select! { _ = token.cancelled() => {}, _ = signal.cancelled() => { if let Some(this) = weak.upgrade() { this.close(); } } }
            Ok(())
        })?);
        Ok(this)
    }
    pub fn id(&self) -> u64 {
        self.id
    }
    pub(crate) fn session_id(&self) -> &hoh_session::SessionId {
        self.agent.id()
    }
    pub fn accept(self: &Arc<Self>, execution: CommandExecution) -> Result<(), BoxError> {
        let input = {
            let mut s = lock(&self.state);
            s.command_id = Some(execution.command_id.to_string());
            s.result = Some(execution.result);
            s.input
        };
        if input {
            let weak = Arc::downgrade(self);
            let agent = self.agent.clone();
            lock(&self.disposers).push(self.ctx.spawn("jsonrpc.queryActivity.idle", move |token| async move {
                tokio::select! { _ = token.cancelled() => {}, _ = agent.when_idle() => {
                    if let Some(this) = weak.upgrade() { if lock(&this.state).closed { return Ok(()); } lock(&this.state).input_idle = true; this.check()?; }
                } }
                Ok(())
            })?);
        }
        self.check()
    }
    pub fn goal_unavailable(&self) -> Result<(), BoxError> {
        if lock(&self.state).goal_id.is_some() {
            self.finish(
                "error",
                "The goal service was unloaded before the goal finished.",
            )?;
        }
        Ok(())
    }
    pub fn close(&self) {
        {
            let mut state = lock(&self.state);
            if state.closed {
                return;
            }
            state.closed = true;
        }
        self.cleanup();
    }
    fn cleanup(&self) {
        for disposer in std::mem::take(&mut *lock(&self.disposers)) {
            drop(disposer.dispose());
        }
        (self.on_close)(self.id);
    }
    fn check(&self) -> Result<(), BoxError> {
        let finish = {
            let s = lock(&self.state);
            if s.closed {
                return Ok(());
            }
            match &s.result {
                None => return Ok(()),
                Some(CommandResult::Error { text }) => ("error".into(), text.clone()),
                Some(CommandResult::Success { text, .. }) => {
                    if s.input && !s.input_idle {
                        return Ok(());
                    }
                    if s.goal_id.is_some() {
                        let Some(outcome) = s.goal_outcome.or(s.goal_stopped.then_some("error"))
                        else {
                            return Ok(());
                        };
                        if self.agent.status() != AgentStatus::Idle {
                            return Ok(());
                        }
                        let reason = if outcome == "completed" {
                            s.turn_reason.clone()
                        } else {
                            outcome.into()
                        };
                        let failure = if s.goal_outcome.is_none() {
                            "Goal execution stopped before reaching a terminal state."
                        } else {
                            ""
                        };
                        (
                            reason,
                            s.response
                                .as_ref()
                                .filter(|r| !r.is_empty())
                                .cloned()
                                .unwrap_or_else(|| failure.into()),
                        )
                    } else if s.input {
                        (
                            s.turn_reason.clone(),
                            s.response.clone().unwrap_or_default(),
                        )
                    } else {
                        ("completed".into(), text.clone().unwrap_or_default())
                    }
                }
            }
        };
        self.finish(&finish.0, &finish.1)
    }
    fn finish(&self, reason: &str, response: &str) -> Result<(), BoxError> {
        let id = {
            let mut state = lock(&self.state);
            if state.closed {
                return Ok(());
            }
            state.closed = true;
            state.command_id.clone()
        };
        // Source builds finishReason before finalResponse (opposite the protocol declaration).
        let result = id.map_or(Ok(()), |id| self.transport.notify("session.query.completed", Some(json!({"sessionId":self.agent.session().id(), "commandId":id, "finishReason":reason, "finalResponse":response}))));
        self.cleanup();
        result
    }
}
