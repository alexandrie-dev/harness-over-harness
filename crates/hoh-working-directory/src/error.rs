//! Machine-routable Session working-directory failures.

use hoh_error::HarnessError;

/// Every `WorkingDirectoryError` code:
/// - `WORKING_DIRECTORY_INVALID` — a requested directory is empty or otherwise malformed.
/// - `WORKING_DIRECTORY_NOT_FOUND` — the requested directory does not exist or is not a directory; the caller picks another path.
/// - `WORKING_DIRECTORY_UNAVAILABLE` — neither the current directory nor the original project exists, or the owning service was disposed.
pub const WORKING_DIRECTORY_ERROR_CODES: [&str; 4] = [
    "WORKING_DIRECTORY_INVALID",
    "WORKING_DIRECTORY_NOT_FOUND",
    "WORKING_DIRECTORY_UNAVAILABLE",
    "WORKING_DIRECTORY_CHANGED",
];

/// One code from [`WORKING_DIRECTORY_ERROR_CODES`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkingDirectoryErrorCode {
    Invalid,
    NotFound,
    Unavailable,
    Changed,
}

impl WorkingDirectoryErrorCode {
    /// The stable code string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => WORKING_DIRECTORY_ERROR_CODES[0],
            Self::NotFound => WORKING_DIRECTORY_ERROR_CODES[1],
            Self::Unavailable => WORKING_DIRECTORY_ERROR_CODES[2],
            Self::Changed => WORKING_DIRECTORY_ERROR_CODES[3],
        }
    }
}

/// Working-directory failure carrying a stable [`WorkingDirectoryErrorCode`].
pub struct WorkingDirectoryError;

impl WorkingDirectoryError {
    /// The `name` of every error built by [`WorkingDirectoryError::new`].
    pub const NAME: &'static str = "WorkingDirectoryError";

    /// `message` names the directory and the safe next step; `code` is the failure class consumers route on.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(message: impl Into<String>, code: WorkingDirectoryErrorCode) -> HarnessError {
        HarnessError::named(Self::NAME, message, code.as_str())
    }
}
