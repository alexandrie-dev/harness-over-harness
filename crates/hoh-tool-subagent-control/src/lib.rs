//! Shared messaging, interruption, waiting, and child-discovery tools for subagents.

mod list_agents;
mod wait_agent;

use std::sync::Arc;

use hoh_agent::{Agents, AssembleAgent};
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, Undo};
use hoh_llm::ContentBlock;
use hoh_subagent::{
    ROOT_AGENT_NAME, SubagentError, SubagentErrorCode, SubagentInterruptAuthority,
    SubagentSendMessageOptions, Subagents,
};
use hoh_system_prompt::{
    PromptContext, PromptContextOrderName, PromptText, SystemPrompt, SystemPromptKey,
};
use hoh_timeout::AbortController;
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, ResolvedToolSet, ToolContextUnavailableError, Tools,
    define_tool,
};
use hoh_util_values::js_json_stringify;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use wait_agent::{AgentWaitError, WAIT_INVALID_TIMEOUT, WAIT_UNAVAILABLE};

/// Cordis plugin name.
pub const NAME: &str = "tool-subagent-control";
/// Required service names in source order.
pub const INJECT: [&str; 4] = ["tools", "subagents", "agents", "systemPrompt"];

const SEND_DESCRIPTION: &str = "Send a message to an agent. A working agent receives it at its next step; an idle agent starts a new turn with it. Returns delivery confirmation.";
const INTERRUPT_DESCRIPTION: &str = "Ask a subagent to stop its current work. This call returns without waiting for it to stop. You can continue a local subagent's conversation later with send_message. External executions stop permanently and cannot receive follow-ups. Subagents it started will keep running.";

const fn default_interrupt() -> bool {
    true
}

/// Optional subagent control tools.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Config {
    /// Register `interrupt_agent`.
    #[schemars(default = "default_interrupt")]
    pub interrupt: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            interrupt: default_interrupt(),
        }
    }
}

#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
    subagents: Service<Subagents>,
    agents: Service<Agents>,
    system_prompt: Service<SystemPromptKey>,
}

/// Shared subagent-control tool plugin.
pub struct ToolSubagentControl;

fn string_arg<'a>(args: &'a Value, name: &str) -> &'a str {
    args.get(name).and_then(Value::as_str).unwrap_or_default()
}

fn quoted(value: &str) -> String {
    js_json_stringify(&Value::String(value.into()))
}

fn register_send_message(
    ctx: &Context,
    deps: &Deps,
) -> Result<Arc<hoh_tools::ToolDefinition>, BoxError> {
    let subagents = deps.subagents.clone();
    let options = DefineToolOptions::new(
        "send_message",
        SEND_DESCRIPTION,
        json!({
            "agent": {
                "type": "string", "required": true,
                "description": "The name of another agent in your subagent tree, as shown by list_agents."
            },
            "message": {
                "type": "string", "required": true,
                "description": "The message to deliver to the agent."
            }
        }),
        DefineToolOutput {
            schema: json!({
                "type": "object", "additionalProperties": false,
                "properties": { "messageId": { "type": "string", "required": true } }
            }),
            render: Arc::new(|args, _| {
                vec![ContentBlock::text(format!(
                    "message delivered to agent {}",
                    string_arg(args, "agent")
                ))]
            }),
            presentation_meta: None,
        },
        move |args, exec| {
            let subagents = subagents.clone();
            Box::pin(async move {
                let sender = exec.agent.clone().ok_or_else(|| -> BoxError {
                    Box::new(ToolContextUnavailableError::new(
                        "send_message requires a calling agent (exec.agent was undefined)",
                    ))
                })?;
                let target = subagents
                    .resolve_name(&sender, string_arg(&args, "agent"), exec.signal())
                    .await?;
                let message_id = subagents
                    .send_message(
                        &sender,
                        &target,
                        vec![ContentBlock::text(string_arg(&args, "message"))],
                        SubagentSendMessageOptions {
                            signal: exec.signal(),
                        },
                    )
                    .await?;
                Ok(json!({ "messageId": message_id }))
            })
        },
    );
    deps.tools
        .register(ctx, define_tool(options)?, Default::default())?;
    let scope = hoh_scope::scope_of(ctx);
    deps.tools
        .get("send_message", scope.as_ref(), None)
        .ok_or_else(|| "send_message registration was not visible".into())
}

fn register_interrupt(ctx: &Context, deps: &Deps) -> Result<(), BoxError> {
    let subagents = deps.subagents.clone();
    let options = DefineToolOptions::new(
        "interrupt_agent",
        INTERRUPT_DESCRIPTION,
        json!({
            "agent": {
                "type": "string", "required": true,
                "description": "The name of an agent created under you: your direct child or a deeper descendant."
            }
        }),
        DefineToolOutput {
            schema: json!({
                "type": "object", "additionalProperties": false,
                "properties": { "accepted": { "type": "boolean", "required": true } }
            }),
            render: Arc::new(|args, _| {
                vec![ContentBlock::text(format!(
                    "interrupt requested for agent {}",
                    string_arg(args, "agent")
                ))]
            }),
            presentation_meta: None,
        },
        move |args, exec| {
            let subagents = subagents.clone();
            Box::pin(async move {
                let caller = exec.agent.clone().ok_or_else(|| -> BoxError {
                    Box::new(ToolContextUnavailableError::new(
                        "interrupt_agent requires a calling agent (exec.agent was undefined)",
                    ))
                })?;
                let target = subagents
                    .resolve_name(&caller, string_arg(&args, "agent"), exec.signal())
                    .await?;
                subagents.interrupt(&target, SubagentInterruptAuthority::Ancestor(caller))?;
                Ok(json!({ "accepted": true }))
            })
        },
    );
    deps.tools
        .register(ctx, define_tool(options)?, Default::default())?;
    Ok(())
}

fn identity_text(
    subagents: &hoh_subagent::SubagentRuntime,
    tools: &hoh_tools::ToolsService,
    send_message: &Arc<hoh_tools::ToolDefinition>,
    list_agents: &Arc<hoh_tools::ToolDefinition>,
    assemble: &hoh_system_prompt::AssembleContext,
) -> Result<String, BoxError> {
    let Some(agent) = assemble.extension::<AssembleAgent>().map(|value| &value.0) else {
        return Ok(String::new());
    };
    let names = match subagents.names_of(agent) {
        Ok(names) => names,
        Err(error) => {
            let unavailable = HarnessError::of(error.as_ref()).is_some_and(|error| {
                error.is_named(SubagentError::NAME)
                    && error.code() == SubagentErrorCode::Unauthorized.as_str()
            });
            if !unavailable {
                return Err(error);
            }
            return Ok(format!(
                "Your agent id is {}. Your subagent tree root is unavailable. Continue independent work; subagent discovery and messaging require an active root.",
                quoted(agent.id().as_str())
            ));
        }
    };
    let mut identity = match names.parent {
        None => format!(
            "You are the root agent. Your name is {}.",
            quoted(&names.self_name)
        ),
        Some(parent) => format!(
            "You are a subagent delegated by agent {}.\nYour name is {}.\nThe root agent of your collaboration tree is {}.",
            quoted(&parent),
            quoted(&names.self_name),
            quoted(ROOT_AGENT_NAME)
        ),
    };
    let scope = assemble.scope.as_ref();
    let tool_set = assemble.extension::<ResolvedToolSet>();
    let mut guidance = Vec::new();
    if tools
        .get(&send_message.name, scope, tool_set)
        .is_some_and(|definition| Arc::ptr_eq(&definition, send_message))
    {
        guidance.push(
            "To message other agents, use send_message({ agent: \"<agent-name>\", message: \"...\" }).",
        );
    }
    if tools
        .get(&list_agents.name, scope, tool_set)
        .is_some_and(|definition| Arc::ptr_eq(&definition, list_agents))
    {
        guidance.push("Use list_agents({}) to find your collaborator agents and their names.");
    }
    if !guidance.is_empty() {
        identity.push_str("\n\n");
        identity.push_str(&guidance.join("\n"));
    }
    Ok(identity)
}

impl Plugin for ToolSubagentControl {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let lifetime = Arc::new(AbortController::new());
        let lifetime_signal = lifetime.signal();
        ctx.effect("subagent-control.waits()", move || {
            Undo::sync(move || {
                lifetime.abort_with(AgentWaitError::error(
                    "wait_agent ended because its plugin was unloaded",
                    WAIT_UNAVAILABLE,
                ));
            })
        })?;

        let list_agents = list_agents::register(
            &ctx,
            &deps.tools,
            deps.subagents.clone(),
            deps.agents.clone(),
        )?;
        wait_agent::register(&ctx, &deps.tools, deps.agents.clone(), lifetime_signal)?;
        let send_message = register_send_message(&ctx, &deps)?;

        let subagents = deps.subagents.clone();
        let tools = deps.tools.clone();
        deps.system_prompt.context(
            &ctx,
            PromptContext {
                name: "subagent:identity".into(),
                order: SystemPrompt::get_context_order(PromptContextOrderName::SubagentDelegation),
                text: PromptText::provider(move |assemble| {
                    identity_text(&subagents, &tools, &send_message, &list_agents, assemble)
                }),
                interpolate: Some(false),
                required: true,
            },
        )?;

        if config.get().interrupt {
            register_interrupt(&ctx, &deps)?;
        }
        Ok(())
    }
}
