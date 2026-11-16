//! Backend contracts. Backends own media; values are opaque JSON to this layer.
use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, LazyLock},
};

/// Safe unit/table names, also usable as SQL identifier segments without escaping.
pub static UNIT_NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    // The source's constant expression is a valid Rust regex.
    Regex::new(r"^[a-z][a-z0-9_]*$").expect("valid unit name expression")
});

/// Object-safe asynchronous backend result, including arbitrary I/O failures.
pub type BackendFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, hoh_error::BoxError>> + Send + 'a>>;

/// One medium, shared across all its optional facets. Closing drains in-flight
/// writes and releases the medium; repeated/concurrent closes await teardown.
pub trait StorageBackend: Send + Sync {
    /// Absent when this medium cannot serve key-value data.
    fn kv(&self) -> Option<&dyn KvFacet> {
        None
    }
    fn close(&self) -> BackendFuture<'_, ()>;
}

pub trait KvFacet: Send + Sync {
    /// Open an empty or existing unit. Duplicate opens are caller errors.
    /// Unaccepted whole-unit stamps reject `version-mismatch`, malformed
    /// media reject `malformed-medium`; per-record unaccepted documents read
    /// as absent. Reads of accepted older versions never restamp them.
    fn open(&self, descriptor: KvUnitDescriptor) -> BackendFuture<'_, Arc<dyn KvUnit>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KvLayout {
    Single,
    PerRecord,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KvUnitDescriptor {
    pub name: String,
    /// Non-negative current format version stamped with each mutation.
    pub version: u64,
    pub tables: Vec<String>,
    pub has_global: bool,
    /// Omission requests the single-document layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<KvLayout>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatible_versions: Option<Vec<u64>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KvSnapshot {
    pub tables: IndexMap<String, IndexMap<String, Value>>,
    /// Null when never written or not declared.
    pub global: Value,
}

/// An opened unit. Callers serialize writes. Resolved mutations are durable;
/// whole-document/row-store calls are atomic; per-record batches may be
/// partially applied on interruption. Every operation after close rejects
/// `closed`. Close drains writes and is idempotent.
pub trait KvUnit: Send + Sync {
    fn load_all(&self) -> BackendFuture<'_, KvSnapshot>;
    /// Replace one opaque value. Single-layout keys are arbitrary strings;
    /// per-record keys must match `[a-zA-Z0-9_-]+`.
    fn put_record<'a>(
        &'a self,
        table: &'a str,
        key: &'a str,
        value: Value,
    ) -> BackendFuture<'a, ()>;
    /// A missing key is a no-op and does not advance the version stamp.
    fn delete_record<'a>(&'a self, table: &'a str, key: &'a str) -> BackendFuture<'a, ()>;
    /// Empty/missing/duplicate keys are harmless. Validate every per-record
    /// key before deleting any. Whole-medium batches use one medium operation.
    /// Rejection leaves the batch unapplied unless it reports `unknown-state`:
    /// then every subsequent call rejects that error until close. Directory
    /// fsync failure may follow atomic replacement; reopen before retrying.
    fn delete_records<'a>(&'a self, table: &'a str, keys: &'a [String]) -> BackendFuture<'a, ()>;
    /// Optional document backup, absent for single-document/row-store media.
    /// Moves the document out of the readable set, preserving its bytes.
    fn backup_record<'a>(
        &'a self,
        _table: &'a str,
        _key: &'a str,
    ) -> Option<BackendFuture<'a, String>> {
        None
    }
    /// Only valid when the descriptor declared hasGlobal.
    fn set_global(&self, value: Value) -> BackendFuture<'_, ()>;
    fn close(&self) -> BackendFuture<'_, ()>;
}
