//! Rust counterparts of dsh's `local:` test plugins (`packages/test-support/session-snapshot/tests/fixtures`
//! and the other TypeScript fixtures that snapshot scenarios load). Each behaves as the TypeScript
//! file does; the registered name is the `local:` name the materialized patch carries.
use hoh_agent::AgentCreated;
use hoh_agent_preset_registry::AgentPresets;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_loader::PluginRegistry;
use hoh_sandbox::SandboxMode;
use hoh_system_prompt::{PromptSection, PromptText, SystemPromptKey};
use hoh_tools::{PostToolDecision, ToolsPostExecute};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::{Arc, Mutex};

mod agents;
pub use agents::release_grant;

/// `session.header.parentSession !== undefined`: a present key, `null` included, marks a child.
pub(crate) fn is_child(header: &serde_json::Value) -> bool {
    header.get("parentSession").is_some()
}

const ROOT: &str = "local:packages/test-support/session-snapshot/tests/fixtures/";

/// `compaction-working-directory.ts`: before any model request, give every Session the effective
/// directory `/tmp` while the original project stays recorded in its header.
pub struct CompactionWorkingDirectory;
impl Plugin for CompactionWorkingDirectory {
    const NAME: &'static str = "compaction-working-directory";
    type Config = ();
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<()>) -> Result<(), BoxError> {
        ctx.listen()
            .on_serial::<AgentCreated, (), _>(|event| async move {
                event.agent.session().append(
                    "working-directory/change",
                    json!({"cwd": "/tmp"}),
                    None,
                )?;
                Ok(None)
            })?;
        Ok(())
    }
}

#[derive(Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ParentAgentPresetConfig {
    pub presets: Option<Vec<String>>,
}
#[derive(Inject)]
pub struct ParentAgentPresetDeps {
    presets: Service<AgentPresets>,
}
/// `parent-agent-preset.ts`: select the root Session's presets (default `parent`) before its first
/// model request; child Sessions (those with a `parentSession`) are skipped.
pub struct ParentAgentPreset;
impl Plugin for ParentAgentPreset {
    const NAME: &'static str = "parent-agent-preset";
    type Config = ParentAgentPresetConfig;
    type Deps = ParentAgentPresetDeps;
    async fn apply(
        ctx: Context,
        deps: ParentAgentPresetDeps,
        config: Cfg<ParentAgentPresetConfig>,
    ) -> Result<(), BoxError> {
        let presets = deps.presets;
        ctx.listen()
            .prepend()
            .on_serial::<AgentCreated, (), _>(move |event| {
                let presets = presets.clone();
                let selected = config
                    .get()
                    .presets
                    .clone()
                    .unwrap_or_else(|| vec!["parent".to_owned()]);
                async move {
                    if is_child(&event.agent.session().header()) {
                        return Ok(None);
                    }
                    for preset in selected {
                        presets.select(event.agent.clone(), preset).await?;
                    }
                    Ok(None)
                }
            })?;
        Ok(())
    }
}

/// `parent-sandbox-override.ts`: switch each ROOT Session to `read-only` at creation (one
/// `sandbox/mode` event), the equivalent of the UI "Access" switch.
pub struct ParentSandboxOverride;
impl Plugin for ParentSandboxOverride {
    const NAME: &'static str = "parent-sandbox-override";
    type Config = ();
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<()>) -> Result<(), BoxError> {
        ctx.listen()
            .on_serial::<AgentCreated, (), _>(|event| async move {
                if is_child(&event.agent.session().header()) {
                    return Ok(None);
                }
                hoh_sandbox_policy::set_sandbox_mode(event.agent.session(), SandboxMode::ReadOnly)?;
                Ok(None)
            })?;
        Ok(())
    }
}

#[derive(Inject)]
pub struct SystemPromptDeps {
    prompt: Service<SystemPromptKey>,
}
/// `in-history-prompt-update.ts`: an empty prompt section (order 400) that becomes
/// `Snapshot guidance added after the first read: reply with the single word DONE.` after the first
/// successful `read`, so the next step renders a changed system prompt.
pub struct InHistoryPromptUpdate;
impl Plugin for InHistoryPromptUpdate {
    const NAME: &'static str = "in-history-prompt-update";
    type Config = ();
    type Deps = SystemPromptDeps;
    async fn apply(ctx: Context, deps: SystemPromptDeps, _: Cfg<()>) -> Result<(), BoxError> {
        let guidance = Arc::new(Mutex::new(String::new()));
        let shown = guidance.clone();
        deps.prompt.section(
            &ctx,
            PromptSection {
                name: "snapshot:in-history-update".into(),
                order: 400.0,
                text: PromptText::provider(move |_| {
                    Ok(shown
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone())
                }),
                ..PromptSection::default()
            },
        )?;
        ctx.listen()
            .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |event, next| {
                let guidance = guidance.clone();
                async move {
                    let downstream = next.run().await?;
                    if !event.result.is_error() && event.exec.name == "read" {
                        *guidance.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
                            "Snapshot guidance added after the first read: reply with the single word DONE.".into();
                    }
                    Ok(downstream)
                }
            })?;
        Ok(())
    }
}

/// Register every ported fixture under the `local:` name of its TypeScript file.
pub fn add(registry: &mut PluginRegistry) {
    registry
        .add::<CompactionWorkingDirectory>(&format!("{ROOT}compaction-working-directory.ts"))
        .add::<ParentAgentPreset>(&format!("{ROOT}parent-agent-preset.ts"))
        .add::<ParentSandboxOverride>(&format!("{ROOT}parent-sandbox-override.ts"))
        .add::<InHistoryPromptUpdate>(&format!("{ROOT}in-history-prompt-update.ts"));
    agents::add(registry);
}
