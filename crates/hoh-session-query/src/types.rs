use hoh_error::BoxError;
use hoh_session::{SessionEvent, SessionHeader, SessionId, SessionLogOffset, SessionSeq};
use hoh_session_persistence::SessionFormatStatus;
use hoh_session_title::SessionTitleSnapshot;
use hoh_timeout::AbortSignal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionSearchCursor(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionEventSurface {
    Current,
    Shadowed,
    LogOnly,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub header: SessionHeader,
    pub live: bool,
    pub persisted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format_status: Option<SessionFormatStatus>,
}

pub trait SessionRecordLike {
    fn session_record(&self) -> &SessionRecord;
}

impl SessionRecordLike for SessionRecord {
    fn session_record(&self) -> &SessionRecord {
        self
    }
}

#[derive(Clone, Debug)]
pub struct SessionSurfaceSnapshot {
    pub session: SessionHeader,
    pub inherited_event_count: SessionLogOffset,
    pub captured_through_seq: Option<SessionSeq>,
    pub events: Vec<Arc<SessionEvent>>,
}

#[derive(Clone, Debug)]
pub struct SessionLogSnapshot {
    pub session: SessionHeader,
    pub inherited_event_count: SessionLogOffset,
    pub events: Vec<Arc<SessionEvent>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEventRecord {
    pub session_id: SessionId,
    pub seq: SessionSeq,
    pub r#type: String,
    pub time: i64,
    pub surface: SessionEventSurface,
}

#[derive(Clone, Debug)]
pub struct SessionLineageNode {
    pub session: SessionRecord,
    pub descendants: Vec<SessionLineageNode>,
}

impl Drop for SessionLineageNode {
    fn drop(&mut self) {
        let mut pending = std::mem::take(&mut self.descendants);
        while let Some(mut node) = pending.pop() {
            pending.append(&mut node.descendants);
        }
    }
}

#[derive(Clone, Debug)]
pub enum SessionLineageCompletion {
    Complete { root: Box<SessionRecord> },
    Incomplete { unresolved_parent_id: SessionId },
}

#[derive(Clone, Debug)]
pub struct SessionLineageTrace {
    pub target: SessionRecord,
    pub ancestors: Vec<SessionRecord>,
    pub descendants: Vec<SessionLineageNode>,
    pub completion: SessionLineageCompletion,
}

#[derive(Clone, Debug)]
pub struct SessionEventTraceRequest {
    pub session_id: SessionId,
    pub seq: SessionSeq,
}

#[derive(Clone, Debug)]
pub struct SessionEventTrace {
    pub target: SessionEventRecord,
    pub replaced_by: Option<SessionSeq>,
    pub replacement_chain: Vec<SessionSeq>,
    pub replaced_event_seqs: Vec<SessionSeq>,
    pub source_event_seqs: Vec<SessionSeq>,
    pub derived_event_seqs: Vec<SessionSeq>,
}

#[derive(Clone, Debug)]
pub struct SessionEventTraceObservation {
    pub session: SessionHeader,
    pub trace: SessionEventTrace,
}

#[derive(Clone, Debug)]
pub struct SessionEventReadRequest {
    pub session_id: SessionId,
    pub seq: SessionSeq,
    pub before: Option<u64>,
    pub after: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct SessionEventWindow {
    pub session: SessionHeader,
    pub inherited_event_count: SessionLogOffset,
    pub target: Arc<SessionEvent>,
    pub events: Vec<Arc<SessionEvent>>,
    pub start_seq: SessionSeq,
    pub end_seq: SessionSeq,
}

#[derive(Clone, Debug)]
pub struct SessionTitleObservation {
    pub session: SessionHeader,
    pub title: Option<SessionTitleSnapshot>,
}

#[derive(Debug)]
pub enum SessionTitleObservationResult {
    Fulfilled {
        session_id: SessionId,
        value: Box<SessionTitleObservation>,
    },
    Rejected {
        session_id: SessionId,
        reason: BoxError,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SessionResultRange {
    pub from: Option<f64>,
    pub to: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionAvailability {
    Live,
    Persisted,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionResultFilter {
    Id(Vec<SessionId>),
    Cwd(Vec<Option<String>>),
    CreatedAt(SessionResultRange),
    Parent(Vec<Option<SessionId>>),
    Availability(Vec<SessionAvailability>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionEventResultFilter {
    Seq(SessionResultRange),
    Time(SessionResultRange),
    Type(Vec<String>),
    Surface(Vec<SessionEventSurface>),
    Text(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionEventMetadataFilter {
    Seq(SessionResultRange),
    Time(SessionResultRange),
    Type(Vec<String>),
    Surface(Vec<SessionEventSurface>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEventSearchDocument {
    #[serde(flatten)]
    pub record: SessionEventRecord,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct SessionSearchPage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<SessionSearchCursor>,
}

#[derive(Clone, Debug)]
pub struct SessionEventSearchPage {
    pub session: SessionHeader,
    pub items: Vec<SessionEventSearchHit>,
    pub next_cursor: Option<SessionSearchCursor>,
}

#[derive(Clone, Debug, Default)]
pub struct SessionSearchExecContext {
    pub signal: Option<AbortSignal>,
}

#[derive(Clone, Debug)]
pub struct SessionSearchRequest {
    pub query: String,
    pub session_filters: Vec<SessionResultFilter>,
    pub event_filters: Vec<SessionEventMetadataFilter>,
    pub limit: Option<u64>,
    pub cursor: Option<SessionSearchCursor>,
}

#[derive(Clone, Debug)]
pub struct SessionEventSearchRequest {
    pub session_id: SessionId,
    pub query: String,
    pub filters: Vec<SessionEventMetadataFilter>,
    pub limit: Option<u64>,
    pub cursor: Option<SessionSearchCursor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEventSearchHit {
    #[serde(flatten)]
    pub record: SessionEventRecord,
    pub snippet: String,
}

#[derive(Clone, Debug)]
pub struct SessionSearchHit {
    pub session: SessionRecord,
    pub best_match: SessionEventSearchHit,
}

impl SessionRecordLike for SessionSearchHit {
    fn session_record(&self) -> &SessionRecord {
        &self.session
    }
}

#[derive(Clone, Debug, schemars::JsonSchema, serde::Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub read_window_max: u64,
    pub persisted_read_concurrency: usize,
    pub prepared_session_cache_size: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            read_window_max: crate::SESSION_QUERY_READ_WINDOW_MAX,
            persisted_read_concurrency: crate::SESSION_QUERY_DEFAULT_PERSISTED_INSPECT_CONCURRENCY,
            prepared_session_cache_size: crate::SESSION_QUERY_DEFAULT_PREPARED_SESSION_CACHE_SIZE,
        }
    }
}
