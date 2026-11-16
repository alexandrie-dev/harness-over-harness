//! Author declarations only: readers own validation and defaults.
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshPackageManifest {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optional_dependencies: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_dependencies: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engines: Option<DshEnginesManifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dsh: Option<DshManifest>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshManifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_version: Option<ManifestVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<DshBundleManifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<DshProfileManifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<DshClientManifest>,
}

/// The source's numeric literal `1` (not a string or an arbitrary integer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManifestVersion;
impl Serialize for ManifestVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(1)
    }
}
impl<'de> Deserialize<'de> for ManifestVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if u8::deserialize(deserializer)? == 1 {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("expected manifest version 1"))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LocalizedText {
    Text(String),
    Translations(IndexMap<String, String>),
}

impl LocalizedText {
    /// Read the English entry of a translation map, if present.
    /// Readers decide how to handle missing English text.
    pub fn en(&self) -> Option<&str> {
        match self {
            Self::Text(_) => None,
            Self::Translations(locales) => locales.get("en").map(String::as_str),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginLocalizedMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<LocalizedText>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<LocalizedText>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DshEnginesManifest(pub IndexMap<String, String>);

impl DshEnginesManifest {
    pub fn dsh(&self) -> Option<&str> {
        self.0.get("dsh").map(String::as_str)
    }

    pub fn node(&self) -> Option<&str> {
        self.0.get("node").map(String::as_str)
    }

    pub fn npm(&self) -> Option<&str> {
        self.0.get("npm").map(String::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BundlePatch {
    Path(String),
    Paths(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshBundleManifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platforms: Option<Vec<String>>,
    pub patch: BundlePatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_by_default: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DshProfileManifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundles: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DshClientManifest {
    pub platform: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inject: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub immediately: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<Vec<String>>,
}
