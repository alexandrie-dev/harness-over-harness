//! Port of @deepseek-ai/dsh-deepseek-llm-api-extensions (packages/llm/deepseek-llm-api-extensions).
//! Effect-scoped providers of additive official DeepSeek API request fields.

pub mod types;
pub use types::*;

use std::sync::{Arc, Mutex, MutexGuard};

use futures_util::{
    FutureExt,
    future::{BoxFuture, join_all},
};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Disposer, NoConfig, Plugin, Undo};
use indexmap::{IndexMap, IndexSet};
use serde::Serialize;

/// Failures local to the registry. A single acceptance failure is forwarded
/// directly as SharedError, rather than wrapped in Aggregate.
#[derive(Debug, thiserror::Error)]
pub enum ExtensionError {
    #[error("deepseek-llm-api-extensions: field must be a non-blank trimmed string")]
    InvalidField,
    #[error("deepseek-llm-api-extensions: field {} is already registered", quote_field(.field))]
    DuplicateField { field: String },
    #[error("DeepSeek LLM API extension acceptance failed")]
    Aggregate { errors: Vec<hoh_error::SharedError> },
}

fn quote_field(field: &str) -> String {
    hoh_util_values::js_json_stringify(&DeepSeekLlmApiJson::String(field.into()))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

trait ErasedProvider: Send + Sync {
    fn prepare(
        &self,
        request: Arc<DeepSeekLlmApiExtensionRequest>,
    ) -> BoxFuture<'static, Result<Option<PreparedDeepSeekLlmApiExtension>, BoxError>>;
}

struct Provider<T, P> {
    provider: Arc<P>,
    marker: std::marker::PhantomData<fn() -> T>,
}

impl<T, P> ErasedProvider for Provider<T, P>
where
    T: Serialize + Send + 'static,
    P: DeepSeekLlmApiExtensionProvider<T>,
{
    fn prepare(
        &self,
        request: Arc<DeepSeekLlmApiExtensionRequest>,
    ) -> BoxFuture<'static, Result<Option<PreparedDeepSeekLlmApiExtension>, BoxError>> {
        let prepared = self.provider.prepare(request);
        async move {
            let Some(result) = prepared.await? else {
                return Ok(None);
            };
            Ok(Some(PreparedDeepSeekLlmApiExtension {
                value: serde_json::to_value(result.value)?,
                accept: result.accept,
            }))
        }
        .boxed()
    }
}

#[derive(Default)]
struct State {
    providers: IndexMap<String, Arc<dyn ErasedProvider>>,
    warned_fields: IndexSet<String>,
}

/// Registry of independently owned fields, also the class-plugin entry point.
pub struct DeepSeekLlmApiExtensionRegistry {
    ctx: Context,
    state: Arc<Mutex<State>>,
}

hoh_kernel::service!(pub DeepSeekLlmApiExtensions: DeepSeekLlmApiExtensionRegistry = "deepseekLlmApiExtensions");

impl Plugin for DeepSeekLlmApiExtensionRegistry {
    const NAME: &'static str = "DeepSeekLlmApiExtensionRegistry";
    type Config = NoConfig;
    type Deps = ();

    async fn apply(ctx: Context, _deps: (), _config: Cfg<NoConfig>) -> Result<(), BoxError> {
        let registry = Arc::new(Self::new(ctx.clone()));
        ctx.provide::<DeepSeekLlmApiExtensions>(registry).await?;
        Ok(())
    }
}

impl DeepSeekLlmApiExtensionRegistry {
    /// Construct the service value; Plugin::apply publishes it to the context.
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    /// Claim one field as an effect of the calling context, as Cordis's service
    /// proxy does. Disposing the registration or its owner releases the field.
    pub fn register<T, P>(
        &self,
        ctx: &Context,
        field: &str,
        provider: P,
    ) -> Result<Disposer, BoxError>
    where
        T: Serialize + Send + 'static,
        P: DeepSeekLlmApiExtensionProvider<T>,
    {
        if field.is_empty() || field.trim_matches(js_whitespace) != field {
            return Err(ExtensionError::InvalidField.into());
        }
        let state = self.state.clone();
        let field = field.to_owned();
        let provider: Arc<dyn ErasedProvider> = Arc::new(Provider {
            provider: Arc::new(provider),
            marker: std::marker::PhantomData,
        });
        // The exact dynamic label awaits owned labels in hoh-kernel (PORT_NOTES §1).
        ctx.try_effect("deepseekLlmApiExtensions.register", move |undos| {
            {
                let mut state = lock(&state);
                if state.providers.contains_key(&field) {
                    return Err(ExtensionError::DuplicateField { field }.into());
                }
                state.providers.insert(field.clone(), provider);
            }
            undos.push(Undo::sync(move || {
                lock(&state).providers.shift_remove(&field);
            }))?;
            Ok(())
        })
    }

    /// Eagerly snapshot the providers, prepare them concurrently and detach JSON.
    /// Cancellation rejects with the exact shared reason, without waiting for
    /// providers that ignore it. Their work remains owned by the registry fiber.
    pub fn prepare(
        &self,
        request: DeepSeekLlmApiExtensionRequest,
    ) -> BoxFuture<'static, Result<PreparedDeepSeekLlmApiExtensions, BoxError>> {
        if let Err(reason) = request.signal.throw_if_aborted() {
            return futures_util::future::ready(Err(reason.into())).boxed();
        }
        let entries: Vec<_> = lock(&self.state)
            .providers
            .iter()
            .map(|(field, provider)| (field.clone(), provider.clone()))
            .collect();
        let request = Arc::new(request);
        let signal = request.signal.clone();
        let state = self.state.clone();
        let logger = self.ctx.logger().named("deepseekLlmApiExtensions");
        let work: Vec<_> = entries.into_iter().map(|(field, provider)| {
            let request = request.clone();
            let state = state.clone();
            let logger = logger.clone();
            // Invoke the provider's synchronous prefix now, before returning
            // the eager preparation future, just as the source async map does.
            let prepared = provider.prepare(request.clone());
            async move {
                match prepared.await {
                    Ok(Some(result)) => Some((field, result)),
                    Ok(None) => None,
                    Err(error) => {
                        let warn = !request.signal.aborted() && lock(&state).warned_fields.insert(field.clone());
                        if warn {
                            // Native kernel logging cannot retain the source format/error pair (PORT_NOTES §2).
                            logger.warn(format_args!("deepseek-llm-api-extensions: omitting field {} from this request because its preparation failed: {error}", quote_field(&field)));
                        }
                        None
                    }
                }
            }
        }).collect();
        let (send, receive) = tokio::sync::oneshot::channel();
        let spawned = self.ctx.spawn(
            "deepseekLlmApiExtensions.prepare",
            move |cancel| async move {
                let prepared = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Ok(()),
                    prepared = join_all(work) => prepared,
                };
                let _ = send.send(prepared);
                Ok(())
            },
        );
        async move {
            let driver = spawned?;
            let prepared = tokio::select! {
                biased;
                reason = signal.cancelled() => return Err(reason.into()),
                prepared = receive => prepared,
            };
            signal.throw_if_aborted()?;
            let prepared = prepared?;
            driver.dispose().await;
            let mut fields = DeepSeekLlmApiExtensionMap::new();
            let mut callbacks = Vec::new();
            for (field, result) in prepared.into_iter().flatten() {
                fields.insert(field, result.value);
                if let Some(accept) = result.accept {
                    callbacks.push(accept);
                }
            }
            // JS object enumeration puts canonical array-index names first;
            // acceptance callbacks still retain the original provider order.
            let mut ordered: Vec<_> = fields.into_iter().collect();
            ordered.sort_by_key(|(field, _)| field_order(field));
            Ok(PreparedDeepSeekLlmApiExtensions {
                fields: Arc::new(ordered.into_iter().collect()),
                callbacks: Arc::new(callbacks),
                acceptance: Arc::new(Mutex::new(None)),
            })
        }
        .boxed()
    }
}

fn field_order(field: &str) -> (u8, u32) {
    match field.parse::<u32>() {
        Ok(index) if index != u32::MAX && index.to_string() == field => (0, index),
        _ => (1, 0),
    }
}

// ECMAScript WhiteSpace + LineTerminator: Rust's trim includes U+0085 and
// excludes U+FEFF, so the ordinary str::trim would validate different fields.
fn js_whitespace(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
        | '\u{205f}' | '\u{3000}' | '\u{feff}')
}
