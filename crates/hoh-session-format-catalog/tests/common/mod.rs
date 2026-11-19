#![allow(dead_code)]
pub use std::sync::Arc;
#[path = "../../../hoh-session-format-v3-to-v4/tests/common/mod.rs"]
mod native;
pub use hoh_session::{Session, SessionEvent, SessionId, SessionLogOffset, SessionSeedEventState};
pub use hoh_session_format_catalog::*;
pub use native::*;
pub fn policy(validation: SessionFormatValidation) -> SessionFormatRestoreOptions {
    SessionFormatRestoreOptions {
        recovery: SessionFormatRecovery::Strict,
        validation,
    }
}
pub fn restore(
    rows: &[SessionFormatEvent],
    header: &Value,
    validation: SessionFormatValidation,
) -> R<SessionFormatArtifact> {
    let c = create_session_format_catalog_with_children(vec![])?;
    let mut r = c.create_restore(header, policy(validation))?;
    for row in rows {
        r.decode_row(row.clone().into())?;
    }
    r.finish()
}
pub fn reopen(a: &SessionFormatArtifact) -> R<SessionFormatArtifact> {
    let c = session_format_catalog()?;
    let header = c.encode_current_header(&a.header, a.inherited_event_count)?;
    let mut r = c.create_restore(&header.into(), policy(SessionFormatValidation::Current))?;
    for row in &a.events {
        r.decode_row(c.encode_current_event(row)?.into())?;
    }
    r.finish()
}
pub fn detached(a: &SessionFormatArtifact, projections: bool) -> hoh_session::Result<Session> {
    let id: SessionId = a.header["id"].as_str().unwrap().into();
    let events: Vec<_> = a
        .events
        .iter()
        .map(|e| Arc::new(SessionEvent::from_value(Value::Object(e.clone()))))
        .collect();
    Session::from_restore(
        id,
        &events,
        a.header.clone().into(),
        SessionLogOffset::new(a.inherited_event_count)?,
        SessionSeedEventState::Detached,
        projections.then(
            hoh_session_format_catalog::message_projections::current_session_message_projections,
        ),
    )
}
pub fn physical(version: u64, id: &str) -> Value {
    let mut p = obj(json!({"type":"session"}));
    p.extend(h(version, id));
    p.into()
}
pub fn append(
    session: &Session,
    tag: &str,
    data: Value,
    surface: bool,
) -> hoh_session::Result<Arc<SessionEvent>> {
    session.append(
        tag,
        data,
        if surface {
            Some(hoh_session::SurfaceIntent::append())
        } else {
            None
        },
    )
}
