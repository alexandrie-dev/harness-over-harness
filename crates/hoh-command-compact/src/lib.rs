//! Human-facing `/compact` command over the backend-independent compaction seam.

use hoh_commands::{
    CommandDefinition, CommandDefinitionId, CommandInvocation, CommandResult, Commands,
};
use hoh_compaction::{
    Compaction, CompactionAgentOptions, ManualCompactAgentContext, ManualCompactionError,
};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, NoConfig, Plugin, Service, Undo};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::{Notify, oneshot};

pub const NAME: &str = "command-compact";
pub const DEFINITION_ID: &str = "@deepseek-ai/dsh-command-compact";
pub const COMMAND_NAME: &str = "compact";
pub const DESCRIPTION: &str = "Compact older conversation history";
pub const USAGE: &str = "Usage: /compact (no arguments)";

const BUSY: &str = "Compaction is unavailable because this process has an active compaction, or the agent is not idle.";
const CANCELLED: &str = "Compaction cancelled.";
const CHANGED: &str = "The history selected for compaction changed before it could be replaced. The attempt is recorded in the session log.";
const SUMMARY: &str =
    "Compaction could not produce a useful summary. The attempt is recorded in the session log.";
const COMMIT: &str = "Compaction did not finish cleanly; some session history may have changed. Inspect the current session state before retrying.";
const PERSISTENCE: &str = "Compaction finished, but the session could not be saved.";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn expected_failure(error: &ManualCompactionError) -> CommandResult {
    let text = match error.code() {
        "busy" => BUSY,
        "cancelled" => CANCELLED,
        "changed" => CHANGED,
        "summary" => SUMMARY,
        "commit" => COMMIT,
        "persistence" => PERSISTENCE,
        code => unreachable!("unknown manual compaction error code: {code}"),
    };
    CommandResult::error(text)
}

fn manual_agent(invocation: &CommandInvocation) -> ManualCompactAgentContext {
    let agent = invocation.agent.clone();
    let runner = agent.clone();
    ManualCompactAgentContext::new(
        agent.session().clone(),
        CompactionAgentOptions {
            provider: agent.options().provider(),
            model: agent.options().model(),
        },
        Arc::new(move |task| runner.run_maintenance_erased(task)),
    )
}

async fn execute_compact(
    compaction: Service<Compaction>,
    invocation: CommandInvocation,
) -> Result<CommandResult, BoxError> {
    if !hoh_util_values::js_trim(&invocation.raw_input).is_empty() {
        return Ok(CommandResult::error(USAGE));
    }
    let agent = manual_agent(&invocation);
    match compaction
        .compact_now(
            &agent,
            invocation.signal.clone(),
            Some(invocation.command_id.clone()),
        )
        .await
    {
        Ok(Some(result)) => Ok(CommandResult::success_from(
            result.summary_seq,
            Some(format!(
                "Compacted {} history items (~{} tokens).",
                result.shadowed_seqs.len(),
                result.shadowed_token_count
            )),
        )),
        Ok(None) => Ok(CommandResult::success(Some("No compactable history yet."))),
        Err(_) if invocation.signal.aborted() => Ok(CommandResult::error(CANCELLED)),
        Err(error) => match error.downcast_ref::<ManualCompactionError>() {
            Some(expected) => Ok(expected_failure(expected)),
            None => Err(error),
        },
    }
}

#[derive(Default)]
struct ActiveState {
    closed: bool,
    count: usize,
}

#[derive(Default)]
struct ActiveOperations {
    state: Mutex<ActiveState>,
    changed: Notify,
}

impl ActiveOperations {
    fn enter(self: &Arc<Self>) -> Result<ActiveOperation, BoxError> {
        let mut state = lock(&self.state);
        if state.closed {
            return Err("command-compact plugin is disposed".into());
        }
        state.count += 1;
        Ok(ActiveOperation {
            operations: self.clone(),
        })
    }

    async fn close_and_drain(&self) {
        lock(&self.state).closed = true;
        loop {
            let changed = self.changed.notified();
            if lock(&self.state).count == 0 {
                return;
            }
            changed.await;
        }
    }
}

struct ActiveOperation {
    operations: Arc<ActiveOperations>,
}

impl Drop for ActiveOperation {
    fn drop(&mut self) {
        let mut state = lock(&self.operations.state);
        state.count -= 1;
        let drained = state.count == 0;
        drop(state);
        if drained {
            self.operations.changed.notify_waiters();
        }
    }
}

async fn receive_operation(
    receiver: oneshot::Receiver<Result<CommandResult, BoxError>>,
) -> Result<CommandResult, BoxError> {
    receiver
        .await
        .map_err(|_| BoxError::from("command-compact operation stopped without a result"))?
}

fn handler(
    compaction: Service<Compaction>,
    operations: Arc<ActiveOperations>,
    invocation: CommandInvocation,
) -> impl std::future::Future<Output = Result<CommandResult, BoxError>> + Send + 'static {
    let entered = operations.enter();
    let (sender, receiver) = oneshot::channel();
    match entered {
        Ok(active) => {
            tokio::spawn(async move {
                let result = execute_compact(compaction, invocation).await;
                let _ = sender.send(result);
                drop(active);
            });
        }
        Err(error) => {
            let _ = sender.send(Err(error));
        }
    }
    receive_operation(receiver)
}

#[derive(Inject)]
pub struct Deps {
    commands: Service<Commands>,
    compaction: Service<Compaction>,
}

pub struct CommandCompact;

impl Plugin for CommandCompact {
    const NAME: &'static str = NAME;
    type Config = NoConfig;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, _config: Cfg<NoConfig>) -> Result<(), BoxError> {
        let operations = Arc::new(ActiveOperations::default());
        let mut definition = CommandDefinition::new(COMMAND_NAME, DESCRIPTION, {
            let compaction = deps.compaction.clone();
            let operations = operations.clone();
            move |invocation| handler(compaction.clone(), operations.clone(), invocation)
        });
        definition.definition_id = Some(CommandDefinitionId::from(DEFINITION_ID));
        let owner = ctx.clone();
        ctx.try_effect(
            "command-compact lifecycle",
            move |undos| -> Result<(), BoxError> {
                let draining = operations.clone();
                undos.push(Undo::future(move || async move {
                    draining.close_and_drain().await;
                    Ok(())
                }))?;
                undos.push(deps.commands.register(&owner, definition)?)?;
                Ok(())
            },
        )?;
        Ok(())
    }
}
