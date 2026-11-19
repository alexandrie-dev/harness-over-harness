use hoh_error::{BoxError, HarnessError};

pub const WORKSPACE_DEPENDENCY_ERROR_CODES: [&str; 3] = [
    "WORKSPACE_DEPENDENCY_INVALID_PAYLOAD",
    "WORKSPACE_DEPENDENCY_UNSUPPORTED_PLATFORM",
    "WORKSPACE_DEPENDENCY_PREPARATION_FAILED",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceDependencyErrorCode {
    InvalidPayload,
    UnsupportedPlatform,
    PreparationFailed,
}

impl WorkspaceDependencyErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidPayload => "WORKSPACE_DEPENDENCY_INVALID_PAYLOAD",
            Self::UnsupportedPlatform => "WORKSPACE_DEPENDENCY_UNSUPPORTED_PLATFORM",
            Self::PreparationFailed => "WORKSPACE_DEPENDENCY_PREPARATION_FAILED",
        }
    }
}

pub struct WorkspaceDependencyError;

impl WorkspaceDependencyError {
    pub const NAME: &'static str = "WorkspaceDependencyError";

    #[allow(clippy::new_ret_no_self)]
    pub fn new(message: impl Into<String>, code: WorkspaceDependencyErrorCode) -> HarnessError {
        HarnessError::named(Self::NAME, message, code.as_str())
    }

    pub(crate) fn with_cause(
        message: impl Into<String>,
        code: WorkspaceDependencyErrorCode,
        cause: impl Into<BoxError>,
    ) -> HarnessError {
        Self::new(message, code).with_cause(cause)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceDependenciesError {
    #[error("workspace dependencies: source and root must be absolute paths")]
    RelativeConfig,
}
