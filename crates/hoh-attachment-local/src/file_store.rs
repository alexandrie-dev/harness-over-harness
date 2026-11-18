//! Verbatim files: one canonical inode per digest, named read-only aliases, bounded reads.
use crate::{
    store::{
        publish_immutable_alias, publish_immutable_object, publish_immutable_object_stream,
        reference_digest,
    },
    util::*,
};
use futures_util::stream::BoxStream;
use hoh_attachment::{
    AttachmentErrorCode as Code, FileAttachmentRef, SaveFileAttachment, SaveFileStreamAttachment,
};
use hoh_error::{BoxError, HarnessError};
use hoh_timeout::AbortSignal;
use hoh_util_values::js_trim;
use hoh_util_workspace_path::node_path::join;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

pub fn file_leaf_name(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "file".into();
    };
    let leaf = value.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = leaf
        .chars()
        .filter(|c| !matches!(*c, '\u{0}'..='\u{1f}' | '\u{7f}'))
        .map(|c| if "<>:\"|?*".contains(c) { '_' } else { c })
        .collect();
    let mut clean = js_trim(&clean).trim_end_matches(['.', ' ']).to_owned();
    let stem = clean
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(['.', ' '])
        .to_ascii_lowercase();
    let device = matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
        || (stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'));
    if device {
        clean.insert(0, '_');
    }
    let mut end = clean.len().min(255);
    while !clean.is_char_boundary(end) {
        end -= 1;
    }
    clean.truncate(end);
    let clean = clean.trim_end_matches(['.', ' ']);
    if clean.is_empty() || matches!(clean, "." | "..") {
        "file".into()
    } else {
        clean.into()
    }
}
fn ensure_reference(reference: &FileAttachmentRef) -> Result<String, HarnessError> {
    let hash = reference_digest(
        reference.attachment_id.as_str(),
        "File attachment reference is invalid.",
    )?;
    if reference.name != file_leaf_name(Some(&reference.name)) {
        return Err(error(
            "File attachment reference is invalid.",
            Code::InvalidAttachmentRef,
        ));
    }
    Ok(hash)
}
pub fn stored_file_path(root: &str, reference: &FileAttachmentRef) -> Result<String, HarnessError> {
    let hash = ensure_reference(reference)?;
    Ok(join(&[root, "files", &hash[..2], &hash, &reference.name]))
}
fn object_path(root: &str, hash: &str) -> String {
    join(&[root, "file-objects", &hash[..2], hash])
}
pub async fn save_file_verbatim(
    root: &str,
    input: &SaveFileAttachment,
) -> Result<FileAttachmentRef, BoxError> {
    let data = input.data.clone();
    let hash = tokio::task::spawn_blocking(move || digest(&data)).await?;
    let reference = FileAttachmentRef {
        attachment_id: format!("sha256:{hash}").into(),
        name: file_leaf_name(input.name.as_deref()),
        bytes: input.data.len() as u64,
    };
    let object = object_path(root, &hash);
    publish_immutable_object(root, &object, &input.data, &hash).await?;
    publish_immutable_alias(root, &object, &stored_file_path(root, &reference)?, &hash).await?;
    Ok(reference)
}
pub async fn save_file_stream_verbatim(
    root: &str,
    input: SaveFileStreamAttachment<'_>,
) -> Result<FileAttachmentRef, BoxError> {
    let name = file_leaf_name(input.name.as_deref());
    let stored = publish_immutable_object_stream(
        root,
        input.data,
        |hash, _| Ok(object_path(root, hash)),
        input.signal.as_ref(),
    )
    .await?;
    let reference = FileAttachmentRef {
        attachment_id: format!("sha256:{}", stored.sha256).into(),
        name,
        bytes: stored.bytes,
    };
    check(input.signal.as_ref())?;
    publish_immutable_alias(
        root,
        &object_path(root, &stored.sha256),
        &stored_file_path(root, &reference)?,
        &stored.sha256,
    )
    .await?;
    check(input.signal.as_ref())?;
    Ok(reference)
}
struct ReadState {
    root: String,
    reference: FileAttachmentRef,
    file: Option<tokio::fs::File>,
    signal: Option<AbortSignal>,
    hash: Sha256,
    bytes: u64,
}
pub fn read_file_stream_verbatim<'a>(
    root: &'a str,
    reference: &'a FileAttachmentRef,
    signal: Option<&'a AbortSignal>,
) -> BoxStream<'a, Result<Vec<u8>, BoxError>> {
    let state = ReadState {
        root: root.into(),
        reference: reference.clone(),
        file: None,
        signal: signal.cloned(),
        hash: Sha256::new(),
        bytes: 0,
    };
    Box::pin(futures_util::stream::try_unfold(
        state,
        |mut state| async move {
            check(state.signal.as_ref())?;
            let expected = ensure_reference(&state.reference)?;
            let path = stored_file_path(&state.root, &state.reference)?;
            if state.file.is_none() {
                state.file = Some(
                    match cancellable(state.signal.as_ref(), tokio::fs::File::open(&path)).await {
                        Ok(file) => file,
                        Err(cause) => {
                            check(state.signal.as_ref())?;
                            if cause
                                .downcast_ref::<std::io::Error>()
                                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
                            {
                                return Err(error(
                                    "File attachment object is missing.",
                                    Code::AttachmentNotFound,
                                )
                                .into());
                            }
                            return Err(error(
                                "Unable to read file attachment.",
                                Code::AttachmentReadFailed,
                            )
                            .with_cause(node_io_cause(cause, "open", &path))
                            .into());
                        }
                    },
                );
            }
            let mut buffer = vec![0; 65536];
            // Initialization above establishes an open descriptor; an Option match also handles it without a panic.
            let length = match state.file.as_mut() {
                Some(file) => cancellable(state.signal.as_ref(), file.read(&mut buffer)).await,
                None => Err(std::io::Error::other("File descriptor is unavailable.").into()),
            };
            let length = match length {
                Ok(length) => length,
                Err(cause) => {
                    check(state.signal.as_ref())?;
                    return Err(error(
                        "Unable to read file attachment.",
                        Code::AttachmentReadFailed,
                    )
                    .with_cause(node_io_cause(cause, "read", &path))
                    .into());
                }
            };
            check(state.signal.as_ref())?;
            if length == 0 {
                if state.bytes != state.reference.bytes || hex(&state.hash.finalize()) != expected {
                    return Err(error(
                        "Stored file attachment failed integrity verification.",
                        Code::AttachmentCorrupt,
                    )
                    .into());
                }
                return Ok(None);
            }
            buffer.truncate(length);
            state.hash.update(&buffer);
            state.bytes += length as u64;
            Ok(Some((buffer, state)))
        },
    ))
}
