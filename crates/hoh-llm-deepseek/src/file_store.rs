//! DeepSeek Files upload reuse, invalidation, and cleanup.

use crate::file_id::DeepSeekFileId;
use crate::files_api::{
    DeepSeekFileObject, DeepSeekFilePage, DeepSeekFilesClient, is_files_quota_error,
};
use crate::messages_api::messages_api_root;
use crate::types::DeepSeekFilePolicy;
use crate::upload_index::{DeepSeekUploadIndex, DeepSeekUploadRecord, deep_seek_file_scope};
use futures_util::future::BoxFuture;
use hoh_attachment::{ImageMediaType, ImageVariantId, RequestImageAttachment};
use hoh_error::{HarnessError, SharedError};
use hoh_llm::{BuiltinLlmFailureCode as Code, LlmError, LlmErrorOptions, LlmFailure};
use hoh_timeout::{AbortController, AbortSignal, TimeoutReason, timeout_of};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;

pub const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
pub const FILES_API_TIMEOUT: &str = "DEEPSEEK_FILES_API_TIMEOUT";
const OWNED_FILE_PREFIX: &str = "dsh-";

trait FilesTransport: Send + Sync {
    fn upload<'a>(
        &'a self,
        data: &'a [u8],
        media_type: ImageMediaType,
        filename: &'a str,
        expires_after_seconds: u64,
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<DeepSeekFileObject, HarnessError>>;
    fn list<'a>(
        &'a self,
        after: Option<&'a DeepSeekFileId>,
        limit: Option<u64>,
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<DeepSeekFilePage, HarnessError>>;
    fn delete<'a>(
        &'a self,
        id: &'a DeepSeekFileId,
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<(), HarnessError>>;
}

impl FilesTransport for DeepSeekFilesClient {
    fn upload<'a>(
        &'a self,
        data: &'a [u8],
        media_type: ImageMediaType,
        filename: &'a str,
        expires_after_seconds: u64,
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<DeepSeekFileObject, HarnessError>> {
        Box::pin(DeepSeekFilesClient::upload(
            self,
            data,
            media_type,
            filename,
            expires_after_seconds,
            signal,
        ))
    }

    fn list<'a>(
        &'a self,
        after: Option<&'a DeepSeekFileId>,
        limit: Option<u64>,
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<DeepSeekFilePage, HarnessError>> {
        Box::pin(DeepSeekFilesClient::list(self, after, limit, signal))
    }

    fn delete<'a>(
        &'a self,
        id: &'a DeepSeekFileId,
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<(), HarnessError>> {
        Box::pin(DeepSeekFilesClient::delete(self, id, signal))
    }
}

type ClientFactory = Arc<
    dyn Fn(&DeepSeekFileConnection) -> Result<Arc<dyn FilesTransport>, HarnessError> + Send + Sync,
>;

#[derive(Clone)]
struct UploadBackoff {
    generation: u64,
    interval: u64,
    remaining: u64,
    reason: TimeoutReason,
}

#[derive(Default)]
struct SharedUploadState {
    waiters: usize,
    result: Option<Result<DeepSeekFileReference, HarnessError>>,
}

struct SharedUpload {
    controller: AbortController,
    state: Mutex<SharedUploadState>,
    notify: Notify,
    previous_backoff_generation: Option<u64>,
}

impl SharedUpload {
    fn new(previous_backoff_generation: Option<u64>) -> Self {
        Self {
            controller: AbortController::new(),
            state: Mutex::new(SharedUploadState::default()),
            notify: Notify::new(),
            previous_backoff_generation,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DeepSeekFileConnection {
    pub base_url: String,
    pub headers: IndexMap<String, String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeepSeekFileReference {
    pub record: DeepSeekUploadRecord,
    pub uploaded: bool,
}

#[derive(Clone)]
pub struct DeepSeekFileStore {
    index: DeepSeekUploadIndex,
    now: Arc<dyn Fn() -> u64 + Send + Sync>,
    client_factory: ClientFactory,
    backoffs: Arc<Mutex<HashMap<crate::DeepSeekFileScope, UploadBackoff>>>,
    inflight: Arc<Mutex<HashMap<String, Arc<SharedUpload>>>>,
}
impl DeepSeekFileStore {
    pub fn new(index: DeepSeekUploadIndex) -> Self {
        Self {
            index,
            now: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |value| value.as_millis() as u64)
            }),
            client_factory: Arc::new(|connection| {
                DeepSeekFilesClient::new(&connection.base_url, connection.headers.clone())
                    .map(|client| Arc::new(client) as Arc<dyn FilesTransport>)
            }),
            backoffs: Arc::new(Mutex::new(HashMap::new())),
            inflight: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    pub fn with_clock(mut self, now: Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
        self.now = now;
        self
    }
    #[cfg(test)]
    fn with_transport(mut self, transport: Arc<dyn FilesTransport>) -> Self {
        self.client_factory = Arc::new(move |_| Ok(transport.clone()));
        self
    }
    fn scope(connection: &DeepSeekFileConnection) -> crate::DeepSeekFileScope {
        let root =
            messages_api_root(&connection.base_url).unwrap_or_else(|_| connection.base_url.clone());
        let mut headers: Vec<_> = connection.headers.iter().collect();
        headers.sort_by(|a, b| a.0.cmp(b.0));
        deep_seek_file_scope(
            &root,
            &serde_json::to_string(&headers).expect("headers are JSON"),
        )
    }
    fn client(
        &self,
        connection: &DeepSeekFileConnection,
    ) -> Result<Arc<dyn FilesTransport>, HarnessError> {
        (self.client_factory)(connection)
    }
    pub async fn ensure_uploaded(
        &self,
        version: &RequestImageAttachment,
        connection: &DeepSeekFileConnection,
        policy: &DeepSeekFilePolicy,
        signal: Option<&AbortSignal>,
    ) -> Result<DeepSeekFileReference, HarnessError> {
        if version.bytes > MAX_IMAGE_BYTES {
            return Err(LlmError::new(
                "DeepSeek image exceeds the 32 MiB per-image limit.",
                Code::InvalidRequest,
            )
            .expect("image limit diagnostic is valid"));
        }
        if let Some(signal) = signal {
            signal.throw_if_aborted().map_err(|reason| {
                LlmError::with_options(
                    "DeepSeek file upload aborted",
                    Code::Aborted,
                    LlmErrorOptions {
                        cause: Some(reason),
                        ..Default::default()
                    },
                )
                .expect("abort diagnostic is valid")
            })?;
        }
        let scope = Self::scope(connection);
        let now = (self.now)();
        let margin = policy.refresh_margin_seconds * 1000;
        if let Some(record) = self
            .index
            .get(&scope, &version.variant_id, now, margin, signal)
            .await
            .map_err(index_error)?
        {
            return Ok(DeepSeekFileReference {
                record,
                uploaded: false,
            });
        }
        if let Some(backoff) = mutex(&self.backoffs).get_mut(&scope)
            && backoff.remaining > 0
        {
            backoff.remaining -= 1;
            return Err(files_api_skipped(backoff.reason.clone()));
        }

        let key = format!("{scope}\0{}", version.variant_id);
        let operation = {
            let mut inflight = mutex(&self.inflight);
            if let Some(active) = inflight.get(&key)
                && !active.controller.signal().aborted()
            {
                active.clone()
            } else {
                inflight.remove(&key);
                let previous = mutex(&self.backoffs)
                    .get(&scope)
                    .map(|backoff| backoff.generation);
                let active = Arc::new(SharedUpload::new(previous));
                inflight.insert(key.clone(), active.clone());
                self.spawn_upload(
                    key,
                    active.clone(),
                    version.clone(),
                    connection.clone(),
                    policy.clone(),
                    scope.clone(),
                );
                active
            }
        };
        self.wait_for_upload(operation, scope, signal).await
    }

    fn spawn_upload(
        &self,
        key: String,
        operation: Arc<SharedUpload>,
        version: RequestImageAttachment,
        connection: DeepSeekFileConnection,
        policy: DeepSeekFilePolicy,
        scope: crate::DeepSeekFileScope,
    ) {
        let store = self.clone();
        tokio::spawn(async move {
            let signal = operation.controller.signal();
            let result = store
                .upload_once(&version, &connection, &policy, &scope, Some(&signal))
                .await;
            {
                let mut state = mutex(&operation.state);
                state.result = Some(result);
            }
            operation.notify.notify_waiters();
            let mut inflight = mutex(&store.inflight);
            if inflight
                .get(&key)
                .is_some_and(|active| Arc::ptr_eq(active, &operation))
            {
                inflight.remove(&key);
            }
        });
    }

    async fn wait_for_upload(
        &self,
        operation: Arc<SharedUpload>,
        scope: crate::DeepSeekFileScope,
        signal: Option<&AbortSignal>,
    ) -> Result<DeepSeekFileReference, HarnessError> {
        {
            let mut state = mutex(&operation.state);
            state.waiters += 1;
        }
        if let Some(signal) = signal {
            if let Err(reason) = signal.throw_if_aborted() {
                self.record_upload_timeout(&operation, &scope, &reason);
                release_waiter(&operation, Some(reason.clone()));
                return Err(aborted_upload(reason));
            }
            tokio::select! {
                result = upload_result(&operation) => {
                    release_waiter(&operation, None);
                    result
                }
                reason = signal.cancelled() => {
                    self.record_upload_timeout(&operation, &scope, &reason);
                    release_waiter(&operation, Some(reason.clone()));
                    Err(aborted_upload(reason))
                }
            }
        } else {
            let result = upload_result(&operation).await;
            release_waiter(&operation, None);
            result
        }
    }

    fn record_upload_timeout(
        &self,
        operation: &SharedUpload,
        scope: &crate::DeepSeekFileScope,
        reason: &SharedError,
    ) {
        let Some(reason) = timeout_of(Some(reason), Some(FILES_API_TIMEOUT)).cloned() else {
            return;
        };
        let mut backoffs = mutex(&self.backoffs);
        let current = backoffs.get(scope);
        if current.map(|backoff| backoff.generation) != operation.previous_backoff_generation {
            return;
        }
        let interval = current.map_or(1, |backoff| backoff.interval.saturating_mul(2));
        let generation = current.map_or(1, |backoff| backoff.generation.saturating_add(1));
        backoffs.insert(
            scope.clone(),
            UploadBackoff {
                generation,
                interval,
                remaining: interval,
                reason,
            },
        );
    }

    async fn upload_once(
        &self,
        version: &RequestImageAttachment,
        connection: &DeepSeekFileConnection,
        policy: &DeepSeekFilePolicy,
        scope: &crate::DeepSeekFileScope,
        signal: Option<&AbortSignal>,
    ) -> Result<DeepSeekFileReference, HarnessError> {
        let margin = policy.refresh_margin_seconds * 1000;
        let client = self.client(connection)?;
        let candidate = match upload_record(client.as_ref(), version, policy, scope, signal).await {
            Ok(candidate) => candidate,
            Err(error) if is_files_quota_error(&error) => {
                let deleted = self
                    .reclaim_oldest_owned(connection, policy.quota_cleanup_batch, signal)
                    .await?;
                if deleted == 0 {
                    return Err(error);
                }
                upload_record(client.as_ref(), version, policy, scope, signal).await?
            }
            Err(error) => return Err(error),
        };
        if signal.is_none_or(|signal| !signal.aborted()) {
            mutex(&self.backoffs).remove(scope);
        }
        let committed = self
            .index
            .commit(candidate.clone(), (self.now)(), margin)
            .await
            .map_err(index_error)?;
        if !committed.accepted {
            let _ = client.delete(&candidate.file_id, signal).await;
        }
        Ok(DeepSeekFileReference {
            record: committed.record,
            uploaded: committed.accepted,
        })
    }
    pub async fn invalidate(
        &self,
        generations: &[(ImageVariantId, DeepSeekFileId)],
        connection: &DeepSeekFileConnection,
    ) -> Result<(), HarnessError> {
        self.index
            .remove(&Self::scope(connection), generations)
            .await
            .map_err(index_error)
    }
    pub async fn release(
        &self,
        version: &RequestImageAttachment,
        connection: &DeepSeekFileConnection,
        policy: &DeepSeekFilePolicy,
        signal: Option<&AbortSignal>,
    ) -> Result<bool, HarnessError> {
        let scope = Self::scope(connection);
        let Some(record) = self
            .index
            .get(
                &scope,
                &version.variant_id,
                (self.now)(),
                policy.refresh_margin_seconds * 1000,
                signal,
            )
            .await
            .map_err(index_error)?
        else {
            return Ok(false);
        };
        self.client(connection)?
            .delete(&record.file_id, signal)
            .await?;
        self.index
            .remove(&scope, &[(record.variant_id, record.file_id)])
            .await
            .map_err(index_error)?;
        Ok(true)
    }

    pub async fn reclaim_oldest_owned(
        &self,
        connection: &DeepSeekFileConnection,
        count: u64,
        signal: Option<&AbortSignal>,
    ) -> Result<u64, HarnessError> {
        let client = self.client(connection)?;
        let mut after: Option<DeepSeekFileId> = None;
        let mut owned = Vec::new();
        loop {
            let page = client.list(after.as_ref(), Some(1_000), signal).await?;
            for file in page.data {
                if file.filename.starts_with(OWNED_FILE_PREFIX) {
                    owned.push((file.id, file.created_at));
                }
            }
            owned.sort_by_key(|(_, created_at)| *created_at);
            owned.truncate(count as usize);
            if !page.has_more || page.last_id.is_none() || page.last_id.as_ref() == after.as_ref() {
                break;
            }
            after = page.last_id;
        }
        let deleted = owned.len() as u64;
        for (file_id, _) in owned {
            client.delete(&file_id, signal).await?;
        }
        Ok(deleted)
    }

    pub async fn release_all(
        &self,
        connection: &DeepSeekFileConnection,
        signal: Option<&AbortSignal>,
    ) -> Result<u64, HarnessError> {
        let mut total = 0;
        loop {
            let deleted = self.reclaim_oldest_owned(connection, 1_000, signal).await?;
            total += deleted;
            if deleted < 1_000 {
                break;
            }
        }
        self.index
            .clear(&Self::scope(connection))
            .await
            .map_err(index_error)?;
        Ok(total)
    }
}

async fn upload_result(operation: &SharedUpload) -> Result<DeepSeekFileReference, HarnessError> {
    loop {
        let notified = operation.notify.notified();
        if let Some(result) = mutex(&operation.state).result.clone() {
            return result;
        }
        notified.await;
    }
}

fn release_waiter(operation: &SharedUpload, cancelled: Option<SharedError>) {
    let abort = {
        let mut state = mutex(&operation.state);
        state.waiters = state.waiters.saturating_sub(1);
        cancelled.is_some() && state.waiters == 0 && state.result.is_none()
    };
    if abort {
        operation
            .controller
            .abort_with(cancelled.expect("cancelled waiter has a reason"));
    }
}

fn files_api_skipped(reason: TimeoutReason) -> HarnessError {
    LlmError::from_failure(
        &LlmFailure::new(
            "DeepSeek Files API upload skipped after a timeout.",
            "FILES_API_SKIPPED",
        ),
        Some(SharedError::new(reason)),
    )
    .expect("Files skip diagnostic is valid")
}

fn aborted_upload(reason: SharedError) -> HarnessError {
    LlmError::with_options(
        "DeepSeek file upload aborted",
        Code::Aborted,
        LlmErrorOptions {
            cause: Some(reason),
            ..Default::default()
        },
    )
    .expect("abort diagnostic is valid")
}

fn mutex<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

async fn upload_record(
    client: &dyn FilesTransport,
    version: &RequestImageAttachment,
    policy: &DeepSeekFilePolicy,
    scope: &crate::DeepSeekFileScope,
    signal: Option<&AbortSignal>,
) -> Result<DeepSeekUploadRecord, HarnessError> {
    let file = client
        .upload(
            &version.data,
            version.media_type,
            &filename(version),
            policy.expires_after_seconds,
            signal,
        )
        .await?;
    if file.bytes != version.data.len() as u64 {
        return Err(LlmError::new(
            "DeepSeek Files API upload response does not match the submitted image.",
            Code::InvalidResponse,
        )
        .expect("upload mismatch is valid"));
    }
    Ok(DeepSeekUploadRecord {
        scope: scope.clone(),
        attachment_id: version.attachment.attachment_id.clone(),
        variant_id: version.variant_id.clone(),
        file_id: file.id,
        bytes: file.bytes,
        created_at: file.created_at * 1000,
        expires_at: file.expires_at.unwrap_or(file.created_at) * 1000,
    })
}
fn extension(media: ImageMediaType) -> &'static str {
    match media {
        ImageMediaType::Png => "png",
        ImageMediaType::Jpeg => "jpeg",
        ImageMediaType::Webp => "webp",
        ImageMediaType::Gif => "gif",
    }
}
fn filename(version: &RequestImageAttachment) -> String {
    let attachment = version
        .attachment
        .attachment_id
        .as_str()
        .strip_prefix("sha256:")
        .unwrap_or(version.attachment.attachment_id.as_str());
    let variant = version
        .variant_id
        .as_str()
        .strip_prefix("sha256:")
        .unwrap_or(version.variant_id.as_str());
    format!(
        "dsh-{}-{}.{}",
        &attachment[..attachment.len().min(16)],
        &variant[..variant.len().min(8)],
        extension(version.media_type)
    )
}
fn index_error(error: std::io::Error) -> HarnessError {
    LlmError::with_options(
        "DeepSeek upload index failed",
        Code::FilesApi,
        LlmErrorOptions {
            cause: Some(SharedError::new(error)),
            ..Default::default()
        },
    )
    .expect("index diagnostic is valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files_api::DeepSeekFilesError;
    use hoh_attachment::{ImageAttachmentRef, RequestImageDepth, RequestImageSpace};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    enum UploadAction {
        Immediate(Result<DeepSeekFileObject, HarnessError>),
        Gated(Arc<Notify>, Result<DeepSeekFileObject, HarnessError>),
        WaitForAbort,
    }

    struct MockTransport {
        upload_actions: Mutex<VecDeque<UploadAction>>,
        list_results: Mutex<VecDeque<Result<DeepSeekFilePage, HarnessError>>>,
        uploads: AtomicUsize,
        upload_started: Notify,
        deleted: Mutex<Vec<DeepSeekFileId>>,
    }

    impl MockTransport {
        fn new(upload_actions: Vec<UploadAction>, pages: Vec<DeepSeekFilePage>) -> Arc<Self> {
            Arc::new(Self {
                upload_actions: Mutex::new(upload_actions.into()),
                list_results: Mutex::new(pages.into_iter().map(Ok).collect()),
                uploads: AtomicUsize::new(0),
                upload_started: Notify::new(),
                deleted: Mutex::new(Vec::new()),
            })
        }

        async fn wait_for_uploads(&self, expected: usize) {
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    let started = self.upload_started.notified();
                    if self.uploads.load(Ordering::SeqCst) >= expected {
                        return;
                    }
                    started.await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("mock did not receive {expected} uploads"));
        }
    }

    impl FilesTransport for MockTransport {
        fn upload<'a>(
            &'a self,
            _data: &'a [u8],
            _media_type: ImageMediaType,
            _filename: &'a str,
            _expires_after_seconds: u64,
            signal: Option<&'a AbortSignal>,
        ) -> BoxFuture<'a, Result<DeepSeekFileObject, HarnessError>> {
            self.uploads.fetch_add(1, Ordering::SeqCst);
            // This mock has one coordination waiter. A stored permit closes the
            // window between its counter check and polling `notified()`.
            self.upload_started.notify_one();
            let action = mutex(&self.upload_actions)
                .pop_front()
                .unwrap_or(UploadAction::WaitForAbort);
            Box::pin(async move {
                match action {
                    UploadAction::Immediate(result) => result,
                    UploadAction::Gated(gate, result) => {
                        gate.notified().await;
                        result
                    }
                    UploadAction::WaitForAbort => {
                        let reason = signal
                            .expect("abortable mock upload has a signal")
                            .cancelled()
                            .await;
                        Err(aborted_upload(reason))
                    }
                }
            })
        }

        fn list<'a>(
            &'a self,
            _after: Option<&'a DeepSeekFileId>,
            _limit: Option<u64>,
            _signal: Option<&'a AbortSignal>,
        ) -> BoxFuture<'a, Result<DeepSeekFilePage, HarnessError>> {
            let result = mutex(&self.list_results)
                .pop_front()
                .expect("mock list response");
            Box::pin(async move { result })
        }

        fn delete<'a>(
            &'a self,
            id: &'a DeepSeekFileId,
            _signal: Option<&'a AbortSignal>,
        ) -> BoxFuture<'a, Result<(), HarnessError>> {
            mutex(&self.deleted).push(id.clone());
            Box::pin(async { Ok(()) })
        }
    }

    fn connection() -> DeepSeekFileConnection {
        DeepSeekFileConnection {
            base_url: "https://example.test/v1".into(),
            headers: IndexMap::new(),
        }
    }

    fn policy() -> DeepSeekFilePolicy {
        DeepSeekFilePolicy {
            expires_after_seconds: 3_600,
            refresh_margin_seconds: 60,
            quota_cleanup_batch: 1,
        }
    }

    fn version() -> RequestImageAttachment {
        let attachment = ImageAttachmentRef {
            attachment_id:
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            media_type: ImageMediaType::Png,
            bytes: 4,
            width: 1,
            height: 1,
            name: None,
            original_dimensions: None,
        };
        RequestImageAttachment {
            variant_id: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .into(),
            attachment,
            data: vec![0; 4],
            media_type: ImageMediaType::Png,
            bytes: 4,
            width: 1,
            height: 1,
            depth: RequestImageDepth::Uchar,
            space: RequestImageSpace::Srgb,
            has_alpha: true,
        }
    }

    fn file(id: &str, filename: &str, created_at: u64, bytes: u64) -> DeepSeekFileObject {
        DeepSeekFileObject {
            id: DeepSeekFileId::new(id).unwrap(),
            bytes,
            created_at,
            filename: filename.into(),
            expires_at: Some(created_at + 3_600),
        }
    }

    fn page(
        files: Vec<DeepSeekFileObject>,
        last: Option<&str>,
        has_more: bool,
    ) -> DeepSeekFilePage {
        DeepSeekFilePage {
            first_id: files.first().map(|file| file.id.clone()),
            last_id: last.and_then(DeepSeekFileId::new),
            data: files,
            has_more,
        }
    }

    fn store(directory: &tempfile::TempDir, transport: Arc<MockTransport>) -> DeepSeekFileStore {
        DeepSeekFileStore::new(DeepSeekUploadIndex::new(
            directory.path().join("files-v3.json"),
        ))
        .with_clock(Arc::new(|| 0))
        .with_transport(transport)
    }

    #[tokio::test]
    async fn concurrent_waiters_share_one_upload() {
        let gate = Arc::new(Notify::new());
        let transport = MockTransport::new(
            vec![UploadAction::Gated(
                gate.clone(),
                Ok(file("file-shared", "dsh-shared.png", 10, 4)),
            )],
            vec![],
        );
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory, transport.clone());
        let image = version();
        let connection = connection();
        let policy = policy();
        let left = {
            let store = store.clone();
            let image = image.clone();
            let connection = connection.clone();
            let policy = policy.clone();
            tokio::spawn(async move {
                store
                    .ensure_uploaded(&image, &connection, &policy, None)
                    .await
            })
        };
        let right = {
            let store = store.clone();
            let image = image.clone();
            let connection = connection.clone();
            let policy = policy.clone();
            tokio::spawn(async move {
                store
                    .ensure_uploaded(&image, &connection, &policy, None)
                    .await
            })
        };
        transport.wait_for_uploads(1).await;
        gate.notify_one();
        let (left, right) = tokio::join!(left, right);
        assert_eq!(left.unwrap().unwrap(), right.unwrap().unwrap());
        assert_eq!(transport.uploads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancelling_one_waiter_preserves_shared_transport() {
        let gate = Arc::new(Notify::new());
        let transport = MockTransport::new(
            vec![UploadAction::Gated(
                gate.clone(),
                Ok(file("file-shared", "dsh-shared.png", 10, 4)),
            )],
            vec![],
        );
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory, transport.clone());
        let image = version();
        let connection = connection();
        let policy = policy();
        let controller = AbortController::new();
        let signal = controller.signal();
        let cancelled = {
            let store = store.clone();
            let image = image.clone();
            let connection = connection.clone();
            let policy = policy.clone();
            tokio::spawn(async move {
                store
                    .ensure_uploaded(&image, &connection, &policy, Some(&signal))
                    .await
            })
        };
        let survivor = {
            let store = store.clone();
            let image = image.clone();
            let connection = connection.clone();
            let policy = policy.clone();
            tokio::spawn(async move {
                store
                    .ensure_uploaded(&image, &connection, &policy, None)
                    .await
            })
        };
        transport.wait_for_uploads(1).await;
        for _ in 0..1_000 {
            let waiters = mutex(&store.inflight)
                .values()
                .next()
                .map_or(0, |active| mutex(&active.state).waiters);
            if waiters == 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            mutex(&store.inflight)
                .values()
                .next()
                .map_or(0, |active| mutex(&active.state).waiters),
            2
        );
        controller.abort();
        assert_eq!(cancelled.await.unwrap().unwrap_err().code(), "ABORTED");
        gate.notify_one();
        assert_eq!(
            survivor.await.unwrap().unwrap().record.file_id.as_str(),
            "file-shared"
        );
        assert_eq!(transport.uploads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn timeout_backoff_skips_one_then_two_uploads() {
        let transport = MockTransport::new(
            vec![UploadAction::WaitForAbort, UploadAction::WaitForAbort],
            vec![],
        );
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory, transport.clone());
        let image = version();
        let connection = connection();
        let policy = policy();
        for (uploads, skipped) in [(1, 1), (2, 2)] {
            let controller = AbortController::new();
            let signal = controller.signal();
            let upload = {
                let store = store.clone();
                let image = image.clone();
                let connection = connection.clone();
                let policy = policy.clone();
                tokio::spawn(async move {
                    store
                        .ensure_uploaded(&image, &connection, &policy, Some(&signal))
                        .await
                })
            };
            transport.wait_for_uploads(uploads).await;
            controller.abort_with(TimeoutReason::new(FILES_API_TIMEOUT, 25.0));
            assert_eq!(upload.await.unwrap().unwrap_err().code(), "ABORTED");
            for _ in 0..skipped {
                let error = store
                    .ensure_uploaded(&image, &connection, &policy, None)
                    .await
                    .unwrap_err();
                assert_eq!(error.code(), "FILES_API_SKIPPED");
            }
            assert_eq!(transport.uploads.load(Ordering::SeqCst), uploads);
        }
    }

    #[tokio::test]
    async fn quota_recovery_reclaims_once_and_retries() {
        let quota = LlmError::with_options(
            "stored files quota exceeded",
            Code::FilesApi,
            LlmErrorOptions {
                cause: Some(SharedError::new(DeepSeekFilesError {
                    message: "stored files quota exceeded".into(),
                    status: 400,
                    detail: "files_quota stored files quota exceeded".into(),
                })),
                ..Default::default()
            },
        )
        .unwrap();
        let transport = MockTransport::new(
            vec![
                UploadAction::Immediate(Err(quota)),
                UploadAction::Immediate(Ok(file("file-new", "dsh-new.png", 10, 4))),
            ],
            vec![page(
                vec![file("file-old", "dsh-old.png", 1, 1)],
                Some("file-old"),
                false,
            )],
        );
        let directory = tempfile::tempdir().unwrap();
        let result = store(&directory, transport.clone())
            .ensure_uploaded(&version(), &connection(), &policy(), None)
            .await
            .unwrap();
        assert_eq!(result.record.file_id.as_str(), "file-new");
        assert_eq!(transport.uploads.load(Ordering::SeqCst), 2);
        assert_eq!(
            mutex(&transport.deleted)
                .iter()
                .map(DeepSeekFileId::as_str)
                .collect::<Vec<_>>(),
            ["file-old"]
        );
    }

    #[tokio::test]
    async fn reclaim_selects_oldest_owned_file_across_pages() {
        let transport = MockTransport::new(
            vec![],
            vec![
                page(
                    vec![
                        file("file-new", "dsh-new.png", 20, 1),
                        file("file-user", "user.png", 0, 1),
                    ],
                    Some("file-new"),
                    true,
                ),
                page(
                    vec![file("file-old", "dsh-old.png", 1, 1)],
                    Some("file-old"),
                    false,
                ),
            ],
        );
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            store(&directory, transport.clone())
                .reclaim_oldest_owned(&connection(), 1, None)
                .await
                .unwrap(),
            1
        );
        assert_eq!(mutex(&transport.deleted)[0].as_str(), "file-old");
    }
}
