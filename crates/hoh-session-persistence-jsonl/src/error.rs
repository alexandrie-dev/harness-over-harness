use hoh_error::BoxError;

pub type Result<T> = std::result::Result<T, BoxError>;

#[derive(Debug, thiserror::Error)]
pub enum JsonlError {
    #[error("{0}")]
    Message(String),
    #[error("{message}")]
    WithCause {
        message: String,
        #[source]
        cause: BoxError,
    },
    #[error("{message}")]
    Aggregate {
        message: String,
        errors: Vec<BoxError>,
    },
}
pub(crate) fn error(message: impl Into<String>) -> BoxError {
    Box::new(JsonlError::Message(message.into()))
}
pub(crate) fn cause(message: impl Into<String>, cause: impl Into<BoxError>) -> BoxError {
    Box::new(JsonlError::WithCause {
        message: message.into(),
        cause: cause.into(),
    })
}
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
pub(crate) fn js_error(mut value: &(dyn std::error::Error + 'static)) -> String {
    loop {
        if let Some(shared) = value.downcast_ref::<hoh_error::SharedError>() {
            value = shared.get_ref();
            continue;
        }
        if let Some(hoh_session_format::SessionFormatError::Other(inner)) =
            value.downcast_ref::<hoh_session_format::SessionFormatError>()
        {
            value = inner.as_ref();
            continue;
        }
        break;
    }
    let class = if let Some(e) = value.downcast_ref::<hoh_session_format::SessionFormatError>() {
        if e.is_unsupported_migration() {
            "SessionFormatUnsupportedMigrationError"
        } else {
            "SessionFormatError"
        }
    } else if let Some(e) = value.downcast_ref::<hoh_session::SessionError>() {
        e.name()
    } else {
        macro_rules! named {($($t:ty),*)=>{$(if let Some(e)=value.downcast_ref::<$t>() {return format!("{}: {e}",e.name());})*};}
        named!(
            crate::generation::JsonlGenerationSourceChangedError,
            crate::generation::JsonlGenerationUnsupportedMigrationError,
            crate::generation::JsonlGenerationTargetConflictError,
            hoh_session_persistence::SessionAlreadyExistsError,
            hoh_session_persistence::SessionAlreadyOwnedError,
            hoh_session_persistence::SessionPersistenceNotFoundError,
            hoh_session_persistence::SessionReadOnlyError,
            hoh_session_persistence::SessionHandleClosedError,
            hoh_session_persistence::SessionOwnershipLostError,
            hoh_session_persistence::SessionPersistenceCorruptionError,
            hoh_session_persistence::SessionFormatUnsupportedError
        );
        if matches!(
            value.downcast_ref::<JsonlError>(),
            Some(JsonlError::Aggregate { .. })
        ) {
            "AggregateError"
        } else {
            "Error"
        }
    };
    format!("{class}: {value}")
}
