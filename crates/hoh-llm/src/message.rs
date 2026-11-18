//! Owned snapshots replace JavaScript structuredClone/deepFreeze.
use crate::types::{ContentBlock, MessageId, ToolCallId};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantProviderMetadata {
    pub provider: String,
    pub model: String,
    /// Some(Null) is explicit null; None is absence.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_json"
    )]
    pub replay_state: Option<Value>,
}
pub(crate) fn present_json<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelMessageSource {
    pub kind: ModelSourceKind,
    #[serde(flatten)]
    pub metadata: AssistantProviderMetadata,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ModelSourceKind {
    #[serde(rename = "model")]
    Model,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolMessageSource {
    pub kind: ToolSourceKind,
    pub call_id: ToolCallId,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ToolSourceKind {
    #[serde(rename = "tool")]
    Tool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SystemPromptMessageSource {
    pub kind: SystemPromptSourceKind,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SystemPromptSourceKind {
    #[serde(rename = "system-prompt")]
    SystemPrompt,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextForm {
    Instructions,
    Catalog,
    Snapshot,
    Notice,
    Relay,
    Recall,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextSnapshotSection {
    pub name: String,
    pub text: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "form", rename_all = "lowercase")]
pub enum ContextFormed {
    Instructions,
    Catalog,
    Snapshot {
        sections: Vec<ContextSnapshotSection>,
    },
    Notice {
        summary: String,
    },
    Relay,
    Recall,
}
/// Declaration-merged source, preserving fields belonging to other producers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessageSource {
    pub kind: String,
    #[serde(flatten)]
    pub data: Map<String, Value>,
}
impl MessageSource {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            data: Map::new(),
        }
    }
}
pub const CONTEXT_SUMMARY_MAX_CHARS: usize = 120;
pub fn bound_context_summary(summary: &str) -> String {
    if summary.encode_utf16().count() <= CONTEXT_SUMMARY_MAX_CHARS {
        return summary.into();
    }
    // Rust strings cannot contain a lone UTF-16 surrogate: retain only complete scalars.
    let mut count = 0;
    let mut end = 0;
    for (offset, ch) in summary.char_indices() {
        if count + ch.len_utf16() > CONTEXT_SUMMARY_MAX_CHARS - 1 {
            break;
        }
        count += ch.len_utf16();
        end = offset + ch.len_utf8();
    }
    format!("{}…", &summary[..end])
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "role", rename = "user")]
pub struct UserMessage {
    pub content: Vec<ContentBlock>,
    pub source: MessageSource,
    pub id: MessageId,
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "role", rename = "developer")]
pub struct DeveloperMessage {
    pub content: Vec<ContentBlock>,
    pub source: MessageSource,
    pub id: MessageId,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename = "system")]
pub struct SystemMessage {
    pub content: Vec<ContentBlock>,
    pub source: SystemPromptMessageSource,
    pub id: MessageId,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename = "assistant")]
pub struct AssistantMessage {
    pub content: Vec<ContentBlock>,
    pub source: ModelMessageSource,
    pub id: MessageId,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename = "tool", rename_all = "camelCase")]
pub struct ToolResultMessage {
    pub source: ToolMessageSource,
    pub tool_call_id: ToolCallId,
    pub content: Vec<ContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    pub id: MessageId,
}
/// Closed role vocabulary; every payload validates and serializes its own fixed role.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    System(SystemMessage),
    Developer(DeveloperMessage),
    User(UserMessage),
    Assistant(AssistantMessage),
    Tool(ToolResultMessage),
}
impl Message {
    pub fn role(&self) -> &'static str {
        match self {
            Self::System(_) => "system",
            Self::Developer(_) => "developer",
            Self::User(_) => "user",
            Self::Assistant(_) => "assistant",
            Self::Tool(_) => "tool",
        }
    }
    pub fn id(&self) -> &MessageId {
        match self {
            Self::System(m) => &m.id,
            Self::Developer(m) => &m.id,
            Self::User(m) => &m.id,
            Self::Assistant(m) => &m.id,
            Self::Tool(m) => &m.id,
        }
    }
    pub fn content(&self) -> &[ContentBlock] {
        match self {
            Self::System(m) => &m.content,
            Self::Developer(m) => &m.content,
            Self::User(m) => &m.content,
            Self::Assistant(m) => &m.content,
            Self::Tool(m) => &m.content,
        }
    }
    pub(crate) fn content_mut(&mut self) -> &mut Vec<ContentBlock> {
        match self {
            Self::System(m) => &mut m.content,
            Self::Developer(m) => &mut m.content,
            Self::User(m) => &mut m.content,
            Self::Assistant(m) => &mut m.content,
            Self::Tool(m) => &mut m.content,
        }
    }
}
#[derive(Clone, Debug)]
pub struct NewUserMessage {
    pub content: Vec<ContentBlock>,
    pub source: MessageSource,
}
pub type NewDeveloperMessage = NewUserMessage;
#[derive(Clone, Debug)]
pub struct NewAssistantMessage {
    pub content: Vec<ContentBlock>,
    pub source: AssistantProviderMetadata,
}
pub fn freeze_message<T: Clone>(message: &T) -> T {
    message.clone()
}
pub fn create_message(mut input: Message) -> Message {
    let id: MessageId = hoh_util_crypto::random_uuid().as_str().into();
    match &mut input {
        Message::System(m) => m.id = id,
        Message::Developer(m) => m.id = id,
        Message::User(m) => m.id = id,
        Message::Assistant(m) => m.id = id,
        Message::Tool(m) => m.id = id,
    }
    input
}
pub fn create_user_message(input: NewUserMessage) -> UserMessage {
    UserMessage {
        content: input.content,
        source: input.source,
        id: hoh_util_crypto::random_uuid().as_str().into(),
    }
}
pub fn create_developer_message(input: NewDeveloperMessage) -> DeveloperMessage {
    DeveloperMessage {
        content: input.content,
        source: input.source,
        id: hoh_util_crypto::random_uuid().as_str().into(),
    }
}
pub fn create_assistant_message(input: NewAssistantMessage) -> AssistantMessage {
    AssistantMessage {
        content: input.content,
        source: ModelMessageSource {
            kind: ModelSourceKind::Model,
            metadata: input.source,
        },
        id: hoh_util_crypto::random_uuid().as_str().into(),
    }
}
pub fn create_system_message(text: &str) -> SystemMessage {
    SystemMessage {
        content: if text.is_empty() {
            vec![]
        } else {
            vec![ContentBlock::text(text)]
        },
        source: SystemPromptMessageSource {
            kind: SystemPromptSourceKind::SystemPrompt,
        },
        id: hoh_util_crypto::random_uuid().as_str().into(),
    }
}
#[derive(Clone, Debug)]
pub struct ToolResultMessageInput {
    pub call_id: ToolCallId,
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
}
pub fn create_tool_result_message(input: ToolResultMessageInput) -> ToolResultMessage {
    ToolResultMessage {
        source: ToolMessageSource {
            kind: ToolSourceKind::Tool,
            call_id: input.call_id.clone(),
        },
        tool_call_id: input.call_id,
        content: input.content,
        is_error: Some(input.is_error),
        id: hoh_util_crypto::random_uuid().as_str().into(),
    }
}

macro_rules! message_value {
    ($type:ident, $variant:ident, $role:literal) => {
        impl $type {
            pub fn role(&self) -> &'static str {
                $role
            }
            pub fn id(&self) -> &MessageId {
                &self.id
            }
            pub fn content(&self) -> &[ContentBlock] {
                &self.content
            }
        }
        impl From<$type> for Message {
            fn from(value: $type) -> Self {
                Self::$variant(value)
            }
        }
        impl From<$type> for crate::types::RequestMessage {
            fn from(value: $type) -> Self {
                Message::$variant(value).into()
            }
        }
    };
}
message_value!(SystemMessage, System, "system");
message_value!(DeveloperMessage, Developer, "developer");
message_value!(UserMessage, User, "user");
message_value!(AssistantMessage, Assistant, "assistant");
message_value!(ToolResultMessage, Tool, "tool");

macro_rules! source_ordered_user_message {
    ($type:ident, $role:literal) => {
        impl Serialize for $type {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeStruct;
                let mut message = serializer.serialize_struct(stringify!($type), 4)?;
                message.serialize_field("content", &self.content)?;
                message.serialize_field("source", &self.source)?;
                message.serialize_field("role", $role)?;
                message.serialize_field("id", &self.id)?;
                message.end()
            }
        }
    };
}
source_ordered_user_message!(UserMessage, "user");
source_ordered_user_message!(DeveloperMessage, "developer");
