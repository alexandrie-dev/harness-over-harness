//! Execution types for the provider-independent shell executor seam.

use std::{future::Future, pin::Pin, sync::Arc};

use futures_util::{FutureExt, future::Shared};
use hoh_error::BoxError;
use hoh_sandbox::{SandboxEnforcement, SandboxExecutionPolicy, SandboxMode};
use hoh_timeout::AbortSignal;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub use hoh_subprocess::{
    CollectedOutput, DSH_ENV_PREFIX, DshEnvironment, DshEnvironmentKey, SubprocessOutputRead,
    SubprocessOutputReader,
};

pub type ShellFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, BoxError>> + Send + 'a>>;
pub type ShellDone = Shared<Pin<Box<dyn Future<Output = ()> + Send>>>;

pub fn shell_done(future: impl Future<Output = ()> + Send + 'static) -> ShellDone {
    future.boxed().shared()
}

#[derive(Clone)]
pub struct ShellObservedStreams {
    pub stdout: Arc<dyn SubprocessOutputReader>,
    pub stderr: Arc<dyn SubprocessOutputReader>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellSandboxInfo {
    pub mode: SandboxMode,
    pub denied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforcement: Option<SandboxEnforcement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_failed: Option<bool>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellExpiryPolicy {
    Kill,
    None,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellExecRequest {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_expiry: Option<ShellExpiryPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_max_bytes: Option<u64>,
    #[serde(skip)]
    pub signal: Option<AbortSignal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dsh_env: Option<DshEnvironment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_policy: Option<SandboxExecutionPolicy>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellExecSpec {
    pub command: String,
    pub workdir: String,
    pub timeout_ms: f64,
    pub on_expiry: ShellExpiryPolicy,
    pub stdout_max_bytes: u64,
    #[serde(skip)]
    pub signal: Option<AbortSignal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dsh_env: Option<DshEnvironment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_policy: Option<SandboxExecutionPolicy>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellRunResult {
    pub exit_code: Option<i64>,
    pub signal: Option<String>,
    pub timed_out: bool,
    pub aborted: bool,
    pub timeout_ms: f64,
    pub stdout: CollectedOutput,
    pub stderr: CollectedOutput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<ShellSandboxInfo>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellProcessStatus {
    Running,
    Completed,
    Killed,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellProcessRead {
    pub delta: String,
    pub lossy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_spill_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_spill_path: Option<String>,
}

pub trait ShellProcess: Send + Sync + 'static {
    fn status(&self) -> ShellProcessStatus;
    fn exit_code(&self) -> Option<i64>;
    fn signal(&self) -> Option<String>;
    fn done(&self) -> ShellDone;
    fn sandbox(&self) -> Option<ShellSandboxInfo> {
        None
    }
    fn read_output(&self) -> ShellProcessRead;
    fn observed(&self) -> ShellObservedStreams;
    fn kill(&self) -> bool;
}

pub trait ShellExecution: ShellProcess {
    fn result(&self) -> ShellFuture<'_, ShellRunResult>;
}
