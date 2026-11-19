//! One storage channel, with explicit access and uncancellable idempotent close.
use futures_util::future::BoxFuture;
use hoh_error::BoxError;
use hoh_session::{
    SessionEvent, SessionHeader, SessionId, SessionLogOffset, SessionSeedEventState,
};
use hoh_timeout::AbortSignal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionAccess {
    Read,
    Write,
}
#[derive(Clone, Default)]
pub struct SessionHandleReadOptions {
    pub signal: Option<AbortSignal>,
}
#[derive(Clone, Default)]
pub struct SessionHandleAppendOptions {
    pub signal: Option<AbortSignal>,
}
#[derive(Clone, Default)]
pub struct SessionHandleFlushOptions {
    pub signal: Option<AbortSignal>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHandleReadResult {
    pub event_state: SessionSeedEventState,
    pub events: Vec<Arc<SessionEvent>>,
}

pub trait SessionHandle: Send + Sync + 'static {
    fn id(&self) -> &SessionId;
    fn header(&self) -> &SessionHeader;
    fn inherited_event_count(&self) -> SessionLogOffset;
    fn access(&self) -> SessionAccess;
    /// Defaults: offset 0, length to the end. Reads never backtrack in freshness.
    /// Returned outer arrays belong to the caller; event graphs are immutable.
    fn read<'a>(
        &'a self,
        offset: Option<u64>,
        length: Option<u64>,
        options: Option<&'a SessionHandleReadOptions>,
    ) -> BoxFuture<'a, Result<SessionHandleReadResult, BoxError>>;
    fn append<'a>(
        &'a self,
        events: &'a [Arc<SessionEvent>],
        options: Option<&'a SessionHandleAppendOptions>,
    ) -> BoxFuture<'a, Result<(), BoxError>>;
    fn flush<'a>(
        &'a self,
        options: Option<&'a SessionHandleFlushOptions>,
    ) -> BoxFuture<'a, Result<(), BoxError>>;
    /// Explicit async disposal: drains a writer before releasing ownership.
    fn close(&self) -> BoxFuture<'_, Result<(), BoxError>>;
}
