//! Field order and SDK names follow packages/sdk/protocol/src/types.ts.
use crate::{Empty, JsonObject, SecretString, choices, object, optional};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

optional!(InitializeParams {
    cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    approval_requests: Option<bool>,
    provider: String,
    model: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    max_tokens: Option<PositiveSafeInteger>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    capabilities: Option<Capabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    hoh: Option<HohInitialize>,
});
optional!(Capabilities {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    user_questions: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    approvals: Option<ApprovalHost>,
});
choices!(ApprovalHost { Host => "host" });
optional!(HohInitialize {
    protocol_version: u32,
    /// 64 lowercase hex characters, native host only; never forward to JS.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    approval_key: Option<SecretString>,
});
object!(ServerInfo {
    name: String,
    version: String
});
optional!(InitializeResult {
    server_info: ServerInfo,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    hoh: Option<HohInitializeResult>,
});
object!(HohInitializeResult {
    protocol_version: u32,
    approvals: ApprovalHost
});
object!(SessionPromptParams { session_id: String, content_blocks: Vec<SdkPromptContentBlock> });
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionQueryParams {
    pub session_id: String,
    pub content_blocks: Vec<SdkPromptContentBlock>,
}
object!(SessionPromptResult { message_id: String });
object!(SessionResumeParams { session_id: String });
pub type SessionWaitParams = SessionResumeParams;
pub type SessionWorkingDirectoryParams = SessionResumeParams;
object!(SessionWorkingDirectorySetParams {
    session_id: String,
    path: String
});
object!(SessionWorkingDirectoryResult { cwd: String });

choices!(ImageMediaType { Png => "image/png", Jpeg => "image/jpeg", Webp => "image/webp", Gif => "image/gif" });
object!(Dimensions {
    width: u64,
    height: u64
});
optional!(ImageAttachmentRef {
    attachment_id: String, media_type: ImageMediaType, bytes: u64, width: u64, height: u64,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    original_dimensions: Option<Dimensions>,
});
object!(FileAttachmentRef {
    attachment_id: String,
    name: String,
    bytes: u64
});

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "reasoning")]
    Reasoning { text: String },
    #[serde(rename = "image")]
    Image {
        attachment: ImageAttachmentRef,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::present"
        )]
        #[schemars(required)]
        offloaded: Option<True>,
    },
    #[serde(rename = "file")]
    File { attachment: FileAttachmentRef },
    #[serde(rename = "tool-call")]
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "tool-addition")]
    ToolAddition {
        #[serde(rename = "toolName")]
        tool_name: String,
    },
    #[serde(rename = "tool-removal")]
    ToolRemoval {
        #[serde(rename = "toolName")]
        tool_name: String,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SdkPromptContentBlock {
    Durable(ContentBlock),
    Encoded(SdkEncodedImageBlock),
}
choices!(ImageTag { Image => "image" });
optional!(SdkEncodedImageBlock {
    #[serde(rename = "type")]
    r#type: ImageTag,
    data: String,
    mime_type: ImageMediaType,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    name: Option<String>,
});
/// Literal true, rather than a bool that would admit false.
#[derive(Clone, Debug, PartialEq, JsonSchema)]
#[schemars(with = "bool", extend("const" = true))]
pub struct True;
impl Serialize for True {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bool(true)
    }
}
impl<'de> Deserialize<'de> for True {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if bool::deserialize(d)? {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("expected true"))
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CommandResult {
    Success {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::present"
        )]
        #[schemars(required)]
        text: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::present",
            rename = "sourceEventSeq"
        )]
        #[schemars(required)]
        source_event_seq: Option<u64>,
    },
    Error {
        text: String,
    },
}
object!(CommandExecution {
    command_id: String,
    result: CommandResult
});
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SessionQueryResult {
    Prompt {
        #[serde(rename = "messageId")]
        message_id: String,
    },
    Command {
        execution: CommandExecution,
    },
}

optional!(AskUserQuestionOption {
    label: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    description: Option<String>,
});
optional!(QuestionSubject {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    icon: Option<String>,
});
optional!(ApprovalDisplay {
    question: LocalizedText,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    detail: Option<LocalizedText>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    approve: Option<LocalizedText>,
});
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LocalizedText {
    pub en: String,
    #[serde(flatten)]
    pub locales: std::collections::BTreeMap<String, String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum AskUserQuestionIntent {
    #[serde(rename = "plan-review")]
    PlanReview {
        approve: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::present",
            rename = "callId"
        )]
        #[schemars(required)]
        call_id: Option<String>,
    },
    #[serde(rename = "approval")]
    Approval {
        approve: String,
        reject: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::present"
        )]
        #[schemars(required)]
        always: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::present"
        )]
        #[schemars(required)]
        subject: Option<QuestionSubject>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::present"
        )]
        #[schemars(required)]
        display: Option<Box<ApprovalDisplay>>,
    },
}
optional!(AskUserQuestionItem {
    id: String, question: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    options: Option<Vec<AskUserQuestionOption>>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    multi_select: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    intent: Option<AskUserQuestionIntent>,
});
optional!(AskUserQuestionAnswerItem {
    id: String, selected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    custom: Option<String>,
});
object!(SdkUserQuestionRequest { request_id: String, session_id: String, questions: Vec<AskUserQuestionItem> });
object!(SdkUserQuestionAnswer { answers: Vec<AskUserQuestionAnswerItem> });
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SdkUserQuestionResult {
    Answer(SdkUserQuestionAnswer),
    Cancelled { cancelled: True },
}
object!(SdkUserQuestionCancelled {
    request_id: String,
    session_id: String
});
optional!(ApprovalRequestParams {
    session_id: String, tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    #[schemars(length(min = 1, max = 128), regex(pattern = "^[A-Za-z0-9_.:-]+$"))]
    approval_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    risk: Option<crate::Risk>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    human_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    #[schemars(length(min = 64, max = 64), regex(pattern = "^[0-9a-f]{64}$"))]
    action_hash: Option<String>,
});
choices!(ApprovalOutcome { AllowedOnce => "allowed-once", Rejected => "rejected", Cancelled => "cancelled", Unavailable => "unavailable" });
optional!(ApprovalRequestResult {
    outcome: ApprovalOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    #[schemars(length(min = 1, max = 128), regex(pattern = "^[A-Za-z0-9_.:-]+$"))]
    approval_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    #[schemars(length(min = 64, max = 64), regex(pattern = "^[0-9a-f]{64}$"))]
    mac: Option<SecretString>,
});
// Plugin-extended event data is deliberately ordered JSON, like SessionEventMap.
optional!(SessionEvent {
    #[serde(rename = "type")]
    r#type: String,
    seq: u64, time: u64, data: Value,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    ignorable: Option<True>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    surface_op: Option<SurfaceOp>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    source_event_seqs: Option<Vec<u64>>,
    #[serde(flatten)]
    extensions: JsonObject,
});
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SurfaceOp {
    Append(Append),
    Replace {
        op: Replace,
        #[serde(rename = "startSeq")]
        start_seq: u64,
        #[serde(rename = "endSeq")]
        end_seq: u64,
    },
}
choices!(Append { Append => "append" });
choices!(Replace { Replace => "replace" });
object!(SessionEventNotification {
    session_id: String,
    event: SessionEvent
});
/// Window-only projection of one process-local assistant attempt. Only visible text deltas cross
/// the JSON-RPC boundary; reasoning, tool-call, usage, and structural model chunks stay internal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum SessionAssistantStreamFrame {
    Start {
        attempt_id: String,
        revision: u64,
        turn: u64,
        step: u64,
    },
    Chunk {
        attempt_id: String,
        revision: u64,
        index: u64,
        time: i64,
        chunk: SessionAssistantStreamChunk,
    },
    End {
        attempt_id: String,
        revision: u64,
        index: u64,
        outcome: SessionAssistantStreamOutcome,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SessionAssistantStreamChunk {
    TextDelta { text: String },
}
choices!(SessionAssistantStreamOutcome {
    Committed => "committed",
    Abandoned => "abandoned"
});
object!(SessionAssistantStreamNotification {
    session_id: String,
    frame: SessionAssistantStreamFrame
});
choices!(SessionStatus { Idle => "idle", Running => "running" });
object!(SessionStatusNotification {
    session_id: String,
    status: SessionStatus
});
object!(SessionQueryCompleted {
    session_id: String,
    command_id: String,
    final_response: String,
    finish_reason: String
});
object!(SubagentStartedNotification {
    parent_session_id: String,
    child_session_id: String
});
choices!(SdkRunStatus { Ok => "ok", Error => "error" });
optional!(SubagentFinishedNotification {
    provider: String, agent_id: String, parent_session_id: String, child_session_id: String,
    status: SdkRunStatus,
    /// Merge-extensible provider-owned stop reason.
    stop_reason: Value,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    last_assistant_message: Option<Vec<ContentBlock>>,
});
pub type ShutdownResult = Empty;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(transparent)]
#[schemars(transparent)]
pub struct PositiveSafeInteger(#[schemars(range(min = 1, max = 9007199254740991u64))] u64);
impl PositiveSafeInteger {
    pub fn get(self) -> u64 {
        self.0
    }
}
impl<'de> Deserialize<'de> for PositiveSafeInteger {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let n = u64::deserialize(d)?;
        if (1..=9_007_199_254_740_991).contains(&n) {
            Ok(Self(n))
        } else {
            Err(serde::de::Error::custom("expected positive safe integer"))
        }
    }
}
