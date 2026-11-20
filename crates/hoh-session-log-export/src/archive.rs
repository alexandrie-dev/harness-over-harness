use crate::{ZipEntry, zip::StreamingZipWriter};
use futures_util::StreamExt;
use hoh_attachment::{
    AttachmentErrorCode, AttachmentId, AttachmentStore, FileAttachmentRef, ImageAttachmentRef,
    ImageMediaType,
};
use hoh_error::{BoxError, HarnessError};
use hoh_session::{SESSION_FORMAT_VERSION, SessionEvent, SessionHeader, SessionId, SessionStore};
use hoh_session_format::session_format_log_filename;
use hoh_session_persistence::{
    SessionAccess, SessionHandleReadOptions, SessionPersistence, SessionPersistenceNotFoundError,
    SessionPersistenceOpenOptions,
};
use hoh_session_query::SessionQueryEngine;
use hoh_timeout::AbortSignal;
use hoh_util_values::js_json_stringify;
use serde_json::{Map, Value};
use std::{borrow::Borrow, collections::HashSet, io, io::Write, sync::LazyLock};

pub const DEFAULT_SESSION_LOG_COMPRESSION_LEVEL: u8 = 6;
pub static SESSION_LOG_FILENAME: LazyLock<String> = LazyLock::new(|| {
    session_format_log_filename(SESSION_FORMAT_VERSION)
        .expect("the current Session generation is a safe integer")
});
pub const MISSING_ATTACHMENTS_FILENAME: &str = "attachments-missing.txt";

pub struct SessionLogExportReady<'a> {
    pub session_query: &'a SessionQueryEngine,
    pub session_persistence: &'a dyn SessionPersistence,
    pub attachments: &'a dyn AttachmentStore,
    pub sessions: Option<&'a SessionStore>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArtifactAttachmentRefs {
    pub images: Vec<ImageAttachmentRef>,
    pub files: Vec<FileAttachmentRef>,
}

#[derive(Clone)]
struct ArchiveImageRef {
    reference: ImageAttachmentRef,
    path: String,
}

#[derive(Clone)]
struct ArchiveFileRef {
    reference: FileAttachmentRef,
    path: Option<String>,
}

#[derive(Default)]
struct ArchiveAttachmentRefs {
    images: Vec<ArchiveImageRef>,
    files: Vec<ArchiveFileRef>,
}

pub fn resolve_compression_level(value: Option<f64>) -> Result<u8, &'static str> {
    let value = value.unwrap_or(f64::from(DEFAULT_SESSION_LOG_COMPRESSION_LEVEL));
    if value.is_finite() && value.fract() == 0.0 && (0.0..=9.0).contains(&value) {
        Ok(value as u8)
    } else {
        Err("compressionLevel must be an integer from 0 through 9")
    }
}

pub fn serialize_session_log(
    header: &SessionHeader,
    events: &[impl Borrow<SessionEvent>],
) -> Result<String, serde_json::Error> {
    let mut physical = Map::new();
    physical.insert("type".to_owned(), Value::String("session".to_owned()));
    physical.insert("version".to_owned(), Value::from(header.version));
    physical.insert("id".to_owned(), Value::String(header.id.to_string()));
    physical.insert("createdAt".to_owned(), Value::from(header.created_at));
    if let Some(value) = &header.cwd {
        physical.insert("cwd".to_owned(), Value::String(value.clone()));
    }
    if let Some(value) = &header.parent_session {
        physical.insert("parentSession".to_owned(), Value::String(value.to_string()));
    }
    physical.insert("isSeeded".to_owned(), Value::Bool(header.is_seeded));
    if let Some(value) = &header.origin {
        physical.insert("origin".to_owned(), Value::String(value.clone()));
    }
    physical.insert(
        "delegationDepth".to_owned(),
        Value::from(header.delegation_depth.unwrap_or(0)),
    );
    if let Some(value) = &header.agent_preset {
        physical.insert("agentPreset".to_owned(), Value::String(value.clone()));
    }
    let mut lines = Vec::with_capacity(events.len() + 1);
    lines.push(js_json_stringify(&Value::Object(physical)));
    for event in events {
        lines.push(js_json_stringify(
            <_ as Borrow<SessionEvent>>::borrow(event).as_value(),
        ));
    }
    Ok(format!("{}\n", lines.join("\n")))
}

pub async fn read_session_log_text(
    persistence: &dyn SessionPersistence,
    id: &SessionId,
    signal: Option<&AbortSignal>,
) -> Result<Option<String>, BoxError> {
    let open_options = SessionPersistenceOpenOptions {
        signal: signal.cloned(),
    };
    let handle = match persistence
        .open(id, SessionAccess::Read, Some(&open_options))
        .await
    {
        Ok(handle) => handle,
        Err(error)
            if error
                .downcast_ref::<SessionPersistenceNotFoundError>()
                .is_some() =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let read_options = SessionHandleReadOptions {
        signal: signal.cloned(),
    };
    let result = handle
        .read(Some(0), None, Some(&read_options))
        .await
        .and_then(|read| serialize_session_log(handle.header(), &read.events).map_err(Into::into));
    handle.close().await?;
    result.map(Some)
}

pub async fn flush_live_session_log(
    sessions: Option<&SessionStore>,
    id: &SessionId,
    signal: Option<&AbortSignal>,
) -> Result<(), BoxError> {
    throw_if_aborted(signal)?;
    let Some(session) = sessions.and_then(|sessions| sessions.get(id)) else {
        return Ok(());
    };
    sessions
        .expect("store exists when it returned a session")
        .flush(&session)
        .await?;
    throw_if_aborted(signal)
}

pub fn safe_session_id_segment(id: &str) -> String {
    id.encode_utf16()
        .map(|unit| {
            let byte = u8::try_from(unit).ok();
            if byte
                .is_some_and(|value| value.is_ascii_alphanumeric() || matches!(value, b'_' | b'-'))
            {
                char::from(byte.unwrap_or_default())
            } else {
                '_'
            }
        })
        .collect()
}

pub fn session_log_zip_filename(session_id: &str) -> String {
    format!("dsh-session-{}.zip", safe_session_id_segment(session_id))
}

pub fn media_entry_path(reference: &ImageAttachmentRef) -> String {
    let extension = match reference.media_type {
        ImageMediaType::Png => "png",
        ImageMediaType::Jpeg => "jpg",
        ImageMediaType::Webp => "webp",
        ImageMediaType::Gif => "gif",
    };
    format!("media/{}.{}", reference.attachment_id, extension)
}

pub fn file_entry_path(reference: &FileAttachmentRef) -> String {
    let raw = reference.attachment_id.to_string();
    let digest = raw.strip_prefix("sha256:").unwrap_or(&raw);
    let name: String = reference
        .name
        .chars()
        .map(|character| {
            if matches!(character, '/' | '\\') || character <= '\u{1f}' || character == '\u{7f}' {
                '_'
            } else {
                character
            }
        })
        .collect();
    let name = if name.is_empty() || matches!(name.as_str(), "." | "..") {
        "file"
    } else {
        &name
    };
    let prefix: String = digest.chars().take(2).collect();
    format!("files/{prefix}/{digest}/{name}")
}

pub fn attachment_refs_in_artifact(content: &str) -> ArtifactAttachmentRefs {
    let refs = archive_attachment_refs_in_artifact(content);
    ArtifactAttachmentRefs {
        images: refs
            .images
            .into_iter()
            .map(|entry| entry.reference)
            .collect(),
        files: refs
            .files
            .into_iter()
            .map(|entry| entry.reference)
            .collect(),
    }
}

fn archive_attachment_refs_in_artifact(content: &str) -> ArchiveAttachmentRefs {
    let mut refs = ArchiveAttachmentRefs::default();
    for line in content.split('\n').filter(|line| !line.is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        collect_event_attachment_refs(&value, &mut refs);
    }
    refs
}

fn collect_event_attachment_refs(event: &Value, refs: &mut ArchiveAttachmentRefs) {
    let Some(kind) = event.get("type").and_then(Value::as_str) else {
        return;
    };
    let Some(data) = event.get("data").and_then(Value::as_object) else {
        return;
    };
    match kind {
        "user/message" | "tool/ptc-dispatch" => collect_attachment_refs(data.get("content"), refs),
        "system/message" | "developer/message" | "tool/result" | "team/message/queued" => {
            collect_attachment_refs(
                data.get("message")
                    .and_then(|message| message.get("content")),
                refs,
            );
        }
        "agent/inbox/spliced" => {
            if let Some(messages) = data.get("inserted").and_then(Value::as_array) {
                for message in messages {
                    collect_attachment_refs(message.get("content"), refs);
                }
            }
        }
        "compaction/summary" => {
            collect_attachment_refs(data.get("summary"), refs);
            collect_attachment_refs(data.get("rawOutput"), refs);
        }
        "assistant/message" | "assistant/attempt" => {
            if kind == "assistant/message" {
                collect_attachment_refs(
                    data.get("message")
                        .and_then(|message| message.get("content")),
                    refs,
                );
            }
            if let Some(stream) = data.get("stream").and_then(Value::as_array) {
                for record in stream {
                    if record.get("type").and_then(Value::as_str) == Some("chunk")
                        && record.pointer("/chunk/type").and_then(Value::as_str)
                            == Some("block-end")
                    {
                        collect_attachment_refs(
                            record.pointer("/chunk/block").map(std::slice::from_ref),
                            refs,
                        );
                    }
                }
            }
        }
        _ => {}
    }
}

trait ContentValues<'a> {
    fn values(self) -> Option<Vec<&'a Value>>;
}

impl<'a> ContentValues<'a> for Option<&'a Value> {
    fn values(self) -> Option<Vec<&'a Value>> {
        self.and_then(Value::as_array)
            .map(|items| items.iter().collect())
    }
}

impl<'a> ContentValues<'a> for Option<&'a [Value]> {
    fn values(self) -> Option<Vec<&'a Value>> {
        self.map(|items| items.iter().collect())
    }
}

fn collect_attachment_refs<'a>(content: impl ContentValues<'a>, refs: &mut ArchiveAttachmentRefs) {
    let Some(blocks) = content.values() else {
        return;
    };
    for block in blocks {
        let Some(reference) = block.get("attachment") else {
            continue;
        };
        match block.get("type").and_then(Value::as_str) {
            Some("image") => {
                if let Some(reference) = archive_image_ref(reference) {
                    remember_image(refs, reference);
                }
            }
            Some("file") => {
                if let Some(reference) = archive_file_ref(reference) {
                    remember_file(refs, reference);
                }
            }
            _ => {}
        }
    }
}

fn archive_image_ref(value: &Value) -> Option<ArchiveImageRef> {
    if !value.is_object() && !value.is_array() {
        return None;
    }
    if let Ok(reference) = serde_json::from_value::<ImageAttachmentRef>(value.clone()) {
        let path = media_entry_path(&reference);
        return Some(ArchiveImageRef { reference, path });
    }
    let attachment_id = loose_attachment_id(value.get("attachmentId"));
    let raw_media_type = value.get("mediaType").and_then(Value::as_str);
    let (media_type, extension) = match raw_media_type {
        Some("image/png") => (ImageMediaType::Png, "png"),
        Some("image/jpeg") => (ImageMediaType::Jpeg, "jpg"),
        Some("image/webp") => (ImageMediaType::Webp, "webp"),
        Some("image/gif") => (ImageMediaType::Gif, "gif"),
        _ => (ImageMediaType::Png, "undefined"),
    };
    let path = format!("media/{attachment_id}.{extension}");
    Some(ArchiveImageRef {
        reference: ImageAttachmentRef {
            attachment_id,
            media_type,
            bytes: value.get("bytes").and_then(Value::as_u64).unwrap_or(0),
            width: value.get("width").and_then(Value::as_u64).unwrap_or(0),
            height: value.get("height").and_then(Value::as_u64).unwrap_or(0),
            name: value.get("name").and_then(Value::as_str).map(str::to_owned),
            original_dimensions: None,
        },
        path,
    })
}

fn archive_file_ref(value: &Value) -> Option<ArchiveFileRef> {
    if !value.is_object() && !value.is_array() {
        return None;
    }
    if let Ok(reference) = serde_json::from_value::<FileAttachmentRef>(value.clone()) {
        let path = file_entry_path(&reference);
        return Some(ArchiveFileRef {
            reference,
            path: Some(path),
        });
    }
    let attachment_id = loose_attachment_id(value.get("attachmentId"));
    let name = value.get("name").and_then(Value::as_str).map(str::to_owned);
    let reference = FileAttachmentRef {
        attachment_id,
        name: name.clone().unwrap_or_default(),
        bytes: value.get("bytes").and_then(Value::as_u64).unwrap_or(0),
    };
    let path = name.map(|_| file_entry_path(&reference));
    Some(ArchiveFileRef { reference, path })
}

fn require_file_entry_path(path: Option<String>) -> Result<String, BoxError> {
    path.ok_or_else(|| boxed_message("file attachment name is not a string".to_owned()))
}

fn loose_attachment_id(value: Option<&Value>) -> AttachmentId {
    let value = match value {
        Some(Value::String(value)) => value.clone(),
        Some(value) => js_json_stringify(value),
        None => "undefined".to_owned(),
    };
    AttachmentId::from(value.as_str())
}

fn remember_image(refs: &mut ArchiveAttachmentRefs, reference: ArchiveImageRef) {
    if let Some(current) = refs
        .images
        .iter_mut()
        .find(|current| current.reference.attachment_id == reference.reference.attachment_id)
    {
        *current = reference;
    } else {
        refs.images.push(reference);
    }
}

fn remember_file(refs: &mut ArchiveAttachmentRefs, reference: ArchiveFileRef) {
    if let Some(current) = refs.files.iter_mut().find(|current| {
        current.reference.attachment_id == reference.reference.attachment_id
            && current.reference.name == reference.reference.name
    }) {
        *current = reference;
    } else {
        refs.files.push(reference);
    }
}

/// Collecting inspection helper used by tests and non-streaming consumers.
/// Network adapters should call [`write_session_log_zip`] instead.
pub async fn collect_session_log_zip_entries(
    deps: &SessionLogExportReady<'_>,
    root_content: String,
    session_id: &SessionId,
    include_descendants: bool,
    signal: Option<&AbortSignal>,
) -> Result<Vec<ZipEntry>, BoxError> {
    let mut refs = archive_attachment_refs_in_artifact(&root_content);
    let mut entries = vec![ZipEntry::text(SESSION_LOG_FILENAME.as_str(), root_content)];
    if include_descendants {
        let lineage = deps
            .session_query
            .trace_session(session_id, signal.cloned())
            .await?;
        throw_if_aborted(signal)?;
        let mut seen = HashSet::from([session_id.clone()]);
        let mut stack = lineage.descendants.iter().rev().collect::<Vec<_>>();
        while let Some(node) = stack.pop() {
            throw_if_aborted(signal)?;
            let id = &node.session.header.id;
            if !seen.insert(id.clone()) {
                continue;
            }
            for descendant in node.descendants.iter().rev() {
                stack.push(descendant);
            }
            flush_live_session_log(deps.sessions, id, signal).await?;
            let content = read_session_log_text(deps.session_persistence, id, signal).await?;
            throw_if_aborted(signal)?;
            let content = content
                .ok_or_else(|| boxed_message(format!("subagent \"{id}\" has no stored log")))?;
            merge_refs(&mut refs, archive_attachment_refs_in_artifact(&content));
            entries.push(ZipEntry::text(
                format!(
                    "subagents/{}/{name}",
                    safe_session_id_segment(id.as_ref()),
                    name = SESSION_LOG_FILENAME.as_str()
                ),
                content,
            ));
        }
    }
    let mut missing = Vec::new();
    for entry in refs.images {
        throw_if_aborted(signal)?;
        let path = entry.path;
        match deps.attachments.read_image(&entry.reference, signal).await {
            Ok(stored) => entries.push(ZipEntry::bytes(path, stored.data)),
            Err(error) if is_unreadable_reference(error.as_ref()) => {
                missing.push(format!("{path}\t{error}"))
            }
            Err(error) => return Err(error),
        }
    }
    for entry in refs.files {
        throw_if_aborted(signal)?;
        let mut stream = deps.attachments.read_file_stream(&entry.reference, signal);
        let mut data = Vec::new();
        let mut first = true;
        let mut unreadable = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => {
                    data.extend_from_slice(&chunk);
                    first = false;
                }
                Err(error) if first && is_unreadable_reference(error.as_ref()) => {
                    unreadable = Some(error.to_string());
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        let path = require_file_entry_path(entry.path)?;
        if let Some(error) = unreadable {
            missing.push(format!("{path}\t{error}"));
        } else {
            entries.push(ZipEntry::bytes(path, data));
        }
    }
    if !missing.is_empty() {
        entries.push(ZipEntry::text(
            MISSING_ATTACHMENTS_FILENAME,
            format!("{}\n", missing.join("\n")),
        ));
    }
    Ok(entries)
}

pub async fn build_session_log_zip(
    deps: &SessionLogExportReady<'_>,
    root_content: String,
    session_id: &SessionId,
    include_descendants: bool,
    compression_level: u8,
    signal: Option<&AbortSignal>,
) -> Result<Vec<u8>, BoxError> {
    write_session_log_zip(
        deps,
        root_content,
        session_id,
        include_descendants,
        compression_level,
        signal,
        Vec::new(),
    )
    .await
}

/// Incrementally write an archive to a synchronous sink. The writer yields before entering the
/// sink and between bounded chunks, but `Write::write` itself must either be non-blocking or be
/// drained by an independent OS thread. A bridge drained only by a task on this same executor can
/// deadlock inside its synchronous `write` call. Attachment files are never collected whole.
pub async fn write_session_log_zip<W: Write>(
    deps: &SessionLogExportReady<'_>,
    root_content: String,
    session_id: &SessionId,
    include_descendants: bool,
    compression_level: u8,
    signal: Option<&AbortSignal>,
    output: W,
) -> Result<W, BoxError> {
    let mut refs = archive_attachment_refs_in_artifact(&root_content);
    let mut archive = StreamingZipWriter::new(output, compression_level)
        .map_err(|error| Box::new(error) as BoxError)?;
    archive
        .add_bytes(
            SESSION_LOG_FILENAME.as_str(),
            root_content.as_bytes(),
            signal,
        )
        .await?;
    if include_descendants {
        let lineage = deps
            .session_query
            .trace_session(session_id, signal.cloned())
            .await?;
        throw_if_aborted(signal)?;
        let mut seen = HashSet::from([session_id.clone()]);
        let mut stack = lineage.descendants.iter().rev().collect::<Vec<_>>();
        while let Some(node) = stack.pop() {
            throw_if_aborted(signal)?;
            let id = &node.session.header.id;
            if !seen.insert(id.clone()) {
                continue;
            }
            for descendant in node.descendants.iter().rev() {
                stack.push(descendant);
            }
            flush_live_session_log(deps.sessions, id, signal).await?;
            let content = read_session_log_text(deps.session_persistence, id, signal).await?;
            throw_if_aborted(signal)?;
            let content = content
                .ok_or_else(|| boxed_message(format!("subagent \"{id}\" has no stored log")))?;
            merge_refs(&mut refs, archive_attachment_refs_in_artifact(&content));
            let path = format!(
                "subagents/{}/{name}",
                safe_session_id_segment(id.as_ref()),
                name = SESSION_LOG_FILENAME.as_str(),
            );
            archive.add_bytes(&path, content.as_bytes(), signal).await?;
        }
    }
    let mut missing = Vec::new();
    for entry in refs.images {
        throw_if_aborted(signal)?;
        let path = entry.path;
        match deps.attachments.read_image(&entry.reference, signal).await {
            Ok(stored) => archive.add_bytes(&path, &stored.data, signal).await?,
            Err(error) if is_unreadable_reference(error.as_ref()) => {
                missing.push(format!("{path}\t{error}"))
            }
            Err(error) => return Err(error),
        }
    }
    for entry in refs.files {
        throw_if_aborted(signal)?;
        let mut stream = deps.attachments.read_file_stream(&entry.reference, signal);
        let first = stream.next().await;
        let path = require_file_entry_path(entry.path)?;
        match first {
            Some(Err(error)) if is_unreadable_reference(error.as_ref()) => {
                missing.push(format!("{path}\t{error}"));
            }
            Some(Err(error)) => return Err(error),
            Some(Ok(first)) => {
                archive
                    .add_stream(&path, Some(first), &mut stream, signal)
                    .await?
            }
            None => archive.add_stream(&path, None, &mut stream, signal).await?,
        }
    }
    if !missing.is_empty() {
        let content = format!("{}\n", missing.join("\n"));
        archive
            .add_bytes(MISSING_ATTACHMENTS_FILENAME, content.as_bytes(), signal)
            .await?;
    }
    archive
        .finish()
        .map_err(|error| Box::new(error) as BoxError)
}

fn merge_refs(target: &mut ArchiveAttachmentRefs, source: ArchiveAttachmentRefs) {
    for reference in source.images {
        remember_image(target, reference);
    }
    for reference in source.files {
        remember_file(target, reference);
    }
}

fn is_unreadable_reference(error: &(dyn std::error::Error + 'static)) -> bool {
    HarnessError::of(error).is_some_and(|error| {
        matches!(
            AttachmentErrorCode::from_code(error.code()),
            Some(
                AttachmentErrorCode::AttachmentNotFound | AttachmentErrorCode::InvalidAttachmentRef
            )
        )
    })
}

fn throw_if_aborted(signal: Option<&AbortSignal>) -> Result<(), BoxError> {
    signal
        .map(AbortSignal::throw_if_aborted)
        .transpose()
        .map_err(|error| Box::new(error) as BoxError)?;
    Ok(())
}

fn boxed_message(message: String) -> BoxError {
    Box::new(io::Error::other(message))
}
