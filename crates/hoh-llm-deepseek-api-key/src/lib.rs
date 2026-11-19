//! API-key authenticated DeepSeek route and official model listing.

mod config;
mod listing;
mod provider;
mod shim;

pub use config::{
    Config, DeepSeekCatalogModel, FilePolicy, ImagePixelBudget, ImagePixelBudgetLow,
    ListingSettings, Options, ReasoningEffort, RequestDefaults, ResolveError,
    ResolvedDeepSeekOptions, Thinking, plain_options, resolve_adapter_options,
};
pub use provider::{DeepSeekApiKey, Deps, apply};

/// Source plugin name.
pub const NAME: &str = "llm-deepseek-api-key";
/// Required capability names.
pub const INJECT: &[&str] = &["llm"];
