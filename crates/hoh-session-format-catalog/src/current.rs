use hoh_error::BoxError;
use hoh_session::{
    SESSION_FORMAT_VERSION, Session, SessionEvent, SessionId, SessionLogOffset,
    SessionSeedEventState,
};
use hoh_session_format::{SessionFormatArtifact, SessionFormatHeader};
use serde_json::Value;
use std::sync::Arc;
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct InstalledCurrentSessionError(pub String);
impl InstalledCurrentSessionError {
    pub fn name(&self) -> &'static str {
        "Error"
    }
}
fn version(header: &SessionFormatHeader) -> Result<(), BoxError> {
    if header.get("version").and_then(Value::as_f64) != Some(SESSION_FORMAT_VERSION as f64) {
        return Err(Box::new(InstalledCurrentSessionError(format!(
            "installed Session format is v{SESSION_FORMAT_VERSION}, got v{}",
            header
                .get("version")
                .map(hoh_util_values::js_string)
                .unwrap_or_else(|| "undefined".into())
        ))));
    }
    Ok(())
}
fn id(header: &SessionFormatHeader) -> SessionId {
    header
        .get("id")
        .map(hoh_util_values::js_string)
        .unwrap_or_else(|| "undefined".into())
        .into()
}
pub fn validate_installed_current_session_header(
    header: &SessionFormatHeader,
) -> Result<(), BoxError> {
    version(header)?;
    Session::from_restore(
        id(header),
        &[],
        Value::Object(header.clone()),
        SessionLogOffset::default(),
        SessionSeedEventState::Detached,
        None,
    )?;
    Ok(())
}
pub fn validate_installed_current_session_artifact(
    artifact: &SessionFormatArtifact,
) -> Result<(), BoxError> {
    version(&artifact.header)?;
    let events: Vec<_> = artifact
        .events
        .iter()
        .map(|e| Arc::new(SessionEvent::from_value(Value::Object(e.clone()))))
        .collect();
    Session::from_restore(
        id(&artifact.header),
        &events,
        Value::Object(artifact.header.clone()),
        SessionLogOffset::new(artifact.inherited_event_count)?,
        SessionSeedEventState::Detached,
        Some(crate::message_projections::current_session_message_projections()),
    )?;
    Ok(())
}
