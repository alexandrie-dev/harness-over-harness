//! Fixtures that hold subagent children until the parent's log reaches a point
//! (`snapshots/session/subagent-spawn-in-process/settlement-order.mjs`, `subagent-parallel/execution-order.mjs`).
use super::SNAPSHOTS;
use hoh_agent::{AgentPreStep, Agents, PreStepDecision};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, Undo};
use hoh_loader::PluginRegistry;
use hoh_session::{SessionEventEmitted, SessionId};
use hoh_subagent::{SubagentRunEndInfo, Subagents};
use hoh_timeout::AbortSignal;
use hoh_tools::{ToolExecutionResult, Tools, ToolsExecute};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::watch;

/// `Promise.withResolvers()`: pending until resolved, or rejected when the fixture is disposed.
#[derive(Clone)]
struct Gate {
    state: Arc<watch::Sender<u8>>,
    disposed: &'static str,
}
impl Gate {
    fn new(disposed: &'static str) -> Self {
        Self {
            state: Arc::new(watch::channel(0).0),
            disposed,
        }
    }
    fn settle(&self, to: u8) {
        self.state.send_if_modified(|s| {
            let pending = *s == 0;
            if pending {
                *s = to;
            }
            pending
        });
    }
    fn resolve(&self) {
        self.settle(1);
    }
    fn reject(&self) {
        self.settle(2);
    }
    /// Wait for the gate, or for the signal's abort (`wait` in the TypeScript fixtures).
    async fn wait(&self, signal: &AbortSignal) -> Result<(), BoxError> {
        signal.throw_if_aborted()?;
        let mut state = self.state.subscribe();
        let rejected = tokio::select! {
            settled = state.wait_for(|s| *s != 0) => *settled? == 2,
            reason = signal.cancelled() => return Err(reason.into()),
        };
        signal.throw_if_aborted()?;
        if rejected {
            return Err(self.disposed.into());
        }
        Ok(())
    }
}

fn parent_of(header: &serde_json::Value) -> Option<SessionId> {
    header["parentSession"].as_str().map(SessionId::from)
}

/// `settlement-order.mjs`: hold the child's first step until the parent has recorded its first
/// `turn/end`, so the background-child notice follows the parent's first turn.
pub struct SubagentSpawnSettlementOrder;
impl Plugin for SubagentSpawnSettlementOrder {
    const NAME: &'static str = "subagent-spawn-settlement-order";
    type Config = ();
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<()>) -> Result<(), BoxError> {
        let finished = Gate::new("subagent settlement fixture disposed");
        let parent: Arc<Mutex<Option<SessionId>>> = Arc::default();
        let on_dispose = finished.clone();
        ctx.effect("subagent-spawn-settlement-order", move || {
            Undo::sync(move || on_dispose.reject())
        })?;
        let seen = parent.clone();
        ctx.listen()
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| {
                if event.exec.name == "subagent"
                    && let Some(agent) = &event.exec.agent
                {
                    *seen.lock().unwrap_or_else(PoisonError::into_inner) = Some(agent.id().clone());
                }
                async move { next.run().await }
            })?;
        let seen = parent.clone();
        let gate = finished.clone();
        ctx.on_contained::<SessionEventEmitted>(move |event| {
            let id = seen.lock().unwrap_or_else(PoisonError::into_inner).clone();
            if id.as_ref() == Some(event.session.id())
                && event.event.event_type() == "turn/end"
                && event.event.data()["turn"] == 1
            {
                gate.resolve();
            }
            Ok(())
        })?;
        ctx.listen()
            .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
                let parent = parent
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                let gate = finished.clone();
                async move {
                    let child = parent.is_some()
                        && parent_of(&event.agent.session().header()) == parent
                        && event.turn == 1
                        && event.step == 1;
                    if child {
                        gate.wait(&event.signal).await?;
                    }
                    next.run().await
                }
            })?;
        Ok(())
    }
}

#[derive(Inject)]
pub struct ParallelDeps {
    #[allow(dead_code)]
    tools: Service<Tools>,
    agents: Service<Agents>,
    subagents: Service<Subagents>,
}

#[derive(Default)]
struct Order {
    parent: Option<SessionId>,
    children: Vec<SessionId>,
    overlapping: bool,
}

/// `execution-order.mjs`: keep two child activations resident while ordering their accepted ids
/// and terminal notices.
pub struct SubagentParallelOrder;
impl Plugin for SubagentParallelOrder {
    const NAME: &'static str = "subagent-parallel-order";
    type Config = ();
    type Deps = ParallelDeps;
    async fn apply(ctx: Context, deps: ParallelDeps, _: Cfg<()>) -> Result<(), BoxError> {
        const DISPOSED: &str = "parallel subagent fixture disposed";
        let first_recorded = Gate::new(DISPOSED);
        let both_accepted = Gate::new(DISPOSED);
        let first_settled = Gate::new(DISPOSED);
        let gates = [
            first_recorded.clone(),
            both_accepted.clone(),
            first_settled.clone(),
        ];
        ctx.effect("subagent-parallel-order", move || {
            Undo::sync(move || gates.iter().for_each(Gate::reject))
        })?;
        let order: Arc<Mutex<Order>> = Arc::default();
        let lock = |order: &Arc<Mutex<Order>>| {
            order
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .parent
                .clone()
        };

        let (state, agents) = (order.clone(), deps.agents.clone());
        let (recorded, accepted) = (first_recorded.clone(), both_accepted.clone());
        ctx.on_contained::<SessionEventEmitted>(move |event| {
            let mut order = state.lock().unwrap_or_else(PoisonError::into_inner);
            if order.parent.as_ref() != Some(event.session.id()) {
                return Ok(());
            }
            let data = event.event.data();
            match event.event.event_type() {
                "tool/result" if data["message"]["source"]["callId"] == "call_parallel_alpha_1" => {
                    recorded.resolve();
                }
                "subagent/catalog" => {
                    if let Some(id) = data["childId"].as_str() {
                        order.children.push(SessionId::from(id));
                    }
                    if order.children.len() == 2 {
                        order.overlapping =
                            order.children.iter().all(|id| agents.get(id).is_some());
                        accepted.resolve();
                    }
                }
                _ => {}
            }
            Ok(())
        })?;

        let (state, settled) = (order.clone(), first_settled.clone());
        ctx.on_contained::<SubagentRunEndInfo>(move |info| {
            let order = state.lock().unwrap_or_else(PoisonError::into_inner);
            if order.children.first() == Some(&info.id) {
                settled.resolve();
            }
            Ok(())
        })?;

        let (state, subagents) = (order.clone(), deps.subagents.clone());
        ctx.listen()
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| {
                let (state, subagents, recorded) =
                    (state.clone(), subagents.clone(), first_recorded.clone());
                async move {
                    if event.exec.name != "subagent" {
                        return next.run().await;
                    }
                    let Some(agent) = event.exec.agent.clone() else {
                        return Err("subagent call without a calling Agent".into());
                    };
                    state.lock().unwrap_or_else(PoisonError::into_inner).parent =
                        Some(agent.id().clone());
                    let second = event.exec.call_id.as_str() == "call_parallel_alpha_2";
                    if second {
                        recorded.wait(&event.exec.signal()).await?;
                    }
                    let result = next.run().await?;
                    if second {
                        let overlapping = state
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .overlapping;
                        if !overlapping {
                            return Err(
                                "parallel fixture requires two simultaneous resident children"
                                    .into(),
                            );
                        }
                        subagents.wait_for_tree_work(&agent).await?;
                    }
                    Ok(result)
                }
            })?;

        ctx.listen()
            .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
                let (state, accepted, settled) =
                    (order.clone(), both_accepted.clone(), first_settled.clone());
                async move {
                    let parent = lock(&state);
                    let first_step = event.turn == 1 && event.step == 1;
                    if parent.is_none()
                        || parent_of(&event.agent.session().header()) != parent
                        || !first_step
                    {
                        return next.run().await;
                    }
                    accepted.wait(&event.signal).await?;
                    let (overlapping, second) = {
                        let order = state.lock().unwrap_or_else(PoisonError::into_inner);
                        (
                            order.overlapping,
                            order.children.get(1) == Some(event.agent.id()),
                        )
                    };
                    if !overlapping {
                        return Err(
                            "parallel fixture released before both children became resident".into(),
                        );
                    }
                    if second {
                        settled.wait(&event.signal).await?;
                    }
                    next.run().await
                }
            })?;
        Ok(())
    }
}

pub fn add(registry: &mut PluginRegistry) {
    registry
        .add::<SubagentSpawnSettlementOrder>(&format!(
            "{SNAPSHOTS}session/subagent-spawn-in-process/settlement-order.mjs"
        ))
        .add::<SubagentParallelOrder>(&format!(
            "{SNAPSHOTS}session/subagent-parallel/execution-order.mjs"
        ));
}
