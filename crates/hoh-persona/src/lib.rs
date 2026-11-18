//! A per-agent persona as a composable row.
//!
//! `hoh-system-prompt` owns the global persona as its own config, and registers
//! that section unconditionally — so this row is **scope-only**. Mounted inside
//! an agent preset it shadows the deployment persona for that one session,
//! exactly like the per-child persona `dsh-subagent` installs; mounted globally
//! it collides with the registry's own registration and fails loud.
//!
//! That constraint is the reason the row exists. An agent preset cannot mount
//! the prompt registry itself, so without a row of its own a preset could
//! change an agent's tools but never its identity.

use hoh_kernel::{Cfg, Context, Error, Inject, Service, plugin};
pub use hoh_system_prompt::{PERSONA_PREFIX_SECTION, PERSONA_SUFFIX_SECTION};
use hoh_system_prompt::{PromptSection, PromptSectionOrderName, SystemPrompt, SystemPromptKey};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Cordis plugin name (`name` of the source).
pub const NAME: &str = "persona";

fn default_true() -> bool {
    true
}

/// Plugin config: the persona text this composition contributes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Persona prose rendered as the `deployment:persona-prefix` section. A template:
    /// complete `{{…}}` groups interpolate strictly against registered prompt
    /// variables. Empty text drops the section at render, matching the registry.
    pub prefix: String,
    /// Persona suffix template rendered after first-party guidance. Omitted or empty
    /// text shadows the deployment suffix away; interpolation is strict.
    #[serde(default)]
    pub suffix: String,
    /// Make the prefix the complete system prompt, suppressing the suffix and every other section.
    #[serde(default)]
    pub complete: bool,
    /// Suppress dynamic runtime-context snapshots for this persona's agent scope.
    #[serde(default = "default_true")]
    pub include_runtime_context: bool,
}

/// The prompt registry this row contributes to (`inject = ['systemPrompt']`).
#[derive(Inject)]
pub struct Deps {
    system_prompt: Service<SystemPromptKey>,
}

/// Register the persona prefix and suffix sections for the mounting context's scope.
///
/// `ctx` must be an agent scope context; an unscoped context collides with the
/// prompt registry's own persona registration and the plugin fails to activate.
#[plugin(Persona, name = "persona")]
pub async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), Error> {
    let config = config.get();
    let prompt = &deps.system_prompt;
    ctx.try_effect("persona.section()", |undos| -> Result<(), Error> {
        undos.push(prompt.section(
            &ctx,
            PromptSection {
                name: PERSONA_PREFIX_SECTION.into(),
                order: SystemPrompt::get_section_order(
                    PromptSectionOrderName::DeploymentPersonaPrefix,
                ),
                text: config.prefix.clone().into(),
                complete: config.complete,
                ..PromptSection::default()
            },
        )?)?;
        Ok(())
    })?;
    ctx.try_effect("persona.suffix()", |undos| -> Result<(), Error> {
        undos.push(prompt.section(
            &ctx,
            PromptSection {
                name: PERSONA_SUFFIX_SECTION.into(),
                order: SystemPrompt::get_section_order(
                    PromptSectionOrderName::DeploymentPersonaSuffix,
                ),
                text: config.suffix.clone().into(),
                ..PromptSection::default()
            },
        )?)?;
        Ok(())
    })?;
    if !config.include_runtime_context {
        prompt.suppress_runtime_context(&ctx)?;
    }
    Ok(())
}
