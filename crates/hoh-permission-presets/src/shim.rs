//! Temporary fixed interfaces for dependencies that are still being ported.

use hoh_error::BoxError;
use hoh_sandbox::SandboxMode;
use hoh_session::Session;
use serde_json::json;

/// SHIM(hoh-sandbox-policy): canonical `sandbox/mode` setter; replace when that crate exists.
pub(crate) fn set_sandbox_mode(session: &Session, mode: SandboxMode) -> Result<(), BoxError> {
    session.append("sandbox/mode", json!({ "mode": mode }), None)?;
    Ok(())
}

/// SHIM(hoh-commands): the closed command result used by `/permission`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommandResult {
    pub kind: &'static str,
    pub text: String,
}
