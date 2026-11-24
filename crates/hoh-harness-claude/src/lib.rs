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

mod channel_plugins;
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

pub use harness::{ClaudeConfig, ClaudeHarness, DEFAULT_SETTING_SOURCES, SettingSources};

/// The name Claude is registered under: also the provider route of a session it drives.
pub const HARNESS_NAME: &str = "claude";

/// Configuration of the plugin.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Config {
    /// The `claude` program; `claude` in `PATH` when absent.
    pub executable: Option<String>,
    /// Whose settings Claude runs with: `personal` (the human's own, as in T3) or `isolated`
    /// (none of them). [`DEFAULT_SETTING_SOURCES`] when absent. Set in the profile, not by a
    /// workspace or a chat.
    pub setting_sources: Option<SettingSources>,
    /// Ignored, with a warning in the log. The access mode is chosen only by the user in the
    /// window (`hoh/executor/access/set`) and reaches Claude from the slot; a line here, from an
    /// older profile or written by hand, never changes it and never widens anything.
    pub access: Option<serde_json::Value>,
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
        if let Some(access) = &config.get().access {
            ctx.logger().warn(format_args!(
                "the configuration of {} names an access mode ({access}): it is ignored, the \
                 mode is chosen only by the user in the window (hoh/executor/access/set)",
                Self::NAME
            ));
        }
        let mut claude = ClaudeConfig::default();
        if let Some(executable) = config.get().executable.as_deref() {
            claude.executable = PathBuf::from(executable);
        }
        if let Some(sources) = config.get().setting_sources {
            claude.setting_sources = sources;
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
    fn an_access_mode_in_the_configuration_is_accepted_but_not_used() {
        // Accepted, so an older profile does not take the window down; nothing reads it but the
        // warning (the plugin builds its `ClaudeConfig`, which has no mode, without it).
        let config: Config = serde_json::from_str(r#"{"access":"full"}"#).unwrap();
        assert_eq!(config.access, Some(serde_json::json!("full")));
        let config: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(config.access, None);
        assert!(serde_json::from_str::<Config>(r#"{"other":1}"#).is_err());
    }

    #[test]
    fn the_setting_sources_are_named_in_the_configuration() {
        let config: Config = serde_json::from_str(r#"{"settingSources":"isolated"}"#).unwrap();
        assert_eq!(config.setting_sources, Some(SettingSources::Isolated));
        let config: Config = serde_json::from_str(r#"{"settingSources":"personal"}"#).unwrap();
        assert_eq!(config.setting_sources, Some(SettingSources::Personal));
        assert!(serde_json::from_str::<Config>(r#"{"settingSources":"all"}"#).is_err());
        assert_eq!(
            ClaudeConfig::default().setting_sources,
            DEFAULT_SETTING_SOURCES
        );
    }
}
