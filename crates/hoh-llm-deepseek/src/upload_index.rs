//! Durable attachment-to-provider-file index.

use crate::file_id::{DeepSeekFileId, DeepSeekFileScope};
use hoh_atomic_write::{
    FileLockOptions, WriteFileAtomicOptions, with_file_lock, write_file_atomic,
};
use hoh_attachment::{AttachmentId, ImageVariantId};
use hoh_error::BoxError;
use hoh_timeout::AbortSignal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepSeekUploadRecord {
    pub scope: DeepSeekFileScope,
    pub attachment_id: AttachmentId,
    pub variant_id: ImageVariantId,
    pub file_id: DeepSeekFileId,
    pub bytes: u64,
    pub created_at: u64,
    pub expires_at: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadIndexCommit {
    pub record: DeepSeekUploadRecord,
    pub accepted: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredIndex {
    format_version: u8,
    records: Vec<DeepSeekUploadRecord>,
}

pub fn deep_seek_file_scope(base_url: &str, credentials: &str) -> DeepSeekFileScope {
    let mut hash = Sha256::new();
    hash.update(base_url.trim_end_matches('/'));
    hash.update([0]);
    hash.update(credentials);
    let digest = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    DeepSeekFileScope::new(digest).expect("SHA-256 is a lowercase 64-byte hex digest")
}

#[derive(Clone)]
pub struct DeepSeekUploadIndex {
    path: PathBuf,
}
impl DeepSeekUploadIndex {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn default_path() -> std::io::Result<Self> {
        Ok(Self::new(
            hoh_home_paths::resolve_dsh_home(None, None)?.join("llm-deepseek/files-v3.json"),
        ))
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    async fn load(&self, signal: Option<&AbortSignal>) -> std::io::Result<StoredIndex> {
        if let Some(signal) = signal {
            signal.throw_if_aborted().map_err(std::io::Error::other)?;
        }
        let text = match tokio::fs::read_to_string(&self.path).await {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(StoredIndex {
                    format_version: 3,
                    records: vec![],
                });
            }
            Err(error) => return Err(error),
        };
        let index: StoredIndex = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => {
                return Ok(StoredIndex {
                    format_version: 3,
                    records: vec![],
                });
            }
        };
        if index.format_version != 3 {
            return Ok(StoredIndex {
                format_version: 3,
                records: vec![],
            });
        }
        if !valid_records(&index.records) {
            return Ok(StoredIndex {
                format_version: 3,
                records: vec![],
            });
        }
        Ok(index)
    }
    async fn save(&self, index: &StoredIndex) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut data = serde_json::to_vec_pretty(index).map_err(std::io::Error::other)?;
        data.push(b'\n');
        write_file_atomic(
            &self.path,
            data,
            WriteFileAtomicOptions {
                mode: 0o600,
                dir_mode: Some(0o700),
            },
        )
        .await
    }
    pub async fn get(
        &self,
        scope: &DeepSeekFileScope,
        variant_id: &ImageVariantId,
        now: u64,
        refresh_margin_ms: u64,
        signal: Option<&AbortSignal>,
    ) -> std::io::Result<Option<DeepSeekUploadRecord>> {
        Ok(self.load(signal).await?.records.into_iter().find(|record| {
            &record.scope == scope
                && &record.variant_id == variant_id
                && record.expires_at.saturating_sub(now) > refresh_margin_ms
        }))
    }
    pub async fn commit(
        &self,
        candidate: DeepSeekUploadRecord,
        now: u64,
        refresh_margin_ms: u64,
    ) -> std::io::Result<UploadIndexCommit> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        with_file_lock(
            &self.path,
            || async {
                let mut index = self.load(None).await?;
                if let Some(existing) = index
                    .records
                    .iter()
                    .find(|record| {
                        record.scope == candidate.scope
                            && record.variant_id == candidate.variant_id
                            && record.expires_at.saturating_sub(now) > refresh_margin_ms
                    })
                    .cloned()
                {
                    return Ok(UploadIndexCommit {
                        record: existing,
                        accepted: false,
                    });
                }
                index.records.retain(|record| {
                    record.expires_at.saturating_sub(now) > refresh_margin_ms
                        && !(record.scope == candidate.scope
                            && record.variant_id == candidate.variant_id)
                });
                index.records.push(candidate.clone());
                self.save(&index).await?;
                Ok(UploadIndexCommit {
                    record: candidate,
                    accepted: true,
                })
            },
            FileLockOptions::default(),
        )
        .await
        .map_err(lock_error)
    }
    pub async fn remove(
        &self,
        scope: &DeepSeekFileScope,
        generations: &[(ImageVariantId, DeepSeekFileId)],
    ) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        with_file_lock(
            &self.path,
            || async {
                let mut index = self.load(None).await?;
                let before = index.records.len();
                index.records.retain(|record| {
                    &record.scope != scope
                        || !generations.iter().any(|(variant, file)| {
                            variant == &record.variant_id && file == &record.file_id
                        })
                });
                if index.records.len() != before {
                    self.save(&index).await?;
                }
                Ok(())
            },
            FileLockOptions::default(),
        )
        .await
        .map_err(lock_error)
    }
    pub async fn clear(&self, scope: &DeepSeekFileScope) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        with_file_lock(
            &self.path,
            || async {
                let mut index = self.load(None).await?;
                let before = index.records.len();
                index.records.retain(|record| &record.scope != scope);
                if index.records.len() != before {
                    self.save(&index).await?;
                }
                Ok(())
            },
            FileLockOptions::default(),
        )
        .await
        .map_err(lock_error)
    }
}

fn lock_error(error: BoxError) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

fn valid_records(records: &[DeepSeekUploadRecord]) -> bool {
    const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
    let digest = |value: &str| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    };
    let attachment = |value: &str| value.strip_prefix("sha256:").is_some_and(digest);
    let mut keys = std::collections::HashSet::new();
    records.iter().all(|record| {
        digest(record.scope.as_str())
            && attachment(record.attachment_id.as_str())
            && attachment(record.variant_id.as_str())
            && !record.file_id.as_str().is_empty()
            && [record.bytes, record.created_at, record.expires_at]
                .into_iter()
                .all(|value| value <= MAX_SAFE_INTEGER)
            && keys.insert((record.scope.clone(), record.variant_id.clone()))
    })
}
