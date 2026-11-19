//! Configuration normalization for instruction discovery and rendering.

use hoh_error::BoxError;
use hoh_home_paths::{resolve_agents_home, resolve_dsh_home};
use hoh_util_values::js_json_stringify;
use hoh_util_workspace_path::node_path;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};
use std::collections::HashSet;

pub const DEFAULT_MAX_SOURCE_BYTES: u64 = 1_048_576;

fn default_project_root_markers() -> Vec<String> {
    vec![".git".into()]
}

fn default_instruction_file_candidates() -> Vec<String> {
    vec!["AGENTS.md".into(), "CLAUDE.md".into()]
}

fn default_local_instruction_file_candidates() -> Vec<String> {
    vec!["AGENTS.local.md".into(), "CLAUDE.local.md".into()]
}

fn default_max_source_bytes() -> u64 {
    DEFAULT_MAX_SOURCE_BYTES
}

/// User-facing workspace instruction loader configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_project_root_markers")]
    pub project_root_markers: Vec<String>,
    pub max_bytes: f64,
    #[serde(default = "default_max_source_bytes")]
    #[schemars(range(min = 1))]
    pub max_source_bytes: u64,
    #[serde(default = "default_instruction_file_candidates")]
    pub instruction_file_candidates: Vec<String>,
    #[serde(default = "default_local_instruction_file_candidates")]
    pub local_instruction_file_candidates: Vec<String>,
}

/// Normalized discovery configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedDiscoveryConfig {
    pub dsh_home: String,
    pub agents_home: String,
    pub project_root_markers: Vec<String>,
    pub instruction_file_candidates: Vec<String>,
    pub local_instruction_file_candidates: Vec<String>,
}

/// Normalized configuration used by discovery and reconciliation.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedConfig {
    pub discovery: ResolvedDiscoveryConfig,
    pub max_bytes: f64,
    pub max_source_bytes: u64,
}

impl std::ops::Deref for ResolvedConfig {
    type Target = ResolvedDiscoveryConfig;

    fn deref(&self) -> &Self::Target {
        &self.discovery
    }
}

fn valid_candidates(candidates: &[String]) -> Vec<String> {
    let reserved = HashSet::from(["", ".", ".."]);
    candidates
        .iter()
        .filter(|candidate| {
            !reserved.contains(candidate.as_str())
                && !candidate.contains('/')
                && !candidate.contains('\\')
        })
        .cloned()
        .collect()
}

/// Resolve homes and filter candidates to same-directory file names.
pub fn resolve_discovery_config(
    config: &Config,
    dsh_home: Option<&str>,
    agents_home: Option<&str>,
) -> Result<ResolvedDiscoveryConfig, BoxError> {
    Ok(ResolvedDiscoveryConfig {
        dsh_home: resolve_dsh_home(dsh_home, None)?
            .to_string_lossy()
            .into_owned(),
        agents_home: resolve_agents_home(agents_home, None)?
            .to_string_lossy()
            .into_owned(),
        project_root_markers: config.project_root_markers.clone(),
        instruction_file_candidates: valid_candidates(&config.instruction_file_candidates),
        local_instruction_file_candidates: valid_candidates(
            &config.local_instruction_file_candidates,
        ),
    })
}

/// Resolve all runtime configuration.
pub fn resolve_config(config: &Config) -> Result<ResolvedConfig, BoxError> {
    Ok(ResolvedConfig {
        discovery: resolve_discovery_config(config, None, None)?,
        max_bytes: config.max_bytes,
        max_source_bytes: config.max_source_bytes,
    })
}

fn number(value: f64) -> Value {
    Number::from_f64(value).map_or(Value::Null, Value::Number)
}

/// Stable serialized identity for baseline compatibility checks.
pub fn workspace_baseline_identity(
    config: &ResolvedConfig,
    cwd: &str,
    project_root: &str,
) -> String {
    let mut value = Map::new();
    value.insert(
        "projectRoot".into(),
        Value::String(node_path::relative("/", cwd, project_root)),
    );
    value.insert(
        "projectRootMarkers".into(),
        serde_json::to_value(&config.project_root_markers).expect("string array serializes"),
    );
    value.insert("maxBytes".into(), number(config.max_bytes));
    value.insert(
        "maxSourceBytes".into(),
        Value::Number(config.max_source_bytes.into()),
    );
    value.insert(
        "instructionFileCandidates".into(),
        serde_json::to_value(&config.instruction_file_candidates).expect("string array serializes"),
    );
    value.insert(
        "localInstructionFileCandidates".into(),
        serde_json::to_value(&config.local_instruction_file_candidates)
            .expect("string array serializes"),
    );
    js_json_stringify(&Value::Object(value))
}
