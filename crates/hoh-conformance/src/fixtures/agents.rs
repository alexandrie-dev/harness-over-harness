//! Fixtures of `packages/test-support/session-snapshot/tests/fixtures` that order or observe the
//! agent loop, subagents and the sandbox policy.
use super::{ROOT, is_child};
use hoh_agent::{AgentCreated, AgentPreStep, PreStepDecision};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, Undo};
use hoh_loader::{LoaderKey, PluginRegistry};
use hoh_sandbox_policy::SandboxPolicy;
use hoh_session::SessionEventEmitted;
use hoh_subagent::Subagents;
use hoh_tools::{ToolExecutionResult, Tools, ToolsExecute};
use hoh_user_questions::{AskUserQuestionAnswer, AskUserQuestionRequestEvent, UserQuestions};
use serde_json::json;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::watch;

#[derive(Inject)]
pub struct SettlementBarrierDeps {
    subagents: Service<Subagents>,
    #[allow(dead_code)]
    tools: Service<Tools>,
}
/// `subagent-settlement-barrier.ts`: hold every `subagent*` tool response until the tree work of
/// its agent (activation settlement) has queued its notice.
pub struct SubagentSettlementBarrier;
impl Plugin for SubagentSettlementBarrier {
    const NAME: &'static str = "subagent-settlement-barrier";
    type Config = ();
    type Deps = SettlementBarrierDeps;
    async fn apply(ctx: Context, deps: SettlementBarrierDeps, _: Cfg<()>) -> Result<(), BoxError> {
        let subagents = deps.subagents;
        ctx.listen()
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| {
                let subagents = subagents.clone();
                async move {
                    let result = next.run().await?;
                    if event.exec.name.starts_with("subagent")
                        && let Some(agent) = &event.exec.agent
                    {
                        subagents.wait_for_tree_work(agent).await?;
                    }
                    Ok(result)
                }
            })?;
        Ok(())
    }
}

#[derive(Inject)]
pub struct ActivationLimitDeps {
    #[allow(dead_code)]
    agents: Service<hoh_agent::Agents>,
    loader: Service<LoaderKey>,
    #[allow(dead_code)]
    subagents: Service<Subagents>,
}
/// `subagent-activation-limit.ts`: hold child execution until the parent's capacity probe has been
/// recorded; the parent's first step lowers `maxActiveSubagents` to 1.
pub struct SubagentActivationLimit;
impl Plugin for SubagentActivationLimit {
    const NAME: &'static str = "subagent-activation-limit";
    type Config = ();
    type Deps = ActivationLimitDeps;
    async fn apply(ctx: Context, deps: ActivationLimitDeps, _: Cfg<()>) -> Result<(), BoxError> {
        let (closed, watcher) = watch::channel(false);
        let closed = Arc::new(closed);
        let on_dispose = closed.clone();
        ctx.effect("subagent-activation-limit.closed", move || {
            Undo::sync(move || {
                on_dispose.send_replace(true);
            })
        })?;
        let on_event = closed.clone();
        ctx.on_contained::<SessionEventEmitted>(move |event| {
            if !is_child(&event.session.header()) && event.event.event_type() == "turn/end" {
                on_event.send_replace(true);
            }
            Ok(())
        })?;
        let loader = deps.loader;
        ctx.listen()
            .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
                let mut watcher = watcher.clone();
                let loader = loader.clone();
                async move {
                    if is_child(&event.agent.session().header()) {
                        watcher.wait_for(|closed| *closed).await?;
                    } else {
                        let entry = loader
                            .entries()
                            .into_iter()
                            .find(|entry| entry.options.id == "subagent")
                            .ok_or("Missing subagent fixture entry")?;
                        let fiber = entry.fiber.ok_or("Missing subagent fixture entry")?;
                        fiber.update(json!({"maxActiveSubagents": 1})).await?;
                    }
                    next.run().await
                }
            })?;
        Ok(())
    }
}

#[derive(Inject)]
pub struct SandboxPolicyDeps {
    policy: Service<SandboxPolicy>,
}
/// `dispose()` of `session-write-grant.ts`: the grant must be gone before the next request. The
/// synchronous undo of the registration runs inside the `dispose()` call itself; the returned
/// future only awaits the (already complete) teardown, so it is not spawned and not awaited.
pub fn release_grant(disposer: hoh_kernel::Disposer) {
    drop(disposer.dispose());
}

/// `session-write-grant.ts`: grant `<cwd>/extra` as a writable root of each new Session until its
/// first tool result.
pub struct SessionWriteGrant;
impl Plugin for SessionWriteGrant {
    const NAME: &'static str = "session-write-grant";
    type Config = ();
    type Deps = SandboxPolicyDeps;
    async fn apply(ctx: Context, deps: SandboxPolicyDeps, _: Cfg<()>) -> Result<(), BoxError> {
        let policy = deps.policy;
        let owner = ctx.clone();
        ctx.listen()
            .on_serial::<AgentCreated, (), _>(move |event| {
                let policy = policy.clone();
                let owner = owner.clone();
                async move {
                    let extra = std::env::current_dir()?.join("extra");
                    let session = event.agent.session();
                    let disposer = policy.register_writable_root(
                        &owner,
                        session,
                        extra.to_string_lossy().into_owned(),
                    )?;
                    let disposer = Mutex::new(Some(disposer));
                    let id = session.id().clone();
                    owner.on_contained::<SessionEventEmitted>(move |event| {
                        if event.session.id() == &id
                            && event.event.event_type() == "tool/result"
                            && let Some(disposer) = disposer
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .take()
                        {
                            release_grant(disposer);
                        }
                        Ok(())
                    })?;
                    Ok(None)
                }
            })?;
        Ok(())
    }
}

#[derive(Inject)]
pub struct QuestionsDeps {
    #[allow(dead_code)]
    questions: Service<UserQuestions>,
}
/// `child-question-tripwire.ts`: an answerer whose invocation means the child guard failed.
pub struct ChildQuestionTripwire;
impl Plugin for ChildQuestionTripwire {
    const NAME: &'static str = "child-question-tripwire";
    type Config = ();
    type Deps = QuestionsDeps;
    async fn apply(ctx: Context, _: QuestionsDeps, _: Cfg<()>) -> Result<(), BoxError> {
        ctx.listen()
            .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(
                |_, _| async {
                    Err("snapshot tripwire: delegated question reached the UI answerer".into())
                },
            )?;
        Ok(())
    }
}

pub fn add(registry: &mut PluginRegistry) {
    registry
        .add::<SubagentSettlementBarrier>(&format!("{ROOT}subagent-settlement-barrier.ts"))
        .add::<SubagentActivationLimit>(&format!("{ROOT}subagent-activation-limit.ts"))
        .add::<SessionWriteGrant>(&format!("{ROOT}session-write-grant.ts"))
        .add::<ChildQuestionTripwire>(&format!("{ROOT}child-question-tripwire.ts"));
}
