//! Loader of HoH: entries, groups, layered patches, recomposition (docs/kernel.md).

pub mod cmdline;
pub mod config_editor;
pub mod config_schema;
pub mod environment;
mod expr;
pub mod fail_loud;
mod identity;
mod patches;
mod profile;
mod profile_context;
mod profile_files;
pub mod profile_sanitize;
mod registry;
mod runtime;
pub mod startup_diagnostics;
mod yaml;
mod yaml_edit;

pub use registry::PluginRegistry;
pub use runtime::{
    Audit, BootOptions, ConfigReload, EntryChange, EntryInfo, EntryStatus, Group, Loader,
    LoaderKey, UpdateAction, boot,
};
pub use startup_diagnostics::{
    StartupCompositionDiagnostic, StartupError, StartupPluginState, StartupServiceState,
};

pub use environment::{load_env, load_layered_env, resolve_config_path};
pub use expr::{Expr, ExprError};
pub use fail_loud::{FAIL_LOUD_RELEASE_TIMEOUT_MS, install_fail_loud};
pub use patches::{apply_patches, compose, render_dump};
pub use profile::{
    Bundle, BundleSource, Profile, SkippedBundle, load_named_optional_patches,
    load_named_overlay_patches, load_optional_patches, read_named_profile_patches,
    read_profile_patches, resolve_telemetry_patch,
};
pub use profile_context::{ProfileContext, ProfileContextKey};
pub use profile_files::{
    init_profile, load_profile, load_profile_with_user_layer, read_profile_manifest,
    resolve_profile_dir, write_profile_manifest,
};
pub use yaml::{parse_entries, parse_patches};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub config: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub disabled: Value,
    #[serde(default, skip_serializing_if = "is_false")]
    pub group: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inject: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolate: Option<IndexMap<String, Isolate>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intercept: Option<IndexMap<String, Value>>,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Clone, Debug, PartialEq)]
pub enum Isolate {
    Private,
    Label(String),
}
impl schemars::JsonSchema for Isolate {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Isolate".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"anyOf":[{"const":true},{"type":"string"}]})
    }
}

impl Serialize for Isolate {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Private => serializer.serialize_bool(true),
            Self::Label(s) => serializer.serialize_str(s),
        }
    }
}
impl<'de> Deserialize<'de> for Isolate {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::Bool(true) => Ok(Self::Private),
            Value::String(s) => Ok(Self::Label(s)),
            _ => Err(serde::de::Error::custom(
                "isolate must be true or a string label",
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    /// Raw selector: JavaScript Map keys distinguish numeric `5` from string `"5"`.
    pub id: Option<Value>,
    pub name: Option<String>,
    pub set: IndexMap<String, Value>,
    pub insert: Option<Vec<Entry>>,
}

#[derive(Clone, Debug)]
pub enum LayerSource {
    Bundle(String),
    Profile,
    Home,
    CommandLine(PathBuf),
    Launcher(&'static str),
}
#[derive(Clone, Debug)]
pub struct Layer {
    pub source: LayerSource,
    pub file: Option<PathBuf>,
    pub patches: Vec<Patch>,
}
#[derive(Clone, Debug)]
pub struct Composition {
    pub entries: Vec<Entry>,
    pub origins: IndexMap<String, Origin>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Origin {
    pub created_by: String,
    pub patched_by: Vec<String>,
}

#[derive(Clone)]
pub struct EvalEnv {
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub exe: PathBuf,
    pub platform: &'static str,
    #[allow(clippy::type_complexity)] // Keep the binding §2.9 field signature.
    pub var: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("hoh: plugin tree failed to load: {0}")]
    Boot(#[source] Box<LoadError>),
    #[error("failed to {stage} config file {file}")]
    ConfigFile {
        stage: &'static str,
        file: PathBuf,
        #[source]
        source: Box<LoadError>,
    },
    #[error(transparent)]
    AtomicWrite(Box<dyn std::error::Error + Send + Sync>),
    #[error(transparent)]
    Startup(Box<startup_diagnostics::StartupError>),
    #[error(transparent)]
    Kernel(#[from] hoh_kernel::KernelError),
    #[error("{0}")]
    Runtime(String),
    #[error("{file}: {source}")]
    Io {
        file: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{file}:{line}:{column}: {message}")]
    Yaml {
        file: PathBuf,
        line: usize,
        column: usize,
        message: String,
    },
    #[error("{file}:{line}:{column}: {message}")]
    Shape {
        file: PathBuf,
        line: usize,
        column: usize,
        message: String,
    },
    #[error("{file}:{line}:{column}: invalid expression {expression:?}: {reason}")]
    Expr {
        file: PathBuf,
        line: usize,
        column: usize,
        expression: String,
        #[source]
        reason: ExprError,
    },
}

pub(crate) fn expression(value: &Value) -> Option<&str> {
    let map = value.as_object()?;
    (map.len() == 1)
        .then(|| map.get("__jsExpr")?.as_str())
        .flatten()
}

pub(crate) fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}
