//! Client-safe credential brands, stored records, safe views and event payloads.
use hoh_kernel::Emit;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

hoh_brand::branded_string!(pub CredentialRef);
hoh_brand::branded_string!(pub CredentialKey);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename = "api-key")]
pub struct ApiKeyRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<IndexMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename = "grant")]
pub struct GrantRecord {
    /// Opaque owner-defined JSON; null is distinct from a missing record.
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CredentialRecord {
    ApiKey(ApiKeyRecord),
    Grant(GrantRecord),
}

impl CredentialRecord {
    pub fn kind(&self) -> CredentialRecordKind {
        match self {
            Self::ApiKey(_) => CredentialRecordKind::ApiKey,
            Self::Grant(_) => CredentialRecordKind::Grant,
        }
    }
}

impl Serialize for CredentialRecord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::ApiKey(record) => record.serialize(serializer),
            Self::Grant(record) => record.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for CredentialRecord {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "kind")]
        enum Tagged {
            #[serde(rename = "api-key")]
            ApiKey(ApiKeyRecord),
            #[serde(rename = "grant")]
            Grant(GrantRecord),
        }
        Ok(match Tagged::deserialize(deserializer)? {
            Tagged::ApiKey(record) => Self::ApiKey(record),
            Tagged::Grant(record) => Self::Grant(record),
        })
    }
}

/// The source's CredentialRecord['kind'] type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CredentialRecordKind {
    ApiKey,
    Grant,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialInfo {
    pub configured: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub writable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub masked: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialsReferenceUpdated {
    pub reference: CredentialRef,
}
hoh_kernel::event!(
    CredentialsReferenceUpdated = "credentials/reference-updated",
    Emit
);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialsRecordUpdated {
    pub key: CredentialKey,
}
hoh_kernel::event!(
    CredentialsRecordUpdated = "credentials/record-updated",
    Emit
);
