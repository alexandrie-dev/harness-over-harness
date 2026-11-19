//! Public request, value, and failure vocabulary for per-message feedback.

use hoh_llm::MessageId;
use hoh_session::SessionId;
use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use serde_json::Value;

use crate::FeedbackCategory;

hoh_brand::branded_string!(pub MessageFeedbackVersion);

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
#[serde(rename_all = "lowercase")]
pub enum MessageFeedbackRating {
    Positive,
    Negative,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageFeedbackItem {
    pub message_id: MessageId,
    pub rating: MessageFeedbackRating,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_option"
    )]
    pub note: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_option"
    )]
    pub category: Option<FeedbackCategory>,
    pub version: MessageFeedbackVersion,
    #[serde(deserialize_with = "deserialize_js_timestamp")]
    pub created_at: u64,
    #[serde(deserialize_with = "deserialize_js_timestamp")]
    pub updated_at: u64,
    #[serde(flatten, default)]
    pub extensions: Box<IndexMap<String, Value>>,
}

fn deserialize_present_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn deserialize_js_timestamp<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

    let value = Value::deserialize(deserializer)?;
    let Some(number) = value.as_f64() else {
        return Err(D::Error::custom("expected a non-negative safe integer"));
    };
    if !number.is_finite() || number < 0.0 || number.fract() != 0.0 || number > MAX_SAFE_INTEGER {
        return Err(D::Error::custom("expected a non-negative safe integer"));
    }
    Ok(number as u64)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageFeedbackPut {
    pub session_id: SessionId,
    pub item: MessageFeedbackItem,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageFeedbackDelete {
    pub session_id: SessionId,
    pub message_id: MessageId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageFeedbackListRequest {
    pub session_id: SessionId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageFeedbackListValue {
    pub items: Vec<MessageFeedbackItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageFeedbackPutRequest {
    pub session_id: SessionId,
    pub message_id: MessageId,
    pub rating: MessageFeedbackRating,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<FeedbackCategory>,
    pub if_version: Option<MessageFeedbackVersion>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageFeedbackDeleteRequest {
    pub session_id: SessionId,
    pub message_id: MessageId,
    pub if_version: MessageFeedbackVersion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageFeedbackDeleteValue {
    pub absent: LiteralTrue,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "code",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum MessageFeedbackListFailure {
    SessionNotFound { session_id: SessionId },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "code",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum MessageFeedbackPutFailure {
    SessionNotFound {
        session_id: SessionId,
    },
    TargetNotFound {
        session_id: SessionId,
        message_id: MessageId,
    },
    VersionConflict {
        current: Option<MessageFeedbackItem>,
    },
    NoteBlank,
    NoteTooLarge {
        max_bytes: usize,
        actual_bytes: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "code",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum MessageFeedbackDeleteFailure {
    SessionNotFound {
        session_id: SessionId,
    },
    VersionConflict {
        current: Option<MessageFeedbackItem>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "code",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum MessageFeedbackFailure {
    SessionNotFound {
        session_id: SessionId,
    },
    TargetNotFound {
        session_id: SessionId,
        message_id: MessageId,
    },
    VersionConflict {
        current: Option<MessageFeedbackItem>,
    },
    NoteBlank,
    NoteTooLarge {
        max_bytes: usize,
        actual_bytes: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageFeedbackSuccess<T> {
    pub ok: LiteralTrue,
    pub value: T,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageFeedbackRejected<E> {
    pub ok: LiteralFalse,
    pub error: E,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageFeedbackResult<T, E> {
    Success(MessageFeedbackSuccess<T>),
    Rejected(MessageFeedbackRejected<E>),
}

impl<T, E> MessageFeedbackResult<T, E> {
    pub fn success(value: T) -> Self {
        Self::Success(MessageFeedbackSuccess {
            ok: LiteralTrue,
            value,
        })
    }

    pub fn rejected(error: E) -> Self {
        Self::Rejected(MessageFeedbackRejected {
            ok: LiteralFalse,
            error,
        })
    }
}

pub type MessageFeedbackListResult =
    MessageFeedbackResult<MessageFeedbackListValue, MessageFeedbackListFailure>;
pub type MessageFeedbackPutResult =
    MessageFeedbackResult<MessageFeedbackItem, MessageFeedbackPutFailure>;
pub type MessageFeedbackDeleteResult =
    MessageFeedbackResult<MessageFeedbackDeleteValue, MessageFeedbackDeleteFailure>;
