//! Deprecated compatibility selector for native or PTC tool presentation.

use hoh_kernel::{Cfg, Context, Error, Inject, NoConfig, Plugin, Service};
use hoh_system_prompt::SystemPromptKey;
use hoh_tool_executor_native::NativeToolExecutorPlugin;
use hoh_tools::Tools;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The form in which an agent's model sees its tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PresentationMode {
    /// Send every visible tool schema directly.
    Native,
    /// Send only `run_code` plus a generated SDK.
    Ptc,
}

/// Compatibility selector configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Config {
    /// The form in which this agent's model sees its tools.
    pub mode: PresentationMode,
}

/// The selector only depends on the shared tool registry.
#[derive(Inject)]
pub struct Deps {
    _tools: Service<Tools>,
}

// SHIM(hoh-tool-executor-ptc): the package is a stage-9 redesign with no Rust crate;
// replace this dependency-only child when that crate provides its Plugin implementation.
mod ptc_shim {
    use super::*;

    /// Uninhabited until the redesign crate supplies the real runtime service value.
    pub(crate) enum PtcRuntimeValue {}
    hoh_kernel::service!(pub(crate) PtcRuntime: PtcRuntimeValue = "ptcRuntime");

    #[derive(Inject)]
    pub(crate) struct PtcDeps {
        _tools: Service<Tools>,
        _system_prompt: Service<SystemPromptKey>,
        _ptc_runtime: Service<PtcRuntime>,
    }

    pub(crate) struct PtcToolExecutorShim;

    impl Plugin for PtcToolExecutorShim {
        const NAME: &'static str = "ptc-tool-executor";
        type Config = NoConfig;
        type Deps = PtcDeps;

        async fn apply(
            _ctx: Context,
            _deps: Self::Deps,
            _config: Cfg<Self::Config>,
        ) -> Result<(), Error> {
            // No safe value can satisfy the uninhabited shim service. Fail closed if that
            // invariant is ever violated instead of pretending to provide a PTC executor.
            Err(std::io::Error::other(
                "PTC executor is unavailable until hoh-tool-executor-ptc is implemented",
            )
            .into())
        }
    }
}

/// Deprecated compatibility plugin selecting one child executor.
pub struct AgentToolPresentation;

impl Plugin for AgentToolPresentation {
    const NAME: &'static str = "tool-presentation";
    type Config = Config;
    type Deps = Deps;

    async fn apply(
        ctx: Context,
        _deps: Self::Deps,
        config: Cfg<Self::Config>,
    ) -> Result<(), Error> {
        match config.get().mode {
            PresentationMode::Native => {
                ctx.plugin::<NativeToolExecutorPlugin>(Value::Null)?;
            }
            PresentationMode::Ptc => {
                ctx.plugin::<ptc_shim::PtcToolExecutorShim>(Value::Null)?;
            }
        }
        Ok(())
    }
}
