//! Port of @deepseek-ai/dsh-official-bundles (packages/util/official-bundles).
//! Installation-owned bundle selection and offline discovery metadata.

pub mod bundles;
pub mod catalog_generated;
pub mod plugin_manager;

pub use bundles::{
    DEFAULT_PROFILE_BUNDLES, INSTALLATION_OWNED_PROFILE_TUPLES, ON_DEMAND_BUNDLES,
    OPTIONAL_BUNDLES, PROFILE_TEMPLATES, ProfileTemplate, RETIRED_BUNDLES,
};
pub use catalog_generated::OFFICIAL_ON_DEMAND_CATALOG;
pub use plugin_manager::{
    EXPERIMENTAL_BUNDLES, EXTENSION_BUNDLES, HIDDEN_BUNDLES, MORE_BUNDLES, PROTECTED_PLUGINS,
};

use hoh_package_manifest::LocalizedText;
use serde::{Deserialize, Serialize};

/// Offline discovery metadata without an installation state or requested version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialBundleCatalogEntry {
    pub package_name: String,
    /// Same optional host platform declaration as `DshBundleManifest::platforms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platforms: Option<Vec<String>>,
    pub meta: OfficialBundleCatalogMeta,
}

/// The required title, description, and icon picked from `PluginLocalizedMeta`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfficialBundleCatalogMeta {
    pub title: LocalizedText,
    pub description: LocalizedText,
    pub icon: String,
}
