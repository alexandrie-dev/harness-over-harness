use crate::{
    model_selection::{AllowedModelRoute, assert_allowed_model_routes},
    model_selection_state::{
        ModelSelectionPolicyState, subagent_model_selection_policy,
        subagent_model_selection_projection_definition,
    },
};
use hoh_agent::AgentCreated;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, Volatile};
use hoh_session::{SessionSeq, Sessions};
use hoh_session_projection::{ProjectionHandle, SessionProjections};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default)]
    pub enabled: Volatile<bool>,
    #[serde(default)]
    pub allowed_models: Volatile<Vec<AllowedModelRoute>>,
}

#[derive(Inject)]
pub struct Deps {
    projections: Service<SessionProjections>,
    sessions: Service<Sessions>,
}

/// Live user preference sampled when a fresh top-level Agent is created.
#[derive(Clone)]
pub struct SubagentModelSelectionConfig {
    config: Cfg<Config>,
}

impl SubagentModelSelectionConfig {
    pub fn current(&self) -> Result<Option<Vec<AllowedModelRoute>>, BoxError> {
        current(&self.config.get())
    }
}

hoh_kernel::service!(
    pub SubagentModelSelection: SubagentModelSelectionConfig = "subagentModelSelection"
);

fn current(config: &Config) -> Result<Option<Vec<AllowedModelRoute>>, BoxError> {
    assert_allowed_model_routes(&config.allowed_models.0)?;
    if config.enabled.0 && config.allowed_models.0.is_empty() {
        return Err("enabled subagent model selection requires at least one allowed model".into());
    }
    Ok(config.enabled.0.then(|| config.allowed_models.0.clone()))
}

fn initialize_agent_policy(
    handle: &ProjectionHandle<ModelSelectionPolicyState>,
    sessions: &hoh_session::SessionStore,
    config: &Config,
    event: &AgentCreated,
) -> Result<(), BoxError> {
    let target = event.agent.session();
    if subagent_model_selection_policy(handle, target)?.is_some() {
        return Ok(());
    }
    let header = target.header();
    let parent_id = (header.get("origin").and_then(|value| value.as_str()) == Some("subagent"))
        .then(|| header.get("parentSession").and_then(|value| value.as_str()))
        .flatten();
    let allowed = if let Some(parent_id) = parent_id {
        sessions
            .get(&hoh_session::SessionId::from(parent_id))
            .map(|parent| subagent_model_selection_policy(handle, &parent))
            .transpose()?
            .flatten()
    } else if u64::from(target.first_live_seq()) == 0
        && target
            .event_at(SessionSeq::default())
            .is_none_or(|event| event.event_type() != "session/end-seed")
    {
        current(config)?
    } else {
        None
    };
    if let Some(allowed_models) = allowed {
        target.append(
            "subagent/model-selection-policy",
            json!({ "allowedModels": allowed_models }),
            None,
        )?;
    }
    Ok(())
}

pub struct SubagentModelSelectionSettings;

impl Plugin for SubagentModelSelectionSettings {
    const NAME: &'static str = "subagent-model-selection-settings";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        ctx.provide::<SubagentModelSelection>(std::sync::Arc::new(SubagentModelSelectionConfig {
            config: config.clone(),
        }))
        .await?;
        // Subscribe before awaiting the deliberately yielding registration so an Agent created
        // in the same turn cannot miss policy initialization.
        let registration = deps
            .projections
            .register(&ctx, subagent_model_selection_projection_definition());
        let handle_cell = std::sync::Arc::new((
            std::sync::Mutex::new(
                None::<Result<ProjectionHandle<ModelSelectionPolicyState>, String>>,
            ),
            tokio::sync::Notify::new(),
        ));
        let listener_cell = handle_cell.clone();
        let sessions = deps.sessions.clone();
        let settings = config.clone();
        ctx.on_serial::<AgentCreated, (), _>(move |event| {
            let cell = listener_cell.clone();
            let sessions = sessions.clone();
            let config = settings.clone();
            async move {
                // `policyReady` of the source: a failed registration rejects Agent publication
                // instead of leaving it waiting.
                let handle = loop {
                    let notified = cell.1.notified();
                    if let Some(ready) = cell
                        .0
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone()
                    {
                        break ready.map_err(|message| -> BoxError { message.into() })?;
                    }
                    notified.await;
                };
                initialize_agent_policy(&handle, &sessions, &config.get(), &event)?;
                Ok(Some(()))
            }
        })?;
        let registered = registration.await;
        *handle_cell
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(
            registered
                .as_ref()
                .map(Clone::clone)
                .map_err(|error| error.to_string()),
        );
        handle_cell.1.notify_waiters();
        registered?;
        Ok(())
    }
}
