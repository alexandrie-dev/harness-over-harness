//! Port of @deepseek-ai/dsh-attachment-local (packages/attachment/attachment-local).
pub mod compression_limiter;
pub mod encoding;
pub mod file_store;
pub mod image;
pub mod normalization;
pub mod request_image;
pub mod store;
mod util;

pub use normalization::{
    NormalizationPolicy, NormalizedImage, can_pass_through_normalization, normalize_image,
};
pub use request_image::{read_request_image_file, request_image_variant_id};
pub use store::{
    PreparedImageFile, commit_prepared_image_file, prepare_image_file, read_image_file,
    save_image_file, validate_image_file,
};

use compression_limiter::CompressionLimiter;
use futures_util::future::BoxFuture;
use futures_util::stream::BoxStream;
use hoh_attachment::*;
use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Cfg, Context, Plugin};
use hoh_timeout::{AbortController, AbortSignal};
use hoh_util_workspace_path::node_path::join;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context as TaskContext, Wake, Waker},
};
use util::{check, lock};

pub const DEFAULT_MAX_IMAGE_BYTES: u64 = 50 * 1024 * 1024;
pub const DEFAULT_MAX_IMAGES_PER_MESSAGE: u64 = 20;
pub const DEFAULT_MAX_MESSAGE_IMAGE_BYTES: u64 = 200 * 1024 * 1024;
pub const DEFAULT_MAX_IMAGE_PIXELS: u64 = 64_000_000;
pub const DEFAULT_MAX_IMAGE_DIMENSION: u64 = 65_535;
pub const DEFAULT_NORMALIZED_IMAGE_MAX_PIXELS: u64 = 2048 * 2048;
pub const DEFAULT_NORMALIZED_IMAGE_MAX_DIMENSION: u64 = 8192;
pub const DEFAULT_NORMALIZED_IMAGE_MAX_BYTES: u64 = 4 * 1024 * 1024;
pub const DEFAULT_IMAGE_COMPRESSION_CONCURRENCY: u64 = 2;
pub const MAX_IMAGE_COMPRESSION_CONCURRENCY: u64 = 8;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dsh_home: Option<String>,
    #[schemars(range(min = 1))]
    pub max_image_bytes: u64,
    #[schemars(range(min = 1))]
    pub max_images_per_message: u64,
    #[schemars(range(min = 1))]
    pub max_message_image_bytes: u64,
    #[schemars(range(min = 1))]
    pub max_image_pixels: u64,
    #[schemars(range(min = 1))]
    pub max_image_dimension: u64,
    #[schemars(range(min = 1))]
    pub normalized_image_max_pixels: u64,
    #[schemars(range(min = 1))]
    pub normalized_image_max_dimension: u64,
    #[schemars(range(min = 1))]
    pub normalized_image_max_bytes: u64,
    #[schemars(range(min=1,max=8),extend("multipleOf"=1))]
    pub image_compression_concurrency: f64,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            dsh_home: None,
            max_image_bytes: DEFAULT_MAX_IMAGE_BYTES,
            max_images_per_message: DEFAULT_MAX_IMAGES_PER_MESSAGE,
            max_message_image_bytes: DEFAULT_MAX_MESSAGE_IMAGE_BYTES,
            max_image_pixels: DEFAULT_MAX_IMAGE_PIXELS,
            max_image_dimension: DEFAULT_MAX_IMAGE_DIMENSION,
            normalized_image_max_pixels: DEFAULT_NORMALIZED_IMAGE_MAX_PIXELS,
            normalized_image_max_dimension: DEFAULT_NORMALIZED_IMAGE_MAX_DIMENSION,
            normalized_image_max_bytes: DEFAULT_NORMALIZED_IMAGE_MAX_BYTES,
            image_compression_concurrency: DEFAULT_IMAGE_COMPRESSION_CONCURRENCY as f64,
        }
    }
}
#[derive(Debug, thiserror::Error)]
#[error("attachment-local: imageCompressionConcurrency must be an integer from 1 through 8")]
pub struct InvalidCompressionConcurrency;

type RequestResult = Result<RequestImageAttachment, SharedError>;
struct Waiter {
    signal: Option<AbortSignal>,
    active: AtomicBool,
    registration: Mutex<Option<BoxFuture<'static, SharedError>>>,
}
struct SharedRequest {
    controller: AbortController,
    result: tokio::sync::watch::Sender<Option<RequestResult>>,
    waiters: Mutex<Vec<Weak<Waiter>>>,
    settled: AtomicBool,
}
struct CancellationWake {
    operation: Weak<SharedRequest>,
    waiter: Weak<Waiter>,
}
impl Wake for CancellationWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        let (Some(operation), Some(waiter)) = (self.operation.upgrade(), self.waiter.upgrade())
        else {
            return;
        };
        let Some(reason) = waiter.signal.as_ref().and_then(AbortSignal::reason) else {
            return;
        };
        // The listener releases its waiter synchronously, as addEventListener does.
        // Never inspect a signal under the waiter lock: refreshing a fused signal
        // can itself wake this listener.
        let abort = {
            let mut waiters = lock(&operation.waiters);
            let released = waiter.active.swap(false, Ordering::AcqRel);
            waiters.retain(|w| w.strong_count() > 0);
            released
                && !operation.settled.load(Ordering::Acquire)
                && !waiters
                    .iter()
                    .filter_map(Weak::upgrade)
                    .any(|w| w.active.load(Ordering::Acquire))
        };
        if abort {
            operation.controller.abort_with(reason);
        }
    }
}
struct WaitGuard {
    waiter: Arc<Waiter>,
}
impl Drop for WaitGuard {
    fn drop(&mut self) {
        self.waiter.active.store(false, Ordering::Release);
    }
}
#[cfg(test)]
type ReadHook = Arc<
    dyn Fn(
            ImageAttachmentRef,
            AbortSignal,
        ) -> BoxFuture<'static, Result<StoredImageAttachment, BoxError>>
        + Send
        + Sync,
>;

#[derive(Clone)]
pub struct LocalAttachmentStore {
    pub root: String,
    pub image_limits: ImageAttachmentLimits,
    pub normalization_policy: NormalizationPolicy,
    pub image_compression_concurrency: u64,
    cache_root: String,
    ctx: Context,
    compression: CompressionLimiter,
    requests: Arc<Mutex<HashMap<String, Arc<SharedRequest>>>>,
    #[cfg(test)]
    read_hook: Arc<Mutex<Option<ReadHook>>>,
}
impl LocalAttachmentStore {
    pub fn new(ctx: Context, config: Config) -> Result<Self, BoxError> {
        Self::new_with_environment(ctx, config, None)
    }
    fn new_with_environment(
        ctx: Context,
        config: Config,
        environment: Option<&HashMap<String, String>>,
    ) -> Result<Self, BoxError> {
        let home = hoh_home_paths::resolve_dsh_home(config.dsh_home.as_deref(), environment)?;
        let cache = hoh_home_paths::dsh_cache_path(home.to_str(), &["attachments"], environment)?;
        let concurrency = config.image_compression_concurrency;
        if !concurrency.is_finite()
            || concurrency.fract() != 0.0
            || concurrency < 1.0
            || concurrency > MAX_IMAGE_COMPRESSION_CONCURRENCY as f64
        {
            return Err(InvalidCompressionConcurrency.into());
        }
        Ok(Self {
            root: join(&[&home.to_string_lossy(), "attachments", "v1"]),
            cache_root: cache.to_string_lossy().into_owned(),
            ctx,
            image_limits: ImageAttachmentLimits {
                max_image_bytes: config.max_image_bytes,
                max_images_per_message: config.max_images_per_message,
                max_message_image_bytes: config.max_message_image_bytes,
                max_image_pixels: config.max_image_pixels,
                max_image_dimension: config.max_image_dimension,
                media_types: vec![
                    ImageMediaType::Png,
                    ImageMediaType::Jpeg,
                    ImageMediaType::Webp,
                    ImageMediaType::Gif,
                ],
            },
            normalization_policy: NormalizationPolicy {
                max_pixels: config.normalized_image_max_pixels,
                max_dimension: config.normalized_image_max_dimension,
                max_bytes: config.normalized_image_max_bytes,
            },
            image_compression_concurrency: concurrency as u64,
            compression: CompressionLimiter::new(concurrency as usize),
            requests: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(test)]
            read_hook: Arc::new(Mutex::new(None)),
        })
    }
    fn prepare_owned(
        &self,
        input: &SaveImageAttachment,
    ) -> BoxFuture<'static, Result<PreparedImageFile, BoxError>> {
        let input = input.clone();
        let limits = self.image_limits.clone();
        let policy = self.normalization_policy;
        let compression = self.compression.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        // Promise.all does not cancel sibling preparations when one rejects.
        // The context owns each preparation, keeping its semaphore permit until
        // native work finishes even if the caller drops its result future.
        let spawn = self
            .ctx
            .spawn("attachment prepare image", move |_| async move {
                let result = compression
                    .run(async || prepare_image_file(&input, &limits, &policy).await)
                    .await;
                let _ = send.send(result);
                Ok(())
            });
        Box::pin(async move {
            spawn?;
            receive.await.map_err(|_| {
                std::io::Error::other("Attachment preparation result channel closed.")
            })?
        })
    }
    fn request_version<'a>(
        &'a self,
        reference: &'a ImageAttachmentRef,
        target: &'a ImageRequestTarget,
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<RequestImageAttachment, BoxError>> {
        if let Err(error) = check(signal) {
            return Box::pin(async { Err(error) });
        }
        let key = request_image_variant_id(reference, target).to_string();
        let mut requests = lock(&self.requests);
        if requests
            .get(&key)
            .is_some_and(|operation| operation.controller.signal().aborted())
        {
            requests.remove(&key);
        }
        let existing = requests.get(&key).cloned();
        let operation = existing.clone().unwrap_or_else(|| {
            Arc::new(SharedRequest {
                controller: AbortController::new(),
                result: tokio::sync::watch::channel(None).0,
                waiters: Mutex::new(vec![]),
                settled: AtomicBool::new(false),
            })
        });
        let waiter = Arc::new(Waiter {
            signal: signal.cloned(),
            active: AtomicBool::new(true),
            registration: Mutex::new(None),
        });
        lock(&operation.waiters).push(Arc::downgrade(&waiter));
        if let Some(signal) = signal.cloned() {
            let wake = Arc::new(CancellationWake {
                operation: Arc::downgrade(&operation),
                waiter: Arc::downgrade(&waiter),
            });
            let waker = Waker::from(wake.clone());
            let mut registration = Box::pin(async move { signal.cancelled().await });
            let ready = registration
                .as_mut()
                .poll(&mut TaskContext::from_waker(&waker))
                .is_ready();
            *lock(&waiter.registration) = Some(registration);
            if ready {
                wake.wake_by_ref();
            }
        }
        let receiver = operation.result.subscribe();
        if existing.is_none() {
            requests.insert(key.clone(), operation.clone());
            let weak_map = Arc::downgrade(&self.requests);
            let work = operation.clone();
            let compression = self.compression.clone();
            let root = self.root.clone();
            let cache = self.cache_root.clone();
            let reference = reference.clone();
            let target = *target;
            #[cfg(test)]
            let hook = lock(&self.read_hook).clone();
            let spawn = self
                .ctx
                .spawn("attachment request image", move |token| async move {
                    let signal = AbortSignal::any([
                        work.controller.signal(),
                        AbortSignal::from_token(token),
                    ]);
                    let result = compression
                        .run(async || {
                            #[cfg(test)]
                            let stored = if let Some(hook) = hook {
                                hook(reference.clone(), signal.clone()).await?
                            } else {
                                read_image_file(&root, &reference, Some(&signal)).await?
                            };
                            #[cfg(not(test))]
                            let stored = read_image_file(&root, &reference, Some(&signal)).await?;
                            read_request_image_file(&cache, &stored, &target, Some(&signal)).await
                        })
                        .await
                        .map_err(SharedError::new);
                    work.settled.store(true, Ordering::Release);
                    work.result.send_replace(Some(result));
                    if let Some(map) = weak_map.upgrade() {
                        let mut map = lock(&map);
                        if map.get(&key).is_some_and(|entry| Arc::ptr_eq(entry, &work)) {
                            map.remove(&key);
                        }
                    }
                    Ok(())
                });
            if let Err(error) = spawn {
                operation.settled.store(true, Ordering::Release);
                operation
                    .result
                    .send_replace(Some(Err(SharedError::new(error))));
                requests.retain(|_, value| !Arc::ptr_eq(value, &operation));
            }
        }
        drop(requests);
        let guard = WaitGuard { waiter };
        Box::pin(async move {
            let mut receiver = receiver;
            loop {
                if let Some(signal) = signal
                    && let Some(reason) = signal.reason()
                {
                    return Err(reason.into());
                }
                let result = receiver.borrow().clone();
                if let Some(result) = result {
                    guard.waiter.active.store(false, Ordering::Release);
                    return result.map_err(Into::into);
                }
                if let Some(signal) = signal {
                    tokio::select! {biased; reason=signal.cancelled()=>return Err(reason.into()), changed=receiver.changed()=>{changed.map_err(|_|std::io::Error::other("Attachment request result channel closed."))?;}}
                } else {
                    receiver.changed().await.map_err(|_| {
                        std::io::Error::other("Attachment request result channel closed.")
                    })?;
                }
            }
        })
    }
}
impl AttachmentStore for LocalAttachmentStore {
    fn image_limits(&self) -> &ImageAttachmentLimits {
        &self.image_limits
    }
    fn validate_image<'a>(
        &'a self,
        input: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.prepare_owned(input).await.map(|_| ()) })
    }
    fn save_image<'a>(
        &'a self,
        input: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<ImageAttachmentRef, BoxError>> {
        Box::pin(async move {
            let prepared = self.prepare_owned(input).await?;
            commit_prepared_image_file(&self.root, &prepared).await
        })
    }
    fn save_images<'a>(
        &'a self,
        inputs: &'a [SaveImageAttachment],
    ) -> BoxFuture<'a, Result<Vec<ImageAttachmentRef>, BoxError>> {
        Box::pin(async move {
            self.validate_image_batch(inputs)?;
            let prepared = futures_util::future::try_join_all(
                inputs.iter().map(|input| self.prepare_owned(input)),
            )
            .await?;
            let mut refs = Vec::with_capacity(prepared.len());
            for image in prepared {
                refs.push(commit_prepared_image_file(&self.root, &image).await?);
            }
            Ok(refs)
        })
    }
    fn read_image<'a>(
        &'a self,
        reference: &'a ImageAttachmentRef,
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<StoredImageAttachment, BoxError>> {
        Box::pin(read_image_file(&self.root, reference, signal))
    }
    fn image_host_path(
        &self,
        reference: &ImageAttachmentRef,
    ) -> Result<Option<String>, hoh_error::HarnessError> {
        store::normalized_image_path(&self.root, reference).map(Some)
    }
    fn save_file<'a>(
        &'a self,
        input: &'a SaveFileAttachment,
    ) -> BoxFuture<'a, Result<FileAttachmentRef, BoxError>> {
        Box::pin(file_store::save_file_verbatim(&self.root, input))
    }
    fn save_file_stream<'a>(
        &'a self,
        input: SaveFileStreamAttachment<'a>,
    ) -> BoxFuture<'a, Result<FileAttachmentRef, BoxError>> {
        Box::pin(file_store::save_file_stream_verbatim(&self.root, input))
    }
    fn read_file_stream<'a>(
        &'a self,
        reference: &'a FileAttachmentRef,
        signal: Option<&'a AbortSignal>,
    ) -> BoxStream<'a, Result<Vec<u8>, BoxError>> {
        file_store::read_file_stream_verbatim(&self.root, reference, signal)
    }
    fn file_host_path(
        &self,
        reference: &FileAttachmentRef,
    ) -> Result<Option<String>, hoh_error::HarnessError> {
        file_store::stored_file_path(&self.root, reference).map(Some)
    }
    fn read_image_request<'a>(
        &'a self,
        reference: &'a ImageAttachmentRef,
        target: &'a ImageRequestTarget,
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<RequestImageAttachment, BoxError>> {
        self.request_version(reference, target, signal)
    }
}
impl Plugin for LocalAttachmentStore {
    const NAME: &'static str = "LocalAttachmentStore";
    type Config = Config;
    type Deps = ();
    async fn apply(ctx: Context, _deps: (), config: Cfg<Config>) -> Result<(), BoxError> {
        let store = Self::new(ctx.clone(), (*config.get()).clone())?;
        ctx.provide::<Attachments>(Arc::new(store)).await?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/cmyk.rs"]
mod cmyk_tests;
#[cfg(test)]
#[path = "../tests/encoding.rs"]
mod encoding_tests;
#[cfg(test)]
#[path = "../tests/file_store.rs"]
mod file_store_tests;
#[cfg(test)]
#[path = "../tests/image.rs"]
mod image_tests;
#[cfg(test)]
#[path = "../tests/index.rs"]
mod index_tests;
#[cfg(test)]
#[path = "../tests/normalization.rs"]
mod normalization_tests;
#[cfg(test)]
#[path = "../tests/normalization_verification.rs"]
mod normalization_verification_tests;
#[cfg(test)]
#[path = "../tests/request_image.rs"]
mod request_image_tests;
#[cfg(test)]
#[path = "../tests/request_image_verification.rs"]
mod request_image_verification_tests;
#[cfg(test)]
#[path = "../tests/store.rs"]
mod store_tests;
#[cfg(test)]
#[path = "../tests/support.rs"]
mod test_support;
