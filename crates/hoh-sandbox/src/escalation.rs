use crate::{SandboxFuture, SandboxMode, locales::escalation_display_reason};
use hoh_error::{BoxError, HarnessError};
use hoh_timeout::AbortSignal;
use hoh_util_values::js_trim;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, LazyLock};
pub const SANDBOX_ESCALATION_ERROR_CODES: [&str; 4] = [
    "SANDBOX_ESCALATION_INVALID_ARGUMENT",
    "SANDBOX_ESCALATION_UNAVAILABLE",
    "SANDBOX_ESCALATION_REJECTED",
    "SANDBOX_ESCALATION_CANCELLED",
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxEscalationErrorCode {
    InvalidArgument,
    Unavailable,
    Rejected,
    Cancelled,
}
impl SandboxEscalationErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => SANDBOX_ESCALATION_ERROR_CODES[0],
            Self::Unavailable => SANDBOX_ESCALATION_ERROR_CODES[1],
            Self::Rejected => SANDBOX_ESCALATION_ERROR_CODES[2],
            Self::Cancelled => SANDBOX_ESCALATION_ERROR_CODES[3],
        }
    }
}
pub struct SandboxEscalationError;
impl SandboxEscalationError {
    pub const NAME: &str = "SandboxEscalationError";
    #[allow(clippy::new_ret_no_self)]
    pub fn new(message: impl Into<String>, code: SandboxEscalationErrorCode) -> HarnessError {
        HarnessError::named(Self::NAME, message, code.as_str())
    }
}
pub static WIDER_MODES: LazyLock<IndexMap<&'static str, Vec<SandboxMode>>> = LazyLock::new(|| {
    [
        (
            "read-only",
            vec![SandboxMode::WorkspaceWrite, SandboxMode::DangerFullAccess],
        ),
        ("workspace-write", vec![SandboxMode::DangerFullAccess]),
    ]
    .into_iter()
    .collect()
});
pub const ESCALATION_TARGETS: [SandboxMode; 2] =
    [SandboxMode::WorkspaceWrite, SandboxMode::DangerFullAccess];
pub fn validate_escalation_args(
    permissions: Option<&str>,
    justification: Option<&str>,
) -> Result<(), HarnessError> {
    let message = if permissions.is_some() && justification.is_none() {
        Some("invalid escalation: sandbox_permissions requires a justification")
    } else if permissions.is_none() && justification.is_some() {
        Some("invalid escalation: justification is only valid together with sandbox_permissions")
    } else if justification.is_some_and(|s| js_trim(s).is_empty()) {
        Some("invalid justification: expected a non-empty sentence")
    } else {
        None
    };
    if let Some(message) = message {
        Err(SandboxEscalationError::new(
            message,
            SandboxEscalationErrorCode::InvalidArgument,
        ))
    } else {
        Ok(())
    }
}
pub fn sandbox_denial_marker(mode: SandboxMode) -> String {
    format!("[sandbox: file access denied under {mode} mode]")
}
pub fn escalation_hint_marker(subject: &str) -> String {
    format!(
        "[sandbox: escalation available — retry this exact {subject} once with sandbox_permissions (the narrowest wider mode that suffices) + justification; the approval prompt asks the user]"
    )
}
pub fn sandbox_permissions_description(subject: &str) -> String {
    format!(
        "The narrowest wider sandbox mode for a one-shot retry of the exact {subject} the sandbox just denied; the retry asks the user for approval."
    )
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EscalationOutcome {
    AllowedOnce,
    Rejected,
    Cancelled,
    Unavailable,
}
pub struct EscalationAsk<A = serde_json::Value, C = String> {
    pub agent: A,
    pub tool_name: String,
    pub call_id: C,
    pub reason: String,
    pub display_reason: IndexMap<String, String>,
    pub signal: Option<AbortSignal>,
}
pub trait EscalationApprover<A = serde_json::Value, C = String>: Send + Sync {
    fn request(&self, request: EscalationAsk<A, C>) -> SandboxFuture<'_, EscalationOutcome>;
}
pub struct EscalationApproval<A = serde_json::Value, C = String> {
    pub approver: Option<Arc<dyn EscalationApprover<A, C>>>,
    pub agent: Option<A>,
    pub call_id: C,
    pub tool_name: String,
    pub signal: Option<AbortSignal>,
}
#[derive(Clone, Debug)]
pub struct EscalationRequest {
    pub requested_mode: String,
    pub justification: String,
    pub effective_mode: SandboxMode,
    pub subject: String,
}
pub async fn approve_escalation<A: Send, C: Send>(
    request: EscalationRequest,
    approval: EscalationApproval<A, C>,
) -> Result<SandboxMode, BoxError> {
    let mode = &request.requested_mode;
    if mode == request.effective_mode.as_str() {
        return Ok(request.effective_mode);
    }
    let wider = WIDER_MODES
        .get(request.effective_mode.as_str())
        .and_then(|m| m.iter().find(|m| m.as_str() == mode))
        .copied();
    let Some(wider) = wider else {
        let message = format!(
            "sandbox escalation to \"{mode}\" is not strictly wider than this call's current \"{}\" mode",
            request.effective_mode
        );
        return Err(SandboxEscalationError::new(
            message,
            SandboxEscalationErrorCode::InvalidArgument,
        )
        .into());
    };
    let approver=approval.approver.ok_or_else(||{
        let message=format!("sandbox escalation to \"{mode}\" requires approval, but no approval service is composed");
        SandboxEscalationError::new(message,SandboxEscalationErrorCode::Unavailable)
    })?;
    let agent=approval.agent.ok_or_else(||{
        let message=format!("sandbox escalation to \"{mode}\" requires approval, but the call has no agent to route it through");
        SandboxEscalationError::new(message,SandboxEscalationErrorCode::Unavailable)
    })?;
    let outcome = approver
        .request(EscalationAsk {
            agent,
            tool_name: approval.tool_name,
            call_id: approval.call_id,
            reason: format!("escalate sandbox to {mode}: {}", request.justification),
            display_reason: escalation_display_reason(mode, &request.justification),
            signal: approval.signal.clone(),
        })
        .await?;
    let error = match outcome {
        EscalationOutcome::AllowedOnce => return Ok(wider),
        EscalationOutcome::Rejected => SandboxEscalationError::new(
            format!(
                "the user rejected escalating this {} to \"{mode}\"; it stays denied, so stop and explain instead of working around it",
                request.subject
            ),
            SandboxEscalationErrorCode::Rejected,
        ),
        EscalationOutcome::Cancelled => {
            if let Some(signal) = approval.signal {
                signal.throw_if_aborted()?;
            }
            SandboxEscalationError::new(
                format!("approval for escalating to \"{mode}\" was cancelled"),
                SandboxEscalationErrorCode::Cancelled,
            )
        }
        EscalationOutcome::Unavailable => SandboxEscalationError::new(
            format!(
                "sandbox escalation to \"{mode}\" requires approval, but no approval channel is available"
            ),
            SandboxEscalationErrorCode::Unavailable,
        ),
    };
    Err(error.into())
}
