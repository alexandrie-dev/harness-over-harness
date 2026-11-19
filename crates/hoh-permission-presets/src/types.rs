//! Pure client-visible permission preset vocabulary.

use serde::{Deserialize, Serialize};

/// Presentation for an available preset or the derived `custom` current value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetOption {
    /// Stable option value: a configured preset key, live `auto`, or derived `custom`.
    pub value: String,
    /// The display label.
    pub name: String,
    /// One user-facing sentence on what the value means; omitted when not configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Process-level permission catalog, separate from session history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionCatalog {
    /// Every currently selectable preset, in contribution order.
    pub options: Vec<PresetOption>,
    /// Configured presets eligible as defaults for future sessions.
    pub default_options: Vec<PresetOption>,
    /// Effective default when the config field is omitted.
    pub default_preset: String,
}

/// Whole `permissions` session projection: current durable selection only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionSelection {
    /// Effective current value: a configured preset key, `auto`, or `custom`.
    pub current_value: String,
}
