//! Machine-routable shell environment failures.

use hoh_error::HarnessError;

/// Every [`ShellEnvError`] code:
/// - `SHELL_ENV_CONTRIBUTION_INVALID` — a registered contributor returned an undeclared key or a
///   non-string value; the shell command does not run, and the contributing plugin must be fixed.
pub const SHELL_ENV_ERROR_CODES: [&str; 1] = ["SHELL_ENV_CONTRIBUTION_INVALID"];

/// One code from [`SHELL_ENV_ERROR_CODES`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellEnvErrorCode {
    ShellEnvContributionInvalid,
}

impl ShellEnvErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ShellEnvContributionInvalid => SHELL_ENV_ERROR_CODES[0],
        }
    }
}

/// Shell environment failure carrying a stable [`ShellEnvErrorCode`] (a `HarnessError` named
/// `ShellEnvError`).
pub struct ShellEnvError;

impl ShellEnvError {
    pub const NAME: &'static str = "ShellEnvError";

    /// `message` names the contributor and the offending key; `code` is the failure class
    /// consumers route on.
    #[allow(clippy::new_ret_no_self)] // Subclass constructor of PORTING.md §7.
    pub fn new(message: impl Into<String>, code: ShellEnvErrorCode) -> HarnessError {
        HarnessError::named(Self::NAME, message, code.as_str())
    }
}
