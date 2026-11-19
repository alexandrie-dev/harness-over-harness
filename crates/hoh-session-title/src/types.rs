//! Pure session-title domain types.

use hoh_brand::ids::SessionTitleProviderId;
use hoh_session::SessionSeq;
use serde::{Deserialize, Serialize};

pub use hoh_brand::ids::SessionTitleProviderId as ProviderId;

/// Exact auxiliary model route that produced a title.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTitleModelIdentity {
    pub provider: String,
    pub model: String,
}

/// Durable ownership record for an accepted session title.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SessionTitleSource {
    Fallback,
    Provider {
        provider: SessionTitleProviderId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<SessionTitleModelIdentity>,
    },
    User,
}

/// Payload of the log-only `session/title` event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTitleEventData {
    pub title: String,
    pub message_seqs: Vec<SessionSeq>,
    pub source: SessionTitleSource,
}

/// Latest folded title plus durable envelope facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTitleSnapshot {
    pub title: String,
    pub message_seqs: Vec<SessionSeq>,
    pub source: SessionTitleSource,
    pub event_seq: SessionSeq,
    pub updated_at: i64,
}

pub type TitleProjection = SessionTitleSnapshot;

/// One eligible human text message exposed to title providers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionTitleUserMessage {
    pub seq: SessionSeq,
    pub text: String,
}

/// Bounded aggregate of eligible title input.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TitleInputState {
    pub first: Option<SessionTitleUserMessage>,
    pub count: u64,
    pub last_seq: Option<SessionSeq>,
}
