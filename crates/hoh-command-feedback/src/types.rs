//! Public Session-level feedback vocabulary.

use hoh_session::SessionId;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiteralTrue;

impl Serialize for LiteralTrue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for LiteralTrue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if bool::deserialize(deserializer)? {
            Ok(Self)
        } else {
            Err(D::Error::custom("expected true"))
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiteralFalse;

impl Serialize for LiteralFalse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(false)
    }
}

impl<'de> Deserialize<'de> for LiteralFalse {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if bool::deserialize(deserializer)? {
            Err(D::Error::custom("expected false"))
        } else {
            Ok(Self)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FeedbackCategory {
    TaskResult,
    InstructionFollowing,
    ProductInteraction,
    ServiceStability,
    ResourceCost,
    SecurityPrivacyPermission,
    Other,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<FeedbackCategory>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFeedbackRecordRequest {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<FeedbackCategory>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFeedbackRecordValue {
    pub recorded: LiteralTrue,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "code",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum SessionFeedbackSessionNotFound {
    SessionNotFound { session_id: SessionId },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFeedbackRecordSuccess {
    pub ok: LiteralTrue,
    pub value: SessionFeedbackRecordValue,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFeedbackRecordFailure {
    pub ok: LiteralFalse,
    pub error: SessionFeedbackSessionNotFound,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SessionFeedbackRecordResult {
    Success(SessionFeedbackRecordSuccess),
    Failure(SessionFeedbackRecordFailure),
}

impl SessionFeedbackRecordResult {
    pub fn recorded() -> Self {
        Self::Success(SessionFeedbackRecordSuccess {
            ok: LiteralTrue,
            value: SessionFeedbackRecordValue {
                recorded: LiteralTrue,
            },
        })
    }

    pub fn session_not_found(session_id: SessionId) -> Self {
        Self::Failure(SessionFeedbackRecordFailure {
            ok: LiteralFalse,
            error: SessionFeedbackSessionNotFound::SessionNotFound { session_id },
        })
    }
}
