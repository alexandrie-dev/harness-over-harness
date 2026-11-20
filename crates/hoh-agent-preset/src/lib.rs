//! Declarative preset rows registered with `hoh-agent-preset-registry`.

use hoh_agent_preset_registry::{AgentPresets, PresetDefinition};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_loader::PluginRegistry;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// Canonical Loader name of the ported package.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-agent-preset";

/// Configuration of one declarative preset row.
///
/// The registry intentionally stores the child rows as raw JSON, while this
/// boundary keeps the source package's stricter top-level array contract.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<f64>,
    pub plugins: Vec<Value>,
}

impl From<Config> for PresetDefinition {
    fn from(config: Config) -> Self {
        Self {
            id: config.id,
            name: config.name,
            description: config.description,
            order: config.order,
            plugins: Value::Array(config.plugins),
        }
    }
}

#[derive(Inject)]
pub struct Deps {
    presets: Service<AgentPresets>,
}

/// Registers a preset for exactly as long as this plugin context is alive.
pub struct AgentPreset;

impl Plugin for AgentPreset {
    const NAME: &'static str = "AgentPreset";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        deps.presets
            .register(&ctx, (*config.get()).clone().into())
            .await?;
        Ok(())
    }
}

/// Add the plugin with the Loader carrier marker used by the TypeScript row.
///
/// Carrier registration is what defers expressions inside `plugins` until the
/// child plugin owning each expression activates.
pub fn register_plugin(registry: &mut PluginRegistry) -> &mut PluginRegistry {
    registry
        .add::<AgentPreset>(PACKAGE_NAME)
        .carrier(PACKAGE_NAME)
        .resource_dir(PACKAGE_NAME, PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}
