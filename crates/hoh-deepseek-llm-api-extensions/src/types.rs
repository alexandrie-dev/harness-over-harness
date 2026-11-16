//! Provider-specific JSON and contribution types for DeepSeek request extensions.

use std::sync::{Arc, Mutex};

use futures_util::{
    FutureExt,
    future::{BoxFuture, Shared, join_all},
};
use hoh_error::{BoxError, SharedError};
use hoh_timeout::AbortSignal;
use serde_json::Map;

/// Lossless JSON values supported by provider-specific request fields.
pub type DeepSeekLlmApiJson = serde_json::Value;

/// Open table of top-level extension fields; contributors own their value types.
pub type DeepSeekLlmApiExtensionMap = Map<String, DeepSeekLlmApiJson>;

/// Exact serialized request facts shared immutably with every provider.
#[derive(Clone, Debug)]
pub struct DeepSeekLlmApiExtensionRequest {
    pub body: DeepSeekLlmApiExtensionMap,
    pub session_id: Option<String>,
    pub purpose: Option<String>,
    pub signal: AbortSignal,
}

/// A captured commit; capture Arc state to preserve the prepared result receiver.
pub type AcceptanceCallback =
    Arc<dyn Fn() -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync>;

/// A prepared contribution and its optional commit after HTTP 2xx.
pub struct PreparedDeepSeekLlmApiExtension<T = DeepSeekLlmApiJson> {
    pub value: T,
    pub accept: Option<AcceptanceCallback>,
}

/// One independently owned top-level request field.
pub trait DeepSeekLlmApiExtensionProvider<T = DeepSeekLlmApiJson>: Send + Sync + 'static {
    fn prepare(
        &self,
        request: Arc<DeepSeekLlmApiExtensionRequest>,
    ) -> BoxFuture<'static, Result<Option<PreparedDeepSeekLlmApiExtension<T>>, BoxError>>;
}

impl<T, F, Fut> DeepSeekLlmApiExtensionProvider<T> for F
where
    F: Fn(Arc<DeepSeekLlmApiExtensionRequest>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Option<PreparedDeepSeekLlmApiExtension<T>>, BoxError>>
        + Send
        + 'static,
{
    fn prepare(
        &self,
        request: Arc<DeepSeekLlmApiExtensionRequest>,
    ) -> BoxFuture<'static, Result<Option<PreparedDeepSeekLlmApiExtension<T>>, BoxError>> {
        self(request).boxed()
    }
}

type Acceptance = Shared<BoxFuture<'static, Result<(), SharedError>>>;

/// Detached, frozen field values and one idempotent joint acceptance transaction.
#[derive(Clone)]
pub struct PreparedDeepSeekLlmApiExtensions {
    pub(crate) fields: Arc<DeepSeekLlmApiExtensionMap>,
    pub(crate) callbacks: Arc<Vec<AcceptanceCallback>>,
    pub(crate) acceptance: Arc<Mutex<Option<Acceptance>>>,
}

impl PreparedDeepSeekLlmApiExtensions {
    /// Read fields in JavaScript enumeration order; no mutable reference is exposed.
    pub fn fields(&self) -> &DeepSeekLlmApiExtensionMap {
        &self.fields
    }

    /// Start every captured callback once. All calls join the same settlement,
    /// even when an earlier caller drops its returned future.
    pub fn accept(&self) -> impl Future<Output = Result<(), SharedError>> + Send + 'static {
        let mut acceptance = super::lock(&self.acceptance);
        acceptance
            .get_or_insert_with(|| {
                let callbacks = self.callbacks.clone();
                let work = async move {
                    let outcomes = join_all(
                        callbacks
                            .iter()
                            .map(|callback| async move { callback().await }),
                    )
                    .await;
                    let mut failures: Vec<_> = outcomes
                        .into_iter()
                        .filter_map(Result::err)
                        .map(SharedError::new)
                        .collect();
                    match failures.len() {
                        0 => Ok(()),
                        1 => Err(failures.remove(0)),
                        _ => Err(SharedError::new(super::ExtensionError::Aggregate {
                            errors: failures,
                        })),
                    }
                }
                .boxed()
                .shared();
                // A JavaScript promise keeps running when its waiter is dropped.
                // Acceptance also remains valid after the registering fiber unloads.
                let driver = work.clone();
                tokio::spawn(async move {
                    let _ = driver.await;
                });
                work
            })
            .clone()
    }
}
