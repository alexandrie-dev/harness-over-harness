use serde::{Deserialize, Serialize};
use serde_json::Value;

// Nominal id of one profile plugin entry.
hoh_brand::branded_string!(pub SettingsNamespace);

/// One schema-declared secret slot inside a redacted namespace value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSecretView {
    pub path: Vec<String>,
    pub set: bool,
}

/// Client-safe view of one live configuration form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsNamespaceView {
    pub auto_generate: bool,
    pub ns: String,
    pub schema: Value,
    pub value: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<Value>,
    pub applies: SettingsApplyMode,
    pub secrets: Vec<SettingsSecretView>,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingsApplyMode {
    Live,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum SettingsPathOpView {
    Set { path: Vec<String>, value: Value },
    Unset { path: Vec<String> },
}

impl SettingsPathOpView {
    pub fn path(&self) -> &[String] {
        match self {
            Self::Set { path, .. } | Self::Unset { path } => path,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDescribeValue {
    pub writable: bool,
    pub has_document: bool,
    pub namespaces: Vec<SettingsNamespaceView>,
}
