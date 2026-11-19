//! Keyless StreamChunk replay, bound by first live-session use.
mod adapter;
mod config;
mod fixture;
mod replay;
mod script;
mod shim;
pub use config::*;
pub use fixture::{
    ParsedSessionHeader, parse_session_header, parse_session_log,
    prepare_session_snapshot_fixture_for_comparison,
};
use hoh_error::BoxError;
use hoh_llm::StreamChunk;
pub use replay::{ReplayHandle, install_llm_replay};
pub use script::{
    derive_replay_script, load_replay_script, load_session_scripts, resolve_scripted_entry,
};
use serde::{Deserialize, Serialize};
pub const NAME: &str = "llm-replay";
pub const INJECT: &[&str] = &["llm"];
pub type Result<T> = std::result::Result<T, BoxError>;
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("{0}")]
    Message(String),
    #[error("{message}")]
    UnsupportedMigration {
        message: String,
        #[source]
        cause: Option<BoxError>,
    },
    #[error("{message}")]
    Format {
        message: String,
        #[source]
        cause: BoxError,
    },
    #[error("{message}")]
    Io {
        message: String,
        #[source]
        cause: std::io::Error,
    },
}
fn error(message: impl Into<String>) -> BoxError {
    Box::new(ReplayError::Message(message.into()))
}
fn lock<T>(value: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn json(value: &serde_json::Value) -> String {
    hoh_util_values::js_json_stringify(value)
}
fn fs_error(cause: std::io::Error, path: &str) -> BoxError {
    Box::new(ReplayError::Io {
        message: hoh_error::node_fs_error_message(&cause, "open", Some(path), None),
        cause,
    })
}
fn read(path: &str) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|e| fs_error(e, path))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ReplayEntry {
    Chunks {
        chunks: Vec<StreamChunk>,
    },
    Throw {
        chunks: Vec<StreamChunk>,
        message: String,
        code: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        accepted: Option<bool>,
    },
    Hang {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ready_file: Option<String>,
    },
    /// A sidecar entry whose known chunk tags have not yet been narrowed to typed chunks.
    /// The file reader validates the entry envelope and chunk tags, preserving other fields.
    #[serde(untagged)]
    Unvalidated(serde_json::Value),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplayOverridePatch {
    pub at: usize,
    pub entry: ReplayEntry,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReplayOverrideDoc {
    Replace(Vec<ReplayEntry>),
    Patches { patches: Vec<ReplayOverridePatch> },
}
#[derive(Clone, Debug)]
pub struct SessionScript {
    pub recorded_id: String,
    pub created_at: i64,
    pub entries: Vec<ReplayEntry>,
    pub primary: bool,
}
#[derive(hoh_kernel::Inject)]
pub struct Deps {
    pub llm: hoh_kernel::Service<hoh_llm::Llm>,
}
pub struct LlmReplay;
impl hoh_kernel::Plugin for LlmReplay {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;
    async fn apply(
        ctx: hoh_kernel::Context,
        _deps: Deps,
        config: hoh_kernel::Cfg<Config>,
    ) -> Result<()> {
        apply(ctx, (*config.get()).clone()).await
    }
}
pub async fn apply(ctx: hoh_kernel::Context, config: Config) -> Result<()> {
    let config = config.resolve()?;
    replay::validate_pace(config.pace_ms)?;
    let (config, scripts) = tokio::task::spawn_blocking(move || {
        let scripts = load_session_scripts(&config)?;
        Ok::<_, BoxError>((config, scripts))
    })
    .await??;
    replay::install_scripts(&ctx, &config, scripts)?;
    Ok(())
}
