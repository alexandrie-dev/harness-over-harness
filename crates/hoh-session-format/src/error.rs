use std::error::Error;

/// Errors raised when a durable Session artifact cannot be restored or migrated losslessly.
///
/// `Format` is `SessionFormatError`; `UnsupportedMigration` is its subclass
/// `SessionFormatUnsupportedMigrationError`; `Other` carries any error raised by a caller-supplied
/// callback (the source lets any thrown value pass through the machinery).
#[derive(Debug, thiserror::Error)]
pub enum SessionFormatError {
    /// A durable Session artifact cannot be restored or migrated losslessly.
    #[error("{0}")]
    Format(String),
    /// A readable artifact whose released source policy has no supported migration.
    #[error("{message}")]
    UnsupportedMigration {
        message: String,
        #[source]
        cause: Option<Box<SessionFormatError>>,
    },
    /// An error raised by a callback that is not a session-format error.
    #[error("{0}")]
    Other(Box<dyn Error + Send + Sync>),
}

impl SessionFormatError {
    /// A plain `SessionFormatError`.
    pub fn new(message: impl Into<String>) -> Self {
        Self::Format(message.into())
    }

    /// A `SessionFormatUnsupportedMigrationError` without a cause.
    pub fn unsupported_migration(message: impl Into<String>) -> Self {
        Self::UnsupportedMigration {
            message: message.into(),
            cause: None,
        }
    }

    /// Wrap a callback error that is not a session-format error.
    pub fn other(error: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self::Other(error.into())
    }

    /// Whether this is a `SessionFormatUnsupportedMigrationError`.
    pub fn is_unsupported_migration(&self) -> bool {
        matches!(self, Self::UnsupportedMigration { .. })
    }
}
