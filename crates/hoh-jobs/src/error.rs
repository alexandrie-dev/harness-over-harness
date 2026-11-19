//! Machine-routable background-job failures.

use hoh_error::HarnessError;

/// One stable background-job failure code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JobErrorCode {
    ControllerUnavailable,
    NotFound,
    ForeignSession,
    OwnerNotLive,
    OwnerRegistryUnavailable,
    LimitReached,
}

impl JobErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ControllerUnavailable => "JOB_CONTROLLER_UNAVAILABLE",
            Self::NotFound => "JOB_NOT_FOUND",
            Self::ForeignSession => "JOB_FOREIGN_SESSION",
            Self::OwnerNotLive => "JOB_OWNER_NOT_LIVE",
            Self::OwnerRegistryUnavailable => "JOB_OWNER_REGISTRY_UNAVAILABLE",
            Self::LimitReached => "JOB_LIMIT_REACHED",
        }
    }
}

/// Every [`JobError`] code, in source order.
pub const JOB_ERROR_CODES: [JobErrorCode; 6] = [
    JobErrorCode::ControllerUnavailable,
    JobErrorCode::NotFound,
    JobErrorCode::ForeignSession,
    JobErrorCode::OwnerNotLive,
    JobErrorCode::OwnerRegistryUnavailable,
    JobErrorCode::LimitReached,
];

/// Background-job failure carrying a stable [`JobErrorCode`].
pub struct JobError;

impl JobError {
    pub const NAME: &'static str = "JobError";

    #[allow(clippy::new_ret_no_self)] // HarnessError subclasses share one concrete Rust type.
    pub fn new(message: impl Into<String>, code: JobErrorCode) -> HarnessError {
        HarnessError::named(Self::NAME, message, code.as_str())
    }
}
