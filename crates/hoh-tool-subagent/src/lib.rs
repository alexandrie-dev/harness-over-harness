//! Model-facing subagent delegation tool.

pub mod list_models;
pub mod model_selection;
pub mod model_selection_settings;
pub mod model_selection_state;
pub mod preset_selection;

use crate::{
    list_models::register_list_subagent_models,
    model_selection::{
        DelegationModelRequest, ModelSelectionPolicy, has_configured_llm_selection,
        has_delegation_model_request, preflight_child_llm_route, requested_agent_options,
    },
    model_selection_state::{
        ModelSelectionPolicyState, subagent_model_selection_policy,
        subagent_model_selection_projection_definition,
    },
    preset_selection::{PresetSelectionConfig, SubagentPresetSelection},
};
use hoh_agent::{AgentOptions, AssembleAgent, Live};
use hoh_agent_preset_registry::AgentPresets;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Disposer, Inject, Plugin, Service};
use hoh_llm::{ContentBlock, Llm, ReasoningEffortId};
use hoh_session_projection::{ProjectionHandle, SessionProjections};
use hoh_subagent::{
    ConfiguredMaxDepth, SubagentActivationSpec, SubagentDeliveryTarget, SubagentError,
    SubagentErrorCode, SubagentProvider, SubagentProviderAdded, SubagentProviderRemoved,
    SubagentStartRequest, Subagents, delegation_depth_of, parent_agent_options_for_delegation,
};
use hoh_system_prompt::{
    PromptContext, PromptContextOrderName, PromptSection, PromptSectionOrderName, PromptText,
    SystemPrompt, SystemPromptKey,
};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, ResolvedToolSet, ToolContextUnavailableError,
    ToolDefinition, ToolRegistrationOptions, Tools, define_tool,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::sync::{Arc, Mutex, MutexGuard};

pub const NAME: &str = "tool-subagent";
pub const INJECT: [&str; 4] = ["tools", "subagents", "systemPrompt", "sessionProjections"];

const GUIDANCE: &str = "Start independent delegations with {TOOLS} together in one assistant message and continue useful work while they run. When only waiting for their results remains, end your turn instead of sleeping or polling; the next result starts a new turn. Collect the subagent results required to complete the task before giving the user a final answer.\n\nBash, formatters, code generators, and scripts can overwrite concurrent edits without conflict detection; coordinate their changes explicitly. The agent integrating the work must review the final diff and run relevant tests.";
const DELEGATION_DESCRIPTION_MARKER: &str =
    " This tool starts an independently managed subagent and immediately returns its name.";

fn default_tool_name() -> String {
    "subagent".into()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum MaxDepthConfig {
    Value(u64),
    ProviderManaged(String),
}

impl MaxDepthConfig {
    fn runtime(&self) -> Result<ConfiguredMaxDepth, BoxError> {
        match self {
            Self::Value(value) if *value <= 9_007_199_254_740_991 => {
                Ok(ConfiguredMaxDepth::Value(*value))
            }
            Self::Value(_) => Err("subagent maxDepth must be a non-negative safe integer".into()),
            Self::ProviderManaged(value) if value == "provider-managed" => {
                Ok(ConfiguredMaxDepth::ProviderManaged)
            }
            Self::ProviderManaged(_) => {
                Err("maxDepth must be a non-negative safe integer or 'provider-managed'".into())
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentOptionsConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub max_tokens: Option<u64>,
}

impl AgentOptionsConfig {
    fn validate(&self) -> Result<(), BoxError> {
        if self.reasoning_effort.as_deref() == Some("") {
            return Err("agentOptions.reasoningEffort must be non-empty".into());
        }
        if self.max_tokens == Some(0) {
            return Err("agentOptions.maxTokens must be a positive integer".into());
        }
        if self
            .max_tokens
            .is_some_and(|value| value > 9_007_199_254_740_991)
        {
            return Err("agentOptions.maxTokens must be a safe integer".into());
        }
        Ok(())
    }

    fn runtime(&self) -> AgentOptions {
        AgentOptions {
            provider: self.provider.clone().map(Live::Value),
            model: self.model.clone().map(Live::Value),
            reasoning_effort: self
                .reasoning_effort
                .as_deref()
                .map(ReasoningEffortId::from)
                .map(Live::Value),
            max_tokens: self.max_tokens.map(Live::Value),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemovedField;

fn removed_persona<'de, D: serde::Deserializer<'de>>(_: D) -> Result<RemovedField, D::Error> {
    Err(serde::de::Error::custom(
        "tool-subagent: persona is no longer supported; configure persona and tool plugins in an Agent Preset and select it with agentPreset (spawn only)",
    ))
}

fn removed_tool_filter<'de, D: serde::Deserializer<'de>>(_: D) -> Result<RemovedField, D::Error> {
    Err(serde::de::Error::custom(
        "tool-subagent: toolFilter is no longer supported; configure persona and tool plugins in an Agent Preset and select it with agentPreset (spawn only)",
    ))
}

fn removed_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::Schema::from(false)
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub provider: String,
    #[serde(default = "default_tool_name")]
    pub tool_name: String,
    #[serde(default)]
    pub model_selection: bool,
    #[serde(default)]
    pub model_discovery: bool,
    pub agent_options: Option<AgentOptionsConfig>,
    pub agent_preset: Option<String>,
    pub allowed_agent_presets: Option<Vec<String>>,
    pub max_depth: Option<MaxDepthConfig>,
    #[serde(default, skip_serializing, deserialize_with = "removed_persona")]
    #[schemars(schema_with = "removed_schema")]
    pub persona: RemovedField,
    #[serde(
        default,
        rename = "toolFilter",
        skip_serializing,
        deserialize_with = "removed_tool_filter"
    )]
    #[schemars(schema_with = "removed_schema")]
    pub tool_filter: RemovedField,
}

#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
    subagents: Service<Subagents>,
    system_prompt: Service<SystemPromptKey>,
    projections: Service<SessionProjections>,
}

struct Mounted {
    definition: Arc<ToolDefinition>,
    disposer: Disposer,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn configured_depth(config: &Config) -> Result<Option<ConfiguredMaxDepth>, BoxError> {
    config
        .max_depth
        .as_ref()
        .map(MaxDepthConfig::runtime)
        .transpose()
}

fn validate_provider(
    ctx: &Context,
    subagents: &hoh_subagent::SubagentRuntime,
    config: &Config,
    presets: &SubagentPresetSelection,
    provider: &dyn SubagentProvider,
) -> Result<(), BoxError> {
    presets.assert_provider(provider)?;
    if presets.configured() && ctx.get::<AgentPresets>().is_none() {
        return Err(
            "tool-subagent: agent preset selection requires the agent-presets service".into(),
        );
    }
    if subagents
        .resolve_max_depth(configured_depth(config)?)?
        .is_some()
        && !provider.capabilities().depth_limit
    {
        return Err(format!(
            "tool-subagent: provider \"{}\" cannot enforce maxDepth (no depthLimit capability) — set maxDepth: 'provider-managed' to leave the recursion budget to the provider",
            provider.name()
        )
        .into());
    }
    if config.agent_options.is_some() && !provider.capabilities().agent_options {
        return Err(format!(
            "tool-subagent: provider \"{}\" does not support child agentOptions",
            provider.name()
        )
        .into());
    }
    if config.model_selection && !provider.capabilities().agent_options {
        return Err(format!(
            "tool-subagent: provider \"{}\" does not support child model selection",
            provider.name()
        )
        .into());
    }
    Ok(())
}

pub fn provider_wording(inherits_conversation: bool) -> (&'static str, &'static str) {
    if inherits_conversation {
        (
            "Delegate a task to a subagent that inherits this conversation: a child agent seeded with all completed turns so far (it does not see the current in-flight turn). Use this when the subtask builds on this conversation's context — a follow-up analysis, a review, a continuation — without consuming this conversation's context for the work itself. You receive its result, not its intermediate steps.",
            "The task for the subagent. It already sees this conversation's completed turns, so build on them freely and state only what is new.",
        )
    } else {
        (
            "Delegate a self-contained task to a subagent (a separate agent that works in its own context) to offload focused, independent work — research, a scoped implementation, an analysis — so it does not consume this conversation's context. The subagent returns its result, not its intermediate steps.",
            "The complete, self-contained task for the subagent. It does not share this conversation's context, so include everything it needs.",
        )
    }
}

fn string_arg(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(str::to_owned)
}

fn build_definition(
    ctx: Context,
    config: Arc<Config>,
    presets: Arc<SubagentPresetSelection>,
    subagents: Service<Subagents>,
    policy: ProjectionHandle<ModelSelectionPolicyState>,
    provider: Arc<dyn SubagentProvider>,
) -> Result<ToolDefinition, BoxError> {
    let (description, prompt_description) = provider_wording(provider.inherits_parent_context());
    let provider_defaults = provider
        .agent_route_defaults()
        .map(|(provider, model)| (provider.to_owned(), model.to_owned()));
    let selection_description = if provider_defaults.is_some() {
        " Omit `provider`, `model`, and `reasoning_effort` to use child defaults, falling back to this backend's route defaults."
    } else {
        " Omit `provider`, `model`, and `reasoning_effort` to use child defaults, falling back to compatible parent values."
    };
    let mut full_description = format!(
        "{description}{DELEGATION_DESCRIPTION_MARKER} If you are still online when it finishes, the runtime automatically delivers its completion notice. The notice includes the final answer unless the child sent you a message just before finishing.{}",
        if provider.supports_continuable() {
            " Use `send_message` to steer it while running or continue its conversation after it finishes."
        } else {
            " This backend does not accept follow-up messages."
        }
    );
    if config.model_selection {
        full_description.push_str(selection_description);
        if provider.inherits_parent_context() {
            full_description.push_str(" Changing the route can prevent provider-side reuse of the inherited conversation prefix.");
        }
    }
    let mut parameters = Map::new();
    parameters.insert("cwd".into(), json!({
        "type": "string",
        "description": "Initial child working directory. Relative paths use your current directory; omitted inherits it. Later directory changes in either agent are independent."
    }));
    parameters.insert("description".into(), json!({
        "type": "string", "required": true,
        "description": "A short (3-5 word) description of the delegated task, for display. Use the language of the user's current request."
    }));
    parameters.insert("name".into(), json!({
        "type": "string",
        "description": "Optional short name for the subagent, unique among the subagents you create: lower-case letters, digits, and hyphens, starting with a letter, at most 32 characters; lead is reserved for the root agent. Omitted names default to agent-1, agent-2, and so on."
    }));
    parameters.insert(
        "prompt".into(),
        json!({
            "type": "string", "required": true, "description": prompt_description
        }),
    );
    if let Some(choices) = presets.choices() {
        parameters.insert(
            "agent_preset".into(),
            json!({
                "type": "string", "enum": choices, "description": presets.description()
            }),
        );
    }
    if config.model_selection {
        parameters.insert(
            "provider".into(),
            json!({ "type": "string", "description": "Child LLM provider id; requires `model`." }),
        );
        parameters.insert("model".into(), json!({ "type": "string", "description": "Model id within that provider; requires `provider`." }));
        parameters.insert("reasoning_effort".into(), json!({ "type": "string", "description": "Reasoning effort id; requires `provider` and `model`. Omit to use the selected model's default, even when it matches the parent or configured route." }));
    }
    let tool_name = config.tool_name.clone();
    let provider_for_execute = provider.clone();
    let mut options = DefineToolOptions::new(
        tool_name,
        full_description,
        Value::Object(parameters),
        DefineToolOutput {
            schema: json!({
                "type": "object", "additionalProperties": false,
                "properties": {
                    "kind": { "type": "string", "required": true, "const": "activation" },
                    "name": { "type": "string", "required": true }
                }
            }),
            render: Arc::new(|_, value| {
                vec![ContentBlock::text(format!(
                    "started subagent {}",
                    value
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                ))]
            }),
            presentation_meta: Some(Arc::new(|_, value| {
                json!({
                    "kind": value.get("kind").cloned().unwrap_or(Value::Null),
                    "name": value.get("name").cloned().unwrap_or(Value::Null)
                })
            })),
        },
        move |args, exec| {
            let ctx = ctx.clone();
            let config = config.clone();
            let presets = presets.clone();
            let subagents = subagents.clone();
            let policy = policy.clone();
            let provider = provider_for_execute.clone();
            let provider_defaults = provider_defaults.clone();
            Box::pin(async move {
                let parent = exec.agent.clone().ok_or_else(|| -> BoxError {
                    Box::new(ToolContextUnavailableError::new(
                        "subagent tool requires a calling agent (exec.agent was undefined)",
                    ))
                })?;
                let routes = subagent_model_selection_policy(&policy, parent.session())?;
                let selection_policy = routes.map(|routes| ModelSelectionPolicy { routes });
                let agent_preset = presets.resolve(args.get("agent_preset"))?;
                let model_request = DelegationModelRequest {
                    provider: string_arg(&args, "provider"),
                    model: string_arg(&args, "model"),
                    reasoning_effort: string_arg(&args, "reasoning_effort"),
                };
                let parent_options = parent_agent_options_for_delegation(parent.as_ref());
                let configured = config
                    .agent_options
                    .as_ref()
                    .map(AgentOptionsConfig::runtime);
                let requires_preflight = has_delegation_model_request(&model_request)
                    || has_configured_llm_selection(configured.as_ref());
                let configured = if requires_preflight {
                    match (&provider_defaults, configured) {
                        (Some((provider, model)), Some(mut configured)) => {
                            if configured.provider.is_none() {
                                configured.provider = Some(Live::Value(provider.clone()));
                            }
                            if configured.model.is_none() {
                                configured.model = Some(Live::Value(model.clone()));
                            }
                            Some(configured)
                        }
                        (Some((provider, model)), None) => Some(AgentOptions {
                            provider: Some(Live::Value(provider.clone())),
                            model: Some(Live::Value(model.clone())),
                            ..AgentOptions::default()
                        }),
                        (None, configured) => configured,
                    }
                } else {
                    configured
                };
                let requested = requested_agent_options(
                    configured.as_ref(),
                    &model_request,
                    config.model_selection,
                    selection_policy.as_ref(),
                    &parent_options,
                )?;
                if requires_preflight {
                    let llm = ctx.get::<Llm>().ok_or_else(|| -> BoxError {
                        Box::new(ToolContextUnavailableError::new(
                            "cannot resolve the selected child LLM route because the `llm` service is unavailable",
                        ))
                    })?;
                    preflight_child_llm_route(
                        &llm,
                        &parent_options,
                        requested.as_ref(),
                        &exec.signal(),
                        provider_defaults.is_none()
                            && !has_delegation_model_request(&model_request),
                    )
                    .await?;
                    if subagents
                        .get_provider(&config.provider)
                        .is_none_or(|current| !Arc::ptr_eq(&current, &provider))
                    {
                        return Err(Box::new(SubagentError::error(
                            format!(
                                "subagent provider \"{}\" changed while resolving the child LLM route; retry the delegation",
                                config.provider
                            ),
                            SubagentErrorCode::ProviderChanged,
                        )) as BoxError);
                    }
                }
                exec.signal().throw_if_aborted()?;
                let max_depth = subagents.resolve_max_depth(configured_depth(&config)?)?;
                let prompt = string_arg(&args, "prompt").unwrap_or_default();
                let label = string_arg(&args, "description").unwrap_or_default();
                let started = subagents
                    .start_activation(SubagentActivationSpec {
                        provider: config.provider.clone(),
                        label: label.clone(),
                        name: string_arg(&args, "name"),
                        child_id: None,
                        delivery: SubagentDeliveryTarget::Parent,
                        request: SubagentStartRequest {
                            cwd: string_arg(&args, "cwd"),
                            agent_preset,
                            label: Some(label),
                            prompt: vec![ContentBlock::text(prompt)],
                            parent,
                            signal: exec.signal(),
                            agent_options: requested,
                            output_schema: None,
                            max_depth,
                        },
                    })
                    .await?;
                Ok::<Value, BoxError>(json!({ "kind": "activation", "name": started.name }))
            })
        },
    );
    options.is_concurrency_safe = Some(Arc::new(|_| true));
    define_tool(options)
}

fn is_delegation_definition(definition: &ToolDefinition) -> bool {
    let properties = definition
        .parameters
        .get("properties")
        .and_then(Value::as_object);
    definition
        .description
        .contains(DELEGATION_DESCRIPTION_MARKER)
        && properties.is_some_and(|properties| {
            properties.contains_key("description") && properties.contains_key("prompt")
        })
}

fn mount(
    ctx: &Context,
    deps: &Deps,
    config: Arc<Config>,
    presets: Arc<SubagentPresetSelection>,
    policy: ProjectionHandle<ModelSelectionPolicyState>,
    mounted: &Arc<Mutex<Option<Mounted>>>,
    provider: Arc<dyn SubagentProvider>,
) -> Result<(), BoxError> {
    validate_provider(ctx, &deps.subagents, &config, &presets, provider.as_ref())?;
    let definition = build_definition(
        ctx.clone(),
        config.clone(),
        presets,
        deps.subagents.clone(),
        policy,
        provider.clone(),
    )?;
    let disposer = deps
        .tools
        .register(ctx, definition, ToolRegistrationOptions::default())?;
    // `ctx.tools.register` lands in the calling agent scope when `ctx` is one; read it back there.
    let registered = deps
        .tools
        .get(&config.tool_name, hoh_scope::scope_of(ctx).as_ref(), None)
        .ok_or("tool-subagent registration was not visible")?;
    *lock(mounted) = Some(Mounted {
        definition: registered,
        disposer,
    });
    Ok(())
}

pub struct ToolSubagent;

impl Plugin for ToolSubagent {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let config = Arc::new(config.get().as_ref().clone());
        if config.tool_name.is_empty() {
            return Err("tool-subagent: toolName must be non-empty".into());
        }
        if config.agent_preset.as_deref() == Some("")
            || config
                .allowed_agent_presets
                .as_ref()
                .is_some_and(|values| values.is_empty() || values.iter().any(String::is_empty))
        {
            return Err("tool-subagent: preset ids must be non-empty".into());
        }
        if let Some(options) = &config.agent_options {
            options.validate()?;
        }
        configured_depth(&config)?;
        let presets = Arc::new(SubagentPresetSelection::new(PresetSelectionConfig {
            agent_preset: config.agent_preset.clone(),
            allowed_agent_presets: config.allowed_agent_presets.clone(),
        })?);
        if presets.configured() && ctx.get::<AgentPresets>().is_none() {
            return Err(
                "tool-subagent: agent preset selection requires the agent-presets service".into(),
            );
        }
        let policy = deps
            .projections
            .register(&ctx, subagent_model_selection_projection_definition())
            .await?;
        if config.model_discovery {
            register_list_subagent_models(&ctx, &deps.tools, policy.clone())?;
        }
        let mounted = Arc::new(Mutex::new(None::<Mounted>));

        let validate_ctx = ctx.clone();
        let validate_config = config.clone();
        let validate_presets = presets.clone();
        let validate_subagents = deps.subagents.clone();
        ctx.on::<SubagentProviderAdded>(move |event| {
            if event.0.name() == validate_config.provider
                && let Err(error) = validate_provider(
                    &validate_ctx,
                    &validate_subagents,
                    &validate_config,
                    &validate_presets,
                    event.0.as_ref(),
                )
            {
                panic!("{error}");
            }
        })?;
        if let Some(provider) = deps.subagents.get_provider(&config.provider) {
            validate_provider(&ctx, &deps.subagents, &config, &presets, provider.as_ref())?;
        }

        let added_ctx = ctx.clone();
        let added_deps = Deps {
            tools: deps.tools.clone(),
            subagents: deps.subagents.clone(),
            system_prompt: deps.system_prompt.clone(),
            projections: deps.projections.clone(),
        };
        let added_config = config.clone();
        let added_presets = presets.clone();
        let added_policy = policy.clone();
        let added_mounted = mounted.clone();
        ctx.on::<SubagentProviderAdded>(move |event| {
            if event.0.name() == added_config.provider
                && lock(&added_mounted).is_none()
                && let Err(error) = mount(
                    &added_ctx,
                    &added_deps,
                    added_config.clone(),
                    added_presets.clone(),
                    added_policy.clone(),
                    &added_mounted,
                    event.0.clone(),
                )
            {
                panic!("{error}");
            }
        })?;
        let removed_config = config.clone();
        let removed_mounted = mounted.clone();
        ctx.on_contained::<SubagentProviderRemoved>(move |event| {
            if event.0 != removed_config.provider {
                return Ok(());
            }
            let current = lock(&removed_mounted).take();
            if let Some(current) = current {
                drop(current.disposer.dispose());
            }
            Ok(())
        })?;
        if let Some(provider) = deps.subagents.get_provider(&config.provider) {
            mount(
                &ctx,
                &deps,
                config.clone(),
                presets,
                policy,
                &mounted,
                provider,
            )?;
        } else {
            ctx.logger().info(format_args!(
                "subagent provider \"{}\" not registered yet; the \"{}\" tool will register when it appears",
                config.provider, config.tool_name
            ));
        }

        let section_tools = deps.tools.clone();
        let section_mounted = mounted.clone();
        let section_name = config.tool_name.clone();
        deps.system_prompt.section(
            &ctx,
            PromptSection {
                name: format!("tool:{}", config.tool_name),
                order: SystemPrompt::get_section_order(PromptSectionOrderName::ToolSubagent),
                text: PromptText::provider(move |assemble| {
                    let mounted = lock(&section_mounted);
                    let Some(own) = mounted.as_ref() else {
                        return Ok(String::new());
                    };
                    let tool_set = assemble.extension::<ResolvedToolSet>();
                    let visible_own = section_tools
                        .get(&section_name, assemble.scope.as_ref(), tool_set)
                        .is_some_and(|definition| Arc::ptr_eq(&definition, &own.definition));
                    if !visible_own {
                        return Ok(String::new());
                    }
                    let mut names = section_tools
                        .definitions(assemble.scope.as_ref(), tool_set)
                        .into_iter()
                        .filter(|definition| is_delegation_definition(definition))
                        .map(|definition| definition.name.clone())
                        .collect::<Vec<_>>();
                    names.sort();
                    names.dedup();
                    if names.first().is_none_or(|name| name != &section_name) {
                        return Ok(String::new());
                    }
                    let names = names
                        .iter()
                        .map(|name| format!("`{name}`"))
                        .collect::<Vec<_>>()
                        .join(" or ");
                    Ok(GUIDANCE.replace("{TOOLS}", &names))
                }),
                ..PromptSection::default()
            },
        )?;

        let depth_tools = deps.tools.clone();
        let depth_subagents = deps.subagents.clone();
        let depth_mounted = mounted;
        let depth_config = config.clone();
        deps.system_prompt.context(
            &ctx,
            PromptContext {
                name: format!("tool:{}:depth", config.tool_name),
                order: SystemPrompt::get_context_order(PromptContextOrderName::SubagentDelegation),
                text: PromptText::provider(move |assemble| {
                    let Some(agent) = assemble.extension::<AssembleAgent>() else {
                        return Ok(String::new());
                    };
                    let mounted = lock(&depth_mounted);
                    let Some(own) = mounted.as_ref() else {
                        return Ok(String::new());
                    };
                    let visible = depth_tools
                        .get(
                            &depth_config.tool_name,
                            assemble.scope.as_ref(),
                            assemble.extension::<ResolvedToolSet>(),
                        )
                        .is_some_and(|definition| Arc::ptr_eq(&definition, &own.definition));
                    if !visible {
                        return Ok(String::new());
                    }
                    let Some(max_depth) = depth_subagents
                        .resolve_max_depth(configured_depth(&depth_config)?)?
                    else {
                        return Ok(String::new());
                    };
                    let depth = delegation_depth_of(agent.0.as_ref());
                    if depth < max_depth {
                        return Ok(String::new());
                    }
                    Ok(format!(
                        "Delegation depth for `{}`: current {depth}, maximum {max_depth}, remaining levels 0. Do not call `{}` at this depth; carry out the work directly.",
                        depth_config.tool_name, depth_config.tool_name
                    ))
                }),
                ..PromptContext::default()
            },
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_depth_accepts_safe_integers_and_provider_managed_only() {
        assert_eq!(
            MaxDepthConfig::Value(9_007_199_254_740_991)
                .runtime()
                .unwrap(),
            ConfiguredMaxDepth::Value(9_007_199_254_740_991)
        );
        assert_eq!(
            MaxDepthConfig::ProviderManaged("provider-managed".into())
                .runtime()
                .unwrap(),
            ConfiguredMaxDepth::ProviderManaged
        );
        assert_eq!(
            MaxDepthConfig::Value(9_007_199_254_740_992)
                .runtime()
                .unwrap_err()
                .to_string(),
            "subagent maxDepth must be a non-negative safe integer"
        );
        assert_eq!(
            MaxDepthConfig::ProviderManaged("managed".into())
                .runtime()
                .unwrap_err()
                .to_string(),
            "maxDepth must be a non-negative safe integer or 'provider-managed'"
        );
    }

    #[test]
    fn agent_option_boundaries_match_the_source_schema() {
        assert!(AgentOptionsConfig::default().validate().is_ok());
        assert_eq!(
            AgentOptionsConfig {
                reasoning_effort: Some(String::new()),
                ..Default::default()
            }
            .validate()
            .unwrap_err()
            .to_string(),
            "agentOptions.reasoningEffort must be non-empty"
        );
        assert_eq!(
            AgentOptionsConfig {
                max_tokens: Some(0),
                ..Default::default()
            }
            .validate()
            .unwrap_err()
            .to_string(),
            "agentOptions.maxTokens must be a positive integer"
        );
    }
}
