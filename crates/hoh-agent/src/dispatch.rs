//! Agent-scoped dispatch and prompt assembly helpers. The fused dispatcher [`AgentEvents`]
//! couples the agent subject to its scope carrier, so the scope key and the payload's `agent`
//! cannot diverge; repeat dispatchers (the loop driver) build it once and reuse it.

use crate::runtime_types::*;
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Contained, Context, Event, ListenerFailure, Serial, Waterfall};
use hoh_scope::{ScopeFilter, Scoped, scope_target};
use hoh_system_prompt::AssembleContext;
use hoh_timeout::AbortSignal;
use std::{future::Future, sync::Arc};

impl ScopeFilter for dyn Agent {
    /// An agent subject has no isolation filter of its own: only its scope key routes.
    fn matches_filter(&self, _listener: &Context) -> bool {
        true
    }
}

/// Marker for the events whose subject is an agent: the payload carries `agent` and the event is
/// dispatched through the agent's scope carrier (the `Scoped<Agent>` `this` contract).
pub trait AgentSubjectEvent: Event {
    /// Overwrite the payload's `agent` with the fused dispatcher's subject (the source spreads
    /// the caller's payload first and the subject last, so a payload that carries its own
    /// `agent` can never override it). The default leaves the payload as built: an event type
    /// outside this crate must override it to get the guarantee.
    fn set_agent(&mut self, _agent: Arc<dyn Agent>) {}
}
macro_rules! subject {
    ($($t:ty),* $(,)?) => { $(impl AgentSubjectEvent for $t {
        fn set_agent(&mut self, agent: Arc<dyn Agent>) {
            self.agent = agent;
        }
    })* };
}
subject!(
    AgentCreated,
    AgentDisposed,
    AgentStatusChanged,
    AgentInboxInserted,
    AgentInboxClaimed,
    AgentInboxDiscarded,
    AgentPreStep,
    AgentRequest,
    AgentRequestError,
    AgentAssistantStream,
    AgentTurnStopping,
    AgentError,
);

/// Build the fused scope carrier for one agent subject.
pub fn agent_carrier(agent: &Arc<dyn Agent>) -> Scoped<dyn Agent> {
    scope_target(agent.clone(), Some(agent.scope_key()))
}

/// The fused dispatcher: each method dispatches the named agent-subject event with the agent's
/// scope carrier and the agent itself injected into the payload.
#[derive(Clone)]
pub struct AgentEvents {
    ctx: Context,
    agent: Arc<dyn Agent>,
    carrier: Scoped<dyn Agent>,
}

/// Build a dispatcher that couples the agent subject to its scope carrier.
pub fn agent_events(ctx: &Context, agent: &Arc<dyn Agent>) -> AgentEvents {
    AgentEvents {
        ctx: ctx.clone(),
        agent: agent.clone(),
        carrier: agent_carrier(agent),
    }
}

/// Like [`agent_events`] but with an existing carrier, built once per agent.
pub fn agent_events_with(
    ctx: &Context,
    agent: &Arc<dyn Agent>,
    carrier: Scoped<dyn Agent>,
) -> AgentEvents {
    AgentEvents {
        ctx: ctx.clone(),
        agent: agent.clone(),
        carrier,
    }
}

/// The text `String(error)` gives in the source: `<class name>: <message>`.
pub(crate) fn js_error_text(error: &BoxError) -> String {
    match HarnessError::of(error.as_ref()) {
        Some(error) if error.message().is_empty() => error.name().to_owned(),
        Some(error) => format!("{}: {}", error.name(), error.message()),
        None => format!("Error: {error}"),
    }
}

impl AgentEvents {
    fn scoped(&self) -> Context {
        self.carrier.context(&self.ctx)
    }

    /// Build the payload with this dispatcher's agent injected last, so it wins over any `agent`
    /// the builder put into the payload.
    fn fuse<E: AgentSubjectEvent>(&self, build: impl FnOnce(Arc<dyn Agent>) -> E) -> E {
        let mut payload = build(self.agent.clone());
        payload.set_agent(self.agent.clone());
        payload
    }

    /// Fire-and-forget notification in the agent's scope. Every listener is invoked; failures are
    /// logged and contained per listener, so a notification cannot veto lifecycle progress or
    /// starve a later observer.
    pub fn emit<E>(&self, build: impl FnOnce(Arc<dyn Agent>) -> E)
    where
        E: AgentSubjectEvent + Event<Mode = Contained>,
    {
        let log = self.ctx.logger();
        self.scoped()
            .dispatch(self.fuse(build))
            .invoke(move |failure| {
                let (kind, error) = match failure {
                    ListenerFailure::Threw(error) => ("threw", error),
                    ListenerFailure::Rejected(error) => ("rejected", error),
                };
                log.warn(format_args!(
                    "agent event \"{}\" listener {kind}: {}",
                    E::NAME,
                    js_error_text(&error)
                ));
            });
    }

    /// Awaited in-order dispatch (Cordis `serial`) in the agent's scope.
    pub async fn serial<E, R>(
        &self,
        build: impl FnOnce(Arc<dyn Agent>) -> E,
    ) -> Result<Option<R>, BoxError>
    where
        E: AgentSubjectEvent + Event<Mode = Serial<R>>,
        R: Send + 'static,
    {
        self.scoped().serial::<E, R>(self.fuse(build)).await
    }

    /// Around-middleware dispatch (Cordis `waterfall`) in the agent's scope; `tail` is the
    /// innermost `next` the listener chain wraps.
    pub async fn waterfall<E, R, Fut>(
        &self,
        build: impl FnOnce(Arc<dyn Agent>) -> E,
        tail: impl FnOnce(Arc<E>) -> Fut + Send + 'static,
    ) -> Result<R, BoxError>
    where
        E: AgentSubjectEvent + Event<Mode = Waterfall<R>>,
        R: Send + 'static,
        Fut: Future<Output = Result<R, BoxError>> + Send + 'static,
    {
        self.scoped()
            .waterfall::<E, R, Fut>(self.fuse(build), tail)
            .await
    }
}

/// Emit one contained agent notification without keeping a dispatcher.
pub fn emit_agent_event<E>(
    ctx: &Context,
    agent: &Arc<dyn Agent>,
    build: impl FnOnce(Arc<dyn Agent>) -> E,
) where
    E: AgentSubjectEvent + Event<Mode = Contained>,
{
    agent_events(ctx, agent).emit(build);
}

/// The `agent` extension of the prompt assembly context.
#[derive(Clone)]
pub struct AssembleAgent(pub Arc<dyn Agent>);

/// The `modelSelection` extension of the prompt assembly context: the captured step selection.
#[derive(Clone)]
pub struct AssembleModelSelection(pub crate::types::ModelSelection);

/// Build the prompt assembly context with agent and scope set together, so agent-scoped prompt
/// and tool contributions cannot be silently omitted.
pub fn assemble_context_for(
    agent: &Arc<dyn Agent>,
    signal: Option<AbortSignal>,
) -> AssembleContext {
    let mut context =
        AssembleContext::new(Some(agent.scope_key())).with_extension(AssembleAgent(agent.clone()));
    context.signal = signal;
    context
}
