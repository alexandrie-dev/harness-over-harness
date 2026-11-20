//! Discover the caller's entire subagent tree.

use std::sync::Arc;

use hoh_agent::{AgentStatus, Agents};
use hoh_error::BoxError;
use hoh_kernel::{Context, Service};
use hoh_llm::ContentBlock;
use hoh_subagent::{ROOT_AGENT_NAME, SubagentDiagnosticReason, SubagentListEntry};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, ToolContextUnavailableError, ToolDefinition, define_tool,
};
use serde_json::{Map, Value, json};

const DESCRIPTION: &str = "List your entire subagent tree: the root agent and all reachable continuable descendants, including yourself. Rows carry names, descriptions, and status. A descendant's name is its path from the root, such as reviewer/checker. You can message any other agent in the subagent tree. running means it is working; inactive means no turn is executing; it does not indicate task completion, success, or failure. corrupt, unsupported, or unavailable means this row's own descendants could not be read; the row itself is still a member you can message. While you remain active, you will be notified when a direct child finishes; there is no need to keep checking its status. Use send_message to continue the conversation.";

fn status_of(agents: &Service<Agents>, id: &hoh_session::SessionId) -> &'static str {
    if agents
        .get(id)
        .is_some_and(|agent| agent.status() == AgentStatus::Running)
    {
        "running"
    } else {
        "inactive"
    }
}

fn reason(reason: SubagentDiagnosticReason) -> &'static str {
    match reason {
        SubagentDiagnosticReason::Corrupt => "corrupt",
        SubagentDiagnosticReason::Unsupported => "unsupported",
        SubagentDiagnosticReason::Unavailable => "unavailable",
    }
}

fn entry(name: String, status: &str, description: Option<String>) -> Value {
    let mut value = Map::from_iter([
        ("name".into(), Value::String(name)),
        ("status".into(), Value::String(status.into())),
    ]);
    if let Some(description) = description {
        value.insert("description".into(), Value::String(description));
    }
    Value::Object(value)
}

fn render_entries(value: &Value) -> Vec<ContentBlock> {
    let text = value
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| {
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let status = entry
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match entry.get("description").and_then(Value::as_str) {
                Some(description) => format!("{name} [{status}] — {description}"),
                None => format!("{name} [{status}]"),
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    vec![ContentBlock::text(text)]
}

pub(crate) fn register(
    ctx: &Context,
    tools: &hoh_tools::ToolsService,
    subagents: Service<hoh_subagent::Subagents>,
    agents: Service<Agents>,
) -> Result<Arc<ToolDefinition>, BoxError> {
    let execute_subagents = subagents.clone();
    let execute_agents = agents.clone();
    let mut options = DefineToolOptions::new(
        "list_agents",
        DESCRIPTION,
        json!({}),
        DefineToolOutput {
            schema: json!({
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "name": { "type": "string", "required": true },
                        "status": {
                            "type": "string",
                            "required": true,
                            "enum": ["running", "inactive", "corrupt", "unsupported", "unavailable"]
                        },
                        "description": { "type": "string" }
                    }
                }
            }),
            render: Arc::new(|_, value| render_entries(value)),
            presentation_meta: None,
        },
        move |_, exec| {
            let subagents = execute_subagents.clone();
            let agents = execute_agents.clone();
            Box::pin(async move {
                let caller = exec.agent.clone().ok_or_else(|| -> BoxError {
                    Box::new(ToolContextUnavailableError::new(
                        "list_agents requires a calling agent (exec.agent was undefined)",
                    ))
                })?;
                let root = subagents.root_of(&caller)?;
                let descendants = subagents
                    .list_descendants(root.id(), Some(exec.signal()))
                    .await?;
                let mut output = vec![entry(
                    ROOT_AGENT_NAME.into(),
                    status_of(&agents, root.id()),
                    Some("root agent".into()),
                )];
                for descendant in descendants {
                    let id = match &descendant.entry {
                        SubagentListEntry::Child { id, .. }
                        | SubagentListEntry::Diagnostic { id, .. } => id.clone(),
                    };
                    let name = descendant.path.unwrap_or_else(|| id.as_str().to_owned());
                    match descendant.entry {
                        SubagentListEntry::Diagnostic { reason: why, .. } => {
                            output.push(entry(name, reason(why), None));
                        }
                        SubagentListEntry::Child { id, fields, .. }
                            if fields.mode == "continuable" =>
                        {
                            output.push(entry(name, status_of(&agents, &id), fields.label));
                        }
                        SubagentListEntry::Child { .. } => {}
                    }
                }
                Ok(Value::Array(output))
            })
        },
    );
    options.is_concurrency_safe = Some(Arc::new(|_| true));
    tools.register(ctx, define_tool(options)?, Default::default())?;
    let scope = hoh_scope::scope_of(ctx);
    tools
        .get("list_agents", scope.as_ref(), None)
        .ok_or_else(|| "list_agents registration was not visible".into())
}
