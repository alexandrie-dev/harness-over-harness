use crate::{
    JsonlError, NAME, Result,
    error::{cause, error},
    format::*,
    io::{FileSystem, IoHook, JsonlPhysicalIdentity, is_io, is_not_found, lock},
    lease::SessionWriteLease,
    storage::{JsonlBackendTracker, JsonlHandleStorage, JsonlSessionHandle, StorageHandleState},
    zstd::*,
};
use futures_util::{StreamExt, future::BoxFuture};
use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Cfg, Context, Plugin, Undo};
use hoh_session::{
    SESSION_FORMAT_VERSION, SessionEvent, SessionHeader, SessionId, SessionLogOffset,
    SessionSeedEventState,
};
use hoh_session_persistence::*;
use hoh_timeout::AbortSignal;
use hoh_util_values::{js_json_parse, js_json_stringify};
use hoh_util_workspace_path::node_path;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};

type HeaderProbe = (
    Selected,
    Option<JsonlPhysicalIdentity>,
    Option<HeaderMetadata>,
);

fn default_concurrency() -> usize {
    3
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub root: String,
    #[serde(default)]
    pub compression: JsonlCompression,
    #[serde(default = "default_concurrency")]
    #[schemars(range(min = 1))]
    pub list_concurrency: usize,
}
impl Config {
    pub fn new(root: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            compression: JsonlCompression::Zstd,
            list_concurrency: 3,
        }
    }
}
#[derive(Clone)]
pub struct JsonlSessionPersistence(Arc<Backend>);
struct Backend {
    ctx: Context,
    root: String,
    compression: JsonlCompression,
    list_concurrency: usize,
    list_limit: tokio::sync::Semaphore,
    fs: FileSystem,
    tracker: Arc<JsonlBackendTracker>,
    identity: SessionPersistenceIdentity,
    root_encoding: tokio::sync::OnceCell<std::result::Result<(), SharedError>>,
    memo: Mutex<IndexMap<SessionId, Arc<StoredLog>>>,
    listed: Mutex<IndexMap<String, ListedHeader>>,
}
#[derive(Clone)]
struct StoredLog {
    meta: SessionHeader,
    events: Vec<Arc<SessionEvent>>,
    cut: SessionLogOffset,
    torn: Option<u64>,
    recovered: Vec<Arc<SessionEvent>>,
    revision: SessionPersistenceRevision,
}
#[derive(Clone)]
struct Selected {
    source_path: String,
    version: u64,
    current_path: String,
}
#[derive(Clone)]
struct HeaderMetadata {
    header: SessionHeader,
    status: SessionFormatStatus,
}
#[derive(Clone)]
struct ListedHeader {
    revision: SessionPersistenceRevision,
    metadata: Option<HeaderMetadata>,
}
struct DirectoryBatch {
    dirs: Vec<String>,
    failure: Option<BoxError>,
}
struct WriteClaimGuard {
    tracker: Arc<JsonlBackendTracker>,
    id: SessionId,
    active: bool,
}
impl Drop for WriteClaimGuard {
    fn drop(&mut self) {
        if self.active {
            self.tracker.release_claim(&self.id);
        }
    }
}
fn check(signal: Option<&AbortSignal>) -> Result<()> {
    if let Some(signal) = signal {
        signal.throw_if_aborted()?;
    }
    Ok(())
}
fn location(path: &str) -> Arc<SessionLocation> {
    Arc::new(SessionLocation {
        kind: "jsonl".into(),
        path: path.into(),
    })
}
fn snapshot(
    header: SessionHeader,
    revision: SessionPersistenceRevision,
    status: Option<SessionFormatStatus>,
    size: Option<u64>,
    path: Option<&str>,
) -> SessionPersistenceSnapshot {
    SessionPersistenceSnapshot {
        header,
        revision,
        format_status: status,
        event_count: None,
        size_bytes: size,
        location: path.map(location),
    }
}
fn random_token(bytes: usize) -> Result<String> {
    let mut value = vec![0; bytes];
    getrandom::fill(&mut value).map_err(|e| error(e.to_string()))?;
    Ok(value.iter().map(|byte| format!("{byte:02x}")).collect())
}

impl JsonlSessionPersistence {
    pub async fn new(ctx: &Context, config: Config) -> Result<Self> {
        Self::with_io_hook(ctx, config, None).await
    }
    pub async fn with_io_hook(ctx: &Context, config: Config, hook: Option<IoHook>) -> Result<Self> {
        if config.list_concurrency == 0
            || config.list_concurrency > hoh_session::MAX_SAFE_INTEGER as usize
        {
            return Err(error("listConcurrency must be a positive integer"));
        }
        let cwd = std::env::current_dir()?;
        let root = node_path::resolve(&cwd.to_string_lossy(), &[&config.root]);
        let fs = FileSystem { hook };
        match fs.read_dir(&root).await {
            Ok(_) => {}
            Err(e) if is_not_found(e.as_ref()) => {}
            Err(e) => return Err(e),
        }
        Ok(Self(Arc::new(Backend {
            ctx: ctx.clone(),
            root,
            compression: config.compression,
            list_concurrency: config.list_concurrency,
            list_limit: tokio::sync::Semaphore::new(config.list_concurrency),
            fs,
            tracker: Arc::new(JsonlBackendTracker::new(NAME)),
            identity: SessionPersistenceIdentity::new(),
            root_encoding: tokio::sync::OnceCell::new(),
            memo: Mutex::new(IndexMap::new()),
            listed: Mutex::new(IndexMap::new()),
        })))
    }
    pub fn root(&self) -> &str {
        &self.0.root
    }
    pub fn compression(&self) -> JsonlCompression {
        self.0.compression
    }
    pub(crate) fn writer_for_testing(&self, id: &SessionId) -> Option<Arc<JsonlSessionHandle>> {
        self.0.tracker.writer(id)
    }
    pub async fn close(&self) -> Result<()> {
        self.0.tracker.close_all().await
    }
    fn target(&self, header: &SessionHeader) -> Result<String> {
        log_path(
            &self.0.root,
            header.cwd.as_deref(),
            &header.id,
            self.0.compression,
        )
    }
    async fn ensure_root_encoding(&self) -> Result<()> {
        self.0
            .root_encoding
            .get_or_init(|| async { self.check_root_encoding().await.map_err(SharedError::new) })
            .await
            .clone()
            .map_err(|e| Box::new(e) as BoxError)
    }
    async fn limited<T>(&self, future: impl std::future::Future<Output = Result<T>>) -> Result<T> {
        let _permit = self
            .0
            .list_limit
            .acquire()
            .await
            .map_err(|e| error(e.to_string()))?;
        future.await
    }
    async fn project_dirs(&self, signal: Option<&AbortSignal>) -> Result<Vec<String>> {
        check(signal)?;
        let entries = match self.0.fs.read_dir(&self.0.root).await {
            Ok(entries) => entries,
            Err(e) if is_not_found(e.as_ref()) => return Ok(vec![]),
            Err(e) => return Err(e),
        };
        check(signal)?;
        Ok(entries
            .into_iter()
            .filter(|e| e.1 && e.0 != "trash")
            .map(|e| node_path::join(&[&self.0.root, &e.0]))
            .collect())
    }
    fn legacy_layout(&self, path: &str) -> BoxError {
        error(format!(
            "session artifact {} uses the unsupported flat-file layout; use a separate root or move it into a project/session directory before loading",
            js_json_stringify(&Value::from(path))
        ))
    }
    fn encoding_mismatch(&self, path: &str) -> BoxError {
        error(format!(
            "session artifact {} uses {}, but this backend is configured for compression {}; use a separate root or select the matching compression mode",
            js_json_stringify(&Value::from(path)),
            log_suffix(self.0.compression.opposite()),
            js_json_stringify(&Value::from(self.0.compression.label()))
        ))
    }
    async fn session_dirs(
        &self,
        project: &str,
        signal: Option<&AbortSignal>,
    ) -> Result<Vec<String>> {
        check(signal)?;
        let entries = self.0.fs.read_dir(project).await?;
        check(signal)?;
        if let Some(entry) = entries
            .iter()
            .find(|e| e.2 && (e.0.ends_with(".jsonl") || e.0.ends_with(".jsonl.zstd")))
        {
            return Err(self.legacy_layout(&node_path::join(&[project, &entry.0])));
        }
        let mut dirs = Vec::new();
        for entry in entries.into_iter().filter(|entry| entry.1) {
            let path = node_path::join(&[project, &entry.0]);
            if entry.0 == "trash" {
                // `trash` can also be a legitimate session ID. Ignore a foreign nested trash
                // directory, but preserve a real session with this exact header and path.
                let valid = match self.resolve_generation(&path, signal).await {
                    Ok(Some(selected)) => matches!(
                        self.read_generation_header(
                            &selected,
                            Some(&SessionId::from("trash")),
                            signal
                        )
                        .await,
                        Ok(Some(_))
                    ),
                    _ => false,
                };
                if !valid {
                    continue;
                }
            }
            dirs.push(path);
        }
        Ok(dirs)
    }
    async fn all_dirs(&self, signal: Option<&AbortSignal>) -> Result<DirectoryBatch> {
        let projects = self.limited(self.project_dirs(signal)).await?;
        let mut jobs: Vec<BoxFuture<'static, Result<Vec<String>>>> = vec![];
        for project in projects {
            let backend = self.clone();
            let signal = signal.cloned();
            jobs.push(Box::pin(async move {
                backend
                    .limited(backend.session_dirs(&project, signal.as_ref()))
                    .await
            }));
        }
        let results = futures_util::stream::iter(jobs)
            .buffered(self.0.list_concurrency)
            .collect::<Vec<_>>()
            .await;
        let mut dirs = vec![];
        let mut failure = None;
        for result in results {
            match result {
                Ok(entries) => dirs.extend(entries),
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        Ok(DirectoryBatch { dirs, failure })
    }
    async fn check_root_encoding(&self) -> Result<()> {
        let batch = self.all_dirs(None).await?;
        for dir in batch.dirs {
            if let Some(path) = self.limited(self.find_opposite(&dir)).await? {
                return Err(self.encoding_mismatch(&path));
            }
        }
        if let Some(failure) = batch.failure {
            return Err(failure);
        }
        Ok(())
    }
    async fn entries_optional(&self, dir: &str) -> Result<Vec<(String, bool, bool)>> {
        match self.0.fs.read_dir(dir).await {
            Ok(entries) => Ok(entries),
            Err(e) if is_not_found(e.as_ref()) => Ok(vec![]),
            Err(e) => Err(e),
        }
    }
    async fn find_opposite(&self, dir: &str) -> Result<Option<String>> {
        let entries = self.entries_optional(dir).await?;
        Ok(entries
            .iter()
            .filter_map(|entry| {
                parse_generation_log_filename(&entry.0, self.0.compression.opposite())
                    .map(|version| (version, &entry.0))
            })
            .max_by_key(|entry| entry.0)
            .map(|(_, name)| node_path::join(&[dir, name])))
    }
    async fn resolve_generation(
        &self,
        dir: &str,
        signal: Option<&AbortSignal>,
    ) -> Result<Option<Selected>> {
        check(signal)?;
        let entries = self.entries_optional(dir).await?;
        check(signal)?;
        let mut selected = None;
        for entry in entries {
            if let Some(version) = parse_generation_log_filename(&entry.0, self.0.compression) {
                if selected.as_ref().is_none_or(|(old, _)| version > *old) {
                    selected = Some((version, entry.0));
                }
            } else if parse_generation_log_filename(&entry.0, self.0.compression.opposite())
                .is_some()
            {
                return Err(self.encoding_mismatch(&node_path::join(&[dir, &entry.0])));
            }
        }
        let current_name = generation_log_filename(SESSION_FORMAT_VERSION, self.0.compression)?;
        Ok(selected.map(|(version, name)| Selected {
            source_path: node_path::join(&[dir, &name]),
            version,
            current_path: node_path::join(&[dir, &current_name]),
        }))
    }
    async fn exists(&self, path: &str) -> Result<bool> {
        match self.0.fs.open(path, "r").await {
            Ok(file) => {
                file.close().await?;
                Ok(true)
            }
            Err(e) if is_not_found(e.as_ref()) => Ok(false),
            Err(e) => Err(e),
        }
    }
    async fn find_log(
        &self,
        id: &SessionId,
        signal: Option<&AbortSignal>,
    ) -> Result<Option<Selected>> {
        let mut matches = vec![];
        for project in self.project_dirs(signal).await? {
            check(signal)?;
            let encoded = encode_segment(id.as_str())?;
            for compression in [JsonlCompression::Zstd, JsonlCompression::None] {
                let path =
                    node_path::join(&[&project, &format!("{encoded}{}", log_suffix(compression))]);
                if self.exists(&path).await? {
                    return Err(self.legacy_layout(&path));
                }
                check(signal)?;
            }
            if let Some(selected) = self
                .resolve_generation(&node_path::join(&[&project, &encoded]), signal)
                .await?
            {
                matches.push(selected);
            }
        }
        if matches.len() > 1 {
            return Err(error(format!(
                "duplicate JSONL session id \"{id}\" appears in multiple project directories"
            )));
        }
        check(signal)?;
        Ok(matches.pop())
    }
    async fn first_line(&self, path: &str, signal: Option<&AbortSignal>) -> Result<Option<String>> {
        check(signal)?;
        let mut handle = self.0.fs.open(path, "r").await?;
        let mut content = vec![];
        let mut scratch = vec![0; 8192];
        let value=async {loop {check(signal)?;let count=handle.read(&mut scratch).await?;check(signal)?;if count==0 {return Ok(None);}let slice=&scratch[..count];
            if self.0.compression==JsonlCompression::None {if let Some(nl)=slice.iter().position(|&b|b==b'\n') {content.extend_from_slice(&slice[..nl]);return Ok(Some(String::from_utf8_lossy(&content).into_owned()));}content.extend_from_slice(slice);}
            else {content.extend_from_slice(slice);let decoded=async {let scan=scan_zstd_frames(&content,Some(1))?;let Some(first)=scan.frames.first() else {return Ok(None);};let bytes=decompress_zstd_frame(&content[first.start..first.end]).await?;assert_header_frame(&bytes)?;Ok(Some(String::from_utf8_lossy(&bytes[..bytes.len()-1]).into_owned()))}.await;
                match decoded {Ok(Some(line))=>return Ok(Some(line)),Ok(None)=>{},Err(failure)=>{check(signal)?;return Err(Box::new(SessionPersistenceCorruptionError::new(format!("corrupt Zstandard session log: header frame failed validation: Error: {failure} (raw log: {path})"),failure)) as BoxError);}}
            }
        }}.await;
        let cleanup = handle.close().await;
        cleanup?;
        value
    }
    async fn assert_stored_identity(
        &self,
        path: &str,
        version: u64,
        header: &SessionHeader,
        id: Option<&SessionId>,
        signal: Option<&AbortSignal>,
    ) -> Result<()> {
        check(signal)?;
        if id.is_some_and(|id| *id != header.id) {
            return Err(error(format!(
                "corrupt session log \"{path}\": requested id \"{}\" does not match header id \"{}\"",
                id.map(SessionId::as_str).unwrap_or(""),
                header.id
            )));
        }
        let expected = generation_log_path(
            &self.0.root,
            header.cwd.as_deref(),
            &header.id,
            version,
            self.0.compression,
        )
        .map_err(|e| {
            cause(
                format!("corrupt session log \"{path}\": header id cannot name a storage path"),
                e,
            )
        })?;
        if path != expected {
            let actual = self.0.fs.canonicalize(path).await;
            let expected_real = self.0.fs.canonicalize(&expected).await;
            let same = match (actual, expected_real) {
                (Ok(a), Ok(b)) => a == b,
                (Err(e), _) | (_, Err(e)) if is_not_found(e.as_ref()) => false,
                (Err(e), _) | (_, Err(e)) => return Err(e),
            };
            if !same {
                return Err(error(format!(
                    "corrupt session log \"{path}\": header id \"{}\" and cwd identify \"{expected}\"",
                    header.id
                )));
            }
        }
        check(signal)
    }
    async fn read_generation_header(
        &self,
        selected: &Selected,
        id: Option<&SessionId>,
        signal: Option<&AbortSignal>,
    ) -> Result<Option<HeaderMetadata>> {
        let text = match self.first_line(&selected.source_path, signal).await {
            Ok(text) => text,
            Err(e) => {
                check(signal)?;
                if is_not_found(e.as_ref()) {
                    return Ok(None);
                }
                return Err(e);
            }
        };
        check(signal)?;
        let Some(text) = text else {
            return Ok(None);
        };
        let value = match js_json_parse(&text) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        assert_no_retired_header_fields(&value)?;
        let version = match hoh_session_format::session_format_count(
            &value["version"],
            "Session format version",
        ) {
            Ok(version) => version,
            Err(_) => return Ok(None),
        };
        if version != selected.version {
            return Err(error(format!(
                "session generation filename identifies v{}, but its header identifies v{version}",
                selected.version
            )));
        }
        if version > SESSION_FORMAT_VERSION {
            return Err(Box::new(SessionFormatUnsupportedError::new(
                format!(
                    "{} (raw log: {})",
                    session_format_version_refusal(
                        &hoh_util_values::js_string(&value["id"]),
                        version
                    ),
                    selected.source_path
                ),
                Some(location(&selected.source_path)),
            )));
        }
        let catalog = hoh_session_format_catalog::session_format_catalog()?;
        let (header, status) = match catalog.read_header(&value) {
            hoh_session_format::SessionFormatHeaderReadResult::Current { header, .. } => {
                (header, SessionFormatStatus::Current)
            }
            hoh_session_format::SessionFormatHeaderReadResult::MigrationRequired {
                header, ..
            } => (header, SessionFormatStatus::MigrationRequired),
            _ => return Ok(None),
        };
        let header: SessionHeader = serde_json::from_value(Value::Object(header))?;
        self.assert_stored_identity(&selected.source_path, version, &header, id, signal)
            .await?;
        Ok(Some(HeaderMetadata { header, status }))
    }
    async fn read_listed_header(
        &self,
        selected: &Selected,
        signal: Option<&AbortSignal>,
    ) -> Result<Option<HeaderMetadata>> {
        match self.read_generation_header(selected, None, signal).await {
            Ok(metadata) => Ok(metadata),
            Err(failure)
                if failure.is::<SessionFormatUnsupportedError>()
                    || failure.is::<SessionPersistenceCorruptionError>() =>
            {
                Ok(None)
            }
            Err(failure) => Err(failure),
        }
    }
    async fn selected_generations(&self, signal: Option<&AbortSignal>) -> Result<Vec<Selected>> {
        let batch = self.all_dirs(signal).await?;
        let mut jobs: Vec<BoxFuture<'static, Result<Option<Selected>>>> = vec![];
        for dir in batch.dirs {
            let backend = self.clone();
            let signal = signal.cloned();
            jobs.push(Box::pin(async move {
                backend
                    .limited(backend.resolve_generation(&dir, signal.as_ref()))
                    .await
            }));
        }
        let results = futures_util::stream::iter(jobs)
            .buffered(self.0.list_concurrency)
            .collect::<Vec<_>>()
            .await;
        let mut selected = vec![];
        for result in results {
            if let Some(value) = result? {
                selected.push(value);
            }
        }
        if let Some(failure) = batch.failure {
            return Err(failure);
        }
        Ok(selected)
    }
    async fn corpus_revision(&self, signal: Option<&AbortSignal>) -> Result<String> {
        let generations = self.selected_generations(signal).await?;
        let mut revisions = vec![];
        for selected in generations {
            check(signal)?;
            let revision = match self
                .limited(self.0.fs.identity(&selected.source_path))
                .await
            {
                Ok(identity) => identity.revision(),
                Err(e) if is_not_found(e.as_ref()) => "missing".into(),
                Err(e) => return Err(e),
            };
            revisions.push((selected.source_path, revision));
        }
        Ok(corpus_digest(revisions))
    }
    async fn list_artifacts(
        &self,
        signal: Option<&AbortSignal>,
    ) -> Result<Vec<SessionPersistenceSnapshot>> {
        let generations = self.selected_generations(signal).await?;
        let previous = lock(&self.0.listed).clone();
        let mut jobs: Vec<BoxFuture<'static, Result<HeaderProbe>>> = vec![];
        for selected in generations {
            let backend = self.clone();
            let previous = previous.clone();
            let signal = signal.cloned();
            jobs.push(Box::pin(async move {
                let signal = signal.as_ref();
                backend
                    .limited(async {
                        check(signal)?;
                        let identity = match backend.0.fs.identity(&selected.source_path).await {
                            Ok(identity) => identity,
                            Err(e) if is_not_found(e.as_ref()) => {
                                return Ok((selected.clone(), None, None));
                            }
                            Err(e) => return Err(e),
                        };
                        check(signal)?;
                        let revision = SessionPersistenceRevision::from(identity.revision());
                        let metadata = if let Some(memo) = previous
                            .get(&selected.source_path)
                            .filter(|memo| memo.revision == revision)
                        {
                            memo.metadata.clone()
                        } else {
                            backend.read_listed_header(&selected, signal).await?
                        };
                        Ok((selected.clone(), Some(identity), metadata))
                    })
                    .await
            }));
        }
        let results = futures_util::stream::iter(jobs)
            .buffered(self.0.list_concurrency)
            .collect::<Vec<_>>()
            .await;
        let mut listed = IndexMap::new();
        let mut entries = vec![];
        let mut revisions = vec![];
        for result in results {
            let (selected, identity, metadata) = result?;
            revisions.push((
                selected.source_path.clone(),
                identity
                    .as_ref()
                    .map_or_else(|| "missing".into(), JsonlPhysicalIdentity::revision),
            ));
            if let Some(identity) = identity {
                listed.insert(
                    selected.source_path.clone(),
                    ListedHeader {
                        revision: SessionPersistenceRevision::from(identity.revision()),
                        metadata: metadata.clone(),
                    },
                );
                if let Some(metadata) = metadata {
                    entries.push((selected, identity, metadata));
                }
            }
        }
        let historical = entries.iter().any(|e| e.0.version < 4);
        let corpus = if historical {
            Some(corpus_digest(revisions))
        } else {
            None
        };
        let mut ids = std::collections::HashSet::new();
        let mut snapshots = vec![];
        for (selected, identity, metadata) in entries {
            if !ids.insert(metadata.header.id.clone()) {
                return Err(error(format!(
                    "duplicate JSONL session id \"{}\" appears in multiple project directories",
                    metadata.header.id
                )));
            }
            let revision = if selected.version < 4 {
                format!(
                    "{}:{}",
                    identity.revision(),
                    corpus.as_deref().unwrap_or("")
                )
            } else {
                identity.revision()
            };
            snapshots.push(snapshot(
                metadata.header,
                revision.into(),
                Some(metadata.status),
                Some(identity.size),
                Some(&selected.source_path),
            ));
        }
        check(signal)?;
        *lock(&self.0.listed) = listed;
        Ok(snapshots)
    }
    async fn require_stored(
        &self,
        id: &SessionId,
        signal: Option<&AbortSignal>,
        publish: bool,
    ) -> Result<Arc<StoredLog>> {
        let selected = self.find_log(id, signal).await?.ok_or_else(|| {
            Box::new(SessionPersistenceNotFoundError::new(id.clone())) as BoxError
        })?;
        if selected.version < 4 {
            return self
                .prepare_historical(&selected, id, signal, publish)
                .await.map_err(|failure| {
                if signal.is_some_and(AbortSignal::aborted) || is_io(failure.as_ref()) || failure.is::<crate::generation::JsonlGenerationSourceChangedError>() || failure.is::<SessionFormatUnsupportedError>() || failure.is::<SessionPersistenceCorruptionError>() {return failure;}
                if let Some(error)=failure.downcast_ref::<crate::generation::JsonlGenerationUnsupportedMigrationError>() {
                    return Box::new(SessionFormatUnsupportedError::new(format!("{error}; source v{} artifact remains unchanged (raw log: {})",error.from_version,selected.source_path),Some(location(&selected.source_path)))) as BoxError;
                }
                Box::new(SessionPersistenceCorruptionError::new(format!("session \"{id}\": stored log is corrupt: {} (raw log: {})",crate::error::js_error(failure.as_ref()),selected.source_path),failure)) as BoxError
            });
        }
        if selected.version != 4 {
            if self
                .read_generation_header(&selected, Some(id), signal)
                .await?
                .is_none()
            {
                let text = self.first_line(&selected.source_path, signal).await?;
                let detail = match text {
                    None => "empty or header-less session log",
                    Some(ref text) => match js_json_parse(text) {
                        Err(_) => "corrupt session log: header line is not valid JSON",
                        Ok(value) if !value.is_object() => {
                            "corrupt session log: first line is not a JSON object"
                        }
                        Ok(_) => "corrupt session log: first line is not a session header",
                    },
                };
                return Err(Box::new(SessionPersistenceCorruptionError::new(
                    format!(
                        "session \"{id}\": stored log is corrupt: Error: {detail} (raw log: {})",
                        selected.source_path
                    ),
                    error(detail),
                )));
            }
            return Err(Box::new(SessionFormatUnsupportedError::new(
                format!(
                    "{} (raw log: {})",
                    session_format_version_refusal(id.as_str(), selected.version),
                    selected.source_path
                ),
                Some(location(&selected.source_path)),
            )));
        }
        self.load_current(&selected.source_path, id, signal).await
    }
    async fn prepare_historical(
        &self,
        selected: &Selected,
        id: &SessionId,
        signal: Option<&AbortSignal>,
        publish: bool,
    ) -> Result<Arc<StoredLog>> {
        use crate::generation::*;
        let mut sources = vec![];
        for child in self.selected_generations(signal).await? {
            if let Some(metadata) = self.read_listed_header(&child, signal).await?
                && metadata.header.parent_session.as_ref() == Some(id)
                && metadata.header.origin.as_deref() == Some("subagent")
            {
                sources.push(crate::catalog_migration::CatalogSource {
                    header: metadata.header,
                    path: child.source_path,
                });
            }
        }
        let controller = hoh_timeout::AbortController::new();
        let fallback = controller.signal();
        let facts = Arc::new(
            crate::catalog_migration::prepare_catalog_facts(
                id,
                &sources,
                self.0.compression,
                signal.unwrap_or(&fallback),
            )
            .await?,
        );
        for failure in &facts.failures {
            self.0
                .ctx
                .logger()
                .warn(format!("{NAME}: session \"{id}\" catalog retained a child with unknown descriptor (raw log: {}): {}", failure.path, crate::error::js_error(failure.error.as_ref())));
        }
        let catalog = hoh_session_format_catalog::create_session_format_catalog_with_children(
            facts.facts.clone(),
        )?;
        let backend = self.clone();
        let parent_id = id.clone();
        let mut membership = sources
            .iter()
            .map(|source| source.path.clone())
            .collect::<Vec<_>>();
        membership.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
        let scheduler = crate::migration_verifier::VerificationScheduler::default();
        let prepared = prepare_jsonl_migration(PrepareJsonlMigrationOptions {
            source_path: selected.source_path.clone(),
            source_version: selected.version,
            current_path: selected.current_path.clone(),
            compression: self.0.compression,
            format: Arc::new(crate::catalog_adapter::CatalogAdapter(catalog)),
            verify_current_file: Arc::new(move |path, compression, id, count, prefix, signal| {
                let scheduler = scheduler.clone();
                Box::pin(async move {
                    crate::migration_verifier::verify_current_generation_in_worker(
                        &scheduler,
                        &path,
                        compression,
                        &id,
                        count,
                        prefix.as_ref(),
                        signal.as_ref(),
                    )
                    .await
                })
            }),
            validate_related_sources: Some(Arc::new(move || {
                let facts = facts.clone();
                let backend = backend.clone();
                let membership = membership.clone();
                let parent_id = parent_id.clone();
                Box::pin(async move {
                    let mut current = vec![];
                    for child in backend.selected_generations(None).await? {
                        if let Some(metadata) = backend.read_listed_header(&child, None).await?
                            && metadata.header.parent_session.as_ref() == Some(&parent_id)
                            && metadata.header.origin.as_deref() == Some("subagent")
                        {
                            current.push(child.source_path);
                        }
                    }
                    current.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
                    if let Some(path) = membership
                        .iter()
                        .filter(|p| !current.contains(p))
                        .chain(current.iter().filter(|p| !membership.contains(p)))
                        .next()
                    {
                        return Err(Box::new(JsonlGenerationSourceChangedError {
                            path: path.clone(),
                        }) as BoxError);
                    }
                    facts.validate().await?;
                    Ok(())
                })
            })),
            validate_historical_header: None,
            signal: signal.cloned(),
        })
        .await?;
        let artifact = &prepared.artifact;
        let meta: SessionHeader = serde_json::from_value(Value::Object(artifact.header.clone()))?;
        self.assert_stored_identity(
            &selected.source_path,
            selected.version,
            &meta,
            Some(id),
            signal,
        )
        .await?;
        let mut events = artifact
            .events
            .iter()
            .cloned()
            .map(|e| SessionEvent::from_value(e.into()))
            .collect::<Vec<_>>();
        validate_stored_events(&meta, &mut events, Some(&location(&selected.source_path)))?;
        if publish {
            prepared.publish().await?;
            return self.load_current(&selected.current_path, id, signal).await;
        }
        Ok(Arc::new(StoredLog {
            meta,
            cut: SessionLogOffset::new(artifact.inherited_event_count)?,
            events: events.into_iter().map(Arc::new).collect(),
            torn: None,
            recovered: vec![],
            revision: prepared.source_identity.revision().into(),
        }))
    }
    fn memo(
        &self,
        id: &SessionId,
        revision: &SessionPersistenceRevision,
    ) -> Option<Arc<StoredLog>> {
        let mut memo = lock(&self.0.memo);
        let value = memo
            .get(id)
            .filter(|value| value.revision == *revision)
            .cloned()?;
        memo.shift_remove(id);
        memo.insert(id.clone(), value.clone());
        Some(value)
    }
    fn memoize(&self, id: &SessionId, value: Arc<StoredLog>) {
        let mut memo = lock(&self.0.memo);
        memo.shift_remove(id);
        memo.insert(id.clone(), value);
        while memo.len() > 2 {
            memo.shift_remove_index(0);
        }
    }
    async fn load_current(
        &self,
        path: &str,
        id: &SessionId,
        signal: Option<&AbortSignal>,
    ) -> Result<Arc<StoredLog>> {
        check(signal)?;
        let probe = SessionPersistenceRevision::from(self.0.fs.identity(path).await?.revision());
        if let Some(memo) = self.memo(id, &probe) {
            return Ok(memo);
        }
        let current = crate::generation::read_stable_with(&self.0.fs, path, signal).await?;
        let parsed = self.decode_bytes(&current.bytes, signal).await;
        let (scan, torn, recovered) = match parsed {
            Ok(scan) => scan,
            Err(failure) => {
                check(signal)?;
                if failure.is::<SessionFormatUnsupportedError>() {
                    return Err(Box::new(SessionFormatUnsupportedError::new(
                        format!("{failure} (raw log: {path})"),
                        Some(location(path)),
                    )));
                }
                if is_io(failure.as_ref()) {
                    return Err(failure);
                }
                return Err(Box::new(SessionPersistenceCorruptionError::new(
                    format!(
                        "session \"{id}\": stored log is corrupt: {} (raw log: {path})",
                        crate::error::js_error(failure.as_ref())
                    ),
                    failure,
                )));
            }
        };
        check(signal)?;
        self.assert_stored_identity(path, 4, &scan.meta, Some(id), signal)
            .await?;
        check(signal)?;
        assert_stored_id(id, &scan.meta)?;
        let mut events = scan.events;
        validate_stored_events(&scan.meta, &mut events, Some(&location(path)))?;
        let stored = Arc::new(StoredLog {
            meta: scan.meta,
            cut: scan.inherited_event_count,
            events: events.into_iter().map(Arc::new).collect(),
            torn,
            recovered: recovered.into_iter().map(Arc::new).collect(),
            revision: current.identity.revision().into(),
        });
        self.memoize(id, stored.clone());
        Ok(stored)
    }
    pub(crate) async fn decode_bytes(
        &self,
        bytes: &[u8],
        signal: Option<&AbortSignal>,
    ) -> Result<(SessionLogScan, Option<u64>, Vec<SessionEvent>)> {
        check(signal)?;
        if self.0.compression == JsonlCompression::None {
            let scan = scan_log(bytes)?;
            let torn = (scan.committed_bytes < bytes.len()).then_some(scan.committed_bytes as u64);
            check(signal)?;
            return Ok((scan, torn, vec![]));
        }
        let frames = scan_zstd_frames(bytes, None)?;
        if frames.frames.is_empty() {
            return Err(error("empty or header-less Zstandard session log"));
        }
        let mut scanner = None;
        let mut deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
        for (index, frame) in frames.frames.iter().enumerate() {
            check(signal)?;
            let plaintext = decompress_zstd_frame(&bytes[frame.start..frame.end])
                .await
                .map_err(|e| {
                    cause(
                        format!(
                            "corrupt Zstandard session log: frame at byte {} failed validation",
                            frame.start
                        ),
                        e,
                    )
                })?;
            if index == 0 {
                assert_header_frame(&plaintext)?;
                scanner = Some(SessionLogScanner::new(
                    &plaintext,
                    hoh_session_format::SessionFormatRecovery::Recoverable,
                )?);
            } else {
                scanner
                    .as_mut()
                    .ok_or_else(|| error("empty or header-less Zstandard session log"))?
                    .write(&plaintext)?;
            }
            if index + 1 < frames.frames.len() && tokio::time::Instant::now() >= deadline {
                tokio::task::yield_now().await;
                check(signal)?;
                deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
            }
        }
        let mut scanner =
            scanner.ok_or_else(|| error("empty or header-less Zstandard session log"))?;
        let checkpoint = scanner.checkpoint()?;
        if checkpoint.input_bytes != checkpoint.committed_bytes {
            return Err(error(
                "corrupt Zstandard session log: complete frame contains a torn JSONL record",
            ));
        }
        if let Some(torn) = frames.torn_start {
            let recovered = decompress_zstd_prefix(&bytes[torn..])
                .await
                .unwrap_or_default();
            check(signal)?;
            scanner.write(&recovered)?;
            let scan = scanner.finish()?;
            let tail = scan.events[checkpoint.event_count.get() as usize..].to_vec();
            Ok((scan, Some(torn as u64), tail))
        } else {
            Ok((scanner.finish()?, None, vec![]))
        }
    }
    async fn acquire_lease(
        &self,
        header: &SessionHeader,
        dir: Option<&str>,
    ) -> Result<Arc<SessionWriteLease>> {
        let path = session_dir(&self.0.root, header.cwd.as_deref(), &header.id)?;
        if let Some(opposite) = self.find_opposite(&path).await? {
            return Err(self.encoding_mismatch(&opposite));
        }
        Ok(Arc::new(
            SessionWriteLease::acquire_with_io_hook(
                dir.unwrap_or(&path),
                &header.id,
                self.0.fs.hook.clone(),
            )
            .await?,
        ))
    }
    async fn materialize(
        &self,
        header: &SessionHeader,
        cut: SessionLogOffset,
        events: &[Arc<SessionEvent>],
    ) -> Result<()> {
        let project = project_dir(&self.0.root, header.cwd.as_deref())?;
        let dir = session_dir(&self.0.root, header.cwd.as_deref(), &header.id)?;
        let target = self.target(header)?;
        if let Some(path) = self.find_opposite(&dir).await? {
            return Err(self.encoding_mismatch(&path));
        }
        let header_line = format!(
            "{}\n",
            js_json_stringify(&to_header_line(header, header.is_seeded.then_some(cut))?)
        );
        let plain = events.iter().map(|e| (**e).clone()).collect::<Vec<_>>();
        let body = if events.is_empty() {
            None
        } else {
            Some(format!("{}\n", event_lines(&plain)?))
        };
        let content = if self.0.compression == JsonlCompression::Zstd {
            let mut bytes = compress_zstd_frame(header_line).await?;
            if let Some(body) = body {
                bytes.extend(compress_zstd_frame(body).await?);
            }
            bytes
        } else {
            format!("{header_line}{}", body.unwrap_or_default()).into_bytes()
        };
        self.0.fs.mkdir(&self.0.root).await?;
        self.0
            .fs
            .sync_dir(&node_path::dirname(&self.0.root))
            .await?;
        self.0.fs.mkdir(&project).await?;
        self.0.fs.sync_dir(&self.0.root).await?;
        self.0.fs.mkdir(&dir).await?;
        self.0.fs.sync_dir(&project).await?;
        if self.resolve_generation(&dir, None).await?.is_some() {
            return Err(error(format!(
                "refusing to materialize \"{}\": a log already exists on disk (open it instead)",
                header.id
            )));
        }
        let temporary = format!("{target}.{}.tmp", random_token(6)?);
        let mut file = self.0.fs.open(&temporary, "wx").await?;
        let written = async {
            file.write(&content).await?;
            file.sync().await
        }
        .await;
        let closed = file.close().await;
        closed?;
        written?;
        if let Err(failure) = self.0.fs.link(&temporary, &target).await {
            self.0.fs.remove(&temporary).await?;
            return Err(failure);
        }
        self.0.fs.sync_dir(&dir).await?;
        let _ = self.0.fs.remove(&temporary).await;
        Ok(())
    }
    async fn append_lines(
        &self,
        header: &SessionHeader,
        events: &[Arc<SessionEvent>],
    ) -> Result<()> {
        let plain = events.iter().map(|e| (**e).clone()).collect::<Vec<_>>();
        let text = format!("{}\n", event_lines(&plain)?);
        let content = if self.0.compression == JsonlCompression::Zstd {
            compress_zstd_frame(text).await?
        } else {
            text.into_bytes()
        };
        let path = self.target(header)?;
        let mut file = self.0.fs.open(&path, "a").await?;
        let before = file.size().await?;
        let written = async {
            file.write(&content).await?;
            file.sync().await
        }
        .await;
        if let Err(failure) = written {
            let rollback = async {
                file.close().await?;
                let mut handle = self.0.fs.open(&path, "r+").await?;
                let result = async {
                    handle.truncate(before).await?;
                    handle.sync().await
                }
                .await;
                handle.close().await?;
                result
            }
            .await;
            if let Err(rollback) = rollback {
                return Err(Box::new(JsonlError::Aggregate {
                    message: format!("failed to roll back append to \"{path}\""),
                    errors: vec![failure, rollback],
                }));
            }
            return Err(failure);
        }
        file.close().await
    }
    pub(crate) fn install(&self, ctx: &Context) -> Result<()> {
        let tracker = self.0.tracker.clone();
        ctx.effect("session-persistence-jsonl open handles", move || {
            Undo::future(move || async move { tracker.close_all().await })
        })?;
        let tracker = self.0.tracker.clone();
        let log = ctx.logger();
        ctx.on_contained::<hoh_session::SessionEventEmitted>(move|event|{
            if let Some(writer)=tracker.writer(event.session.id()) {let id=event.session.id().clone();let log=log.clone();writer.enqueue_live(&event.event,Arc::new(move|failure|log.warn(format!("session-persistence: background write for session \"{id}\" failed (buffered events retained): Error: {failure}"))));}
            Ok(())
        })?;
        let tracker = self.0.tracker.clone();
        ctx.on_contained::<hoh_session::SessionFlush>(move |event| {
            if let Some(writer) = tracker.writer(event.session.id()) {
                event.wait_for(async move {
                    writer.drain_live().await?;
                    writer.flush(None).await
                });
            }
            Ok(())
        })?;
        let tracker = self.0.tracker.clone();
        let log = ctx.logger();
        let owner = ctx.clone();
        ctx.on_contained::<hoh_session::SessionDisposed>(move|event|{if let Some(writer)=tracker.writer(event.session.id()) {let id=event.session.id().clone();let log=log.clone();owner.spawn("session-persistence-jsonl final drain",move|_|async move {if let Err(failure)=writer.close().await {log.warn(format!("session-persistence: final drain for session \"{id}\" failed: Error: {failure}"));}Ok(())})?;}Ok(())})?;
        Ok(())
    }
}

fn assert_header_frame(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() || bytes.iter().position(|&b| b == b'\n') != Some(bytes.len() - 1) {
        Err(error(
            "corrupt Zstandard session log: first frame is not exactly one header line",
        ))
    } else {
        Ok(())
    }
}
fn corpus_digest(revisions: Vec<(String, String)>) -> String {
    use sha2::{Digest, Sha256};
    let mut values: IndexMap<_, _> = revisions.into_iter().collect();
    values.sort_by(|a, _, b, _| a.encode_utf16().cmp(b.encode_utf16()));
    let mut hash = Sha256::new();
    for (path, revision) in values {
        hash.update(js_json_stringify(&serde_json::json!([path, revision])).as_bytes());
    }
    crate::error::hex(&hash.finalize())
}

impl SessionPersistence for JsonlSessionPersistence {
    fn identity(&self) -> SessionPersistenceIdentity {
        self.0.identity.clone()
    }
    fn create<'a>(
        &'a self,
        header: &'a SessionHeader,
        options: Option<&'a SessionPersistenceCreateOptions>,
    ) -> BoxFuture<'a, Result<Arc<dyn SessionHandle>>> {
        let admission = check(options.and_then(|o| o.signal.as_ref()))
            .and_then(|_| materialize_create_header(header));
        let options = options.cloned().unwrap_or_default();
        let backend = self.clone();
        Box::pin(async move {
            let header = admission?;
            to_header_line(&header, options.inherited_event_count)?;
            let cut = options.inherited_event_count.unwrap_or_default();
            backend.ensure_root_encoding().await?;
            check(options.signal.as_ref())?;
            if backend.0.tracker.has_pending(&header.id)
                || backend
                    .find_log(&header.id, options.signal.as_ref())
                    .await?
                    .is_some()
            {
                return Err(Box::new(SessionAlreadyExistsError::new(header.id)) as BoxError);
            }
            check(options.signal.as_ref())?;
            backend.0.tracker.register_created(header.clone(), cut)?;
            let handle = JsonlSessionHandle::new(
                Arc::new(backend.clone()),
                header.id.clone(),
                header,
                SessionAccess::Write,
                StorageHandleState {
                    inherited_event_count: cut,
                    ..Default::default()
                },
                None,
            );
            Ok(backend.0.tracker.adopt(handle) as Arc<dyn SessionHandle>)
        })
    }
    fn open<'a>(
        &'a self,
        id: &'a SessionId,
        access: SessionAccess,
        options: Option<&'a SessionPersistenceOpenOptions>,
    ) -> BoxFuture<'a, Result<Arc<dyn SessionHandle>>> {
        Box::pin(async move {
            let signal = options.and_then(|o| o.signal.as_ref());
            check(signal)?;
            self.ensure_root_encoding().await?;
            check(signal)?;
            if access == SessionAccess::Read
                && let Some(pending) = self.0.tracker.pending_of(id)
            {
                let handle = JsonlSessionHandle::new(
                    Arc::new(self.clone()),
                    id.clone(),
                    pending.header,
                    access,
                    StorageHandleState {
                        inherited_event_count: pending.inherited_event_count,
                        ..Default::default()
                    },
                    None,
                );
                return Ok(self.0.tracker.adopt(handle) as Arc<dyn SessionHandle>);
            }
            let mut claim = if access == SessionAccess::Write {
                self.0.tracker.claim_write(id)?;
                Some(WriteClaimGuard {
                    tracker: self.0.tracker.clone(),
                    id: id.clone(),
                    active: true,
                })
            } else {
                None
            };
            let mut lease = None;
            let opened = async {
                if access == SessionAccess::Write {
                    let selected = self.find_log(id, signal).await?.ok_or_else(|| {
                        Box::new(SessionPersistenceNotFoundError::new(id.clone())) as BoxError
                    })?;
                    let provisional = SessionHeader::new(id.clone(), 0);
                    lease = Some(
                        self.acquire_lease(
                            &provisional,
                            Some(&node_path::dirname(&selected.current_path)),
                        )
                        .await?,
                    );
                }
                let stored = self
                    .require_stored(id, signal, access == SessionAccess::Write)
                    .await?;
                check(signal)?;
                let state = StorageHandleState {
                    cursor: if access == SessionAccess::Write {
                        stored.events.len() as u64
                    } else {
                        0
                    },
                    materialized: true,
                    torn_truncate_to: if access == SessionAccess::Write {
                        stored.torn
                    } else {
                        None
                    },
                    recovered_tail: if access == SessionAccess::Write {
                        Some(stored.recovered.clone())
                    } else {
                        None
                    },
                    inherited_event_count: stored.cut,
                    primed: if access == SessionAccess::Write
                        || self
                            .find_log(id, signal)
                            .await?
                            .is_some_and(|s| s.version < 4)
                    {
                        Some(SessionHandleReadResult {
                            events: stored.events.clone(),
                            event_state: SessionSeedEventState::SharedFrozen,
                        })
                    } else {
                        None
                    },
                };
                Ok(JsonlSessionHandle::new(
                    Arc::new(self.clone()),
                    id.clone(),
                    stored.meta.clone(),
                    access,
                    state,
                    lease.clone(),
                ))
            }
            .await;
            match opened {
                Ok(handle) => {
                    let handle = self.0.tracker.adopt(handle);
                    if let Some(claim) = &mut claim {
                        claim.active = false;
                    }
                    Ok(handle as Arc<dyn SessionHandle>)
                }
                Err(failure) => {
                    if let Some(lease) = lease {
                        let released = lease.release().await;
                        if let Err(release) = released {
                            return Err(Box::new(JsonlError::Aggregate {
                                message: format!(
                                    "session \"{id}\": write open failed and its lock release failed"
                                ),
                                errors: vec![failure, release],
                            }) as BoxError);
                        }
                    }
                    Err(failure)
                }
            }
        })
    }
    fn trash<'a>(&'a self, id: &'a SessionId) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Share the backend's writer gate and the interprocess lease with normal opens.
            self.0.tracker.claim_write(id)?;
            let _claim = WriteClaimGuard {
                tracker: self.0.tracker.clone(),
                id: id.clone(),
                active: true,
            };
            let Some(selected) = self.find_log(id, None).await? else {
                return Ok(());
            };
            let source = std::path::Path::new(&selected.source_path)
                .parent()
                .ok_or_else(|| error("session log has no parent directory"))?;
            let lease = SessionWriteLease::acquire(source.to_string_lossy().as_ref(), id).await?;
            // A name match is insufficient on case-insensitive filesystems.
            self.read_generation_header(&selected, Some(id), None)
                .await?
                .ok_or_else(|| error("session header disappeared before deletion"))?;
            let trash = std::path::Path::new(&self.0.root).join("trash");
            match std::fs::create_dir(&trash) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e.into()),
            }
            // Pin both directories. Renaming through the descriptors cannot be redirected by
            // replacing `trash` with a symlink after the no-follow open.
            use std::os::fd::AsRawFd;
            use std::os::unix::fs::OpenOptionsExt;
            let open_dir = |path: &std::path::Path| {
                std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(
                        (rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::NOFOLLOW).bits()
                            as i32,
                    )
                    .open(path)
            };
            let trash_dir = open_dir(&trash)?;
            let parent = source
                .parent()
                .ok_or_else(|| error("session directory has no parent"))?;
            let source_name = source
                .file_name()
                .ok_or_else(|| error("session directory has no name"))?;
            let source_parent = open_dir(parent)?;
            let anchored_source =
                std::path::PathBuf::from(format!("/proc/self/fd/{}", source_parent.as_raw_fd()))
                    .join(source_name);
            if !std::fs::symlink_metadata(&anchored_source)?
                .file_type()
                .is_dir()
            {
                return Err(error("session directory is not a real directory"));
            }
            let token = random_token(16)?;
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis();
            let destination =
                std::path::PathBuf::from(format!("/proc/self/fd/{}", trash_dir.as_raw_fd())).join(
                    format!(
                        "{stamp}-{}-{token}",
                        crate::format::encode_segment(id.as_str())?
                    ),
                );
            tokio::fs::rename(anchored_source, destination).await?;
            // The rename is the commit point. A close-hook failure must not report a
            // failed deletion after the directory has already moved to trash.
            let _ = lease.release().await;
            self.0
                .memo
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .shift_remove(id);
            self.0
                .listed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
            Ok(())
        })
    }
    fn is_trashed<'a>(&'a self, id: &'a SessionId) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            use std::os::fd::AsRawFd;
            use std::os::unix::fs::OpenOptionsExt;
            let path = std::path::Path::new(&self.0.root).join("trash");
            let directory = match std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(
                    (rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::NOFOLLOW).bits() as i32,
                )
                .open(&path)
            {
                Ok(directory) => directory,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(e) => return Err(e.into()),
            };
            let encoded = encode_segment(id.as_str())?;
            let mut dirs =
                tokio::fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd())).await?;
            while let Some(entry) = dirs.next_entry().await? {
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                let Some((stamp, tail)) = name.split_once('-') else {
                    continue;
                };
                let Some((key, nonce)) = tail.rsplit_once('-') else {
                    continue;
                };
                if key == encoded
                    && !stamp.is_empty()
                    && stamp.bytes().all(|c| c.is_ascii_digit())
                    && nonce.len() == 32
                    && nonce.bytes().all(|c| c.is_ascii_hexdigit())
                {
                    return Ok(true);
                }
            }
            Ok(false)
        })
    }
    fn flush(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(self.0.tracker.flush_all())
    }
    fn prepare_delete<'a>(&'a self, id: &'a SessionId) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.0.tracker.prepare_delete(id))
    }
    fn stat<'a>(
        &'a self,
        id: &'a SessionId,
        options: Option<&'a SessionPersistenceStatOptions>,
    ) -> BoxFuture<'a, Result<Option<SessionPersistenceSnapshot>>> {
        Box::pin(async move {
            let signal = options.and_then(|o| o.signal.as_ref());
            check(signal)?;
            self.ensure_root_encoding().await?;
            check(signal)?;
            if let Some(pending) = self.0.tracker.pending_of(id) {
                return Ok(Some(snapshot(
                    pending.header,
                    pending.revision,
                    None,
                    None,
                    None,
                )));
            }
            let Some(selected) = self.find_log(id, signal).await? else {
                return Ok(None);
            };
            let Some(metadata) = self
                .read_generation_header(&selected, Some(id), signal)
                .await?
            else {
                return Ok(None);
            };
            let identity = match self.0.fs.identity(&selected.source_path).await {
                Ok(identity) => identity,
                Err(e) => {
                    check(signal)?;
                    if is_not_found(e.as_ref()) {
                        return Ok(None);
                    }
                    return Err(e);
                }
            };
            check(signal)?;
            let revision = if selected.version < 4 {
                format!(
                    "{}:{}",
                    identity.revision(),
                    self.corpus_revision(signal).await?
                )
            } else {
                identity.revision()
            };
            Ok(Some(snapshot(
                metadata.header,
                revision.into(),
                Some(metadata.status),
                Some(identity.size),
                Some(&selected.source_path),
            )))
        })
    }
    fn list<'a>(
        &'a self,
        options: Option<&'a SessionPersistenceListOptions>,
    ) -> BoxFuture<'a, Result<Vec<SessionPersistenceSnapshot>>> {
        Box::pin(async move {
            let signal = options.and_then(|o| o.signal.as_ref());
            check(signal)?;
            let pending = self.0.tracker.pending_entries();
            self.ensure_root_encoding().await?;
            check(signal)?;
            let mut snapshots = self.list_artifacts(signal).await?;
            let ids = snapshots
                .iter()
                .map(|s| s.header.id.clone())
                .collect::<std::collections::HashSet<_>>();
            for (id, pending) in pending {
                if !ids.contains(&id) {
                    snapshots.push(snapshot(pending.header, pending.revision, None, None, None));
                }
            }
            check(signal)?;
            Ok(snapshots)
        })
    }
}
impl JsonlHandleStorage for JsonlSessionPersistence {
    fn persist_batch<'a>(
        &'a self,
        header: &'a SessionHeader,
        events: &'a [Arc<SessionEvent>],
        materialized: bool,
        cut: SessionLogOffset,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            lock(&self.0.memo).shift_remove(&header.id);
            self.ensure_root_encoding().await?;
            if materialized {
                self.append_lines(header, events).await?;
            } else {
                self.materialize(header, cut, events).await?;
                self.0.tracker.materialized(&header.id);
            }
            Ok(())
        })
    }
    fn persist_header<'a>(
        &'a self,
        header: &'a SessionHeader,
        cut: SessionLogOffset,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            lock(&self.0.memo).shift_remove(&header.id);
            self.ensure_root_encoding().await?;
            self.materialize(header, cut, &[]).await?;
            self.0.tracker.materialized(&header.id);
            Ok(())
        })
    }
    fn truncate_torn_tail<'a>(
        &'a self,
        header: &'a SessionHeader,
        offset: u64,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            lock(&self.0.memo).shift_remove(&header.id);
            let path = self.target(header)?;
            let mut file = self.0.fs.open(&path, "r+").await?;
            let result = async {
                file.truncate(offset).await?;
                file.sync().await
            }
            .await;
            file.close().await?;
            result?;
            self.0.ctx.logger().warn(format!("{NAME}: session \"{}\" recovered from a torn tail; incomplete tail bytes were discarded",header.id));
            Ok(())
        })
    }
    fn resolve_current_log<'a>(
        &'a self,
        id: &'a SessionId,
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<Option<String>>> {
        Box::pin(async move {
            self.ensure_root_encoding().await?;
            check(signal)?;
            let Some(selected) = self.find_log(id, signal).await? else {
                return Ok(None);
            };
            if selected.version == 4 {
                Ok(Some(selected.source_path))
            } else if selected.version < 4 {
                Ok(None)
            } else {
                Err(Box::new(SessionFormatUnsupportedError::new(
                    format!(
                        "{} (raw log: {})",
                        session_format_version_refusal(id.as_str(), selected.version),
                        selected.source_path
                    ),
                    Some(location(&selected.source_path)),
                )) as BoxError)
            }
        })
    }
    fn read_stored_log<'a>(
        &'a self,
        path: &'a str,
        id: &'a SessionId,
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<SessionHandleReadResult>> {
        Box::pin(async move {
            let stored = self.load_current(path, id, signal).await?;
            Ok(SessionHandleReadResult {
                events: stored.events.clone(),
                event_state: SessionSeedEventState::SharedFrozen,
            })
        })
    }
    fn has_pending_session(&self, id: &SessionId) -> bool {
        self.0.tracker.has_pending(id)
    }
    fn acquire_write_lease<'a>(
        &'a self,
        header: &'a SessionHeader,
    ) -> BoxFuture<'a, Result<Arc<SessionWriteLease>>> {
        Box::pin(self.acquire_lease(header, None))
    }
    fn release_handle(&self, handle: &JsonlSessionHandle, materialized: bool) {
        self.0.tracker.release(handle, materialized)
    }
}
pub struct JsonlSessionPersistencePlugin;
impl Plugin for JsonlSessionPersistencePlugin {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = ();
    async fn apply(
        ctx: Context,
        _deps: (),
        config: Cfg<Config>,
    ) -> std::result::Result<(), BoxError> {
        let backend = JsonlSessionPersistence::new(&ctx, (*config.get()).clone()).await?;
        backend.install(&ctx)?;
        ctx.provide::<SessionPersistenceService>(Arc::new(backend))
            .await?;
        Ok(())
    }
}
