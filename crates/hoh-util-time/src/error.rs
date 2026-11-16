//! Failures the source raises as exceptions: a `TypeError` or a `RangeError` with a fixed message.

/// A violated precondition of this library; the message is the source's text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TimeError {
    /// The source throws a `TypeError`.
    #[error("{0}")]
    Type(String),
    /// The source throws a `RangeError`.
    #[error("{0}")]
    Range(String),
}
