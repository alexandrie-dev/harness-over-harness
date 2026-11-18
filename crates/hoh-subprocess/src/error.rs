//! Harness subclasses and classification of foreign spawn failures.

use hoh_error::{HarnessError, SharedError, node_error_code, node_spawn_error_message};
use hoh_util_values::js_json_stringify;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{error::Error, fmt, io, sync::Arc};

pub const SUBPROCESS_ERROR_CODES: [&str; 7] = [
    "SUBPROCESS_INVALID_ARGV",
    "SUBPROCESS_EXECUTABLE_NOT_FOUND",
    "SUBPROCESS_SPAWN_NOT_FOUND",
    "SUBPROCESS_SPAWN_FAILED",
    "SUBPROCESS_RUNNER_FAILED",
    "SUBPROCESS_RUNNER_IPC_FAILED",
    "SUBPROCESS_PTY_UNAVAILABLE",
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SubprocessErrorCode {
    #[serde(rename = "SUBPROCESS_INVALID_ARGV")]
    InvalidArgv,
    #[serde(rename = "SUBPROCESS_EXECUTABLE_NOT_FOUND")]
    ExecutableNotFound,
    #[serde(rename = "SUBPROCESS_SPAWN_NOT_FOUND")]
    SpawnNotFound,
    #[serde(rename = "SUBPROCESS_SPAWN_FAILED")]
    SpawnFailed,
    #[serde(rename = "SUBPROCESS_RUNNER_FAILED")]
    RunnerFailed,
    #[serde(rename = "SUBPROCESS_RUNNER_IPC_FAILED")]
    RunnerIpcFailed,
    #[serde(rename = "SUBPROCESS_PTY_UNAVAILABLE")]
    PtyUnavailable,
}

impl SubprocessErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgv => SUBPROCESS_ERROR_CODES[0],
            Self::ExecutableNotFound => SUBPROCESS_ERROR_CODES[1],
            Self::SpawnNotFound => SUBPROCESS_ERROR_CODES[2],
            Self::SpawnFailed => SUBPROCESS_ERROR_CODES[3],
            Self::RunnerFailed => SUBPROCESS_ERROR_CODES[4],
            Self::RunnerIpcFailed => SUBPROCESS_ERROR_CODES[5],
            Self::PtyUnavailable => SUBPROCESS_ERROR_CODES[6],
        }
    }
}

pub struct SubprocessError;
impl SubprocessError {
    pub const NAME: &'static str = "SubprocessError";

    #[allow(clippy::new_ret_no_self)] // Harness subclass constructor required by PORTING.md §7.
    pub fn new(message: impl Into<String>, code: SubprocessErrorCode) -> HarnessError {
        HarnessError::named(Self::NAME, message, code.as_str())
    }

    /// Includes the source's executable-not-found subclass, without classifying by code alone.
    pub fn is(error: &(dyn Error + 'static)) -> bool {
        HarnessError::of(error).is_some_and(|error| {
            matches!(
                error.name(),
                Self::NAME | SubprocessExecutableNotFoundError::NAME
            )
        })
    }
}

pub struct SubprocessExecutableNotFoundError;
impl SubprocessExecutableNotFoundError {
    pub const NAME: &'static str = "SubprocessExecutableNotFoundError";

    #[allow(clippy::new_ret_no_self)] // Harness subclass constructor required by PORTING.md §7.
    pub fn new(message: impl Into<String>) -> HarnessError {
        HarnessError::named(
            Self::NAME,
            message,
            SubprocessErrorCode::ExecutableNotFound.as_str(),
        )
    }
}

/// An ordinary Error carrying a JavaScript-shaped errno property. This is not a HarnessError.
#[derive(Debug, thiserror::Error)]
pub enum SubprocessForeignError {
    #[error("{message}")]
    Coded { message: String, code: Value },
}

/// Provider rejection for a native start failure, retaining the actual spawned file's diagnostic.
/// This is an ordinary platform error, not a HarnessError; consumers classify it separately.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct SubprocessSpawnRejection {
    message: String,
    code: Option<&'static str>,
    #[source]
    cause: io::Error,
}

impl SubprocessSpawnRejection {
    /// Build the Node-shaped message once from the file passed to Command::new, not a display name.
    pub fn new(error: io::Error, spawned_file: &str) -> Self {
        Self {
            message: node_spawn_error_message(&error, spawned_file),
            code: node_error_code(&error),
            cause: error,
        }
    }

    pub fn code(&self) -> Option<&'static str> {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Preserve the distinction between a thrown Error and a non-Error value, including identity.
#[derive(Clone, Debug)]
pub enum SubprocessFailure {
    Error(SharedError),
    Value(Arc<Value>),
    Undefined,
}

impl From<SharedError> for SubprocessFailure {
    fn from(error: SharedError) -> Self {
        Self::Error(error)
    }
}
impl From<hoh_error::BoxError> for SubprocessFailure {
    fn from(error: hoh_error::BoxError) -> Self {
        Self::Error(SharedError::new(error))
    }
}
impl fmt::Display for SubprocessFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error(error) => error.fmt(f),
            Self::Value(value) => f.write_str(&hoh_util_values::js_string(value)),
            Self::Undefined => f.write_str("undefined"),
        }
    }
}

/// Classify a provider rejection while retaining its message and identity as the cause.
/// Providers should use SubprocessSpawnRejection::new with the actual spawned file.
/// For a bare io::Error only, the fallback approximates that file by `program`.
pub fn subprocess_spawn_failure(error: SubprocessFailure, program: &str) -> SubprocessFailure {
    let SubprocessFailure::Error(original) = &error else {
        return error;
    };
    if HarnessError::of(original.get_ref()).is_some() {
        return error;
    }
    let (errno, detail) =
        if let Some(SubprocessForeignError::Coded { message, code }) = original.downcast_ref() {
            let Some(code) = code.as_str() else {
                return error;
            };
            (code.to_owned(), message.clone())
        } else if let Some(rejection) = original.downcast_ref::<SubprocessSpawnRejection>() {
            let Some(code) = rejection.code() else {
                return error;
            };
            (code.to_owned(), rejection.message().to_owned())
        } else if let Some(io) = original.downcast_ref::<io::Error>() {
            let Some(code) = node_error_code(io) else {
                return error;
            };
            (code.to_owned(), node_spawn_error_message(io, program))
        } else {
            return error;
        };
    let quoted = js_json_stringify(&Value::String(program.to_owned()));
    let (message, code) = if errno == "ENOENT" {
        (
            format!(
                "cannot start {quoted}: the program or the working directory does not exist ({detail})"
            ),
            SubprocessErrorCode::SpawnNotFound,
        )
    } else {
        (
            format!("cannot start {quoted}: {detail}"),
            SubprocessErrorCode::SpawnFailed,
        )
    };
    SharedError::new(SubprocessError::new(message, code).with_cause(original.clone())).into()
}
