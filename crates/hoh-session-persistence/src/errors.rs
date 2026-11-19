//! Plain Error subclasses from the persistence seam, never HarnessError.
use hoh_error::BoxError;
use hoh_session::{SESSION_FORMAT_VERSION, SessionId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

macro_rules! identity_error {
    ($name:ident, $tail:literal) => {
        #[derive(Debug, thiserror::Error)]
        #[error("{message}")]
        pub struct $name {
            pub session_id: SessionId,
            message: String,
        }
        impl $name {
            pub fn new(session_id: SessionId) -> Self {
                let message = format!(concat!("session \"{}\"", $tail), session_id);
                Self {
                    session_id,
                    message,
                }
            }
            pub fn name(&self) -> &'static str {
                stringify!($name)
            }
        }
    };
}
identity_error!(SessionPersistenceNotFoundError, " not found");
identity_error!(SessionAlreadyExistsError, " already exists");
identity_error!(
    SessionAlreadyOwnedError,
    " is already owned by an active write handle"
);
identity_error!(
    SessionOwnershipLostError,
    ": write ownership was lost; close this handle and reopen"
);

#[derive(Debug, thiserror::Error)]
#[error("session \"{session_id}\": {operation} is not available on a read handle")]
pub struct SessionReadOnlyError {
    pub session_id: SessionId,
    operation: String,
}
impl SessionReadOnlyError {
    pub fn new(session_id: SessionId, operation: impl Into<String>) -> Self {
        Self {
            session_id,
            operation: operation.into(),
        }
    }
    pub fn name(&self) -> &'static str {
        "SessionReadOnlyError"
    }
}
#[derive(Debug, thiserror::Error)]
#[error("session \"{session_id}\": {operation} on a closed handle")]
pub struct SessionHandleClosedError {
    pub session_id: SessionId,
    operation: String,
}
impl SessionHandleClosedError {
    pub fn new(session_id: SessionId, operation: impl Into<String>) -> Self {
        Self {
            session_id,
            operation: operation.into(),
        }
    }
    pub fn name(&self) -> &'static str {
        "SessionHandleClosedError"
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLocation {
    pub kind: String,
    pub path: String,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct SessionPersistenceCorruptionError {
    message: String,
    #[source]
    cause: BoxError,
}
impl SessionPersistenceCorruptionError {
    pub fn new(message: impl Into<String>, cause: BoxError) -> Self {
        Self {
            message: message.into(),
            cause,
        }
    }
    pub fn name(&self) -> &'static str {
        "SessionPersistenceCorruptionError"
    }
}
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct SessionFormatUnsupportedError {
    message: String,
    pub location: Option<Arc<SessionLocation>>,
}
impl SessionFormatUnsupportedError {
    pub fn new(message: impl Into<String>, location: Option<Arc<SessionLocation>>) -> Self {
        Self {
            message: message.into(),
            location,
        }
    }
    pub fn name(&self) -> &'static str {
        "SessionFormatUnsupportedError"
    }
}
pub fn session_format_version_refusal(id: &str, version: u64) -> String {
    let newer = version > SESSION_FORMAT_VERSION;
    let version = hoh_util_values::js_number_string(version as f64);
    if newer {
        format!(
            "session \"{id}\" uses log format v{version}, but this harness reads only v{SESSION_FORMAT_VERSION}: the log was written by a newer harness — upgrade the harness to open it"
        )
    } else {
        format!(
            "session \"{id}\" uses log format v{version}, older than the supported v{SESSION_FORMAT_VERSION}, and this build ships no upgrade path for it"
        )
    }
}
