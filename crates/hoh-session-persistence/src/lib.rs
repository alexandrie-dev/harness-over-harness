//! Port of @deepseek-ai/dsh-session-persistence (packages/session/session-persistence).
//! Provider-neutral durable session storage. Providers own the storage runtime.
pub mod errors;
pub mod handle;
pub mod revision;
pub mod storage_contract;
pub use errors::*;
pub use handle::*;
pub use hoh_session::SessionHeader;
pub use revision::SessionPersistenceRevision;
pub use storage_contract::*;

use futures_util::future::BoxFuture;
use hoh_error::BoxError;
use hoh_session::{SessionEvent, SessionId, SessionLogOffset};
use hoh_timeout::AbortSignal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A process-local identity shared by context views, distinct after replacement.
#[derive(Clone, Debug, Default)]
pub struct SessionPersistenceIdentity(Arc<()>);
impl SessionPersistenceIdentity {
    pub fn new() -> Self {
        Self::default()
    }
}
impl PartialEq for SessionPersistenceIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for SessionPersistenceIdentity {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionFormatStatus {
    Current,
    MigrationRequired,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPersistenceSnapshot {
    pub header: SessionHeader,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format_status: Option<SessionFormatStatus>,
    pub revision: SessionPersistenceRevision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Arc<SessionLocation>>,
}
#[derive(Clone, Default)]
pub struct SessionPersistenceCreateOptions {
    pub signal: Option<AbortSignal>,
    pub inherited_event_count: Option<SessionLogOffset>,
}
#[derive(Clone, Default)]
pub struct SessionPersistenceOpenOptions {
    pub signal: Option<AbortSignal>,
}
#[derive(Clone, Default)]
pub struct SessionPersistenceStatOptions {
    pub signal: Option<AbortSignal>,
}
#[derive(Clone, Default)]
pub struct SessionPersistenceListOptions {
    pub signal: Option<AbortSignal>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStorageMetadata {
    pub meta: SessionHeader,
    pub inherited_event_count: SessionLogOffset,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInspection {
    pub meta: SessionHeader,
    pub inherited_event_count: SessionLogOffset,
    pub events: Vec<Arc<SessionEvent>>,
}

/// Durable append-only storage. Append acknowledges ordered visibility; flush
/// is the durability barrier. Create/open(write) claim single-writer ownership.
pub trait SessionPersistence: Send + Sync + 'static {
    fn identity(&self) -> SessionPersistenceIdentity;
    fn create<'a>(
        &'a self,
        header: &'a SessionHeader,
        options: Option<&'a SessionPersistenceCreateOptions>,
    ) -> BoxFuture<'a, Result<Arc<dyn SessionHandle>, BoxError>>;
    fn open<'a>(
        &'a self,
        id: &'a SessionId,
        access: SessionAccess,
        options: Option<&'a SessionPersistenceOpenOptions>,
    ) -> BoxFuture<'a, Result<Arc<dyn SessionHandle>, BoxError>>;
    /// Move a session's complete durable storage to the backend's recoverable trash.
    /// Backends without trash support must reject the operation.
    fn trash<'a>(&'a self, _id: &'a SessionId) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async {
            Err(std::io::Error::other("session trash is unsupported by this backend").into())
        })
    }
    /// Flush every active writer; aggregate failures without abandoning the sweep.
    fn flush(&self) -> BoxFuture<'_, Result<(), BoxError>>;
    fn stat<'a>(
        &'a self,
        id: &'a SessionId,
        options: Option<&'a SessionPersistenceStatOptions>,
    ) -> BoxFuture<'a, Result<Option<SessionPersistenceSnapshot>, BoxError>>;
    fn list<'a>(
        &'a self,
        options: Option<&'a SessionPersistenceListOptions>,
    ) -> BoxFuture<'a, Result<Vec<SessionPersistenceSnapshot>, BoxError>>;
}
hoh_kernel::service!(pub SessionPersistenceService: dyn SessionPersistence = "sessionPersistence");
