//! Shared contracts for terminal backends, the registry, and consumers.

use futures_util::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use hoh_agent::Agent;
use hoh_error::{BoxError, SharedError};
use hoh_timeout::AbortSignal;
use serde::{Deserialize, Serialize};
use std::{fmt, future::Future, sync::Arc};

hoh_brand::branded_string!(pub TerminalSessionId);

/// Brand one registry-minted string as a terminal session identity.
pub fn terminal_session_id(value: impl Into<String>) -> TerminalSessionId {
    TerminalSessionId::from(value.into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalWaitReason {
    StdinRead,
    InferredIdle,
    Timeout,
    SessionExit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalSignal {
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerminalSessionStatus {
    Running,
    Exited {
        #[serde(rename = "exitCode")]
        exit_code: Option<i32>,
        signal: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSpawnRequest {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[derive(Clone)]
pub struct TerminalBackendSpawnSpec {
    pub session_id: TerminalSessionId,
    pub owner: Arc<dyn Agent>,
    pub kind: String,
    pub name: Option<String>,
    pub cwd: Option<String>,
    pub signal: Option<AbortSignal>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TerminalSendRequest {
    pub text: String,
    pub submit: bool,
    #[serde(skip)]
    pub signal: Option<AbortSignal>,
}

impl fmt::Debug for TerminalSendRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalSendRequest")
            .field("text", &"<redacted>")
            .field("submit", &self.submit)
            .field("signal", &self.signal.as_ref().map(|_| "<abort-signal>"))
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSendRead {
    pub delta: String,
    pub truncated: bool,
}

impl fmt::Debug for TerminalSendRead {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalSendRead")
            .field("delta", &"<redacted>")
            .field("truncated", &self.truncated)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSendResult {
    pub viewport: String,
    pub wait_reason: TerminalWaitReason,
    pub session_status: TerminalSessionStatus,
    pub truncated: bool,
}

impl fmt::Debug for TerminalSendResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalSendResult")
            .field("viewport", &"<redacted>")
            .field("wait_reason", &self.wait_reason)
            .field("session_status", &self.session_status)
            .field("truncated", &self.truncated)
            .finish()
    }
}

pub(crate) type SharedSendResult =
    Shared<BoxFuture<'static, Result<TerminalSendResult, SharedError>>>;

/// Live backend-owned send; the settlement is memoized like one JavaScript Promise.
#[derive(Clone)]
pub struct TerminalSendOperation {
    done: SharedSendResult,
    read_output: Arc<dyn Fn() -> TerminalSendRead + Send + Sync>,
    cancel: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl TerminalSendOperation {
    pub fn new<F>(
        done: F,
        read_output: impl Fn() -> TerminalSendRead + Send + Sync + 'static,
        cancel: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self
    where
        F: Future<Output = Result<TerminalSendResult, BoxError>> + Send + 'static,
    {
        let done = async move { done.await.map_err(SharedError::new) }
            .boxed()
            .shared();
        Self {
            done,
            read_output: Arc::new(read_output),
            cancel: Arc::new(cancel),
        }
    }

    pub(crate) fn with_done(&self, done: SharedSendResult) -> Self {
        Self {
            done,
            read_output: self.read_output.clone(),
            cancel: self.cancel.clone(),
        }
    }

    pub(crate) fn shared_done(&self) -> SharedSendResult {
        self.done.clone()
    }

    pub fn done(&self) -> BoxFuture<'static, Result<TerminalSendResult, BoxError>> {
        let done = self.done.clone();
        Box::pin(async move { done.await.map_err(|error| Box::new(error) as BoxError) })
    }

    pub fn read_output(&self) -> TerminalSendRead {
        (self.read_output)()
    }

    pub fn cancel(&self) -> bool {
        (self.cancel)()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalReadRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalReadResult {
    pub text: String,
    pub total_lines: u64,
    pub line_begin: u64,
    pub line_end: u64,
    pub truncated: bool,
}

impl fmt::Debug for TerminalReadResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalReadResult")
            .field("text", &"<redacted>")
            .field("total_lines", &self.total_lines)
            .field("line_begin", &self.line_begin)
            .field("line_end", &self.line_end)
            .field("truncated", &self.truncated)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSignalResult {
    pub delivered: bool,
    pub target_pgid: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionSnapshot {
    pub session_id: TerminalSessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub status: TerminalSessionStatus,
}

pub trait TerminalBackendSession: Send + Sync + 'static {
    fn motd(&self) -> &str;
    fn pid(&self) -> Option<u32>;
    fn start_send(&self, request: TerminalSendRequest) -> Result<TerminalSendOperation, BoxError>;
    fn read(&self, request: TerminalReadRequest) -> Result<TerminalReadResult, BoxError>;
    fn signal(
        &self,
        signal: TerminalSignal,
    ) -> BoxFuture<'static, Result<TerminalSignalResult, BoxError>>;
    fn status(&self) -> TerminalSessionStatus;
    fn close(&self, reason: String) -> BoxFuture<'static, Result<(), BoxError>>;
}

pub trait TerminalBackend: Send + Sync + 'static {
    fn kind(&self) -> &str;
    fn spawn(
        &self,
        spec: TerminalBackendSpawnSpec,
    ) -> BoxFuture<'static, Result<Arc<dyn TerminalBackendSession>, BoxError>>;
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSpawnResult {
    pub session_id: TerminalSessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub status: TerminalSessionStatus,
    pub motd: String,
}

impl fmt::Debug for TerminalSpawnResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalSpawnResult")
            .field("session_id", &self.session_id)
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("pid", &self.pid)
            .field("status", &self.status)
            .field("motd", &"<redacted>")
            .finish()
    }
}
