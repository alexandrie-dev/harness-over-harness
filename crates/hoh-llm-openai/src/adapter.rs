//! `LlmAdapter` for an OpenAI-compatible server.
use crate::compat::effort_ids;
use crate::config::OpenAiConfig;
use crate::error::{
    classify, error_fields, failure, http_error, redact, rejects_limit_field, retry_after_ms,
    truncate,
};
use crate::http::{authorized, client, guarded, model_ids, user_agent};
use crate::request::{build_body, limit_field, swap_limit_field};
use crate::sse::{SseEvent, SseParser};
use crate::stream::Translator;
use hoh_attachment::AttachmentStore;
use hoh_error::{BoxError, HarnessError};
use hoh_llm::{
    BuiltinLlmFailureCode as Code, GenerateOptions, LlmAdapter, LlmFuture, LlmModelContext,
    LlmModelInfo, LlmModelReasoningInfo, LlmProviderInfo, LlmReasoningEffortInfo,
    LlmResolvedModelInfo, LlmStream, StreamChunk,
};
use hoh_timeout::AbortSignal;
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

struct Inner {
    config: OpenAiConfig,
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
    user_agent: String,
    attachments: Option<Arc<dyn AttachmentStore>>,
    /// The server refused the output-limit field of the table and accepted the other one:
    /// later requests start with the other one.
    swapped_limit_field: AtomicBool,
}

/// Cheap to clone: the state is shared.
#[derive(Clone)]
pub struct OpenAiAdapter {
    inner: Arc<Inner>,
}

impl OpenAiAdapter {
    pub fn new(config: OpenAiConfig) -> Result<Self, HarnessError> {
        config.validate()?;
        let api_key = match config.api_key.as_deref().filter(|k| !k.trim().is_empty()) {
            Some(key) => Some(hoh_llm::assert_usable_api_key(
                key,
                "hoh-llm-openai",
                "apiKey",
            )?),
            None => None,
        };
        Ok(Self {
            inner: Arc::new(Inner {
                base_url: config.normalized_base_url()?,
                client: client(config.connect_timeout_ms)?,
                user_agent: user_agent(),
                api_key,
                attachments: None,
                swapped_limit_field: AtomicBool::new(false),
                config,
            }),
        })
    }

    /// Needed only to send images: their bytes are read from this store.
    pub fn with_attachments(self, store: Arc<dyn AttachmentStore>) -> Self {
        let inner = &self.inner;
        Self {
            inner: Arc::new(Inner {
                config: inner.config.clone(),
                base_url: inner.base_url.clone(),
                api_key: inner.api_key.clone(),
                client: inner.client.clone(),
                user_agent: inner.user_agent.clone(),
                attachments: Some(store),
                swapped_limit_field: AtomicBool::new(
                    inner.swapped_limit_field.load(Ordering::Relaxed),
                ),
            }),
        }
    }

    pub fn config(&self) -> &OpenAiConfig {
        &self.inner.config
    }

    fn modalities(&self) -> Vec<String> {
        let mut modalities = vec!["text".to_owned()];
        if self.inner.config.model.image_input {
            modalities.push("image".into());
        }
        modalities
    }

    fn model_info(&self, provider: &str, model: &str) -> LlmModelInfo {
        // A model name the caller passed in may contain the key (a mistake, but it must not travel on).
        let model = crate::error::redact_text(model, self.inner.api_key.as_deref());
        LlmModelInfo {
            provider: provider.into(),
            id: model.as_str().into(),
            name: model.as_str().into(),
            input_modalities: Some(self.modalities()),
            ..Default::default()
        }
    }
}

impl std::fmt::Debug for OpenAiAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The configuration's own `Debug` hides the API key; `Inner` holds a second copy of it.
        f.debug_struct("OpenAiAdapter")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

impl LlmAdapter for OpenAiAdapter {
    fn provider_info(&self, provider: &str) -> Result<LlmProviderInfo, BoxError> {
        Ok(LlmProviderInfo {
            id: provider.into(),
            name: self.inner.config.provider_name.clone(),
        })
    }

    fn list_models<'a>(&'a self, provider: &'a str) -> LlmFuture<'a, Vec<LlmModelInfo>> {
        Box::pin(async move {
            let inner = &self.inner;
            let listing = async {
                let idle = Duration::from_millis(inner.config.stream_idle_timeout_ms);
                let request = authorized(
                    inner.client.get(format!("{}/models", inner.base_url)),
                    inner.api_key.as_deref(),
                    &inner.user_agent,
                );
                let response = guarded(None, idle, "model list", request.send()).await?;
                let status = response.status().as_u16();
                let text = guarded(None, idle, "model list", response.text()).await?;
                if !(200..300).contains(&status) {
                    return Err(http_error(status, &text, None, None));
                }
                let body: Value = serde_json::from_str(&text).map_err(|e| {
                    failure(
                        Code::MalformedResponse,
                        format!("model list is not JSON: {e}"),
                    )
                })?;
                Ok::<_, HarnessError>(
                    model_ids(&body, inner.api_key.as_deref())
                        .into_iter()
                        .map(|id| self.model_info(provider, &id))
                        .collect(),
                )
            };
            listing
                .await
                .map_err(|e| redact(e, inner.api_key.as_deref()).into())
        })
    }

    fn resolve_model<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
        _signal: Option<&'a AbortSignal>,
    ) -> LlmFuture<'a, LlmResolvedModelInfo> {
        Box::pin(async move {
            let settings = &self.inner.config.model;
            let efforts: Vec<_> = effort_ids(self.inner.config.compat().thinking_control)
                .iter()
                .map(|(id, name)| LlmReasoningEffortInfo {
                    id: (*id).into(),
                    name: (*name).into(),
                    description: None,
                })
                .collect();
            Ok(LlmResolvedModelInfo {
                model: self.model_info(provider, model),
                context: settings
                    .context_window
                    .map(|context_window| LlmModelContext { context_window }),
                default_max_tokens: settings.default_max_tokens,
                reasoning: (!efforts.is_empty()).then_some(LlmModelReasoningInfo {
                    efforts,
                    default_effort: None,
                }),
                ..Default::default()
            })
        })
    }

    fn stream(&self, options: GenerateOptions) -> Result<LlmStream, BoxError> {
        let driver = Driver {
            inner: self.inner.clone(),
            options,
            phase: Phase::Start,
            queue: VecDeque::new(),
        };
        Ok(LlmStream::new(futures_util::stream::unfold(
            driver,
            |mut driver| async move { driver.next().await.map(|item| (item, driver)) },
        )))
    }
}

struct Reading {
    response: reqwest::Response,
    parser: SseParser,
    translator: Translator,
    done_seen: bool,
}

enum Phase {
    Start,
    Reading(Box<Reading>),
    Done,
}

struct Driver {
    inner: Arc<Inner>,
    options: GenerateOptions,
    phase: Phase,
    queue: VecDeque<StreamChunk>,
}

impl Driver {
    fn idle(&self) -> Duration {
        Duration::from_millis(self.inner.config.stream_idle_timeout_ms)
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
                self.queue.clear();
                // Nothing the server wrote may carry the API key out of the adapter.
                return Some(Err(redact(error, self.inner.api_key.as_deref()).into()));
            }
        }
    }

    /// Send the request and, for a plain JSON reply, translate it at once.
    async fn start(&mut self) -> Result<(), HarnessError> {
        let inner = self.inner.clone();
        let signal = self.options.signal.clone();
        let idle = self.idle();
        if let Some(signal) = &signal {
            signal
                .throw_if_aborted()
                .map_err(|r| crate::http::aborted(Some(r)))?;
        }
        let swapped = inner.swapped_limit_field.load(Ordering::Relaxed);
        let mut body = build_body(
            &inner.config,
            &self.options,
            inner.attachments.as_ref(),
            swapped,
        )
        .await?;
        let mut field = limit_field(&inner.config, swapped);
        let mut retried = false;
        let response = loop {
            let payload = serde_json::to_string(&body).map_err(|e| {
                failure(
                    Code::InvalidRequest,
                    format!("request cannot be serialized: {e}"),
                )
            })?;
            let request = authorized(
                inner
                    .client
                    .post(format!("{}/chat/completions", inner.base_url)),
                inner.api_key.as_deref(),
                &inner.user_agent,
            )
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .body(payload);
            let response = guarded(signal.as_ref(), idle, "request", request.send()).await?;
            if response.status().is_success() {
                if retried {
                    inner.swapped_limit_field.store(!swapped, Ordering::Relaxed);
                }
                break response;
            }
            let status = response.status().as_u16();
            let header = |name: &str| {
                response
                    .headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            };
            let retry_after = header("retry-after").and_then(|v| retry_after_ms(&v));
            let request_id = header("x-request-id");
            let text = guarded(signal.as_ref(), idle, "error body", response.text()).await?;
            let error = http_error(status, &text, retry_after, request_id);
            // `max_tokens` against `max_completion_tokens`: one retry with the other spelling.
            if !retried && self.options.max_tokens.is_some() && rejects_limit_field(&error, field) {
                let other = crate::compat::other_limit_field(field);
                swap_limit_field(&mut body, field, other);
                field = other;
                retried = true;
                continue;
            }
            return Err(error);
        };
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let mut translator = Translator::new(&inner.config, self.options.tools.clone());
        let is_json = header("content-type").is_some_and(|t| t.contains("json"));
        if is_json {
            // The server ignored `stream: true`: one chat.completion object.
            let text = guarded(signal.as_ref(), idle, "response body", response.text()).await?;
            let value: Value = serde_json::from_str(&text).map_err(|e| {
                failure(
                    Code::MalformedResponse,
                    format!("response is not valid JSON ({e}): {}", truncate(&text, 200)),
                )
            })?;
            self.queue.extend(translator.feed(&value)?);
            self.queue.extend(translator.finish(true)?);
            return Ok(());
        }
        self.phase = Phase::Reading(Box::new(Reading {
            response,
            parser: SseParser::new(),
            translator,
            done_seen: false,
        }));
        Ok(())
    }

    async fn read_more(&mut self, mut reading: Box<Reading>) -> Result<(), HarnessError> {
        let signal = self.options.signal.clone();
        let next = guarded(
            signal.as_ref(),
            self.idle(),
            "stream",
            reading.response.chunk(),
        )
        .await?;
        let eof = next.is_none();
        let events = match next {
            Some(bytes) => reading.parser.push(&bytes).map_err(|_| {
                failure(
                    Code::MalformedResponse,
                    "OpenAI-compatible SSE line exceeds the size limit".into(),
                )
            })?,
            None => reading.parser.finish().into_iter().collect(),
        };
        for event in events {
            if handle_event(&mut reading, &event, &mut self.queue)? {
                break;
            }
        }
        if reading.done_seen || eof {
            // Dropping `reading` closes the connection; a server still sending is not waited for.
            self.queue
                .extend(reading.translator.finish(reading.done_seen)?);
        } else {
            self.phase = Phase::Reading(reading);
        }
        Ok(())
    }
}

/// Returns true once `[DONE]` was seen.
fn handle_event(
    reading: &mut Reading,
    event: &SseEvent,
    queue: &mut VecDeque<StreamChunk>,
) -> Result<bool, HarnessError> {
    let data = event.data.trim();
    if data == "[DONE]" {
        reading.done_seen = true;
        return Ok(true);
    }
    if data.is_empty() {
        return Ok(false);
    }
    let value: Value = serde_json::from_str(data).map_err(|e| {
        failure(
            Code::MalformedResponse,
            format!(
                "OpenAI-compatible SSE data is not valid JSON ({e}): {}",
                truncate(data, 200)
            ),
        )
    })?;
    if event.event.as_deref() == Some("error") {
        let mut fields = error_fields(&value).unwrap_or_default();
        fields.message.get_or_insert_with(|| truncate(data, 500));
        return Err(classify(&fields, None, None, None));
    }
    queue.extend(reading.translator.feed(&value)?);
    Ok(false)
}
