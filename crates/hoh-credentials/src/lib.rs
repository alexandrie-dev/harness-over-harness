//! Port of @deepseek-ai/dsh-credentials (packages/credentials/credentials).
//! Credential service seam: references never carry secrets in configuration.
pub mod types;
pub use types::{
    ApiKeyRecord, CredentialInfo, CredentialKey, CredentialRecord, CredentialRecordKind,
    CredentialRef, CredentialsRecordUpdated, CredentialsReferenceUpdated, GrantRecord,
};

use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};

/// Native TypeError equivalents, retaining the exact source messages.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TypeError {
    #[error("credential ref \"{0}\" must match /^[A-Za-z_][A-Za-z0-9_]*$/")]
    Reference(String),
    #[error("credential key segment \"{0}\" must match /^[a-z][a-z0-9-]*$/")]
    KeySegment(String),
    #[error("credential key \"{0}\" must be \"<scope>/<id>\"")]
    Key(String),
}

impl TypeError {
    pub fn name(&self) -> &'static str {
        "TypeError"
    }
}

pub fn is_credential_ref_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

pub fn credential_ref(value: &str) -> Result<CredentialRef, TypeError> {
    if !is_credential_ref_name(value) {
        return Err(TypeError::Reference(value.into()));
    }
    Ok(hoh_brand::brand_string(value))
}

pub fn is_credential_key_segment(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub fn credential_key(scope: &str, id: &str) -> Result<CredentialKey, TypeError> {
    for segment in [scope, id] {
        if !is_credential_key_segment(segment) {
            return Err(TypeError::KeySegment(segment.into()));
        }
    }
    Ok(hoh_brand::brand_string(format!("{scope}/{id}")))
}

pub fn parse_credential_key(value: &str) -> Result<CredentialKey, TypeError> {
    let mut segments = value.split('/');
    let scope = segments.next();
    let id = segments.next();
    match (scope, id, segments.next()) {
        (Some(scope), Some(id), None) => credential_key(scope, id),
        _ => Err(TypeError::Key(value.into())),
    }
}

pub fn credential_key_scope(key: &CredentialKey) -> &str {
    let value = key.as_str();
    // Validated keys always have a slash; the fallback follows slice(0, -1)
    // for unchecked ASCII brands constructed through the low-level brand API.
    let end = value.find('/').unwrap_or_else(|| {
        value
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    });
    &value[..end]
}

pub fn credential_key_id(key: &CredentialKey) -> &str {
    let value = key.as_str();
    &value[value.find('/').map_or(0, |index| index + 1)..]
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCredential {
    pub value: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialRecordInfo {
    pub configured: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<CredentialRecordKind>,
    pub writable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialRecordEntry {
    pub key: CredentialKey,
    pub kind: CredentialRecordKind,
}

/// Object-safe asynchronous result; providers may fail with arbitrary errors.
pub type CredentialFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, hoh_kernel::Error>> + Send + 'a>>;
pub type CredentialMutator = Box<
    dyn FnOnce(Option<CredentialRecord>) -> CredentialFuture<'static, Option<CredentialRecord>>
        + Send,
>;

/// Abstract seam; a provider plugin publishes its implementation as Credentials.
/// Empty reference values are absent in both resolve/describe; set rejects an
/// empty value. Resolution happens per call, without a cross-operation cache.
/// Records have no fallback layers and their presence alone means configured.
pub trait CredentialProvider: Send + Sync {
    fn resolve<'a>(
        &'a self,
        reference: &'a CredentialRef,
    ) -> CredentialFuture<'a, Option<ResolvedCredential>>;
    fn describe<'a>(&'a self, reference: &'a CredentialRef)
    -> CredentialFuture<'a, CredentialInfo>;
    fn set<'a>(&'a self, reference: &'a CredentialRef, value: &'a str) -> CredentialFuture<'a, ()>;
    /// Missing reference removal is silent and may expose ambient fallbacks.
    fn unset<'a>(&'a self, reference: &'a CredentialRef) -> CredentialFuture<'a, ()>;
    fn read_record<'a>(
        &'a self,
        key: &'a CredentialKey,
    ) -> CredentialFuture<'a, Option<CredentialRecord>>;
    fn describe_record<'a>(
        &'a self,
        key: &'a CredentialKey,
    ) -> CredentialFuture<'a, CredentialRecordInfo>;
    /// Enumeration contains addresses and kinds only, never values.
    fn list_records(&self) -> CredentialFuture<'_, Vec<CredentialRecordEntry>>;
    /// Serialized exclusive read/decide/replace; None leaves the current entry
    /// untouched and returns it. Exclusion spans processes where supported.
    /// Only a committed replacement emits CredentialsRecordUpdated.
    fn modify_record<'a>(
        &'a self,
        key: &'a CredentialKey,
        mutate: CredentialMutator,
    ) -> CredentialFuture<'a, Option<CredentialRecord>>;
    /// Missing record removal is silent; a committed removal emits an event.
    fn delete_record<'a>(&'a self, key: &'a CredentialKey) -> CredentialFuture<'a, ()>;
}

hoh_kernel::service!(pub Credentials: dyn CredentialProvider = "credentials");
