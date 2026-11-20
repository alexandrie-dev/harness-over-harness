use hoh_error::{BoxError, HarnessError};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionQueryErrorCode {
    Aborted,
    CorruptSession,
    EventNotFound,
    IndexFailed,
    InvalidConfig,
    InvalidCursor,
    InvalidFilter,
    InvalidLimit,
    InvalidQuery,
    InvalidLineage,
    InvalidSurface,
    InvalidWindow,
    PersistenceFailed,
    SearchDisabled,
    SessionNotFound,
    StaleCursor,
    SourceConflict,
}

impl SessionQueryErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Aborted => "SESSION_QUERY_ABORTED",
            Self::CorruptSession => "SESSION_QUERY_CORRUPT_SESSION",
            Self::EventNotFound => "SESSION_QUERY_EVENT_NOT_FOUND",
            Self::IndexFailed => "SESSION_QUERY_INDEX_FAILED",
            Self::InvalidConfig => "SESSION_QUERY_INVALID_CONFIG",
            Self::InvalidCursor => "SESSION_QUERY_INVALID_CURSOR",
            Self::InvalidFilter => "SESSION_QUERY_INVALID_FILTER",
            Self::InvalidLimit => "SESSION_QUERY_INVALID_LIMIT",
            Self::InvalidQuery => "SESSION_QUERY_INVALID_QUERY",
            Self::InvalidLineage => "SESSION_QUERY_INVALID_LINEAGE",
            Self::InvalidSurface => "SESSION_QUERY_INVALID_SURFACE",
            Self::InvalidWindow => "SESSION_QUERY_INVALID_WINDOW",
            Self::PersistenceFailed => "SESSION_QUERY_PERSISTENCE_FAILED",
            Self::SearchDisabled => "SESSION_QUERY_SEARCH_DISABLED",
            Self::SessionNotFound => "SESSION_QUERY_SESSION_NOT_FOUND",
            Self::StaleCursor => "SESSION_QUERY_STALE_CURSOR",
            Self::SourceConflict => "SESSION_QUERY_SOURCE_CONFLICT",
        }
    }
}

pub struct SessionQueryError;

impl SessionQueryError {
    pub const NAME: &'static str = "SessionQueryError";

    #[allow(clippy::new_ret_no_self)] // HarnessError subclasses share one concrete Rust type.
    pub fn new(message: impl Into<String>, code: SessionQueryErrorCode) -> HarnessError {
        HarnessError::named(Self::NAME, message, code.as_str())
    }

    pub fn with_cause(
        message: impl Into<String>,
        code: SessionQueryErrorCode,
        cause: impl Into<BoxError>,
    ) -> HarnessError {
        HarnessError::named(Self::NAME, message, code.as_str()).with_cause(cause)
    }

    pub(crate) fn boxed(message: impl Into<String>, code: SessionQueryErrorCode) -> BoxError {
        Box::new(Self::new(message, code))
    }
}

pub(crate) type Result<T> = std::result::Result<T, BoxError>;
