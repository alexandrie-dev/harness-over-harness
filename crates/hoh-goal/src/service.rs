use crate::{goal_change_ref, goal_projection_definition, types::*};
use hoh_agent::{Agent, AgentCreated, AgentSubjectEvent, Agents, agent_events};
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Cfg, Contained, Context, Inject, ListenerFailure, Plugin, Service};
use hoh_session::{Session, SessionEventEmitted};
use hoh_session_projection::{ProjectionHandle, SessionProjections};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

pub struct GoalError;
impl GoalError {
    pub const NAME: &'static str = "GoalError";
    #[allow(clippy::new_ret_no_self)]
    pub fn new(message: impl Into<String>, code: impl Into<String>) -> HarnessError {
        HarnessError::named(Self::NAME, message, code)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_max_goal_rounds")]
    pub default_max_goal_rounds: f64,
}
const fn default_max_goal_rounds() -> f64 {
    256.0
}
impl Default for Config {
    fn default() -> Self {
        Self {
            default_max_goal_rounds: default_max_goal_rounds(),
        }
    }
}

#[derive(Clone, Debug)]
struct PendingActivation {
    offset: u64,
    activation: GoalActivation,
}

#[derive(Clone, Debug)]
struct RuntimeState {
    activation: GoalActivation,
    pending_activation: Option<PendingActivation>,
}
impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            activation: GoalActivation::Disarmed,
            pending_activation: None,
        }
    }
}

struct RuntimeEntry {
    session: Session,
    state: RuntimeState,
}

#[derive(Default)]
struct MutationGateState {
    owner: Option<std::thread::ThreadId>,
    depth: usize,
}

#[derive(Default)]
struct MutationGate {
    state: Mutex<MutationGateState>,
    ready: Condvar,
}

struct MutationGuard<'a> {
    gate: &'a MutationGate,
}

impl MutationGate {
    fn enter(&self) -> MutationGuard<'_> {
        let owner = std::thread::current().id();
        let mut state = lock(&self.state);
        while state.owner.is_some_and(|current| current != owner) {
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        state.owner = Some(owner);
        state.depth += 1;
        MutationGuard { gate: self }
    }
}

impl Drop for MutationGuard<'_> {
    fn drop(&mut self) {
        let mut state = lock(&self.gate.state);
        state.depth -= 1;
        if state.depth == 0 {
            state.owner = None;
            self.gate.ready.notify_one();
        }
    }
}

struct MutationEntry {
    session: Session,
    gate: Arc<MutationGate>,
}

struct Inner {
    ctx: Context,
    agents: Service<Agents>,
    projection: ProjectionHandle<GoalProjectionState>,
    default_max_goal_rounds: u64,
    runtime: Mutex<Vec<RuntimeEntry>>,
    mutations: Mutex<Vec<MutationEntry>>,
}

#[derive(Clone)]
pub struct GoalService {
    inner: Arc<Inner>,
}

hoh_kernel::service!(pub Goals: GoalService = "goals");

pub struct GoalChangedEvent {
    pub agent: Arc<dyn Agent>,
    pub change: GoalChanged,
}
hoh_kernel::event!(GoalChangedEvent = "goal/changed", Contained);
impl AgentSubjectEvent for GoalChangedEvent {
    fn set_agent(&mut self, agent: Arc<dyn Agent>) {
        self.agent = agent;
    }
}

hoh_kernel::event!(GoalActivationChanged = "goal/activation-changed", Contained);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn lower_kebab(value: &str) -> bool {
    let mut parts = value.split('-');
    let Some(first) = parts.next() else {
        return false;
    };
    !first.is_empty()
        && first.as_bytes()[0].is_ascii_lowercase()
        && first
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && parts.all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

fn objective(value: &str) -> Result<String, BoxError> {
    let value = hoh_util_values::js_trim(value);
    if value.is_empty() {
        return Err(Box::new(GoalError::new(
            "goal objective must be a non-empty string",
            "GOAL_INVALID_OBJECTIVE",
        )));
    }
    Ok(value.to_owned())
}

fn max_rounds(value: u64) -> Result<u64, BoxError> {
    if value == 0 || value > hoh_session::MAX_SAFE_INTEGER {
        return Err(Box::new(GoalError::new(
            "maxGoalRounds must be a positive safe integer",
            "GOAL_INVALID_MAX_ROUNDS",
        )));
    }
    Ok(value)
}

fn configured_max_rounds(value: f64) -> Result<u64, BoxError> {
    if !value.is_finite()
        || value.fract() != 0.0
        || value < 1.0
        || value > hoh_session::MAX_SAFE_INTEGER as f64
    {
        return Err(Box::new(GoalError::new(
            "maxGoalRounds must be a positive safe integer",
            "GOAL_INVALID_MAX_ROUNDS",
        )));
    }
    Ok(value as u64)
}

fn block_reason(reason: GoalBlockReason) -> Result<GoalBlockReason, BoxError> {
    let message = hoh_util_values::js_trim(&reason.message);
    if !lower_kebab(&reason.code) || message.is_empty() {
        return Err(Box::new(GoalError::new(
            "goal block reason requires a lower-kebab-case code and a non-empty message",
            "GOAL_INVALID_BLOCK_REASON",
        )));
    }
    Ok(GoalBlockReason {
        code: reason.code,
        message: message.to_owned(),
    })
}

impl GoalService {
    pub async fn start(
        ctx: Context,
        agents: Service<Agents>,
        projections: Service<SessionProjections>,
        config: Config,
    ) -> Result<Self, BoxError> {
        let default_max_goal_rounds = configured_max_rounds(config.default_max_goal_rounds)?;
        let projection = projections
            .register(&ctx, goal_projection_definition())
            .await?;
        let service = Self {
            inner: Arc::new(Inner {
                ctx: ctx.clone(),
                agents,
                projection,
                default_max_goal_rounds,
                runtime: Mutex::new(vec![]),
                mutations: Mutex::new(vec![]),
            }),
        };
        {
            let service = service.clone();
            ctx.on_serial::<AgentCreated, (), _>(move |event| {
                let service = service.clone();
                async move {
                    service.set_activation(event.agent.session(), GoalActivation::Disarmed)?;
                    Ok(None)
                }
            })?;
        }
        {
            let service = service.clone();
            ctx.on_contained::<SessionEventEmitted>(move |event| {
                if event.event.event_type() == "goal/change" {
                    let activation = service.with_runtime(&event.session, |runtime| {
                        runtime
                            .pending_activation
                            .as_ref()
                            .filter(|pending| pending.offset == event.event.seq().get())
                            .map_or(GoalActivation::Disarmed, |pending| pending.activation)
                    });
                    service.set_activation(&event.session, activation)?;
                }
                Ok(())
            })?;
        }
        Ok(service)
    }

    fn with_runtime<R>(&self, session: &Session, apply: impl FnOnce(&mut RuntimeState) -> R) -> R {
        let mut entries = lock(&self.inner.runtime);
        let index = entries
            .iter()
            .position(|entry| entry.session == *session)
            .unwrap_or_else(|| {
                entries.push(RuntimeEntry {
                    session: session.clone(),
                    state: RuntimeState::default(),
                });
                entries.len() - 1
            });
        apply(&mut entries[index].state)
    }

    fn runtime(&self, session: &Session) -> RuntimeState {
        self.with_runtime(session, |runtime| runtime.clone())
    }

    fn mutation_gate(&self, session: &Session) -> Arc<MutationGate> {
        let mut entries = lock(&self.inner.mutations);
        if let Some(entry) = entries.iter().find(|entry| entry.session == *session) {
            return entry.gate.clone();
        }
        let gate = Arc::new(MutationGate::default());
        entries.push(MutationEntry {
            session: session.clone(),
            gate: gate.clone(),
        });
        gate
    }

    fn assert_live(&self, agent: &Arc<dyn Agent>) -> Result<(), BoxError> {
        let live = self.inner.agents.get(agent.id());
        if live.as_ref().is_none_or(|live| !Arc::ptr_eq(live, agent)) {
            return Err(Box::new(GoalError::new(
                format!("agent \"{}\" is not live in this registry", agent.id()),
                "GOAL_AGENT_NOT_LIVE",
            )));
        }
        Ok(())
    }

    fn state(&self, session: &Session) -> Result<Option<GoalProjection>, BoxError> {
        let state = self.inner.projection.state_of(session)?;
        if let Some(failure) = &state.failure {
            return Err(Box::new(GoalError::new(
                failure.clone(),
                "GOAL_STATE_CORRUPT",
            )));
        }
        Ok(state.current.clone())
    }

    fn view(&self, state: Option<GoalProjection>, runtime: &RuntimeState) -> Option<GoalView> {
        state.map(|state| GoalView {
            goal: state.goal,
            rounds_started: state.rounds_started,
            created_at: state.created_at,
            updated_at: state.updated_at,
            activation: runtime.activation,
        })
    }

    pub fn get(&self, agent: &Arc<dyn Agent>) -> Result<Option<GoalView>, BoxError> {
        self.assert_live(agent)?;
        Ok(self.view(self.state(agent.session())?, &self.runtime(agent.session())))
    }

    pub fn disarm(&self, agent: &Arc<dyn Agent>) -> Result<Option<GoalView>, BoxError> {
        self.assert_live(agent)?;
        self.set_activation(agent.session(), GoalActivation::Disarmed)?;
        Ok(self.view(self.state(agent.session())?, &self.runtime(agent.session())))
    }

    fn prepare(
        &self,
        agent: &Arc<dyn Agent>,
    ) -> Result<(Option<GoalProjection>, RuntimeState), BoxError> {
        self.assert_live(agent)?;
        Ok((self.state(agent.session())?, self.runtime(agent.session())))
    }

    pub fn create(
        &self,
        agent: &Arc<dyn Agent>,
        request: CreateGoalRequest,
    ) -> Result<GoalView, BoxError> {
        let objective = objective(&request.objective)?;
        let maximum = max_rounds(
            request
                .max_goal_rounds
                .unwrap_or(self.inner.default_max_goal_rounds),
        )?;
        let gate = self.mutation_gate(agent.session());
        let _guard = gate.enter();
        let (state, runtime) = self.prepare(agent)?;
        if let Some(current) = state
            .as_ref()
            .map(|current| &current.goal)
            .filter(|goal| goal.phase != GoalPhase::Complete)
        {
            return Err(Box::new(GoalError::new(
                format!(
                    "goal \"{}\" already exists with phase \"{}\"",
                    current.id, current.phase
                ),
                "GOAL_ALREADY_EXISTS",
            )));
        }
        let now = now_ms();
        let goal = GoalSnapshot {
            id: format!("goal-{}", hoh_util_crypto::random_uuid()).into(),
            revision: 1,
            objective,
            phase: GoalPhase::Active,
            blocked_reason: None,
            max_goal_rounds: maximum,
        };
        self.commit_snapshot(
            agent,
            &runtime,
            GoalOperation::Create,
            goal,
            0,
            now,
            now,
            GoalActivation::Armed,
        )
    }

    fn expect_current(
        &self,
        state: Option<GoalProjection>,
        reference: &GoalRef,
    ) -> Result<GoalProjection, BoxError> {
        let Some(state) = state else {
            return Err(Box::new(GoalError::new(
                "no current goal",
                "GOAL_NOT_FOUND",
            )));
        };
        if reference.id != state.goal.id || reference.revision != state.goal.revision {
            return Err(Box::new(GoalError::new(
                format!(
                    "stale goal ref \"{}\" revision {}; current is \"{}\" revision {}",
                    reference.id, reference.revision, state.goal.id, state.goal.revision
                ),
                "GOAL_STALE_REVISION",
            )));
        }
        Ok(state)
    }

    pub fn edit(
        &self,
        agent: &Arc<dyn Agent>,
        reference: &GoalRef,
        request: EditGoalRequest,
    ) -> Result<GoalView, BoxError> {
        let gate = self.mutation_gate(agent.session());
        let _guard = gate.enter();
        let (state, runtime) = self.prepare(agent)?;
        let state = self.expect_current(state, reference)?;
        if request.objective.is_none() && request.max_goal_rounds.is_none() {
            return Err(Box::new(GoalError::new(
                "goal edit requires objective and/or maxGoalRounds",
                "GOAL_INVALID_EDIT",
            )));
        }
        let mut goal = state.goal.clone();
        goal.revision += 1;
        if let Some(value) = request.objective {
            goal.objective = objective(&value)?;
        }
        if let Some(value) = request.max_goal_rounds {
            goal.max_goal_rounds = max_rounds(value)?;
        }
        self.commit_current(
            agent,
            &state,
            &runtime,
            GoalOperation::Edit,
            goal,
            runtime.activation,
        )
    }

    pub fn pause(&self, agent: &Arc<dyn Agent>, reference: &GoalRef) -> Result<GoalView, BoxError> {
        let gate = self.mutation_gate(agent.session());
        let _guard = gate.enter();
        self.transition(
            agent,
            reference,
            GoalOperation::Pause,
            &[GoalPhase::Active],
            GoalPhase::Paused,
            GoalActivation::Disarmed,
        )
    }

    pub fn resume(
        &self,
        agent: &Arc<dyn Agent>,
        reference: &GoalRef,
    ) -> Result<GoalView, BoxError> {
        let gate = self.mutation_gate(agent.session());
        let _guard = gate.enter();
        let (state, runtime) = self.prepare(agent)?;
        let state = self.expect_current(state, reference)?;
        let current = &state.goal;
        let allowed = [GoalPhase::Active, GoalPhase::Paused, GoalPhase::Blocked];
        if !allowed.contains(&current.phase) {
            return Err(self.transition_error(current, GoalOperation::Resume, &allowed));
        }
        if current.phase == GoalPhase::Active && runtime.activation == GoalActivation::Armed {
            return Err(Box::new(GoalError::new(
                format!("goal \"{}\" is already active and armed", current.id),
                "GOAL_INVALID_TRANSITION",
            )));
        }
        if state.rounds_started >= current.max_goal_rounds {
            return Err(Box::new(GoalError::new(
                format!(
                    "goal \"{}\" exhausted {} goal rounds; increase maxGoalRounds before resuming",
                    current.id, current.max_goal_rounds
                ),
                "GOAL_INVALID_TRANSITION",
            )));
        }
        let goal = self.with_phase(current, GoalPhase::Active);
        self.commit_current(
            agent,
            &state,
            &runtime,
            GoalOperation::Resume,
            goal,
            GoalActivation::Armed,
        )
    }

    pub fn complete(
        &self,
        agent: &Arc<dyn Agent>,
        reference: &GoalRef,
    ) -> Result<GoalView, BoxError> {
        let gate = self.mutation_gate(agent.session());
        let _guard = gate.enter();
        self.transition(
            agent,
            reference,
            GoalOperation::Complete,
            &[GoalPhase::Active, GoalPhase::Paused, GoalPhase::Blocked],
            GoalPhase::Complete,
            GoalActivation::Disarmed,
        )
    }

    pub fn block(
        &self,
        agent: &Arc<dyn Agent>,
        reference: &GoalRef,
        reason: GoalBlockReason,
    ) -> Result<GoalView, BoxError> {
        let gate = self.mutation_gate(agent.session());
        let _guard = gate.enter();
        let (state, runtime) = self.prepare(agent)?;
        let state = self.expect_current(state, reference)?;
        if state.goal.phase != GoalPhase::Active {
            return Err(self.transition_error(
                &state.goal,
                GoalOperation::Block,
                &[GoalPhase::Active],
            ));
        }
        let mut goal = self.with_phase(&state.goal, GoalPhase::Blocked);
        goal.blocked_reason = Some(block_reason(reason)?);
        self.commit_current(
            agent,
            &state,
            &runtime,
            GoalOperation::Block,
            goal,
            GoalActivation::Disarmed,
        )
    }

    pub fn clear(&self, agent: &Arc<dyn Agent>, reference: &GoalRef) -> Result<GoalRef, BoxError> {
        let gate = self.mutation_gate(agent.session());
        let _guard = gate.enter();
        let (state, runtime) = self.prepare(agent)?;
        let state = self.expect_current(state, reference)?;
        let tombstone = GoalRef {
            id: state.goal.id.clone(),
            revision: state.goal.revision + 1,
        };
        let change = GoalChangeMeta::Clear(GoalClearChangeMeta {
            kind: "goal/change".into(),
            version: GOAL_CHANGE_VERSION,
            operation: GoalOperation::Clear,
            cleared: tombstone.clone(),
            cleared_at: now_ms().max(state.updated_at),
        });
        self.commit(agent, &runtime, &change, GoalActivation::Disarmed)?;
        Ok(tombstone)
    }

    fn with_phase(&self, current: &GoalSnapshot, phase: GoalPhase) -> GoalSnapshot {
        GoalSnapshot {
            id: current.id.clone(),
            revision: current.revision + 1,
            objective: current.objective.clone(),
            phase,
            blocked_reason: None,
            max_goal_rounds: current.max_goal_rounds,
        }
    }

    fn transition(
        &self,
        agent: &Arc<dyn Agent>,
        reference: &GoalRef,
        operation: GoalOperation,
        allowed: &[GoalPhase],
        phase: GoalPhase,
        activation: GoalActivation,
    ) -> Result<GoalView, BoxError> {
        let (state, runtime) = self.prepare(agent)?;
        let state = self.expect_current(state, reference)?;
        if !allowed.contains(&state.goal.phase) {
            return Err(self.transition_error(&state.goal, operation, allowed));
        }
        let goal = self.with_phase(&state.goal, phase);
        self.commit_current(agent, &state, &runtime, operation, goal, activation)
    }

    fn transition_error(
        &self,
        current: &GoalSnapshot,
        operation: GoalOperation,
        allowed: &[GoalPhase],
    ) -> BoxError {
        let allowed = allowed
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" or ");
        Box::new(GoalError::new(
            format!(
                "cannot {operation} goal \"{}\" from phase \"{}\"; expected {allowed}",
                current.id, current.phase
            ),
            "GOAL_INVALID_TRANSITION",
        ))
    }

    fn commit_current(
        &self,
        agent: &Arc<dyn Agent>,
        state: &GoalProjection,
        runtime: &RuntimeState,
        operation: GoalOperation,
        goal: GoalSnapshot,
        activation: GoalActivation,
    ) -> Result<GoalView, BoxError> {
        self.commit_snapshot(
            agent,
            runtime,
            operation,
            goal,
            state.rounds_started,
            state.created_at,
            now_ms().max(state.updated_at),
            activation,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_snapshot(
        &self,
        agent: &Arc<dyn Agent>,
        runtime: &RuntimeState,
        operation: GoalOperation,
        goal: GoalSnapshot,
        rounds_started: u64,
        created_at: i64,
        updated_at: i64,
        activation: GoalActivation,
    ) -> Result<GoalView, BoxError> {
        let change = GoalChangeMeta::Snapshot(GoalSnapshotChangeMeta {
            kind: "goal/change".into(),
            version: GOAL_CHANGE_VERSION,
            operation,
            goal: goal.clone(),
            rounds_started,
            created_at,
            updated_at,
        });
        self.commit(agent, runtime, &change, activation)?;
        Ok(GoalView {
            goal,
            rounds_started,
            created_at,
            updated_at,
            activation: self.runtime(agent.session()).activation,
        })
    }

    fn commit(
        &self,
        agent: &Arc<dyn Agent>,
        _runtime: &RuntimeState,
        change: &GoalChangeMeta,
        activation: GoalActivation,
    ) -> Result<(), BoxError> {
        let reference = goal_change_ref(change);
        let offset = agent.session().seq().get();
        self.with_runtime(agent.session(), |runtime| {
            runtime.pending_activation = Some(PendingActivation { offset, activation });
        });
        let appended = agent
            .session()
            .append("goal/change", serde_json::to_value(change)?, None);
        self.with_runtime(agent.session(), |runtime| {
            if appended
                .as_ref()
                .is_ok_and(|event| event.seq().get() == offset)
            {
                runtime.activation = activation;
            }
            runtime.pending_activation = None;
        });
        appended?;
        let goal = self.view(self.state(agent.session())?, &self.runtime(agent.session()));
        let notification = GoalChanged {
            operation: change.operation(),
            r#ref: reference,
            goal,
        };
        agent_events(&self.inner.ctx, agent).emit(|agent| GoalChangedEvent {
            agent,
            change: notification,
        });
        Ok(())
    }

    fn set_activation(
        &self,
        session: &Session,
        activation: GoalActivation,
    ) -> Result<(), BoxError> {
        let changed = self.with_runtime(session, |runtime| {
            if runtime.activation == activation {
                false
            } else {
                runtime.activation = activation;
                true
            }
        });
        if !changed {
            return Ok(());
        }
        let runtime = self.runtime(session);
        let state = self.inner.projection.state_of(session)?;
        if state.failure.is_some() {
            return Ok(());
        }
        let goal = self
            .view(state.current.clone(), &runtime)
            .map(|view| GoalActivationView {
                id: view.goal.id,
                revision: view.goal.revision,
                activation: view.activation,
            });
        let logger = self.inner.ctx.logger();
        self.inner
            .ctx
            .dispatch(GoalActivationChanged {
                session_id: session.id().clone(),
                goal,
            })
            .invoke(move |failure| {
                let error = match failure {
                    ListenerFailure::Threw(error) | ListenerFailure::Rejected(error) => error,
                };
                logger.warn(format!(
                    "goal/activation-changed listener failed: Error: {error}"
                ));
            });
        Ok(())
    }

    pub fn remote_export_create(
        &self,
        agent: &Arc<dyn Agent>,
        request: CreateGoalRequest,
    ) -> Result<CreateGoalResult, BoxError> {
        let goal = self.create(agent, request)?;
        Ok(CreateGoalResult {
            r#ref: goal.as_ref(),
        })
    }
}

#[derive(Inject)]
pub struct Deps {
    agents: Service<Agents>,
    projections: Service<SessionProjections>,
}

pub struct GoalServicePlugin;
impl Plugin for GoalServicePlugin {
    const NAME: &'static str = "GoalService";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let service = GoalService::start(
            ctx.clone(),
            deps.agents.clone(),
            deps.projections.clone(),
            (*config.get()).clone(),
        )
        .await?;
        ctx.provide::<Goals>(Arc::new(service)).await?;
        Ok(())
    }
}
