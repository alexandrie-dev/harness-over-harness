//! Port of @deepseek-ai/dsh-credentials (packages/credentials/credentials).
//! Credential service seam: references never carry secrets in configuration.
pub mod types;
pub use types::{
    ApiKeyRecord, CredentialInfo, CredentialKey, CredentialRecord, CredentialRecordKind,
    CredentialRef, CredentialsRecordUpdated, CredentialsReferenceUpdated, GrantRecord,
};

use hoh_kernel::{Contained, Context, Event, ListenerFailure, Logger};
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

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCredential {
    pub value: String,
    pub source: String,
}

impl std::fmt::Debug for ResolvedCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedCredential")
            .field("value", &"[REDACTED]")
            .field("source", &self.source)
            .finish()
    }
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
    Pin<Box<dyn Future<Output = Result<T, hoh_error::BoxError>> + Send + 'a>>;
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

    /// Notify reference observers only after a committed write or reload.
    /// A failing listener never changes that operation's outcome. Notifications
    /// are unfiltered; ctx supplies the provider's diagnostic logger.
    fn notify_updated(&self, ctx: &Context, reference: &CredentialRef) {
        fan_out(
            ctx,
            CredentialsReferenceUpdated {
                reference: reference.clone(),
            },
            reference.as_str(),
        );
    }

    /// Notify record observers with the same containment and non-waiting
    /// semantics as reference notifications, after a committed change.
    fn notify_record_updated(&self, ctx: &Context, key: &CredentialKey) {
        fan_out(
            ctx,
            CredentialsRecordUpdated { key: key.clone() },
            key.as_str(),
        );
    }
}

fn fan_out<E: Event<Mode = Contained>>(ctx: &Context, event: E, subject: &str) {
    let subject = subject.to_owned();
    let logger = ctx.logger();
    // The source dispatch starts with the event name, so it has no receiver filter.
    ctx.root().dispatch(event).invoke(move |failure| {
        let error = match failure {
            ListenerFailure::Threw(error) | ListenerFailure::Rejected(error) => error,
        };
        warn_listener_failure(&logger, E::NAME, &subject, error.as_ref());
    });
}

fn warn_listener_failure(
    logger: &Logger,
    event: &str,
    subject: &str,
    error: &(dyn std::error::Error + 'static),
) {
    logger.warn(format_args!(
        "credentials: a {event} listener for \"{subject}\" failed"
    ));
    logger.warn(ErrorChain(error));
}

// The journal takes one formatted string per call (docs/kernel.md §7).
struct ErrorChain<'a>(&'a (dyn std::error::Error + 'static));

impl std::fmt::Display for ErrorChain<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)?;
        let mut cause = self.0.source();
        while let Some(error) = cause {
            write!(f, ": {error}")?;
            cause = error.source();
        }
        Ok(())
    }
}

hoh_kernel::service!(pub Credentials: dyn CredentialProvider = "credentials");
