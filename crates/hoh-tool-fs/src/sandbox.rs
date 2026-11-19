//! The sandbox-escalation API shared by the `write`, `edit` and `apply_patch` tools: the per-call
//! policy resolution, the advertised escalation fields, and the denial-marker mapping — all
//! delegating the vocabulary and the fail-closed approval sequence to `hoh-sandbox` (the same pieces
//! `hoh-tool-bash` uses), so bash and fs escalate identically. Built once per plugin from
//! `ctx.fs.sandbox_mode()` (the capability fact — is a confining backend mounted?) and shared by the
//! mutating tools.

use std::sync::Arc;

use hoh_agent::Agent;
use hoh_error::{BoxError, HarnessError};
use hoh_fs::{FileSystem, FsError, FsErrorCode};
use hoh_kernel::{Context, Service};
use hoh_llm::ToolCallId;
use hoh_sandbox::{
    ESCALATION_TARGETS, EscalationApproval, EscalationApprover, EscalationAsk, EscalationOutcome,
    EscalationRequest, SandboxExecutionPolicy, SandboxFuture, SandboxMode, approve_escalation,
    escalation_hint_marker, sandbox_denial_marker, sandbox_permissions_description,
    validate_escalation_args,
};
use hoh_tools::{ToolArgsError, ToolExecution};
use hoh_user_approval::{Approval, ApprovalOutcome, ApprovalRequest, DisplayReason};
use hoh_util_values::js_trim;
use serde_json::{Map, Value, json};

use crate::shim::{SandboxPolicyKey, live_agent};

/// The escalation approver of this composition: the `approval` service behind the sandbox
/// vocabulary's approver seam.
struct ApprovalApprover {
    approval: Service<Approval>,
}

impl EscalationApprover<Arc<dyn Agent>, ToolCallId> for ApprovalApprover {
    fn request(
        &self,
        request: EscalationAsk<Arc<dyn Agent>, ToolCallId>,
    ) -> SandboxFuture<'_, EscalationOutcome> {
        Box::pin(async move {
            let mut texts = request.display_reason.into_iter();
            let display_reason = texts.next().map(|(locale, text)| {
                let mut reason = DisplayReason::new(text.clone());
                reason.insert(locale, text);
                for (locale, text) in texts {
                    reason.insert(locale, text);
                }
                reason
            });
            let mut asked = ApprovalRequest::new(request.agent, request.tool_name);
            asked.call_id = Some(request.call_id);
            asked.reason = Some(request.reason);
            asked.display_reason = display_reason;
            asked.signal = request.signal;
            let outcome = self.approval.request(asked).await?;
            Ok(match outcome {
                ApprovalOutcome::AllowedOnce => EscalationOutcome::AllowedOnce,
                ApprovalOutcome::Rejected => EscalationOutcome::Rejected,
                ApprovalOutcome::Cancelled => EscalationOutcome::Cancelled,
                ApprovalOutcome::Unavailable => EscalationOutcome::Unavailable,
            })
        })
    }
}

/// The filesystem escalation API: advertisement gating, per-call policy resolution, the one-approved
/// wider retry, and denial-marker mapping. A pure product of `ctx` at plugin apply time.
pub struct FsSandboxController {
    ctx: Context,
    /// The escalation targets this composition advertises (empty when no confining backend is mounted).
    pub escalation_modes: Vec<SandboxMode>,
    /// Shared per-session policy resolver, required by a confining backend.
    policy: Option<Service<SandboxPolicyKey>>,
}

impl FsSandboxController {
    /// Build the controller from the mounted filesystem.
    ///
    /// # Errors
    /// The mounted filesystem confines but `sandboxPolicy` is missing.
    pub fn new(ctx: &Context, fs: &dyn FileSystem) -> Result<Self, BoxError> {
        let default_mode = fs.sandbox_mode();
        let escalation_modes = if default_mode.is_none() {
            Vec::new()
        } else {
            ESCALATION_TARGETS.to_vec()
        };
        let policy = if default_mode.is_none() {
            None
        } else {
            ctx.get::<SandboxPolicyKey>()
        };
        if default_mode.is_some() && policy.is_none() {
            return Err(
                "tool-fs: the mounted filesystem confines but ctx.sandboxPolicy is missing".into(),
            );
        }
        Ok(Self {
            ctx: ctx.clone(),
            escalation_modes,
            policy,
        })
    }

    /// The escalation schema fields for a mutating tool's `parameters`. Call it only under a
    /// confining backend (guard on `escalation_modes`); the enum pins the closed target vocabulary,
    /// the strict-wider check happens per call at execution.
    pub fn schema_fields(&self) -> Map<String, Value> {
        let modes: Vec<Value> = self
            .escalation_modes
            .iter()
            .map(|mode| json!(mode.as_str()))
            .collect();
        let mut fields = Map::new();
        fields.insert(
            "sandbox_permissions".into(),
            json!({
                "type": "string",
                "enum": modes,
                "description": sandbox_permissions_description("operation"),
            }),
        );
        fields.insert(
            "justification".into(),
            json!({
                "type": "string",
                "description": "Required with sandbox_permissions: one sentence for the user explaining \
                    why this exact file operation needs the wider access. Use the language of the user’s current request.",
            }),
        );
        fields
    }

    /// The policy to stamp onto this mutation: an approved escalation grant (a strictly wider retry
    /// resolved through `ctx.approval` before anything executes), else the Session's standing mode.
    /// An omitted or repeated mode permits an omitted or blank justification; repeating the mode
    /// needs no approval. A different mode requires a non-empty justification, and a non-empty
    /// justification without a mode is rejected.
    ///
    /// `tool_name` is the mutating tool's name for the approval audit trail; `args` the call's
    /// validated arguments; `exec` the tool-execution context. Returns the policy to pass to the
    /// mutation, or `None` for an unsandboxed backend.
    pub async fn resolve_policy(
        &self,
        tool_name: &str,
        args: &Value,
        exec: &Arc<ToolExecution>,
    ) -> Result<Option<SandboxExecutionPolicy>, BoxError> {
        let permissions = args.get("sandbox_permissions").and_then(Value::as_str);
        let justification_arg = args.get("justification").and_then(Value::as_str);
        // The schema omits both fields without a sandboxing filesystem; reject them before pairing checks.
        if self.escalation_modes.is_empty()
            && (permissions.is_some() || !js_trim(justification_arg.unwrap_or("")).is_empty())
        {
            return Err(ToolArgsError::new(vec![
                "sandbox_permissions and justification are not available in this composition (no sandboxing filesystem to escalate)"
                    .into(),
            ])
            .into());
        }
        let session = exec.agent.as_ref().map(|agent| agent.session());
        let standing: Option<SandboxExecutionPolicy> = self
            .policy
            .as_ref()
            .map(|policy| policy.resolve(session.as_ref()));
        if let (Some(requested), Some(standing)) = (permissions, &standing)
            && requested == standing.mode.as_str()
        {
            return Ok(Some(standing.clone()));
        }
        let justification = if permissions.is_none()
            && justification_arg.is_some_and(|text| js_trim(text).is_empty())
        {
            None
        } else {
            justification_arg
        };
        validate_escalation_args(permissions, justification)?;
        let (Some(requested), Some(justification)) = (permissions, justification_arg) else {
            return Ok(standing);
        };
        let Some(policy) = standing else {
            return Ok(None);
        };
        let approver: Option<Arc<dyn EscalationApprover<Arc<dyn Agent>, ToolCallId>>> = self
            .ctx
            .get::<Approval>()
            .map(|approval| Arc::new(ApprovalApprover { approval }) as Arc<_>);
        let approved = approve_escalation(
            EscalationRequest {
                requested_mode: requested.to_owned(),
                justification: justification.to_owned(),
                effective_mode: policy.mode,
                subject: "operation".into(),
            },
            EscalationApproval {
                approver,
                agent: exec
                    .agent
                    .as_ref()
                    .and_then(|agent| live_agent(&self.ctx, agent)),
                call_id: exec.call_id.clone(),
                tool_name: tool_name.to_owned(),
                signal: Some(exec.signal()),
            },
        )
        .await?;
        Ok(Some(SandboxExecutionPolicy {
            mode: approved,
            ..policy
        }))
    }

    /// Map a thrown provider error for the model: a `FS_SANDBOX_DENIED` becomes a filesystem error
    /// whose text is the shared `[sandbox: …]` denial marker plus the same-turn escalation hint, so a
    /// policy denial reads identically to bash's while keeping the structured `FS_SANDBOX_DENIED`
    /// code. Any other error passes through unchanged. A `FS_SANDBOX_DENIED` only arises under a
    /// confining backend, which always advertises the escalation fields, so the hint always applies.
    ///
    /// `policy` is the policy stamped onto the call (names the mode in the marker).
    pub fn map_error(&self, error: BoxError, policy: Option<&SandboxExecutionPolicy>) -> BoxError {
        match HarnessError::of(error.as_ref()) {
            Some(found) => match self.map_fs_error(found, policy) {
                Some(mapped) => Box::new(mapped),
                None => error,
            },
            None => error,
        }
    }

    /// [`map_error`](Self::map_error) for an error already known to be a [`HarnessError`]: the
    /// denial marker error, or `None` when the error is not a sandbox denial.
    pub fn map_fs_error(
        &self,
        error: &HarnessError,
        policy: Option<&SandboxExecutionPolicy>,
    ) -> Option<HarnessError> {
        if !error.is_named(FsError::NAME) || error.code() != "FS_SANDBOX_DENIED" {
            return None;
        }
        let policy = policy?;
        Some(
            FsError::new(
                format!(
                    "{}\n{}",
                    sandbox_denial_marker(policy.mode),
                    escalation_hint_marker("operation")
                ),
                FsErrorCode::SandboxDenied,
            )
            .with_cause(error.clone()),
        )
    }
}
