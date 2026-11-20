//! Bounded source admission, shared content conversion, and caller-owned delivery.

use crate::{
    Config, OfficeConversionFormat, OfficeImage, OfficeKitError, OfficeKitErrorCode,
    OfficeKitGeneration, OfficeKitKey, OfficeKitPriority, OfficeKitSource,
    OfficeRecalculationFormat,
};
use hoh_error::{BoxError, SharedError};
use hoh_timeout::{AbortController, AbortSignal};
use indexmap::{IndexMap, IndexSet};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard},
};
use tokio::sync::oneshot;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum OfficeOperation {
    Pdf,
    Images {
        #[serde(skip_serializing_if = "Option::is_none")]
        pages: Option<Vec<u32>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        sheet: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        range: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        dpi: Option<u32>,
        #[serde(rename = "maxImages")]
        max_images: u64,
        #[serde(rename = "maxPixels")]
        max_pixels: u64,
        #[serde(rename = "maxDimension")]
        max_dimension: u64,
        #[serde(rename = "maxBytes")]
        max_bytes: u64,
    },
    Convert {
        format: OfficeConversionFormat,
        #[serde(skip_serializing_if = "Option::is_none")]
        sheet: Option<String>,
    },
    Recalculate {
        format: OfficeRecalculationFormat,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OfficeOperationOutput {
    Pdf {
        pdf: Vec<u8>,
        missing_fonts: Vec<String>,
    },
    Images {
        images: Vec<OfficeImage>,
        page_count: u32,
        dpi: u32,
        missing_fonts: Vec<String>,
    },
    File {
        data: Vec<u8>,
        page_count: Option<u32>,
        missing_fonts: Vec<String>,
    },
}

#[derive(Clone, Debug)]
pub struct QueueRequest {
    pub extension: String,
    pub priority: OfficeKitPriority,
    pub source: OfficeKitSource,
    pub operation: OfficeOperation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueResult {
    pub output: OfficeOperationOutput,
    pub cache_key: OfficeKitKey,
    pub generation: OfficeKitGeneration,
}

pub type ConvertFuture = Pin<
    Box<dyn Future<Output = std::result::Result<OfficeOperationOutput, BoxError>> + Send + 'static>,
>;
pub type Convert =
    dyn Fn(Vec<u8>, QueueRequest, AbortSignal) -> ConvertFuture + Send + Sync + 'static;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JobState {
    Queued,
    Running,
    Finished,
}

struct Reader {
    source: String,
    priority: OfficeKitPriority,
    sender: oneshot::Sender<std::result::Result<QueueResult, SharedError>>,
}

struct Job {
    request: QueueRequest,
    operation: String,
    controller: AbortController,
    readers: IndexMap<u64, Reader>,
    sources: IndexSet<String>,
    priority: OfficeKitPriority,
    state: JobState,
    started_background: bool,
    reserved: u64,
    digest: Option<OfficeKitKey>,
}

#[derive(Default)]
struct State {
    ready: IndexMap<OfficeKitKey, QueueResult>,
    aliases: IndexMap<String, OfficeKitKey>,
    sources: HashMap<String, u64>,
    digests: HashMap<OfficeKitKey, u64>,
    queue: Vec<u64>,
    jobs: HashMap<u64, Job>,
    readers_to_jobs: HashMap<u64, u64>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    cached_bytes: u64,
    source_bytes: u64,
    running: u64,
    background: u64,
    readers: u64,
    next_job: u64,
    next_reader: u64,
    disposed: bool,
}

struct Inner {
    config: Config,
    generation: OfficeKitGeneration,
    convert: Arc<Convert>,
    state: Mutex<State>,
}

#[derive(Clone)]
pub struct ConversionQueue(Arc<Inner>);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn shared(error: OfficeKitError) -> SharedError {
    SharedError::new(Box::new(error) as BoxError)
}

fn busy() -> SharedError {
    shared(OfficeKitError::new(
        OfficeKitErrorCode::Busy,
        "The document converter has reached its admission limit.",
    ))
}

fn unavailable() -> SharedError {
    shared(OfficeKitError::new(
        OfficeKitErrorCode::Unavailable,
        "The document converter is unavailable.",
    ))
}

impl ConversionQueue {
    pub fn new(
        config: Config,
        generation: OfficeKitGeneration,
        convert: impl Fn(Vec<u8>, QueueRequest, AbortSignal) -> ConvertFuture + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(Inner {
            config,
            generation,
            convert: Arc::new(convert),
            state: Mutex::new(State::default()),
        }))
    }

    pub async fn read(
        &self,
        request: QueueRequest,
        signal: Option<AbortSignal>,
    ) -> std::result::Result<QueueResult, SharedError> {
        if let Some(signal) = &signal {
            signal.throw_if_aborted()?;
        }
        let operation = operation_text(&request.operation)?;
        let source = source_identity(&request, &operation)?;
        let (sender, receiver) = oneshot::channel();
        let reader_id;
        {
            let mut state = lock(&self.0.state);
            if state.disposed {
                return Err(unavailable());
            }
            if request
                .source
                .bytes
                .is_some_and(|bytes| bytes > self.0.config.max_input_bytes)
            {
                return Err(shared(OfficeKitError::new(
                    OfficeKitErrorCode::InputTooLarge,
                    "The Office source exceeds maxInputBytes.",
                )));
            }
            if let Some(key) = state.aliases.get(&source).cloned()
                && let Some(cached) = state.ready.shift_remove(&key)
            {
                state.ready.insert(key.clone(), cached.clone());
                state.aliases.shift_remove(&source);
                state.aliases.insert(source, key);
                return Ok(cached);
            }
            let reader_limit = if request.priority == OfficeKitPriority::Background {
                self.0.config.max_readers.saturating_sub(1)
            } else {
                self.0.config.max_readers
            };
            if state.readers >= reader_limit {
                return Err(busy());
            }
            if request.priority == OfficeKitPriority::Background
                && self.0.config.max_background_conversions == 0
            {
                return Err(busy());
            }
            let job_id = if let Some(job_id) = state.sources.get(&source).copied() {
                job_id
            } else {
                if state.queue.len() as u64 >= self.0.config.max_queued_jobs {
                    let obsolete = (request.priority == OfficeKitPriority::Foreground)
                        .then(|| {
                            state.queue.iter().copied().find(|id| {
                                state.jobs.get(id).is_some_and(|job| {
                                    job.priority == OfficeKitPriority::Background
                                })
                            })
                        })
                        .flatten();
                    if let Some(obsolete) = obsolete {
                        fail_job(&mut state, obsolete, busy());
                    } else {
                        return Err(busy());
                    }
                }
                let id = state.next_job;
                state.next_job = state.next_job.wrapping_add(1);
                state.sources.insert(source.clone(), id);
                state.queue.push(id);
                state.jobs.insert(
                    id,
                    Job {
                        request: request.clone(),
                        operation,
                        controller: AbortController::new(),
                        readers: IndexMap::new(),
                        sources: IndexSet::from_iter([source.clone()]),
                        priority: request.priority,
                        state: JobState::Queued,
                        started_background: false,
                        reserved: 0,
                        digest: None,
                    },
                );
                id
            };
            reader_id = state.next_reader;
            state.next_reader = state.next_reader.wrapping_add(1);
            let job = state
                .jobs
                .get_mut(&job_id)
                .expect("source index names a job");
            if request.priority == OfficeKitPriority::Foreground {
                job.priority = OfficeKitPriority::Foreground;
            }
            job.sources.insert(source.clone());
            job.readers.insert(
                reader_id,
                Reader {
                    source,
                    priority: request.priority,
                    sender,
                },
            );
            state.readers_to_jobs.insert(reader_id, job_id);
            state.readers += 1;
        }
        self.0.drain();
        match signal {
            Some(signal) => {
                tokio::select! {
                    result = receiver => result.unwrap_or_else(|_| Err(unavailable())),
                    reason = signal.cancelled() => {
                        self.0.cancel_reader(reader_id);
                        Err(reason.clone())
                    }
                }
            }
            None => receiver.await.unwrap_or_else(|_| Err(unavailable())),
        }
    }

    pub async fn dispose(&self) {
        let tasks = {
            let mut state = lock(&self.0.state);
            state.disposed = true;
            let jobs: Vec<_> = state.jobs.keys().copied().collect();
            for job in jobs {
                fail_job(&mut state, job, unavailable());
            }
            state.ready.clear();
            state.aliases.clear();
            state.cached_bytes = 0;
            std::mem::take(&mut state.tasks)
        };
        for task in tasks {
            let _ = task.await;
        }
    }
}

impl Inner {
    fn reservation(&self, job: &Job) -> u64 {
        job.request
            .source
            .bytes
            .unwrap_or(self.config.max_input_bytes)
            .max(1)
    }

    fn drain(self: &Arc<Self>) {
        let mut state = lock(&self.state);
        state.tasks.retain(|task| !task.is_finished());
        while !state.disposed && state.running < self.config.max_concurrent_conversions {
            let foreground_waiting = state.queue.iter().any(|id| {
                state
                    .jobs
                    .get(id)
                    .is_some_and(|job| job.priority == OfficeKitPriority::Foreground)
            });
            let next = state.queue.iter().copied().find(|id| {
                let Some(job) = state.jobs.get(id) else {
                    return false;
                };
                let reservation = self.reservation(job);
                let background_limit = self.config.max_background_conversions.min(
                    self.config
                        .max_concurrent_conversions
                        .saturating_sub(1)
                        .max(1),
                );
                (!foreground_waiting || job.priority == OfficeKitPriority::Foreground)
                    && state.source_bytes + reservation <= self.config.max_source_bytes
                    && (job.priority == OfficeKitPriority::Foreground
                        || state.background < background_limit)
            });
            let Some(job_id) = next else {
                break;
            };
            state.queue.retain(|id| *id != job_id);
            let reservation = self.reservation(state.jobs.get(&job_id).expect("queued job exists"));
            let background = state
                .jobs
                .get(&job_id)
                .is_some_and(|job| job.priority == OfficeKitPriority::Background);
            let job = state.jobs.get_mut(&job_id).expect("queued job exists");
            job.state = JobState::Running;
            job.started_background = background;
            job.reserved = reservation;
            state.running += 1;
            state.source_bytes += reservation;
            if background {
                state.background += 1;
            }
            let inner = self.clone();
            state.tasks.push(tokio::spawn(async move {
                inner.execute(job_id, reservation).await;
                inner.settle(job_id);
            }));
        }
    }

    async fn execute(self: &Arc<Self>, job_id: u64, reserved: u64) {
        let (request, signal) = {
            let state = lock(&self.state);
            let Some(job) = state.jobs.get(&job_id) else {
                return;
            };
            (job.request.clone(), job.controller.signal())
        };
        let outcome = async {
            signal.throw_if_aborted()?;
            let input = (request.source.read)(signal.clone(), reserved)
                .await
                .map_err(SharedError::new)?;
            signal.throw_if_aborted()?;
            if input.version != request.source.version {
                return Err(shared(OfficeKitError::new(
                    OfficeKitErrorCode::SourceChanged,
                    "The source changed while waiting for conversion.",
                )));
            }
            if input.bytes.len() as u64 > reserved {
                return Err(shared(OfficeKitError::new(
                    OfficeKitErrorCode::InputTooLarge,
                    "The source exceeds its reserved read capacity.",
                )));
            }
            let (operation, extension) = {
                let state = lock(&self.state);
                let Some(job) = state.jobs.get(&job_id) else {
                    return Ok(());
                };
                (job.operation.clone(), job.request.extension.clone())
            };
            let key = content_key(&self.generation, &extension, &operation, &input.bytes);
            let action = {
                let mut state = lock(&self.state);
                if let Some(cached) = state.ready.shift_remove(&key) {
                    state.ready.insert(key.clone(), cached.clone());
                    finish_job(&self.config, &mut state, job_id, cached);
                    DigestAction::Done
                } else if let Some(existing) = state.digests.get(&key).copied().filter(|existing| {
                    state.jobs.get(existing).is_some_and(|job| {
                        job.state != JobState::Finished && !job.controller.signal().aborted()
                    })
                }) {
                    join_jobs(&mut state, job_id, existing);
                    DigestAction::Done
                } else {
                    state.digests.insert(key.clone(), job_id);
                    if let Some(job) = state.jobs.get_mut(&job_id) {
                        job.digest = Some(key.clone());
                    }
                    DigestAction::Convert(key)
                }
            };
            let DigestAction::Convert(key) = action else {
                return Ok(());
            };
            let converted = (self.convert)(input.bytes, request, signal.clone())
                .await
                .map_err(SharedError::new)?;
            signal.throw_if_aborted()?;
            let result = QueueResult {
                output: converted,
                cache_key: key,
                generation: self.generation.clone(),
            };
            let mut state = lock(&self.state);
            retain(&self.config, &mut state, result.clone());
            finish_job(&self.config, &mut state, job_id, result);
            Ok(())
        }
        .await;
        if let Err(error) = outcome {
            let mut state = lock(&self.state);
            fail_job(&mut state, job_id, error);
        }
    }

    fn settle(self: &Arc<Self>, job_id: u64) {
        {
            let mut state = lock(&self.state);
            if let Some(job) = state.jobs.remove(&job_id) {
                state.running = state.running.saturating_sub(1);
                state.source_bytes = state.source_bytes.saturating_sub(job.reserved);
                if job.started_background {
                    state.background = state.background.saturating_sub(1);
                }
                if let Some(key) = job.digest
                    && state.digests.get(&key) == Some(&job_id)
                {
                    state.digests.remove(&key);
                }
                for source in job.sources {
                    if state.sources.get(&source) == Some(&job_id) {
                        state.sources.remove(&source);
                    }
                }
            }
        }
        self.drain();
    }

    fn cancel_reader(self: &Arc<Self>, reader_id: u64) {
        {
            let mut state = lock(&self.state);
            let Some(job_id) = state.readers_to_jobs.remove(&reader_id) else {
                return;
            };
            let (reader, empty, queued) = {
                let Some(job) = state.jobs.get_mut(&job_id) else {
                    return;
                };
                let reader = job.readers.shift_remove(&reader_id);
                if job.state == JobState::Queued {
                    job.priority = if job
                        .readers
                        .values()
                        .any(|reader| reader.priority == OfficeKitPriority::Foreground)
                    {
                        OfficeKitPriority::Foreground
                    } else {
                        OfficeKitPriority::Background
                    };
                }
                (
                    reader,
                    job.readers.is_empty(),
                    job.state == JobState::Queued,
                )
            };
            if let Some(reader) = reader {
                state.readers = state.readers.saturating_sub(1);
                remove_source_without_reader(&mut state, job_id, &reader.source);
            }
            if empty {
                if let Some(job) = state.jobs.get(&job_id) {
                    job.controller.abort();
                }
                if queued {
                    state.queue.retain(|id| *id != job_id);
                    state.jobs.remove(&job_id);
                }
            }
        }
        self.drain();
    }
}

enum DigestAction {
    Done,
    Convert(OfficeKitKey),
}

fn operation_text(operation: &OfficeOperation) -> std::result::Result<String, SharedError> {
    let value =
        serde_json::to_value(operation).map_err(|error| SharedError::new(Box::new(error)))?;
    Ok(hoh_util_values::js_json_stringify(&value))
}

fn source_identity(
    request: &QueueRequest,
    operation: &str,
) -> std::result::Result<String, SharedError> {
    let value = serde_json::json!([
        request.source.key.as_str(),
        request.source.version,
        request.extension,
        operation
    ]);
    Ok(hoh_util_values::js_json_stringify(&value))
}

fn content_key(
    generation: &OfficeKitGeneration,
    extension: &str,
    operation: &str,
    bytes: &[u8],
) -> OfficeKitKey {
    let mut hash = Sha256::new();
    hash.update(extension.as_bytes());
    hash.update([0]);
    hash.update(operation.as_bytes());
    hash.update([0]);
    hash.update(bytes);
    let digest = hash.finalize();
    let mut text = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(text, "{byte:02x}").expect("writing to String cannot fail");
    }
    OfficeKitKey::from(format!("{generation}:{text}"))
}

fn output_bytes(output: &OfficeOperationOutput) -> u64 {
    match output {
        OfficeOperationOutput::Pdf { pdf, .. } => pdf.len() as u64,
        OfficeOperationOutput::File { data, .. } => data.len() as u64,
        OfficeOperationOutput::Images { images, .. } => {
            images.iter().map(|image| image.png.len() as u64).sum()
        }
    }
}

fn retain(config: &Config, state: &mut State, result: QueueResult) {
    let bytes = output_bytes(&result.output);
    if bytes > config.max_cached_bytes {
        return;
    }
    while state.ready.len() as u64 >= config.max_cached_entries
        || state.cached_bytes + bytes > config.max_cached_bytes
    {
        let Some((key, oldest)) = state.ready.shift_remove_index(0) else {
            break;
        };
        state.cached_bytes -= output_bytes(&oldest.output);
        state.aliases.retain(|_, digest| digest != &key);
    }
    state.ready.insert(result.cache_key.clone(), result);
    state.cached_bytes += bytes;
}

fn finish_job(config: &Config, state: &mut State, job_id: u64, result: QueueResult) {
    let retained = state.ready.contains_key(&result.cache_key);
    let Some(job) = state.jobs.get_mut(&job_id) else {
        return;
    };
    job.state = JobState::Finished;
    let digest = job.digest.take();
    let sources = std::mem::take(&mut job.sources);
    let readers = std::mem::take(&mut job.readers);
    release_digest(state, job_id, digest);
    for source in &sources {
        if state.sources.get(source) == Some(&job_id) {
            state.sources.remove(source);
        }
        if retained {
            state.aliases.shift_remove(source);
            state
                .aliases
                .insert(source.clone(), result.cache_key.clone());
            while state.aliases.len() as u64 > config.max_source_entries {
                state.aliases.shift_remove_index(0);
            }
        }
    }
    for (reader_id, reader) in readers {
        state.readers_to_jobs.remove(&reader_id);
        state.readers = state.readers.saturating_sub(1);
        let _ = reader.sender.send(Ok(result.clone()));
    }
}

fn fail_job(state: &mut State, job_id: u64, error: SharedError) {
    let Some(job) = state.jobs.get_mut(&job_id) else {
        return;
    };
    job.controller.abort_with(error.clone());
    let queued = job.state == JobState::Queued;
    job.state = JobState::Finished;
    let digest = job.digest.take();
    let sources = std::mem::take(&mut job.sources);
    let readers = std::mem::take(&mut job.readers);
    release_digest(state, job_id, digest);
    for source in sources {
        if state.sources.get(&source) == Some(&job_id) {
            state.sources.remove(&source);
        }
    }
    for (reader_id, reader) in readers {
        state.readers_to_jobs.remove(&reader_id);
        state.readers = state.readers.saturating_sub(1);
        let _ = reader.sender.send(Err(error.clone()));
    }
    if queued {
        state.queue.retain(|id| *id != job_id);
        state.jobs.remove(&job_id);
    }
}

/// A finished job must stop attracting joiners at once, not when its task settles.
fn release_digest(state: &mut State, job_id: u64, digest: Option<OfficeKitKey>) {
    if let Some(key) = digest
        && state.digests.get(&key) == Some(&job_id)
    {
        state.digests.remove(&key);
    }
}

fn join_jobs(state: &mut State, from: u64, into: u64) {
    if from == into {
        return;
    }
    let (readers, sources, foreground) = {
        let Some(job) = state.jobs.get_mut(&from) else {
            return;
        };
        (
            std::mem::take(&mut job.readers),
            std::mem::take(&mut job.sources),
            job.priority == OfficeKitPriority::Foreground,
        )
    };
    let Some(existing) = state.jobs.get_mut(&into) else {
        return;
    };
    if foreground {
        existing.priority = OfficeKitPriority::Foreground;
    }
    for (reader_id, reader) in readers {
        state.readers_to_jobs.insert(reader_id, into);
        existing.readers.insert(reader_id, reader);
    }
    for source in sources {
        state.sources.insert(source.clone(), into);
        existing.sources.insert(source);
    }
}

fn remove_source_without_reader(state: &mut State, job_id: u64, source: &str) {
    let still_used = state
        .jobs
        .get(&job_id)
        .is_some_and(|job| job.readers.values().any(|reader| reader.source == source));
    if !still_used {
        if let Some(job) = state.jobs.get_mut(&job_id) {
            job.sources.shift_remove(source);
        }
        if state.sources.get(source) == Some(&job_id) {
            state.sources.remove(source);
        }
    }
}
