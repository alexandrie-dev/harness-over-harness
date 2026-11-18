use crate::*;
use ::image::{DynamicImage, ImageEncoder, ImageFormat};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{StreamExt, stream::BoxStream};
use hoh_error::{BoxError, HarnessError};
use std::io::Cursor;

pub fn png() -> Vec<u8> {
    STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAACXBIWXMAAAPoAAAD6AG1e1JrAAAADElEQVQImWNgZGIGAAAOAAeCcsnOAAAAAElFTkSuQmCC").unwrap()
}
pub fn policy() -> NormalizationPolicy {
    NormalizationPolicy {
        max_pixels: 2048 * 2048,
        max_dimension: 8192,
        max_bytes: 4 * 1024 * 1024,
    }
}
pub fn limits() -> ImageAttachmentLimits {
    ImageAttachmentLimits {
        max_image_bytes: 1024,
        max_images_per_message: 2,
        max_message_image_bytes: 2048,
        max_image_pixels: 16,
        max_image_dimension: 2000,
        media_types: vec![
            ImageMediaType::Png,
            ImageMediaType::Jpeg,
            ImageMediaType::Webp,
            ImageMediaType::Gif,
        ],
    }
}
pub fn input(data: Vec<u8>) -> SaveImageAttachment {
    SaveImageAttachment {
        data,
        media_type: ImageMediaType::Png,
        name: None,
    }
}
pub fn root(home: &tempfile::TempDir) -> String {
    hoh_util_workspace_path::node_path::join(&[&home.path().to_string_lossy(), "attachments", "v1"])
}
pub fn assert_code<T>(result: Result<T, BoxError>, expected: &str) -> BoxError {
    let failure = match result {
        Ok(_) => panic!("expected {expected}"),
        Err(e) => e,
    };
    assert_eq!(
        HarnessError::of(failure.as_ref()).map(HarnessError::code),
        Some(expected),
        "{failure}"
    );
    failure
}
pub fn sha256(data: &[u8]) -> String {
    crate::util::digest(data)
}
pub async fn encode(image: DynamicImage, format: ImageFormat) -> Vec<u8> {
    tokio::task::spawn_blocking(move || {
        let mut data = Cursor::new(vec![]);
        image.write_to(&mut data, format).unwrap();
        data.into_inner()
    })
    .await
    .unwrap()
}
pub async fn flat(width: u32, height: u32, format: ImageFormat, alpha: bool) -> Vec<u8> {
    let image = if alpha {
        DynamicImage::ImageRgba8(::image::RgbaImage::from_pixel(
            width,
            height,
            ::image::Rgba([12, 200, 64, 128]),
        ))
    } else {
        DynamicImage::ImageRgb8(::image::RgbImage::from_pixel(
            width,
            height,
            ::image::Rgb([12, 200, 64]),
        ))
    };
    encode(image, format).await
}
pub fn noise(width: u32, height: u32) -> Vec<u8> {
    let mut data = vec![0; (width * height * 3) as usize];
    let mut state = 0x2545f491u32;
    for byte in &mut data {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *byte = state as u8;
    }
    data
}
pub async fn noisy(width: u32, height: u32, format: ImageFormat, alpha: Option<bool>) -> Vec<u8> {
    let rgb = noise(width, height);
    let pixels = if let Some(transparent) = alpha {
        let rgba: Vec<u8> = rgb
            .chunks_exact(3)
            .enumerate()
            .flat_map(|(i, p)| [p[0], p[1], p[2], if transparent { i as u8 } else { 255 }])
            .collect();
        DynamicImage::ImageRgba8(::image::RgbaImage::from_raw(width, height, rgba).unwrap())
    } else {
        DynamicImage::ImageRgb8(::image::RgbImage::from_raw(width, height, rgb).unwrap())
    };
    encode(pixels, format).await
}
pub async fn png16(width: u32, height: u32, alpha: bool) -> Vec<u8> {
    let image = if alpha {
        DynamicImage::ImageRgba16(::image::ImageBuffer::from_pixel(
            width,
            height,
            ::image::Rgba([12u16 * 257, 34 * 257, 56 * 257, 32768]),
        ))
    } else {
        DynamicImage::ImageRgb16(::image::ImageBuffer::from_pixel(
            width,
            height,
            ::image::Rgb([12u16 * 257, 34 * 257, 56 * 257]),
        ))
    };
    encode(image, ImageFormat::Png).await
}
pub async fn oriented(width: u32, height: u32, orientation: u16) -> Vec<u8> {
    tokio::task::spawn_blocking(move || {
        let pixels = ::image::RgbImage::from_pixel(width, height, ::image::Rgb([1, 2, 3]));
        let mut data = vec![];
        let mut encoder = ::image::codecs::jpeg::JpegEncoder::new_with_quality(&mut data, 85);
        let mut exif = b"II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
        exif.extend_from_slice(&orientation.to_le_bytes());
        exif.extend_from_slice(&[0; 6]);
        encoder.set_exif_metadata(exif).unwrap();
        encoder
            .write_image(&pixels, width, height, ::image::ExtendedColorType::Rgb8)
            .unwrap();
        data
    })
    .await
    .unwrap()
}
pub async fn profiled_png() -> Vec<u8> {
    tokio::task::spawn_blocking(|| {
        let pixels = ::image::RgbImage::from_pixel(4, 2, ::image::Rgb([100, 160, 50]));
        let mut data = vec![];
        let mut encoder = ::image::codecs::png::PngEncoder::new(&mut data);
        encoder
            .set_icc_profile(moxcms::ColorProfile::new_display_p3().encode().unwrap())
            .unwrap();
        encoder
            .write_image(&pixels, 4, 2, ::image::ExtendedColorType::Rgb8)
            .unwrap();
        data
    })
    .await
    .unwrap()
}
pub async fn collect(
    mut stream: BoxStream<'_, Result<Vec<u8>, BoxError>>,
) -> Result<Vec<u8>, BoxError> {
    let mut data = vec![];
    while let Some(chunk) = stream.next().await {
        data.extend(chunk?);
    }
    Ok(data)
}
pub fn local(home: &tempfile::TempDir) -> (hoh_kernel::Kernel, LocalAttachmentStore) {
    let kernel = hoh_kernel::Kernel::start();
    let service = LocalAttachmentStore::new(
        kernel.root(),
        Config {
            dsh_home: Some(home.path().to_string_lossy().into_owned()),
            ..Config::default()
        },
    )
    .unwrap();
    (kernel, service)
}

#[derive(Debug, thiserror::Error)]
#[error("Attachment request cancelled with a non-Error reason.")]
pub struct NonErrorCancellationReason {
    pub cause: serde_json::Value,
}

pub fn cmyk_jpeg() -> Vec<u8> {
    fn segment(out: &mut Vec<u8>, marker: u8, data: &[u8]) {
        out.extend([255, marker]);
        out.extend(((data.len() + 2) as u16).to_be_bytes());
        out.extend(data);
    }
    let mut data = vec![255, 216];
    segment(&mut data, 0xee, b"Adobe\0\x64\0\0\0\0\0");
    let mut quant = vec![0];
    quant.extend([1; 64]);
    segment(&mut data, 0xdb, &quant);
    segment(
        &mut data,
        0xc0,
        &[
            8, 0, 8, 0, 16, 4, 1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0, 4, 0x11, 0,
        ],
    );
    let mut huffman = vec![0, 1];
    huffman.extend([0; 15]);
    huffman.push(0);
    huffman.extend([0x10, 1]);
    huffman.extend([0; 15]);
    huffman.push(0);
    segment(&mut data, 0xc4, &huffman);
    segment(&mut data, 0xda, &[4, 1, 0, 2, 0, 3, 0, 4, 0, 0, 63, 0]);
    data.extend([0, 0, 255, 217]);
    data
}
