//! HoH window profile: a bash command that may delete or overwrite an existing file is put to the
//! user before it runs (`PreToolDecision::Ask`; in host mode the window shows the approval
//! screen). There is no dsh counterpart: dsh's sandbox lets `rm` inside the session folder run
//! silently, and the window's product decision is that it must not.
//!
//! Only the `bash` tool is looked at. The file tools (`write`, `edit`, `apply_patch`) work inside
//! the folder without a question, as before. See `PORT_NOTES.md`.

mod classify;
mod fs;
mod shell;
mod text;

use std::{path::PathBuf, sync::Arc};

use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, NoConfig, Plugin, Service};
use hoh_tools::{
    APPROVAL_CANCELLED, APPROVAL_ERROR_NAME, APPROVAL_REJECTED, APPROVAL_UNAVAILABLE,
    PreToolDecision, ToolErrorInfo, ToolsPreExecute,
};
use hoh_user_approval::{Approval, ApprovalOutcome, ApprovalRequest, DisplayReason};
use hoh_working_directory::WorkingDirectoryKey;

pub use classify::{GitKind, Risk, Source, assess, assess_blocking, assess_with};
pub use fs::{Fs, RealFs};
pub use text::{Question, escalation_question, question};

/// Mount dependencies of the plugin.
#[derive(Inject)]
pub struct Deps {
    working_directory: Service<WorkingDirectoryKey>,
    approval: Service<Approval>,
}

pub struct DestructiveGuard;

impl Plugin for DestructiveGuard {
    const NAME: &'static str = "WindowDestructiveGuard";
    type Config = NoConfig;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, _config: Cfg<NoConfig>) -> Result<(), BoxError> {
        let working_directory = deps.working_directory;
        let approval = deps.approval;
        let fs: Arc<dyn Fs> = Arc::new(RealFs);
        ctx.on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |event, next| {
            let working_directory = working_directory.clone();
            let approval = approval.clone();
            let fs = fs.clone();
            async move {
                let exec = &event.exec;
                if exec.name == "bash"
                    && let Some(command) = exec.arguments["command"].as_str()
                {
                    let session_cwd = exec
                        .agent
                        .as_ref()
                        .map(|agent| PathBuf::from(working_directory.get(agent.session())));
                    let base = match (exec.arguments["workdir"].as_str(), session_cwd) {
                        (Some(workdir), Some(cwd)) => Some(cwd.join(workdir)),
                        (Some(workdir), None) => Some(PathBuf::from(workdir)),
                        (None, cwd) => cwd,
                    };
                    if let Some(justification) = full_access_justification(&exec.arguments)
                        && let Some(agent) = exec.agent.clone()
                    {
                        // One question per call, worded here. The bash tool asks for the
                        // escalation itself right after; that second ask is pre-approved, but only
                        // once the host has said `allowed-once` to this question — so the question
                        // is put here directly (not as an `Ask`, whose answer we would not see).
                        let prompt =
                            escalation_prompt(command, base, justification, fs.clone()).await;
                        let mut request = ApprovalRequest::new(agent.clone(), "bash");
                        request.call_id = Some(exec.call_id.clone());
                        request.reason = Some(prompt.details);
                        request.display_reason = Some(DisplayReason::new(prompt.headline));
                        request.signal = Some(exec.signal());
                        return match approval.request(request).await? {
                            ApprovalOutcome::AllowedOnce => {
                                approval.pre_approve_escalation(
                                    agent.session(),
                                    &exec.call_id,
                                    "bash",
                                    &escalation_reason(justification),
                                );
                                next.run().await
                            }
                            ApprovalOutcome::Rejected => Ok(denial(
                                "the user rejected tool \"bash\"",
                                APPROVAL_REJECTED,
                            )),
                            ApprovalOutcome::Cancelled if exec.signal().aborted() => {
                                Ok(PreToolDecision::Cancel)
                            }
                            ApprovalOutcome::Cancelled => Ok(denial(
                                "approval for tool \"bash\" was cancelled",
                                APPROVAL_CANCELLED,
                            )),
                            ApprovalOutcome::Unavailable => Ok(denial(
                                "tool \"bash\" requires approval, but no approval channel is available",
                                APPROVAL_UNAVAILABLE,
                            )),
                        };
                    }
                    if let Some(ask) = decide(command, base, fs).await {
                        return Ok(ask);
                    }
                }
                next.run().await
            }
        })?;
        Ok(())
    }
}

/// The justification of a call that carries `sandbox_permissions: "danger-full-access"`: the bash
/// tool asks the user for that escalation itself. Only the strictly wider mode counts: the other
/// mode the tool accepts is the one the sandbox already runs in, which it grants without asking.
fn full_access_justification(arguments: &serde_json::Value) -> Option<&str> {
    if arguments["sandbox_permissions"].as_str() != Some("danger-full-access") {
        return None;
    }
    arguments["justification"]
        .as_str()
        .filter(|text| !text.trim().is_empty())
}

/// The decision for one bash command run in `cwd`: `None` leaves it to the next listener. The
/// file system is only touched on a blocking thread.
pub async fn decide(
    command: &str,
    cwd: Option<PathBuf>,
    fs: Arc<dyn Fs>,
) -> Option<PreToolDecision> {
    let risks = assess_blocking(command.to_owned(), cwd.clone(), fs).await;
    if risks.is_empty() {
        return None;
    }
    let question = question(&risks, command, cwd.as_deref());
    Some(PreToolDecision::Ask {
        reason: Some(question.details),
        // The host protocol carries one text, keyed `en`; the window shows it as written.
        display_reason: Some(DisplayReason::new(question.headline)),
    })
}

/// What the bash tool asks the host when the call escalates (`hoh-sandbox`'s
/// `approve_escalation`): the grant is made for exactly this text.
fn escalation_reason(justification: &str) -> String {
    format!("escalate sandbox to danger-full-access: {justification}")
}

fn denial(reason: &str, code: &str) -> PreToolDecision {
    PreToolDecision::Deny {
        reason: reason.to_owned(),
        info: Some(ToolErrorInfo::new(APPROVAL_ERROR_NAME, code)),
    }
}

/// The question for a bash command that asks to run without the sandbox: always asked, in the
/// guard's wording, with what the command would do (whatever the classifier finds). `headline` is
/// the host's `humanText`, `details` its `reason`.
pub async fn escalation_prompt(
    command: &str,
    cwd: Option<PathBuf>,
    justification: &str,
    fs: Arc<dyn Fs>,
) -> Question {
    let risks = assess_blocking(command.to_owned(), cwd, fs).await;
    escalation_question(&risks, command, justification)
}
