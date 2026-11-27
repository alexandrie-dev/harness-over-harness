//! The installed `codex app-server` as a HoH external harness (HOH-18).
//!
//! Authentication remains entirely inside the Codex executable. HoH uses the app-server protocol
//! for sessions and a separate ephemeral command for an optional title.

mod harness;
mod installation;
mod session;
mod title;
pub mod version;

use std::path::PathBuf;
use std::sync::Arc;

use hoh_error::BoxError;
use hoh_external_harness::ExternalHarnesses;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use harness::{CodexConfig, CodexHarness};

pub const HARNESS_NAME: &str = "codex";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Config {
    /// The Codex launcher. By default, search `PATH` and the standalone installation.
    pub executable: Option<String>,
    /// Accepted only for compatibility with hand-written profiles and ignored. Access is chosen
    /// by the privileged main-window method, never by package configuration.
    pub access: Option<serde_json::Value>,
}

#[derive(Inject)]
pub struct Deps {
    harnesses: Service<ExternalHarnesses>,
}

pub struct HarnessCodexPlugin;

impl Plugin for HarnessCodexPlugin {
    const NAME: &'static str = "HarnessCodex";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        if let Some(access) = &config.get().access {
            ctx.logger().warn(format_args!(
                "the configuration of {} names an access mode ({access}): it is ignored, the \
                 mode is chosen only by the user in the main window",
                Self::NAME
            ));
        }
        let mut codex = CodexConfig::default();
        if let Some(executable) = config.get().executable.as_deref() {
            codex.executable = PathBuf::from(executable);
        }
        let _registration =
            deps.harnesses
                .register(&ctx, HARNESS_NAME, Arc::new(CodexHarness::new(codex)))?;
        Ok(())
    }
}
