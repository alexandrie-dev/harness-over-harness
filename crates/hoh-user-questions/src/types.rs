//! Client-safe question, answer, request, and projection types.

use hoh_agent::Agent;
use hoh_llm::ToolCallId;
use hoh_timeout::AbortSignal;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskUserQuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

pub type LocalizedQuestionText = IndexMap<String, String>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalSubject {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalDisplay {
    pub question: LocalizedQuestionText,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<LocalizedQuestionText>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approve: Option<LocalizedQuestionText>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AskUserQuestionIntent {
    SecretFile,
    PlanReview {
        approve: String,
        #[serde(default, rename = "callId", skip_serializing_if = "Option::is_none")]
        call_id: Option<ToolCallId>,
    },
    Approval {
        approve: String,
        reject: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        always: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subject: Option<ApprovalSubject>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<Box<ApprovalDisplay>>,
    },
}

impl AskUserQuestionIntent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SecretFile => "secret-file",
            Self::PlanReview { .. } => "plan-review",
            Self::Approval { .. } => "approval",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AskUserQuestionItem {
    pub id: String,
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<AskUserQuestionOption>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multi_select: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<AskUserQuestionIntent>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskUserQuestionAnswerItem {
    pub id: String,
    pub selected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskUserQuestionAnswer {
    pub answers: Vec<AskUserQuestionAnswerItem>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserQuestionState {
    Open,
    Continued,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingUserQuestion {
    pub call_id: ToolCallId,
    pub questions: Vec<AskUserQuestionItem>,
    pub state: UserQuestionState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettledUserQuestion {
    pub call_id: ToolCallId,
    pub answers: Vec<AskUserQuestionAnswerItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserQuestionProjectionView {
    pub active: Vec<PendingUserQuestion>,
    pub settled: Vec<SettledUserQuestion>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserQuestionWait {
    pub call_id: ToolCallId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timed: Option<bool>,
}

#[derive(Clone)]
pub struct AskUserQuestionRequestEvent {
    pub questions: Vec<AskUserQuestionItem>,
    pub agent: Option<Arc<dyn Agent>>,
    pub signal: Option<AbortSignal>,
    pub wait: Option<UserQuestionWait>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TimedUserQuestionResult {
    Answer(AskUserQuestionAnswer),
    Pending {
        pending: bool,
        #[serde(rename = "callId")]
        call_id: ToolCallId,
    },
}

impl TimedUserQuestionResult {
    pub fn pending(call_id: ToolCallId) -> Self {
        Self::Pending {
            pending: true,
            call_id,
        }
    }
}
