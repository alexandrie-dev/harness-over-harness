//! In-process subagent backend seeded from the parent's completed turns.

use futures_util::future::BoxFuture;
use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, Undo};
use hoh_session::SessionEvent;
use hoh_subagent::{
    ContinuableCreateRequest, ContinuableCreateSpec, SubagentCapabilities, SubagentProvider,
    Subagents,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Cordis/Loader plugin name.
pub const NAME: &str = "subagent-fork-in-process";

/// Required kernel services.
pub const INJECT: [&str; 1] = ["subagents"];

fn default_provider_name() -> String {
    "fork".into()
}

/// Provider registration configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Name registered on the subagent provider registry.
    #[serde(default = "default_provider_name")]
    pub provider_name: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider_name: default_provider_name(),
        }
    }
}

#[derive(Inject)]
pub struct Deps {
    subagents: Service<Subagents>,
}

fn completed_turn_prefix(parent: &dyn hoh_agent::Agent) -> Vec<Arc<SessionEvent>> {
    let events = parent.session().snapshot_events(None, None);
    let Some(last_end) = events
        .iter()
        .rposition(|event| event.event_type() == "turn/end")
    else {
        return Vec::new();
    };
    events[..=last_end].to_vec()
}

/// Continuable provider whose child session inherits a one-time balanced parent prefix.
pub struct ForkInProcessProvider {
    name: String,
}

impl ForkInProcessProvider {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl SubagentProvider for ForkInProcessProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> SubagentCapabilities {
        SubagentCapabilities {
            agent_options: true,
            agent_preset: false,
            output_schema: true,
            depth_limit: true,
        }
    }

    fn inherits_parent_context(&self) -> bool {
        true
    }

    fn supports_continuable(&self) -> bool {
        true
    }

    fn prepare_continuable(
        &self,
        request: ContinuableCreateRequest,
    ) -> Option<BoxFuture<'static, Result<ContinuableCreateSpec, BoxError>>> {
        // Capture before returning the future, matching Promise.resolve(sourceSnapshot).
        let seed = completed_turn_prefix(request.parent.as_ref());
        Some(Box::pin(async move {
            Ok(ContinuableCreateSpec {
                seed: (!seed.is_empty()).then_some(seed),
            })
        }))
    }
}

/// Plugin that registers the fork provider for its fiber lifetime.
pub struct SubagentForkInProcess;

impl Plugin for SubagentForkInProcess {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let registration =
            deps.subagents
                .register_provider(Arc::new(ForkInProcessProvider::new(
                    config.get().provider_name.clone(),
                )))?;
        ctx.effect("subagent-fork-in-process provider", move || {
            Undo::sync(move || drop(registration))
        })
        .map_err(SharedError::new)?;
        Ok(())
    }
}
