//! In-process subagent backend that starts children with a fresh session.

use futures_util::future::BoxFuture;
use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, Undo};
use hoh_subagent::{
    ContinuableCreateRequest, ContinuableCreateSpec, SubagentCapabilities, SubagentProvider,
    Subagents,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Cordis/Loader plugin name.
pub const NAME: &str = "subagent-spawn-in-process";

/// Required kernel services. Tools are deliberately supplied by child composition instead.
pub const INJECT: [&str; 1] = ["subagents"];

fn default_provider_name() -> String {
    "spawn".into()
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

/// Continuable provider whose child begins with no inherited session events.
pub struct SpawnInProcessProvider {
    name: String,
}

impl SpawnInProcessProvider {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl SubagentProvider for SpawnInProcessProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> SubagentCapabilities {
        SubagentCapabilities {
            agent_options: true,
            agent_preset: true,
            output_schema: true,
            depth_limit: true,
        }
    }

    fn inherits_parent_context(&self) -> bool {
        false
    }

    fn supports_continuable(&self) -> bool {
        true
    }

    fn prepare_continuable(
        &self,
        _request: ContinuableCreateRequest,
    ) -> Option<BoxFuture<'static, Result<ContinuableCreateSpec, BoxError>>> {
        Some(Box::pin(async { Ok(ContinuableCreateSpec::default()) }))
    }
}

/// Plugin that registers the spawn provider for its fiber lifetime.
pub struct SubagentSpawnInProcess;

impl Plugin for SubagentSpawnInProcess {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let registration =
            deps.subagents
                .register_provider(Arc::new(SpawnInProcessProvider::new(
                    config.get().provider_name.clone(),
                )))?;
        ctx.effect("subagent-spawn-in-process provider", move || {
            Undo::sync(move || drop(registration))
        })
        .map_err(SharedError::new)?;
        Ok(())
    }
}
