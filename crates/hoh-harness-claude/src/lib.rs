//! Claude Code as an external harness for HoH (HOH-18).
//!
//! HoH starts the user's installed `claude` CLI (`-p --input-format stream-json
//! --output-format stream-json --verbose`) and speaks its control protocol itself: `initialize`,
//! `interrupt`, `can_use_tool`. The Agent SDK is not used and not shipped. Everything about Claude
//! lives in this crate; the slot ([`hoh_external_harness`]) and the core know nothing of it.
//!
//! The package is a plugin: loading [`HarnessClaudePlugin`] registers the harness under the name
//! [`HARNESS_NAME`] with the slot's service; leaving it out of a profile removes Claude without a
//! change to any other crate. Logic and message schemas adapted from T3 Code (MIT), see `NOTICE`
//! and `PORT_NOTES.md`.

mod harness;
mod session;
pub mod version;

use std::path::PathBuf;
use std::sync::Arc;

use hoh_error::BoxError;
use hoh_external_harness::ExternalHarnesses;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use harness::{Access, ClaudeConfig, ClaudeHarness};

/// The name Claude is registered under: also the provider route of a session it drives.
pub const HARNESS_NAME: &str = "claude";

/// Configuration of the plugin.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Config {
    /// The `claude` program; `claude` in `PATH` when absent.
    pub executable: Option<String>,
    /// `confirm` (default) or `full`: chosen by the human in the window.
    pub access: Access,
}

#[derive(Inject)]
pub struct Deps {
    harnesses: Service<ExternalHarnesses>,
}

pub struct HarnessClaudePlugin;

impl Plugin for HarnessClaudePlugin {
    const NAME: &'static str = "HarnessClaude";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let mut claude = ClaudeConfig {
            access: config.get().access,
            ..ClaudeConfig::default()
        };
        if let Some(executable) = config.get().executable.as_deref() {
            claude.executable = PathBuf::from(executable);
        }
        let _registration =
            deps.harnesses
                .register(&ctx, HARNESS_NAME, Arc::new(ClaudeHarness::new(claude)))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_comes_from_the_configuration_and_defaults_to_confirm() {
        let config: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(config.access, Access::Confirm);
        let config: Config = serde_json::from_str(r#"{"access":"full"}"#).unwrap();
        assert_eq!(config.access, Access::Full);
        assert!(serde_json::from_str::<Config>(r#"{"access":"bypass"}"#).is_err());
    }
}
