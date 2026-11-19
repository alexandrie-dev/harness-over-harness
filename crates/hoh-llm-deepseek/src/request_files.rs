//! Request-local Files resolution, stale-id recovery, and normalized-image diagnostics.

use crate::{DeepSeekFileConnection, DeepSeekFileId, DeepSeekFilePolicy, DeepSeekFileStore};
use hoh_attachment::RequestImageAttachment;
use hoh_error::{HarnessError, SharedError};
use hoh_llm::{BuiltinLlmFailureCode as Code, LlmError, LlmErrorOptions};
use hoh_timeout::{AbortSignal, deadline};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageWireLocation {
    pub message: usize,
    pub image: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum RequestFileError {
    #[error("DeepSeek Files API could not resolve a request image.")]
    Resolution(#[source] HarnessError),
    #[error(transparent)]
    Fatal(#[from] HarnessError),
}

impl RequestFileError {
    pub(crate) fn missing_image() -> Self {
        Self::Fatal(
            LlmError::new(
                "DeepSeek Messages request image is missing",
                Code::InvalidRequest,
            )
            .expect("image diagnostic is valid"),
        )
    }

    pub fn into_fatal(self) -> Option<HarnessError> {
        match self {
            Self::Fatal(error) => Some(error),
            Self::Resolution(_) => None,
        }
    }
}

#[derive(Clone)]
struct UsedRequestFile {
    version: RequestImageAttachment,
    file_id: DeepSeekFileId,
    location: ImageWireLocation,
}

pub struct RequestFiles<'a> {
    store: &'a DeepSeekFileStore,
    connection: &'a DeepSeekFileConnection,
    policy: &'a DeepSeekFilePolicy,
    timeout_ms: f64,
    signal: Option<&'a AbortSignal>,
    activity: Arc<dyn Fn() + Send + Sync>,
    used: Vec<UsedRequestFile>,
    retried: bool,
}

impl<'a> RequestFiles<'a> {
    pub fn new(
        store: &'a DeepSeekFileStore,
        connection: &'a DeepSeekFileConnection,
        policy: &'a DeepSeekFilePolicy,
        timeout_ms: f64,
        signal: Option<&'a AbortSignal>,
        activity: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            store,
            connection,
            policy,
            timeout_ms,
            signal,
            activity,
            used: Vec::new(),
            retried: false,
        }
    }

    pub fn begin_attempt(&mut self) {
        self.used.clear();
    }

    pub async fn resolve(
        &mut self,
        version: &RequestImageAttachment,
        location: ImageWireLocation,
    ) -> Result<DeepSeekFileId, RequestFileError> {
        let limit =
            deadline(self.signal, self.timeout_ms, crate::FILES_API_TIMEOUT).map_err(|error| {
                RequestFileError::Fatal(
                    LlmError::with_options(
                        "DeepSeek Files API timeout is invalid",
                        Code::InvalidConfig,
                        LlmErrorOptions {
                            cause: Some(SharedError::new(error)),
                            ..Default::default()
                        },
                    )
                    .expect("timeout diagnostic is valid"),
                )
            })?;
        let resolved = match self
            .store
            .ensure_uploaded(version, self.connection, self.policy, Some(&limit.signal))
            .await
        {
            Ok(resolved) => resolved,
            Err(error) if self.signal.is_some_and(AbortSignal::aborted) => {
                return Err(RequestFileError::Fatal(error));
            }
            Err(error) => return Err(RequestFileError::Resolution(error)),
        };
        (self.activity)();
        self.used.push(UsedRequestFile {
            version: version.clone(),
            file_id: resolved.record.file_id.clone(),
            location,
        });
        Ok(resolved.record.file_id)
    }

    pub async fn retry(&mut self, detail: &str) -> Result<bool, HarnessError> {
        if self.used.is_empty() || !provider_rejected_file_id(detail) {
            return Ok(false);
        }
        let mappings = stale_mappings(&self.used, detail)
            .into_iter()
            .map(|file| (file.version.variant_id.clone(), file.file_id.clone()))
            .collect::<Vec<_>>();
        self.store.invalidate(&mappings, self.connection).await?;
        if self.retried {
            return Ok(false);
        }
        self.retried = true;
        Ok(true)
    }

    pub fn error_message(&self, status: u16, message: &str, detail: &str) -> String {
        if status == 400 && !self.used.is_empty() && provider_rejected_normalized_image(detail) {
            normalized_image_diagnostic(&self.used, message, detail)
        } else {
            message.to_owned()
        }
    }
}

fn provider_rejected_normalized_image(detail: &str) -> bool {
    let detail = detail.to_lowercase();
    [
        "unsupported",
        "invalid",
        "cannot read",
        "failed to decode",
        "failed to process",
    ]
    .iter()
    .any(|reason| within_characters_ordered(&detail, reason, "image", 40))
        || ["unsupported", "invalid", "cannot be decoded"]
            .iter()
            .any(|reason| within_characters_ordered(&detail, "image", reason, 40))
}

fn provider_rejected_file_id(detail: &str) -> bool {
    let detail = detail.to_lowercase();
    let file = detail.match_indices("file").any(|(index, _)| {
        detail[..index].chars().next_back().is_none_or(|character| {
            !character.is_ascii_alphanumeric() && character != '_' && character != '\u{017f}'
        })
    });
    let missing = [
        "expired",
        "deleted",
        "does not exist",
        "do not exist",
        "not created under this account",
        "not created under your account",
    ]
    .iter()
    .any(|needle| detail.contains(needle))
        || ["notfound", "not_found", "not-found", "not found"]
            .iter()
            .any(|needle| detail.contains(needle));
    let invalid = [
        "fileid", "file_id", "file-id", "file id", "fileapi", "file_api", "file-api", "file api",
    ]
    .iter()
    .any(|file_kind| within_characters(&detail, "invalid", file_kind, 20));
    file && (missing || invalid)
}

fn within_characters(text: &str, left: &str, right: &str, maximum: usize) -> bool {
    within_characters_ordered(text, left, right, maximum)
        || within_characters_ordered(text, right, left, maximum)
}

fn within_characters_ordered(text: &str, first: &str, second: &str, maximum: usize) -> bool {
    text.match_indices(first).any(|(start, _)| {
        let after = &text[start + first.len()..];
        after.match_indices(second).any(|(end, _)| {
            let between = &after[..end];
            between.chars().count() <= maximum
                && !between
                    .chars()
                    .any(|ch| matches!(ch, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
        })
    })
}

fn detail_names_file_id(detail: &str, file_id: &DeepSeekFileId) -> bool {
    let value = file_id.as_str();
    detail.match_indices(value).any(|(index, _)| {
        let before = detail[..index].chars().next_back();
        let after = detail[index + value.len()..].chars().next();
        let boundary = |character: Option<char>| {
            character.is_none_or(|character| {
                !character.is_alphanumeric() && character != '_' && character != '-'
            })
        };
        boundary(before) && boundary(after)
    })
}

fn stale_mappings<'a>(files: &'a [UsedRequestFile], detail: &str) -> Vec<&'a UsedRequestFile> {
    let mut unique = Vec::new();
    for file in files {
        if !unique.iter().any(|candidate: &&UsedRequestFile| {
            candidate.version.variant_id == file.version.variant_id
                && candidate.file_id == file.file_id
        }) {
            unique.push(file);
        }
    }
    let exact = unique
        .iter()
        .copied()
        .filter(|file| detail_names_file_id(detail, &file.file_id))
        .collect::<Vec<_>>();
    if exact.is_empty() { unique } else { exact }
}

fn normalized_image_facts(file: &UsedRequestFile) -> String {
    let version = &file.version;
    let name = version
        .attachment
        .name
        .as_deref()
        .unwrap_or_else(|| version.attachment.attachment_id.as_str());
    let colour = if version.has_alpha { "sRGBA" } else { "sRGB" };
    format!(
        "\"{name}\" at message {}, image {} ({}, 8-bit {colour}, {}x{})",
        file.location.message,
        file.location.image,
        version.media_type,
        version.width,
        version.height
    )
}

fn normalized_image_diagnostic(
    files: &[UsedRequestFile],
    provider_message: &str,
    provider_detail: &str,
) -> String {
    let exact = files
        .iter()
        .find(|file| detail_names_file_id(provider_detail, &file.file_id));
    let target = exact.or_else(|| (files.len() == 1).then(|| &files[0]));
    if let Some(target) = target {
        return format!(
            "DeepSeek rejected normalized image {}: {provider_message}. The provider rejected bytes already normalized by the harness; PNG, JPEG, WebP, and GIF remain supported input formats.",
            normalized_image_facts(target)
        );
    }
    let mut candidates = Vec::new();
    for file in files {
        if !candidates.iter().any(|candidate: &&UsedRequestFile| {
            candidate.version.variant_id == file.version.variant_id
                && candidate.location == file.location
        }) {
            candidates.push(file);
        }
    }
    format!(
        "DeepSeek rejected a normalized request image: {provider_message}. Candidate images: {}. The provider rejected bytes already normalized by the harness; PNG, JPEG, WebP, and GIF remain supported input formats.",
        candidates
            .into_iter()
            .map(normalized_image_facts)
            .collect::<Vec<_>>()
            .join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages_api::messages_api_root;
    use crate::upload_index::{DeepSeekUploadIndex, DeepSeekUploadRecord, deep_seek_file_scope};
    use hoh_attachment::{
        ImageAttachmentRef, ImageMediaType, RequestImageDepth, RequestImageSpace,
    };
    use indexmap::IndexMap;

    fn version() -> RequestImageAttachment {
        let attachment = ImageAttachmentRef {
            attachment_id:
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            media_type: ImageMediaType::Png,
            bytes: 4,
            width: 1,
            height: 1,
            name: Some("image.png".into()),
            original_dimensions: None,
        };
        RequestImageAttachment {
            variant_id: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .into(),
            attachment,
            data: vec![0; 4],
            media_type: ImageMediaType::Png,
            bytes: 4,
            width: 1,
            height: 1,
            depth: RequestImageDepth::Uchar,
            space: RequestImageSpace::Srgb,
            has_alpha: true,
        }
    }

    #[test]
    fn file_id_matching_requires_token_boundaries() {
        let id = DeepSeekFileId::new("file-123").unwrap();
        assert!(detail_names_file_id("expired file-123.", &id));
        assert!(!detail_names_file_id("expired xfile-123", &id));
        assert!(!detail_names_file_id("expired file-123_more", &id));
    }

    #[test]
    fn provider_rejection_classifiers_cover_source_spellings() {
        assert!(provider_rejected_file_id("file_id not_found"));
        assert!(provider_rejected_file_id("Invalid file API value"));
        assert!(provider_rejected_file_id("invalid value for file id"));
        assert!(!provider_rejected_file_id("invalid request"));
        assert!(provider_rejected_normalized_image("failed to decode image"));
        assert!(provider_rejected_normalized_image(
            "image cannot be decoded"
        ));
    }

    #[test]
    fn provider_image_rejection_matches_source_order_distance_and_line_boundaries() {
        for (detail, expected) in [
            ("image failed to decode".to_owned(), false),
            ("image cannot read".to_owned(), false),
            ("cannot be decoded image".to_owned(), false),
            (format!("unsupported{}image", "x".repeat(41)), false),
            (format!("image{}invalid", "x".repeat(41)), false),
            ("unsupported\nimage".to_owned(), false),
            ("invalid image".to_owned(), true),
            ("failed to decode image".to_owned(), true),
            ("image unsupported".to_owned(), true),
            ("image cannot be decoded".to_owned(), true),
        ] {
            assert_eq!(
                provider_rejected_normalized_image(&detail),
                expected,
                "unexpected normalized-image classification for {detail:?}"
            );
        }
    }

    #[test]
    fn provider_file_rejection_matches_source_boundaries_and_optional_separators() {
        for (detail, expected) in [
            ("profile expired", false),
            ("my_file expired", false),
            ("filename expired", true),
            ("invalid fileid", true),
            ("notfound file", true),
        ] {
            assert_eq!(
                provider_rejected_file_id(detail),
                expected,
                "unexpected stale-file classification for {detail:?}"
            );
        }
    }

    #[tokio::test]
    async fn invalid_value_for_file_id_invalidates_once_and_requests_one_retry() {
        let directory = tempfile::tempdir().unwrap();
        let index = DeepSeekUploadIndex::new(directory.path().join("files-v3.json"));
        let store = DeepSeekFileStore::new(index.clone());
        let connection = DeepSeekFileConnection {
            base_url: "https://example.test/v1".into(),
            headers: IndexMap::new(),
        };
        let root = messages_api_root(&connection.base_url).unwrap();
        let scope = deep_seek_file_scope(&root, "[]");
        let version = version();
        let file_id = DeepSeekFileId::new("file-stale").unwrap();
        index
            .commit(
                DeepSeekUploadRecord {
                    scope: scope.clone(),
                    attachment_id: version.attachment.attachment_id.clone(),
                    variant_id: version.variant_id.clone(),
                    file_id: file_id.clone(),
                    bytes: version.bytes,
                    created_at: 1,
                    expires_at: 10_000,
                },
                0,
                0,
            )
            .await
            .unwrap();
        let policy = DeepSeekFilePolicy {
            expires_after_seconds: 3_600,
            refresh_margin_seconds: 60,
            quota_cleanup_batch: 1,
        };
        let mut files =
            RequestFiles::new(&store, &connection, &policy, 1_000.0, None, Arc::new(|| {}));
        files.used.push(UsedRequestFile {
            version: version.clone(),
            file_id: file_id.clone(),
            location: ImageWireLocation {
                message: 0,
                image: 0,
            },
        });

        assert!(files.retry("invalid value for file id").await.unwrap());
        assert!(
            index
                .get(&scope, &version.variant_id, 0, 0, None)
                .await
                .unwrap()
                .is_none()
        );
        assert!(!files.retry("invalid value for file id").await.unwrap());
    }
}
