//! Provider-neutral, lossless vocabulary. Extensible tags retain unknown payloads.
pub use crate::brand::*;
pub use crate::message::*;
use hoh_attachment::{FileAttachmentRef, ImageAttachmentRef};
use hoh_package_manifest::LocalizedText;
use hoh_timeout::AbortSignal;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmFailure {
    pub message: String,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_retry_after_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<ProviderRequestId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offload_images: Option<u64>,
}
impl LlmFailure {
    pub fn new(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: code.into(),
            status: None,
            provider_retry_after_ms: None,
            request_id: None,
            offload_images: None,
        }
    }
}

/// Declaration-merged content: the owner retains a tag and arbitrary ordered JSON.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContentBlock {
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(flatten)]
    pub data: Map<String, Value>,
}
pub type ContentBlockType = String;
impl ContentBlock {
    pub fn new(kind: impl Into<String>, data: Map<String, Value>) -> Self {
        Self {
            r#type: kind.into(),
            data,
        }
    }
    pub fn text(text: impl Into<String>) -> Self {
        Self::new(
            "text",
            Map::from_iter([("text".into(), Value::String(text.into()))]),
        )
    }
    pub fn reasoning(text: impl Into<String>) -> Self {
        Self {
            r#type: "reasoning".into(),
            ..Self::text(text)
        }
    }
    pub fn text_value(&self) -> Option<&str> {
        self.data.get("text").and_then(Value::as_str)
    }
    pub fn tool_name(&self) -> Option<&str> {
        self.data.get("toolName").and_then(Value::as_str)
    }
    pub fn image(&self) -> Option<ImageBlock> {
        if self.r#type != "image" {
            return None;
        }
        serde_json::from_value(serde_json::to_value(self).ok()?).ok()
    }
    pub fn file(&self) -> Option<FileBlock> {
        if self.r#type != "file" {
            return None;
        }
        serde_json::from_value(serde_json::to_value(self).ok()?).ok()
    }
}
// Typed payloads convert into the open tagged representation without constraining extensions.
macro_rules! payload {
    ($name:ident, $tag:literal, {$($field:ident: $ty:ty),* $(,)?}) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[serde(tag = "type", rename = $tag, rename_all = "camelCase")]
        pub struct $name { $(pub $field: $ty),* }
        impl From<$name> for ContentBlock {
            fn from(value: $name) -> Self {
                // All payload fields are JSON values or primitive types.
                let Value::Object(mut data) = serde_json::to_value(value).expect("content payload is an object") else { unreachable!("struct serializes as object") };
                data.shift_remove("type");
                Self::new($tag, data)
            }
        }
    }
}
payload!(TextBlock, "text", {text: String});
payload!(ReasoningBlock, "reasoning", {text: String});
payload!(FileBlock, "file", {attachment: FileAttachmentRef});
payload!(ToolCallBlock, "tool-call", {id: ToolCallId, name: String, arguments: String});
payload!(ToolAdditionBlock, "tool-addition", {tool_name: String});
payload!(ToolRemovalBlock, "tool-removal", {tool_name: String});
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "image")]
pub struct ImageBlock {
    pub attachment: ImageAttachmentRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offloaded: Option<bool>,
}
impl From<ImageBlock> for ContentBlock {
    fn from(value: ImageBlock) -> Self {
        // Image payload contains only serializable attachment metadata.
        let Value::Object(mut data) =
            serde_json::to_value(value).expect("image payload is an object")
        else {
            unreachable!("struct serializes as object")
        };
        data.shift_remove("type");
        Self::new("image", data)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FinishReason {
    pub kind: String,
    #[serde(flatten)]
    pub data: Map<String, Value>,
}
impl FinishReason {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            data: Map::new(),
        }
    }
    pub fn failed(kind: &str, failure: LlmFailure) -> Self {
        // Failure facts consist exclusively of JSON-safe fields after normalization.
        Self {
            kind: kind.into(),
            data: Map::from_iter([(
                "failure".into(),
                serde_json::to_value(failure).expect("failure is JSON"),
            )]),
        }
    }
    pub fn failure(&self) -> Option<LlmFailure> {
        serde_json::from_value(self.data.get("failure")?.clone()).ok()
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmImageRequestPrice {
    pub visual_tokens: u64,
    pub text: String,
}
pub trait LlmImageRequestPricing: Send + Sync + 'static {
    fn price_images(&self, images: &[ImageBlock]) -> Vec<LlmImageRequestPrice>;
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LlmProviderInfo {
    pub id: String,
    pub name: String,
}
/// Open modality and request-purpose vocabularies are strings, as for declaration merging.
pub type ModelModality = String;
pub type LlmRequestPurpose = String;
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmConfigurableProvider {
    pub provider: String,
    pub display_name: String,
    pub settings_ns: String,
    pub settings_path: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared: Option<bool>,
    #[serde(
        default,
        rename = "defaultBaseURL",
        skip_serializing_if = "Option::is_none"
    )]
    pub default_base_url: Option<String>,
    #[serde(
        default,
        rename = "baseURLEnv",
        skip_serializing_if = "Option::is_none"
    )]
    pub base_url_env: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_auth: Option<LlmNativeAuth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LlmNativeAuth {
    pub env: Vec<String>,
    pub oauth: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmModelDiscoveryRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, rename = "baseURL", skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LlmModelDiscoveryOperation {
    #[serde(flatten)]
    pub request: LlmModelDiscoveryRequest,
    #[serde(skip)]
    pub signal: Option<AbortSignal>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmDiscoveredModel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_modalities: Option<Vec<ModelModality>>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmModelBadgeTone {
    Neutral,
    Info,
    Success,
    Warning,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LlmModelBadge {
    pub text: LocalizedText,
    pub tone: LlmModelBadgeTone,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmModelInfo {
    pub provider: String,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_modalities: Option<Vec<ModelModality>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<LlmModelBadge>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmModelContext {
    pub context_window: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageRepresentation {
    Raw,
    Base64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmImageRequestBudget {
    pub representation: ImageRepresentation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_images: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_quantum: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count_quantum: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LlmReasoningEffortInfo {
    pub id: ReasoningEffortId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmModelReasoningInfo {
    pub efforts: Vec<LlmReasoningEffortInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<ReasoningEffortId>,
}
// Strings at the adapter boundary allow the runtime to return the source's coded validation errors.
pub type SystemPromptUpdate = String;
pub type ToolUpdate = String;
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmResolvedModelInfo {
    #[serde(flatten)]
    pub model: LlmModelInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<LlmModelContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<LlmModelReasoningInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_update: Option<SystemPromptUpdate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_update: Option<ToolUpdate>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayEnvelope {
    pub response: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<Value>>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum StreamChunk {
    BlockStart {
        index: u64,
        block_type: ContentBlockType,
    },
    TextDelta {
        index: u64,
        text: String,
    },
    ReasoningDelta {
        index: u64,
        text: String,
    },
    ToolCallDelta {
        index: u64,
        id: ToolCallId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        arguments_delta: String,
    },
    BlockEnd {
        index: u64,
        block: ContentBlock,
    },
    Usage {
        usage: TokenUsage,
    },
    Finish {
        reason: FinishReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replay_state: Option<ReplayEnvelope>,
    },
}
impl StreamChunk {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::BlockStart { .. } => "block-start",
            Self::TextDelta { .. } => "text-delta",
            Self::ReasoningDelta { .. } => "reasoning-delta",
            Self::ToolCallDelta { .. } => "tool-call-delta",
            Self::BlockEnd { .. } => "block-end",
            Self::Usage { .. } => "usage",
            Self::Finish { .. } => "finish",
        }
    }
    pub fn finish(reason: FinishReason) -> Self {
        Self::Finish {
            reason,
            replay_state: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSchema {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defer_loading: Option<bool>,
    pub name: String,
    pub description: String,
    pub parameters: Map<String, Value>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestUserInput {
    pub role: RequestUserRole,
    pub content: Vec<ContentBlock>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RequestUserRole {
    #[serde(rename = "user")]
    User,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestMessage {
    Durable(Arc<Message>),
    Input(RequestUserInput),
}
impl RequestMessage {
    pub fn content(&self) -> &[ContentBlock] {
        match self {
            Self::Durable(m) => m.content(),
            Self::Input(m) => &m.content,
        }
    }
    pub fn role(&self) -> &str {
        match self {
            Self::Durable(m) => m.role(),
            Self::Input(_) => "user",
        }
    }
    pub fn id(&self) -> Option<&MessageId> {
        match self {
            Self::Durable(m) => Some(m.id()),
            Self::Input(_) => None,
        }
    }
    pub fn with_content(&self, content: Vec<ContentBlock>) -> Self {
        match self {
            Self::Durable(m) => {
                let mut m = (**m).clone();
                *m.content_mut() = content;
                Self::Durable(Arc::new(m))
            }
            Self::Input(m) => Self::Input(RequestUserInput {
                content,
                ..m.clone()
            }),
        }
    }
}
impl From<Message> for RequestMessage {
    fn from(m: Message) -> Self {
        Self::Durable(Arc::new(m))
    }
}
impl From<Arc<Message>> for RequestMessage {
    fn from(m: Arc<Message>) -> Self {
        Self::Durable(m)
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolHistory {
    pub tools: Vec<ToolSchema>,
    pub updates: Vec<ToolHistoryUpdate>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolHistoryUpdate {
    pub message_id: MessageId,
    pub additions: Vec<ToolSchema>,
}
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateOptions {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffortId>,
    pub messages: Vec<RequestMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolSchema>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_history: Option<ToolHistory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    #[serde(skip)]
    pub signal: Option<AbortSignal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<LlmRequestPurpose>,
    #[serde(skip)]
    #[doc(hidden)]
    pub agent_loop_request: bool,
}
impl Clone for GenerateOptions {
    fn clone(&self) -> Self {
        Self {
            provider: self.provider.clone(),
            model: self.model.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            messages: self.messages.clone(),
            system: self.system.clone(),
            tools: self.tools.clone(),
            tool_history: self.tool_history.clone(),
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            stop: self.stop.clone(),
            signal: self.signal.clone(),
            session_id: self.session_id.clone(),
            purpose: self.purpose.clone(),
            agent_loop_request: false,
        }
    }
}
