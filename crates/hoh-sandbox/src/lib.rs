//! Same-world process confinement vocabulary and provider seam.
pub mod diagnostics;
pub mod escalation;
pub mod locales;
pub mod roots;
pub use diagnostics::{
    RunnerFailureMatch, classify_runner_failure, is_runner_spawn_failure, matches_signature,
};
pub use escalation::*;
use hoh_error::{BoxError, HarnessError};
use hoh_session::SessionId;
use hoh_timeout::AbortSignal;
pub use roots::{
    SandboxInvalidRootError, canonical_path, canonical_path_checked, workspace_writable_roots,
    writable_roots,
};
use serde::{Deserialize, Serialize};
use std::{fmt, future::Future, pin::Pin};

fn is_false(value: &bool) -> bool {
    !value
}

pub type SandboxFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, BoxError>> + Send + 'a>>;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}
impl SandboxMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
}
impl fmt::Display for SandboxMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ConfinedSandboxMode {
    ReadOnly,
    WorkspaceWrite,
}
impl From<ConfinedSandboxMode> for SandboxMode {
    fn from(v: ConfinedSandboxMode) -> Self {
        match v {
            ConfinedSandboxMode::ReadOnly => Self::ReadOnly,
            ConfinedSandboxMode::WorkspaceWrite => Self::WorkspaceWrite,
        }
    }
}
impl fmt::Display for ConfinedSandboxMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        SandboxMode::from(*self).fmt(f)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxExecutionPolicy {
    pub mode: SandboxMode,
    pub workspace_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_writable_roots: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protected_roots: Option<Vec<String>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxPolicy {
    pub mode: ConfinedSandboxMode,
    pub workspace_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_writable_roots: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protected_roots: Option<Vec<String>>,
    /// Preserve host-wide writes while enforcing the non-escalatable protected-root mask.
    #[serde(default, skip_serializing_if = "is_false")]
    pub unrestricted_writes: bool,
    /// A host directory the runner binds read-write at the same path. The confined process creates
    /// `ready` in it as its first act, after the runner finished its path-based setup: the caller
    /// learns exactly when the paths it checked stopped mattering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_dir: Option<String>,
}
impl From<SandboxPolicy> for SandboxExecutionPolicy {
    fn from(p: SandboxPolicy) -> Self {
        Self {
            mode: p.mode.into(),
            workspace_root: p.workspace_root,
            additional_writable_roots: p.additional_writable_roots,
            session_id: p.session_id,
            protected_roots: p.protected_roots,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxEnforcement {
    Full,
    Partial,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerFailureRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_exit_codes: Option<Vec<i32>>,
    pub fatal_signatures: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub informational_lines: Option<Vec<String>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfinedArgv {
    pub argv: Vec<String>,
    pub enforcement: SandboxEnforcement,
    pub denial_signatures: Vec<String>,
    pub runner_failure_rules: Vec<RunnerFailureRule>,
}
pub const SANDBOX_UNAVAILABLE: &str = "SANDBOX_UNAVAILABLE";
pub struct SandboxUnavailableError;
impl SandboxUnavailableError {
    pub const NAME: &str = "SandboxUnavailableError";
    #[allow(clippy::new_ret_no_self)]
    pub fn new(mode: ConfinedSandboxMode, detail: Option<&str>) -> HarnessError {
        let mut message = format!(
            "sandbox mode \"{mode}\" is requested but no sandbox backend is usable on this host; refusing to run the command unconfined. Install bubblewrap or run a Landlock-enforcing kernel (Linux), ensure sandbox-exec is usable (macOS), or ensure the ACL restricted-token runner can start (Windows) — otherwise switch the consumer to danger-full-access."
        );
        if let Some(detail) = detail {
            message.push_str(&format!(" Runner failure: {detail}"));
        }
        HarnessError::named(Self::NAME, message, SANDBOX_UNAVAILABLE)
    }
}
pub const SANDBOX_PREPARATION_ERROR_CODES: [&str; 2] =
    ["SANDBOX_PREPARATION_FAILED", "SANDBOX_INVALID_TEMP_ROOT"];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxPreparationErrorCode {
    PreparationFailed,
    InvalidTempRoot,
}
impl SandboxPreparationErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PreparationFailed => SANDBOX_PREPARATION_ERROR_CODES[0],
            Self::InvalidTempRoot => SANDBOX_PREPARATION_ERROR_CODES[1],
        }
    }
}
pub struct SandboxPreparationError;
impl SandboxPreparationError {
    pub const NAME: &str = "SandboxPreparationError";
    #[allow(clippy::new_ret_no_self)]
    pub fn new(message: impl Into<String>, code: SandboxPreparationErrorCode) -> HarnessError {
        HarnessError::named(Self::NAME, message, code.as_str())
    }
}
pub trait SandboxProvider: Send + Sync {
    fn confine<'a>(
        &'a self,
        argv: &'a [String],
        policy: SandboxPolicy,
        signal: Option<AbortSignal>,
    ) -> SandboxFuture<'a, ConfinedArgv>;
}
hoh_kernel::service!(pub Sandbox:dyn SandboxProvider="sandbox");
