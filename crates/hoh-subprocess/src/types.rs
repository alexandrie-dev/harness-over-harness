//! Fully specified subprocess requests and provider-owned process/terminal handles.

use futures_util::{FutureExt, future::Shared};
use hoh_error::{BoxError, SharedError};
use hoh_timeout::AbortSignal;
use indexmap::IndexMap;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    ser::{SerializeMap, SerializeStruct},
};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::io::{AsyncRead, AsyncWrite};

pub const DSH_ENV_PREFIX: &str = "DSH_";
/// The source's template-literal type is compile-time-only; no namespace validation is added.
pub type DshEnvironmentKey = String;
pub type DshEnvironment = IndexMap<DshEnvironmentKey, String>;
/// `None` is an explicit tombstone, distinct from an absent key.
pub type SubprocessEnvironment = IndexMap<String, Option<String>>;

fn serialize_environment<S: Serializer>(
    environment: &Option<SubprocessEnvironment>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let Some(environment) = environment else {
        return serializer.serialize_none();
    };
    let mut map = serializer.serialize_map(Some(
        environment.values().filter(|value| value.is_some()).count(),
    ))?;
    for (key, value) in environment {
        if let Some(value) = value {
            map.serialize_entry(key, value)?;
        }
    }
    map.end()
}

pub type SubprocessFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, BoxError>> + Send + 'a>>;
pub type SubprocessReadable = Box<dyn AsyncRead + Unpin + Send>;
pub type SubprocessWritable = Box<dyn AsyncWrite + Unpin + Send>;
pub trait SubprocessDuplex: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> SubprocessDuplex for T {}
pub type SubprocessControl = Box<dyn SubprocessDuplex>;

/// A repeatably awaitable result. Providers initiate the process and promise producer eagerly.
pub type SubprocessDone =
    Shared<Pin<Box<dyn Future<Output = Result<SubprocessOutcome, SharedError>> + Send>>>;

pub fn subprocess_done(
    future: impl Future<Output = Result<SubprocessOutcome, SharedError>> + Send + 'static,
) -> SubprocessDone {
    future.boxed().shared()
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectedOutput {
    pub text: String,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spill_path: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubprocessStdinMode {
    Ignore,
    Pipe,
    Data { data: String },
}

impl Serialize for SubprocessStdinMode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Ignore => serializer.serialize_str("ignore"),
            Self::Pipe => serializer.serialize_str("pipe"),
            Self::Data { data } => {
                let mut object = serializer.serialize_struct("SubprocessStdinMode", 1)?;
                object.serialize_field("data", data)?;
                object.end()
            }
        }
    }
}
impl<'de> Deserialize<'de> for SubprocessStdinMode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Named(String),
            Data { data: String },
        }
        match Wire::deserialize(deserializer)? {
            Wire::Named(value) if value == "ignore" => Ok(Self::Ignore),
            Wire::Named(value) if value == "pipe" => Ok(Self::Pipe),
            Wire::Named(value) => Err(serde::de::Error::unknown_variant(
                &value,
                &["ignore", "pipe"],
            )),
            Wire::Data { data } => Ok(Self::Data { data }),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubprocessSpill {
    pub max_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubprocessCollect {
    pub max_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spill: Option<SubprocessSpill>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubprocessOutputMode {
    Pipe,
    Inherit,
    Collect(SubprocessCollect),
}
impl Serialize for SubprocessOutputMode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Pipe => serializer.serialize_str("pipe"),
            Self::Inherit => serializer.serialize_str("inherit"),
            Self::Collect(collect) => collect.serialize(serializer),
        }
    }
}
impl<'de> Deserialize<'de> for SubprocessOutputMode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Named(String),
            Collect(SubprocessCollect),
        }
        match Wire::deserialize(deserializer)? {
            Wire::Named(value) if value == "pipe" => Ok(Self::Pipe),
            Wire::Named(value) if value == "inherit" => Ok(Self::Inherit),
            Wire::Named(value) => Err(serde::de::Error::unknown_variant(
                &value,
                &["pipe", "inherit"],
            )),
            Wire::Collect(collect) => Ok(Self::Collect(collect)),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SubprocessControlMode {
    Pipe,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubprocessStdio {
    pub stdin: SubprocessStdinMode,
    pub stdout: SubprocessOutputMode,
    pub stderr: SubprocessOutputMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control: Option<SubprocessControlMode>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubprocessSpawnSpec {
    pub argv: Vec<String>,
    pub cwd: String,
    pub stdio: SubprocessStdio,
    pub grace_ms: f64,
    /// Runtime-only cancellation; transports serialize their own requests without signals.
    #[serde(skip)]
    pub signal: Option<AbortSignal>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_environment"
    )]
    pub env: Option<SubprocessEnvironment>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubprocessOutcome {
    pub exit_code: Option<i64>,
    pub signal: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubprocessOutputRead {
    pub text: String,
    pub next_offset: u64,
    pub lossy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spill_path: Option<String>,
}

pub trait SubprocessOutputReader: Send + Sync {
    fn read_from(&self, from_byte: u64) -> SubprocessOutputRead;
}

#[derive(Clone, Default)]
pub struct SubprocessCollectedOutputs {
    pub stdout: Option<Arc<dyn SubprocessOutputReader>>,
    pub stderr: Option<Arc<dyn SubprocessOutputReader>>,
}

pub trait SubprocessLifecycle: Send + Sync {
    fn terminate(&self);
    fn wait_for_exit(&self, signal: Option<AbortSignal>) -> SubprocessFuture<'_, bool>;
}

pub struct SubprocessHandle {
    pub stdin: Option<SubprocessWritable>,
    pub stdout: Option<SubprocessReadable>,
    pub stderr: Option<SubprocessReadable>,
    pub control: Option<SubprocessControl>,
    pub collected: SubprocessCollectedOutputs,
    pub done: SubprocessDone,
    pub lifecycle: Arc<dyn SubprocessLifecycle>,
}
impl SubprocessHandle {
    pub fn terminate(&self) {
        self.lifecycle.terminate();
    }
    pub fn wait_for_exit(&self, signal: Option<AbortSignal>) -> SubprocessFuture<'_, bool> {
        self.lifecycle.wait_for_exit(signal)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SubprocessTerminalSignal {
    #[serde(rename = "SIGINT")]
    Sigint,
    #[serde(rename = "SIGTERM")]
    Sigterm,
    #[serde(rename = "SIGKILL")]
    Sigkill,
    #[serde(rename = "SIGTSTP")]
    Sigtstp,
    #[serde(rename = "SIGHUP")]
    Sighup,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SubprocessPlatform {
    Posix,
    Windows,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubprocessTerminalEnvironment {
    pub platform: SubprocessPlatform,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_shell: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubprocessTerminalSpawnSpec {
    pub argv: Vec<String>,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<IndexMap<String, String>>,
    pub rows: u64,
    pub cols: u64,
    pub terminal_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell_activity: Option<bool>,
    pub grace_ms: f64,
    #[serde(skip)]
    pub signal: Option<AbortSignal>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubprocessTerminalForeground {
    pub process_group_id: u64,
    pub input_waiting: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SubprocessTerminalActivityState {
    Idle,
    Busy,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubprocessTerminalActivity {
    pub state: SubprocessTerminalActivityState,
    pub revision: u64,
}

pub trait SubprocessTerminalControl: Send + Sync {
    fn write<'a>(&'a self, data: &'a str) -> SubprocessFuture<'a, ()>;
    fn resize(&self, cols: u64, rows: u64) -> SubprocessFuture<'_, ()>;
    fn inspect_foreground(&self) -> SubprocessFuture<'_, Option<SubprocessTerminalForeground>>;
    fn inspect_activity(&self) -> SubprocessFuture<'_, SubprocessTerminalActivity>;
    fn signal_foreground(&self, signal: SubprocessTerminalSignal) -> SubprocessFuture<'_, u64>;
    fn terminate(&self) -> SubprocessFuture<'_, ()>;
}

pub struct SubprocessTerminalHandle {
    pub pid: u64,
    pub output: SubprocessReadable,
    pub done: SubprocessDone,
    pub lifecycle: Arc<dyn SubprocessTerminalControl>,
}
impl SubprocessTerminalHandle {
    pub fn write<'a>(&'a self, data: &'a str) -> SubprocessFuture<'a, ()> {
        self.lifecycle.write(data)
    }
    pub fn resize(&self, cols: u64, rows: u64) -> SubprocessFuture<'_, ()> {
        self.lifecycle.resize(cols, rows)
    }
    pub fn inspect_foreground(&self) -> SubprocessFuture<'_, Option<SubprocessTerminalForeground>> {
        self.lifecycle.inspect_foreground()
    }
    pub fn inspect_activity(&self) -> SubprocessFuture<'_, SubprocessTerminalActivity> {
        self.lifecycle.inspect_activity()
    }
    pub fn signal_foreground(&self, signal: SubprocessTerminalSignal) -> SubprocessFuture<'_, u64> {
        self.lifecycle.signal_foreground(signal)
    }
    pub fn terminate(&self) -> SubprocessFuture<'_, ()> {
        self.lifecycle.terminate()
    }
}
