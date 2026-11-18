//! Staged immutable publication, hard-link deduplication, and verified image reads.
use crate::{
    image::{DecodedImageLimits, attachment_failure, detect_image, probe_image},
    normalization::{NormalizationPolicy, normalize_image},
    util::*,
};
use futures_util::{StreamExt, stream::BoxStream};
use hoh_attachment::{
    AttachmentErrorCode as Code, ImageAttachmentLimits, ImageAttachmentRef, ProjectedDimensions,
    SaveImageAttachment, StoredImageAttachment,
};
use hoh_error::{BoxError, HarnessError};
use hoh_timeout::AbortSignal;
use hoh_util_values::js_trim;
use hoh_util_workspace_path::node_path::{dirname, join};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedImageFile {
    pub data: Vec<u8>,
    pub r#ref: ImageAttachmentRef,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamedImmutableObject {
    pub sha256: String,
    pub bytes: u64,
}
struct StagedImmutableObject {
    path: String,
    boundary: String,
    object: StreamedImmutableObject,
}

pub(crate) fn reference_digest(id: &str, message: &str) -> Result<String, HarnessError> {
    let valid = id.strip_prefix("sha256:").filter(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    });
    valid
        .map(str::to_owned)
        .ok_or_else(|| error(message, Code::InvalidAttachmentRef))
}
pub fn normalized_image_path(
    root: &str,
    reference: &ImageAttachmentRef,
) -> Result<String, HarnessError> {
    let hash = reference_digest(
        reference.attachment_id.as_str(),
        "Attachment reference is invalid.",
    )?;
    Ok(join(&[root, "objects", &hash[..2], &hash]))
}
fn display_name(value: Option<&str>) -> Option<String> {
    let leaf = value?.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = leaf
        .chars()
        .filter(|c| !matches!(*c, '\u{0000}'..='\u{001f}' | '\u{007f}'))
        .collect();
    let mut units = 0;
    let name: String = js_trim(&clean)
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= 255
        })
        .collect();
    if name.is_empty() { None } else { Some(name) }
}
pub async fn prepare_image_file(
    input: &SaveImageAttachment,
    limits: &ImageAttachmentLimits,
    policy: &NormalizationPolicy,
) -> Result<PreparedImageFile, BoxError> {
    if input.data.len() as u64 > limits.max_image_bytes {
        return Err(error(
            "Image exceeds the configured byte limit.",
            Code::ImageTooLarge,
        )
        .into());
    }
    if input.data.is_empty() {
        return Err(error("Image is empty.", Code::InvalidImage).into());
    }
    let detected = detect_image(
        &input.data,
        Some(DecodedImageLimits {
            max_pixels: Some(limits.max_image_pixels),
            max_dimension: Some(limits.max_image_dimension),
        }),
    )
    .await?;
    if detected.media_type != input.media_type {
        return Err(error(
            "Declared image type does not match its bytes.",
            Code::ImageTypeMismatch,
        )
        .into());
    }
    let normalized = normalize_image(&input.data, &detected, policy).await?;
    let data = normalized.data;
    let hash_data = data.clone();
    let hash = tokio::task::spawn_blocking(move || digest(&hash_data)).await?;
    let reference = ImageAttachmentRef {
        attachment_id: format!("sha256:{hash}").into(),
        media_type: normalized.media_type,
        bytes: data.len() as u64,
        width: normalized.width,
        height: normalized.height,
        name: display_name(input.name.as_deref()),
        original_dimensions: if detected.width != normalized.width
            || detected.height != normalized.height
        {
            Some(ProjectedDimensions {
                width: detected.width,
                height: detected.height,
            })
        } else {
            None
        },
    };
    Ok(PreparedImageFile {
        data,
        r#ref: reference,
    })
}
pub async fn validate_image_file(
    input: &SaveImageAttachment,
    limits: &ImageAttachmentLimits,
    policy: &NormalizationPolicy,
) -> Result<(), BoxError> {
    prepare_image_file(input, limits, policy).await?;
    Ok(())
}

#[cfg(test)]
tokio::task_local! { pub(crate) static SYNC_TRACE: std::sync::Arc<std::sync::Mutex<Vec<String>>>; }
#[cfg(test)]
tokio::task_local! { pub(crate) static READ_SIGNALS: std::sync::Arc<std::sync::Mutex<Vec<AbortSignal>>>; }
async fn sync_directory(path: &str) -> Result<(), BoxError> {
    #[cfg(not(windows))]
    {
        #[cfg(test)]
        let _ = SYNC_TRACE.try_with(|trace| lock(trace).push(path.to_owned()));
        let handle = tokio::fs::File::open(path)
            .await
            .map_err(|e| fs_error(e, "open", path, None))?;
        handle
            .sync_all()
            .await
            .map_err(|e| fs_error(e, "fsync", path, None))?;
    }
    Ok(())
}
pub(crate) async fn create_private_directory(path: &str) -> Result<(), BoxError> {
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder
        .create(path)
        .await
        .map_err(|e| fs_error(e, "mkdir", path, None))
}
pub(crate) async fn private_directory(path: &str) -> Result<(), BoxError> {
    create_private_directory(path).await?;
    permissions(path, 0o700).await
}
pub(crate) async fn permissions(path: &str, mode: u32) -> Result<(), BoxError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .await
            .map_err(|e| fs_error(e, "chmod", path, None))?;
    }
    #[cfg(windows)]
    {
        let mut permissions = tokio::fs::metadata(path).await?.permissions();
        permissions.set_readonly(mode & 0o200 == 0);
        tokio::fs::set_permissions(path, permissions)
            .await
            .map_err(|e| fs_error(e, "chmod", path, None))?;
    }
    Ok(())
}
async fn ensure_durable_directory(path: &str, boundary: &str) -> Result<(), BoxError> {
    let mut level = absolute(path)?;
    let stop = absolute(boundary)?;
    private_directory(&level).await?;
    while level != stop {
        let parent = dirname(&level);
        sync_directory(&parent).await?;
        if parent == level {
            break;
        }
        level = parent;
    }
    Ok(())
}
async fn ensure_home(root: &str) -> Result<String, BoxError> {
    let home = dirname(&dirname(&absolute(root)?));
    // The free-function API has no process-global durability cache: prove every ancestor.
    ensure_durable_directory(&home, "/").await?;
    Ok(home)
}
async fn remove_temporary(path: &str) -> Result<(), BoxError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(fs_error(error, "unlink", path, None)),
    }
}
async fn stage(
    root: &str,
    mut data: BoxStream<'_, Result<Vec<u8>, BoxError>>,
    signal: Option<&AbortSignal>,
) -> Result<StagedImmutableObject, BoxError> {
    let staging = join(&[root, "tmp"]);
    let boundary = ensure_home(root).await?;
    ensure_durable_directory(&staging, &boundary).await?;
    let temporary = join(&[&staging, &uuid()?]);
    let result: Result<StreamedImmutableObject, BoxError> = async {
        let mut options = tokio::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut handle = options
            .open(&temporary)
            .await
            .map_err(|e| fs_error(e, "open", &temporary, None))?;
        let mut hash = Sha256::new();
        let mut bytes = 0;
        while let Some(chunk) = data.next().await {
            let chunk = chunk?;
            check(signal)?;
            handle
                .write_all(&chunk)
                .await
                .map_err(|e| fs_error(e, "write", &temporary, None))?;
            hash.update(&chunk);
            bytes += chunk.len() as u64;
        }
        check(signal)?;
        // Tokio sync_all discards pending write errors; flush must observe them.
        handle
            .flush()
            .await
            .map_err(|e| fs_error(e, "write", &temporary, None))?;
        handle
            .sync_all()
            .await
            .map_err(|e| fs_error(e, "fsync", &temporary, None))?;
        check(signal)?;
        drop(handle);
        Ok(StreamedImmutableObject {
            sha256: hex(&hash.finalize()),
            bytes,
        })
    }
    .await;
    match result {
        Ok(object) => Ok(StagedImmutableObject {
            path: temporary,
            boundary,
            object,
        }),
        Err(cause) => {
            remove_temporary(&temporary).await?;
            if attachment_failure(cause.as_ref()) || signal.is_some_and(AbortSignal::aborted) {
                Err(cause)
            } else {
                Err(
                    error("Unable to persist attachment.", Code::AttachmentWriteFailed)
                        .with_cause(cause)
                        .into(),
                )
            }
        }
    }
}
async fn digest_file(path: &str) -> Result<String, BoxError> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| fs_error(e, "open", path, None))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 65536];
    loop {
        let length = file
            .read(&mut buffer)
            .await
            .map_err(|e| fs_error(e, "read", path, None))?;
        if length == 0 {
            break;
        }
        hash.update(&buffer[..length]);
    }
    Ok(hex(&hash.finalize()))
}
async fn link_verified(source: &str, target: &str, hash: &str) -> Result<(), BoxError> {
    match tokio::fs::hard_link(source, target).await {
        Ok(()) => Ok(()),
        Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => {
            if digest_file(target).await? != hash {
                return Err(error(
                    "Stored attachment failed integrity verification.",
                    Code::AttachmentCorrupt,
                )
                .into());
            }
            Ok(())
        }
        Err(cause) => Err(fs_error(cause, "link", source, Some(target))),
    }
}
async fn sync_publication(root: &str, parent: &str) -> Result<(), BoxError> {
    let stop = absolute(root)?;
    let mut level = parent.to_owned();
    while level != stop {
        sync_directory(&level).await?;
        let next = dirname(&level);
        if next == level {
            break;
        }
        level = next;
    }
    Ok(())
}
async fn publish_staged(
    root: &str,
    target: &str,
    staged: StagedImmutableObject,
) -> Result<(), BoxError> {
    let result: Result<(), BoxError> = async {
        let parent = dirname(target);
        ensure_durable_directory(&parent, &staged.boundary).await?;
        link_verified(&staged.path, target, &staged.object.sha256).await?;
        tokio::fs::remove_file(&staged.path)
            .await
            .map_err(|e| fs_error(e, "unlink", &staged.path, None))?;
        permissions(target, 0o400).await?;
        sync_publication(root, &parent).await
    }
    .await;
    if let Err(cause) = result {
        remove_temporary(&staged.path).await?;
        if attachment_failure(cause.as_ref()) {
            return Err(cause);
        }
        return Err(
            error("Unable to persist attachment.", Code::AttachmentWriteFailed)
                .with_cause(cause)
                .into(),
        );
    }
    Ok(())
}
pub async fn publish_immutable_object(
    root: &str,
    target: &str,
    data: &[u8],
    sha256: &str,
) -> Result<(), BoxError> {
    let staged = stage(
        root,
        Box::pin(futures_util::stream::once(async { Ok(data.to_vec()) })),
        None,
    )
    .await?;
    if staged.object.sha256 != sha256 {
        remove_temporary(&staged.path).await?;
        return Err(error(
            "Attachment bytes do not match their publication digest.",
            Code::AttachmentCorrupt,
        )
        .into());
    }
    publish_staged(root, target, staged).await
}
pub async fn publish_immutable_object_stream(
    root: &str,
    data: BoxStream<'_, Result<Vec<u8>, BoxError>>,
    target_for: impl FnOnce(&str, u64) -> Result<String, BoxError>,
    signal: Option<&AbortSignal>,
) -> Result<StreamedImmutableObject, BoxError> {
    let staged = stage(root, data, signal).await?;
    let target = match target_for(&staged.object.sha256, staged.object.bytes) {
        Ok(value) => value,
        Err(cause) => {
            remove_temporary(&staged.path).await?;
            return Err(cause);
        }
    };
    let object = staged.object.clone();
    publish_staged(root, &target, staged).await?;
    Ok(object)
}
pub async fn publish_immutable_alias(
    root: &str,
    source: &str,
    target: &str,
    sha256: &str,
) -> Result<(), BoxError> {
    let result: Result<(), BoxError> = async {
        let parent = dirname(target);
        let boundary = ensure_home(root).await?;
        ensure_durable_directory(&parent, &boundary).await?;
        link_verified(source, target, sha256).await?;
        permissions(target, 0o400).await?;
        sync_publication(root, &parent).await
    }
    .await;
    result.map_err(|cause| {
        if attachment_failure(cause.as_ref()) {
            cause
        } else {
            error("Unable to persist attachment.", Code::AttachmentWriteFailed)
                .with_cause(cause)
                .into()
        }
    })
}
pub async fn commit_prepared_image_file(
    root: &str,
    prepared: &PreparedImageFile,
) -> Result<ImageAttachmentRef, BoxError> {
    let hash = reference_digest(
        prepared.r#ref.attachment_id.as_str(),
        "Attachment reference is invalid.",
    )?;
    let bytes = prepared.data.clone();
    let computed = tokio::task::spawn_blocking(move || digest(&bytes)).await?;
    if computed != hash || prepared.data.len() as u64 != prepared.r#ref.bytes {
        return Err(error(
            "Prepared attachment bytes do not match their reference.",
            Code::AttachmentCorrupt,
        )
        .into());
    }
    publish_immutable_object(
        root,
        &normalized_image_path(root, &prepared.r#ref)?,
        &prepared.data,
        &hash,
    )
    .await?;
    Ok(prepared.r#ref.clone())
}
pub async fn save_image_file(
    root: &str,
    input: &SaveImageAttachment,
    limits: &ImageAttachmentLimits,
    policy: &NormalizationPolicy,
) -> Result<ImageAttachmentRef, BoxError> {
    commit_prepared_image_file(root, &prepare_image_file(input, limits, policy).await?).await
}
pub async fn read_image_file(
    root: &str,
    reference: &ImageAttachmentRef,
    signal: Option<&AbortSignal>,
) -> Result<StoredImageAttachment, BoxError> {
    #[cfg(test)]
    if let Some(signal) = signal {
        let _ = READ_SIGNALS.try_with(|signals| lock(signals).push(signal.clone()));
    }
    check(signal)?;
    let hash = reference_digest(
        reference.attachment_id.as_str(),
        "Attachment reference is invalid.",
    )?;
    let path = normalized_image_path(root, reference)?;
    let read = cancellable(signal, tokio::fs::read(&path)).await;
    let data = match read {
        Ok(data) => data,
        Err(cause) => {
            check(signal)?;
            if cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            {
                return Err(
                    error("Attachment object is missing.", Code::AttachmentNotFound).into(),
                );
            }
            return Err(error(
                "Unable to read image attachment.",
                Code::AttachmentReadFailed,
            )
            .with_cause(node_io_cause(cause, "open", &path))
            .into());
        }
    };
    check(signal)?;
    let copy = data.clone();
    let actual = tokio::task::spawn_blocking(move || digest(&copy)).await?;
    if actual != hash {
        return Err(error(
            "Stored attachment failed integrity verification.",
            Code::AttachmentCorrupt,
        )
        .into());
    }
    let metadata = probe_image(&data).await?;
    check(signal)?;
    if metadata.media_type != reference.media_type
        || data.len() as u64 != reference.bytes
        || metadata.width != reference.width
        || metadata.height != reference.height
    {
        return Err(error(
            "Stored attachment metadata does not match its reference.",
            Code::AttachmentCorrupt,
        )
        .into());
    }
    Ok(StoredImageAttachment {
        r#ref: reference.clone(),
        data,
    })
}
