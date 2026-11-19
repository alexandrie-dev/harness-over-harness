//! Machine-routable terminal failures and ordered aggregate errors.

use hoh_error::{BoxError, HarnessError, SharedError};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalErrorCode {
    CleanupFailed,
    DuplicateBackend,
    DuplicateName,
    ForeignSession,
    InitRejected,
    NoBackend,
    NoSandbox,
    NoSession,
    OwnerNotLive,
    SendActive,
    ServiceDisposing,
    SessionClosing,
    SessionExited,
    StartupExited,
    StartupTimeout,
}

impl TerminalErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CleanupFailed => "CLEANUP_FAILED",
            Self::DuplicateBackend => "DUPLICATE_BACKEND",
            Self::DuplicateName => "DUPLICATE_NAME",
            Self::ForeignSession => "FOREIGN_SESSION",
            Self::InitRejected => "INIT_REJECTED",
            Self::NoBackend => "NO_BACKEND",
            Self::NoSandbox => "NO_SANDBOX",
            Self::NoSession => "NO_SESSION",
            Self::OwnerNotLive => "OWNER_NOT_LIVE",
            Self::SendActive => "SEND_ACTIVE",
            Self::ServiceDisposing => "SERVICE_DISPOSING",
            Self::SessionClosing => "SESSION_CLOSING",
            Self::SessionExited => "SESSION_EXITED",
            Self::StartupExited => "STARTUP_EXITED",
            Self::StartupTimeout => "STARTUP_TIMEOUT",
        }
    }
}

pub struct TerminalError;

impl TerminalError {
    pub const NAME: &'static str = "TerminalError";

    #[allow(clippy::new_ret_no_self)]
    pub fn new(message: impl Into<String>, code: TerminalErrorCode) -> HarnessError {
        HarnessError::named(Self::NAME, message, code.as_str())
    }
}

#[derive(Clone, Debug)]
pub struct AggregateError {
    pub message: String,
    pub errors: Vec<SharedError>,
}

impl AggregateError {
    pub fn new(message: impl Into<String>, errors: Vec<SharedError>) -> Self {
        Self {
            message: message.into(),
            errors,
        }
    }
}

impl fmt::Display for AggregateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AggregateError {}

#[derive(Clone, Debug)]
pub struct TerminalBackendCleanupDetail {
    pub spawn_error: SharedError,
    pub cleanup_error: SharedError,
    pub errors: Vec<SharedError>,
}

pub struct TerminalBackendCleanupError;

impl TerminalBackendCleanupError {
    pub const NAME: &'static str = "TerminalBackendCleanupError";

    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        spawn_error: impl Into<BoxError>,
        cleanup_error: impl Into<BoxError>,
    ) -> HarnessError {
        let spawn_error = SharedError::new(spawn_error);
        let cleanup_error = SharedError::new(cleanup_error);
        let errors = vec![spawn_error.clone(), cleanup_error.clone()];
        HarnessError::named(
            Self::NAME,
            "PTY backend startup and cleanup both failed",
            TerminalErrorCode::CleanupFailed.as_str(),
        )
        .with_cause(AggregateError::new(
            "PTY backend startup and cleanup both failed",
            errors.clone(),
        ))
        .with_detail(TerminalBackendCleanupDetail {
            spawn_error,
            cleanup_error,
            errors,
        })
    }

    pub fn detail<'a>(
        error: &'a (dyn std::error::Error + 'static),
    ) -> Option<&'a TerminalBackendCleanupDetail> {
        let error = HarnessError::of(error)?;
        (error.name() == Self::NAME).then(|| error.detail())?
    }
}
