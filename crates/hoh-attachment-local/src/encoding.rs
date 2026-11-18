//! Shared lazy quality ladder for normalization and request projections.
use ::image::{DynamicImage, ImageEncoder};
use futures_util::future::BoxFuture;
use hoh_attachment::ImageMediaType;
use hoh_error::BoxError;
use std::sync::Arc;

pub const IMAGE_ENCODING_QUALITIES: [u8; 3] = [85, 75, 60];
pub const WEBP_ENCODING_EFFORT: i32 = 0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedImage {
    pub data: Vec<u8>,
    pub media_type: ImageMediaType,
    pub width: u64,
    pub height: u64,
}
pub trait EncodedCandidate {
    fn data(&self) -> &[u8];
}
impl EncodedCandidate for EncodedImage {
    fn data(&self) -> &[u8] {
        &self.data
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExhaustedEncoding<T> {
    pub smallest: T,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EncodingResult<T> {
    Fitting(T),
    Exhausted(ExhaustedEncoding<T>),
}
impl<T> EncodingResult<T> {
    pub fn into_chosen(self) -> T {
        match self {
            Self::Fitting(value) => value,
            Self::Exhausted(value) => value.smallest,
        }
    }
}
pub type EncodingAttempt<'a, T> =
    Box<dyn FnOnce() -> BoxFuture<'a, Result<T, BoxError>> + Send + 'a>;
#[derive(Debug, thiserror::Error)]
#[error("image encoding requires at least one candidate")]
pub struct EmptyEncodingCandidates;

pub async fn encode_first_within_limit<T: EncodedCandidate>(
    attempts: Vec<EncodingAttempt<'_, T>>,
    max_bytes: u64,
) -> Result<EncodingResult<T>, BoxError> {
    let mut attempts = attempts.into_iter();
    let first = attempts.next().ok_or(EmptyEncodingCandidates)?;
    let mut smallest = first().await?;
    if smallest.data().len() as u64 <= max_bytes {
        return Ok(EncodingResult::Fitting(smallest));
    }
    for attempt in attempts {
        let candidate = attempt().await?;
        if candidate.data().len() as u64 <= max_bytes {
            return Ok(EncodingResult::Fitting(candidate));
        }
        if candidate.data().len() < smallest.data().len() {
            smallest = candidate;
        }
    }
    Ok(EncodingResult::Exhausted(ExhaustedEncoding { smallest }))
}
pub fn is_exhausted_encoding<T>(result: &EncodingResult<T>) -> bool {
    matches!(result, EncodingResult::Exhausted(_))
}
pub fn encoding_ladder(
    prepared: DynamicImage,
    has_alpha: bool,
) -> Vec<EncodingAttempt<'static, EncodedImage>> {
    let prepared = Arc::new(prepared);
    IMAGE_ENCODING_QUALITIES
        .into_iter()
        .map(|quality| {
            let pixels = prepared.clone();
            Box::new(move || {
                Box::pin(async move {
                    tokio::task::spawn_blocking(move || encode(&pixels, has_alpha, quality)).await?
                }) as BoxFuture<'static, Result<EncodedImage, BoxError>>
            }) as EncodingAttempt<'static, EncodedImage>
        })
        .collect()
}
fn encode(pixels: &DynamicImage, has_alpha: bool, quality: u8) -> Result<EncodedImage, BoxError> {
    let width = pixels.width();
    let height = pixels.height();
    let (data, media_type) = if has_alpha {
        let pixels = pixels.to_rgba8();
        let encoder = webp::Encoder::from_rgba(&pixels, width, height);
        let mut config = webp::WebPConfig::new()
            .map_err(|_| std::io::Error::other("Unable to initialize WebP encoder."))?;
        config.quality = f32::from(quality);
        config.method = WEBP_ENCODING_EFFORT;
        let data = encoder
            .encode_advanced(&config)
            .map_err(|e| std::io::Error::other(format!("{e:?}")))?;
        (data.to_vec(), ImageMediaType::Webp)
    } else {
        let pixels = pixels.to_rgb8();
        let mut data = Vec::new();
        ::image::codecs::jpeg::JpegEncoder::new_with_quality(&mut data, quality).write_image(
            &pixels,
            width,
            height,
            ::image::ExtendedColorType::Rgb8,
        )?;
        (data, ImageMediaType::Jpeg)
    };
    Ok(EncodedImage {
        data,
        media_type,
        width: u64::from(width),
        height: u64::from(height),
    })
}
