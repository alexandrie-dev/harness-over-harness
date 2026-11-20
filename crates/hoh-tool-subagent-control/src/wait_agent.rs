//! Exact-caller inbox waiting owned by the subagent-control plugin.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, Weak},
    time::Duration,
};

use hoh_agent::{Agent, AgentDisposed, AgentInboxInserted, Agents};
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Context, KernelError, Service};
use hoh_llm::{ContentBlock, MessageId, ToolCallId};
use hoh_timeout::AbortSignal;
use hoh_tools::{DefineToolOptions, DefineToolOutput, define_tool};
use serde_json::{Value, json};
use tokio::sync::Notify;

pub const WAIT_INVALID_TIMEOUT: &str = "WAIT_INVALID_TIMEOUT";
pub const WAIT_UNAVAILABLE: &str = "WAIT_UNAVAILABLE";

/// Stable machine-readable failure identity for `wait_agent`.
pub struct AgentWaitError;

impl AgentWaitError {
    pub const NAME: &'static str = "AgentWaitError";

    pub fn error(message: impl Into<String>, code: &'static str) -> HarnessError {
        HarnessError::named(Self::NAME, message, code)
    }
}

struct Reported {
    agent: Weak<dyn Agent>,
    root_call_id: ToolCallId,
    ids: Vec<MessageId>,
}

#[derive(Default)]
struct ReportedInput(Mutex<HashMap<hoh_session::SessionId, Reported>>);

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn reported_ids(
    agent: &Arc<dyn Agent>,
    reported: &ReportedInput,
    root_call_id: &ToolCallId,
) -> Vec<MessageId> {
    let mut state = lock(&reported.0);
    state.retain(|_, entry| entry.agent.strong_count() > 0);
    state
        .get(agent.id())
        .and_then(|entry| {
            entry
                .agent
                .upgrade()
                .filter(|registered| Arc::ptr_eq(registered, agent))
                .filter(|_| &entry.root_call_id == root_call_id)
                .map(|_| entry.ids.clone())
        })
        .unwrap_or_default()
}

fn has_new_input(
    agent: &Arc<dyn Agent>,
    reported: &ReportedInput,
    root_call_id: &ToolCallId,
    seen: &[MessageId],
) -> bool {
    let pending = agent.inbox().next_step();
    if !pending
        .iter()
        .any(|message| !seen.iter().any(|id| id == message.id()))
    {
        return false;
    }
    let mut state = lock(&reported.0);
    state.retain(|_, entry| entry.agent.strong_count() > 0);
    state.insert(
        agent.id().clone(),
        Reported {
            agent: Arc::downgrade(agent),
            root_call_id: root_call_id.clone(),
            ids: pending.iter().map(|message| message.id().clone()).collect(),
        },
    );
    true
}

fn inactive_effect(error: KernelError) -> BoxError {
    match error {
        KernelError::Inactive => "cannot create effect on inactive context".into(),
        error => Box::new(error),
    }
}

#[allow(clippy::too_many_arguments)]
async fn wait_for_input(
    ctx: Context,
    agents: Service<Agents>,
    agent: Arc<dyn Agent>,
    timeout_ms: u64,
    signal: AbortSignal,
    lifetime: AbortSignal,
    reported: Arc<ReportedInput>,
    root_call_id: ToolCallId,
) -> Result<Value, BoxError> {
    lifetime.throw_if_aborted()?;
    if !agents
        .get(agent.id())
        .is_some_and(|live| Arc::ptr_eq(&live, &agent))
    {
        return Err(Box::new(AgentWaitError::error(
            "wait_agent requires the exact live calling agent",
            WAIT_UNAVAILABLE,
        )));
    }
    // Match the source WeakMap semantics: every concurrent wait captures the reported ids once.
    // A sibling wait may update the shared record later, but cannot retroactively change this wait.
    let seen = reported_ids(&agent, &reported, &root_call_id);

    let input = Arc::new(Notify::new());
    let disposed = Arc::new(Notify::new());
    let expected = agent.clone();
    let input_notice = input.clone();
    let off_inbox = ctx
        .on_contained::<AgentInboxInserted>(move |event| {
            if Arc::ptr_eq(&event.agent, &expected) {
                input_notice.notify_one();
            }
            Ok(())
        })
        .map_err(inactive_effect)?;
    let expected = agent.clone();
    let disposed_notice = disposed.clone();
    let off_disposed = match ctx.on_contained::<AgentDisposed>(move |event| {
        if Arc::ptr_eq(&event.agent, &expected) {
            disposed_notice.notify_one();
        }
        Ok(())
    }) {
        Ok(disposer) => disposer,
        Err(error) => {
            off_inbox.dispose().await;
            return Err(inactive_effect(error));
        }
    };

    let timeout = tokio::time::sleep(Duration::from_millis(timeout_ms));
    tokio::pin!(timeout);
    let result = async {
        loop {
            signal
                .throw_if_aborted()
                .map_err(|reason| Box::new(reason) as BoxError)?;
            lifetime
                .throw_if_aborted()
                .map_err(|reason| Box::new(reason) as BoxError)?;
            if has_new_input(&agent, &reported, &root_call_id, &seen) {
                return Ok(json!({ "timedOut": false }));
            }
            tokio::select! {
                biased;
                reason = signal.cancelled() => return Err(Box::new(reason) as BoxError),
                reason = lifetime.cancelled() => return Err(Box::new(reason) as BoxError),
                () = disposed.notified() => {
                    return Err(Box::new(AgentWaitError::error(
                        "wait_agent ended because the calling agent was disposed",
                        WAIT_UNAVAILABLE,
                    )));
                }
                () = input.notified() => {}
                () = &mut timeout => {
                    return Ok(json!({ "timedOut": true }));
                }
            }
        }
    }
    .await;
    off_inbox.dispose().await;
    off_disposed.dispose().await;
    result
}

pub(crate) fn register(
    ctx: &Context,
    tools: &hoh_tools::ToolsService,
    agents: Service<Agents>,
    lifetime: AbortSignal,
) -> Result<(), BoxError> {
    let execute_ctx = ctx.clone();
    let reported = Arc::new(ReportedInput::default());
    let options = DefineToolOptions::new(
        "wait_agent",
        "Wait for input at your next step, including agent messages, completion notifications, and user steering. Returns immediately if new input is pending. `timedOut: false` means new input is pending. Pending input is delivered to the model after code execution finishes.",
        json!({
            "timeout_ms": {
                "type": "integer",
                "description": "Wait duration in milliseconds, from 10000 through 3600000. Defaults to 30000."
            }
        }),
        DefineToolOutput {
            schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": { "timedOut": { "type": "boolean", "required": true } }
            }),
            render: Arc::new(|_, value| {
                vec![ContentBlock::text(hoh_util_values::js_json_stringify(
                    value,
                ))]
            }),
            presentation_meta: None,
        },
        move |args, exec| {
            let ctx = execute_ctx.clone();
            let agents = agents.clone();
            let lifetime = lifetime.clone();
            let reported = reported.clone();
            Box::pin(async move {
                let timeout = match args.get("timeout_ms") {
                    None => 30_000,
                    Some(value) => {
                        let Some(timeout) = value.as_f64() else {
                            return Err(Box::new(AgentWaitError::error(
                                "timeout_ms must be an integer from 10000 through 3600000",
                                WAIT_INVALID_TIMEOUT,
                            )) as BoxError);
                        };
                        if !timeout.is_finite()
                            || timeout.fract() != 0.0
                            || !(10_000.0..=3_600_000.0).contains(&timeout)
                        {
                            return Err(Box::new(AgentWaitError::error(
                                "timeout_ms must be an integer from 10000 through 3600000",
                                WAIT_INVALID_TIMEOUT,
                            )) as BoxError);
                        }
                        timeout as u64
                    }
                };
                let agent = exec.agent.clone().ok_or_else(|| -> BoxError {
                    Box::new(AgentWaitError::error(
                        "wait_agent requires a calling agent",
                        WAIT_UNAVAILABLE,
                    ))
                })?;
                wait_for_input(
                    ctx,
                    agents,
                    agent,
                    timeout,
                    exec.signal(),
                    lifetime,
                    reported,
                    exec.root_call_id.clone(),
                )
                .await
            })
        },
    );
    tools.register(ctx, define_tool(options)?, Default::default())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ReportedInput, has_new_input, reported_ids, wait_for_input};
    use hoh_agent::Agents;
    use hoh_agent_loop_testkit::{
        AgentLoopTestDependenciesOptions, mount_agent_loop_test_dependencies,
        mount_agent_loop_test_harness,
    };
    use hoh_error::HarnessError;
    use hoh_kernel::Kernel;
    use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, ToolCallId, create_user_message};
    use hoh_session::SessionId;
    use hoh_timeout::AbortController;
    use std::sync::Arc;

    #[tokio::test]
    async fn cancellation_at_body_entry_wins_without_reporting_pending_input() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        mount_agent_loop_test_dependencies(
            &ctx,
            AgentLoopTestDependenciesOptions {
                working_directory: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let harness = mount_agent_loop_test_harness(&ctx).await.unwrap();
        let agent = harness
            .create(SessionId::from("cancelled-wait"))
            .await
            .unwrap();
        agent
            .inject(create_user_message(NewUserMessage {
                content: vec![ContentBlock::text("pending")],
                source: MessageSource::new("user"),
            }))
            .unwrap();
        let caller = AbortController::new();
        caller.abort_with(HarnessError::named(
            "CallerStopped",
            "caller stopped before wait",
            "CALLER_STOPPED",
        ));
        let lifetime = AbortController::new();
        let reported = Arc::new(ReportedInput::default());
        let root_call_id = ToolCallId::from("same-code");

        let error = wait_for_input(
            ctx.clone(),
            ctx.get::<Agents>().unwrap(),
            agent.clone(),
            10_000,
            caller.signal(),
            lifetime.signal(),
            reported.clone(),
            root_call_id.clone(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            HarnessError::of(error.as_ref()).map(HarnessError::code),
            Some("CALLER_STOPPED")
        );
        let seen = reported_ids(&agent, &reported, &root_call_id);
        assert!(has_new_input(&agent, &reported, &root_call_id, &seen));
        kernel.shutdown().await;
    }
}
