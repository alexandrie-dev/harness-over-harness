//! Workspace archive admission for running subagent descendants.

use std::{collections::HashSet, sync::Arc};

use hoh_agent::{Agent, AgentPreStep, AgentStatus, Agents, CancelOptions, PreStepDecision};
use hoh_kernel::{Context, Disposer, KernelError};
use hoh_session::{AgentCancelCause, Session, SessionHeader, SessionId};
use hoh_workspace::{
    SessionActivity, SessionActivityItem, WorkspaceRegistryService, WorkspaceSessionActivity,
    WorkspaceSessionStop,
};

use crate::projection_types::SubagentIdentityProjection;

type AncestorsOf = Arc<dyn Fn(&Arc<dyn Agent>) -> Option<Vec<SessionId>> + Send + Sync>;
type IdentityOf = Arc<dyn Fn(&Session) -> Option<SubagentIdentityProjection> + Send + Sync>;

/// Install activity, stop and archived-ancestor pre-step listeners.
pub fn install_subagent_archive_admission(
    ctx: &Context,
    resident_ancestors: AncestorsOf,
    identity_of: IdentityOf,
) -> Result<Vec<Disposer>, KernelError> {
    let agents = ctx.get::<Agents>();
    let context = ctx.clone();
    let activity_ancestors = resident_ancestors.clone();
    let activity_identity = identity_of.clone();
    let activity_agents = agents.clone();
    let activity = ctx.on_waterfall::<WorkspaceSessionActivity, Vec<SessionActivity>, _>(
        move |request, next| {
            let agents = activity_agents.clone();
            let context = context.clone();
            let ancestors = activity_ancestors.clone();
            let identity = activity_identity.clone();
            async move {
                let running = agents
                    .as_ref()
                    .map(|agents| {
                        agents
                            .list()
                            .into_iter()
                            .filter(|agent| {
                                agent.status() == AgentStatus::Running
                                    && ancestors_of(&context, agent, &ancestors)
                                        .contains(&request.session_id)
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let rest = next.run().await?;
                if running.is_empty() {
                    return Ok(rest);
                }
                let items = running
                    .into_iter()
                    .map(|agent| {
                        let label = identity(agent.session()).and_then(|identity| {
                            let (label, seq) = match identity {
                                SubagentIdentityProjection::OneShot { label, seq } => (label, seq),
                                SubagentIdentityProjection::Continuable { label, seq } => {
                                    (Some(label), seq)
                                }
                            };
                            agent.session().is_own_seq(seq).then_some(label).flatten()
                        });
                        SessionActivityItem {
                            id: agent.id().to_string(),
                            label,
                        }
                    })
                    .collect();
                Ok(std::iter::once(SessionActivity {
                    kind: "subagent".into(),
                    items: Some(items),
                })
                .chain(rest)
                .collect())
            }
        },
    )?;

    let stop_agents = agents.clone();
    let stop_context = ctx.clone();
    let stop_ancestors = resident_ancestors.clone();
    let logger = ctx.logger();
    let stop = ctx.on_parallel::<WorkspaceSessionStop, _>(move |request| {
        let agents = stop_agents.clone();
        let context = stop_context.clone();
        let ancestors = stop_ancestors.clone();
        let logger = logger.clone();
        async move {
            if let Some(agents) = agents {
                for agent in agents.list().into_iter().filter(|agent| {
                    agent.status() == AgentStatus::Running
                        && ancestors_of(&context, agent, &ancestors).contains(&request.session_id)
                }) {
                    if let Err(error) =
                        agent.cancel(AgentCancelCause::Parent, CancelOptions::default())
                    {
                        logger.warn(format_args!(
                            "subagent: cancelling \"{}\" for an archived Session failed: {error}",
                            agent.id()
                        ));
                    }
                }
            }
            Ok(())
        }
    })?;

    let pre_context = ctx.clone();
    let pre_ancestors = resident_ancestors;
    let pre_step = ctx.on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
        let context = pre_context.clone();
        let ancestors = pre_ancestors.clone();
        async move {
            let archived = context
                .get::<WorkspaceRegistryService>()
                .map(|registry| registry.archived_session_ids())
                .unwrap_or_default();
            if ancestors_of(&context, &event.agent, &ancestors)
                .iter()
                .any(|id| archived.contains(id))
            {
                Ok(PreStepDecision::Reject)
            } else {
                next.run().await
            }
        }
    })?;
    Ok(vec![activity, stop, pre_step])
}

fn ancestors_of(ctx: &Context, agent: &Arc<dyn Agent>, resident: &AncestorsOf) -> Vec<SessionId> {
    if let Some(ancestors) = resident(agent) {
        return ancestors;
    }
    let Some(agents) = ctx.get::<Agents>() else {
        return Vec::new();
    };
    let mut ancestors = Vec::new();
    let mut visited = HashSet::from([agent.id().clone()]);
    let mut header = serde_json::from_value::<SessionHeader>((*agent.session().header()).clone());
    while let Ok(current) = header {
        if current.origin.as_deref() != Some("subagent") {
            break;
        }
        let Some(parent_id) = current.parent_session else {
            break;
        };
        if !visited.insert(parent_id.clone()) {
            break;
        }
        ancestors.push(parent_id.clone());
        let Some(parent) = agents.get(&parent_id) else {
            break;
        };
        header = serde_json::from_value((*parent.session().header()).clone());
    }
    ancestors
}
