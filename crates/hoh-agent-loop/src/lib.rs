//! Port of @deepseek-ai/dsh-agent-loop (packages/core/agent-loop): the concrete agent-loop plugin.

pub mod agent;
pub mod assistant_stream;
pub mod constants;
pub mod factory;
pub mod inbox;
pub mod runtime_context;

pub use agent::{LoopProjections, ReactLoopAgent};
pub use assistant_stream::AssistantStreamAttempt;
pub use constants::DEFAULT_MAX_PARALLEL_TOOL_CALLS;
pub use factory::{AgentLoop, AgentLoopKey, FactoryOwnership};
pub use inbox::{ReactLoopInbox, inbox_projection_definition};
pub use runtime_context::{
    RuntimeContextProjection, SystemPromptCommit, SystemPromptDecisionInput, SystemPromptProjection,
};

use hoh_agent::{
    Agent, AgentOptions, AgentSessionMeta, AgentSetup, Agents, AssembleAgent,
    AssembleModelSelection, Live, ModelSelection, ResumeAgentOptions,
    model_selection_projection_definition,
};
use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Cfg, Context, Event, Inject, Plugin, Service, Undo, Volatile};
use hoh_llm::error_chain;
use hoh_session::{SessionId, Sessions};
use hoh_session_persistence::{SessionPersistenceNotFoundError, SessionPersistenceService};
use hoh_system_prompt::SystemPromptKey;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};

use factory::Inner;
use hoh_session_projection::SessionProjections;

/// One launcher-selected session identity for a configured agent. `resume` distinguishes
/// rehydrating existing persisted history from creating the session fresh under that exact id,
/// which the two config keys express as `resumeSessionId` and `sessionId`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LauncherAgentIdentity {
    /// Exact session id to create fresh or resume.
    pub id: SessionId,
    /// Resume existing persisted history instead of creating the session fresh.
    pub resume: bool,
}

/// Launcher-selected identities keyed by the configured agent's `id`.
pub type ConfiguredAgentIdentities = HashMap<String, LauncherAgentIdentity>;

hoh_kernel::service!(pub ConfiguredAgentIdentitiesKey: ConfiguredAgentIdentities = "configuredAgentIdentities");

/// `agent-loop/config-start-failed`: a declarative agent entry failed before it could publish a
/// live agent. Consumers that buffer work for the configured identity use this transient signal to
/// reject that work instead of waiting forever. Normal factory teardown suppresses failures from
/// the cancelled startup attempt.
pub struct ConfigStartFailed {
    /// Exact shared agent/session identity that failed startup.
    pub session_id: SessionId,
    /// Persistence, setup, or publication failure.
    pub error: SharedError,
}
hoh_kernel::event!(
    ConfigStartFailed = "agent-loop/config-start-failed",
    hoh_kernel::Contained
);

fn default_max_parallel() -> Volatile<u64> {
    Volatile(DEFAULT_MAX_PARALLEL_TOOL_CALLS)
}

/// One configured agent entry.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConfiguredAgent {
    /// Stable config label used in logs and as the fresh combined-id prefix.
    pub id: String,
    /// Optional stable identity; remounts resume its materialized history, while first use
    /// creates it fresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 9007199254740991u64))]
    pub max_tokens: Option<u64>,
    /// Optional workspace for a fresh session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Persisted session to resume instead of creating a fresh session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_session_id: Option<String>,
}

/// Agent-loop plugin configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Maximum parallel-safe calls in flight per agent step. `1` is serial; omission defaults to
    /// [`DEFAULT_MAX_PARALLEL_TOOL_CALLS`].
    #[serde(default = "default_max_parallel")]
    #[schemars(range(min = 1))]
    pub max_parallel_tool_calls: Volatile<u64>,
    /// Agents created or resumed at plugin startup; a resumed agent records its configured route
    /// as the Session's model selection.
    #[serde(default)]
    pub agents: Vec<ConfiguredAgent>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_parallel_tool_calls: default_max_parallel(),
            agents: Vec::new(),
        }
    }
}

/// Apply launcher-owned identities over the configured agents, replacing both identity keys for
/// every entry the launcher named so a config-supplied identity can never survive alongside a
/// launcher-supplied one.
pub fn apply_launcher_identities(
    agents: &[ConfiguredAgent],
    identities: Option<&ConfiguredAgentIdentities>,
) -> Vec<ConfiguredAgent> {
    let Some(identities) = identities else {
        return agents.to_vec();
    };
    agents
        .iter()
        .map(|agent| {
            let Some(identity) = identities.get(&agent.id) else {
                return agent.clone();
            };
            let mut next = agent.clone();
            next.session_id = None;
            next.resume_session_id = None;
            if identity.resume {
                next.resume_session_id = Some(identity.id.to_string());
            } else {
                next.session_id = Some(identity.id.to_string());
            }
            next
        })
        .collect()
}

/// Reject self-contained identity conflicts before any configured agent starts.
pub fn validate_configured_agents(agents: &[ConfiguredAgent]) -> Result<(), BoxError> {
    let mut exact: HashMap<String, String> = HashMap::new();
    for agent in agents {
        let resume = agent
            .resume_session_id
            .as_deref()
            .filter(|id| !id.is_empty());
        if agent.session_id.is_some() && resume.is_some() {
            return Err(format!(
                "agent \"{}\": sessionId and resumeSessionId are mutually exclusive",
                agent.id
            )
            .into());
        }
        let Some(identity) = resume.or(agent.session_id.as_deref()) else {
            continue;
        };
        if let Some(first) = exact.get(identity) {
            return Err(format!(
                "agents \"{first}\" and \"{}\" use duplicate exact session identity \"{identity}\"",
                agent.id
            )
            .into());
        }
        exact.insert(identity.to_owned(), agent.id.clone());
    }
    Ok(())
}

fn agent_options_of(agent: &ConfiguredAgent) -> AgentOptions {
    AgentOptions {
        provider: agent.provider.clone().map(Live::Value),
        model: agent.model.clone().map(Live::Value),
        reasoning_effort: agent
            .reasoning_effort
            .as_deref()
            .map(|effort| Live::Value(effort.into())),
        max_tokens: agent.max_tokens.map(Live::Value),
    }
}

/// Record the configured route as the session's model selection, keeping a previously recorded
/// effort when the configured route has none and names the same provider/model.
fn apply_configured_route(agent: &Arc<dyn Agent>, options: &AgentOptions) -> Result<(), BoxError> {
    let (Some(provider), Some(model)) = (options.provider(), options.model()) else {
        return Ok(());
    };
    if provider.is_empty() || model.is_empty() {
        return Ok(());
    }
    let previous = agent.model().current();
    let mut selection = ModelSelection {
        provider,
        model,
        reasoning_effort: options.reasoning_effort(),
    };
    if selection.reasoning_effort.is_none()
        && let Some(previous) = previous
        && previous.provider == selection.provider
        && previous.model == selection.model
        && previous.reasoning_effort.is_some()
    {
        selection.reasoning_effort = previous.reasoning_effort;
    }
    agent.model().select(&selection)
}

fn route_setup(options: AgentOptions) -> AgentSetup {
    Arc::new(move |_ctx, agent| {
        let options = options.clone();
        Box::pin(async move {
            apply_configured_route(&agent, &options)?;
            Ok(None)
        })
    })
}

#[derive(Inject)]
pub struct Deps {
    _agents: Service<Agents>,
    _sessions: Service<Sessions>,
    _llm: Service<hoh_llm::Llm>,
    _tools: Service<hoh_tools::Tools>,
    system_prompt: Service<SystemPromptKey>,
    projections: Service<SessionProjections>,
}

/// The agent-loop plugin (`AgentLoop` service of the source).
pub struct AgentLoopPlugin;

impl Plugin for AgentLoopPlugin {
    const NAME: &'static str = "AgentLoop";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let initial = config.get();
        let agents = apply_launcher_identities(
            &initial.agents,
            ctx.get::<ConfiguredAgentIdentitiesKey>().as_deref(),
        );
        validate_configured_agents(&agents)?;
        let ownership = factory::FactoryOwnership::new(ctx.clone());
        // Agent projections remain readable until teardown finishes.
        {
            let ownership = ownership.clone();
            ctx.try_effect(
                "agentLoop.transactions()",
                move |undos| -> Result<(), BoxError> {
                    undos.push(Undo::future(
                        move || async move { ownership.dispose().await },
                    ))?;
                    Ok(())
                },
            )?;
        }
        let model_projection = Arc::new(
            deps.projections
                .register(&ctx, model_selection_projection_definition())
                .await?,
        );
        let inbox_projection = Arc::new(
            deps.projections
                .register(&ctx, inbox_projection_definition())
                .await?,
        );
        let max_parallel = {
            let config = config.clone();
            Arc::new(move || config.get().max_parallel_tool_calls.0)
        };
        let agent_loop = AgentLoop::new(Inner {
            ctx: ctx.clone(),
            ownership: ownership.clone(),
            model_projection,
            inbox_projection,
            max_parallel,
        });
        let registry = ctx.get::<Agents>().ok_or("agents service is unavailable")?;
        registry.set_factory(&ctx, Arc::new(agent_loop.clone()))?;
        deps.system_prompt.variable(&ctx, "provider", |context| {
            Ok(context
                .extension::<AssembleModelSelection>()
                .map(|selection| selection.0.provider.clone())
                .or_else(|| {
                    context
                        .extension::<AssembleAgent>()
                        .and_then(|agent| agent.0.options().provider())
                }))
        })?;
        deps.system_prompt.variable(&ctx, "model", |context| {
            Ok(context
                .extension::<AssembleModelSelection>()
                .map(|selection| selection.0.model.clone())
                .or_else(|| {
                    context
                        .extension::<AssembleAgent>()
                        .and_then(|agent| agent.0.options().model())
                }))
        })?;

        for configured in agents {
            agent_loop.start_configured(&ctx, configured)?;
        }
        ctx.provide::<AgentLoopKey>(Arc::new(agent_loop)).await?;
        Ok(())
    }
}

impl AgentLoop {
    fn start_configured(&self, ctx: &Context, configured: ConfiguredAgent) -> Result<(), BoxError> {
        let options = agent_options_of(&configured);
        let meta = AgentSessionMeta {
            cwd: configured.cwd.clone(),
            ..AgentSessionMeta::default()
        };
        let resume = configured
            .resume_session_id
            .clone()
            .filter(|id| !id.is_empty());
        let Some(resume) = resume else {
            let configured_id = configured.session_id.clone().map_or_else(
                || {
                    SessionId::from(format!(
                        "{}-session-{}",
                        configured.id,
                        hoh_util_crypto::random_uuid()
                    ))
                },
                SessionId::from,
            );
            let persistence = configured
                .session_id
                .as_ref()
                .and_then(|_| ctx.get::<SessionPersistenceService>());
            let this = self.clone();
            let config_id = configured.id.clone();
            let guard = self.inner.ownership.track_task();
            let ctx = ctx.clone();
            tokio::spawn(async move {
                let _guard = guard;
                let outcome = match persistence {
                    None => this
                        .create(configured_id.clone(), options, meta)
                        .await
                        .map(|_| ()),
                    Some(persistence) => {
                        this.restore_or_create_configured(
                            &ctx,
                            persistence,
                            configured_id.clone(),
                            options,
                            meta,
                        )
                        .await
                    }
                };
                if let Err(error) = outcome {
                    this.report_configured_startup_failure(
                        &config_id,
                        "restore",
                        configured_id,
                        error,
                    );
                }
            });
            return Ok(());
        };
        let this = self.clone();
        let config_id = configured.id.clone();
        let resume_id = SessionId::from(resume);
        let owner = ctx.clone();
        let label = format!("agentLoop.resume({config_id})");
        ctx.try_effect(label, move |undos| -> Result<(), BoxError> {
            let fiber = owner
                .clone()
                .inject(move |child: Context, deps: PersistenceDeps| {
                    let (this, owner, options, config_id, resume_id) = (
                        this.clone(),
                        owner.clone(),
                        options.clone(),
                        config_id.clone(),
                        resume_id.clone(),
                    );
                    async move {
                        let _ = child;
                        let guard = this.inner.ownership.track_task();
                        tokio::spawn(async move {
                            let _guard = guard;
                            let mut resume_options = ResumeAgentOptions::new(resume_id.clone());
                            resume_options.agent_options = Some(options.clone());
                            resume_options.setup = Some(route_setup(options));
                            if let Err(error) = this
                                .resume_with(owner, deps.persistence, resume_options)
                                .await
                            {
                                this.report_configured_startup_failure(
                                    &config_id, "resume", resume_id, error,
                                );
                            }
                        });
                        Ok(())
                    }
                })?;
            undos.push(Undo::future(move || async move {
                fiber.dispose().await;
                Ok(())
            }))?;
            Ok(())
        })?;
        Ok(())
    }

    /// Report a contained declarative-start failure to identity-bound consumers.
    fn report_configured_startup_failure(
        &self,
        config_id: &str,
        action: &str,
        session_id: SessionId,
        error: BoxError,
    ) {
        if !self.inner.ownership.is_active() {
            return;
        }
        let log = self.inner.ctx.logger();
        log.warn(format!(
            "agent \"{config_id}\": config-driven {action} of \"{session_id}\" failed: {}",
            error_chain(error.as_ref())
        ));
        let config_id = config_id.to_owned();
        self.inner
            .ctx
            .dispatch(ConfigStartFailed {
                session_id,
                error: SharedError::new(error),
            })
            .invoke(move |failure| {
                let (kind, error) = match failure {
                    hoh_kernel::ListenerFailure::Threw(error) => ("threw", error),
                    hoh_kernel::ListenerFailure::Rejected(error) => ("rejected", error),
                };
                log.warn(format!(
                    "agent \"{config_id}\": config-start-failed listener {kind}: {}",
                    error_chain(error.as_ref())
                ));
            });
    }

    /// Restore a materialized exact config identity on remount, or create it on first use.
    async fn restore_or_create_configured(
        &self,
        owner: &Context,
        persistence: Service<SessionPersistenceService>,
        session_id: SessionId,
        agent_options: AgentOptions,
        meta: AgentSessionMeta,
    ) -> Result<(), BoxError> {
        self.wait_for_draining_configured_identity(owner, &session_id)
            .await;
        if !self.inner.ownership.is_active() {
            return Ok(());
        }
        let mut options = ResumeAgentOptions::new(session_id.clone());
        options.agent_options = Some(agent_options.clone());
        options.setup = Some(route_setup(agent_options.clone()));
        match self.resume_with(owner.clone(), persistence, options).await {
            Ok(_) => return Ok(()),
            Err(error) => {
                if !self.inner.ownership.is_active() {
                    return Ok(());
                }
                // Only a genuinely absent stored session falls back to first creation;
                // corruption, ownership conflicts, and backend failures stay loud.
                if !is_not_found(error.as_ref()) {
                    return Err(error);
                }
            }
        }
        self.create(session_id, agent_options, meta)
            .await
            .map(|_| ())
    }

    /// Wait for a draining same-id lifecycle to finish registry teardown.
    async fn wait_for_draining_configured_identity(&self, owner: &Context, session_id: &SessionId) {
        let occupied = || {
            owner
                .get::<Agents>()
                .is_some_and(|agents| agents.get(session_id).is_some())
                || owner
                    .get::<Sessions>()
                    .is_some_and(|sessions| sessions.get(session_id).is_some())
        };
        // Only an id still occupying a registry needs waiting for; a live healthy occupant is a
        // collision the create/resume that follows will surface itself.
        self.inner
            .ownership
            .wait_while_active(async {
                while occupied() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await;
    }
}

fn is_not_found(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(error) = current {
        if error
            .downcast_ref::<SessionPersistenceNotFoundError>()
            .is_some()
        {
            return true;
        }
        if let Some(shared) = SharedError::of(error)
            && shared
                .downcast_ref::<SessionPersistenceNotFoundError>()
                .is_some()
        {
            return true;
        }
        current = error.source();
    }
    false
}

#[derive(Inject)]
struct PersistenceDeps {
    persistence: Service<SessionPersistenceService>,
}

#[doc(hidden)]
pub fn _event_marker<E: Event>() {}

#[cfg(test)]
extern crate self as hoh_agent_loop;
#[cfg(test)]
#[path = "window_tests.rs"]
mod window_tests;
