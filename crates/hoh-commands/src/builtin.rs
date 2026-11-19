//! First-party command identities and input spellings shared by adapters.

use crate::{CommandDefinitionId, CommandDescriptor};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuiltinCommandName {
    Goal,
    Plan,
    Feedback,
    Compact,
    Permission,
    Export,
}

impl BuiltinCommandName {
    pub const ALL: [Self; 6] = [
        Self::Goal,
        Self::Plan,
        Self::Feedback,
        Self::Compact,
        Self::Permission,
        Self::Export,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Goal => "goal",
            Self::Plan => "plan",
            Self::Feedback => "feedback",
            Self::Compact => "compact",
            Self::Permission => "permission",
            Self::Export => "export",
        }
    }

    pub const fn definition_id(self) -> &'static str {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltinCommandTokenSet {
    pub goal: &'static str,
    pub plan: &'static str,
    pub feedback: &'static str,
    pub compact: &'static str,
    pub permission: &'static str,
    pub export: &'static str,
}

impl BuiltinCommandTokenSet {
    pub const fn get(self, name: BuiltinCommandName) -> &'static str {
        match name {
            BuiltinCommandName::Goal => self.goal,
            BuiltinCommandName::Plan => self.plan,
            BuiltinCommandName::Feedback => self.feedback,
            BuiltinCommandName::Compact => self.compact,
            BuiltinCommandName::Permission => self.permission,
            BuiltinCommandName::Export => self.export,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltinCommandTokens {
    pub en: BuiltinCommandTokenSet,
    pub zh: BuiltinCommandTokenSet,
}

pub const BUILTIN_COMMAND_TOKENS: BuiltinCommandTokens = BuiltinCommandTokens {
    en: BuiltinCommandTokenSet {
        goal: "goal",
        plan: "plan",
        feedback: "feedback",
        compact: "compact",
        permission: "permission",
        export: "export",
    },
    zh: BuiltinCommandTokenSet {
        goal: "目标",
        plan: "计划",
        feedback: "反馈",
        compact: "压缩",
        permission: "权限",
        export: "导出",
    },
};

pub fn builtin_command_name(descriptor: &CommandDescriptor) -> Option<BuiltinCommandName> {
    BuiltinCommandName::ALL.into_iter().find(|name| {
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
    if let Some(exact) = descriptors
        .iter()
        .find(|descriptor| descriptor.name == token)
    {
        return Some(exact);
    }
    let name = BuiltinCommandName::ALL.into_iter().find(|name| {
        BUILTIN_COMMAND_TOKENS.en.get(*name) == token
            || BUILTIN_COMMAND_TOKENS.zh.get(*name) == token
    })?;
    descriptors.iter().find(|descriptor| {
        descriptor
            .definition_id
            .as_ref()
            .is_some_and(|id: &CommandDefinitionId| id.as_str() == name.definition_id())
    })
}
