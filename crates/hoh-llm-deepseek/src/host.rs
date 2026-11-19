//! Rust host wiring for the DeepSeek adapter.

use crate::{DeepSeekAdapter, ListingCacheDependencies, ModelListingCache};
use hoh_deepseek_llm_api_extensions::DeepSeekLlmApiExtensions;
use hoh_error::BoxError;
use hoh_kernel::Context;
use hoh_llm::{AdapterRegistrationHandle, LlmRuntime};
use std::sync::Arc;

pub fn create_model_listing_cache(dependencies: ListingCacheDependencies) -> ModelListingCache {
    ModelListingCache::new(dependencies)
}

pub fn register_deep_seek_provider(
    ctx: &Context,
    runtime: &LlmRuntime,
    provider: impl Into<String>,
    mut adapter: DeepSeekAdapter,
) -> Result<AdapterRegistrationHandle, BoxError> {
    if let Some(extensions) = ctx.get::<DeepSeekLlmApiExtensions>() {
        adapter = adapter.with_extensions(extensions);
    }
    runtime.register_adapter(ctx, vec![provider.into()], Arc::new(adapter))
}
