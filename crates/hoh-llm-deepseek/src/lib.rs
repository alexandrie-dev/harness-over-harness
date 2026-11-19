//! DeepSeek Messages transport, request configuration, and model capabilities.

pub mod adapter;
pub mod config;
pub mod credential;
pub mod defaults;
pub mod file_id;
pub mod file_store;
pub mod files_api;
pub mod host;
pub mod image_tokens;
pub mod images;
pub mod listed_catalog;
pub mod listing;
pub mod listing_cache;
pub mod listing_config;
pub mod messages_api;
pub mod model_info;
pub mod models;
pub mod replay;
pub mod request_extensions;
pub mod request_files;
pub mod request_pricing;
pub mod serialize;
pub mod sse;
pub mod translate;
pub mod transport;
pub mod types;
pub mod upload_index;
pub mod wire_types;

pub use adapter::{
    DeepSeekAdapter, DeepSeekAttachmentsResolver, DeepSeekAuthResolver, DeepSeekConnectionResolver,
    DeepSeekExtensionReporter, DeepSeekModelAdmission, DeepSeekModelDiscovery,
    DeepSeekOptionsResolver, DeepSeekReplayReporter, DeepSeekUserIdResolver,
};
pub use config::{
    Options, PUBLIC_BASE_URL, ResolvedDeepSeekOptions, is_built_in_catalog,
    resolve_adapter_options, resolve_adapter_options_with_environment,
};
pub use credential::{
    DeepSeekAccount, DeepSeekAccountApi, deep_seek_api_key_configured,
    deep_seek_api_key_configured_from_sources, reject_deep_seek_account_token,
    resolve_deep_seek_account_token, resolve_deep_seek_api_key,
    resolve_deep_seek_api_key_from_sources,
};
pub use defaults::*;
pub use file_id::{DeepSeekFileId, DeepSeekFileScope};
pub use file_store::{
    DeepSeekFileConnection, DeepSeekFileReference, DeepSeekFileStore, FILES_API_TIMEOUT,
    MAX_IMAGE_BYTES,
};
pub use files_api::{
    DeepSeekFileObject, DeepSeekFilePage, DeepSeekFilesClient, MAX_FILE_EXPIRY_SECONDS,
    MAX_FILE_UPLOAD_BYTES, MAX_STORED_FILE_BYTES, MAX_STORED_FILE_COUNT, MIN_FILE_EXPIRY_SECONDS,
};
pub use host::{create_model_listing_cache, register_deep_seek_provider};
pub use image_tokens::{deep_seek_image_tokens, deep_seek_request_image_dimensions};
pub use listed_catalog::{
    CatalogRefusal, ListedCatalog, ListedCatalogInput, ModelOverride, refuse_listed_model,
    resolve_listed_catalog,
};
pub use listing::{
    ClientRequirement, LISTING_TIMEOUT, ListedBadge, ListedCapabilities, ListedFacts, ListingEntry,
    MAX_BADGE_TEXT_LENGTH, ModelListing, model_listing_url, parse_model_listing,
    request_model_listing,
};
pub use listing_cache::{ListingCacheDependencies, ListingView, ModelListingCache};
pub use listing_config::{
    DEFAULT_MODELS_MAX_AGE_MS, DEFAULT_MODELS_TIMEOUT_MS, ListingSettings, ListingTiming,
    ModelListingOptions, resolve_listing_timing,
};
pub use model_info::catalog_model_info;
pub use request_pricing::{
    DEFAULT_LOW_DETAIL_IMAGE_PIXEL_BUDGET, DEFAULT_MAX_IMAGES_PER_REQUEST,
    DEFAULT_MAX_REQUEST_FILES_BYTES, DEFAULT_REQUEST_IMAGE_MAX_BYTES, DeepSeekImageRequestPricing,
    REQUEST_IMAGE_MAX_DIMENSION, deep_seek_image_request_pricing, resolve_request_image_max_bytes,
    resolve_request_image_target,
};
pub use types::*;
pub use upload_index::{
    DeepSeekUploadIndex, DeepSeekUploadRecord, UploadIndexCommit, deep_seek_file_scope,
};
