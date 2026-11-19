//! Direct DeepSeek Messages transport with a cancellable stream lifecycle.

use crate::file_store::{DeepSeekFileConnection, DeepSeekFileStore};
use crate::images::{inline_images, prepare_file_ids, prepare_images};
use crate::messages_api::{MESSAGES_FILES_BETA, MESSAGES_TOOL_CHANGES_BETA, messages_api_root};
use crate::model_info::model_info;
use crate::request_extensions::{prepare_request_extensions, stringify_payload};
use crate::request_pricing::{deep_seek_image_request_pricing, resolve_request_image_target};
use crate::serialize::serialize;
use crate::sse::{SseParser, decode_event};
use crate::translate::Translator;
use crate::transport::{provider_error, provider_error_detail};
use crate::types::{DeepSeekConnectionOptions, DeepSeekRequestAuth, DeepSeekRequestErrorMapper};
use crate::upload_index::DeepSeekUploadIndex;
use futures_util::{future::BoxFuture, stream};
use hoh_anonymous_user_id::AnonymousUserId;
use hoh_attachment::{AttachmentStore, ImageRequestTarget};
use hoh_deepseek_llm_api_extensions::{DeepSeekLlmApiExtensionRequest, DeepSeekLlmApiExtensions};
use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_kernel::Service;
use hoh_llm::{
    BuiltinLlmFailureCode as Code, GenerateOptions, ImageAttachmentAccessResolver, LlmAdapter,
    LlmFailure, LlmFuture, LlmImageRequestPricing, LlmModelInfo, LlmProviderInfo,
    LlmResolvedModelInfo, LlmStream, PreparedAdapterCall, ResolvedRetryPolicy, StreamChunk,
    attribution_headers,
};
use hoh_timeout::{AbortController, AbortSignal, timeout_of};
use indexmap::IndexMap;
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

fn failure(message: impl Into<String>, code: Code) -> HarnessError {
    hoh_llm::LlmError::new(message, code).expect("adapter diagnostic is valid")
}

pub type DeepSeekOptionsResolver =
    dyn Fn() -> Result<DeepSeekConnectionOptions, BoxError> + Send + Sync + 'static;
pub type DeepSeekConnectionResolver = dyn Fn(Option<AbortSignal>) -> BoxFuture<'static, Result<DeepSeekConnectionOptions, BoxError>>
    + Send
    + Sync
    + 'static;
pub type DeepSeekAuthResolver = dyn Fn(DeepSeekConnectionOptions) -> BoxFuture<'static, Result<DeepSeekRequestAuth, BoxError>>
    + Send
    + Sync
    + 'static;
pub type DeepSeekModelDiscovery = dyn Fn(String) -> BoxFuture<'static, Result<Vec<LlmModelInfo>, BoxError>>
    + Send
    + Sync
    + 'static;
pub type DeepSeekModelAdmission =
    dyn Fn(&DeepSeekConnectionOptions, &str) -> Result<(), BoxError> + Send + Sync + 'static;
pub type DeepSeekUserIdResolver = dyn Fn() -> AnonymousUserId + Send + Sync + 'static;
pub type DeepSeekAttachmentsResolver =
    dyn Fn() -> Option<Arc<dyn AttachmentStore>> + Send + Sync + 'static;
pub type DeepSeekReplayReporter = dyn Fn(&str, &str, &str) + Send + Sync + 'static;
pub type DeepSeekExtensionReporter = dyn Fn(&str, &str, &[String], &str) + Send + Sync + 'static;

#[derive(Clone)]
struct Inner {
    connection: DeepSeekConnectionOptions,
    options: Option<Arc<DeepSeekOptionsResolver>>,
    connect: Option<Arc<DeepSeekConnectionResolver>>,
    resolve_auth: Option<Arc<DeepSeekAuthResolver>>,
    discover_models: Option<Arc<DeepSeekModelDiscovery>>,
    admit: Option<Arc<DeepSeekModelAdmission>>,
    on_replay_degrade: Option<Arc<DeepSeekReplayReporter>>,
    on_extensions_omitted: Option<Arc<DeepSeekExtensionReporter>>,
    on_extensions_unaccepted: Option<Arc<DeepSeekExtensionReporter>>,
    provider_name: String,
    headers: IndexMap<String, String>,
    user_id: Option<String>,
    resolve_user_id: Option<Arc<DeepSeekUserIdResolver>>,
    client: reqwest::Client,
    files: DeepSeekFileStore,
    extensions: Option<Service<DeepSeekLlmApiExtensions>>,
    attachments: Option<Arc<dyn AttachmentStore>>,
    resolve_attachments: Option<Arc<DeepSeekAttachmentsResolver>>,
    resolve_access: Option<Arc<ImageAttachmentAccessResolver<'static>>>,
}

impl Inner {
    fn user_id(&self) -> Option<String> {
        self.resolve_user_id
            .as_ref()
            .map(|resolve| resolve().to_string())
            .or_else(|| self.user_id.clone())
    }

    fn attachments(&self) -> Option<Arc<dyn AttachmentStore>> {
        self.resolve_attachments
            .as_ref()
            .map_or_else(|| self.attachments.clone(), |resolve| resolve())
    }
}

#[derive(Clone)]
pub struct DeepSeekAdapter {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for DeepSeekAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepSeekAdapter")
            .field("base_url", &self.inner.connection.base_url)
            .field("provider_name", &self.inner.provider_name)
            .finish_non_exhaustive()
    }
}

impl DeepSeekAdapter {
    pub fn new(connection: DeepSeekConnectionOptions) -> Result<Self, HarnessError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| {
                failure(
                    format!("DeepSeek HTTP client cannot be built: {error}"),
                    Code::InvalidConfig,
                )
            })?;
        let files = DeepSeekUploadIndex::default_path()
            .map(DeepSeekFileStore::new)
            .map_err(|error| {
                hoh_llm::LlmError::with_options(
                    "DeepSeek upload index path cannot be resolved",
                    Code::InvalidConfig,
                    hoh_llm::LlmErrorOptions {
                        cause: Some(SharedError::new(error)),
                        ..Default::default()
                    },
                )
                .expect("upload index diagnostic is valid")
            })?;
        Ok(Self {
            inner: Arc::new(Inner {
                connection,
                options: None,
                connect: None,
                resolve_auth: None,
                discover_models: None,
                admit: None,
                on_replay_degrade: None,
                on_extensions_omitted: None,
                on_extensions_unaccepted: None,
                provider_name: "DeepSeek".into(),
                headers: IndexMap::new(),
                user_id: None,
                resolve_user_id: None,
                client,
                files,
                extensions: None,
                attachments: None,
                resolve_attachments: None,
                resolve_access: None,
            }),
        })
    }

    fn rebuild(self, update: impl FnOnce(&mut Inner)) -> Self {
        let mut inner = (*self.inner).clone();
        update(&mut inner);
        Self {
            inner: Arc::new(inner),
        }
    }

    pub fn with_provider_name(self, name: impl Into<String>) -> Self {
        self.rebuild(|inner| inner.provider_name = name.into())
    }
    pub fn with_headers(self, headers: IndexMap<String, String>) -> Self {
        self.rebuild(|inner| inner.headers = headers)
    }
    pub fn with_user_id(self, user_id: impl Into<String>) -> Self {
        self.rebuild(|inner| inner.user_id = Some(user_id.into()))
    }
    pub fn with_user_id_resolver(self, resolver: Arc<DeepSeekUserIdResolver>) -> Self {
        self.rebuild(|inner| inner.resolve_user_id = Some(resolver))
    }
    pub fn with_attachments(
        self,
        attachments: Arc<dyn AttachmentStore>,
        access: Option<Arc<ImageAttachmentAccessResolver<'static>>>,
    ) -> Self {
        self.rebuild(|inner| {
            inner.attachments = Some(attachments);
            inner.resolve_access = access;
        })
    }
    pub fn with_attachments_resolver(self, resolver: Arc<DeepSeekAttachmentsResolver>) -> Self {
        self.rebuild(|inner| inner.resolve_attachments = Some(resolver))
    }
    pub fn with_file_store(self, store: DeepSeekFileStore) -> Self {
        self.rebuild(|inner| inner.files = store)
    }
    pub fn with_extensions(self, extensions: Service<DeepSeekLlmApiExtensions>) -> Self {
        self.rebuild(|inner| inner.extensions = Some(extensions))
    }
    pub fn with_options_resolver(self, resolver: Arc<DeepSeekOptionsResolver>) -> Self {
        self.rebuild(|inner| inner.options = Some(resolver))
    }
    pub fn with_connection_resolver(self, resolver: Arc<DeepSeekConnectionResolver>) -> Self {
        self.rebuild(|inner| inner.connect = Some(resolver))
    }
    pub fn with_auth_resolver(self, resolver: Arc<DeepSeekAuthResolver>) -> Self {
        self.rebuild(|inner| inner.resolve_auth = Some(resolver))
    }
    pub fn with_model_discovery(self, discovery: Arc<DeepSeekModelDiscovery>) -> Self {
        self.rebuild(|inner| inner.discover_models = Some(discovery))
    }
    pub fn with_model_admission(self, admission: Arc<DeepSeekModelAdmission>) -> Self {
        self.rebuild(|inner| inner.admit = Some(admission))
    }
    pub fn with_replay_reporter(self, reporter: Arc<DeepSeekReplayReporter>) -> Self {
        self.rebuild(|inner| inner.on_replay_degrade = Some(reporter))
    }
    pub fn with_extensions_omitted_reporter(
        self,
        reporter: Arc<DeepSeekExtensionReporter>,
    ) -> Self {
        self.rebuild(|inner| inner.on_extensions_omitted = Some(reporter))
    }
    pub fn with_extensions_unaccepted_reporter(
        self,
        reporter: Arc<DeepSeekExtensionReporter>,
    ) -> Self {
        self.rebuild(|inner| inner.on_extensions_unaccepted = Some(reporter))
    }
    pub fn connection(&self) -> &DeepSeekConnectionOptions {
        &self.inner.connection
    }

    fn options(&self) -> Result<DeepSeekConnectionOptions, BoxError> {
        self.inner
            .options
            .as_ref()
            .map_or_else(|| Ok(self.inner.connection.clone()), |resolver| resolver())
    }

    async fn connected(
        &self,
        signal: Option<AbortSignal>,
    ) -> Result<DeepSeekConnectionOptions, BoxError> {
        match &self.inner.connect {
            Some(connect) => connect(signal).await,
            None => self.options(),
        }
    }

    fn admit(&self, connection: &DeepSeekConnectionOptions, model: &str) -> Result<(), BoxError> {
        self.inner
            .admit
            .as_ref()
            .map_or(Ok(()), |admit| admit(connection, model))
    }

    fn stream_with_connection(
        &self,
        options: GenerateOptions,
        connection: DeepSeekConnectionOptions,
    ) -> Result<LlmStream, BoxError> {
        let driver = Driver {
            inner: self.inner.clone(),
            connection,
            options,
            phase: Phase::Start,
            queue: VecDeque::new(),
            on_request_error: None,
        };
        Ok(LlmStream::new(stream::unfold(
            driver,
            |mut driver| async move { driver.next().await.map(|item| (item, driver)) },
        )))
    }
}

impl LlmAdapter for DeepSeekAdapter {
    fn provider_info(&self, provider: &str) -> Result<LlmProviderInfo, BoxError> {
        Ok(LlmProviderInfo {
            id: provider.into(),
            name: self.inner.provider_name.clone(),
        })
    }
    fn provider_retry_policy(
        &self,
        _provider: &str,
    ) -> Result<Option<ResolvedRetryPolicy>, BoxError> {
        Ok(Some(self.options()?.retry_policy))
    }
    fn list_models<'a>(&'a self, provider: &'a str) -> LlmFuture<'a, Vec<LlmModelInfo>> {
        Box::pin(async move {
            if let Some(discover) = &self.inner.discover_models {
                return discover(provider.to_owned()).await;
            }
            Ok(Vec::new())
        })
    }
    fn resolve_model<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
        signal: Option<&'a AbortSignal>,
    ) -> LlmFuture<'a, LlmResolvedModelInfo> {
        let signal = signal.cloned();
        Box::pin(async move {
            let connection = self.connected(signal).await?;
            Ok(model_info(&connection, provider, model))
        })
    }
    fn image_request_pricing(
        &self,
        _provider: &str,
        model: &str,
    ) -> Option<Arc<dyn LlmImageRequestPricing>> {
        let connection = self.options().ok()?;
        let inner = self.inner.clone();
        let resolve_access = self.inner.resolve_access.clone().map(|resolve| {
            Arc::new(move |image: &hoh_attachment::ImageAttachmentRef| {
                inner.attachments()?;
                resolve(image)
            }) as Arc<ImageAttachmentAccessResolver<'static>>
        });
        Some(Arc::new(deep_seek_image_request_pricing(
            &connection,
            model,
            resolve_access,
        )))
    }
    fn image_request_target(
        &self,
        _provider: &str,
        model: &str,
        image: &hoh_attachment::ImageAttachmentRef,
    ) -> ImageRequestTarget {
        self.options()
            .ok()
            .and_then(|connection| {
                connection
                    .models
                    .into_iter()
                    .find(|entry| entry.id == model)
            })
            .map_or(
                ImageRequestTarget {
                    width: image.width,
                    height: image.height,
                    max_bytes: image.bytes,
                },
                |entry| resolve_request_image_target(&entry, image),
            )
    }
    fn prepare_call(
        self: Arc<Self>,
        provider: String,
        model: String,
        signal: Option<AbortSignal>,
    ) -> LlmFuture<'static, PreparedAdapterCall> {
        Box::pin(async move {
            let connection = self.connected(signal).await?;
            self.admit(&connection, &model)?;
            let resolved = model_info(&connection, &provider, &model);
            let adapter = self.clone();
            Ok(PreparedAdapterCall {
                model: resolved,
                dispatch: Arc::new(move |options| {
                    adapter.stream_with_connection(options, connection.clone())
                }),
            })
        })
    }
    fn stream(&self, options: GenerateOptions) -> Result<LlmStream, BoxError> {
        let connection = self.options()?;
        self.admit(&connection, &options.model)?;
        self.stream_with_connection(options, connection)
    }
}

struct Reading {
    response: reqwest::Response,
    parser: SseParser,
    translator: Translator,
}
enum Phase {
    Start,
    Reading(Box<Reading>),
    Done,
}
struct Driver {
    inner: Arc<Inner>,
    connection: DeepSeekConnectionOptions,
    options: GenerateOptions,
    phase: Phase,
    queue: VecDeque<StreamChunk>,
    on_request_error: Option<Arc<DeepSeekRequestErrorMapper>>,
}

async fn map_request_error(
    mapper: Option<&Arc<DeepSeekRequestErrorMapper>>,
    error: HarnessError,
) -> HarnessError {
    let original = error.clone();
    match mapper {
        Some(mapper) => mapper(error).await.unwrap_or(original),
        None => error,
    }
}

impl Driver {
    async fn guarded<T>(
        &self,
        what: &str,
        future: impl std::future::Future<Output = Result<T, reqwest::Error>>,
    ) -> Result<T, HarnessError> {
        let signal = self.options.signal.as_ref();
        let aborted = async {
            match signal {
                Some(signal) => Some(signal.cancelled().await),
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            biased;
            reason = aborted => {
                if timeout_of(reason.as_ref(), Some("MESSAGES_IDLE")).is_some() { Err(failure("DeepSeek Messages stream idle timeout", Code::Timeout)) }
                else { Err(failure("DeepSeek Messages request aborted", Code::Aborted)) }
            }
            result = tokio::time::timeout(Duration::from_millis(self.connection.stream_idle_timeout_ms as u64), future) => match result {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(error)) => Err(hoh_llm::LlmError::with_options("DeepSeek Messages transport failed", Code::Transport, hoh_llm::LlmErrorOptions { cause: Some(SharedError::new(error)), ..Default::default() }).expect("transport diagnostic is valid")),
                Err(_) => Err(failure(format!("DeepSeek Messages {what} idle timeout"), Code::Timeout)),
            }
        }
    }

    async fn next(&mut self) -> Option<Result<StreamChunk, BoxError>> {
        loop {
            if let Some(chunk) = self.queue.pop_front() {
                return Some(Ok(chunk));
            }
            let result = match std::mem::replace(&mut self.phase, Phase::Done) {
                Phase::Done => return None,
                Phase::Start => self.start().await,
                Phase::Reading(reading) => self.read_more(reading).await,
            };
            if let Err(error) = result {
                self.phase = Phase::Done;
                let error = map_request_error(self.on_request_error.as_ref(), error).await;
                return Some(Err(error.into()));
            }
        }
    }

    async fn start(&mut self) -> Result<(), HarnessError> {
        if let Some(signal) = &self.options.signal {
            signal.throw_if_aborted().map_err(|reason| {
                hoh_llm::LlmError::with_options(
                    "DeepSeek Messages request aborted",
                    Code::Aborted,
                    hoh_llm::LlmErrorOptions {
                        cause: Some(reason),
                        ..Default::default()
                    },
                )
                .expect("abort diagnostic is valid")
            })?;
        }
        let attachments = self.inner.attachments();
        let access = |image: &hoh_attachment::ImageAttachmentRef| {
            self.inner.attachments()?;
            self.inner
                .resolve_access
                .as_ref()
                .and_then(|resolve| resolve(image))
        };
        let (history, images) = prepare_images(
            &self.options.messages,
            &self.connection,
            &self.options.model,
            attachments.as_deref(),
            &access,
            self.options.signal.as_ref(),
        )
        .await?;
        let auth = match &self.inner.resolve_auth {
            Some(resolve) => resolve(self.connection.clone()).await.map_err(|error| {
                HarnessError::of(error.as_ref())
                    .cloned()
                    .unwrap_or_else(|| {
                        hoh_llm::LlmError::with_options(
                            "DeepSeek credential resolution failed",
                            Code::Auth,
                            hoh_llm::LlmErrorOptions {
                                cause: Some(SharedError::new(error)),
                                ..Default::default()
                            },
                        )
                        .expect("credential diagnostic is valid")
                    })
            })?,
            None => DeepSeekRequestAuth {
                headers: self.inner.headers.clone(),
                on_request_error: None,
            },
        };
        self.on_request_error = auth.on_request_error.clone();
        let root = messages_api_root(&self.connection.base_url).map_err(|error| {
            failure(
                format!("DeepSeek Messages base URL is invalid: {error}"),
                Code::InvalidConfig,
            )
        })?;
        let file_connection = DeepSeekFileConnection {
            base_url: self.connection.base_url.clone(),
            headers: auth.headers.clone(),
        };
        let mut files = crate::request_files::RequestFiles::new(
            &self.inner.files,
            &file_connection,
            &self.connection.file_policy,
            self.connection.files_api_timeout_ms,
            self.options.signal.as_ref(),
            Arc::new(|| {}),
        );
        let mut inline = false;
        loop {
            files.begin_attempt();
            let file_ids = if inline {
                None
            } else {
                match prepare_file_ids(&history, &images, &mut files).await {
                    Ok(ids) => Some(ids),
                    Err(error) => match error.into_fatal() {
                        Some(error) => return Err(error),
                        None => {
                            inline = true;
                            continue;
                        }
                    },
                }
            };
            let request_history = if inline {
                inline_images(&history, &images, &self.connection)?
            } else {
                history.clone()
            };
            let mut replay_reporter = |reason: &str| {
                if let Some(reporter) = &self.inner.on_replay_degrade {
                    reporter(&self.options.provider, &self.options.model, reason);
                }
            };
            let body = serialize(
                &self.options,
                &self.connection,
                &request_history,
                &images,
                &access,
                self.inner
                    .on_replay_degrade
                    .as_ref()
                    .map(|_| &mut replay_reporter as &mut dyn FnMut(&str)),
                file_ids.as_ref(),
            )?;
            let (payload, prepared_extensions) = if let Some(extensions) = &self.inner.extensions {
                let value = serde_json::to_value(&body).map_err(|error| {
                    hoh_llm::LlmError::with_options(
                        "DeepSeek Messages request cannot be serialized",
                        Code::InvalidRequest,
                        hoh_llm::LlmErrorOptions {
                            cause: Some(SharedError::new(error)),
                            ..Default::default()
                        },
                    )
                    .expect("serialization diagnostic is valid")
                })?;
                let Value::Object(body_fields) = value else {
                    return Err(failure(
                        "DeepSeek Messages request cannot be serialized",
                        Code::InvalidRequest,
                    ));
                };
                let signal = self
                    .options
                    .signal
                    .clone()
                    .unwrap_or_else(|| AbortController::new().signal());
                let request = DeepSeekLlmApiExtensionRequest {
                    body: body_fields.clone(),
                    session_id: self.options.session_id.as_ref().map(ToString::to_string),
                    purpose: self.options.purpose.clone(),
                    signal,
                };
                let extensions = extensions.clone();
                let reporter = self.inner.on_extensions_omitted.clone();
                let provider = self.options.provider.clone();
                let model = self.options.model.clone();
                let prepared = prepare_request_extensions(
                    body_fields,
                    request,
                    move |request| extensions.prepare(request),
                    move |fields, error| {
                        if let Some(reporter) = &reporter {
                            reporter(&provider, &model, fields, &error.to_string());
                        }
                    },
                )
                .await?;
                (prepared.payload.clone(), Some(prepared))
            } else {
                let payload = stringify_payload(&serde_json::to_value(&body).map_err(|error| {
                    hoh_llm::LlmError::with_options(
                        "DeepSeek Messages request cannot be serialized",
                        Code::InvalidRequest,
                        hoh_llm::LlmErrorOptions {
                            cause: Some(SharedError::new(error)),
                            ..Default::default()
                        },
                    )
                    .expect("serialization diagnostic is valid")
                })?);
                (payload, None)
            };
            let has_tool_changes = body.messages.iter().any(|message| {
                message.content.iter().any(|block| {
                    matches!(
                        block.get("type").and_then(Value::as_str),
                        Some("tool_addition" | "tool_removal")
                    )
                })
            });
            let mut betas = Vec::new();
            if file_ids.as_ref().is_some_and(|ids| !ids.is_empty()) {
                betas.push(MESSAGES_FILES_BETA);
            }
            if has_tool_changes {
                betas.push(MESSAGES_TOOL_CHANGES_BETA);
            }
            let mut request = self
                .inner
                .client
                .post(format!("{root}/messages"))
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .header("anthropic-version", "2023-06-01")
                .body(payload);
            for (name, value) in attribution_headers(None)
                .unwrap_or_default()
                .iter()
                .chain(auth.headers.iter())
            {
                request = request.header(name, value);
            }
            if !betas.is_empty() {
                request = request.header("anthropic-beta", betas.join(","));
            }
            let user_id = self.inner.user_id();
            if let Some(user_id) = user_id {
                request = request.header("x-deepseek-harness-user-id", user_id);
            }
            if let Some(session_id) = &self.options.session_id {
                request = request.header("x-deepseek-harness-session-id", session_id.as_str());
            }
            if self.options.purpose.as_deref() == Some("compaction") {
                request = request.header("x-deepseek-harness-compact", "1");
            }
            let response = self.guarded("request", request.send()).await?;
            let status = response.status().as_u16();
            if (200..300).contains(&status) {
                if let Some(extensions) = prepared_extensions
                    && let Err(error) = extensions.accept().await
                    && let Some(reporter) = &self.inner.on_extensions_unaccepted
                {
                    reporter(
                        &self.options.provider,
                        &self.options.model,
                        &[],
                        &error.to_string(),
                    );
                }
                self.phase = Phase::Reading(Box::new(Reading {
                    response,
                    parser: SseParser::new(),
                    translator: Translator::new(&self.options.model),
                }));
                return Ok(());
            }
            let headers = response.headers().clone();
            let text = self.guarded("error body", response.text()).await?;
            let raw: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let detail = provider_error_detail(&raw);
            if files.retry(&detail).await? {
                continue;
            }
            let mut error = provider_error(&raw, Some(status), Some(&headers));
            let message = files.error_message(status, error.message(), &detail);
            if message != error.message()
                && let Some(failure) = error.detail::<LlmFailure>()
            {
                let mut failure = failure.clone();
                failure.message = message;
                error = hoh_llm::LlmError::from_failure(&failure, None)
                    .expect("provider failure remains valid");
            }
            return Err(error);
        }
    }

    async fn read_more(&mut self, mut reading: Box<Reading>) -> Result<(), HarnessError> {
        let next = self.guarded("stream", reading.response.chunk()).await?;
        let Some(bytes) = next else {
            reading.translator.finish()?;
            self.phase = Phase::Done;
            return Ok(());
        };
        let frames = reading.parser.push(&bytes).map_err(|_| {
            failure(
                "DeepSeek Messages SSE line is too long",
                Code::MalformedResponse,
            )
        })?;
        for frame in frames {
            let event = decode_event(frame)?;
            self.queue.extend(reading.translator.feed(&event)?);
        }
        self.phase = if reading.translator.finish().is_ok()
            && self
                .queue
                .iter()
                .any(|chunk| matches!(chunk, StreamChunk::Finish { .. }))
        {
            Phase::Done
        } else {
            Phase::Reading(reading)
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn test_error(message: &str, code: Code) -> HarnessError {
        hoh_llm::LlmError::new(message, code).expect("test diagnostic is valid")
    }

    #[tokio::test]
    async fn request_error_mapper_replaces_the_original_failure() {
        let mapper: Arc<DeepSeekRequestErrorMapper> =
            Arc::new(|_| Box::pin(async { Ok(test_error("mapped", Code::Auth)) }));
        let mapped =
            map_request_error(Some(&mapper), test_error("original", Code::Transport)).await;

        assert_eq!(mapped.message(), "mapped");
        assert_eq!(mapped.code(), "AUTH");
    }

    #[tokio::test]
    async fn request_error_mapper_failure_preserves_the_original_failure() {
        let mapper: Arc<DeepSeekRequestErrorMapper> = Arc::new(|_| {
            Box::pin(async {
                Err::<HarnessError, BoxError>(
                    std::io::Error::other("credential storage unavailable").into(),
                )
            })
        });
        let original = test_error("original", Code::RateLimit);
        let mapped = map_request_error(Some(&mapper), original).await;

        assert_eq!(mapped.message(), "original");
        assert_eq!(mapped.code(), "RATE_LIMIT");
    }

    #[test]
    fn operation_local_identity_and_attachment_resolvers_override_static_values() {
        let connection = crate::config::resolve_adapter_options(&crate::types::Options::default())
            .expect("default connection is valid");
        let attachment_calls = Arc::new(AtomicUsize::new(0));
        let adapter = DeepSeekAdapter::new(connection)
            .unwrap()
            .with_user_id("static")
            .with_user_id_resolver(Arc::new(|| "dynamic".into()))
            .with_attachments_resolver(Arc::new({
                let calls = attachment_calls.clone();
                move || {
                    calls.fetch_add(1, Ordering::Relaxed);
                    None
                }
            }));

        assert_eq!(adapter.inner.user_id().as_deref(), Some("dynamic"));
        assert!(adapter.inner.attachments().is_none());
        assert!(adapter.inner.attachments().is_none());
        assert_eq!(attachment_calls.load(Ordering::Relaxed), 2);
    }
}
