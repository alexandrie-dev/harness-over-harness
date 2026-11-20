//! Execution-time authority checks for the model-facing goal tools.

use std::sync::Arc;

use hoh_agent::{Agent, AgentRegistry, AgentStatus, StepBoundaryKind};
use hoh_error::{BoxError, HarnessError};
use hoh_goal::{GoalService, GoalView};
use hoh_session::{SessionEvent, SessionSeq};
use hoh_tools::ToolRunContext;
use serde_json::Value;

/// The calling agent plus the immutable event cut and open-turn start seq used for checks.
pub struct GoalToolExecution {
    pub agent: Arc<dyn Agent>,
    pub events: Arc<[Arc<SessionEvent>]>,
    pub open_turn_start_seq: SessionSeq,
}

/// Hard authority granted to one state-changing call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GoalToolAuthority {
    DirectHuman,
    GoalRound { goal: GoalView },
}

fn reject<T>(message: &str, code: &str) -> Result<T, BoxError> {
    Err(Box::new(HarnessError::new(message, code)))
}

/// Resolve and authenticate the calling agent and its driver boundary.
pub fn goal_tool_execution(
    agents: &AgentRegistry,
    exec: &ToolRunContext,
) -> Result<GoalToolExecution, BoxError> {
    let Some(agent) = exec.agent.clone() else {
        return reject(
            "goal tools require a calling agent",
            "GOAL_TOOL_AGENT_REQUIRED",
        );
    };
    let exact_live = agents
        .get(agent.id())
        .is_some_and(|live| Arc::ptr_eq(&live, &agent));
    if !exact_live || agent.status() != AgentStatus::Running {
        return reject(
            "goal tools require the exact live calling agent inside its active driver",
            "GOAL_TOOL_DRIVER_REQUIRED",
        );
    }
    let events = agent.session().snapshot_events(None, None);
    let boundary = agents.turn_boundary_of(agent.session())?;
    let Some(open_turn_start_seq) = boundary.open_turn_start_seq else {
        return reject(
            "goal tools require an open model turn",
            "GOAL_TOOL_DRIVER_REQUIRED",
        );
    };
    if !boundary
        .last_step_boundary
        .as_ref()
        .is_some_and(|step| step.kind == StepBoundaryKind::Start && step.seq > open_turn_start_seq)
    {
        return reject(
            "goal tools require an active model step",
            "GOAL_TOOL_DRIVER_REQUIRED",
        );
    }
    Ok(GoalToolExecution {
        agent,
        events,
        open_turn_start_seq,
    })
}

fn some_open_turn_event(
    execution: &GoalToolExecution,
    predicate: impl Fn(&SessionEvent) -> bool,
) -> bool {
    execution
        .events
        .iter()
        .skip(execution.open_turn_start_seq.get() as usize + 1)
        .any(|event| predicate(event))
}

fn source(event: &SessionEvent) -> Option<&serde_json::Map<String, Value>> {
    (event.event_type() == "user/message")
        .then(|| event.data().get("source")?.as_object())
        .flatten()
}

fn has_direct_human_input(agents: &AgentRegistry, execution: &GoalToolExecution) -> bool {
    let exact_live = agents
        .get(execution.agent.id())
        .is_some_and(|live| Arc::ptr_eq(&live, &execution.agent));
    let is_subagent = execution
        .agent
        .session()
        .header()
        .get("origin")
        .and_then(Value::as_str)
        == Some("subagent");
    exact_live
        && !is_subagent
        && some_open_turn_event(execution, |event| {
            source(event).and_then(|source| source.get("kind"))
                == Some(&Value::String("user".into()))
        })
}

fn is_matching_goal_round(execution: &GoalToolExecution, goal: &GoalView) -> bool {
    some_open_turn_event(execution, |event| {
        let Some(source) = source(event) else {
            return false;
        };
        source.get("kind").and_then(Value::as_str) == Some("goal")
            && source.get("goalId").and_then(Value::as_str) == Some(goal.goal.id.as_str())
            && source.get("revision").and_then(Value::as_u64) == Some(goal.goal.revision)
            && source.get("round").and_then(Value::as_u64) == Some(goal.rounds_started)
    })
}

/// Require authority originating in a human message accepted by a runtime root.
pub fn require_direct_human(
    agents: &AgentRegistry,
    execution: &GoalToolExecution,
) -> Result<(), BoxError> {
    if has_direct_human_input(agents, execution) {
        return Ok(());
    }
    reject(
        "this goal operation requires a direct human turn on a top-level agent",
        "GOAL_TOOL_AUTHORITY_REQUIRED",
    )
}

/// Resolve completion authority from direct human input or the exact admitted goal round.
pub fn completion_authority(
    agents: &AgentRegistry,
    goals: &GoalService,
    execution: &GoalToolExecution,
) -> Result<GoalToolAuthority, BoxError> {
    if has_direct_human_input(agents, execution) {
        return Ok(GoalToolAuthority::DirectHuman);
    }
    if let Some(goal) = goals.get(&execution.agent)?
        && is_matching_goal_round(execution, &goal)
    {
        return Ok(GoalToolAuthority::GoalRound { goal });
    }
    reject(
        "complete and blocked require a direct human turn or the current goal round",
        "GOAL_TOOL_AUTHORITY_REQUIRED",
    )
}
