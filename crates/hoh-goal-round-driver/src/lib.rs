//! Same-session goal-round admission driver.

mod prompt;

pub use prompt::render_goal_round_prompt;

use futures_util::FutureExt;
use hoh_agent::{
    Agent, AgentCreated, AgentDisposed, AgentError, AgentInboxClaimed, AgentInboxDiscarded,
    AgentInboxInserted, AgentPreStep, AgentStatus, AgentStatusChanged, Agents, CancelOptions,
    InboxTarget, PreStepDecision,
};
use hoh_error::BoxError;
use hoh_goal::{
    GoalActivation, GoalBlockReason, GoalChangedEvent, GoalOperation, GoalRef, GoalService,
    GoalView, Goals,
};
use hoh_kernel::{Cfg, Context, FiberState, Inject, Next, NoConfig, Plugin, Service, Undo, Undos};
use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, UserMessage, create_user_message};
use hoh_session::{AgentCancelCause, SessionEventEmitted, Sessions};
use hoh_subagent::Subagents;
use serde_json::{Map, Value};
use std::{
    any::Any,
    panic::AssertUnwindSafe,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Notify;

#[derive(Clone, Debug, PartialEq, Eq)]
struct RoundIdentity {
    goal_id: hoh_goal::GoalId,
    revision: u64,
    round: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttemptPhase {
    Queued,
    Claimed,
    Admitted,
}

#[derive(Clone, Debug)]
struct RoundAttempt {
    identity: RoundIdentity,
    message_id: hoh_llm::MessageId,
    content: Vec<ContentBlock>,
    phase: AttemptPhase,
    cancelled: bool,
    stale: bool,
}

struct Run {
    finished: AtomicBool,
    done: Notify,
}

impl Run {
    fn new() -> Self {
        Self {
            finished: AtomicBool::new(false),
            done: Notify::new(),
        }
    }

    fn finish(&self) {
        self.finished.store(true, Ordering::Release);
        self.done.notify_waiters();
    }

    async fn wait(&self) {
        loop {
            let notified = self.done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.finished.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}

#[derive(Default)]
struct StateData {
    attempt: Option<RoundAttempt>,
    competing_queued: bool,
    needs_checkpoint: bool,
    requested: bool,
    run: Option<Arc<Run>>,
    stopping: bool,
}

struct DriverState {
    agent: Arc<dyn Agent>,
    data: Mutex<StateData>,
}

#[derive(Inject)]
pub struct GoalRoundDriverDeps {
    agents: Service<Agents>,
    goals: Service<Goals>,
    sessions: Service<Sessions>,
}

struct Driver {
    ctx: Context,
    agents: Service<Agents>,
    goals: GoalService,
    sessions: Service<Sessions>,
    states: Mutex<Vec<Arc<DriverState>>>,
    unloading: AtomicBool,
    unload: Notify,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn render_error(error: &(dyn std::error::Error + 'static)) -> String {
    error.to_string()
}

fn render_panic(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&'static str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("task panicked")
}

fn source_identity(source: &MessageSource) -> Option<RoundIdentity> {
    if source.kind != "goal" {
        return None;
    }
    let goal_id = source.data.get("goalId")?.as_str()?.into();
    let revision = source.data.get("revision")?.as_u64()?;
    let round = source.data.get("round")?.as_u64()?;
    (round > 0).then_some(RoundIdentity {
        goal_id,
        revision,
        round,
    })
}

fn is_goal_round_source(source: &MessageSource) -> bool {
    source.kind == "goal"
        && source
            .data
            .get("round")
            .and_then(Value::as_f64)
            .is_some_and(|round| round > 0.0)
}

fn is_round_zero(source: &MessageSource) -> bool {
    source.kind == "goal" && source.data.get("round").and_then(Value::as_f64) == Some(0.0)
}

fn round_source(identity: &RoundIdentity) -> MessageSource {
    MessageSource {
        kind: "goal".into(),
        data: Map::from_iter([
            (
                "goalId".into(),
                Value::String(identity.goal_id.as_str().to_owned()),
            ),
            ("revision".into(), Value::from(identity.revision)),
            ("round".into(), Value::from(identity.round)),
        ]),
    }
}

fn same_round(source: &MessageSource, identity: &RoundIdentity) -> bool {
    source_identity(source).as_ref() == Some(identity)
}

fn same_queued(message: &UserMessage, attempt: &RoundAttempt) -> bool {
    message.content == attempt.content && same_round(&message.source, &attempt.identity)
}

fn take_attempt_with_id(
    data: &mut StateData,
    message_id: &hoh_llm::MessageId,
) -> Option<RoundAttempt> {
    data.attempt
        .as_ref()
        .is_some_and(|attempt| &attempt.message_id == message_id)
        .then(|| data.attempt.take())
        .flatten()
}

fn goal_ref(goal: &GoalView) -> GoalRef {
    goal.as_ref()
}

impl Driver {
    fn state_for(self: &Arc<Self>, agent: &Arc<dyn Agent>) -> Arc<DriverState> {
        let mut states = lock(&self.states);
        if let Some(state) = states.iter().find(|state| Arc::ptr_eq(&state.agent, agent)) {
            return state.clone();
        }
        let state = Arc::new(DriverState {
            agent: agent.clone(),
            data: Mutex::new(StateData::default()),
        });
        states.push(state.clone());
        state
    }

    fn retire(&self, agent: &Arc<dyn Agent>) {
        lock(&self.states).retain(|state| !Arc::ptr_eq(&state.agent, agent));
    }

    fn current_goal(&self, state: &DriverState) -> Result<Option<GoalView>, BoxError> {
        let Some(live) = self.agents.get(state.agent.id()) else {
            return Ok(None);
        };
        if !Arc::ptr_eq(&live, &state.agent) {
            return Ok(None);
        }
        self.goals.get(&state.agent)
    }

    fn ready_to_drive(&self, state: &DriverState) -> bool {
        if self.ctx.fiber().state() != FiberState::Active
            || state.agent.status() != AgentStatus::Idle
        {
            return false;
        }
        let data = lock(&state.data);
        if data.stopping || data.competing_queued {
            return false;
        }
        self.agents
            .get(state.agent.id())
            .is_some_and(|live| Arc::ptr_eq(&live, &state.agent))
    }

    fn ready_after_checkpoint(&self, state: &DriverState) -> bool {
        self.ready_to_drive(state) && !lock(&state.data).needs_checkpoint
    }

    fn disarm(&self, state: &DriverState) {
        let result = self.current_goal(state).and_then(|goal| {
            if goal.is_some_and(|goal| goal.activation == GoalActivation::Armed) {
                self.goals.disarm(&state.agent)?;
            }
            Ok(())
        });
        if let Err(error) = result {
            self.ctx.logger().warn(format_args!(
                "goal-round-driver: could not disarm agent \"{}\": {}",
                state.agent.id(),
                render_error(error.as_ref())
            ));
        }
    }

    fn restore_other_claimed(
        &self,
        agent: &Arc<dyn Agent>,
        messages: &[UserMessage],
        message_id: &hoh_llm::MessageId,
    ) -> Result<(), BoxError> {
        let mut retained = messages
            .iter()
            .filter(|message| &message.id != message_id && !is_round_zero(&message.source))
            .cloned()
            .collect::<Vec<_>>();
        retained.reverse();
        for message in retained {
            let already_queued = agent
                .inbox()
                .next_step()
                .iter()
                .chain(agent.inbox().next_turn().iter())
                .any(|candidate| candidate.id == message.id);
            if !already_queued {
                agent.inbox().prepend(InboxTarget::NextStep, message)?;
            }
        }
        Ok(())
    }

    async fn wait_for_descendants(&self, state: &DriverState) -> Result<bool, BoxError> {
        let Some(subagents) = self.ctx.get::<Subagents>() else {
            return Ok(true);
        };
        let notified = self.unload.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.unloading.load(Ordering::Acquire) {
            return Ok(false);
        }
        tokio::select! {
            result = subagents.wait_for_tree_work(&state.agent) => {
                result.map(|_| true)
            }
            () = &mut notified => Ok(false),
        }
    }

    async fn drive(self: &Arc<Self>, state: &Arc<DriverState>) -> Result<(), BoxError> {
        if !self.ready_to_drive(state) {
            return Ok(());
        }

        let needs_checkpoint = {
            let mut data = lock(&state.data);
            let value = data.needs_checkpoint;
            if value {
                data.needs_checkpoint = false;
            }
            value
        };
        if needs_checkpoint {
            if let Err(error) = self.sessions.flush(state.agent.session()).await {
                self.ctx.logger().warn(format_args!(
                    "goal-round-driver: durability checkpoint failed for agent \"{}\": {}",
                    state.agent.id(),
                    render_error(error.as_ref())
                ));
                self.disarm(state);
                return Ok(());
            }
            if !self.ready_after_checkpoint(state) {
                return Ok(());
            }
        }

        {
            let mut data = lock(&state.data);
            if data.attempt.take().is_some() {
                data.needs_checkpoint = true;
                data.requested = true;
                return Ok(());
            }
        }

        let Some(goal) = self.current_goal(state)? else {
            return Ok(());
        };
        if goal.goal.phase != hoh_goal::GoalPhase::Active
            || goal.activation != GoalActivation::Armed
        {
            return Ok(());
        }
        if !self.wait_for_descendants(state).await? || !self.ready_after_checkpoint(state) {
            return Ok(());
        }
        if goal.rounds_started >= goal.goal.max_goal_rounds {
            self.goals.block(
                &state.agent,
                &goal_ref(&goal),
                GoalBlockReason {
                    code: "round-limit".into(),
                    message: format!(
                        "Goal reached its configured limit of {} rounds.",
                        goal.goal.max_goal_rounds
                    ),
                },
            )?;
            return Ok(());
        }

        let identity = RoundIdentity {
            goal_id: goal.goal.id.clone(),
            revision: goal.goal.revision,
            round: goal.rounds_started + 1,
        };
        let content = render_goal_round_prompt(&goal, identity.round);
        let message = create_user_message(NewUserMessage {
            content: content.clone(),
            source: round_source(&identity),
        });
        let attempt = RoundAttempt {
            identity: identity.clone(),
            message_id: message.id.clone(),
            content,
            phase: AttemptPhase::Queued,
            cancelled: false,
            stale: false,
        };
        lock(&state.data).attempt = Some(attempt);
        if let Err(error) = state.agent.followup(message) {
            lock(&state.data).attempt = None;
            self.ctx.logger().warn(format_args!(
                "goal-round-driver: could not queue round {} for agent \"{}\": {}",
                identity.round,
                state.agent.id(),
                render_error(error.as_ref())
            ));
            let latest = self.current_goal(state)?;
            if let Some(latest) = latest.filter(|latest| {
                latest.goal.id == identity.goal_id
                    && latest.goal.revision == identity.revision
                    && latest.goal.phase == hoh_goal::GoalPhase::Active
                    && latest.activation == GoalActivation::Armed
            }) {
                self.goals.block(
                    &state.agent,
                    &goal_ref(&latest),
                    GoalBlockReason {
                        code: "queue-failed".into(),
                        message: format!(
                            "Could not queue goal round {}: {}",
                            identity.round,
                            render_error(error.as_ref())
                        ),
                    },
                )?;
            }
        }
        Ok(())
    }

    fn request_drive(self: &Arc<Self>, state: &Arc<DriverState>) {
        let run = {
            let mut data = lock(&state.data);
            if data.stopping {
                return;
            }
            data.requested = true;
            if data.run.is_some() {
                return;
            }
            let run = Arc::new(Run::new());
            data.run = Some(run.clone());
            run
        };

        let driver = self.clone();
        let subject = state.clone();
        let operation = async move {
            loop {
                let requested = {
                    let mut data = lock(&subject.data);
                    if data.stopping || !data.requested {
                        false
                    } else {
                        data.requested = false;
                        true
                    }
                };
                if !requested {
                    break;
                }
                if let Err(error) = driver.drive(&subject).await {
                    driver.ctx.logger().warn(format_args!(
                        "goal-round-driver: driver failed for agent \"{}\": {}",
                        subject.agent.id(),
                        render_error(error.as_ref())
                    ));
                    driver.disarm(&subject);
                }
            }
        };
        let operation = match self.agents.without_initiator(operation) {
            Ok(operation) => operation,
            Err(error) => {
                let mut data = lock(&state.data);
                if data
                    .run
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &run))
                {
                    data.run = None;
                }
                drop(data);
                self.ctx.logger().warn(format_args!(
                    "goal-round-driver: could not start driver for agent \"{}\": {}",
                    state.agent.id(),
                    render_error(error.as_ref())
                ));
                self.disarm(state);
                run.finish();
                return;
            }
        };
        let driver = self.clone();
        let subject = state.clone();
        let retiring = run.clone();
        drop(tokio::spawn(async move {
            if let Err(payload) = AssertUnwindSafe(operation).catch_unwind().await {
                driver.ctx.logger().warn(format_args!(
                    "goal-round-driver: driver task rejected for agent \"{}\": {}",
                    subject.agent.id(),
                    render_panic(payload.as_ref())
                ));
                driver.disarm(&subject);
            }
            let restart = {
                let mut data = lock(&subject.data);
                if data
                    .run
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &retiring))
                {
                    data.run = None;
                }
                data.requested && !data.stopping
            };
            retiring.finish();
            if restart {
                driver.request_drive(&subject);
            }
        }));
    }

    fn valid_reservation(
        &self,
        state: &DriverState,
        message: &UserMessage,
        identity: &RoundIdentity,
    ) -> Result<bool, BoxError> {
        // The source reads the current goal before evaluating the boolean fence. Preserve that
        // ordering so a projection failure is contained and disarms even for a stale attempt.
        let goal = self.current_goal(state)?;
        let attempt_valid = {
            let data = lock(&state.data);
            !data.stopping
                && data.attempt.as_ref().is_some_and(|attempt| {
                    attempt.phase == AttemptPhase::Claimed
                        && !attempt.stale
                        && same_queued(message, attempt)
                })
        };
        if self.ctx.fiber().state() != FiberState::Active || !attempt_valid {
            return Ok(false);
        }
        Ok(goal.is_some_and(|goal| {
            goal.goal.id == identity.goal_id
                && goal.goal.revision == identity.revision
                && goal.goal.phase == hoh_goal::GoalPhase::Active
                && goal.activation == GoalActivation::Armed
                && identity.round == goal.rounds_started + 1
        }))
    }

    async fn pre_step(
        self: Arc<Self>,
        event: Arc<AgentPreStep>,
        next: Next<PreStepDecision>,
    ) -> Result<PreStepDecision, BoxError> {
        let Some(submitted) = event
            .messages
            .iter()
            .find(|message| is_goal_round_source(&message.source))
            .cloned()
        else {
            return next.run().await;
        };
        let state = self.state_for(&event.agent);
        let Some(identity) = source_identity(&submitted.source) else {
            self.restore_other_claimed(&event.agent, &event.messages, &submitted.id)?;
            self.request_drive(&state);
            return Ok(PreStepDecision::Reject);
        };
        let valid = match self.valid_reservation(&state, &submitted, &identity) {
            Ok(valid) => valid,
            Err(error) => {
                self.ctx.logger().warn(format_args!(
                    "goal-round-driver: pre-step check failed for agent \"{}\": {}",
                    event.agent.id(),
                    render_error(error.as_ref())
                ));
                self.disarm(&state);
                false
            }
        };
        if !valid {
            {
                let mut data = lock(&state.data);
                if data
                    .attempt
                    .as_ref()
                    .is_some_and(|attempt| attempt.identity == identity)
                {
                    if let Some(attempt) = data.attempt.as_mut() {
                        attempt.stale = true;
                    }
                    data.attempt = None;
                }
            }
            self.restore_other_claimed(&event.agent, &event.messages, &submitted.id)?;
            self.request_drive(&state);
            return Ok(PreStepDecision::Reject);
        }

        let decision = match next.run().await {
            Ok(decision) => decision,
            Err(error) => {
                if event.signal.aborted() {
                    return Err(error);
                }
                lock(&state.data).attempt = None;
                self.request_drive(&state);
                return Err(error);
            }
        };
        if event.signal.aborted() {
            if let PreStepDecision::Enter { messages, .. } = &decision {
                self.restore_other_claimed(&event.agent, messages, &submitted.id)?;
            }
            return Ok(decision);
        }
        if decision == PreStepDecision::Reject {
            lock(&state.data).attempt = None;
            if let Some(goal) = self.current_goal(&state)?.filter(|goal| {
                goal.goal.id == identity.goal_id
                    && goal.goal.revision == identity.revision
                    && goal.goal.phase == hoh_goal::GoalPhase::Active
                    && goal.activation == GoalActivation::Armed
            }) {
                self.goals.block(
                    &event.agent,
                    &goal_ref(&goal),
                    GoalBlockReason {
                        code: "prompt-rejected".into(),
                        message: "Goal round was rejected before entering its step.".into(),
                    },
                )?;
            }
            return Ok(decision);
        }

        let valid = match self.valid_reservation(&state, &submitted, &identity) {
            Ok(valid) => valid,
            Err(error) => {
                self.ctx.logger().warn(format_args!(
                    "goal-round-driver: post-decision check failed for agent \"{}\": {}",
                    event.agent.id(),
                    render_error(error.as_ref())
                ));
                self.disarm(&state);
                false
            }
        };
        let PreStepDecision::Enter { messages, .. } = decision else {
            return Ok(decision);
        };
        if !valid {
            lock(&state.data).attempt = None;
            self.restore_other_claimed(&event.agent, &messages, &submitted.id)?;
            self.request_drive(&state);
            return Ok(PreStepDecision::Reject);
        }
        Ok(PreStepDecision::Enter {
            messages,
            starts_request_series: true,
        })
    }

    fn install(self: &Arc<Self>, undos: &Undos) -> Result<(), BoxError> {
        let driver = self.clone();
        undos.push(self.ctx.on_contained::<AgentError>(move |event| {
            let state = driver.state_for(&event.agent);
            driver.disarm(&state);
            Ok(())
        })?)?;

        let driver = self.clone();
        undos.push(self.ctx.on_contained::<AgentDisposed>(move |event| {
            driver.retire(&event.agent);
            Ok(())
        })?)?;

        let driver = self.clone();
        undos.push(self.ctx.on_serial::<AgentCreated, (), _>(move |event| {
            let driver = driver.clone();
            async move {
                let state = driver.state_for(&event.agent);
                let mut data = lock(&state.data);
                data.attempt = None;
                data.competing_queued = false;
                data.needs_checkpoint = false;
                Ok(None)
            }
        })?)?;

        let driver = self.clone();
        undos.push(self.ctx.on_contained::<AgentStatusChanged>(move |event| {
            if event.status != AgentStatus::Idle {
                return Ok(());
            }
            let state = driver.state_for(&event.agent);
            let attempt = {
                let mut data = lock(&state.data);
                data.competing_queued = false;
                data.attempt.clone()
            };
            let goal = driver.current_goal(&state)?;
            let pause = attempt.as_ref().is_some_and(|attempt| {
                (matches!(attempt.phase, AttemptPhase::Queued | AttemptPhase::Claimed)
                    || attempt.cancelled)
                    && goal.as_ref().is_some_and(|goal| {
                        goal.goal.phase == hoh_goal::GoalPhase::Active
                            && goal.activation == GoalActivation::Armed
                            && attempt.identity.goal_id == goal.goal.id
                            && attempt.identity.revision == goal.goal.revision
                    })
            });
            if pause || attempt.as_ref().is_some_and(|a| a.phase == AttemptPhase::Queued) {
                let settled = {
                    let mut data = lock(&state.data);
                    attempt
                        .as_ref()
                        .and_then(|observed| take_attempt_with_id(&mut data, &observed.message_id))
                };
                let Some(attempt) = settled else {
                    driver.request_drive(&state);
                    return Ok(());
                };
                let settlement = (|| -> Result<(), BoxError> {
                    if attempt.phase == AttemptPhase::Queued {
                        event.agent.inbox().remove(&attempt.message_id)?;
                    }
                    if pause && let Some(goal) = goal.as_ref() {
                        driver.goals.pause(&event.agent, &goal_ref(goal))?;
                    }
                    Ok(())
                })();
                if let Err(error) = settlement {
                    driver.ctx.logger().warn(format_args!(
                        "goal-round-driver: could not settle cancelled goal round for agent \"{}\": {}",
                        event.agent.id(),
                        render_error(error.as_ref())
                    ));
                    driver.disarm(&state);
                }
            }
            driver.request_drive(&state);
            Ok(())
        })?)?;

        let driver = self.clone();
        undos.push(self.ctx.on_contained::<GoalChangedEvent>(move |event| {
            let state = driver.state_for(&event.agent);
            lock(&state.data).needs_checkpoint = true;
            if event.change.operation == GoalOperation::Pause
                && event.agent.status() == AgentStatus::Running
            {
                let initiated_here = driver
                    .agents
                    .current_initiator()?
                    .is_some_and(|agent| Arc::ptr_eq(&agent, &event.agent));
                if !initiated_here {
                    event
                        .agent
                        .cancel(AgentCancelCause::User, CancelOptions { keep_inbox: true })?;
                }
            }
            driver.request_drive(&state);
            Ok(())
        })?)?;

        let driver = self.clone();
        undos.push(self.ctx.on_contained::<AgentInboxInserted>(move |event| {
            if !event
                .agent
                .inbox()
                .next_turn()
                .iter()
                .any(|message| message.id == event.message.id)
            {
                return Ok(());
            }
            let state = driver.state_for(&event.agent);
            let mut data = lock(&state.data);
            if data
                .attempt
                .as_ref()
                .is_some_and(|attempt| same_queued(&event.message, attempt))
            {
                return Ok(());
            }
            data.competing_queued = true;
            if let Some(attempt) = data
                .attempt
                .as_mut()
                .filter(|a| a.phase == AttemptPhase::Queued)
            {
                attempt.stale = true;
            }
            Ok(())
        })?)?;

        let driver = self.clone();
        undos.push(self.ctx.on_contained::<AgentInboxClaimed>(move |event| {
            let state = driver.state_for(&event.agent);
            let mut data = lock(&state.data);
            if let Some(attempt) = data
                .attempt
                .as_mut()
                .filter(|attempt| same_queued(&event.message, attempt))
            {
                attempt.phase = AttemptPhase::Claimed;
            }
            Ok(())
        })?)?;

        let driver = self.clone();
        undos.push(self.ctx.on_contained::<AgentInboxDiscarded>(move |event| {
            let state = driver.state_for(&event.agent);
            let mut data = lock(&state.data);
            if let Some(attempt) = data
                .attempt
                .as_mut()
                .filter(|attempt| same_queued(&event.message, attempt))
            {
                attempt.cancelled = true;
            }
            Ok(())
        })?)?;

        let driver = self.clone();
        undos.push(self.ctx.on_contained::<SessionEventEmitted>(move |event| {
            let Some(agent) = driver.agents.get(event.session.id()) else {
                return Ok(());
            };
            if agent.session() != &event.session {
                return Ok(());
            }
            let state = driver.state_for(&agent);
            match event.event.event_type() {
                "user/message" => {
                    let id = event.event.data().get("id").and_then(Value::as_str);
                    let mut data = lock(&state.data);
                    if let Some(attempt) = data
                        .attempt
                        .as_mut()
                        .filter(|attempt| Some(attempt.message_id.as_str()) == id)
                    {
                        attempt.phase = AttemptPhase::Admitted;
                    }
                }
                "turn/end" => {
                    let reason = &event.event.data()["reason"];
                    match reason.get("kind").and_then(Value::as_str) {
                        Some("max-tokens") => driver.disarm(&state),
                        Some("aborted")
                            if reason
                                .get("reason")
                                .and_then(|reason| reason.get("kind"))
                                .and_then(Value::as_str)
                                == Some("disposed") =>
                        {
                            driver.disarm(&state);
                        }
                        Some("aborted") => {
                            let mut data = lock(&state.data);
                            if let Some(attempt) = data.attempt.as_mut().filter(|attempt| {
                                matches!(
                                    attempt.phase,
                                    AttemptPhase::Claimed | AttemptPhase::Admitted
                                )
                            }) {
                                attempt.cancelled = true;
                            } else {
                                drop(data);
                                driver.disarm(&state);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            Ok(())
        })?)?;

        let driver = self.clone();
        undos.push(self.ctx.on_waterfall::<AgentPreStep, PreStepDecision, _>(
            move |event, next| driver.clone().pre_step(event, next),
        )?)?;
        Ok(())
    }

    async fn stop(self: Arc<Self>) -> Result<(), BoxError> {
        self.unloading.store(true, Ordering::Release);
        self.unload.notify_waiters();
        let states = lock(&self.states).clone();
        let mut runs = Vec::new();
        let mut idle = Vec::new();
        for state in &states {
            let attempt = {
                let mut data = lock(&state.data);
                data.stopping = true;
                if let Some(attempt) = data.attempt.as_mut() {
                    attempt.stale = true;
                }
                if let Some(run) = &data.run {
                    runs.push(run.clone());
                }
                data.attempt.clone()
            };
            self.disarm(state);
            if attempt.is_some() && state.agent.status() == AgentStatus::Running {
                match state
                    .agent
                    .cancel(AgentCancelCause::Parent, CancelOptions::default())
                {
                    Ok(()) => idle.push(state.agent.when_idle()),
                    Err(error) => {
                        self.ctx.logger().warn(format_args!(
                            "goal-round-driver: could not cancel agent \"{}\" during teardown: {}",
                            state.agent.id(),
                            render_error(error.as_ref())
                        ));
                    }
                }
            }
        }
        for run in runs {
            run.wait().await;
        }
        for wait in idle {
            wait.await;
        }
        lock(&self.states).clear();
        Ok(())
    }
}

/// Same-session automatic goal continuation plugin.
pub struct GoalRoundDriver;

impl Plugin for GoalRoundDriver {
    const NAME: &'static str = "goal-round-driver";
    type Config = NoConfig;
    type Deps = GoalRoundDriverDeps;

    async fn apply(
        ctx: Context,
        deps: GoalRoundDriverDeps,
        _config: Cfg<NoConfig>,
    ) -> Result<(), BoxError> {
        let driver = Arc::new(Driver {
            ctx: ctx.clone(),
            agents: deps.agents,
            goals: (*deps.goals).clone(),
            sessions: deps.sessions,
            states: Mutex::new(Vec::new()),
            unloading: AtomicBool::new(false),
            unload: Notify::new(),
        });
        let lifecycle = driver.clone();
        ctx.try_effect(
            "goal-round-driver lifecycle",
            move |undos| -> Result<(), BoxError> {
                lifecycle.install(undos)?;
                for agent in lifecycle.agents.list() {
                    let state = lifecycle.state_for(&agent);
                    lifecycle.disarm(&state);
                }
                let stopping = lifecycle.clone();
                undos.push(Undo::future(move || stopping.stop()))?;
                Ok(())
            },
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt(message_id: &str) -> RoundAttempt {
        RoundAttempt {
            identity: RoundIdentity {
                goal_id: "goal".into(),
                revision: 1,
                round: 1,
            },
            message_id: message_id.into(),
            content: vec![],
            phase: AttemptPhase::Queued,
            cancelled: false,
            stale: false,
        }
    }

    #[test]
    fn idle_settlement_cannot_clear_a_newer_reservation() {
        let mut data = StateData {
            attempt: Some(attempt("new-reservation")),
            ..StateData::default()
        };

        assert!(take_attempt_with_id(&mut data, &"old-reservation".into()).is_none());
        assert_eq!(
            data.attempt
                .as_ref()
                .map(|attempt| attempt.message_id.as_str()),
            Some("new-reservation")
        );
    }
}
