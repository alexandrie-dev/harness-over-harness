//! Named request, response, and notification payloads for the SDK wire protocol.

use crate::shim::SubagentStopReason;
use hoh_commands::CommandId;
use hoh_llm::{ContentBlock, ReasoningEffortId};
use hoh_session::{SessionEvent, SessionId};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

pub use hoh_commands::CommandExecution;
pub use hoh_user_questions::{AskUserQuestionAnswer, AskUserQuestionItem};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_requests: Option<bool>,
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffortId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<InitializeCapabilities>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_questions: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SdkUserQuestionRequest {
    pub request_id: String,
    pub session_id: String,
    pub questions: Vec<AskUserQuestionItem>,
}

pub type SdkUserQuestionAnswer = AskUserQuestionAnswer;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiteralTrue;

impl Serialize for LiteralTrue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for LiteralTrue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if bool::deserialize(deserializer)? {
            Ok(Self)
        } else {
            Err(D::Error::custom("expected true"))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdkUserQuestionCancelledResult {
    pub cancelled: LiteralTrue,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SdkUserQuestionResult {
    Answer(SdkUserQuestionAnswer),
    Cancelled(SdkUserQuestionCancelledResult),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SdkUserQuestionCancelled {
    pub request_id: String,
    pub session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub server_info: ServerInfo,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPromptParams {
    pub session_id: String,
    pub content_blocks: Vec<SdkPromptContentBlock>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SdkImageMediaType {
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/webp")]
    Webp,
    #[serde(rename = "image/gif")]
    Gif,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SdkEncodedImageBlock {
    #[serde(rename = "type")]
    pub r#type: EncodedImageTag,
    pub data: String,
    pub mime_type: SdkImageMediaType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EncodedImageTag {
    #[default]
    #[serde(rename = "image")]
    Image,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SdkPromptContentBlock {
    // This must precede the lossless/open ContentBlock: both use `type: image`.
    EncodedImage(SdkEncodedImageBlock),
    Content(ContentBlock),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPromptResult {
    pub message_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionQueryParams {
    pub session_id: SessionId,
    pub content_blocks: Vec<SdkPromptContentBlock>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum SessionQueryResult {
    Prompt { message_id: String },
    Command { execution: CommandExecution },
}

macro_rules! session_id_params {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub struct $name {
            pub session_id: String,
        }
    };
}

session_id_params!(SessionResumeParams);
session_id_params!(SessionWaitParams);
session_id_params!(SessionWorkingDirectoryParams);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionWorkingDirectorySetParams {
    pub session_id: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionWorkingDirectoryResult {
    pub cwd: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SdkRunStatus {
    Ok,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEventNotification {
    pub session_id: String,
    pub event: SessionEvent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Idle,
    Running,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatusNotification {
    pub session_id: String,
    pub status: SessionStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentStartedNotification {
    pub parent_session_id: String,
    pub child_session_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentFinishedNotification {
    pub provider: String,
    pub agent_id: String,
    pub parent_session_id: String,
    pub child_session_id: String,
    pub status: SdkRunStatus,
    pub stop_reason: SubagentStopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_assistant_message: Option<Vec<ContentBlock>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionQueryCompletedNotification {
    pub session_id: SessionId,
    pub command_id: CommandId,
    pub final_response: String,
    pub finish_reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmptyResult {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequestParams {
    pub session_id: String,
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalOutcome {
    AllowedOnce,
    Rejected,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequestResult {
    pub outcome: ApprovalOutcome,
}

pub const HOST_REQUEST_USER_QUESTION_ASK: &str = "user-question/ask";
pub const REQUEST_INITIALIZE: &str = "initialize";
pub const REQUEST_SESSION_PROMPT: &str = "session/prompt";
pub const REQUEST_SESSION_QUERY: &str = "session/query";
pub const REQUEST_SESSION_RESUME: &str = "session/resume";
pub const REQUEST_SESSION_WAIT: &str = "session/wait";
pub const REQUEST_WORKING_DIRECTORY_GET: &str = "session/working-directory/get";
pub const REQUEST_WORKING_DIRECTORY_SET: &str = "session/working-directory/set";
pub const REQUEST_SHUTDOWN: &str = "shutdown";
pub const CLIENT_REQUEST_APPROVAL: &str = "approval/request";
pub const NOTIFICATION_USER_QUESTION_CANCELLED: &str = "user-question.cancelled";
pub const NOTIFICATION_SESSION_EVENT: &str = "session.event";
pub const NOTIFICATION_SESSION_STATUS: &str = "session.status";
pub const NOTIFICATION_SESSION_QUERY_COMPLETED: &str = "session.query.completed";
pub const NOTIFICATION_SUBAGENT_STARTED: &str = "subagent.started";
pub const NOTIFICATION_SUBAGENT_FINISHED: &str = "subagent.finished";

pub const HARNESS_SDK_REQUEST_METHODS: &[&str] = &[
    REQUEST_INITIALIZE,
    REQUEST_SESSION_PROMPT,
    REQUEST_SESSION_QUERY,
    REQUEST_SESSION_RESUME,
    REQUEST_SESSION_WAIT,
    REQUEST_WORKING_DIRECTORY_GET,
    REQUEST_WORKING_DIRECTORY_SET,
    REQUEST_SHUTDOWN,
];

pub const HARNESS_SDK_NOTIFICATION_METHODS: &[&str] = &[
    NOTIFICATION_USER_QUESTION_CANCELLED,
    NOTIFICATION_SESSION_EVENT,
    NOTIFICATION_SESSION_STATUS,
    NOTIFICATION_SESSION_QUERY_COMPLETED,
    NOTIFICATION_SUBAGENT_STARTED,
    NOTIFICATION_SUBAGENT_FINISHED,
];

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_commands::CommandResult;
    use serde_json::{Value, json};

    #[test]
    fn initialize_omits_optional_fields_instead_of_writing_nulls() {
        let params = InitializeParams {
            cwd: "/work".into(),
            approval_requests: None,
            provider: "openai".into(),
            model: "gpt".into(),
            reasoning_effort: None,
            max_tokens: None,
            capabilities: None,
        };
        assert_eq!(
            serde_json::to_value(params).unwrap(),
            json!({ "cwd": "/work", "provider": "openai", "model": "gpt" })
        );
    }

    #[test]
    fn user_question_cancelled_requires_literal_true() {
        let result: SdkUserQuestionResult =
            serde_json::from_value(json!({ "cancelled": true })).unwrap();
        assert!(matches!(result, SdkUserQuestionResult::Cancelled(_)));
        assert!(
            serde_json::from_value::<SdkUserQuestionResult>(json!({ "cancelled": false })).is_err()
        );
    }

    #[test]
    fn prompt_blocks_distinguish_inline_and_durable_images_and_keep_open_blocks() {
        let encoded = json!({
            "type": "image",
            "data": "AA==",
            "mimeType": "image/png",
            "name": "pixel.png"
        });
        let durable = json!({
            "type": "image",
            "attachment": { "kind": "image", "id": "att-1" }
        });
        let plugin = json!({ "type": "plugin/custom", "answer": 42 });

        let parsed: SdkPromptContentBlock = serde_json::from_value(encoded.clone()).unwrap();
        assert!(matches!(parsed, SdkPromptContentBlock::EncodedImage(_)));
        assert_eq!(serde_json::to_value(parsed).unwrap(), encoded);

        let parsed: SdkPromptContentBlock = serde_json::from_value(durable.clone()).unwrap();
        assert!(matches!(parsed, SdkPromptContentBlock::Content(_)));
        assert_eq!(serde_json::to_value(parsed).unwrap(), durable);

        let parsed: SdkPromptContentBlock = serde_json::from_value(plugin.clone()).unwrap();
        assert!(matches!(parsed, SdkPromptContentBlock::Content(_)));
        assert_eq!(serde_json::to_value(parsed).unwrap(), plugin);
    }

    #[test]
    fn prompt_block_with_unknown_fields_remains_lossless_open_content() {
        let extended = json!({
            "type": "image",
            "data": "AA==",
            "mimeType": "image/png",
            "futureField": { "answer": 42 }
        });

        let parsed: SdkPromptContentBlock = serde_json::from_value(extended.clone()).unwrap();
        assert!(matches!(parsed, SdkPromptContentBlock::Content(_)));
        assert_eq!(serde_json::to_value(parsed).unwrap(), extended);
    }

    #[test]
    fn prompt_query_result_uses_camel_case_in_both_wire_directions() {
        let wire = json!({ "kind": "prompt", "messageId": "message-1" });
        let parsed: SessionQueryResult = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(
            parsed,
            SessionQueryResult::Prompt {
                message_id: "message-1".into()
            }
        );
        assert_eq!(serde_json::to_value(parsed).unwrap(), wire);
    }

    #[test]
    fn command_result_uses_owner_branded_id_and_exact_wire_shape() {
        let result = SessionQueryResult::Command {
            execution: CommandExecution {
                command_id: "command-1".into(),
                result: CommandResult::success(Some("done")),
            },
        };
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            json!({
                "kind": "command",
                "execution": {
                    "commandId": "command-1",
                    "result": { "kind": "success", "text": "done" }
                }
            })
        );
    }

    #[test]
    fn session_event_notification_preserves_raw_extension_fields() {
        let event = json!({
            "type": "plugin/future",
            "seq": 3,
            "time": 10,
            "data": { "x": 1 },
            "futureField": [true, null]
        });
        let notification = SessionEventNotification {
            session_id: "session-1".into(),
            event: SessionEvent::from_value(event.clone()),
        };
        assert_eq!(
            serde_json::to_value(notification).unwrap(),
            json!({ "sessionId": "session-1", "event": event })
        );
    }

    #[test]
    fn method_constants_cover_source_maps_exactly() {
        assert_eq!(
            HARNESS_SDK_REQUEST_METHODS,
            [
                "initialize",
                "session/prompt",
                "session/query",
                "session/resume",
                "session/wait",
                "session/working-directory/get",
                "session/working-directory/set",
                "shutdown",
            ]
        );
        assert_eq!(
            HARNESS_SDK_NOTIFICATION_METHODS,
            [
                "user-question.cancelled",
                "session.event",
                "session.status",
                "session.query.completed",
                "subagent.started",
                "subagent.finished",
            ]
        );
        assert_eq!(HOST_REQUEST_USER_QUESTION_ASK, "user-question/ask");
        assert_eq!(CLIENT_REQUEST_APPROVAL, "approval/request");
        assert_eq!(
            serde_json::to_value(EmptyResult::default()).unwrap(),
            Value::Object(Default::default())
        );
    }
}
