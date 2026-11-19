//! Default Agent driver over queued turns and step-boundary input. Every request is derived from
//! the session log.

use crate::{
    assistant_stream::AssistantStreamAttempt,
    inbox::{DispatchFn, ReactLoopInbox, ReadInbox},
    runtime_context::{
        RuntimeContextProjection, SystemPromptDecisionInput, SystemPromptProjection,
    },
};
use futures_util::{FutureExt, StreamExt, future::BoxFuture};
use hoh_agent::{
    Agent, AgentAssistantStream, AgentError, AgentEvents, AgentModel, AgentOptions, AgentPreStep,
    AgentRequest, AgentRequestError, AgentStatus, AgentStatusChanged, AgentTurnStopping, Agents,
    AssembleModelSelection, CancelCauseError, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
    MaintenanceTask, Model, ModelSelection, ModelSelectionProjectionState, PreStepDecision,
    RequestErrorAction, agent_events, assemble_context_for,
};
use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_kernel::Context;
use hoh_llm::{
    AggregateError, ContentBlock, GenerateOptions, Llm, LlmCallConfig, LlmCallControls, LlmError,
    LlmFailure, Message, PreparedLlmCall, RequestMessage, ToolCallBlock, UserMessage, error_chain,
    mark_agent_loop_request,
};
use hoh_scope::{CreateScopeOptions, Scope, ScopeKey, create_scope};
use hoh_session::repair::OpenTurnCloseCause;
use hoh_session::{
    AgentCancelCause, Session, SessionEvent, SessionEventEmitted, SessionId, SessionSeq,
    SurfaceIntent, ToolCallRecovery, canonical_header, header_equals,
};
use hoh_system_prompt::{PromptAssembly, SystemPromptKey, render_context_sections, render_prompt};
use hoh_timeout::{AbortController, AbortSignal};
use hoh_tools::{
    ToolActivationContext, ToolActivationError, ToolBatchExecutionError, ToolBatchInput,
    ToolLogStep, Tools,
};
use serde_json::{Map, Value, json};
use std::{
    any::Any,
    collections::HashSet,
    sync::{Arc, Mutex, MutexGuard, Weak},
};
use tokio::sync::watch;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Reader of the live model-selection projection.
pub type ReadModel = Arc<dyn Fn() -> Arc<ModelSelectionProjectionState> + Send + Sync>;
/// Error-returning reader of the same projection (a persisted-state failure reaches the caller).
pub type CheckModel =
    Arc<dyn Fn() -> Result<Arc<ModelSelectionProjectionState>, BoxError> + Send + Sync>;
/// Live value of the configured `maxParallelToolCalls`.
pub type MaxParallel = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Projected facts consumed by this driver; registration stays with the factory.
pub struct LoopProjections {
    /// Current model selection, including pending changes.
    pub model_selection: ReadModel,
    /// The same projection read with its error preserved.
    pub model_check: CheckModel,
    /// Current queued and claimed input.
    pub inbox: ReadInbox,
}

/// Counts the queue as seen when `turn_once` leaves before its steps ran (disarmed once the steps
/// have returned and the driver has taken its decision): work queued before such an exit stays
/// parked, as in the source.
struct SeesQueueOnExit<'a> {
    agent: &'a ReactLoopAgent,
    armed: bool,
}

impl Drop for SeesQueueOnExit<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.agent.driver_saw_queue();
        }
    }
}

/// What a waking send knew before it inserted its message.
struct WakeTicket {
    epoch: u64,
    message: String,
}

/// Marks a cancellation's clear-to-abort window in the shared state while it lives.
struct CancelWindow<'a>(&'a Mutex<State>);

impl<'a> CancelWindow<'a> {
    fn open(state: &'a Mutex<State>) -> Self {
        lock(state).cancelling += 1;
        Self(state)
    }
}

impl Drop for CancelWindow<'_> {
    fn drop(&mut self) {
        lock(self.0).cancelling -= 1;
    }
}

/// Points between the steps of a driver transition where tests inject a competing call.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Window {
    /// `cancel` cleared the queue and has not aborted yet.
    CancelAfterClear,
    /// `wake_driver` reserved the running phase and has not created its driver yet.
    AfterReserve,
    /// Inside `wake_driver`'s critical section: the agent was found idle, not yet reserved.
    WakeCheckedIdle,
    /// Inside `settle_idle`'s critical section: the latch was read, the phase not yet idle.
    SettleLatchRead,
    /// `turn_steps` returned (success or failure); the driver has looked at the queue for the
    /// last time and has not yet formed the turn's ending.
    AfterTurnStepsOutcome,
    /// The driver left its loop and has not settled to idle yet.
    BeforeSettleIdle,
    /// `turn` returned its outcome to `kick`.
    AfterTurnOutcome,
}

#[cfg(test)]
type WindowProbe = Arc<dyn Fn(Window) + Send + Sync>;

#[cfg(test)]
static WINDOW_PROBES: Mutex<Vec<(String, WindowProbe)>> = Mutex::new(Vec::new());

/// Install a probe for the agent with this id; it runs at every `Window` of that agent.
#[cfg(test)]
pub(crate) fn set_window_probe(id: &str, probe: WindowProbe) {
    let mut probes = lock(&WINDOW_PROBES);
    probes.retain(|(known, _)| known != id);
    probes.push((id.to_owned(), probe));
}

enum Phase {
    Idle {
        last_turn: u64,
    },
    Maintenance {
        abort: Arc<AbortController>,
        last_turn: u64,
        wake_requested: bool,
    },
    Running {
        abort: Arc<AbortController>,
        turn: u64,
        step: u64,
        wake_requested: bool,
    },
}

impl Phase {
    fn status(&self) -> AgentStatus {
        match self {
            Self::Idle { .. } | Self::Maintenance { .. } => AgentStatus::Idle,
            Self::Running { .. } => AgentStatus::Running,
        }
    }
    fn abort(&self) -> Option<&Arc<AbortController>> {
        match self {
            Self::Idle { .. } => None,
            Self::Maintenance { abort, .. } | Self::Running { abort, .. } => Some(abort),
        }
    }
    fn last_turn(&self) -> u64 {
        match self {
            Self::Idle { last_turn } | Self::Maintenance { last_turn, .. } => *last_turn,
            Self::Running { turn, .. } => *turn,
        }
    }
}

struct State {
    phase: Phase,
    /// Completion of the latest whole-agent activity (`true` once it settled).
    activity: Arc<watch::Sender<bool>>,
    /// Whether this loop instance has appended its initial/resume request anchor.
    request_header_logged: bool,
    /// Surface generation at attachment or the preceding built request.
    request_surface_generation: u64,
    /// Process-local revision of assistant frames for this attached Session.
    assistant_stream_revision: u64,
    assistant_attempt_counter: u64,
    /// Cancellations (other than disposal) currently between clearing the queue and aborting:
    /// a waking send landing in that window is "after the cancel" and must latch its wake.
    cancelling: u32,
    /// Waking sends that reached a live driver since it last looked at the queue (its steps
    /// returned or it started). Counted and reset under the lock `settle_idle` reads it under:
    /// whatever lands after the driver's last look, on any exit path, earns one more lap.
    unseen_wakes: u64,
    /// Drivers that have settled to idle so far: a waking send that straddles one may find its
    /// message already handled.
    settled_epoch: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StepEnd {
    Completed,
    MaxTokens,
}

impl StepEnd {
    fn reason(self) -> Value {
        match self {
            Self::Completed => json!({ "kind": "completed" }),
            Self::MaxTokens => json!({ "kind": "max-tokens" }),
        }
    }
}

/// An `enter` decision with the facts the step needs.
struct Entered {
    messages: Vec<UserMessage>,
    starts_request_series: bool,
    assembly: PromptAssembly,
    selection: ModelSelection,
    /// Admission shared by the step's declarations and dispatch; the turn disposes it after the step.
    tool_set: hoh_tools::ResolvedToolSet,
}

enum Prepared {
    Reject,
    Enter(Box<Entered>),
}

/// Closes step admission when the step ends, whichever way it exits.
struct ToolSetGuard(hoh_tools::ResolvedToolSet);
impl Drop for ToolSetGuard {
    fn drop(&mut self) {
        self.0.dispose();
    }
}

enum LoopExit {
    Break,
    ReturnFalse,
}

/// Read the cause `cancel()` passed when aborting a loop-owned signal, copying only the fields
/// `turn/end` records. `None` while the signal is still live.
fn aborted_cancel_cause(signal: &AbortSignal) -> Option<AgentCancelCause> {
    let reason = signal.reason()?;
    // `cancel()` is the only aborter of the signals this loop owns.
    let cause = reason.downcast_ref::<CancelCauseError>()?;
    Some(match &cause.0 {
        AgentCancelCause::User => AgentCancelCause::User,
        AgentCancelCause::Parent => AgentCancelCause::Parent,
        AgentCancelCause::Disposed => AgentCancelCause::Disposed,
        AgentCancelCause::Hook { reason } => AgentCancelCause::Hook {
            reason: reason.clone(),
        },
    })
}

fn boxed(error: SharedError) -> BoxError {
    Box::new(error)
}

/// Remove adapter-derived values before plugins propose the next request config.
fn request_proposal(header: &Value) -> Result<LlmCallConfig, BoxError> {
    let mut config: LlmCallConfig = serde_json::from_value(header["config"].clone())?;
    if let Some(defaults) = header.get("adapterDefaults") {
        if defaults.get("reasoningEffort") == Some(&Value::Bool(true)) {
            config.controls.reasoning_effort = None;
        }
        if defaults.get("maxTokens") == Some(&Value::Bool(true)) {
            config.controls.max_tokens = None;
        }
    }
    Ok(config)
}

fn selection_label(effort: &Option<hoh_llm::ReasoningEffortId>) -> String {
    effort
        .as_ref()
        .map_or_else(|| "adapter default".to_owned(), |e| e.to_string())
}

/// Drives one session through turn and step boundaries.
pub struct ReactLoopAgent {
    id: SessionId,
    options: AgentOptions,
    session: Session,
    /// The agent-scoped registration boundary; the lifecycle owner unwinds it after the driver exits.
    pub scope: Scope,
    scope_key: ScopeKey,
    ctx: Context,
    loop_ctx: Context,
    model: AgentModel,
    model_check: CheckModel,
    inbox: ReactLoopInbox,
    state: Arc<Mutex<State>>,
    lifetime_signal: AbortSignal,
    runtime_context: RuntimeContextProjection,
    system_prompt: SystemPromptProjection,
    max_parallel: MaxParallel,
    weak: Weak<ReactLoopAgent>,
}

impl ReactLoopAgent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        loop_ctx: Context,
        id: SessionId,
        options: AgentOptions,
        session: Session,
        projections: LoopProjections,
        last_turn: u64,
        lifetime_signal: AbortSignal,
        max_parallel: MaxParallel,
    ) -> Result<Arc<Self>, BoxError> {
        let scope_key = ScopeKey::new();
        let scope = create_scope(&loop_ctx, scope_key.clone(), CreateScopeOptions::default())?;
        let ctx = scope.ctx.clone();
        let surface_generation = session.surface()?.content_generation()?;
        let runtime_context = RuntimeContextProjection::new(&ctx, &session)?;
        let model = AgentModel::new(
            projections.model_selection,
            session.clone(),
            options.clone(),
        );
        let activity = {
            let (done, _) = watch::channel(true);
            Arc::new(done)
        };
        Ok(Arc::new_cyclic(|weak: &Weak<ReactLoopAgent>| {
            let dispatch_weak = weak.clone();
            let dispatch: DispatchFn =
                Arc::new(move || dispatch_weak.upgrade().map(|agent| agent.events()));
            ReactLoopAgent {
                id,
                options,
                model_check: projections.model_check,
                inbox: ReactLoopInbox::new(projections.inbox, session.clone(), dispatch),
                system_prompt: SystemPromptProjection::new(session.clone()),
                session,
                scope,
                scope_key,
                ctx,
                loop_ctx,
                model,
                state: Arc::new(Mutex::new(State {
                    phase: Phase::Idle { last_turn },
                    activity,
                    request_header_logged: false,
                    request_surface_generation: surface_generation,
                    assistant_stream_revision: 0,
                    assistant_attempt_counter: 0,
                    cancelling: 0,
                    unseen_wakes: 0,
                    settled_epoch: 0,
                })),
                lifetime_signal,
                runtime_context,
                max_parallel,
                weak: weak.clone(),
            }
        }))
    }

    /// This agent as the trait object events carry.
    fn arc(&self) -> Arc<ReactLoopAgent> {
        // Every `ReactLoopAgent` is built by `Arc::new_cyclic` and used through its `Arc`.
        self.weak.upgrade().expect("agent is alive while borrowed")
    }

    #[cfg(test)]
    fn window(&self, window: Window) {
        let probe = lock(&WINDOW_PROBES)
            .iter()
            .find(|(id, _)| id == self.id.as_str())
            .map(|(_, probe)| probe.clone());
        if let Some(probe) = probe {
            probe(window);
        }
    }

    fn as_agent(&self) -> Arc<dyn Agent> {
        self.arc()
    }

    fn events(&self) -> hoh_agent::AgentEvents {
        agent_events(&self.loop_ctx, &self.as_agent())
    }

    /// The reader-facing inbox with the driver-only operations.
    pub fn loop_inbox(&self) -> &ReactLoopInbox {
        &self.inbox
    }

    fn current_status(&self) -> AgentStatus {
        lock(&self.state).phase.status()
    }

    fn send_impl(
        &self,
        message: UserMessage,
        target: InboxTarget,
        wakeup: bool,
    ) -> Result<(), BoxError> {
        // Waking input cannot join an aborted activity, so it starts the next turn. Captured before
        // the insertion so a reentrant cancel from a splice observer cannot reclassify it.
        let (waking_after_abort, ticket) = {
            let state = lock(&self.state);
            (
                wakeup
                    && state
                        .phase
                        .abort()
                        .is_some_and(|abort| abort.signal().aborted()),
                WakeTicket {
                    epoch: state.settled_epoch,
                    message: message.id.to_string(),
                },
            )
        };
        let resolved = if waking_after_abort {
            InboxTarget::NextTurn
        } else {
            target
        };
        self.inbox
            .splice(resolved, f64::INFINITY, 0.0, vec![message])?;
        if wakeup {
            self.wake_driver(waking_after_abort, Some(&ticket))?;
        }
        Ok(())
    }

    /// Start one driver, or latch its wake behind maintenance or an aborted activity. A wake sent
    /// while idle always opens its turn boundary, even when its message was cleared.
    ///
    /// The phase check, the latch write and the reservation of the running phase are ONE critical
    /// section: two concurrent wakes cannot both see idle, and a latch cannot slip past the
    /// running-to-idle transition (`settle_idle` reads the latch in the same kind of section).
    fn wake_driver(
        &self,
        wake_after_abort: bool,
        ticket: Option<&WakeTicket>,
    ) -> Result<(), BoxError> {
        if self.lifetime_signal.aborted() {
            return Ok(());
        }
        let agents = self
            .loop_ctx
            .get::<Agents>()
            .ok_or("agents service is unavailable")?;
        let completion = {
            let mut guard = lock(&self.state);
            let state = &mut *guard;
            let cancelling = state.cancelling > 0;
            let last_turn = match &mut state.phase {
                Phase::Idle { last_turn } => {
                    // A driver ran to idle between this send's insertion and its wake (the sender
                    // was held up in a synchronous inserted-listener) and already handled the
                    // message: the late wake must not open an empty turn. A message that is
                    // still queued, or a wake with no driver in between, opens its turn as ever.
                    let last_turn = *last_turn;
                    if let Some(ticket) = ticket
                        && state.settled_epoch != ticket.epoch
                        && !self.inbox.is_queued(&ticket.message)
                    {
                        return Ok(());
                    }
                    last_turn
                }
                Phase::Maintenance {
                    abort,
                    wake_requested,
                    ..
                } => {
                    // Maintenance cannot deliver the wake: latch it for replay at convergence.
                    let disposed =
                        aborted_cancel_cause(&abort.signal()) == Some(AgentCancelCause::Disposed);
                    if !disposed {
                        *wake_requested = true;
                    }
                    return Ok(());
                }
                Phase::Running {
                    abort,
                    wake_requested,
                    ..
                } => {
                    let disposed =
                        aborted_cancel_cause(&abort.signal()) == Some(AgentCancelCause::Disposed);
                    // Judged after the insertion: a send classified before an abort, or landing
                    // inside a cancellation's clear-to-abort window, is a send after the cancel.
                    if !disposed && (wake_after_abort || cancelling || abort.signal().aborted()) {
                        *wake_requested = true;
                    }
                    // Counted; the driver resets it when it looks at the queue, and `settle_idle`
                    // reads what is left under this lock. A wake whose own message the driver
                    // already took (its sender was held up after the insertion) has nothing left
                    // to deliver and is not counted: one rule for idle and running.
                    if !disposed
                        && ticket.is_none_or(|ticket| self.inbox.is_queued(&ticket.message))
                    {
                        state.unseen_wakes += 1;
                    }
                    return Ok(());
                }
            };
            #[cfg(test)]
            self.window(Window::WakeCheckedIdle);
            let completion = Arc::new(watch::channel(false).0);
            state.activity = completion.clone();
            state.unseen_wakes = 0;
            state.phase = Phase::Running {
                abort: Arc::new(AbortController::new()),
                turn: last_turn,
                step: 0,
                wake_requested: false,
            };
            completion
        };
        self.events().emit(|agent| AgentStatusChanged {
            agent,
            status: AgentStatus::Running,
        });
        #[cfg(test)]
        self.window(Window::AfterReserve);
        let this = self.arc();
        let driver = agents.with_initiator(self.as_agent(), async move { this.kick().await });
        let mut driver = match driver {
            Ok(driver) => Box::pin(driver),
            Err(error) => {
                // The reservation is released: a failed start must not leave the agent running.
                let _ = self.settle_idle(true);
                completion.send_replace(true);
                return Err(error);
            }
        };
        // The source driver is an async function: it runs synchronously up to its first await,
        // so a waking send has already opened its turn and claimed its input when it returns.
        // Poll the prefix here, then hand the rest to a task (a task's first poll re-registers
        // every waker, so nothing is lost to the no-op waker used for this poll).
        let mut prefix = std::task::Context::from_waker(futures_util::task::noop_waker_ref());
        if std::future::Future::poll(driver.as_mut(), &mut prefix).is_ready() {
            completion.send_replace(true);
            return Ok(());
        }
        tokio::spawn(async move {
            driver.await;
            completion.send_replace(true);
        });
        Ok(())
    }

    /// Leave the active phase for idle in ONE critical section with the read of its wake latch,
    /// so a latch written concurrently is either seen here or finds the agent idle (and starts
    /// its own driver). `only_running` restricts the transition to a driver's phase. Returns
    /// the latch, or `None` when nothing was settled.
    fn settle_idle(&self, only_running: bool) -> Option<bool> {
        let (latched, was_running) = {
            let mut state = lock(&self.state);
            let (last_turn, latched) = match &state.phase {
                Phase::Running {
                    turn,
                    wake_requested,
                    ..
                } => (*turn, *wake_requested),
                Phase::Maintenance {
                    last_turn,
                    wake_requested,
                    ..
                } if !only_running => (*last_turn, *wake_requested),
                other if !only_running => (other.last_turn(), false),
                _ => return None,
            };
            // A wake that landed after the driver's last look at the queue was never delivered.
            let latched = latched || (only_running && state.unseen_wakes > 0);
            #[cfg(test)]
            self.window(Window::SettleLatchRead);
            let was_running = state.phase.status() == AgentStatus::Running;
            state.unseen_wakes = 0;
            if only_running {
                state.settled_epoch += 1;
            }
            state.phase = Phase::Idle { last_turn };
            (latched, was_running)
        };
        if was_running {
            self.events().emit(|agent| AgentStatusChanged {
                agent,
                status: AgentStatus::Idle,
            });
        }
        Some(latched)
    }

    /// Report one failure at its live boundary, then preserve it for driver containment.
    fn throw_error(&self, error: BoxError) -> BoxError {
        let (turn, step) = {
            let state = lock(&self.state);
            match &state.phase {
                Phase::Running { turn, step, .. } => (*turn, *step),
                other => (other.last_turn(), 0),
            }
        };
        let shared = SharedError::new(error);
        let event = shared.clone();
        self.events().emit(move |agent| AgentError {
            agent,
            turn,
            step,
            error: event,
        });
        boxed(shared)
    }

    async fn kick(self: Arc<Self>) {
        while let Ok(true) = self.turn().await {}
        // Reported failures and cancellation are contained at the driver boundary.
        #[cfg(test)]
        self.window(Window::BeforeSettleIdle);
        if let Some(wake_requested) = self.settle_idle(true)
            && wake_requested
            && self.inbox.has_pending().unwrap_or(true)
        {
            // A failure to restart is contained like any other driver failure.
            let _ = self.wake_driver(false, None);
        }
    }

    fn running(&self) -> Option<(Arc<AbortController>, u64, u64)> {
        match &lock(&self.state).phase {
            Phase::Running {
                abort, turn, step, ..
            } => Some((abort.clone(), *turn, *step)),
            _ => None,
        }
    }

    fn set_running(
        &self,
        update: impl FnOnce(&mut u64, &mut u64, &mut Arc<AbortController>, &mut bool),
    ) {
        if let Phase::Running {
            abort,
            turn,
            step,
            wake_requested,
        } = &mut lock(&self.state).phase
        {
            update(turn, step, abort, wake_requested);
        }
    }

    async fn pre_step(
        &self,
        target: InboxTarget,
        turn: u64,
        step: u64,
    ) -> Result<Prepared, BoxError> {
        let Some((abort, _, _)) = self.running() else {
            return Err(format!("agent \"{}\": pre-step outside running phase", self.id).into());
        };
        let signal = abort.signal();
        let claimed = self.inbox.claim(target, turn)?;
        (self.model_check)()?;
        let selection = self.model.current().ok_or_else(|| {
            format!(
                "agent \"{}\" has no provider/model: set AgentOptions.provider and AgentOptions.model or record a model selection",
                self.id
            )
        })?;
        let tools = self
            .loop_ctx
            .get::<Tools>()
            .ok_or("tools service is unavailable")?;
        let tool_set = tools
            .resolve(ToolActivationContext {
                agent: self.as_agent(),
                model_selection: hoh_tools::ModelSelection {
                    provider: selection.provider.clone(),
                    model: selection.model.clone(),
                    reasoning_effort: selection.reasoning_effort.clone(),
                },
                signal: signal.clone(),
            })
            .await?;
        let outcome = self
            .pre_step_resolved(claimed, &selection, &tool_set, &signal, turn, step)
            .await;
        match outcome {
            Ok(Some((decision, assembly))) => match decision {
                PreStepDecision::Reject => {
                    tool_set.dispose();
                    Ok(Prepared::Reject)
                }
                PreStepDecision::Enter {
                    messages,
                    starts_request_series,
                } => Ok(Prepared::Enter(Box::new(Entered {
                    messages,
                    starts_request_series,
                    assembly,
                    selection,
                    tool_set,
                }))),
            },
            Ok(None) => unreachable!("pre_step_resolved always yields a decision"),
            Err(error) => {
                tool_set.dispose();
                Err(error)
            }
        }
    }

    async fn pre_step_resolved(
        &self,
        claimed: Vec<UserMessage>,
        selection: &ModelSelection,
        tool_set: &hoh_tools::ResolvedToolSet,
        signal: &AbortSignal,
        turn: u64,
        step: u64,
    ) -> Result<Option<(PreStepDecision, PromptAssembly)>, BoxError> {
        let prompt = self
            .loop_ctx
            .get::<SystemPromptKey>()
            .ok_or("systemPrompt service is unavailable")?;
        let context = assemble_context_for(&self.as_agent(), Some(signal.clone()))
            .with_extension(AssembleModelSelection(selection.clone()))
            .with_extension(tool_set.clone());
        let assembly = prompt.assemble(context).await?;
        signal.throw_if_aborted()?;
        let runtime_context = self.runtime_context.clone();
        let rendered = assembly.clone();
        let offered = claimed.clone();
        let (entering, selected, abort) = (claimed.clone(), selection.clone(), signal.clone());
        let decision = self
            .events()
            .waterfall::<AgentPreStep, PreStepDecision, _>(
                move |agent| AgentPreStep {
                    agent,
                    messages: offered,
                    turn,
                    step,
                    signal: abort,
                    model_selection: selected,
                },
                move |_| async move {
                    let sections = render_context_sections(&rendered)?;
                    let context = runtime_context.project(&sections);
                    let mut messages = entering;
                    messages.extend(context);
                    Ok(PreStepDecision::Enter {
                        messages,
                        starts_request_series: false,
                    })
                },
            )
            .await?;
        signal.throw_if_aborted()?;
        Ok(Some((decision, assembly)))
    }

    /// Whether the assembled tool schemas differ from the logged request header's.
    fn tools_changed(&self, tools: &[hoh_llm::ToolSchema]) -> Result<bool, BoxError> {
        let Some(baseline) = self.session.request_header() else {
            return Ok(false);
        };
        let mut candidate = (*baseline).clone();
        candidate["tools"] = serde_json::to_value(tools)?;
        Ok(!header_equals(&baseline, &canonical_header(&candidate)))
    }

    /// One turn.
    async fn turn(self: &Arc<Self>) -> Result<bool, BoxError> {
        let outcome = self.turn_once().await;
        #[cfg(test)]
        self.window(Window::AfterTurnOutcome);
        outcome
    }

    /// The driver has just looked at the queue and decided what to leave in it: wakes counted so
    /// far are accounted for (delivered, or deliberately left parked).
    fn driver_saw_queue(&self) {
        lock(&self.state).unseen_wakes = 0;
    }

    /// Open one turn before claiming its first proposed step.
    async fn turn_once(self: &Arc<Self>) -> Result<bool, BoxError> {
        let mut sees_queue = SeesQueueOnExit {
            agent: self,
            armed: true,
        };
        let Some((abort, last_turn, _)) = self.running() else {
            return Err(self.throw_error(
                format!("agent \"{}\": turn without driver reservation", self.id).into(),
            ));
        };
        let signal = abort.signal();
        signal.throw_if_aborted().map_err(boxed)?;
        let turn = last_turn + 1;
        if let Err(error) = self
            .session
            .append("turn/start", json!({ "turn": turn }), None)
        {
            return Err(self.throw_error(error));
        }
        self.set_running(|t, _, _, _| *t = turn);
        let mut turn_ends: Option<Value> = None;
        let outcome = self.turn_steps(turn, &signal, &mut turn_ends).await;
        // The steps returned: this is the driver's last look at the queue before it ends or
        // continues. Everything counted until here was visible to it (and left parked on purpose
        // when the turn was rejected, failed or aborted); a wake from here on is a new one.
        self.driver_saw_queue();
        sees_queue.armed = false;
        #[cfg(test)]
        self.window(Window::AfterTurnStepsOutcome);
        let thrown = match outcome {
            Ok(exit) => Ok(exit),
            Err(error) => {
                // A cause is present exactly while the signal is aborted.
                if let Some(cause) = aborted_cancel_cause(&signal) {
                    turn_ends = Some(json!({ "kind": "aborted", "reason": cause }));
                    Err(error)
                } else {
                    // Every failure is structured: an `LlmError` keeps its facts, a tool
                    // activation failure keeps its code, and anything else flattens to
                    // `errorChain` text under the `UNKNOWN` code.
                    let failure = match HarnessError::of(error.as_ref()) {
                        Some(h) if h.is_named(LlmError::NAME) => h
                            .detail::<LlmFailure>()
                            .and_then(|f| serde_json::to_value(f).ok()),
                        _ => None,
                    };
                    let failure = failure.unwrap_or_else(|| {
                        let code = HarnessError::of(error.as_ref())
                            .filter(|h| h.is_named(ToolActivationError::NAME))
                            .map_or("UNKNOWN".to_owned(), |h| h.code().to_owned());
                        json!({ "message": error_chain(error.as_ref()), "code": code })
                    });
                    turn_ends = Some(json!({ "kind": "error", "error": failure }));
                    Err(self.throw_error(error))
                }
            }
        };
        // `finally`: the turn ending is always recorded; a failure to record it wins.
        let ends = turn_ends.clone().unwrap_or(Value::Null);
        if let Err(error) =
            self.session
                .append("turn/end", json!({ "turn": turn, "reason": ends }), None)
        {
            return Err(self.throw_error(error));
        }
        match thrown? {
            LoopExit::ReturnFalse => return Ok(false),
            LoopExit::Break => {}
        }
        if !self.inbox.has_pending()? {
            return Ok(false);
        }
        self.set_running(|_, step, abort, wake| {
            *abort = Arc::new(AbortController::new());
            // A fresh controller makes a latch set on the old one stale.
            *wake = false;
            *step = 0;
        });
        Ok(true)
    }

    async fn turn_steps(
        self: &Arc<Self>,
        turn: u64,
        signal: &AbortSignal,
        turn_ends: &mut Option<Value>,
    ) -> Result<LoopExit, BoxError> {
        let mut target = InboxTarget::NextTurn;
        loop {
            signal.throw_if_aborted().map_err(boxed)?;
            let (_, _, current_step) = self.running().ok_or("agent left the running phase")?;
            let step = current_step + 1;
            let decision = match self.pre_step(target, turn, step).await? {
                Prepared::Reject => {
                    *turn_ends = Some(json!({ "kind": "blocked" }));
                    return Ok(LoopExit::ReturnFalse);
                }
                Prepared::Enter(decision) => decision,
            };
            let _admission = ToolSetGuard(decision.tool_set.clone());
            if turn_ends.is_some() && decision.messages.is_empty() {
                break;
            }
            // A removed waking message or an enter decision rewritten to empty still owns the
            // initial turn boundary, but it spends no model call.
            if current_step == 0 && decision.messages.is_empty() {
                *turn_ends = Some(json!({ "kind": "completed" }));
                return Ok(LoopExit::ReturnFalse);
            }
            signal.throw_if_aborted().map_err(boxed)?;
            self.session
                .append("step/start", json!({ "turn": turn, "step": step }), None)?;
            self.set_running(|_, s, _, _| *s = step);
            let recovery = Arc::new(Mutex::new(ToolCallRecovery::new(
                OpenTurnCloseCause::Interrupted,
            )));
            let (observer, subject) = (recovery.clone(), self.session.clone());
            let stop_recovery = self
                .ctx
                .on_contained::<SessionEventEmitted>(move |emitted| {
                    if emitted.session == subject {
                        lock(&observer).observe(&emitted.event);
                    }
                    Ok(())
                })?;
            let step_result = self.step(&decision, turn, step).await;
            let failure = match step_result {
                Ok(end) => {
                    // max-tokens is sticky: once any step hits the ceiling, later steps that
                    // complete normally must not downgrade the turn outcome.
                    let sticky = turn_ends
                        .as_ref()
                        .is_some_and(|ends| ends["kind"] == "max-tokens");
                    if !sticky {
                        *turn_ends = end.map(StepEnd::reason);
                    }
                    None
                }
                Err(error) => {
                    let recorded: Result<(), BoxError> = (|| {
                        // The guard must not outlive this statement: appending notifies the
                        // recovery observer, which takes the same lock.
                        let pending = lock(&recovery).results()?;
                        for event in pending {
                            self.session.append(
                                "tool/result",
                                event.data().clone(),
                                Some(recovery_intent(&event)),
                            )?;
                        }
                        Ok(())
                    })();
                    Some(match recorded {
                        Ok(()) => error,
                        Err(recovery_error) => Box::new(AggregateError {
                            message:
                                "Step failed and its pending tool results could not be recorded"
                                    .into(),
                            errors: vec![SharedError::new(error), SharedError::new(recovery_error)],
                        }) as BoxError,
                    })
                }
            };
            drop(stop_recovery.dispose());
            let ended =
                self.session
                    .append("step/end", json!({ "turn": turn, "step": step }), None);
            // `finally` semantics: a failure to close the step supersedes the step's own error.
            ended?;
            if let Some(error) = failure {
                return Err(error);
            }
            drop(_admission);
            signal.throw_if_aborted().map_err(boxed)?;
            if turn_ends.is_some() && self.inbox.next_step().is_empty() {
                self.events()
                    .serial::<AgentTurnStopping, ()>(|agent| AgentTurnStopping {
                        agent,
                        turn,
                        signal: signal.clone(),
                    })
                    .await?;
                signal.throw_if_aborted().map_err(boxed)?;
            }
            if turn_ends.is_some() && self.inbox.next_step().is_empty() {
                break;
            }
            target = InboxTarget::NextStep;
        }
        Ok(LoopExit::Break)
    }

    async fn step(
        &self,
        decision: &Entered,
        turn: u64,
        step: u64,
    ) -> Result<Option<StepEnd>, BoxError> {
        let Some((abort, _, _)) = self.running() else {
            return Err(format!("agent \"{}\": step outside running phase", self.id).into());
        };
        let signal = abort.signal();
        signal.throw_if_aborted().map_err(boxed)?;
        let rendered_prompt = render_prompt(&decision.assembly)?;
        let mut first_attempt = true;
        loop {
            let (config, prepared_call) = self
                .prepare_request(turn, step, &decision.selection, &signal)
                .await?;
            let prompt = self
                .loop_ctx
                .get::<SystemPromptKey>()
                .ok_or("systemPrompt service is unavailable")?;
            let context_for = assemble_context_for(&self.as_agent(), Some(signal.clone()))
                .with_extension(AssembleModelSelection(decision.selection.clone()));
            let current_context = prompt.refresh_context(&decision.assembly, &context_for)?;
            let sections = render_context_sections(&current_context)?;
            let context = self.runtime_context.project(&sections);
            signal.throw_if_aborted().map_err(boxed)?;
            let starts_request_series = first_attempt && decision.starts_request_series;
            let in_history = prepared_call
                .as_ref()
                .and_then(|call| call.system_prompt_update.as_deref())
                == Some("in-history");
            let tool_update_missing = prepared_call
                .as_ref()
                .is_none_or(|call| call.tool_update.is_none());
            let surface_generation = lock(&self.state).request_surface_generation;
            let starts_series = starts_request_series
                || surface_generation != self.session.surface()?.content_generation()?
                || (tool_update_missing && self.tools_changed(&decision.assembly.tools)?);
            let commits = self.system_prompt.project(
                &rendered_prompt,
                SystemPromptDecisionInput {
                    in_history,
                    starts_series,
                },
            )?;
            for commit in commits {
                self.session.append(
                    "system/message",
                    json!({ "turn": turn, "step": step, "message": serde_json::to_value(&commit.message)? }),
                    Some(commit.intent),
                )?;
            }
            let mut context_admitted = false;
            if first_attempt {
                for message in &decision.messages {
                    if message.source.kind == "runtime-context" {
                        if let (Some(context), false) = (&context, context_admitted) {
                            self.session.append(
                                "user/message",
                                serde_json::to_value(context)?,
                                Some(SurfaceIntent::append()),
                            )?;
                            context_admitted = true;
                        }
                    } else {
                        self.session.append(
                            "user/message",
                            serde_json::to_value(message)?,
                            Some(SurfaceIntent::append()),
                        )?;
                    }
                }
            }
            if let (Some(context), false) = (&context, context_admitted) {
                self.session.append(
                    "user/message",
                    serde_json::to_value(context)?,
                    Some(SurfaceIntent::append()),
                )?;
            }
            first_attempt = false;
            let request = self.build_request(
                &config,
                prepared_call.as_ref(),
                &decision.assembly.tools,
                (turn, step),
                starts_request_series,
                &signal,
            )?;
            let mut live = self.new_attempt(turn, step);
            let mut started = false;
            let streamed = self
                .consume_stream(
                    &request,
                    prepared_call.as_ref(),
                    &mut live,
                    &mut started,
                    &signal,
                )
                .await;
            if let Err(error) = streamed {
                if !started {
                    return Err(error);
                }
                let settled: Result<(), BoxError> = if signal.aborted() {
                    let content = live.interrupted_blocks();
                    if content.is_empty() {
                        self.settle_attempt(&mut live, turn, step)
                    } else {
                        self.settle_interrupted(&mut live, &request, content, turn, step)
                    }
                } else {
                    self.settle_attempt(&mut live, turn, step)
                };
                if let Err(settlement_error) = settled {
                    return Err(Box::new(AggregateError {
                        message: "Assistant stream failed and its durable settlement was rejected"
                            .into(),
                        errors: vec![SharedError::new(error), SharedError::new(settlement_error)],
                    }));
                }
                return Err(error);
            }
            let result = self
                .finish_attempt(
                    &mut live,
                    decision,
                    &request,
                    prepared_call.as_ref(),
                    turn,
                    step,
                    &signal,
                )
                .await;
            match result {
                Ok(Some(next)) => return Ok(next),
                Ok(None) => continue,
                Err(error) => {
                    if !live.ended() {
                        live.abandon();
                    }
                    return Err(error);
                }
            }
        }
    }

    fn new_attempt(&self, turn: u64, step: u64) -> AssistantStreamAttempt {
        let attempt = {
            let mut state = lock(&self.state);
            state.assistant_attempt_counter += 1;
            state.assistant_attempt_counter
        };
        let revisions = self.state.clone();
        let weak = self.weak.clone();
        AssistantStreamAttempt::new(
            &self.id,
            attempt,
            Box::new(move || {
                let mut state = lock(&revisions);
                state.assistant_stream_revision += 1;
                state.assistant_stream_revision
            }),
            turn,
            step,
            Box::new(move |frame| {
                if let Some(agent) = weak.upgrade() {
                    agent
                        .events()
                        .emit(|a| AgentAssistantStream { agent: a, frame });
                }
            }),
        )
    }

    async fn consume_stream(
        &self,
        request: &GenerateOptions,
        prepared: Option<&PreparedLlmCall>,
        live: &mut AssistantStreamAttempt,
        started: &mut bool,
        signal: &AbortSignal,
    ) -> Result<(), BoxError> {
        let mut stream = match prepared {
            Some(call) => call.stream(dispatch_copy(request))?,
            None => self
                .loop_ctx
                .get::<Llm>()
                .ok_or("llm service is unavailable")?
                .stream(&self.loop_ctx, dispatch_copy(request)),
        };
        signal.throw_if_aborted().map_err(boxed)?;
        live.start();
        *started = true;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            signal.throw_if_aborted().map_err(boxed)?;
            live.push(chunk)?;
        }
        signal.throw_if_aborted().map_err(boxed)?;
        Ok(())
    }

    fn settle_attempt(
        &self,
        live: &mut AssistantStreamAttempt,
        turn: u64,
        step: u64,
    ) -> Result<(), BoxError> {
        let stream = serde_json::to_value(live.stream())?;
        let session = self.session.clone();
        live.settle("assistant/attempt", move || {
            Ok(session
                .append(
                    "assistant/attempt",
                    json!({ "turn": turn, "step": step, "stream": stream }),
                    None,
                )?
                .seq())
        })
    }

    fn settle_interrupted(
        &self,
        live: &mut AssistantStreamAttempt,
        request: &GenerateOptions,
        content: Vec<ContentBlock>,
        turn: u64,
        step: u64,
    ) -> Result<(), BoxError> {
        let message = assistant_message(request, content, live.replay_state()?)?;
        let mut data = Map::new();
        data.insert("turn".into(), turn.into());
        data.insert("step".into(), step.into());
        data.insert("message".into(), message);
        data.insert("interrupted".into(), true.into());
        if let Some(usage) = live.usage() {
            data.insert("usage".into(), serde_json::to_value(usage)?);
        }
        data.insert("stream".into(), serde_json::to_value(live.stream())?);
        let session = self.session.clone();
        live.settle("assistant/message", move || {
            Ok(session
                .append(
                    "assistant/message",
                    Value::Object(data),
                    Some(SurfaceIntent::append()),
                )?
                .seq())
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_attempt(
        &self,
        live: &mut AssistantStreamAttempt,
        decision: &Entered,
        request: &GenerateOptions,
        prepared: Option<&PreparedLlmCall>,
        turn: u64,
        step: u64,
        signal: &AbortSignal,
    ) -> Result<Option<Option<StepEnd>>, BoxError> {
        let finish = live.finish();
        if finish.kind == "error" || finish.kind == "aborted" {
            self.settle_attempt(live, turn, step)?;
            let failure = finish
                .failure()
                .ok_or("failed finish record without a failure")?;
            let (provider, retry_policy, abort, event_failure) = (
                request.provider.clone(),
                prepared.map(|call| call.retry_policy.clone()),
                signal.clone(),
                failure.clone(),
            );
            let action = self
                .events()
                .waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(
                    move |agent| AgentRequestError {
                        agent,
                        turn,
                        step,
                        provider,
                        failure: event_failure,
                        retry_policy,
                        signal: abort,
                    },
                    |_| async { Ok(None) },
                )
                .await?;
            signal.throw_if_aborted().map_err(boxed)?;
            if action != Some(RequestErrorAction::Retry) {
                return Err(Box::new(LlmError::from_failure(&failure, None)?));
            }
            return Ok(None);
        }
        let message = assistant_message(request, live.blocks()?, live.replay_state()?)?;
        let mut data = Map::new();
        data.insert("turn".into(), turn.into());
        data.insert("step".into(), step.into());
        data.insert("message".into(), message.clone());
        if let Some(usage) = live.usage() {
            data.insert("usage".into(), serde_json::to_value(usage)?);
        }
        data.insert("stream".into(), serde_json::to_value(live.stream())?);
        let session = self.session.clone();
        live.settle("assistant/message", move || {
            Ok(session
                .append(
                    "assistant/message",
                    Value::Object(data),
                    Some(SurfaceIntent::append()),
                )?
                .seq())
        })?;
        if finish.kind == "max-tokens" {
            return Ok(Some(Some(StepEnd::MaxTokens)));
        }
        let calls: Vec<ToolCallBlock> = message["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|block| block["type"] == "tool-call")
                    .filter_map(|block| serde_json::from_value(block.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();
        if calls.is_empty() {
            return Ok(Some(Some(StepEnd::Completed)));
        }
        let tools = self
            .loop_ctx
            .get::<Tools>()
            .ok_or("tools service is unavailable")?;
        let max_parallel = (self.max_parallel)();
        let input = ToolBatchInput {
            calls,
            signal: signal.clone(),
            max_parallel_tool_calls: Some(max_parallel),
            agent: Some(self.as_agent()),
            tool_set: Some(decision.tool_set.clone()),
            log: Some(ToolLogStep { turn, step }),
        };
        match tools.execute(input).await {
            Ok(result) => {
                for context in result.additional_contexts {
                    self.append_step_context(context)?;
                }
                Ok(Some(if result.concluded {
                    Some(StepEnd::Completed)
                } else {
                    None
                }))
            }
            Err(error) => match error.downcast::<ToolBatchExecutionError>() {
                Ok(batch) => {
                    let batch = *batch;
                    for context in batch.partial.additional_contexts {
                        self.append_step_context(context)?;
                    }
                    Err(batch.cause)
                }
                Err(error) => Err(error),
            },
        }
    }

    fn append_step_context(&self, context: UserMessage) -> Result<(), BoxError> {
        let end = self.inbox.next_step().len();
        self.inbox
            .splice(InboxTarget::NextStep, end as f64, 0.0, vec![context])?;
        Ok(())
    }

    /// Resolve request config for the step's selection and bind its adapter before admitting
    /// model-visible input.
    async fn prepare_request(
        &self,
        turn: u64,
        step: u64,
        selection: &ModelSelection,
        signal: &AbortSignal,
    ) -> Result<(LlmCallConfig, Option<PreparedLlmCall>), BoxError> {
        // A loop instance seeds output controls from its options until it logs its own header;
        // later steps re-resolve marked defaults.
        let persisted = if lock(&self.state).request_header_logged {
            let header = self
                .session
                .request_header()
                .ok_or("the instance logged the header it now folds")?;
            request_proposal(&header)?.controls
        } else {
            LlmCallControls {
                max_tokens: self.options.max_tokens(),
                ..LlmCallControls::default()
            }
        };
        let seed = LlmCallConfig {
            controls: LlmCallControls {
                reasoning_effort: selection.reasoning_effort.clone(),
                ..persisted
            },
            provider: selection.provider.clone(),
            model: selection.model.clone(),
        };
        let abort = signal.clone();
        let proposed = self
            .events()
            .waterfall::<AgentRequest, LlmCallConfig, _>(
                move |agent| AgentRequest {
                    agent,
                    turn,
                    step,
                    signal: abort,
                },
                move |_| async move { Ok(seed) },
            )
            .await?;
        signal.throw_if_aborted().map_err(boxed)?;
        if proposed.provider != selection.provider || proposed.model != selection.model {
            return Err(format!(
                "agent \"{}\": agent/request proposed \"{}/{}\" after step preparation fixed \"{}/{}\"; use agent.model.select() before step preparation",
                self.id, proposed.provider, proposed.model, selection.provider, selection.model
            )
            .into());
        }
        if proposed.controls.reasoning_effort != selection.reasoning_effort {
            return Err(format!(
                "agent \"{}\": agent/request proposed reasoning effort \"{}\" after step preparation fixed \"{}\"; use agent.model.select() before step preparation",
                self.id,
                selection_label(&proposed.controls.reasoning_effort),
                selection_label(&selection.reasoning_effort)
            )
            .into());
        }
        let llm = self
            .loop_ctx
            .get::<Llm>()
            .ok_or("llm service is unavailable")?;
        let (config, prepared) = match llm
            .prepare_call(&self.loop_ctx, proposed.clone(), Some(signal.clone()), None)
            .await
        {
            Ok(prepared) => (prepared.config.clone(), Some(prepared)),
            Err(error) => {
                // Middleware may serve an unregistered route; terminal dispatch still requires
                // an adapter.
                let no_adapter = HarnessError::of(error.as_ref())
                    .is_some_and(|h| h.is_named(LlmError::NAME) && h.code() == "NO_ADAPTER");
                if !no_adapter {
                    return Err(error);
                }
                (proposed, None)
            }
        };
        signal.throw_if_aborted().map_err(boxed)?;
        Ok((config, prepared))
    }

    /// Log the resolved envelope and derive a frozen request from the admitted surface.
    fn build_request(
        &self,
        config: &LlmCallConfig,
        prepared: Option<&PreparedLlmCall>,
        tools: &[hoh_llm::ToolSchema],
        position: (u64, u64),
        starts_request_series: bool,
        signal: &AbortSignal,
    ) -> Result<GenerateOptions, BoxError> {
        let (turn, step) = position;
        let surface_generation = self.session.surface()?.content_generation()?;
        let mut raw = Map::new();
        raw.insert("config".into(), serde_json::to_value(config)?);
        if let Some(call) = prepared {
            raw.insert(
                "adapterDefaults".into(),
                serde_json::to_value(&call.adapter_defaults)?,
            );
        }
        if !tools.is_empty() {
            raw.insert("tools".into(), serde_json::to_value(tools)?);
        }
        let header = canonical_header(&Value::Object(raw));
        let baseline = self.session.request_header();
        let request_surface_generation = lock(&self.state).request_surface_generation;
        let starts_series =
            starts_request_series || request_surface_generation != surface_generation;
        let mut header_seq: Option<SessionSeq> = None;
        let logged = lock(&self.state).request_header_logged;
        if !logged {
            // Compaction during the first resumed pre-step must still mark a new series.
            let mut data = Map::new();
            data.insert("header".into(), header.clone());
            data.insert(
                "reason".into(),
                if baseline.is_none() {
                    "initial"
                } else {
                    "resume"
                }
                .into(),
            );
            if starts_series {
                data.insert("startsSeries".into(), true.into());
            }
            header_seq = Some(
                self.session
                    .append("request/header", Value::Object(data), None)?
                    .seq(),
            );
            lock(&self.state).request_header_logged = true;
        } else if baseline
            .as_deref()
            .is_none_or(|b| !header_equals(b, &header))
        {
            let mut data = Map::new();
            data.insert("header".into(), header.clone());
            data.insert("reason".into(), "change".into());
            if starts_series {
                data.insert("startsSeries".into(), true.into());
            }
            header_seq = Some(
                self.session
                    .append("request/header", Value::Object(data), None)?
                    .seq(),
            );
        } else if starts_series {
            self.session.append(
                "request/header",
                json!({ "header": header.clone(), "reason": "series" }),
                None,
            )?;
        }
        if let (Some(baseline), Some(header_seq)) = (&baseline, header_seq) {
            let names = |value: &Value| -> Vec<String> {
                value
                    .as_array()
                    .map(|list| {
                        list.iter()
                            .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let previous = names(&baseline["tools"]);
            let current: Vec<String> = tools.iter().map(|tool| tool.name.clone()).collect();
            let previous_set: HashSet<&String> = previous.iter().collect();
            let current_set: HashSet<&String> = current.iter().collect();
            let additions: Vec<&String> = current
                .iter()
                .filter(|name| !previous_set.contains(name))
                .collect();
            let removals: Vec<&String> = previous
                .iter()
                .filter(|name| !current_set.contains(name))
                .collect();
            if !additions.is_empty() || !removals.is_empty() {
                let content: Vec<Value> = additions
                    .iter()
                    .map(|name| json!({ "type": "tool-addition", "toolName": name }))
                    .chain(
                        removals
                            .iter()
                            .map(|name| json!({ "type": "tool-removal", "toolName": name })),
                    )
                    .collect();
                let mut data = Map::new();
                data.insert("turn".into(), turn.into());
                data.insert("step".into(), step.into());
                data.insert(
                    "message".into(),
                    json!({
                        "content": content,
                        "source": { "kind": "tool-registry" },
                        "role": "developer",
                        "id": hoh_util_crypto::random_uuid(),
                    }),
                );
                if !additions.is_empty() {
                    data.insert("headerSeq".into(), header_seq.get().into());
                }
                self.session.append(
                    "developer/message",
                    Value::Object(data),
                    Some(SurfaceIntent::append()),
                )?;
            }
        }
        lock(&self.state).request_surface_generation = surface_generation;

        let mut request_context = Map::new();
        request_context.insert("provider".into(), config.provider.clone().into());
        request_context.insert("model".into(), config.model.clone().into());
        if let Some(window) = prepared.and_then(|call| call.context.as_ref()) {
            request_context.insert("contextWindow".into(), window.context_window.into());
        }
        if let Some(update) = prepared.and_then(|call| call.system_prompt_update.as_ref()) {
            request_context.insert("systemPromptUpdate".into(), update.clone().into());
        }
        let request_context = Value::Object(request_context);
        let differs = match self.session.request_context() {
            None => true,
            Some(previous) => ["provider", "model", "contextWindow", "systemPromptUpdate"]
                .iter()
                .any(|key| previous.get(*key) != request_context.get(*key)),
        };
        if differs {
            self.session
                .append("request/context", request_context, None)?;
        }
        signal.throw_if_aborted().map_err(boxed)?;

        let messages: Vec<RequestMessage> = self
            .session
            .derive_messages()?
            .iter()
            .map(|message| request_message(message))
            .collect::<Result<_, _>>()?;
        let header_config: LlmCallConfig = serde_json::from_value(header["config"].clone())?;
        let header_tools: Option<Vec<hoh_llm::ToolSchema>> = match header.get("tools") {
            Some(tools) => Some(serde_json::from_value(tools.clone())?),
            None => None,
        };
        let mut request = GenerateOptions {
            provider: header_config.provider,
            model: header_config.model,
            reasoning_effort: header_config.controls.reasoning_effort,
            messages,
            system: None,
            tools: header_tools,
            tool_history: Some(serde_json::from_value(
                (*self.session.tool_history()?).clone(),
            )?),
            temperature: header_config.controls.temperature,
            max_tokens: header_config.controls.max_tokens,
            stop: header_config.controls.stop,
            signal: Some(signal.clone()),
            session_id: Some(self.session.id().clone()),
            purpose: None,
            agent_loop_request: false,
        };
        mark_agent_loop_request(&mut request);
        Ok(request)
    }
}

/// A copy of the loop-built request for one dispatch. `GenerateOptions::clone` deliberately
/// clears the loop-request marker (a clone is no longer the frozen object), so it is carried over
/// explicitly: the adapter must receive the request the loop built.
fn dispatch_copy(request: &GenerateOptions) -> GenerateOptions {
    let mut copy = request.clone();
    copy.agent_loop_request = request.agent_loop_request;
    copy
}

/// One derived history message as a request message, dispatched on its `role`.
///
/// A derived history value as a durable request message: `hoh_llm::Message` selects its payload
/// by `role` (an unknown role is an error, not a silent reinterpretation).
fn request_message(value: &Value) -> Result<RequestMessage, BoxError> {
    let message: Message = serde_json::from_value(value.clone())?;
    Ok(RequestMessage::Durable(Arc::new(message)))
}

fn recovery_intent(event: &SessionEvent) -> SurfaceIntent {
    SurfaceIntent {
        surface_op: Some(hoh_session::SurfaceOp::append()),
        source_event_seqs: event
            .source_event_seqs()
            .and_then(|seqs| serde_json::from_value(seqs.clone()).ok()),
    }
}

/// An assistant message value in the source's key order (`createAssistantMessage`).
fn assistant_message(
    request: &GenerateOptions,
    content: Vec<ContentBlock>,
    replay_state: Option<hoh_llm::ReplayEnvelope>,
) -> Result<Value, BoxError> {
    let message = hoh_llm::create_assistant_message(hoh_llm::NewAssistantMessage {
        content,
        source: hoh_llm::AssistantProviderMetadata {
            provider: request.provider.clone(),
            model: request.model.clone(),
            replay_state: replay_state.map(serde_json::to_value).transpose()?,
        },
    });
    Ok(serde_json::to_value(message)?)
}

impl Agent for ReactLoopAgent {
    fn id(&self) -> &SessionId {
        &self.id
    }
    fn scope_key(&self) -> ScopeKey {
        self.scope_key.clone()
    }
    fn options(&self) -> &AgentOptions {
        &self.options
    }
    fn session(&self) -> &Session {
        &self.session
    }
    fn model(&self) -> &dyn Model {
        &self.model
    }
    fn inbox(&self) -> &dyn Inbox {
        &self.inbox
    }
    fn status(&self) -> AgentStatus {
        self.current_status()
    }
    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn cancel(&self, cause: AgentCancelCause, options: CancelOptions) -> Result<(), BoxError> {
        // The source cancels in one synchronous step. Here the window between clearing the queue
        // and aborting is marked first (and the latch reset in the same section), so a waking
        // send landing inside it latches its wake: it is "after the cancel". A plain `inject`
        // never wakes, so it is never replayed.
        let _window = (!options.keep_inbox && cause != AgentCancelCause::Disposed)
            .then(|| CancelWindow::open(&self.state));
        if !options.keep_inbox {
            let mut state = lock(&self.state);
            if let Phase::Maintenance { wake_requested, .. }
            | Phase::Running { wake_requested, .. } = &mut state.phase
            {
                *wake_requested = false;
            }
        }
        let cleared = if options.keep_inbox {
            Ok(())
        } else {
            self.inbox.clear()
        };
        #[cfg(test)]
        self.window(Window::CancelAfterClear);
        // Queue persistence must not prevent cancellation of admitted work.
        let abort = lock(&self.state).phase.abort().cloned();
        if let Some(abort) = abort {
            abort.abort_with(CancelCauseError(cause));
        }
        cleared
    }

    fn when_idle(&self) -> BoxFuture<'static, ()> {
        let state = self.state.clone();
        Box::pin(async move {
            loop {
                let activity = lock(&state).activity.clone();
                let mut done = activity.subscribe();
                let _ = done.wait_for(|settled| *settled).await;
                if Arc::ptr_eq(&activity, &lock(&state).activity) {
                    break;
                }
            }
        })
    }

    fn run_maintenance_erased(&self, task: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        let abort = Arc::new(AbortController::new());
        let completion = Arc::new(watch::channel(false).0);
        {
            // The admission check and the phase change are one critical section.
            let mut state = lock(&self.state);
            let Phase::Idle { last_turn } = state.phase else {
                return Err(format!("agent \"{}\" already has active work", self.id).into());
            };
            state.activity = completion.clone();
            state.phase = Phase::Maintenance {
                abort: abort.clone(),
                last_turn,
                wake_requested: false,
            };
        }
        let this = self.arc();
        let job = task(abort.signal());
        // The work runs on its own task, independent of whether the caller keeps polling the
        // returned future: dropping that future must never leave the agent in the maintenance
        // phase (the source's `finally` always returns the agent to idle).
        let work = tokio::spawn(async move {
            let result = std::panic::AssertUnwindSafe(job).catch_unwind().await;
            let wake_requested = this.settle_idle(false).unwrap_or(false);
            let cause = aborted_cancel_cause(&abort.signal());
            if cause != Some(AgentCancelCause::Disposed)
                && wake_requested
                && this.inbox.has_pending().unwrap_or(true)
            {
                let _ = this.wake_driver(false, None);
            }
            completion.send_replace(true);
            result
        });
        Ok(Box::pin(async move {
            match work.await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err("maintenance task panicked".into()),
                Err(error) => Err(format!("maintenance task failed: {error}").into()),
            }
        }))
    }

    fn send(
        &self,
        message: UserMessage,
        target: InboxTarget,
        wakeup: bool,
    ) -> Result<(), BoxError> {
        self.send_impl(message, target, wakeup)
    }
    fn followup(&self, message: UserMessage) -> Result<(), BoxError> {
        self.send_impl(message, InboxTarget::NextTurn, true)
    }
    fn steer(&self, message: UserMessage) -> Result<(), BoxError> {
        self.send_impl(message, InboxTarget::NextStep, true)
    }
    fn inject(&self, message: UserMessage) -> Result<(), BoxError> {
        self.send_impl(message, InboxTarget::NextStep, false)
    }
}

/// Unused in the driver itself; keeps the type-erased result helper in scope for dependents.
#[doc(hidden)]
pub type MaintenanceResult = Box<dyn Any + Send>;

#[doc(hidden)]
pub type EventsHandle = AgentEvents;
