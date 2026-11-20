//! Transport-neutral request validation and response vocabulary.

use crate::{
    SessionLogExportReady, build_session_log_zip, flush_live_session_log, format_session_markdown,
    read_session_log_text, session_log_zip_filename,
};
use hoh_attachment::AttachmentStore;
use hoh_error::{BoxError, HarnessError};
use hoh_session::{SessionId, SessionStore};
use hoh_session_persistence::{SessionPersistence, SessionPersistenceStatOptions};
use hoh_session_query::{SessionQueryEngine, SessionQueryErrorCode};
use hoh_timeout::AbortSignal;
use hoh_util_values::js_trim;

pub const SESSION_LOG_EXPORT_PATH: &str = "/api/session.export";
pub const SESSION_LOG_EXPORT_ROUTE: &str = "api/session.export";
pub const SESSION_MARKDOWN_EXPORT_PATH: &str = "/api/session.markdown";
pub const SESSION_MARKDOWN_EXPORT_ROUTE: &str = "api/session.markdown";

pub const EXPORT_REQUESTED_TEXT: &str = "Session log download requested.";
pub const EXPORT_ARGUMENT_ERROR_TEXT: &str = "The Web /export command does not accept a path.";
pub const INVALID_SESSION_QUERY_TEXT: &str = "missing or invalid sessionId query parameter";
pub const SESSION_NOT_FOUND_TEXT: &str = "session not found";
pub const LOG_SERVICES_UNAVAILABLE_TEXT: &str = "session log export is unavailable: missing session-query, session-persistence, or attachments service";
pub const MARKDOWN_SERVICE_UNAVAILABLE_TEXT: &str =
    "session markdown export is unavailable: missing session-query service";
pub const LOG_READ_FAILED_TEXT: &str = "session log export failed to read the stored log";
pub const MARKDOWN_READ_FAILED_TEXT: &str = "session markdown export failed to read the stored log";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionLogExportQuery {
    pub session_id: String,
    pub include_descendants: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportResponse {
    pub status: u16,
    pub content_type: Option<&'static str>,
    pub content_disposition: Option<String>,
    pub body: Vec<u8>,
}

impl ExportResponse {
    pub fn text(status: u16, body: &'static str) -> Self {
        Self {
            status,
            content_type: None,
            content_disposition: None,
            body: body.as_bytes().to_vec(),
        }
    }

    /// Fetch `HEAD` preserves the prepared status and headers and cancels its body.
    pub fn into_head(mut self) -> Self {
        self.body.clear();
        self
    }
}

#[derive(Clone, Copy)]
pub struct SessionLogExportServices<'a> {
    pub session_query: Option<&'a SessionQueryEngine>,
    pub session_persistence: Option<&'a dyn SessionPersistence>,
    pub attachments: Option<&'a dyn AttachmentStore>,
    pub sessions: Option<&'a SessionStore>,
}

#[derive(Clone, Debug)]
pub struct PreparedSessionLogExport {
    pub session_id: SessionId,
    pub root_content: String,
    pub include_descendants: bool,
    pub compression_level: u8,
    pub content_disposition: String,
}

#[derive(Clone, Debug)]
pub enum SessionLogExportPreparation {
    Immediate(ExportResponse),
    Ready(PreparedSessionLogExport),
}

pub fn validate_session_log_export_query(
    session_id: Option<&str>,
    include_descendants: Option<&str>,
) -> Result<SessionLogExportQuery, &'static str> {
    let session_id = session_id
        .filter(|value| !value.is_empty())
        .ok_or(INVALID_SESSION_QUERY_TEXT)?;
    let include_descendants = match include_descendants {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => return Err(INVALID_SESSION_QUERY_TEXT),
    };
    Ok(SessionLogExportQuery {
        session_id: session_id.to_owned(),
        include_descendants,
    })
}

pub fn validate_session_markdown_query(session_id: Option<&str>) -> Result<&str, &'static str> {
    session_id
        .filter(|value| !value.is_empty())
        .ok_or(INVALID_SESSION_QUERY_TEXT)
}

pub fn export_command(raw_input: &str) -> Result<&'static str, &'static str> {
    if js_trim(raw_input).is_empty() {
        Ok(EXPORT_REQUESTED_TEXT)
    } else {
        Err(EXPORT_ARGUMENT_ERROR_TEXT)
    }
}

/// Perform only the synchronous HTTP preparation phase. A Host can publish the
/// returned headers immediately and drive [`crate::write_session_log_zip`] into
/// its bounded response bridge; descendant and attachment failures then error
/// the body rather than being translated into an HTTP status.
pub async fn prepare_session_log_export_response(
    services: SessionLogExportServices<'_>,
    session_id: Option<&str>,
    include_descendants: Option<&str>,
    compression_level: u8,
    signal: Option<&AbortSignal>,
) -> Result<SessionLogExportPreparation, BoxError> {
    let query = match validate_session_log_export_query(session_id, include_descendants) {
        Ok(query) => query,
        Err(body) => {
            return Ok(SessionLogExportPreparation::Immediate(
                ExportResponse::text(400, body),
            ));
        }
    };
    let (Some(_), Some(session_persistence), Some(_)) = (
        services.session_query,
        services.session_persistence,
        services.attachments,
    ) else {
        return Ok(SessionLogExportPreparation::Immediate(
            ExportResponse::text(500, LOG_SERVICES_UNAVAILABLE_TEXT),
        ));
    };
    let id = SessionId::from(query.session_id.as_str());
    let root_content = match async {
        flush_live_session_log(services.sessions, &id, signal).await?;
        let content = read_session_log_text(session_persistence, &id, signal).await?;
        throw_if_aborted(signal)?;
        Ok::<_, BoxError>(content)
    }
    .await
    {
        Ok(content) => content,
        Err(error) => {
            throw_if_aborted(signal)?;
            drop(error);
            return Ok(SessionLogExportPreparation::Immediate(
                ExportResponse::text(500, LOG_READ_FAILED_TEXT),
            ));
        }
    };
    let Some(root_content) = root_content else {
        return Ok(SessionLogExportPreparation::Immediate(
            ExportResponse::text(404, SESSION_NOT_FOUND_TEXT),
        ));
    };
    Ok(SessionLogExportPreparation::Ready(
        PreparedSessionLogExport {
            session_id: id,
            root_content,
            include_descendants: query.include_descendants,
            compression_level,
            content_disposition: format!(
                "attachment; filename=\"{}\"",
                session_log_zip_filename(&query.session_id),
            ),
        },
    ))
}

/// Buffered convenience adapter. Network Hosts should use
/// [`prepare_session_log_export_response`] plus [`crate::write_session_log_zip`].
pub async fn session_log_export_buffered_response(
    services: SessionLogExportServices<'_>,
    session_id: Option<&str>,
    include_descendants: Option<&str>,
    compression_level: u8,
    signal: Option<&AbortSignal>,
) -> Result<ExportResponse, BoxError> {
    let prepared = prepare_session_log_export_response(
        services,
        session_id,
        include_descendants,
        compression_level,
        signal,
    )
    .await?;
    let plan = match prepared {
        SessionLogExportPreparation::Immediate(response) => return Ok(response),
        SessionLogExportPreparation::Ready(plan) => plan,
    };
    let ready = SessionLogExportReady {
        session_query: services
            .session_query
            .expect("preparation checked session-query"),
        session_persistence: services
            .session_persistence
            .expect("preparation checked persistence"),
        attachments: services
            .attachments
            .expect("preparation checked attachments"),
        sessions: services.sessions,
    };
    let archive = build_session_log_zip(
        &ready,
        plan.root_content,
        &plan.session_id,
        plan.include_descendants,
        plan.compression_level,
        signal,
    )
    .await?;
    Ok(ExportResponse {
        status: 200,
        content_type: Some("application/zip"),
        content_disposition: Some(plan.content_disposition),
        body: archive,
    })
}

/// Prepare the archive `HEAD` preflight without traversing descendants or
/// reading attachments. This is the browser controller's cheap readiness check.
pub async fn session_log_export_head_response(
    services: SessionLogExportServices<'_>,
    session_id: Option<&str>,
    include_descendants: Option<&str>,
    signal: Option<&AbortSignal>,
) -> Result<ExportResponse, BoxError> {
    let query = match validate_session_log_export_query(session_id, include_descendants) {
        Ok(query) => query,
        Err(body) => return Ok(ExportResponse::text(400, body).into_head()),
    };
    let (Some(_), Some(session_persistence), Some(_)) = (
        services.session_query,
        services.session_persistence,
        services.attachments,
    ) else {
        return Ok(ExportResponse::text(500, LOG_SERVICES_UNAVAILABLE_TEXT).into_head());
    };
    let id = SessionId::from(query.session_id.as_str());
    let root_content = match async {
        flush_live_session_log(services.sessions, &id, signal).await?;
        let content = read_session_log_text(session_persistence, &id, signal).await?;
        throw_if_aborted(signal)?;
        Ok::<_, BoxError>(content)
    }
    .await
    {
        Ok(content) => content,
        Err(error) => {
            throw_if_aborted(signal)?;
            drop(error);
            return Ok(ExportResponse::text(500, LOG_READ_FAILED_TEXT).into_head());
        }
    };
    if root_content.is_none() {
        return Ok(ExportResponse::text(404, SESSION_NOT_FOUND_TEXT).into_head());
    }
    Ok(ExportResponse {
        status: 200,
        content_type: Some("application/zip"),
        content_disposition: Some(format!(
            "attachment; filename=\"{}\"",
            session_log_zip_filename(&query.session_id),
        )),
        body: Vec::new(),
    })
}

/// Prepare the Markdown response behind the Host's authenticated Fetch adapter.
pub async fn session_markdown_response(
    session_query: Option<&SessionQueryEngine>,
    persistence: Option<&dyn SessionPersistence>,
    session_id: Option<&str>,
    signal: Option<&AbortSignal>,
) -> Result<ExportResponse, BoxError> {
    let id = match validate_session_markdown_query(session_id) {
        Ok(id) => SessionId::from(id),
        Err(body) => return Ok(ExportResponse::text(400, body)),
    };
    let Some(session_query) = session_query else {
        return Ok(ExportResponse::text(500, MARKDOWN_SERVICE_UNAVAILABLE_TEXT));
    };
    throw_if_aborted(signal)?;
    let result = async {
        let snapshot = session_query.read_session(&id).await?;
        throw_if_aborted(signal)?;
        let source_path = if let Some(persistence) = persistence {
            let options = SessionPersistenceStatOptions {
                signal: signal.cloned(),
            };
            persistence
                .stat(&id, Some(&options))
                .await?
                .and_then(|metadata| {
                    metadata
                        .location
                        .as_ref()
                        .map(|location| location.path.clone())
                })
        } else {
            None
        };
        throw_if_aborted(signal)?;
        Ok::<_, BoxError>(format_session_markdown(
            &snapshot.events,
            source_path.as_deref(),
        ))
    }
    .await;
    match result {
        Ok(markdown) => Ok(ExportResponse {
            status: 200,
            content_type: Some("text/markdown; charset=utf-8"),
            content_disposition: None,
            body: markdown.into_bytes(),
        }),
        Err(error) => {
            throw_if_aborted(signal)?;
            if HarnessError::of(error.as_ref()).is_some_and(|error| {
                error.code() == SessionQueryErrorCode::SessionNotFound.as_str()
            }) {
                Ok(ExportResponse::text(404, SESSION_NOT_FOUND_TEXT))
            } else {
                Ok(ExportResponse::text(500, MARKDOWN_READ_FAILED_TEXT))
            }
        }
    }
}

fn throw_if_aborted(signal: Option<&AbortSignal>) -> Result<(), BoxError> {
    if let Some(signal) = signal {
        signal
            .throw_if_aborted()
            .map_err(|error| Box::new(error) as BoxError)?;
    }
    Ok(())
}
