//! Test-only drivers that route a scenario to a second model after a tool call
//! (`session/model-switch-compaction/driver.mjs`, `session/model-switch-notice/model-switch-driver.mjs`).
use super::SNAPSHOTS;
use hoh_agent::{AgentCreated, AgentPreStep, ModelSelection};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Plugin};
use hoh_loader::PluginRegistry;
use hoh_tools::{ToolExecutionResult, ToolsExecute};

/// `driver.mjs`: select the smaller model after the first successful `bash` call, before the next
/// step prepares. Both `model-switch-compaction` and `model-switch-overflow-recovery` load it.
pub struct ModelSwitchCompactionDriver;
impl Plugin for ModelSwitchCompactionDriver {
    const NAME: &'static str = "model-switch-compaction-driver";
    type Config = ();
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<()>) -> Result<(), BoxError> {
        ctx.listen()
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(|event, next| async move {
                let result = next.run().await?;
                if event.exec.name == "bash" && !result.is_error() {
                    let Some(agent) = &event.exec.agent else {
                        return Err("model-switch compaction requires a calling Agent".into());
                    };
                    agent.model().select(&ModelSelection {
                        provider: "deepseek-official".into(),
                        model: "deepseek-v4-pro".into(),
                        reasoning_effort: None,
                    })?;
                }
                Ok(result)
            })?;
        Ok(())
    }
}

fn selected() -> ModelSelection {
    ModelSelection {
        provider: "deepseek-official".into(),
        model: "deepseek-v4-pro".into(),
        reasoning_effort: Some("max".into()),
    }
}

/// `model-switch-driver.mjs`: record an explicit initial choice (twice: the second is a no-op) and
/// switch to `deepseek-v4-pro`/`max` after the first successful `todo_write`.
pub struct ModelSwitchDriver;
impl Plugin for ModelSwitchDriver {
    const NAME: &'static str = "model-switch-driver";
    type Config = ();
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<()>) -> Result<(), BoxError> {
        ctx.listen()
            .on_serial::<AgentCreated, (), _>(|event| async move {
                let model = event.agent.model();
                let Some(initial) = model.current() else {
                    return Err("model-switch driver requires an initial route".into());
                };
                model.select(&initial)?;
                model.select(&initial)?;
                Ok(None)
            })?;
        ctx.listen()
            .on_waterfall::<AgentPreStep, hoh_agent::PreStepDecision, _>(
                |event, next| async move {
                    let selected = selected();
                    if event.model_selection.model == selected.model
                        && event.model_selection.reasoning_effort != selected.reasoning_effort
                    {
                        return Err(
                        "model-switch driver requires the explicit effort before the second step"
                            .into(),
                    );
                    }
                    next.run().await
                },
            )?;
        ctx.listen()
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(|event, next| async move {
                let result = next.run().await?;
                if event.exec.name == "todo_write" && !result.is_error() {
                    let Some(agent) = &event.exec.agent else {
                        return Err("model-switch driver requires a calling Agent".into());
                    };
                    agent.model().select(&selected())?;
                }
                Ok(result)
            })?;
        Ok(())
    }
}

pub fn add(registry: &mut PluginRegistry) {
    registry
        .add::<ModelSwitchCompactionDriver>(&format!(
            "{SNAPSHOTS}session/model-switch-compaction/driver.mjs"
        ))
        .add::<ModelSwitchDriver>(&format!(
            "{SNAPSHOTS}session/model-switch-notice/model-switch-driver.mjs"
        ));
}
