//! Stable reads and immutable, verified, no-overwrite generation publication.
pub use crate::io::JsonlPhysicalIdentity;
use crate::{
    JsonlError, Result,
    error::{cause, error, hex},
    format::*,
    io::{FileSystem, IoHook, NodeFileError, is_io, lock},
    zstd::*,
};
use futures_util::future::BoxFuture;
use hoh_error::{BoxError, SharedError};
use hoh_session_format::{
    SessionFormatArtifact, SessionFormatError, SessionFormatEvent, SessionFormatHeader,
    SessionFormatRecovery, SessionFormatRestore,
};
use hoh_timeout::AbortSignal;
use hoh_util_values::{deep_equal_json, js_json_parse, js_json_stringify};
use hoh_util_workspace_path::node_path;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    sync::{Arc, Mutex},
};

pub trait JsonlGenerationRestore: Send {
    fn decode_row(&mut self, value: Value) -> Result<()>;
    fn finish(self: Box<Self>) -> Result<SessionFormatArtifact>;
}
impl JsonlGenerationRestore for SessionFormatRestore {
    fn decode_row(&mut self, value: Value) -> Result<()> {
        Ok(SessionFormatRestore::decode_row(self, value)?)
    }
    fn finish(self: Box<Self>) -> Result<SessionFormatArtifact> {
        Ok(SessionFormatRestore::finish(*self)?)
    }
}
pub trait JsonlGenerationFormatAdapter: Send + Sync + 'static {
    fn current_version(&self) -> u64;
    fn create_restore(&self, header: &Value) -> Result<Box<dyn JsonlGenerationRestore>>;
    fn encode_header(
        &self,
        header: &SessionFormatHeader,
        inherited_event_count: u64,
    ) -> Result<Value>;
    fn encode_event(&self, event: &SessionFormatEvent) -> Result<Value>;
    fn is_unsupported_migration_error(&self, error: &(dyn std::error::Error + 'static)) -> bool {
        error
            .downcast_ref::<SessionFormatError>()
            .is_some_and(SessionFormatError::is_unsupported_migration)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonlExpectedPrefix {
    pub bytes: usize,
    pub digest: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonlVerifiedGeneration {
    pub identity: JsonlPhysicalIdentity,
    pub bytes: usize,
    pub digest: String,
}
#[derive(Clone, Debug)]
pub struct StablePhysicalFile {
    pub bytes: Vec<u8>,
    pub identity: JsonlPhysicalIdentity,
}
pub type ValidateRelatedSources = Arc<dyn Fn() -> BoxFuture<'static, Result<()>> + Send + Sync>;
pub type ValidateHistoricalHeader =
    Arc<dyn Fn(Value) -> BoxFuture<'static, Result<()>> + Send + Sync>;
pub type VerifyCurrentFile = Arc<
    dyn Fn(
            String,
            JsonlCompression,
            String,
            usize,
            Option<JsonlExpectedPrefix>,
            Option<AbortSignal>,
        ) -> BoxFuture<'static, Result<JsonlVerifiedGeneration>>
        + Send
        + Sync,
>;
#[derive(Clone)]
pub struct PrepareJsonlMigrationOptions {
    pub source_path: String,
    pub source_version: u64,
    pub current_path: String,
    pub compression: JsonlCompression,
    pub format: Arc<dyn JsonlGenerationFormatAdapter>,
    pub verify_current_file: VerifyCurrentFile,
    pub validate_related_sources: Option<ValidateRelatedSources>,
    pub validate_historical_header: Option<ValidateHistoricalHeader>,
    pub signal: Option<AbortSignal>,
}
#[derive(Debug, thiserror::Error)]
#[error("historical session generation changed during migration: \"{path}\"")]
pub struct JsonlGenerationSourceChangedError {
    pub path: String,
}
#[derive(Debug, thiserror::Error)]
#[error("{reason}")]
pub struct JsonlGenerationUnsupportedMigrationError {
    pub from_version: u64,
    #[source]
    pub reason: BoxError,
}
#[derive(Debug, thiserror::Error)]
#[error("current session generation already exists at \"{path}\": {reason}")]
pub struct JsonlGenerationTargetConflictError {
    pub path: String,
    #[source]
    pub reason: BoxError,
}
impl JsonlGenerationSourceChangedError {
    pub fn name(&self) -> &'static str {
        "JsonlGenerationSourceChangedError"
    }
}
impl JsonlGenerationUnsupportedMigrationError {
    pub fn name(&self) -> &'static str {
        "JsonlGenerationUnsupportedMigrationError"
    }
}
impl JsonlGenerationTargetConflictError {
    pub fn name(&self) -> &'static str {
        "JsonlGenerationTargetConflictError"
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationBarrierPhase {
    BeforeSourceCheck,
    AfterPublication,
}
pub type GenerationBarrier =
    Arc<dyn Fn(GenerationBarrierPhase, u32) -> BoxFuture<'static, Result<()>> + Send + Sync>;
#[derive(Clone, Default)]
pub struct JsonlGenerationRuntimeOverrides {
    pub io_hook: Option<IoHook>,
    pub random_token: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    pub barrier: Option<GenerationBarrier>,
}
#[derive(Clone, Default)]
pub struct JsonlGenerationRuntime {
    overrides: JsonlGenerationRuntimeOverrides,
}
impl JsonlGenerationRuntime {
    fn fs(&self) -> FileSystem {
        FileSystem {
            hook: self.overrides.io_hook.clone(),
        }
    }
    pub async fn read_stable(
        &self,
        path: &str,
        signal: Option<&AbortSignal>,
    ) -> Result<StablePhysicalFile> {
        read_stable_with(&self.fs(), path, signal).await
    }
    pub async fn verify(
        &self,
        path: &str,
        compression: JsonlCompression,
        id: &str,
        count: usize,
        prefix: Option<&JsonlExpectedPrefix>,
    ) -> Result<JsonlVerifiedGeneration> {
        verify_current_with(&self.fs(), path, compression, id, count, prefix).await
    }
    pub async fn prepare(
        &self,
        options: PrepareJsonlMigrationOptions,
    ) -> Result<PreparedJsonlMigration> {
        let source_name = generation_log_filename(options.source_version, options.compression)?;
        let target_name =
            generation_log_filename(options.format.current_version(), options.compression)?;
        if node_path::basename(&options.source_path) != source_name {
            return Err(error(format!(
                "resolved JSONL source path must end with \"{source_name}\": {}",
                options.source_path
            )));
        }
        if node_path::basename(&options.current_path) != target_name {
            return Err(error(format!(
                "current JSONL generation path must end with \"{target_name}\": {}",
                options.current_path
            )));
        }
        if node_path::dirname(&options.source_path) != node_path::dirname(&options.current_path) {
            return Err(error(
                "source and current JSONL generations must share one Session directory",
            ));
        }
        if options.source_version >= options.format.current_version() {
            return Err(error(format!(
                "migration preparation requires a historical source, got v{}",
                options.source_version
            )));
        }
        let source = self
            .read_stable(&options.source_path, options.signal.as_ref())
            .await?;
        let artifact = match decode_streaming_migration(
            &source.bytes,
            options.compression,
            options.source_version,
            options.format.as_ref(),
            options.validate_historical_header.as_ref(),
            options.signal.as_ref(),
        )
        .await
        {
            Ok(artifact) => artifact,
            Err(failure) => {
                if options
                    .format
                    .is_unsupported_migration_error(failure.as_ref())
                {
                    return Err(Box::new(JsonlGenerationUnsupportedMigrationError {
                        from_version: options.source_version,
                        reason: failure,
                    }));
                }
                return Err(failure);
            }
        };
        if artifact.header.get("version").and_then(Value::as_f64)
            != Some(options.format.current_version() as f64)
        {
            return Err(error(format!(
                "format migration returned v{}, expected v{}",
                artifact
                    .header
                    .get("version")
                    .map_or_else(|| "undefined".into(), hoh_util_values::js_string),
                options.format.current_version()
            )));
        }
        if let Some(validate) = &options.validate_related_sources {
            validate().await?;
        }
        let artifact = Arc::new(artifact);
        let identity = source.identity;
        Ok(PreparedJsonlMigration {
            source_identity: identity.clone(),
            artifact: artifact.clone(),
            inner: Arc::new(Prepared {
                options,
                runtime: self.clone(),
                identity,
                artifact,
                publication: Mutex::new(None),
            }),
        })
    }
}
pub fn create_jsonl_generation_runtime(
    overrides: JsonlGenerationRuntimeOverrides,
) -> JsonlGenerationRuntime {
    JsonlGenerationRuntime { overrides }
}
pub async fn read_stable_jsonl_file(
    path: &str,
    signal: Option<&AbortSignal>,
) -> Result<StablePhysicalFile> {
    read_stable_with(&FileSystem::default(), path, signal).await
}
fn check(signal: Option<&AbortSignal>) -> Result<()> {
    if let Some(signal) = signal {
        signal.throw_if_aborted()?;
    }
    Ok(())
}
pub(crate) async fn read_stable_with(
    fs: &FileSystem,
    path: &str,
    signal: Option<&AbortSignal>,
) -> Result<StablePhysicalFile> {
    check(signal)?;
    let mut before = fs.identity(path).await?;
    for attempt in 0..2 {
        let mut bytes = fs.read(path).await?;
        check(signal)?;
        let after = fs.identity(path).await?;
        if before == after {
            check(signal)?;
            return Ok(StablePhysicalFile {
                bytes,
                identity: after,
            });
        }
        if attempt == 1 {
            bytes.truncate(before.size.min(bytes.len() as u64) as usize);
            return Ok(StablePhysicalFile {
                bytes,
                identity: before,
            });
        }
        before = after;
    }
    Err(error("stable JSONL snapshot did not settle"))
}
struct MigratingRows {
    restore: Option<Box<dyn JsonlGenerationRestore>>,
    fragments: Vec<u8>,
    row_index: u64,
    issue: Option<SharedError>,
}
impl MigratingRows {
    fn write(&mut self, chunk: &[u8]) -> Result<()> {
        let mut start = 0;
        for (offset, &byte) in chunk.iter().enumerate() {
            if byte != b'\n' {
                continue;
            }
            self.fragments.extend_from_slice(&chunk[start..offset]);
            let line = std::mem::take(&mut self.fragments);
            self.consume(&line)?;
            start = offset + 1;
        }
        self.fragments.extend_from_slice(&chunk[start..]);
        Ok(())
    }
    fn consume(&mut self, line: &[u8]) -> Result<()> {
        self.row_index += 1;
        let row = match js_json_parse(&String::from_utf8_lossy(line)) {
            Ok(row) => row,
            Err(failure) => {
                if self.issue.is_none() {
                    self.issue = Some(SharedError::new(cause(
                        format!(
                            "corrupt session log: row {} is not valid JSON",
                            self.row_index
                        ),
                        failure,
                    )));
                }
                return Ok(());
            }
        };
        if let Some(issue) = &self.issue {
            return if row["type"] == "turn/end" {
                Err(Box::new(issue.clone()))
            } else {
                Ok(())
            };
        }
        self.restore
            .as_mut()
            .ok_or_else(|| error("migration decoder is finished"))?
            .decode_row(row)
    }
    fn finish(mut self) -> Result<SessionFormatArtifact> {
        self.restore
            .take()
            .ok_or_else(|| error("migration decoder is finished"))?
            .finish()
    }
}
async fn start_migration(
    header: &[u8],
    version: u64,
    format: &dyn JsonlGenerationFormatAdapter,
    validate: Option<&ValidateHistoricalHeader>,
) -> Result<MigratingRows> {
    let header = js_json_parse(&String::from_utf8_lossy(&header[..header.len() - 1])).map_err(
        |failure| {
            cause(
                "corrupt session log: header line is not valid JSON",
                failure,
            )
        },
    )?;
    if !header.is_object() {
        return Err(error(
            "corrupt session log: first line is not a JSON object",
        ));
    }
    let actual = hoh_session_format::session_format_count(&header["version"], "header version")
        .map_err(|_| {
            error("corrupt session log: header version is not a non-negative safe integer")
        })?;
    if actual != version {
        return Err(error(format!(
            "resolved JSONL source filename identifies v{version}, but its header identifies v{actual}"
        )));
    }
    if let Some(validate) = validate {
        validate(header.clone()).await?;
    }
    let restore = format.create_restore(&header)?;
    Ok(MigratingRows {
        restore: Some(restore),
        fragments: vec![],
        row_index: 0,
        issue: None,
    })
}
async fn consume(
    rows: &mut MigratingRows,
    bytes: &[u8],
    signal: Option<&AbortSignal>,
) -> Result<()> {
    check(signal)?;
    let mut deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
    for chunk in bytes.chunks(1024 * 1024) {
        rows.write(chunk)?;
        if tokio::time::Instant::now() >= deadline {
            tokio::task::yield_now().await;
            check(signal)?;
            deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
        }
    }
    Ok(())
}
async fn decode_streaming_migration(
    bytes: &[u8],
    compression: JsonlCompression,
    version: u64,
    format: &dyn JsonlGenerationFormatAdapter,
    validate: Option<&ValidateHistoricalHeader>,
    signal: Option<&AbortSignal>,
) -> Result<SessionFormatArtifact> {
    check(signal)?;
    if compression == JsonlCompression::None {
        let header_end = bytes
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(|| error("empty or header-less session log"))?;
        let mut rows = start_migration(&bytes[..=header_end], version, format, validate).await?;
        check(signal)?;
        let body_end = bytes
            .iter()
            .rposition(|&b| b == b'\n')
            .unwrap_or(header_end);
        if body_end > header_end {
            consume(&mut rows, &bytes[header_end + 1..=body_end], signal).await?;
        }
        return rows.finish();
    }
    let scan = scan_zstd_frames(bytes, None)?;
    if scan.frames.is_empty() {
        return Err(error("empty or header-less Zstandard session log"));
    }
    let first = scan.frames[0];
    let header = decompress_zstd_frame(&bytes[first.start..first.end])
        .await
        .map_err(|e| {
            cause(
                "corrupt Zstandard session log: frame at byte 0 failed validation",
                e,
            )
        })?;
    assert_header_frame(&header)?;
    let mut rows = start_migration(&header, version, format, validate).await?;
    check(signal)?;
    for frame in &scan.frames[1..] {
        let decoded = decompress_zstd_frame(&bytes[frame.start..frame.end])
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
        consume(&mut rows, &decoded, signal).await?;
    }
    if !rows.fragments.is_empty() {
        return Err(error(
            "corrupt Zstandard session log: complete frame contains a torn JSONL record",
        ));
    }
    if let Some(start) = scan.torn_start {
        let recovered = decompress_zstd_prefix(&bytes[start..])
            .await
            .unwrap_or_default();
        check(signal)?;
        if let Some(end) = recovered.iter().rposition(|&b| b == b'\n') {
            consume(&mut rows, &recovered[..=end], signal).await?;
        }
    }
    rows.finish()
}
pub async fn read_decoded_jsonl_source(
    path: &str,
    version: u64,
    compression: JsonlCompression,
    format: &dyn JsonlGenerationFormatAdapter,
    signal: Option<&AbortSignal>,
) -> Result<(SessionFormatArtifact, JsonlPhysicalIdentity)> {
    let source = read_stable_jsonl_file(path, signal).await?;
    let artifact =
        match decode_streaming_migration(&source.bytes, compression, version, format, None, signal)
            .await
        {
            Ok(artifact) => artifact,
            Err(failure) => {
                if signal.is_some_and(AbortSignal::aborted) || failure.is::<SessionFormatError>() {
                    return Err(failure);
                }
                return Err(Box::new(SessionFormatError::new_with_cause(
                    crate::error::js_error(failure.as_ref()),
                    failure,
                )));
            }
        };
    Ok((artifact, source.identity))
}
pub async fn prepare_jsonl_migration(
    options: PrepareJsonlMigrationOptions,
) -> Result<PreparedJsonlMigration> {
    JsonlGenerationRuntime::default().prepare(options).await
}
#[derive(Clone)]
pub struct PreparedJsonlMigration {
    pub source_identity: JsonlPhysicalIdentity,
    pub artifact: Arc<SessionFormatArtifact>,
    inner: Arc<Prepared>,
}
type PublicationCompletion =
    tokio::sync::watch::Receiver<Option<std::result::Result<JsonlPhysicalIdentity, SharedError>>>;
struct Prepared {
    options: PrepareJsonlMigrationOptions,
    runtime: JsonlGenerationRuntime,
    identity: JsonlPhysicalIdentity,
    artifact: Arc<SessionFormatArtifact>,
    publication: Mutex<Option<PublicationCompletion>>,
}
impl PreparedJsonlMigration {
    pub fn publish(&self) -> BoxFuture<'static, Result<JsonlPhysicalIdentity>> {
        let mut state = lock(&self.inner.publication);
        let mut receiver = if let Some(receiver) = &*state {
            receiver.clone()
        } else {
            let (sender, receiver) = tokio::sync::watch::channel(None);
            *state = Some(receiver.clone());
            let prepared = self.inner.clone();
            tokio::spawn(async move {
                let result = publish_prepared(&prepared).await.map_err(SharedError::new);
                let _ = sender.send(Some(result));
            });
            receiver
        };
        Box::pin(async move {
            loop {
                let result = receiver.borrow().clone();
                if let Some(result) = result {
                    return result.map_err(|e| Box::new(e) as BoxError);
                }
                if receiver.changed().await.is_err() {
                    return Err(error("migration publication ended without a result"));
                }
            }
        })
    }
}
fn assert_header_frame(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() || bytes.iter().position(|&b| b == b'\n') != Some(bytes.len() - 1) {
        return Err(error(
            "corrupt Zstandard session log: first frame is not exactly one header line",
        ));
    }
    Ok(())
}
pub async fn verify_jsonl_current_generation(
    path: &str,
    compression: JsonlCompression,
    id: &str,
    count: usize,
    prefix: Option<&JsonlExpectedPrefix>,
) -> Result<JsonlVerifiedGeneration> {
    verify_current_with(&FileSystem::default(), path, compression, id, count, prefix).await
}
async fn verify_current_with(
    fs: &FileSystem,
    path: &str,
    compression: JsonlCompression,
    id: &str,
    count: usize,
    prefix: Option<&JsonlExpectedPrefix>,
) -> Result<JsonlVerifiedGeneration> {
    let before = fs.identity(path).await?;
    let bytes = fs.read(path).await?;
    let after = fs.identity(path).await?;
    if let Some(prefix) = prefix {
        if bytes.len() < prefix.bytes {
            return Err(error(
                "target bytes are shorter than the migrated generation",
            ));
        }
        let digest = hex(&Sha256::digest(&bytes[..prefix.bytes]));
        if digest != prefix.digest {
            return Err(error(
                "target bytes do not begin with the migrated generation",
            ));
        }
        return Ok(JsonlVerifiedGeneration {
            identity: after,
            bytes: prefix.bytes,
            digest,
        });
    }
    if before != after {
        return Err(error(
            "current session generation changed during verification",
        ));
    }
    let scan = decode_current(&bytes, compression).await?;
    let mut events = scan.events;
    let location = Arc::new(hoh_session_persistence::SessionLocation {
        kind: "jsonl".into(),
        path: path.into(),
    });
    hoh_session_persistence::validate_stored_events(&scan.meta, &mut events, Some(&location))?;
    if scan.meta.id.as_str() != id {
        return Err(error(format!(
            "current session generation contains id \"{}\", expected \"{id}\"",
            scan.meta.id
        )));
    }
    if events.len() != count {
        return Err(error(format!(
            "current session generation contains {} events, expected {count}",
            events.len()
        )));
    }
    let seed = events.iter().cloned().map(Arc::new).collect::<Vec<_>>();
    hoh_session::Session::from_restore(
        scan.meta.id.clone(),
        &seed,
        serde_json::to_value(&scan.meta)?,
        scan.inherited_event_count,
        hoh_session::SessionSeedEventState::Detached,
        None,
    )?;
    assert_assistant_streams(&events)?;
    Ok(JsonlVerifiedGeneration {
        identity: after,
        bytes: bytes.len(),
        digest: hex(&Sha256::digest(&bytes)),
    })
}
async fn decode_current(bytes: &[u8], compression: JsonlCompression) -> Result<SessionLogScan> {
    let (mut scanner, input) = if compression == JsonlCompression::None {
        let end = bytes
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(|| error("empty or header-less session log"))?;
        let mut scanner = SessionLogScanner::new(&bytes[..=end], SessionFormatRecovery::Strict)?;
        scanner.write(&bytes[end + 1..])?;
        (scanner, bytes.len())
    } else {
        let frames = scan_zstd_frames(bytes, None)?;
        if frames.frames.is_empty() {
            return Err(error("empty or header-less Zstandard session log"));
        }
        if frames.torn_start.is_some() {
            return Err(error("current session generation has a torn physical tail"));
        }
        let first = frames.frames[0];
        let header = decompress_zstd_frame(&bytes[first.start..first.end]).await?;
        assert_header_frame(&header)?;
        let mut scanner = SessionLogScanner::new(&header, SessionFormatRecovery::Strict)?;
        for frame in &frames.frames[1..] {
            scanner.write(&decompress_zstd_frame(&bytes[frame.start..frame.end]).await?)?;
        }
        let input = scanner.checkpoint()?.input_bytes;
        (scanner, input)
    };
    let scan = scanner.finish()?;
    if scan.committed_bytes != input {
        return Err(error("current session generation has a torn physical tail"));
    }
    Ok(scan)
}
fn assert_assistant_streams(events: &[hoh_session::SessionEvent]) -> Result<()> {
    for (index, event) in events.iter().enumerate() {
        if !matches!(
            event.event_type(),
            "assistant/message" | "assistant/attempt"
        ) {
            continue;
        }
        let mut assembler = hoh_llm::BlockAssembler::new();
        let timed = (|| -> Result<_> {
            let timed = hoh_llm::expand_assistant_stream_value(&event.data()["stream"])?;
            for member in &timed {
                assembler.push_value(member["chunk"].clone())?;
            }
            Ok(timed)
        })()
        .map_err(|e| {
            cause(
                format!(
                    "seed {} at index {index} has an invalid embedded stream",
                    event.event_type()
                ),
                e,
            )
        })?;
        if event.event_type() == "assistant/attempt" || timed.is_empty() {
            continue;
        }
        let blocks = if event.data()["interrupted"] == true {
            assembler.interrupted_blocks()
        } else {
            assembler.blocks()?
        };
        if !deep_equal_json(
            &event.data()["message"]["content"],
            &serde_json::to_value(blocks)?,
        ) {
            return Err(error(format!(
                "seed assistant/message at index {index} content disagrees with its embedded stream"
            )));
        }
        let usage = assembler.usage().map(serde_json::to_value).transpose()?;
        if event.data().get("usage") != usage.as_ref() {
            return Err(error(format!(
                "seed assistant/message at index {index} usage disagrees with its embedded stream"
            )));
        }
        let replay = assembler
            .replay_state()?
            .map(serde_json::to_value)
            .transpose()?;
        if event.data()["message"]["source"].get("replayState") != replay.as_ref() {
            return Err(error(format!(
                "seed assistant/message at index {index} replay state disagrees with its embedded stream"
            )));
        }
    }
    Ok(())
}
struct Stage {
    path: String,
    bytes: usize,
    digest: String,
}
struct CompressedSink(tokio::sync::mpsc::Sender<Vec<u8>>);
impl Write for CompressedSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.blocking_send(bytes.to_vec()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "migration stage consumer closed",
            )
        })?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
async fn write_stage(prepared: &Prepared) -> Result<Stage> {
    let fs = prepared.runtime.fs();
    let options = &prepared.options;
    let suffix = log_suffix(options.compression);
    let dir = node_path::dirname(&options.current_path);
    let (path, mut file) = loop {
        let token = if let Some(token) = &prepared.runtime.overrides.random_token {
            token()
        } else {
            let mut bytes = [0; 8];
            getrandom::fill(&mut bytes).map_err(|e| error(e.to_string()))?;
            bytes.iter().map(|b| format!("{b:02x}")).collect()
        };
        let path = node_path::join(&[&dir, &format!("session.migration.{token}{suffix}.tmp")]);
        match fs.open(&path, "wx").await {
            Ok(file) => break (path, file),
            Err(e)
                if e.downcast_ref::<NodeFileError>()
                    .is_some_and(|e| e.code == Some("EEXIST")) => {}
            Err(e) => return Err(e),
        }
    };
    let mut hash = Sha256::new();
    let mut total = 0;
    let written = async {
        let header = format!(
            "{}\n",
            js_json_stringify(&options.format.encode_header(
                &prepared.artifact.header,
                prepared.artifact.inherited_event_count
            )?)
        );
        let header = if options.compression == JsonlCompression::Zstd {
            compress_zstd_frame(header).await?
        } else {
            header.into_bytes()
        };
        file.write(&header).await?;
        hash.update(&header);
        total += header.len();
        if !prepared.artifact.events.is_empty() {
            let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1);
            let (output_tx, mut output_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
            let compression = options.compression;
            let compressor = tokio::task::spawn_blocking(move || -> Result<()> {
                if compression == JsonlCompression::None {
                    while let Some(bytes) = input_rx.blocking_recv() {
                        output_tx
                            .blocking_send(bytes)
                            .map_err(|_| error("migration stage consumer closed"))?;
                    }
                    return Ok(());
                }
                let mut encoder = ::zstd::stream::Encoder::new(CompressedSink(output_tx), 0)?;
                encoder.include_checksum(true)?;
                while let Some(bytes) = input_rx.blocking_recv() {
                    encoder.write_all(&bytes)?;
                }
                encoder.finish()?;
                Ok(())
            });
            let producer = async {
                let mut chunk = Vec::new();
                for event in &prepared.artifact.events {
                    let line = format!(
                        "{}\n",
                        js_json_stringify(&options.format.encode_event(event)?)
                    );
                    if !chunk.is_empty() && chunk.len() + line.len() > 1024 * 1024 {
                        input_tx
                            .send(std::mem::take(&mut chunk))
                            .await
                            .map_err(|_| error("migration stage compressor closed"))?;
                        tokio::task::yield_now().await;
                    }
                    chunk.extend(line.as_bytes());
                }
                input_tx
                    .send(chunk)
                    .await
                    .map_err(|_| error("migration stage compressor closed"))?;
                drop(input_tx);
                Ok::<_, BoxError>(())
            };
            let consumer = async {
                let mut pending = Vec::new();
                while let Some(chunk) = output_rx.recv().await {
                    pending.extend(chunk);
                    if pending.len() >= 4 * 1024 * 1024 {
                        file.write(&pending).await?;
                        hash.update(&pending);
                        total += pending.len();
                        pending.clear();
                    }
                }
                if !pending.is_empty() {
                    file.write(&pending).await?;
                    hash.update(&pending);
                    total += pending.len();
                }
                Ok::<_, BoxError>(())
            };
            let result = tokio::try_join!(producer, consumer);
            drop(output_rx);
            let compression_result = compressor.await?;
            result?;
            compression_result?;
        }
        file.sync().await
    }
    .await;
    let closed = file.close().await;
    let failure = match (written, closed) {
        (Ok(()), Ok(())) => None,
        (Err(a), Err(b)) => Some(Box::new(JsonlError::Aggregate {
            message: format!("failed to write and close migration stage \"{path}\""),
            errors: vec![a, b],
        }) as BoxError),
        (Err(e), _) | (_, Err(e)) => Some(e),
    };
    if let Some(failure) = failure {
        return Err(cleanup(&fs, &path, failure).await);
    }
    Ok(Stage {
        path,
        bytes: total,
        digest: hex(&hash.finalize()),
    })
}
async fn cleanup(fs: &FileSystem, path: &str, primary: BoxError) -> BoxError {
    if let Err(cleanup) = fs.remove(path).await {
        return Box::new(JsonlError::Aggregate {
            message: format!(
                "failed to clean migration temporary \"{path}\" after an earlier failure"
            ),
            errors: vec![primary, cleanup],
        });
    }
    primary
}
async fn publish_prepared(prepared: &Prepared) -> Result<JsonlPhysicalIdentity> {
    tokio::task::yield_now().await;
    let fs = prepared.runtime.fs();
    let options = &prepared.options;
    let mut staged = write_stage(prepared).await?;
    let result = async {
        let id = prepared
            .artifact
            .header
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let count = prepared.artifact.events.len();
        let verified = (options.verify_current_file)(
            staged.path.clone(),
            options.compression,
            id.into(),
            count,
            None,
            None,
        )
        .await?;
        if verified.bytes != staged.bytes || verified.digest != staged.digest {
            return Err(error(
                "staged session generation changed during verification",
            ));
        }
        if let Some(barrier) = &prepared.runtime.overrides.barrier {
            barrier(GenerationBarrierPhase::BeforeSourceCheck, 1).await?;
        }
        if let Some(validate) = &options.validate_related_sources {
            validate().await?;
        }
        if fs.identity(&options.source_path).await? != prepared.identity {
            return Err(Box::new(JsonlGenerationSourceChangedError {
                path: options.source_path.clone(),
            }) as BoxError);
        }
        let published = match fs.link(&staged.path, &options.current_path).await {
            Ok(()) => {
                fs.sync_dir(&node_path::dirname(&options.current_path))
                    .await?;
                true
            }
            Err(e)
                if e.downcast_ref::<NodeFileError>()
                    .is_some_and(|e| e.code == Some("EEXIST")) =>
            {
                false
            }
            Err(e) => return Err(e),
        };
        if let Some(barrier) = &prepared.runtime.overrides.barrier {
            barrier(GenerationBarrierPhase::AfterPublication, 1).await?;
        }
        let identity = if published {
            let _ = fs.remove(&staged.path).await;
            staged.path.clear();
            fs.identity(&options.current_path).await?
        } else {
            let inspected = async {
                let expected = node_path::basename(&options.current_path);
                let names = fs
                    .read_dir(&node_path::dirname(&options.current_path))
                    .await?;
                if !names.iter().any(|entry| entry.0 == expected)
                    && let Some(entry) = names
                        .iter()
                        .find(|entry| entry.0.eq_ignore_ascii_case(&expected))
                {
                    return Err(error(format!(
                        "target resolves to noncanonical directory entry \"{}\"",
                        entry.0
                    )));
                }
                let metadata = fs.symlink_metadata(&options.current_path).await?;
                if metadata.is_symlink() || !metadata.is_file() {
                    return Err(error(format!(
                        "target is a {}",
                        if metadata.is_symlink() {
                            "symbolic link"
                        } else {
                            "non-regular file"
                        }
                    )));
                }
                let prefix = JsonlExpectedPrefix {
                    bytes: staged.bytes,
                    digest: staged.digest.clone(),
                };
                let candidate = (options.verify_current_file)(
                    options.current_path.clone(),
                    options.compression,
                    id.into(),
                    count,
                    Some(prefix),
                    None,
                )
                .await?;
                if candidate.bytes != staged.bytes || candidate.digest != staged.digest {
                    return Err(error("target bytes differ from the migrated generation"));
                }
                Ok(candidate.identity)
            }
            .await;
            let identity = match inspected {
                Ok(identity) => identity,
                Err(e) if is_io(e.as_ref()) => return Err(e),
                Err(e) => {
                    return Err(Box::new(JsonlGenerationTargetConflictError {
                        path: options.current_path.clone(),
                        reason: e,
                    }) as BoxError);
                }
            };
            let _ = fs.remove(&staged.path).await;
            staged.path.clear();
            identity
        };
        Ok(identity)
    }
    .await;
    match result {
        Ok(identity) => Ok(identity),
        Err(failure) => {
            if !staged.path.is_empty() {
                Err(cleanup(&fs, &staged.path, failure).await)
            } else {
                Err(failure)
            }
        }
    }
}
