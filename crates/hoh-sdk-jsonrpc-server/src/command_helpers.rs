//! Only the helpers imported by SDK: packages/interaction/commands/src/builtin.ts.
//! The command registry and handlers are not ported here.
use serde::{Deserialize, Serialize};

hoh_brand::branded_string!(pub CommandDefinitionId);

/// SHIM(hoh-commands): the descriptor fields used by builtin.ts; replace with its descriptor.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandDescriptor {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition_id: Option<CommandDefinitionId>,
    pub description: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinCommandName {
    Goal,
    Plan,
    Feedback,
    Compact,
    Permission,
    Export,
}
impl BuiltinCommandName {
    pub fn definition_id(self) -> &'static str {
        match self {
            Self::Goal => "@deepseek-ai/dsh-command-goal",
            Self::Plan => "@deepseek-ai/dsh-plan-mode",
            Self::Feedback => "@deepseek-ai/dsh-command-feedback",
            Self::Compact => "@deepseek-ai/dsh-command-compact",
            Self::Permission => "@deepseek-ai/dsh-permission-presets",
            Self::Export => "@deepseek-ai/dsh-session-log-export",
        }
    }
}
pub fn builtin_command_name(descriptor: &CommandDescriptor) -> Option<BuiltinCommandName> {
    use BuiltinCommandName::*;
    [Goal, Plan, Feedback, Compact, Permission, Export]
        .into_iter()
        .find(|name| {
            descriptor
                .definition_id
                .as_ref()
                .is_some_and(|id| id.as_str() == name.definition_id())
        })
}
pub fn resolve_command<'a>(
    token: &str,
    descriptors: &'a [CommandDescriptor],
) -> Option<&'a CommandDescriptor> {
    if let Some(exact) = descriptors.iter().find(|d| d.name == token) {
        return Some(exact);
    }
    use BuiltinCommandName::*;
    let name = match token {
        "goal" | "目标" => Goal,
        "plan" | "计划" => Plan,
        "feedback" | "反馈" => Feedback,
        "compact" | "压缩" => Compact,
        "permission" | "权限" => Permission,
        "export" | "导出" => Export,
        _ => return None,
    };
    descriptors
        .iter()
        .find(|d| builtin_command_name(d) == Some(name))
}
