//! Provider-independent, metadata-free, oriented normalization.
use crate::{
    encoding::{encode_first_within_limit, encoding_ladder},
    image::{
        DetectedImage, attachment_failure, decode_srgb, detect_image, encoded_alpha_is_compatible,
        resize_srgb,
    },
    util::error,
};
use hoh_attachment::{AttachmentErrorCode as Code, ImageMediaType, request_image_dimensions};
use hoh_error::BoxError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizationPolicy {
    pub max_pixels: u64,
    pub max_dimension: u64,
    pub max_bytes: u64,
}
pub type NormalizedImage = crate::encoding::EncodedImage;

pub fn can_pass_through_normalization(
    detected: &DetectedImage,
    bytes: u64,
    policy: &NormalizationPolicy,
) -> bool {
    detected.media_type != ImageMediaType::Gif
        && !detected.animated
        && !detected.carries_metadata
        && detected.depth == "uchar"
        && detected.space == "srgb"
        && bytes <= policy.max_bytes
        && detected.width.saturating_mul(detected.height) <= policy.max_pixels
        && detected.width.max(detected.height) <= policy.max_dimension
}
pub(crate) async fn verify_normalized_image(
    image: NormalizedImage,
    expected_alpha: Option<bool>,
) -> Result<NormalizedImage, BoxError> {
    let detected = detect_image(&image.data, None).await?;
    if detected.media_type != image.media_type
        || detected.width != image.width
        || detected.height != image.height
        || detected.animated
        || detected.carries_metadata
        || detected.depth != "uchar"
        || detected.space != "srgb"
        || !encoded_alpha_is_compatible(expected_alpha, &detected)
    {
        return Err(error("Image normalization did not produce a single-frame 8-bit sRGB image with matching metadata.",Code::AttachmentWriteFailed).into());
    }
    Ok(image)
}
pub async fn normalize_image(
    data: &[u8],
    detected: &DetectedImage,
    policy: &NormalizationPolicy,
) -> Result<NormalizedImage, BoxError> {
    if can_pass_through_normalization(detected, data.len() as u64, policy) {
        return Ok(NormalizedImage {
            data: data.to_vec(),
            media_type: detected.media_type,
            width: detected.width,
            height: detected.height,
        });
    }
    let result: Result<NormalizedImage, BoxError> = async {
        let mut dimensions =
            request_image_dimensions(detected.width, detected.height, policy.max_pixels);
        let long_edge = dimensions.width.max(dimensions.height);
        if long_edge > policy.max_dimension {
            let scale = policy.max_dimension as f64 / long_edge as f64;
            dimensions.width = ((dimensions.width as f64 * scale).floor() as u64).max(1);
            dimensions.height = ((dimensions.height as f64 * scale).floor() as u64).max(1);
        }
        let data = data.to_vec();
        let prepared = tokio::task::spawn_blocking(move || {
            let pixels = decode_srgb(&data, true).map_err(std::io::Error::other)?;
            Ok::<_, BoxError>(resize_srgb(
                pixels,
                u32::try_from(dimensions.width)?,
                u32::try_from(dimensions.height)?,
                true,
            ))
        })
        .await??;
        let chosen = encode_first_within_limit(
            encoding_ladder(prepared, detected.has_alpha),
            policy.max_bytes,
        )
        .await?
        .into_chosen();
        verify_normalized_image(
            chosen,
            if detected.media_type == ImageMediaType::Gif {
                None
            } else {
                Some(detected.has_alpha)
            },
        )
        .await
    }
    .await;
    result.map_err(|cause| {
        if attachment_failure(cause.as_ref()) {
            return cause;
        }
        let source = if detected.media_type == ImageMediaType::Png && detected.depth != "uchar" {
            format!(
                "{} PNG",
                if detected.depth == "ushort" {
                    "16-bit"
                } else {
                    &detected.depth
                }
            )
        } else {
            format!(
                "{} {}",
                detected.depth,
                detected.media_type.as_str()[6..].to_ascii_uppercase()
            )
        };
        error(
            format!("The {source} could not be converted to the normalized 8-bit sRGB form."),
            Code::AttachmentWriteFailed,
        )
        .with_cause(cause)
        .into()
    })
}
