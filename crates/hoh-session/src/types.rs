//! Session identities, durable envelopes and append inputs.
use crate::{Result, SessionError};
pub use hoh_brand::ids::SessionId;
pub use hoh_llm::{
    AssistantMessage, DeveloperMessage, SystemMessage, ToolResultMessage, UserMessage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, OnceLock};

pub const SESSION_FORMAT_VERSION: u64 = 4;
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

macro_rules! position {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(u64);
        impl $name {
            pub fn new(value: u64) -> Result<Self> {
                if value <= MAX_SAFE_INTEGER {
                    Ok(Self(value))
                } else {
                    Err(SessionError::type_error(format!(
                        "{} must be a non-negative safe integer, got {}",
                        stringify!($name),
                        value
                    )))
                }
            }
            pub fn from_number(value: f64) -> Result<Self> {
                if value.is_finite()
                    && (0.0..=MAX_SAFE_INTEGER as f64).contains(&value)
                    && value.fract() == 0.0
                    && !(value == 0.0 && value.is_sign_negative())
                {
                    Ok(Self(value as u64))
                } else {
                    Err(SessionError::type_error(format!(
                        "{} must be a non-negative safe integer, got {}",
                        stringify!($name),
                        hoh_util_values::js_number_string(value)
                    )))
                }
            }
            pub fn get(self) -> u64 {
                self.0
            }
        }
        impl From<$name> for u64 {
            fn from(value: $name) -> u64 {
                value.0
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(
                d: D,
            ) -> std::result::Result<Self, D::Error> {
                let v = Value::deserialize(d)?;
                seq_number(&v).map(Self).ok_or_else(|| {
                    serde::de::Error::custom(concat!(
                        stringify!($name),
                        " must be a non-negative safe integer"
                    ))
                })
            }
        }
    };
}
position!(SessionSeq);
position!(SessionLogOffset);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionSeqCursor {
    #[default]
    BeforeFirst,
    Seq(SessionSeq),
}
impl From<SessionSeq> for SessionSeqCursor {
    fn from(seq: SessionSeq) -> Self {
        Self::Seq(seq)
    }
}
impl Serialize for SessionSeqCursor {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::BeforeFirst => s.serialize_i64(-1),
            Self::Seq(seq) => seq.serialize(s),
        }
    }
}
impl<'de> Deserialize<'de> for SessionSeqCursor {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let value = Value::deserialize(d)?;
        if signed_number(&value) == Some(-1) {
            return Ok(Self::BeforeFirst);
        }
        seq_number(&value)
            .map(|n| Self::Seq(SessionSeq(n)))
            .ok_or_else(|| {
                serde::de::Error::custom(
                    "SessionSeqCursor must be -1 or a non-negative safe integer",
                )
            })
    }
}
pub type OptionalSessionSeq = Option<SessionSeq>;
pub type SessionEventType = String;
pub type SurfaceEventType = String;
pub type SurfaceEvent = SessionEvent;
pub type PluginRecordType = String;
/// Extension payloads remain ordered JSON, including unknown fields.
pub type TurnEndReason = Value;
pub type EpochHeader = Value;
pub type RequestContext = Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AgentCancelCause {
    User,
    Parent,
    Hook { reason: String },
    Disposed,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TurnEndCancelCause {
    User,
    Parent,
    Hook { reason: String },
    Disposed,
    Legacy,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestHeaderReason {
    Initial,
    Resume,
    Change,
    Series,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionSeedEventState {
    Detached,
    SharedFrozen,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHeader {
    pub version: u64,
    pub id: SessionId,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<SessionId>,
    pub is_seeded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegation_depth: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_preset: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}
impl SessionHeader {
    pub fn new(id: SessionId, created_at: i64) -> Self {
        Self {
            version: SESSION_FORMAT_VERSION,
            id,
            created_at,
            cwd: None,
            parent_session: None,
            is_seeded: false,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
            extra: Default::default(),
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct CreateSessionOptions {
    pub seed: Option<Vec<Arc<SessionEvent>>>,
    pub inherited_event_count: Option<SessionLogOffset>,
    /// Creation metadata without version/id. Raw JSON retains absent/null distinctions for validation.
    pub meta: Option<Value>,
}
#[derive(Clone, Debug)]
pub struct RestoredSessionOptions {
    pub seed: Vec<Arc<SessionEvent>>,
    pub meta: Value,
    pub inherited_event_count: SessionLogOffset,
    pub event_state: SessionSeedEventState,
}
#[derive(Clone, Debug)]
pub enum PrepareSessionOptions {
    Create(CreateSessionOptions),
    Restored(RestoredSessionOptions),
}
impl Default for PrepareSessionOptions {
    fn default() -> Self {
        Self::Create(CreateSessionOptions::default())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SurfaceOp {
    Append(AppendOp),
    Replace(ReplaceOp),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppendOp {
    #[serde(rename = "append")]
    Append,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplaceOp {
    pub op: ReplaceTag,
    pub start_seq: SessionSeq,
    pub end_seq: SessionSeq,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplaceTag {
    #[serde(rename = "replace")]
    Replace,
}
impl SurfaceOp {
    pub fn append() -> Self {
        Self::Append(AppendOp::Append)
    }
    pub fn replace(start_seq: SessionSeq, end_seq: SessionSeq) -> Self {
        Self::Replace(ReplaceOp {
            op: ReplaceTag::Replace,
            start_seq,
            end_seq,
        })
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceIntent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_event_seqs: Option<Vec<SessionSeq>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_op: Option<SurfaceOp>,
}
impl SurfaceIntent {
    pub fn append() -> Self {
        Self {
            surface_op: Some(SurfaceOp::append()),
            source_event_seqs: None,
        }
    }
    pub fn replace(start: SessionSeq, end: SessionSeq, sources: Vec<SessionSeq>) -> Self {
        Self {
            surface_op: Some(SurfaceOp::replace(start, end)),
            source_event_seqs: Some(sources),
        }
    }
}
#[derive(Clone, Debug)]
pub struct SessionEventInput {
    pub r#type: String,
    pub data: Value,
    pub opts: Value,
}
impl SessionEventInput {
    pub fn new(r#type: impl Into<String>, data: Value) -> Self {
        Self {
            r#type: r#type.into(),
            data,
            opts: Value::Null,
        }
    }
    pub fn with_surface(mut self, intent: SurfaceIntent) -> Self {
        // SurfaceIntent is composed solely of JSON-compatible primitive fields.
        self.opts = serde_json::to_value(intent).expect("surface intent is JSON");
        self
    }
    pub fn with_options(mut self, opts: Value) -> Self {
        self.opts = opts;
        self
    }
}
/// Ordered, immutable-on-publication event. Keeping the envelope as JSON preserves
/// key order and every accepted extension field across snapshot and restore.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionEvent(Value, #[serde(skip)] OnceLock<Option<Arc<Value>>>);
impl PartialEq for SessionEvent {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl SessionEvent {
    /// Raw persistence-boundary value; Session creation validates before admission.
    pub fn from_value(value: Value) -> Self {
        Self(value, OnceLock::new())
    }
    pub fn as_value(&self) -> &Value {
        &self.0
    }
    pub fn event_type(&self) -> &str {
        self.0["type"].as_str().unwrap_or("")
    }
    pub fn seq(&self) -> SessionSeq {
        // Only accepted events are read by log consumers; boundary checks precede this accessor.
        SessionSeq::new(seq_number(&self.0["seq"]).expect("accepted event has a sequence"))
            .expect("accepted sequence is safe")
    }
    pub fn time(&self) -> i64 {
        // Accepted envelopes contain exact safe integer timestamps.
        signed_number(&self.0["time"]).expect("accepted event has an integer timestamp")
    }
    pub fn data(&self) -> &Value {
        &self.0["data"]
    }
    pub fn surface_op(&self) -> Option<&Value> {
        self.0.get("surfaceOp")
    }
    pub fn source_event_seqs(&self) -> Option<&Value> {
        self.0.get("sourceEventSeqs")
    }
    pub fn ignorable(&self) -> bool {
        self.0.get("ignorable") == Some(&Value::Bool(true))
    }
    pub fn message(&self) -> Option<&Value> {
        match self.event_type() {
            "user/message" => Some(self.data()),
            "system/message" | "developer/message" | "assistant/message" | "tool/result" => {
                self.data().get("message")
            }
            _ => None,
        }
    }
    pub fn llm_message(&self) -> std::result::Result<Option<hoh_llm::Message>, serde_json::Error> {
        self.message()
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()
    }
    pub(crate) fn message_snapshot(&self) -> Option<Arc<Value>> {
        self.1
            .get_or_init(|| self.message().cloned().map(Arc::new))
            .clone()
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginRecord {
    pub r#type: PluginRecordType,
    pub seq: SessionSeq,
    pub time: i64,
    pub data: Value,
}

pub(crate) fn seq_number(v: &Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        return (n <= MAX_SAFE_INTEGER).then_some(n);
    }
    let n = v.as_f64()?;
    if n.is_finite()
        && (0.0..=MAX_SAFE_INTEGER as f64).contains(&n)
        && !n.is_sign_negative()
        && n.fract() == 0.0
    {
        Some(n as u64)
    } else {
        None
    }
}
pub(crate) fn signed_number(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        return (n.unsigned_abs() <= MAX_SAFE_INTEGER).then_some(n);
    }
    let n = v.as_f64()?;
    if n.is_finite() && n.abs() <= MAX_SAFE_INTEGER as f64 && n.fract() == 0.0 {
        Some(n as i64)
    } else {
        None
    }
}
