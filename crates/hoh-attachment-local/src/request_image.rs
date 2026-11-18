//! Deterministic request transforms and a replaceable, header-verified cache.
use crate::{
    encoding::{
        EncodedImage, IMAGE_ENCODING_QUALITIES, WEBP_ENCODING_EFFORT, encode_first_within_limit,
        encoding_ladder,
    },
    image::{
        decode_srgb, detect_image, encoded_alpha_is_compatible, probe_image, resize_for_target,
    },
    store::create_private_directory,
    util::*,
};
use hoh_attachment::{
    AttachmentErrorCode as Code, ImageAttachmentRef, ImageRequestTarget, ImageVariantId,
    RequestImageAttachment, RequestImageDepth, RequestImageSpace, StoredImageAttachment,
};
use hoh_error::BoxError;
use hoh_timeout::AbortSignal;
use hoh_util_values::js_json_stringify;
use hoh_util_workspace_path::node_path::{dirname, join};
use serde_json::json;

pub const REQUEST_IMAGE_TRANSFORM_VERSION: &str = "request-image-v6";
pub fn request_image_variant_id(
    attachment: &ImageAttachmentRef,
    target: &ImageRequestTarget,
) -> ImageVariantId {
    let descriptor = json!({
        "transformVersion":REQUEST_IMAGE_TRANSFORM_VERSION,
        "attachmentId":attachment.attachment_id,
        "targetWidth":target.width,"targetHeight":target.height,"encodedByteBudget":target.max_bytes,
        "encoding":{"webpQualities":IMAGE_ENCODING_QUALITIES,"webpEffort":WEBP_ENCODING_EFFORT,"jpegQualities":IMAGE_ENCODING_QUALITIES,"order":["alpha:webp","opaque:jpeg"],"colourspace":"srgb"}
    });
    format!(
        "sha256:{}",
        digest(js_json_stringify(&descriptor).as_bytes())
    )
    .into()
}
fn validate_target(target: &ImageRequestTarget) -> Result<(), BoxError> {
    for (value, name) in [
        (target.width, "Image request width"),
        (target.height, "Image request height"),
        (target.max_bytes, "Image request maxBytes"),
    ] {
        if value == 0 || value > 9_007_199_254_740_991 {
            return Err(error(
                format!("{name} must be a positive integer."),
                Code::InvalidAttachmentRef,
            )
            .into());
        }
    }
    Ok(())
}
#[derive(Clone)]
struct Verified {
    image: EncodedImage,
    has_alpha: bool,
}
pub(crate) async fn verify_request_image(
    image: EncodedImage,
    expected_alpha: bool,
) -> Result<(EncodedImage, bool), BoxError> {
    let detected = detect_image(&image.data, None).await?;
    if detected.depth != "uchar"
        || detected.space != "srgb"
        || detected.width != image.width
        || detected.height != image.height
        || detected.media_type != image.media_type
        || !encoded_alpha_is_compatible(Some(expected_alpha), &detected)
    {
        return Err(error(
            "Encoded model-request image does not match its verified 8-bit sRGB metadata.",
            Code::AttachmentWriteFailed,
        )
        .into());
    }
    Ok((image, detected.has_alpha))
}
async fn read_cached(
    path: &str,
    target: &ImageRequestTarget,
    alpha: bool,
    signal: Option<&AbortSignal>,
) -> Result<Option<Verified>, BoxError> {
    let result: Result<Option<Verified>, BoxError> = async {
        let data = cancellable(signal, tokio::fs::read(path)).await?;
        let detected = probe_image(&data).await?;
        if detected.depth != "uchar"
            || detected.space != "srgb"
            || detected.width > target.width
            || detected.height > target.height
            || !encoded_alpha_is_compatible(Some(alpha), &detected)
        {
            return Ok(None);
        }
        Ok(Some(Verified {
            image: EncodedImage {
                data,
                media_type: detected.media_type,
                width: detected.width,
                height: detected.height,
            },
            has_alpha: detected.has_alpha,
        }))
    }
    .await;
    match result {
        Ok(value) => Ok(value),
        Err(_) => {
            check(signal)?;
            Ok(None)
        }
    }
}
async fn write_cached(path: &str, data: &[u8]) -> Result<(), BoxError> {
    create_private_directory(&dirname(path)).await?;
    let temporary = format!("{path}.{}.tmp", uuid()?);
    let result: Result<(), BoxError> = async {
        let mut options = tokio::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&temporary)
            .await
            .map_err(|e| fs_error(e, "open", &temporary, None))?;
        use tokio::io::AsyncWriteExt;
        file.write_all(data)
            .await
            .map_err(|e| fs_error(e, "write", &temporary, None))?;
        file.flush()
            .await
            .map_err(|e| fs_error(e, "write", &temporary, None))?;
        drop(file);
        tokio::fs::rename(&temporary, path)
            .await
            .map_err(|e| fs_error(e, "rename", &temporary, Some(path)))?;
        Ok(())
    }
    .await;
    match tokio::fs::remove_file(&temporary).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(fs_error(e, "unlink", &temporary, None)),
    }
    result
}
pub async fn read_request_image_file(
    root: &str,
    attachment: &StoredImageAttachment,
    target: &ImageRequestTarget,
    signal: Option<&AbortSignal>,
) -> Result<RequestImageAttachment, BoxError> {
    check(signal)?;
    validate_target(target)?;
    let source = probe_image(&attachment.data).await?;
    let variant_id = request_image_variant_id(&attachment.r#ref, target);
    let hash = &variant_id.as_str()[7..];
    let path = join(&[root, "request-images", &hash[..2], hash]);
    let cached = read_cached(&path, target, source.has_alpha, signal).await?;
    let mut write_cache = false;
    let version = if let Some(cached) = cached {
        cached
    } else if target.width >= attachment.r#ref.width
        && target.height >= attachment.r#ref.height
        && attachment.data.len() as u64 <= target.max_bytes
    {
        Verified {
            image: EncodedImage {
                data: attachment.data.clone(),
                media_type: attachment.r#ref.media_type,
                width: attachment.r#ref.width,
                height: attachment.r#ref.height,
            },
            has_alpha: source.has_alpha,
        }
    } else {
        let data = attachment.data.clone();
        let reference = attachment.r#ref.clone();
        let target = *target;
        let prepared = tokio::task::spawn_blocking(move || {
            let pixels = decode_srgb(&data, false)?;
            let (width, height) = if reference.width >= reference.height {
                (Some(target.width), None)
            } else {
                (None, Some(target.height))
            };
            Ok::<_, BoxError>(resize_for_target(
                pixels,
                width,
                height,
                reference.media_type,
            ))
        })
        .await??;
        let chosen = encode_first_within_limit(
            encoding_ladder(prepared, source.has_alpha),
            target.max_bytes,
        )
        .await?
        .into_chosen();
        let (image, has_alpha) = verify_request_image(chosen, source.has_alpha).await?;
        write_cache = true;
        Verified { image, has_alpha }
    };
    check(signal)?;
    if write_cache {
        write_cached(&path, &version.image.data).await?;
    }
    Ok(RequestImageAttachment {
        variant_id,
        attachment: attachment.r#ref.clone(),
        bytes: version.image.data.len() as u64,
        data: version.image.data,
        media_type: version.image.media_type,
        width: version.image.width,
        height: version.image.height,
        depth: RequestImageDepth::Uchar,
        space: RequestImageSpace::Srgb,
        has_alpha: version.has_alpha,
    })
}
