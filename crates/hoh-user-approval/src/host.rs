//! Native host approval path. Cryptographic records stay owned by `hoh-protocol`.
use crate::{ApprovalOutcome, ApprovalRequest};
use futures_util::future::BoxFuture;
use hoh_error::BoxError;
use hoh_kernel::Context;
use hoh_protocol::{
    ApprovalOutcome as ProtocolOutcome, ApprovalPending, ApprovalResolved, HostApprovalDecision,
    HostApprovalRequest, Risk,
};
use hoh_timeout::{AbortSignal, deadline};
use serde_json::{Map, Value, json};
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const HOST_APPROVAL_LIFETIME: Duration = Duration::from_secs(300);
const HOST_APPROVAL_TIMEOUT_CODE: &str = "HOST_APPROVAL_EXPIRED";

/// GLUE-SHIM(2b): the SDK JSON-RPC transport implements this without creating a
/// dependency cycle from user-approval back to the server crate.
pub trait HostApprovalPeer: Send + Sync + 'static {
    fn request(
        &self,
        method: &'static str,
        params: Value,
        signal: Option<AbortSignal>,
    ) -> BoxFuture<'static, Result<Value, BoxError>>;

    fn notify(&self, method: &'static str, params: Value) -> Result<(), BoxError>;
}

#[derive(Clone)]
pub(crate) struct HostApprovalMode {
    key: [u8; 32],
    peer: Arc<dyn HostApprovalPeer>,
}

impl HostApprovalMode {
    pub(crate) fn new(key: [u8; 32], peer: Arc<dyn HostApprovalPeer>) -> Self {
        Self { key, peer }
    }

    pub(crate) async fn resolve(
        &self,
        context: &Context,
        approval: Arc<ApprovalRequest>,
        approval_id: &str,
    ) -> ApprovalOutcome {
        let request = match build_request(&approval, approval_id, now_ms()) {
            Ok(request) => request,
            Err(error) => {
                context
                    .logger()
                    .warn(format!("host approval request was invalid: {error}"));
                return ApprovalOutcome::Unavailable;
            }
        };
        self.ask(context, request, approval.signal.as_ref()).await
    }

    /// The pending notification, the signed `approval/request`, the verified answer and the
    /// resolved notification of one request; `signal` is the caller's abort.
    pub(crate) async fn ask(
        &self,
        context: &Context,
        request: HostApprovalRequest,
        signal: Option<&AbortSignal>,
    ) -> ApprovalOutcome {
        let pending = ApprovalPending {
            approval_id: request.approval_id.clone(),
            session_id: request.session_id.clone(),
            risk: request.risk.clone(),
            human_text: request.human_text.clone(),
            expires_at: request.expires_at,
        };
        if let Err(error) = self.notify("approval.pending", &pending) {
            context.logger().warn(format!(
                "host approval pending notification failed: {error}"
            ));
            return ApprovalOutcome::Unavailable;
        }

        let mut deadline = match deadline(
            signal,
            HOST_APPROVAL_LIFETIME.as_millis() as f64,
            HOST_APPROVAL_TIMEOUT_CODE,
        ) {
            Ok(deadline) => deadline,
            Err(error) => {
                context
                    .logger()
                    .warn(format!("host approval deadline failed: {error}"));
                return self.finish(context, &request.approval_id, ApprovalOutcome::Unavailable);
            }
        };
        let params = match serde_json::to_value(&request) {
            Ok(value) => value,
            Err(error) => {
                context.logger().warn(format!(
                    "host approval request serialization failed: {error}"
                ));
                return self.finish(context, &request.approval_id, ApprovalOutcome::Unavailable);
            }
        };
        let answer = self
            .peer
            .request("approval/request", params, Some(deadline.signal.clone()));
        let outcome = tokio::select! {
            biased;
            _ = deadline.signal.cancelled() => ApprovalOutcome::Cancelled,
            answer = answer => self.verify_answer(context, &request, answer),
        };
        deadline.dispose();
        self.finish(context, &request.approval_id, outcome)
    }

    /// Ask for one privileged JSON-RPC method call; only `allowed-once` lets it run.
    pub(crate) async fn approve_method(
        &self,
        context: &Context,
        method: &str,
        params: &Value,
        approval_id: String,
    ) -> Result<(), BoxError> {
        let expires_at = now_ms().saturating_add(HOST_APPROVAL_LIFETIME.as_millis() as u64);
        let request = crate::method::build_method_request(method, params, approval_id, expires_at)
            .map_err(|error| format!("{method} cannot be approved: {error}"))?;
        match self.ask(context, request, None).await {
            ApprovalOutcome::AllowedOnce => Ok(()),
            outcome => Err(format!("{method} was not approved: {}", outcome.as_str()).into()),
        }
    }

    fn verify_answer(
        &self,
        context: &Context,
        request: &HostApprovalRequest,
        answer: Result<Value, BoxError>,
    ) -> ApprovalOutcome {
        let decision = match answer.map_err(|error| error.to_string()).and_then(|value| {
            serde_json::from_value::<HostApprovalDecision>(value).map_err(|error| error.to_string())
        }) {
            Ok(decision) => decision,
            Err(error) => {
                context
                    .logger()
                    .warn(format!("host approval answer was unavailable: {error}"));
                return ApprovalOutcome::Unavailable;
            }
        };
        if let Err(error) =
            hoh_protocol::approval::verify_decision(&self.key, request, &decision, now_ms())
        {
            context
                .logger()
                .warn(format!("host approval answer was rejected: {error}"));
            return ApprovalOutcome::Unavailable;
        }
        from_protocol(decision.outcome)
    }

    fn finish(
        &self,
        context: &Context,
        approval_id: &str,
        outcome: ApprovalOutcome,
    ) -> ApprovalOutcome {
        let resolved = ApprovalResolved {
            approval_id: approval_id.to_owned(),
            outcome: to_protocol(outcome),
        };
        if let Err(error) = self.notify("approval.resolved", &resolved) {
            context.logger().warn(format!(
                "host approval resolved notification failed: {error}"
            ));
        }
        outcome
    }

    fn notify<T: serde::Serialize>(
        &self,
        method: &'static str,
        params: &T,
    ) -> Result<(), BoxError> {
        self.peer.notify(method, serde_json::to_value(params)?)
    }
}

/// The prefix of the reason the sandbox's escalation ask carries (`hoh-sandbox`'s
/// `approve_escalation`): `escalate sandbox to {mode}: {justification}`.
const ESCALATION_PREFIX: &str = "escalate sandbox to ";
const MAX_JUSTIFICATION_CHARS: usize = 3600;

/// What the window shows for an escalation ask: built here in Russian, so that not a word of the
/// model's justification is in `humanText` or `reason`; the justification goes to `details`.
struct EscalationText {
    human_text: String,
    reason: String,
    details: String,
}

fn escalation_text(approval: &ApprovalRequest) -> Option<EscalationText> {
    let rest = approval
        .reason
        .as_deref()?
        .strip_prefix(ESCALATION_PREFIX)?;
    let (mode, justification) = rest.split_once(": ")?;
    let what = if approval.tool_name == "bash" {
        "команду"
    } else {
        "действие"
    };
    // Words for people who never wrote code (HOH-35): no jargon, as the destructive guard
    // words the same escalation.
    let human_text = match mode {
        "danger-full-access" => format!(
            "Помощник просит разрешения работать за пределами своей папки: выполнить {what}"
        ),
        "workspace-write" => {
            format!("Помощник просит выполнить {what} с правом записи в рабочую папку")
        }
        _ => format!("Помощник просит выполнить {what} с дополнительными правами"),
    };
    let justification: String = justification
        .chars()
        .take(MAX_JUSTIFICATION_CHARS)
        .collect();
    Some(EscalationText {
        reason: format!("{human_text}. Обоснование Помощника — в подробностях"),
        human_text,
        details: format!("Обоснование Помощника: {justification}"),
    })
}

fn build_request(
    approval: &ApprovalRequest,
    approval_id: &str,
    now: u64,
) -> Result<HostApprovalRequest, String> {
    let escalation = escalation_text(approval);
    let reason = match &escalation {
        Some(text) => Some(text.reason.clone()),
        None => approval.reason.clone(),
    };
    let details = match (&approval.details, &escalation) {
        (Some(details), _) => Some(details.clone()),
        (None, Some(text)) => Some(text.details.clone()),
        (None, None) => None,
    };
    let mut params = Map::new();
    params.insert(
        "sessionId".into(),
        Value::String(approval.agent.session().id().to_string()),
    );
    params.insert("toolName".into(), Value::String(approval.tool_name.clone()));
    if let Some(call_id) = &approval.call_id {
        params.insert("callId".into(), Value::String(call_id.to_string()));
    }
    if let Some(reason) = &reason {
        params.insert("reason".into(), Value::String(reason.clone()));
    }
    let action = json!({"method":"approval/request","params":params});
    let request = HostApprovalRequest {
        session_id: approval.agent.session().id().to_string(),
        tool_name: approval.tool_name.clone(),
        call_id: approval.call_id.as_ref().map(ToString::to_string),
        reason,
        details,
        approval_id: approval_id.to_owned(),
        risk: risk_of(approval),
        human_text: match &escalation {
            Some(text) => text.human_text.clone(),
            None => human_text(approval),
        },
        action_hash: hoh_protocol::approval::action_hash(&action),
        expires_at: now.saturating_add(HOST_APPROVAL_LIFETIME.as_millis() as u64),
        action,
    };
    hoh_protocol::approval::validate_host_request(&request)?;
    Ok(request)
}

/// The key of the display reason that marks a request nothing judged the risk of: a request of an
/// external executor the destructive classifier found harmless or could not weigh. Not a locale.
pub const UNASSESSED_RISK_KEY: &str = "x-risk";

/// Every request is high risk unless its sender says it was not assessed; a request the
/// classifier found dangerous carries no such mark and stays high.
fn risk_of(approval: &ApprovalRequest) -> Risk {
    match approval
        .display_reason
        .as_ref()
        .and_then(|reason| reason.get(UNASSESSED_RISK_KEY))
    {
        Some("unassessed") => Risk::Unassessed,
        _ => Risk::High,
    }
}

fn human_text(approval: &ApprovalRequest) -> String {
    if let Some(text) = approval
        .display_reason
        .as_ref()
        .and_then(|reason| reason.get("en"))
        .map(str::trim)
        .filter(|text| hoh_protocol::approval::validate_human_text(text).is_ok())
    {
        return text.to_owned();
    }
    let fallback = format!("Allow tool action {}?", approval.tool_name);
    if hoh_protocol::approval::validate_human_text(&fallback).is_ok() {
        fallback
    } else {
        "Allow this tool action?".into()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn from_protocol(outcome: ProtocolOutcome) -> ApprovalOutcome {
    match outcome {
        ProtocolOutcome::AllowedOnce => ApprovalOutcome::AllowedOnce,
        ProtocolOutcome::Rejected => ApprovalOutcome::Rejected,
        ProtocolOutcome::Cancelled => ApprovalOutcome::Cancelled,
        ProtocolOutcome::Unavailable => ApprovalOutcome::Unavailable,
    }
}

fn to_protocol(outcome: ApprovalOutcome) -> ProtocolOutcome {
    match outcome {
        ApprovalOutcome::AllowedOnce => ProtocolOutcome::AllowedOnce,
        ApprovalOutcome::Rejected => ProtocolOutcome::Rejected,
        ApprovalOutcome::Cancelled => ProtocolOutcome::Cancelled,
        ApprovalOutcome::Unavailable => ProtocolOutcome::Unavailable,
    }
}
